use crate::{
    ApiResult, Offset, Shared, Upload, check, constant_eq,
    model::{Invitation, Session, now},
    pairing::Pairing,
    storage::{safe_path, validate},
    transport::Connection,
};
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{net::SocketAddr, path::PathBuf};
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

pub type AssetLookup = fn(&str) -> Option<&'static [u8]>;

/// Native mobile bundles contain immutable assets, not an unpacked filesystem.
pub async fn serve_embedded(
    engine: Shared,
    bind: SocketAddr,
    lookup: AssetLookup,
) -> Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let router = control_routes(engine.clone()).fallback(move |request: Request| async move {
        if request.method() != axum::http::Method::GET
            && request.method() != axum::http::Method::HEAD
        {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
        let path = request.uri().path().trim_start_matches('/');
        let path = if path.is_empty() { "index.html" } else { path };
        if path.split('/').any(|part| part == "." || part == "..") || path.contains('\\') {
            return StatusCode::NOT_FOUND.into_response();
        }
        let Some(bytes) = lookup(path) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let mime = match path.rsplit('.').next().unwrap_or("") {
            "html" => "text/html; charset=utf-8",
            "js" => "text/javascript",
            "css" => "text/css",
            "png" => "image/png",
            "svg" => "image/svg+xml",
            "webmanifest" => "application/manifest+json",
            _ => "application/octet-stream",
        };
        let body = if request.method() == axum::http::Method::HEAD {
            &[][..]
        } else {
            bytes
        };
        ([(header::CONTENT_TYPE, mime)], body).into_response()
    });
    serve_router(engine, bind, router).await
}

fn control_routes(engine: Shared) -> Router<Shared> {
    let protected = Router::new()
        .route("/state", get(state))
        .route("/invitation", post(invitation))
        .route("/pair", post(pair))
        .route("/session/{id}", post(approve_session))
        .route("/confirm/{id}", post(confirm))
        .route("/receive/{id}", post(receive))
        .route("/batch", post(batch))
        .route("/stage/{id}/{index}", put(stage))
        .route("/send", post(send))
        .route("/control/{id}", post(control))
        .route("/history/clear", post(clear_history))
        .route("/phone", get(phone_info))
        .route("/phone/invitation", post(phone_invitation))
        .route("/phone/offer", post(phone_offer))
        .route("/native/batch", post(native_batch))
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
    let peers: Vec<_> = engine
        .discovery
        .lock()
        .unwrap()
        .as_ref()
        .map(|d| d.peers.lock().unwrap().values().cloned().collect())
        .unwrap_or_default();
    let mut connections: Vec<_> = engine
        .connections
        .lock()
        .await
        .iter()
        .map(|(id, c)| json!({"id":id,"name":c.name,"confirmed":c.confirmed}))
        .collect();
    if let Some(hub) = engine.phone.lock().await.clone() {
        let ids = hub.sessions.lock().unwrap().clone();
        let sessions = engine.sessions.lock().await;
        connections.extend(sessions.values().filter(|session| ids.contains(&session.id) && session.approved).map(|session| json!({"id":format!("phone:{}",session.id),"name":session.name,"confirmed":session.confirmed,"phone":true})));
    }
    let sessions: Vec<_> = engine.sessions.lock().await.values().cloned().collect();
    let invitation = engine.pairing.lock().await.invitation.clone();
    let discovery_error = engine.discovery_error.lock().unwrap().clone();
    let last_error = engine.last_error.lock().await.clone();
    Ok(Json(
        json!({"name":engine.name,"os":std::env::consts::OS,"invitation":invitation,"sessions":sessions,"connections":connections,
        "transfers":engine.store.list()?,"peers":peers,"discoveryError":discovery_error,"lastError":last_error,"receiveDirectory":engine.store.root.join("received").display().to_string()}),
    ))
}
async fn invitation(State(engine): State<Shared>) -> Json<Invitation> {
    let mut pairing = engine.pairing.lock().await;
    *pairing = Pairing::new(pairing.invitation.host.clone(), pairing.invitation.port);
    Json(pairing.invitation.clone())
}
async fn pair(
    State(engine): State<Shared>,
    Json(invite): Json<Invitation>,
) -> ApiResult<Json<Value>> {
    check(
        engine.connections.lock().await.len() < 64,
        "Too many connections; restart Drift to clear",
    )?;
    let connection = Connection::pair(&invite, &engine.name).await?;
    let id = connection.session.clone();
    engine
        .connections
        .lock()
        .await
        .insert(id.clone(), connection);
    Ok(Json(json!({"id":id})))
}
#[derive(Deserialize)]
struct Decision {
    approve: bool,
}
async fn approve_session(
    State(engine): State<Shared>,
    Path(id): Path<String>,
    Json(decision): Json<Decision>,
) -> ApiResult<Json<Value>> {
    let mut sessions = engine.sessions.lock().await;
    if decision.approve {
        sessions.get_mut(&id).context("Session not found")?.approved = true;
    } else {
        sessions.remove(&id);
        engine.owners.lock().await.retain(|_, owner| owner != &id);
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
            sha256: "0".repeat(64),
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

async fn phone_info(State(engine): State<Shared>) -> ApiResult<Json<Value>> {
    if engine.phone.lock().await.is_none() {
        return Ok(Json(json!({"enabled":false})));
    }
    Ok(Json(crate::phone::info(&engine).await?))
}
async fn phone_invitation(State(engine): State<Shared>) -> ApiResult<Json<Value>> {
    let hub = crate::phone::hub(&engine).await?;
    *hub.enrollment.lock().await = crate::phone::Enrollment::default();
    Ok(Json(crate::phone::info(&engine).await?))
}
#[derive(Deserialize)]
struct PhoneSend {
    phone: String,
    batch: String,
}
async fn phone_offer(
    State(engine): State<Shared>,
    Json(request): Json<PhoneSend>,
) -> ApiResult<Json<Value>> {
    let id = crate::phone::create_offer(&engine, &request.phone, &request.batch).await?;
    Ok(Json(json!({"id":id})))
}

#[derive(Deserialize)]
struct NativeFile {
    path: String,
    source: PathBuf,
}
async fn native_batch(
    State(engine): State<Shared>,
    headers: HeaderMap,
    Json(files): Json<Vec<NativeFile>>,
) -> ApiResult<Json<Value>> {
    let token = headers
        .get("x-drift-native")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    check(
        engine
            .native_token
            .as_ref()
            .is_some_and(|expected| constant_eq(expected, token)),
        "Native file selection requires the desktop app",
    )?;
    check(
        !files.is_empty() && files.len() <= crate::model::MAX_FILES,
        "Choose between 1 and 10,000 files",
    )?;
    let mut uploads = Vec::new();
    let mut sources = Vec::new();
    for file in files {
        let metadata = tokio::fs::symlink_metadata(&file.source).await?;
        check(
            file.source.is_absolute() && metadata.is_file() && !metadata.file_type().is_symlink(),
            "Choose regular files, not symlinks",
        )?;
        uploads.push(Upload {
            path: file.path,
            size: metadata.len(),
        });
        sources.push(file.source);
    }
    let entries: Vec<_> = uploads
        .iter()
        .map(|upload| crate::model::Entry {
            path: upload.path.clone(),
            size: upload.size,
            sha256: "0".repeat(64),
        })
        .collect();
    validate(&entries)?;
    let mut batches = engine.uploads.lock().await;
    check(
        batches.len() < 100,
        "Too many selected batches; restart Drift to clear them",
    )?;
    let id = uuid::Uuid::new_v4().to_string();
    engine
        .native_sources
        .lock()
        .await
        .insert(id.clone(), sources);
    batches.insert(id.clone(), uploads.clone());
    Ok(Json(json!({"id":id,"files":uploads})))
}
