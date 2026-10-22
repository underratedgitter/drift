//! Remembered devices and visible delivery, driven through the same control API
//! the app uses, between two real engines over TLS on loopback.
use axum::body::Body;
use drift_engine::{Options, Running, Shared, model::Transfer, selection, web::app_router};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};
use tower::ServiceExt;

async fn launch(root: &Path, name: &str, inbox: Option<PathBuf>) -> Running {
    drift_engine::launch(Options {
        root: root.to_owned(),
        name: name.into(),
        host: "127.0.0.1".into(),
        bind: SocketAddr::from(([127, 0, 0, 1], 0)),
        discover: false,
        inbox,
    })
    .await
    .unwrap()
}

async fn call(engine: &Shared, path: &str, body: Option<Value>) -> Result<Value, String> {
    let request = axum::http::Request::builder()
        .method(if body.is_some() { "POST" } else { "GET" })
        .uri(format!("/api{path}"))
        .header("authorization", format!("Bearer {}", engine.ui_token))
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap();
    let response = app_router(engine.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if status.is_success() {
        Ok(value)
    } else {
        Err(value["error"]
            .as_str()
            .unwrap_or("request failed")
            .to_owned())
    }
}

async fn state(engine: &Shared) -> Value {
    call(engine, "/state", None).await.unwrap()
}

async fn eventually<F: AsyncFnMut() -> bool>(what: &str, mut check: F) {
    for _ in 0..200 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

/// `from` pairs with `to`; `to` approves (as its own device unless `own` is false).
async fn pair(from: &Shared, to: &Shared, own: bool) {
    let invitation = state(to).await["invitation"].clone();
    call(from, "/pair", Some(invitation)).await.unwrap();
    let pending = state(to).await["sessions"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        to,
        &format!("/session/{pending}"),
        Some(json!({"approve": true, "own": own})),
    )
    .await
    .unwrap();
    eventually("pairing to complete", async || {
        state(from).await["connections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|device| device["remembered"] == true)
    })
    .await;
}

fn device_id(snapshot: &Value) -> String {
    snapshot["connections"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn send(from: &Shared, to_device: &str, files: &[PathBuf]) -> String {
    let sources = selection::collect(files).unwrap();
    let batch = selection::select(from, sources).await.unwrap();
    call(
        from,
        "/send",
        Some(json!({"connection": to_device, "batch": batch.id})),
    )
    .await
    .unwrap();
    batch.id
}

async fn completed(engine: &Shared, direction: &str, count: usize) -> Vec<Transfer> {
    let mut done = Vec::new();
    eventually("transfers to complete", async || {
        done = engine
            .store
            .list()
            .unwrap()
            .into_iter()
            .filter(|t| t.direction == direction && t.status == "complete")
            .collect();
        done.len() >= count
    })
    .await;
    done
}

#[tokio::test]
async fn devices_stay_paired_both_ways_across_restarts_and_own_files_arrive_without_asking() {
    let (laptop_root, phone_root, inbox, sources) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let photo = sources.path().join("photo.jpg");
    std::fs::write(&photo, b"first photo").unwrap();
    let laptop = launch(
        laptop_root.path(),
        "Laptop",
        Some(inbox.path().join("Drift")),
    )
    .await;
    let phone = launch(phone_root.path(), "Phone", None).await;

    // The phone starts pairing; the laptop approves once. No confirmation step.
    pair(&phone.engine, &laptop.engine, true).await;
    let laptop_view = state(&laptop.engine).await;
    let phone_view = state(&phone.engine).await;
    assert_eq!(laptop_view["connections"][0]["name"], "Phone");
    assert_eq!(phone_view["connections"][0]["name"], "Laptop");
    assert_eq!(device_id(&phone_view), laptop.engine.fingerprint);
    assert_eq!(device_id(&laptop_view), phone.engine.fingerprint);
    assert!(laptop_view["sessions"].as_array().unwrap().is_empty());

    // Phone → laptop: arrives without a second approval, under its own name.
    send(
        &phone.engine,
        &laptop.engine.fingerprint,
        std::slice::from_ref(&photo),
    )
    .await;
    completed(&laptop.engine, "receive", 1).await;
    let saved = inbox.path().join("Drift/photo.jpg");
    assert_eq!(std::fs::read(&saved).unwrap(), b"first photo");
    let view = state(&laptop.engine).await;
    let id = view["transfers"][0]["id"].as_str().unwrap();
    assert_eq!(view["saved"][id], json!([saved.display().to_string()]));

    // Laptop → phone over the same pairing.
    let notes = sources.path().join("notes.txt");
    std::fs::write(&notes, b"from the laptop").unwrap();
    send(&laptop.engine, &phone.engine.fingerprint, &[notes]).await;
    let received = completed(&phone.engine, "receive", 1).await;
    let published = phone_root
        .path()
        .join("received")
        .join(&received[0].id)
        .join("notes.txt");
    assert_eq!(std::fs::read(published).unwrap(), b"from the laptop");

    // Both restart: still paired, same identities and ports, and sending again
    // delivers a second, numbered copy instead of overwriting the first.
    let laptop_port = laptop.address.port();
    laptop.stop().await;
    phone.stop().await;
    let laptop = launch(
        laptop_root.path(),
        "Laptop",
        Some(inbox.path().join("Drift")),
    )
    .await;
    let phone = launch(phone_root.path(), "Phone", None).await;
    assert_eq!(laptop.address.port(), laptop_port);
    let remembered = state(&phone.engine).await;
    assert_eq!(remembered["connections"][0]["remembered"], true);
    assert_eq!(device_id(&remembered), laptop.engine.fingerprint);
    std::fs::write(&photo, b"second photo").unwrap();
    send(&phone.engine, &laptop.engine.fingerprint, &[photo]).await;
    completed(&laptop.engine, "receive", 2).await;
    assert_eq!(std::fs::read(&saved).unwrap(), b"first photo");
    assert_eq!(
        std::fs::read(inbox.path().join("Drift/photo (2).jpg")).unwrap(),
        b"second photo"
    );
}

#[tokio::test]
async fn files_from_other_peoples_devices_wait_for_approval() {
    let (a, b, files) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let receiver = launch(a.path(), "Receiver", None).await;
    let friend = launch(b.path(), "Friend", None).await;
    pair(&friend.engine, &receiver.engine, false).await;
    assert_eq!(
        state(&receiver.engine).await["connections"][0]["own"],
        false
    );
    let file = files.path().join("shared.txt");
    std::fs::write(&file, b"hello").unwrap();
    send(&friend.engine, &receiver.engine.fingerprint, &[file]).await;
    let mut pending = String::new();
    eventually("the offer to arrive", async || {
        let list = receiver.engine.store.list().unwrap();
        pending = list.first().map(|t| t.status.clone()).unwrap_or_default();
        !pending.is_empty()
    })
    .await;
    assert_eq!(pending, "pending");
    let id = receiver.engine.store.list().unwrap()[0].id.clone();
    call(
        &receiver.engine,
        &format!("/receive/{id}"),
        Some(json!({"approve": true})),
    )
    .await
    .unwrap();
    completed(&receiver.engine, "receive", 1).await;
}

#[tokio::test]
async fn declining_or_forgetting_removes_the_pairing_on_both_sides() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let laptop = launch(a.path(), "Laptop", None).await;
    let phone = launch(b.path(), "Phone", None).await;

    let invitation = state(&laptop.engine).await["invitation"].clone();
    call(&phone.engine, "/pair", Some(invitation))
        .await
        .unwrap();
    let pending = state(&laptop.engine).await["sessions"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        &laptop.engine,
        &format!("/session/{pending}"),
        Some(json!({"approve": false})),
    )
    .await
    .unwrap();
    eventually("the decline to reach the phone", async || {
        state(&phone.engine).await["connections"]
            .as_array()
            .unwrap()
            .is_empty()
    })
    .await;
    let message = state(&phone.engine).await["lastError"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(message.contains("declined"), "{message}");
    assert!(laptop.engine.store.devices().unwrap().is_empty());

    pair(&phone.engine, &laptop.engine, true).await;
    call(
        &phone.engine,
        &format!("/devices/{}/forget", laptop.engine.fingerprint),
        Some(json!({})),
    )
    .await
    .unwrap();
    assert!(phone.engine.store.devices().unwrap().is_empty());
    eventually("the other side to forget too", async || {
        laptop.engine.store.devices().unwrap().is_empty()
    })
    .await;
    assert!(
        state(&laptop.engine).await["connections"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn folders_and_many_files_are_delivered_with_readable_names() {
    let (a, b, inbox, files) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let laptop = launch(a.path(), "Laptop", Some(inbox.path().to_owned())).await;
    let phone = launch(b.path(), "Phone", None).await;
    pair(&phone.engine, &laptop.engine, true).await;
    let folder = files.path().join("Trip");
    std::fs::create_dir_all(folder.join("day 2")).unwrap();
    std::fs::write(folder.join("a.jpg"), b"a").unwrap();
    std::fs::write(folder.join("day 2/b.jpg"), b"b").unwrap();
    std::fs::write(folder.join("empty.txt"), b"").unwrap();
    let loose = files.path().join("readme.md");
    std::fs::write(&loose, b"r").unwrap();
    // Someone's existing folder of the same name is never merged into or replaced.
    std::fs::create_dir(inbox.path().join("Trip")).unwrap();
    std::fs::write(inbox.path().join("Trip/a.jpg"), b"theirs").unwrap();

    send(&phone.engine, &laptop.engine.fingerprint, &[folder, loose]).await;
    let done = completed(&laptop.engine, "receive", 1).await;
    assert_eq!(
        std::fs::read(inbox.path().join("Trip/a.jpg")).unwrap(),
        b"theirs"
    );
    assert_eq!(
        std::fs::read(inbox.path().join("Trip (2)/a.jpg")).unwrap(),
        b"a"
    );
    assert_eq!(
        std::fs::read(inbox.path().join("Trip (2)/day 2/b.jpg")).unwrap(),
        b"b"
    );
    assert_eq!(
        std::fs::metadata(inbox.path().join("Trip (2)/empty.txt"))
            .unwrap()
            .len(),
        0
    );
    assert_eq!(std::fs::read(inbox.path().join("readme.md")).unwrap(), b"r");
    // The hidden published copy is gone; only the empty marker remains.
    let marker = a.path().join("received").join(&done[0].id);
    assert_eq!(std::fs::read_dir(&marker).unwrap().count(), 0);
    // Recovery copies go right after completion is committed.
    let partial = a.path().join("partial").join(&done[0].id);
    eventually("recovery copies to be removed", async || !partial.exists()).await;
}

#[tokio::test]
async fn tapping_a_nearby_device_pairs_after_both_screens_show_the_same_code() {
    let (a, b, files) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let studio = launch(a.path(), "Studio Mac", None).await;
    let laptop = launch(b.path(), "MacBook Air", None).await;
    let nearby = json!({"host": "127.0.0.1", "port": studio.address.port(), "id": studio.engine.fingerprint});

    // An advertised identity that does not match the device that answers.
    let wrong = json!({"host": "127.0.0.1", "port": studio.address.port(), "id": "a".repeat(64)});
    let refused = call(&laptop.engine, "/introduce", Some(wrong))
        .await
        .unwrap_err();
    assert!(refused.contains("different identity"), "{refused}");

    let started = call(&laptop.engine, "/introduce", Some(nearby))
        .await
        .unwrap();
    let code = started["code"].as_str().unwrap().to_owned();
    assert_eq!(code.len(), 7);
    assert_eq!(started["name"], "Studio Mac");
    let waiting = state(&laptop.engine).await;
    assert_eq!(waiting["connections"][0]["code"], code.as_str());
    assert_eq!(waiting["connections"][0]["confirmed"], false);
    let asking = state(&studio.engine).await;
    let sessions = asking["sessions"].as_array().unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "the refused attempt must not leave a request behind"
    );
    assert_eq!(sessions[0]["name"], "MacBook Air");
    assert_eq!(sessions[0]["code"], code.as_str());

    let pending = sessions[0]["id"].as_str().unwrap();
    call(
        &studio.engine,
        &format!("/session/{pending}"),
        Some(json!({"approve": true, "own": true})),
    )
    .await
    .unwrap();
    eventually("the laptop to remember the studio", async || {
        state(&laptop.engine).await["connections"][0]["remembered"] == true
    })
    .await;
    let file = files.path().join("cut.mov");
    std::fs::write(&file, b"final cut").unwrap();
    send(&laptop.engine, &studio.engine.fingerprint, &[file]).await;
    completed(&studio.engine, "receive", 1).await;
}

#[tokio::test]
async fn a_reveal_that_does_not_match_its_commitment_is_rejected_once_and_for_all() {
    let root = tempfile::tempdir().unwrap();
    let studio = launch(root.path(), "Studio Mac", None).await;
    let base = format!("https://127.0.0.1:{}", studio.address.port());
    // Test-only client: the server certificate is irrelevant to this check.
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .build()
        .unwrap();
    let hello =
        json!({"id": "b".repeat(64), "port": 1, "capability": "c".repeat(64), "os": "macos"});
    let commitment = drift_engine::pairing::commitment("the real nonce");
    let introduced: Value = client
        .post(format!("{base}/introduce"))
        .json(&json!({"name": "Impostor", "device": hello, "commitment": commitment}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = introduced["id"].as_str().unwrap();
    let forged = client
        .post(format!("{base}/introduce/{id}/reveal"))
        .json(&json!({"nonce": "a chosen nonce"}))
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), 400);
    // The request is spent: even the right nonce cannot be tried afterwards.
    let replay = client
        .post(format!("{base}/introduce/{id}/reveal"))
        .json(&json!({"nonce": "the real nonce"}))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 400);
    assert!(
        state(&studio.engine).await["sessions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
