use crate::{
    ApiResult, Offset, Shared, Upload, check, constant_eq,
    model::{DeviceHello, Invitation, Session, TrustedDevice, capability_hash, now},
    pairing::{Pairing, secret},
    storage::{safe_path, validate},
    transport::{Connection, Refused},
};
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashSet, net::SocketAddr, path::PathBuf, time::Duration};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer};

pub async fn serve(
    engine: Shared,
    bind: SocketAddr,
    assets: PathBuf,
) -> Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let router = control_routes(engine.clone()).fallback_service(ServeDir::new(assets));
    serve_router(engine, bind, router).await
}

/// The control API without a socket, for a native shell that calls it in
/// process. Requests still need `Authorization: Bearer <ui_token>`.
pub fn app_router(engine: Shared) -> Router {
    control_routes(engine.clone())
        .layer(DefaultBodyLimit::max(crate::model::MAX_METADATA))
        .with_state(engine)
}

fn control_routes(engine: Shared) -> Router<Shared> {
    let protected = Router::new()
        .route("/state", get(state))
        .route("/invitation", post(invitation))
        .route("/pair", post(pair))
        .route("/introduce", post(introduce))
        .route("/session/{id}", post(approve_session))
        .route("/confirm/{id}", post(confirm))
        .route("/receive/{id}", post(receive))
        .route("/batch", post(batch))
        .route("/stage/{id}/{index}", put(stage))
        .route("/send", post(send))
        .route("/control/{id}", post(control))
        .route("/history/clear", post(clear_history))
        .route("/devices/{id}", post(update_device))
        .route("/devices/{id}/forget", post(forget_device))
        .route_layer(middleware::from_fn_with_state(engine.clone(), authorize));
    Router::new().nest("/api", protected)
}

async fn serve_router(
    engine: Shared,
    bind: SocketAddr,
    router: Router<Shared>,
) -> Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    ensure!(
        bind.ip().is_loopback(),
        "The control interface must bind to loopback"
    );
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let address = listener.local_addr()?;
    let router = router
        .layer(DefaultBodyLimit::max(crate::model::MAX_METADATA))
        .layer(SetResponseHeaderLayer::overriding(header::CACHE_CONTROL, HeaderValue::from_static("no-store")))
        .layer(SetResponseHeaderLayer::overriding(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::overriding(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY")))
        .layer(SetResponseHeaderLayer::overriding(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'")))
        .with_state(engine);
    let task = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            eprintln!("Control interface stopped: {e}");
        }
    });
    Ok((address, task))
}

async fn authorize(State(engine): State<Shared>, request: Request, next: Next) -> Response {
    let token = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if !constant_eq(token, &engine.ui_token) {
        return (
            StatusCode::UNAUTHORIZED,
            "Open the private interface URL printed by Drift.",
        )
            .into_response();
    }
    // No CORS, no cookies, no query-string capabilities. Cross-site forms cannot
    // set Authorization; cross-site fetches fail preflight.
    next.run(request).await
}

async fn state(State(engine): State<Shared>) -> ApiResult<Json<Value>> {
    let discovery_running = engine.discovery.lock().unwrap().is_some();
    let peers: Vec<_> = engine
        .discovery
        .lock()
        .unwrap()
        .as_ref()
        .map(|d| d.peers.lock().unwrap().values().cloned().collect())
        .unwrap_or_default();
    let nearby: HashSet<_> = peers.iter().map(|peer| peer.id.clone()).collect();
    let devices = engine.devices.lock().unwrap().clone();
    let mut connections: Vec<_> = engine
        .connections
        .lock()
        .await
        .iter()
        .map(|(id, c)| match devices.get(id) {
            Some(device) => json!({"id":id,"name":device.name,"os":device.os,"confirmed":true,"remembered":true,"own":device.own,
                "online": if discovery_running { Value::Bool(nearby.contains(id)) } else { Value::Null }}),
            None => json!({"id":id,"name":c.name,"os":c.os,"confirmed":c.confirmed,"code":c.code}),
        })
        .collect();
    connections.sort_by_key(|device| device["name"].as_str().unwrap_or("").to_lowercase());
    // Remembered devices are listed as devices, not as strangers nearby.
    let peers: Vec<_> = peers
        .into_iter()
        .filter(|peer| !devices.contains_key(&peer.id))
        .collect();
    let sessions: Vec<_> = engine.sessions.lock().await.values().cloned().collect();
    let invitation = {
        let mut pairing = engine.pairing.lock().await;
        // An expired code is useless; replace it before anyone copies it.
        if pairing.invitation.expires <= crate::model::now() {
            *pairing = Pairing::new(pairing.invitation.host.clone(), pairing.invitation.port);
        }
        pairing.invitation.clone()
    };
    let transfers = engine.store.list()?;
    let mut saved = serde_json::Map::new();
    for transfer in transfers
        .iter()
        .filter(|t| t.direction == "receive" && t.status == "complete")
    {
        let paths = engine.store.saved(&transfer.id)?;
        if !paths.is_empty() {
            saved.insert(transfer.id.clone(), json!(paths));
        }
    }
    let receive_directory = engine
        .store
        .inbox
        .clone()
        .unwrap_or_else(|| engine.store.root.join("received"));
    let discovery_error = engine.discovery_error.lock().unwrap().clone();
    let last_error = engine.last_error.lock().await.clone();
    Ok(Json(
        json!({"name":engine.name,"os":std::env::consts::OS,"invitation":invitation,"sessions":sessions,"connections":connections,
        "transfers":transfers,"saved":saved,"peers":peers,"discoveryError":discovery_error,"lastError":last_error,"receiveDirectory":receive_directory.display().to_string()}),
    ))
}
async fn invitation(State(engine): State<Shared>) -> Json<Invitation> {
    let mut pairing = engine.pairing.lock().await;
    *pairing = Pairing::new(pairing.invitation.host.clone(), pairing.invitation.port);
    Json(pairing.invitation.clone())
}
fn hello(engine: &Shared) -> (DeviceHello, String) {
    let capability = secret(32);
    let hello = DeviceHello {
        id: engine.fingerprint.clone(),
        port: engine.port,
        capability: capability.clone(),
        os: std::env::consts::OS.into(),
    };
    (hello, capability_hash(&capability))
}

/// Lists a pairing that waits for the other device's approval and finishes it
/// in the background.
async fn begin(
    engine: Shared,
    connection: Connection,
    incoming: String,
    host: String,
    port: u16,
) -> Result<()> {
    check(
        connection.fingerprint != engine.fingerprint,
        "This is the same device. Pair with another one.",
    )?;
    engine.outgoing_pairs.lock().unwrap().insert(
        incoming.clone(),
        (connection.fingerprint.clone(), connection.name.clone()),
    );
    engine
        .connections
        .lock()
        .await
        .insert(connection.session.clone(), connection.clone());
    tokio::spawn(complete_pairing(engine, connection, incoming, host, port));
    Ok(())
}

async fn pair(
    State(engine): State<Shared>,
    Json(invite): Json<Invitation>,
) -> ApiResult<Json<Value>> {
    check(
        engine.connections.lock().await.len() < 256,
        "Too many devices; forget some in Devices first",
    )?;
    let (hello, incoming) = hello(&engine);
    let connection = Connection::pair_device(&invite, &engine.name, Some(hello)).await?;
    let id = connection.session.clone();
    begin(engine, connection, incoming, invite.host, invite.port).await?;
    Ok(Json(json!({"id":id})))
}

#[derive(Deserialize)]
struct Nearby {
    host: String,
    port: u16,
    /// The fingerprint discovery advertised for this device, if any.
    #[serde(default)]
    id: Option<String>,
}

/// Tap-to-pair: no code to type; both screens show six digits to compare.
async fn introduce(
    State(engine): State<Shared>,
    Json(nearby): Json<Nearby>,
) -> ApiResult<Json<Value>> {
    check(
        engine.connections.lock().await.len() < 256,
        "Too many devices; forget some in Devices first",
    )?;
    let (hello, incoming) = hello(&engine);
    let connection = Connection::introduce(
        &nearby.host,
        nearby.port,
        &engine.name,
        hello,
        nearby.id.as_deref(),
    )
    .await?;
    let reply = json!({"id": connection.session, "code": connection.code, "name": connection.name});
    begin(engine, connection, incoming, nearby.host, nearby.port).await?;
    Ok(Json(reply))
}

/// Waits for the other device's approval, then remembers it. There is no
/// separate confirmation step here: starting the pairing was the consent.
async fn complete_pairing(
    engine: Shared,
    connection: Connection,
    incoming: String,
    host: String,
    port: u16,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    let outcome = loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if !engine
            .connections
            .lock()
            .await
            .contains_key(&connection.session)
        {
            break None;
        }
        if tokio::time::Instant::now() >= deadline {
            break Some(format!(
                "{} did not approve the connection in time. Pair again.",
                connection.name
            ));
        }
        match connection.get::<Session>("/session").await {
            Ok(session) if session.approved => {
                if !session.confirmed
                    && let Err(error) = connection.post::<_, Session>("/confirm", &()).await
                {
                    eprintln!("Pairing confirmation will be retried: {error}");
                    continue;
                }
                let device = TrustedDevice {
                    id: connection.fingerprint.clone(),
                    name: connection.name.clone(),
                    os: connection.os.clone(),
                    host: host.clone(),
                    port,
                    outgoing: connection.capability().to_owned(),
                    incoming: incoming.clone(),
                    own: true,
                    added: now(),
                };
                engine.connections.lock().await.remove(&connection.session);
                break match engine.trust(device).await {
                    Ok(()) => None,
                    Err(error) => Some(format!("Could not remember {}: {error}", connection.name)),
                };
            }
            Ok(_) => {}
            Err(error) if error.downcast_ref::<Refused>().is_some() => {
                break Some(format!("{} declined the connection.", connection.name));
            }
            // Unreachable for a moment (Wi-Fi roaming, sleep): keep waiting.
            Err(_) => {}
        }
    };
    engine.connections.lock().await.remove(&connection.session);
    engine.outgoing_pairs.lock().unwrap().remove(&incoming);
    if let Some(message) = outcome {
        *engine.last_error.lock().await = Some(message);
    }
}
#[derive(Deserialize)]
struct Decision {
    approve: bool,
    /// For a device that can be remembered: accept its files without asking.
    #[serde(default)]
    own: Option<bool>,
}
async fn approve_session(
    State(engine): State<Shared>,
    Path(id): Path<String>,
    Json(decision): Json<Decision>,
) -> ApiResult<Json<Value>> {
    let mut sessions = engine.sessions.lock().await;
    if !decision.approve {
        sessions.remove(&id);
        engine.owners.lock().await.retain(|_, owner| owner != &id);
        return Ok(Json(json!({"ok":true})));
    }
    let session = sessions.get_mut(&id).context("Session not found")?;
    let Some((hello, host)) = session.device.clone() else {
        session.approved = true;
        return Ok(Json(json!({"ok":true})));
    };
    // A device that introduced itself is remembered in both directions; its
    // capability now lives in the device list instead of this session.
    let device = TrustedDevice {
        id: hello.id,
        name: session.name.clone(),
        os: hello.os,
        host,
        port: hello.port,
        outgoing: hello.capability,
        incoming: capability_hash(&session.capability),
        own: decision.own.unwrap_or(true),
        added: now(),
    };
    sessions.remove(&id);
    drop(sessions);
    engine.trust(device).await?;
    Ok(Json(json!({"ok":true})))
}

#[derive(Deserialize)]
struct DeviceUpdate {
    own: bool,
}
async fn update_device(
    State(engine): State<Shared>,
    Path(id): Path<String>,
    Json(update): Json<DeviceUpdate>,
) -> ApiResult<Json<Value>> {
    let device = {
        let mut devices = engine.devices.lock().unwrap();
        let device = devices.get_mut(&id).context("Device not found")?;
        device.own = update.own;
        device.clone()
    };
    engine.store.save_device(&device)?;
    Ok(Json(json!({"ok":true})))
}

async fn forget_device(
    State(engine): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let connection = engine.connections.lock().await.get(&id).cloned();
    check(
        engine.devices.lock().unwrap().contains_key(&id) || connection.is_some(),
        "Device not found",
    )?;
    engine.forget(&id).await?;
    // Tell the other device too, if it is reachable; it forgets us on its side.
    if let Some(connection) = connection {
        tokio::spawn(async move {
            let _ = tokio::time::timeout(
                Duration::from_secs(10),
                connection.post::<_, Value>("/forget", &()),
            )
            .await;
        });
    }
    Ok(Json(json!({"ok":true})))
}
async fn confirm(State(engine): State<Shared>, Path(id): Path<String>) -> ApiResult<Json<Session>> {
    let connection = engine
        .connections
        .lock()
        .await
        .get(&id)
        .cloned()
        .context("Connection not found")?;
    let session = connection.post("/confirm", &()).await?;
    if let Some(connection) = engine.connections.lock().await.get_mut(&id) {
        connection.confirmed = true;
    }
    Ok(Json(session))
}
async fn receive(
    State(engine): State<Shared>,
    Path(id): Path<String>,
    Json(decision): Json<Decision>,
) -> ApiResult<Json<Value>> {
    engine.receive_approval(&id, decision.approve).await?;
    Ok(Json(json!({"ok":true})))
}
async fn batch(
    State(engine): State<Shared>,
    Json(files): Json<Vec<Upload>>,
) -> ApiResult<Json<Value>> {
    let dummy: Vec<_> = files
        .iter()
        .map(|f| crate::model::Entry {
            path: f.path.clone(),
            size: f.size,
            blake3: "0".repeat(64),
        })
        .collect();
    validate(&dummy)?;
    let mut uploads = engine.uploads.lock().await;
    check(
        uploads.len() < 100,
        "Outbox is full. Restart Drift and remove old outbox files.",
    )?;
    let id = uuid::Uuid::new_v4().to_string();
    tokio::fs::create_dir_all(engine.store.root.join("outbox").join(&id)).await?;
    for index in 0..files.len() {
        tokio::fs::File::create(
            engine
                .store
                .root
                .join("outbox")
                .join(&id)
                .join(index.to_string()),
        )
        .await?;
    }
    uploads.insert(id.clone(), files);
    Ok(Json(json!({"id":id})))
}
async fn stage(
    State(engine): State<Shared>,
    Path((id, index)): Path<(String, usize)>,
    Query(query): Query<Offset>,
    bytes: Bytes,
) -> ApiResult<Json<Value>> {
    let _lock = engine.writes.lock().await;
    let uploads = engine.uploads.lock().await;
    let upload = uploads
        .get(&id)
        .and_then(|v| v.get(index))
        .context("Upload missing")?;
    safe_path(&upload.path)?;
    check(
        bytes.len() <= crate::model::CHUNK && !bytes.is_empty(),
        "Invalid staging chunk",
    )?;
    check(
        query
            .offset
            .checked_add(bytes.len() as u64)
            .is_some_and(|n| n <= upload.size),
        "Staged file exceeds declared size",
    )?;
    let path = engine
        .store
        .root
        .join("outbox")
        .join(&id)
        .join(index.to_string());
    let mut file = tokio::fs::OpenOptions::new().write(true).open(path).await?;
    check(
        file.metadata().await?.len() == query.offset,
        "Staging offset mismatch",
    )?;
    file.seek(std::io::SeekFrom::Start(query.offset)).await?;
    file.write_all(&bytes).await?;
    // Tokio hands the write to a background thread; without this flush the reply
    // can beat the bytes to disk, and the next piece fails "Staging offset mismatch".
    file.flush().await?;
    Ok(Json(json!({"offset":query.offset + bytes.len() as u64})))
}
#[derive(Deserialize)]
struct Send {
    connection: String,
    batch: String,
}
async fn send(State(engine): State<Shared>, Json(request): Json<Send>) -> ApiResult<Json<Value>> {
    let connection = engine
        .connections
        .lock()
        .await
        .get(&request.connection)
        .cloned()
        .context("Pair a device first")?;
    let session: Session = connection.get("/session").await?;
    check(
        session.approved && session.confirmed,
        "Approve on the receiver, then open Devices and confirm the connection here",
    )?;
    check(
        engine.uploads.lock().await.contains_key(&request.batch),
        "Choose files first",
    )?;
    check(
        engine.sending.try_lock().is_ok(),
        "Another send is active. Pause it or wait for it to finish.",
    )?;
    *engine.last_error.lock().await = None;
    tokio::spawn(async move {
        if let Err(e) = engine.send(&request.connection, &request.batch).await {
            *engine.last_error.lock().await = Some(e.to_string());
        }
    });
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
struct Control {
    action: String,
}
async fn control(
    State(engine): State<Shared>,
    Path(id): Path<String>,
    Json(request): Json<Control>,
) -> ApiResult<Json<Value>> {
    let _lock = engine.writes.lock().await;
    let mut transfer = engine.store.get(&id)?;
    check(
        transfer.status != "complete",
        "Completed transfers cannot be changed",
    )?;
    check(
        ["paused", "cancelled"].contains(&request.action.as_str()),
        "Unsupported action",
    )?;
    transfer.status = request.action;
    if transfer.direction == "send" {
        engine
            .send_controls
            .lock()
            .unwrap()
            .insert(id, transfer.status.clone());
    }
    transfer.updated = now();
    engine.store.save(&transfer)?;
    Ok(Json(json!({"ok":true})))
}
async fn clear_history(State(engine): State<Shared>) -> ApiResult<Json<Value>> {
    engine.store.clear_history()?;
    Ok(Json(json!({"ok":true})))
}
