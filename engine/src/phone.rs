//! Separate, TLS-only phone surface. Never mounts the desktop control router.
use crate::{
    ApiResult, Shared, Upload, check, constant_eq,
    model::{Entry, Session, Transfer, now},
    pairing::secret,
    storage::{hash_file, validate},
};
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    routing::{get, post},
};
use base64::Engine as _;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    sync::Mutex,
};
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};

const DOWNLOAD_CHUNK: usize = 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Credentials {
    host: String,
    expires: u64,
    certificate: String,
    key: String,
    ca: String,
    ca_der: String,
    fingerprint: String,
}

fn credentials(root: &std::path::Path, host: &str) -> Result<Credentials> {
    let path = root.join("phone-tls.json");
    if path.exists() {
        let saved: Credentials = serde_json::from_slice(&std::fs::read(&path)?)?;
        if saved.host == host && saved.expires > now() + 86400 {
            return Ok(saved);
        }
    }
    let start = time::OffsetDateTime::now_utc() - time::Duration::days(1);
    let end = start + time::Duration::days(365);
    let mut ca = CertificateParams::new(Vec::<String>::new())?;
    ca.not_before = start;
    ca.not_after = end;
    ca.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    ca.distinguished_name = DistinguishedName::new();
    ca.distinguished_name
        .push(DnType::CommonName, "Drift local phone connection");
    let issuing_key = KeyPair::generate()?;
    let issuer = ca.self_signed(&issuing_key)?;
    let mut leaf = CertificateParams::new(vec![
        host.to_owned(),
        "localhost".into(),
        "127.0.0.1".into(),
    ])?;
    leaf.not_before = start;
    leaf.not_after = end;
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    leaf.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    leaf.distinguished_name
        .push(DnType::CommonName, "Drift phone portal");
    let leaf_key = KeyPair::generate()?;
    let certificate = leaf.signed_by(&leaf_key, &issuer, &issuing_key)?;
    let saved = Credentials {
        host: host.into(),
        expires: end.unix_timestamp() as u64,
        certificate: format!("{}{}", certificate.pem(), issuer.pem()),
        key: leaf_key.serialize_pem(),
        ca: issuer.pem(),
        ca_der: base64::engine::general_purpose::STANDARD.encode(issuer.der()),
        fingerprint: hex::encode(Sha256::digest(issuer.der())),
    };
    // The CA issuing key is never serialized. Only a leaf key is retained.
    drop(issuing_key);
    let temporary = root.join(format!("phone-tls-{}.tmp", uuid::Uuid::new_v4()));
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(&serde_json::to_vec(&saved)?)?;
        file.sync_all()?;
    }
    std::fs::rename(temporary, path)?;
    Ok(saved)
}

pub struct Enrollment {
    pub token: String,
    pub expires: u64,
    pub used: bool,
    attempts: u32,
    window: u64,
}
impl Default for Enrollment {
    fn default() -> Self {
        Self {
            token: secret(24),
            expires: now() + 300,
            used: false,
            attempts: 0,
            window: now(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Offer {
    pub id: String,
    #[serde(skip_serializing, default)]
    pub session: String,
    #[serde(skip_serializing, default)]
    pub batch: String,
    pub entries: Vec<Entry>,
    pub offsets: Vec<u64>,
    pub verified: Vec<bool>,
    pub status: String,
    pub updated: u64,
}

pub struct Hub {
    pub origin: String,
    pub fingerprint: String,
    pub ca_pem: String,
    pub profile: String,
    pub enrollment: Mutex<Enrollment>,
    pub offers: Mutex<HashMap<String, Offer>>,
    pub sessions: std::sync::Mutex<HashSet<String>>,
}

pub struct Running {
    pub hub: Arc<Hub>,
    handle: axum_server::Handle,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Running {
    fn drop(&mut self) {
        self.handle.shutdown();
        self.task.abort();
    }
}

pub async fn start(
    engine: Shared,
    host: String,
    bind: SocketAddr,
    assets: PathBuf,
) -> Result<Running> {
    let saved = credentials(&engine.store.root, &host)?;
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        saved.certificate.into_bytes(),
        saved.key.into_bytes(),
    )
    .await?;
    let listener = std::net::TcpListener::bind(bind)?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let profile = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>PayloadContent</key><array><dict><key>PayloadType</key><string>com.apple.security.root</string><key>PayloadVersion</key><integer>1</integer><key>PayloadIdentifier</key><string>local.drift.phone.certificate</string><key>PayloadUUID</key><string>{}</string><key>PayloadDisplayName</key><string>Drift local phone connection</string><key>PayloadCertificateFileName</key><string>Drift-Phone-CA.cer</string><key>PayloadContent</key><data>{}</data></dict></array><key>PayloadType</key><string>Configuration</string><key>PayloadVersion</key><integer>1</integer><key>PayloadIdentifier</key><string>local.drift.phone</string><key>PayloadUUID</key><string>{}</string><key>PayloadDisplayName</key><string>Drift local phone connection</string><key>PayloadDescription</key><string>Local TLS trust for your Mac. Verify the certificate fingerprint on the Mac before installing. The CA issuing key is not retained. Remove this profile when no longer needed.</string><key>PayloadRemovalDisallowed</key><false/></dict></plist>"#,
        uuid::Uuid::new_v4(),
        saved.ca_der,
        uuid::Uuid::new_v4()
    );
    // Different Macs must not replace each other's iOS trust profile.
    let profile_id = format!(
        "local.drift.phone.{}",
        saved.fingerprint.chars().take(32).collect::<String>()
    );
    let profile = profile
        .replace(
            "local.drift.phone.certificate",
            &format!("{profile_id}.certificate"),
        )
        .replace(
            "<string>local.drift.phone</string>",
            &format!("<string>{profile_id}</string>"),
        );
    let hub = Arc::new(Hub {
        origin: format!("https://{host}:{port}"),
        fingerprint: saved.fingerprint,
        ca_pem: saved.ca,
        profile,
        enrollment: Mutex::new(Enrollment::default()),
        offers: Mutex::new(HashMap::new()),
        sessions: std::sync::Mutex::new(HashSet::new()),
    });
    *engine.phone.lock().await = Some(hub.clone());
    let router = Router::new().route("/phone", get_service_file(assets.join("phone.html")))
        .route_service("/manifest.webmanifest", ServeFile::new(assets.join("manifest.webmanifest")))
        .route_service("/phone-sw.js", ServeFile::new(assets.join("phone-sw.js")))
        .nest_service("/assets", ServeDir::new(assets.join("assets")))
        .nest_service("/icons", ServeDir::new(assets.join("icons")))
        .route("/mobile/pair", post(enroll)).route("/mobile/session", get(super::session_status))
        .route("/mobile/confirm", post(super::confirm)).route("/mobile/state", get(state))
        .route("/mobile/offer", post(super::offer)).route("/mobile/transfer/{id}", get(super::transfer_status))
        .route("/mobile/transfer/{id}/finish", post(super::finish))
        .route("/mobile/transfer/{id}/control", post(super::peer_control))
        .route("/mobile/transfer/{id}/{index}", axum::routing::put(super::chunk).layer(DefaultBodyLimit::max(crate::model::CHUNK)))
        .route("/mobile/receive/{id}", post(accept)).route("/mobile/download/{id}/{index}", get(download))
        .route("/mobile/progress/{id}", post(progress))
        .layer(DefaultBodyLimit::max(crate::model::MAX_METADATA))
        .layer(tower::limit::ConcurrencyLimitLayer::new(4))
        .layer(SetResponseHeaderLayer::overriding(header::CACHE_CONTROL, HeaderValue::from_static("no-store")))
        .layer(SetResponseHeaderLayer::overriding(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::overriding(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; worker-src 'self'; frame-ancestors 'none'; base-uri 'none'")))
        .with_state(engine);
    let handle = axum_server::Handle::new();
    let server = axum_server::from_tcp_rustls(listener, tls).handle(handle.clone());
    let task = tokio::spawn(async move {
        if let Err(error) = server.serve(router.into_make_service()).await {
            eprintln!("Phone listener stopped: {error}");
        }
    });
    Ok(Running { hub, handle, task })
}

fn get_service_file(path: PathBuf) -> axum::routing::MethodRouter<Shared> {
    axum::routing::get_service(ServeFile::new(path))
}
pub async fn hub(engine: &Shared) -> Result<Arc<Hub>> {
    engine
        .phone
        .lock()
        .await
        .clone()
        .context("Phone access is not enabled")
}

pub async fn info(engine: &Shared) -> Result<Value> {
    let hub = hub(engine).await?;
    let invite = hub.enrollment.lock().await;
    Ok(
        json!({"origin":hub.origin,"url":format!("{}/phone#{}",hub.origin,invite.token),"expires":invite.expires,"used":invite.used,
        "fingerprint":hub.fingerprint,"certificate":hub.ca_pem,"profile":hub.profile}),
    )
}

#[derive(Deserialize)]
struct Enroll {
    token: String,
    name: String,
}
async fn enroll(
    State(engine): State<Shared>,
    Json(request): Json<Enroll>,
) -> ApiResult<Json<Value>> {
    let hub = hub(&engine).await?;
    let mut invite = hub.enrollment.lock().await;
    if now().saturating_sub(invite.window) >= 60 {
        invite.attempts = 0;
        invite.window = now();
    }
    invite.attempts += 1;
    check(
        invite.attempts <= 30,
        "Too many phone pairing requests; wait a minute",
    )?;
    check(
        !invite.used && now() < invite.expires,
        "Phone invitation expired or already used; generate a new invitation on the Mac",
    )?;
    check(
        constant_eq(&invite.token, &request.token),
        "Invalid phone invitation",
    )?;
    check(
        !request.name.trim().is_empty() && request.name.len() <= 100,
        "Use a device name of 1–100 bytes",
    )?;
    let mut sessions = engine.sessions.lock().await;
    check(
        sessions.len() < 64,
        "Too many sessions; revoke unused sessions",
    )?;
    let id = uuid::Uuid::new_v4().to_string();
    let capability = secret(32);
    sessions.insert(
        id.clone(),
        Session {
            id: id.clone(),
            name: request.name,
            approved: false,
            confirmed: false,
            created: now(),
            capability: capability.clone(),
        },
    );
    hub.sessions.lock().unwrap().insert(id.clone());
    invite.used = true;
    Ok(Json(
        json!({"session":id,"capability":capability,"name":engine.name}),
    ))
}

async fn state(State(engine): State<Shared>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let session = engine.auth(&headers, true).await?;
    let hub = hub(&engine).await?;
    let offers: Vec<_> = hub
        .offers
        .lock()
        .await
        .values()
        .filter(|o| o.session == session.id)
        .cloned()
        .collect();
    let owners = engine.owners.lock().await;
    let uploads: Vec<_> = engine
        .store
        .list()?
        .into_iter()
        .filter(|t| owners.get(&t.id) == Some(&session.id))
        .collect();
    Ok(Json(
        json!({"name":engine.name,"offers":offers,"uploads":uploads}),
    ))
}

pub async fn create_offer(engine: &Shared, session_id: &str, batch: &str) -> Result<String> {
    let hub = hub(engine).await?;
    ensure!(
        hub.sessions.lock().unwrap().contains(session_id),
        "This is not a phone session"
    );
    let session = engine
        .sessions
        .lock()
        .await
        .get(session_id)
        .cloned()
        .context("Phone is no longer connected")?;
    ensure!(
        session.approved && session.confirmed && now().saturating_sub(session.created) < 86400,
        "Approve the connection on both devices first"
    );
    let uploads: Vec<Upload> = engine
        .uploads
        .lock()
        .await
        .get(batch)
        .cloned()
        .context("Select files again")?;
    let mut entries = Vec::new();
    for (index, upload) in uploads.iter().enumerate() {
        let path = engine.source_path(batch, index).await;
        ensure!(
            tokio::fs::metadata(&path).await?.len() == upload.size,
            "Staging is incomplete"
        );
        entries.push(Entry {
            path: upload.path.clone(),
            size: upload.size,
            sha256: hash_file(&path).await?,
        });
    }
    validate(&entries)?;
    let mut offers = hub.offers.lock().await;
    ensure!(
        offers.len() < 32,
        "Phone outbox is full; restart Drift to clear expired offers"
    );
    let id = uuid::Uuid::new_v4().to_string();
    let offer = Offer {
        id: id.clone(),
        session: session_id.into(),
        batch: batch.into(),
        offsets: vec![0; entries.len()],
        verified: vec![false; entries.len()],
        entries,
        status: "pending".into(),
        updated: now(),
    };
    save_offer(engine, &offer, &session.name)?;
    offers.insert(id.clone(), offer);
    Ok(id)
}
fn save_offer(engine: &Shared, offer: &Offer, peer: &str) -> Result<()> {
    engine.store.save(&Transfer {
        id: format!("phone-send-{}", offer.id),
        entries: offer.entries.clone(),
        offsets: offer.offsets.clone(),
        status: if offer.status == "pending" {
            "awaiting approval".into()
        } else {
            offer.status.clone()
        },
        peer: peer.into(),
        updated: now(),
        error: None,
        direction: "send".into(),
    })
}
#[derive(Deserialize)]
struct Accept {
    approve: bool,
}
async fn accept(
    State(engine): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<Accept>,
) -> ApiResult<Json<Offer>> {
    let session = engine.auth(&headers, true).await?;
    let hub = hub(&engine).await?;
    let mut offers = hub.offers.lock().await;
    let offer = offers.get_mut(&id).context("Offer not found")?;
    check(
        offer.session == session.id && offer.status != "complete",
        "Offer is unavailable",
    )?;
    let local = engine.store.get(&format!("phone-send-{id}"))?;
    check(
        local.status != "cancelled" && local.status != "paused",
        "Mac paused or cancelled this offer; ask it to send a new offer",
    )?;
    offer.status = if request.approve {
        "receiving"
    } else {
        "cancelled"
    }
    .into();
    save_offer(&engine, offer, &session.name)?;
    Ok(Json(offer.clone()))
}
#[derive(Deserialize)]
struct Download {
    offset: u64,
}
async fn download(
    State(engine): State<Shared>,
    headers: HeaderMap,
    Path((id, index)): Path<(String, usize)>,
    Query(query): Query<Download>,
) -> ApiResult<Bytes> {
    let session = engine.auth(&headers, true).await?;
    let hub = hub(&engine).await?;
    let offer = hub
        .offers
        .lock()
        .await
        .get(&id)
        .cloned()
        .context("Offer not found")?;
    check(
        offer.session == session.id && offer.status == "receiving",
        "Approve this offer on the phone first",
    )?;
    let entry = offer.entries.get(index).context("File index not found")?;
    let local = engine.store.get(&format!("phone-send-{id}"))?;
    check(
        local.status != "cancelled" && local.status != "paused",
        "Mac paused or cancelled this transfer",
    )?;
    check(query.offset <= entry.size, "Invalid offset")?;
    let path = engine.source_path(&offer.batch, index).await;
    let mut file = tokio::fs::File::open(path).await?;
    file.seek(std::io::SeekFrom::Start(query.offset)).await?;
    let mut bytes = vec![0; DOWNLOAD_CHUNK.min((entry.size - query.offset) as usize)];
    file.read_exact(&mut bytes).await?;
    Ok(Bytes::from(bytes))
}
#[derive(Deserialize)]
struct Progress {
    index: usize,
    offset: u64,
    sha256: Option<String>,
}
async fn progress(
    State(engine): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<Progress>,
) -> ApiResult<Json<Offer>> {
    let session = engine.auth(&headers, true).await?;
    let hub = hub(&engine).await?;
    let mut offers = hub.offers.lock().await;
    let offer = offers.get_mut(&id).context("Offer not found")?;
    check(
        offer.session == session.id && offer.status == "receiving",
        "Offer is unavailable",
    )?;
    let entry = offer
        .entries
        .get(request.index)
        .context("Invalid file index")?;
    check(request.offset <= entry.size, "Invalid offset")?;
    let local = engine.store.get(&format!("phone-send-{id}"))?;
    check(
        local.status != "cancelled" && local.status != "paused",
        "Mac paused or cancelled this offer",
    )?;
    if let Some(hash) = request.sha256.as_ref() {
        check(
            request.offset == entry.size && hash == &entry.sha256,
            "Phone integrity check failed",
        )?;
    }
    // Completion requires an explicit verified digest for each file, including empty files.
    if request.sha256.is_none() {
        check(
            request.offset < entry.size,
            "Send the verified SHA-256 to mark a file complete",
        )?;
    }
    offer.offsets[request.index] = request.offset;
    if request.sha256.is_some() {
        offer.verified[request.index] = true;
    }
    if offer.verified.iter().all(|verified| *verified) {
        offer.status = "complete".into();
    }
    offer.updated = now();
    save_offer(&engine, offer, &session.name)?;
    Ok(Json(offer.clone()))
}
