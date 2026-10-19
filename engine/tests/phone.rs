use drift_engine::{
    Upload,
    model::{Entry, Transfer},
    phone::{self, Enrollment},
    storage::hash_file,
};
use reqwest::{Client, Response};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{net::SocketAddr, path::Path};

async fn device(root: &Path) -> drift_engine::Running {
    drift_engine::start(
        root.into(),
        "Test Mac".into(),
        "127.0.0.1".into(),
        SocketAddr::from(([127, 0, 0, 1], 0)),
        false,
    )
    .await
    .unwrap()
}
fn client(hub: &phone::Hub) -> Client {
    // Real certificate validation, with only this generated CA as a trust anchor.
    Client::builder()
        .tls_built_in_root_certs(false)
        .add_root_certificate(reqwest::Certificate::from_pem(hub.ca_pem.as_bytes()).unwrap())
        .no_proxy()
        .build()
        .unwrap()
}
async fn enroll(
    engine: &drift_engine::Shared,
    hub: &phone::Hub,
    client: &Client,
    approve: bool,
) -> (String, String) {
    let token = hub.enrollment.lock().await.token.clone();
    let response: Value = client
        .post(format!("{}/mobile/pair", hub.origin))
        .json(&json!({"token":token,"name":"My phone"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = response["session"].as_str().unwrap().to_owned();
    let capability = response["capability"].as_str().unwrap().to_owned();
    if approve {
        engine.sessions.lock().await.get_mut(&id).unwrap().approved = true;
        assert!(
            client
                .post(format!("{}/mobile/confirm", hub.origin))
                .bearer_auth(&capability)
                .json(&())
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
    }
    (id, capability)
}
async fn post(client: &Client, hub: &phone::Hub, cap: &str, path: &str, body: Value) -> Response {
    client
        .post(format!("{}{path}", hub.origin))
        .bearer_auth(cap)
        .json(&body)
        .send()
        .await
        .unwrap()
}
fn entry(path: &str, data: &[u8]) -> Entry {
    Entry {
        path: path.into(),
        size: data.len() as u64,
        sha256: hex::encode(Sha256::digest(data)),
    }
}

#[tokio::test]
async fn phone_enrollment_expiry_replay_approval_and_admin_isolation() {
    let root = tempfile::tempdir().unwrap();
    let device = device(root.path()).await;
    let phone = phone::start(
        device.engine.clone(),
        "127.0.0.1".into(),
        SocketAddr::from(([127, 0, 0, 1], 0)),
        root.path().into(),
    )
    .await
    .unwrap();
    let client = client(&phone.hub);
    assert!(
        !post(
            &client,
            &phone.hub,
            "",
            "/mobile/pair",
            json!({"token":"wrong","name":"Attacker"})
        )
        .await
        .status()
        .is_success()
    );
    phone.hub.enrollment.lock().await.expires = 0;
    let token = phone.hub.enrollment.lock().await.token.clone();
    assert!(
        !post(
            &client,
            &phone.hub,
            "",
            "/mobile/pair",
            json!({"token":token,"name":"Phone"})
        )
        .await
        .status()
        .is_success()
    );
    *phone.hub.enrollment.lock().await = Enrollment::default();
    let token = phone.hub.enrollment.lock().await.token.clone();
    let (id, cap) = enroll(&device.engine, &phone.hub, &client, false).await;
    assert!(
        !post(
            &client,
            &phone.hub,
            "",
            "/mobile/pair",
            json!({"token":token,"name":"Replay"})
        )
        .await
        .status()
        .is_success()
    );
    assert!(
        !client
            .get(format!("{}/mobile/state", phone.hub.origin))
            .bearer_auth(&cap)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert_eq!(
        client
            .get(format!("{}/api/state", phone.hub.origin))
            .bearer_auth(&device.engine.ui_token)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert!(
        !post(&client, &phone.hub, &cap, "/mobile/confirm", json!({}))
            .await
            .status()
            .is_success()
    );
    device
        .engine
        .sessions
        .lock()
        .await
        .get_mut(&id)
        .unwrap()
        .approved = true;
    assert!(
        post(&client, &phone.hub, &cap, "/mobile/confirm", json!({}))
            .await
            .status()
            .is_success()
    );
    assert!(
        client
            .get(format!("{}/mobile/state", phone.hub.origin))
            .bearer_auth(&cap)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    device.engine.sessions.lock().await.remove(&id);
    assert!(
        !client
            .get(format!("{}/mobile/state", phone.hub.origin))
            .bearer_auth(&cap)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert!(
        !phone::info(&device.engine)
            .await
            .unwrap()
            .to_string()
            .contains("PRIVATE KEY")
    );
    assert!(phone.hub.profile.contains(&format!(
        "local.drift.phone.{}",
        phone.hub.fingerprint.chars().take(32).collect::<String>()
    )));
}

#[tokio::test]
async fn phone_upload_is_approved_chunked_and_verified() {
    let root = tempfile::tempdir().unwrap();
    let device = device(root.path()).await;
    let phone = phone::start(
        device.engine.clone(),
        "127.0.0.1".into(),
        SocketAddr::from(([127, 0, 0, 1], 0)),
        root.path().into(),
    )
    .await
    .unwrap();
    let client = client(&phone.hub);
    let (_, cap) = enroll(&device.engine, &phone.hub, &client, true).await;
    let bytes = vec![73; 1024 * 1024 + 11];
    let entries = vec![entry("phone/photos.bin", &bytes)];
    let transfer: Transfer = post(&client, &phone.hub, &cap, "/mobile/offer", json!(entries))
        .await
        .json()
        .await
        .unwrap();
    let url = format!(
        "{}/mobile/transfer/{}/0?offset=0",
        phone.hub.origin, transfer.id
    );
    assert!(
        !client
            .put(&url)
            .bearer_auth(&cap)
            .body(vec![1])
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    device
        .engine
        .receive_approval(&transfer.id, true)
        .await
        .unwrap();
    for (index, chunk) in bytes.chunks(1024 * 1024).enumerate() {
        assert!(
            client
                .put(format!(
                    "{}/mobile/transfer/{}/0?offset={}",
                    phone.hub.origin,
                    transfer.id,
                    index * 1024 * 1024
                ))
                .bearer_auth(&cap)
                .body(chunk.to_vec())
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
    }
    assert!(
        post(
            &client,
            &phone.hub,
            &cap,
            &format!("/mobile/transfer/{}/finish", transfer.id),
            json!({})
        )
        .await
        .status()
        .is_success()
    );
    assert_eq!(
        hash_file(
            &root
                .path()
                .join("received")
                .join(&transfer.id)
                .join("phone/photos.bin")
        )
        .await
        .unwrap(),
        entries[0].sha256
    );
}

#[tokio::test]
async fn phone_download_uses_only_offered_files_and_verifies_every_empty_file() {
    let root = tempfile::tempdir().unwrap();
    let device = device(root.path()).await;
    let phone = phone::start(
        device.engine.clone(),
        "127.0.0.1".into(),
        SocketAddr::from(([127, 0, 0, 1], 0)),
        root.path().into(),
    )
    .await
    .unwrap();
    let client = client(&phone.hub);
    let (session, cap) = enroll(&device.engine, &phone.hub, &client, true).await;
    let batch = uuid::Uuid::new_v4().to_string();
    let source = root.path().join("native-selected.txt");
    tokio::fs::write(&source, b"native source, no staging copy")
        .await
        .unwrap();
    let empty = root.path().join("empty");
    tokio::fs::write(&empty, b"").await.unwrap();
    let sources = vec![source.clone(), empty.clone(), empty];
    device
        .engine
        .native_sources
        .lock()
        .await
        .insert(batch.clone(), sources);
    device.engine.uploads.lock().await.insert(
        batch.clone(),
        vec![
            Upload {
                path: "native.txt".into(),
                size: 30,
            },
            Upload {
                path: "empty-one".into(),
                size: 0,
            },
            Upload {
                path: "empty-two".into(),
                size: 0,
            },
        ],
    );
    let id = phone::create_offer(&device.engine, &session, &batch)
        .await
        .unwrap();
    assert!(!root.path().join("outbox").join(batch).exists());
    let url = format!("{}/mobile/download/{id}/0?offset=0", phone.hub.origin);
    assert!(
        !client
            .get(&url)
            .bearer_auth(&cap)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert!(
        post(
            &client,
            &phone.hub,
            &cap,
            &format!("/mobile/receive/{id}"),
            json!({"approve":true})
        )
        .await
        .status()
        .is_success()
    );
    let received = client
        .get(&url)
        .bearer_auth(&cap)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&received[..], b"native source, no staging copy");
    assert!(
        !post(
            &client,
            &phone.hub,
            &cap,
            &format!("/mobile/progress/{id}"),
            json!({"index":0,"offset":30,"sha256":"bad"})
        )
        .await
        .status()
        .is_success()
    );
    let hash = hash_file(&source).await.unwrap();
    let first: Value = post(
        &client,
        &phone.hub,
        &cap,
        &format!("/mobile/progress/{id}"),
        json!({"index":0,"offset":30,"sha256":hash}),
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(first["status"], "receiving");
    let empty_hash = hex::encode(Sha256::digest(b""));
    for index in 1..=2 {
        let response: Value = post(
            &client,
            &phone.hub,
            &cap,
            &format!("/mobile/progress/{id}"),
            json!({"index":index,"offset":0,"sha256":empty_hash}),
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(
            response["status"],
            if index == 2 { "complete" } else { "receiving" }
        );
    }
    *phone.hub.enrollment.lock().await = Enrollment::default();
    let (_, other) = enroll(&device.engine, &phone.hub, &client, true).await;
    assert!(
        !client
            .get(&url)
            .bearer_auth(&other)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
}

#[tokio::test]
async fn native_file_import_requires_an_additional_non_renderer_capability() {
    let root = tempfile::tempdir().unwrap();
    let device = device(root.path()).await;
    let (address, task) = drift_engine::web::serve(
        device.engine.clone(),
        SocketAddr::from(([127, 0, 0, 1], 0)),
        root.path().into(),
    )
    .await
    .unwrap();
    let response = Client::new()
        .post(format!("http://{address}/api/native/batch"))
        .bearer_auth(&device.engine.ui_token)
        .json(&json!([{"path":"secret","source":root.path().join("history.redb")}]))
        .send()
        .await
        .unwrap();
    assert!(!response.status().is_success());
    task.abort();
}

#[tokio::test]
async fn authenticated_phone_cannot_create_unbounded_pending_manifests() {
    let root = tempfile::tempdir().unwrap();
    let device = device(root.path()).await;
    let phone = phone::start(
        device.engine.clone(),
        "127.0.0.1".into(),
        SocketAddr::from(([127, 0, 0, 1], 0)),
        root.path().into(),
    )
    .await
    .unwrap();
    let client = client(&phone.hub);
    let (_, cap) = enroll(&device.engine, &phone.hub, &client, true).await;
    let mut first = String::new();
    for index in 0..32 {
        let response = post(
            &client,
            &phone.hub,
            &cap,
            "/mobile/offer",
            json!([entry(&format!("pending-{index}"), b"")]),
        )
        .await;
        assert!(response.status().is_success());
        let transfer: Transfer = response.json().await.unwrap();
        if index == 0 {
            first = transfer.id;
        }
    }
    let additional = json!([entry("over-quota", b"")]);
    assert!(
        !post(
            &client,
            &phone.hub,
            &cap,
            "/mobile/offer",
            additional.clone()
        )
        .await
        .status()
        .is_success()
    );
    device.engine.receive_approval(&first, false).await.unwrap();
    assert!(
        post(&client, &phone.hub, &cap, "/mobile/offer", additional)
            .await
            .status()
            .is_success()
    );
}
