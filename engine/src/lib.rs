pub mod discovery;
pub mod model;
pub mod network;
pub mod pairing;
pub mod phone;
mod pipeline;
pub mod storage;
pub mod transport;
pub mod web;

use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post, put},
};
use model::{Entry, PairRequest, Session, Transfer, manifest_id, now};
use pairing::{Pairing, secret};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::Arc};
use storage::{Store, hash_file, validate};
use tokio::sync::Mutex;
use transport::{Connection, PairReply, ProofReply};

pub type Shared = Arc<Engine>;
pub type ApiResult<T> = std::result::Result<T, ApiError>;
pub fn check(condition: bool, message: &str) -> Result<()> {
    ensure!(condition, "{message}");
    Ok(())
}
pub struct ApiError(pub anyhow::Error);
impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}
impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": self.0.to_string()})),
        )
            .into_response()
    }
}

pub struct Engine {
    pub name: String,
    pub fingerprint: String,
    pub pairing: Mutex<Pairing>,
    pub sessions: Mutex<HashMap<String, Session>>,
    pub connections: Mutex<HashMap<String, Connection>>,
    pub owners: Mutex<HashMap<String, String>>,
    pub store: Store,
    pub writes: Mutex<()>,
    pub sending: Mutex<()>,
    pub uploads: Mutex<HashMap<String, Vec<Upload>>>,
    pub ui_token: String,
    pub discovery: std::sync::Mutex<Option<discovery::Discovery>>,
    pub discovery_error: std::sync::Mutex<Option<String>>,
    pub last_error: Mutex<Option<String>>,
    pub send_controls: std::sync::Mutex<HashMap<String, String>>,
    pub phone: Mutex<Option<Arc<phone::Hub>>>,
    pub native_sources: Mutex<HashMap<String, Vec<PathBuf>>>,
    pub native_token: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Upload {
    pub path: String,
    pub size: u64,
}

pub struct Running {
    pub engine: Shared,
    pub address: SocketAddr,
    handle: axum_server::Handle,
    task: tokio::task::JoinHandle<()>,
}

impl Running {
    pub fn shutdown(&self) {
        self.handle.shutdown();
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.handle.shutdown();
        self.task.abort();
    }
}

pub async fn start(
    root: PathBuf,
    name: String,
    host: String,
    bind: SocketAddr,
    discover: bool,
) -> Result<Running> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    ensure!(
        !name.trim().is_empty() && name.len() <= 100,
        "Device name must be 1–100 bytes"
    );
    let certified = rcgen::generate_simple_self_signed(vec![
        host.clone(),
        "localhost".into(),
        "127.0.0.1".into(),
    ])?;
    let fingerprint = hex::encode(Sha256::digest(certified.cert.der()));
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        certified.cert.pem().into_bytes(),
        certified.key_pair.serialize_pem().into_bytes(),
    )
    .await?;
    let listener = std::net::TcpListener::bind(bind)?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let engine = Arc::new(Engine {
        name,
        fingerprint,
        pairing: Mutex::new(Pairing::new(host.clone(), address.port())),
        sessions: Mutex::new(HashMap::new()),
        connections: Mutex::new(HashMap::new()),
        owners: Mutex::new(HashMap::new()),
        store: Store::open(root)?,
        writes: Mutex::new(()),
        sending: Mutex::new(()),
        uploads: Mutex::new(HashMap::new()),
        ui_token: secret(32),
        discovery: std::sync::Mutex::new(None),
        discovery_error: std::sync::Mutex::new(None),
        last_error: Mutex::new(None),
        send_controls: std::sync::Mutex::new(HashMap::new()),
        phone: Mutex::new(None),
        native_sources: Mutex::new(HashMap::new()),
        native_token: std::env::var("DRIFT_NATIVE_TOKEN")
            .ok()
            .filter(|value| value.len() == 64),
    });
    if discover {
        match discovery::Discovery::start(
            &uuid::Uuid::new_v4().to_string(),
            &engine.name,
            &host,
            address.port(),
        ) {
            Ok(d) => *engine.discovery.lock().unwrap() = Some(d),
            Err(e) => *engine.discovery_error.lock().unwrap() = Some(e.to_string()),
        }
    }
    let router = Router::new()
        .route("/proof", get(challenge))
        .route("/pair", post(pair))
        .route("/session", get(session_status))
        .route("/confirm", post(confirm))
        .route("/offer", post(offer))
        .route("/features", get(features))
        .route("/transfer/{id}", get(transfer_status))
        .route("/transfer/{id}/finish", post(finish))
        .route("/transfer/{id}/control", post(peer_control))
        .route(
            "/transfer/{id}/{index}",
            put(chunk).layer(DefaultBodyLimit::max(model::CHUNK)),
        )
        .route(
            "/transfer/{id}/{index}/window",
            put(window_chunk)
                .layer::<_, std::convert::Infallible>(DefaultBodyLimit::max(model::WINDOW))
                .layer(tower::limit::ConcurrencyLimitLayer::new(1)),
        )
        .layer(DefaultBodyLimit::max(model::MAX_METADATA))
        .layer(tower::limit::ConcurrencyLimitLayer::new(4))
        .with_state(engine.clone());
    let handle = axum_server::Handle::new();
    let server = axum_server::from_tcp_rustls(listener, tls).handle(handle.clone());
    let task = tokio::spawn(async move {
        if let Err(e) = server.serve(router.into_make_service()).await {
            eprintln!("TLS listener stopped: {e}");
        }
    });
    Ok(Running {
        engine,
        address,
        handle,
        task,
    })
}

#[derive(Deserialize)]
struct Challenge {
    nonce: String,
}
async fn challenge(
    State(engine): State<Shared>,
    Query(query): Query<Challenge>,
) -> ApiResult<Json<ProofReply>> {
    let proof = engine
        .pairing
        .lock()
        .await
        .challenge(&engine.fingerprint, &query.nonce)?;
    Ok(Json(ProofReply { proof }))
}
async fn pair(
    State(engine): State<Shared>,
    Json(request): Json<PairRequest>,
) -> ApiResult<Json<PairReply>> {
    let mut sessions = engine.sessions.lock().await;
    check(
        sessions.len() < 64,
        "Too many sessions; restart the app to clear sessions",
    )?;
    engine
        .pairing
        .lock()
        .await
        .consume(&engine.fingerprint, &request)?;
    let capability = secret(32);
    let session = Session {
        id: uuid::Uuid::new_v4().to_string(),
        name: request.name,
        approved: false,
        confirmed: false,
        created: now(),
        capability: capability.clone(),
    };
    let id = session.id.clone();
    sessions.insert(id.clone(), session);
    Ok(Json(PairReply {
        session: id,
        capability,
        name: engine.name.clone(),
    }))
}

impl Engine {
    pub async fn source_path(&self, batch: &str, index: usize) -> PathBuf {
        self.native_sources
            .lock()
            .await
            .get(batch)
            .and_then(|paths| paths.get(index))
            .cloned()
            .unwrap_or_else(|| {
                self.store
                    .root
                    .join("outbox")
                    .join(batch)
                    .join(index.to_string())
            })
    }
    pub async fn auth(&self, headers: &HeaderMap, approved: bool) -> Result<Session> {
        let token = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .context("Authentication required")?;
        let sessions = self.sessions.lock().await;
        let session = sessions
            .values()
            .find(|s| constant_eq(&s.capability, token))
            .context("Invalid session")?;
        ensure!(
            now().saturating_sub(session.created) < 86400,
            "Session expired; pair again"
        );
        if approved {
            ensure!(
                session.approved && session.confirmed,
                "Approve the connection on both devices first"
            );
        }
        Ok(session.clone())
    }

    pub async fn owned(&self, headers: &HeaderMap, id: &str) -> Result<()> {
        let session = self.auth(headers, true).await?;
        ensure!(
            self.owners.lock().await.get(id) == Some(&session.id),
            "Transfer is not approved for this session"
        );
        Ok(())
    }

    pub async fn receive_approval(&self, id: &str, approve: bool) -> Result<()> {
        let _lock = self.writes.lock().await;
        let mut transfer = self.store.get(id)?;
        ensure!(
            transfer.direction == "receive" && transfer.status != "complete",
            "Transfer cannot be changed"
        );
        transfer.status = if approve { "receiving" } else { "cancelled" }.into();
        transfer.updated = now();
        self.store.save(&transfer)
    }

    pub async fn send(&self, connection_id: &str, batch: &str) -> Result<String> {
        let _permit = self
            .sending
            .try_lock()
            .context("Another send is active. Pause or wait for it to finish.")?;
        let connection = self
            .connections
            .lock()
            .await
            .get(connection_id)
            .cloned()
            .context("Pair the device first")?;
        let uploads = self
            .uploads
            .lock()
            .await
            .get(batch)
            .cloned()
            .context("Select files again; this batch is no longer available")?;
        let mut entries = Vec::new();
        for (index, upload) in uploads.iter().enumerate() {
            let path = self.source_path(batch, index).await;
            ensure!(
                tokio::fs::metadata(&path).await?.len() == upload.size,
                "File has not finished staging"
            );
            entries.push(Entry {
                path: upload.path.clone(),
                size: upload.size,
                sha256: hash_file(&path).await?,
            });
        }
        validate(&entries)?;
        let remote: Transfer = connection.post("/offer", &entries).await?;
        ensure!(
            remote.id == manifest_id(&entries) && remote.entries == entries,
            "Peer returned a different manifest"
        );
        ensure!(
            remote.offsets.len() == entries.len()
                && remote
                    .offsets
                    .iter()
                    .zip(&entries)
                    .all(|(offset, entry)| *offset <= entry.size),
            "Peer returned invalid resume offsets"
        );
        let id = format!("send-{}", remote.id);
        self.send_controls.lock().unwrap().remove(&id);
        let mut local = Transfer {
            id: id.clone(),
            entries,
            offsets: remote.offsets.clone(),
            status: "awaiting approval".into(),
            peer: connection.name.clone(),
            updated: now(),
            error: None,
            direction: "send".into(),
        };
        self.store.save(&local)?;
        let result = self
            .send_inner(&connection, &remote.id, batch, &mut local)
            .await;
        if let Err(ref error) = result {
            let stored = self.store.get(&local.id)?;
            let control = self.send_controls.lock().unwrap().get(&local.id).cloned();
            local.status = if let Some(control) = control {
                control
            } else if ["paused", "cancelled"].contains(&stored.status.as_str()) {
                stored.status
            } else {
                "interrupted".into()
            };
            if local.status == "paused" || local.status == "cancelled" {
                let _ = connection
                    .post::<_, Transfer>(
                        &format!("/transfer/{}/control", remote.id),
                        &serde_json::json!({"action":local.status}),
                    )
                    .await;
            }
            local.error = Some(error.to_string());
            local.updated = now();
            self.store.save(&local)?;
        }
        result?;
        Ok(id)
    }

    async fn send_inner(
        &self,
        connection: &Connection,
        remote_id: &str,
        batch: &str,
        local: &mut Transfer,
    ) -> Result<()> {
        let mut waited = 0;
        loop {
            let remote: Transfer = connection.get(&format!("/transfer/{remote_id}")).await?;
            ensure!(
                remote.status != "cancelled",
                "Receiver declined or cancelled the transfer"
            );
            if remote.status == "complete" {
                local.status = "complete".into();
                self.store.save(local)?;
                return Ok(());
            }
            self.check_send_control(&local.id)?;
            if remote.status == "receiving" {
                ensure!(
                    remote.offsets.len() == local.entries.len()
                        && remote
                            .offsets
                            .iter()
                            .zip(&local.entries)
                            .all(|(offset, entry)| *offset <= entry.size),
                    "Invalid receiver offsets"
                );
                local.offsets = remote.offsets;
                break;
            }
            ensure!(waited < 600, "Receiver approval timed out; retry to resume");
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            waited += 1;
        }
        local.status = "sending".into();
        let window = connection.window_size().await?;
        self.store.save(local)?;
        let mut progress_saved = std::time::Instant::now();
        for index in 0..local.entries.len() {
            let mut reader = pipeline::FilePipeline::new(
                self.source_path(batch, index).await,
                local.offsets[index],
                local.entries[index].size,
                window,
            )?;
            while let Some(chunk) = reader.next().await? {
                self.check_send_control(&local.id)?;
                let offset = chunk.offset;
                ensure!(
                    offset == local.offsets[index],
                    "Source pipeline offset mismatch"
                );
                let bytes = chunk.bytes;
                let count = bytes.len();
                let mut sent = false;
                for attempt in 0..4 {
                    let result = if window > model::CHUNK {
                        connection
                            .window_chunk(remote_id, index, offset, bytes.clone())
                            .await
                    } else {
                        connection
                            .chunk(remote_id, index, offset, bytes.clone())
                            .await
                    };
                    match result {
                        Ok(()) => {
                            sent = true;
                            break;
                        }
                        Err(error) => {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                250 * (1 << attempt),
                            ))
                            .await;
                            if let Ok(remote) = connection
                                .get::<Transfer>(&format!("/transfer/{remote_id}"))
                                .await
                            {
                                ensure!(
                                    remote.status == "receiving",
                                    "Receiver paused or cancelled. Retry after approval."
                                );
                                if remote.offsets.get(index) == Some(&(offset + count as u64)) {
                                    sent = true;
                                    break;
                                }
                            }
                            if attempt == 3 {
                                return Err(
                                    error.context("Connection interrupted. Retry to resume.")
                                );
                            }
                        }
                    }
                }
                ensure!(sent, "Chunk failed");
                local.offsets[index] = offset + count as u64;
                local.updated = now();
                // Receiver ACKs remain fully durable. Sender progress is a UI
                // cache: after a restart/re-pair, the receiver's offsets win.
                // Avoid a second fsync transaction on every network chunk.
                if progress_saved.elapsed() >= std::time::Duration::from_millis(250)
                    || local.offsets[index] == local.entries[index].size
                {
                    self.store.save(local)?;
                    progress_saved = std::time::Instant::now();
                }
            }
            ensure!(
                local.offsets[index] == local.entries[index].size,
                "Source pipeline ended before the declared file size"
            );
        }
        local.status = "verifying".into();
        self.check_send_control(&local.id)?;
        self.store.save(local)?;
        let finished: Transfer = connection
            .post(&format!("/transfer/{remote_id}/finish"), &())
            .await?;
        ensure!(
            finished.id == remote_id && finished.status == "complete",
            "Receiver did not confirm integrity verification"
        );
        local.status = "complete".into();
        local.updated = now();
        self.store.save(local)?;
        Ok(())
    }

    fn check_send_control(&self, id: &str) -> Result<()> {
        if let Some(control) = self.send_controls.lock().unwrap().get(id) {
            anyhow::bail!("Transfer {control}. Retry to resume.");
        }
        let status = self.store.get(id)?.status;
        ensure!(
            status != "paused" && status != "cancelled",
            "Transfer {status}. Retry to resume."
        );
        Ok(())
    }
}

pub fn constant_eq(a: &str, b: &str) -> bool {
    use hmac::{Hmac, Mac};
    let mut first = Hmac::<Sha256>::new_from_slice(b"drift constant time comparison").unwrap();
    first.update(a.as_bytes());
    let mut second = Hmac::<Sha256>::new_from_slice(b"drift constant time comparison").unwrap();
    second.update(b.as_bytes());
    second.verify_slice(&first.finalize().into_bytes()).is_ok()
}

async fn session_status(
    State(engine): State<Shared>,
    headers: HeaderMap,
) -> ApiResult<Json<Session>> {
    Ok(Json(engine.auth(&headers, false).await?))
}
async fn confirm(State(engine): State<Shared>, headers: HeaderMap) -> ApiResult<Json<Session>> {
    let session = engine.auth(&headers, false).await?;
    let mut sessions = engine.sessions.lock().await;
    let session = sessions.get_mut(&session.id).context("Session missing")?;
    check(session.approved, "Approve on the receiving device first")?;
    session.confirmed = true;
    Ok(Json(session.clone()))
}
async fn offer(
    State(engine): State<Shared>,
    headers: HeaderMap,
    Json(entries): Json<Vec<Entry>>,
) -> ApiResult<Json<Transfer>> {
    let session = engine.auth(&headers, true).await?;
    validate(&entries)?;
    let _lock = engine.writes.lock().await;
    let id = manifest_id(&entries);
    let owners = engine.owners.lock().await;
    if let Some(owner) = owners.get(&id) {
        check(
            owner == &session.id,
            "This transfer belongs to another session. Revoke its previous incoming connection in Devices, then retry.",
        )?;
    }
    drop(owners);
    let existing = engine.store.get(&id).ok();
    if existing.is_none() {
        check(
            engine
                .store
                .list()?
                .iter()
                .filter(|transfer| {
                    transfer.direction == "receive"
                        && !["complete", "cancelled"].contains(&transfer.status.as_str())
                })
                .count()
                < 32,
            "Too many pending or active incoming transfers. Cancel unused transfers before offering more.",
        )?;
    }
    let mut transfer = existing.unwrap_or(Transfer {
        id: id.clone(),
        offsets: vec![0; entries.len()],
        entries,
        status: "pending".into(),
        peer: session.name.clone(),
        updated: now(),
        error: None,
        direction: "receive".into(),
    });
    if engine.store.root.join("received").join(&id).exists() {
        transfer.offsets = transfer.entries.iter().map(|entry| entry.size).collect();
    }
    if engine.owners.lock().await.get(&id) != Some(&session.id) || transfer.status != "complete" {
        transfer.status = "pending".into();
        transfer.peer = session.name;
        transfer.updated = now();
    }
    engine.store.save(&transfer)?;
    engine.owners.lock().await.insert(id, session.id);
    Ok(Json(transfer))
}
async fn transfer_status(
    State(engine): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Transfer>> {
    engine.owned(&headers, &id).await?;
    Ok(Json(engine.store.get(&id)?))
}
#[derive(Deserialize)]
pub struct Offset {
    pub offset: u64,
}
async fn features(
    State(engine): State<Shared>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    engine.auth(&headers, true).await?;
    Ok(Json(
        serde_json::json!({"version":1,"max_window_bytes":model::WINDOW,"durable_window_ack":true}),
    ))
}

async fn window_chunk(
    State(engine): State<Shared>,
    headers: HeaderMap,
    Path((id, index)): Path<(String, usize)>,
    Query(query): Query<Offset>,
    bytes: Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    engine.owned(&headers, &id).await?;
    let _lock = engine.writes.lock().await;
    let mut transfer = engine.store.get(&id)?;
    engine
        .store
        .write_window(&mut transfer, index, query.offset, &bytes)
        .await?;
    Ok(Json(serde_json::json!({"offset":transfer.offsets[index]})))
}

async fn chunk(
    State(engine): State<Shared>,
    headers: HeaderMap,
    Path((id, index)): Path<(String, usize)>,
    Query(query): Query<Offset>,
    bytes: Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    engine.owned(&headers, &id).await?;
    let _lock = engine.writes.lock().await;
    let mut transfer = engine.store.get(&id)?;
    engine
        .store
        .write_chunk(&mut transfer, index, query.offset, &bytes)
        .await?;
    Ok(Json(serde_json::json!({"offset": transfer.offsets[index]})))
}
async fn finish(
    State(engine): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Transfer>> {
    engine.owned(&headers, &id).await?;
    let _lock = engine.writes.lock().await;
    let mut transfer = engine.store.get(&id)?;
    if let Err(error) = engine.store.finish(&mut transfer).await {
        transfer.error = Some(error.to_string());
        engine.store.save(&transfer)?;
        return Err(error.into());
    }
    Ok(Json(transfer))
}

#[derive(Deserialize)]
struct PeerControl {
    action: String,
}
async fn peer_control(
    State(engine): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(control): Json<PeerControl>,
) -> ApiResult<Json<Transfer>> {
    engine.owned(&headers, &id).await?;
    check(
        ["paused", "cancelled"].contains(&control.action.as_str()),
        "Unsupported action",
    )?;
    let _lock = engine.writes.lock().await;
    let mut transfer = engine.store.get(&id)?;
    check(transfer.status != "complete", "Transfer already complete")?;
    transfer.status = control.action;
    transfer.updated = now();
    engine.store.save(&transfer)?;
    Ok(Json(transfer))
}
