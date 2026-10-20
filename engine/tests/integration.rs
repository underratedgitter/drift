use drift_engine::{
    model::{CHUNK, Entry, Invitation, PairRequest, Transfer, manifest_id},
    pairing::{Pairing, proof, secret},
    storage::{Store, hash_file, safe_path, validate},
    transport::Connection,
};
use sha2::{Digest, Sha256};
use std::{net::SocketAddr, path::Path};

fn entry(path: &str, bytes: &[u8]) -> Entry {
    Entry {
        path: path.into(),
        size: bytes.len() as u64,
        sha256: hex::encode(Sha256::digest(bytes)),
    }
}
async fn device(root: &Path) -> drift_engine::Running {
    drift_engine::start(
        root.to_owned(),
        "Test device".into(),
        "127.0.0.1".into(),
        SocketAddr::from(([127, 0, 0, 1], 0)),
        false,
    )
    .await
    .unwrap()
}
async fn paired(device: &drift_engine::Running) -> Connection {
    let invitation = device.engine.pairing.lock().await.invitation.clone();
    let connection = Connection::pair(&invitation, "Sender").await.unwrap();
    device
        .engine
        .sessions
        .lock()
        .await
        .get_mut(&connection.session)
        .unwrap()
        .approved = true;
    connection
        .post::<_, drift_engine::model::Session>("/confirm", &())
        .await
        .unwrap();
    connection
}
async fn offer(
    device: &drift_engine::Running,
    connection: &Connection,
    entries: &[Entry],
) -> Transfer {
    let transfer: Transfer = connection.post("/offer", &entries).await.unwrap();
    device
        .engine
        .receive_approval(&transfer.id, true)
        .await
        .unwrap();
    transfer
}

#[test]
fn pairing_expiry_replay_and_certificate_binding() {
    let mut pairing = Pairing::new("127.0.0.1".into(), 1);
    let nonce = secret(32);
    let code = pairing.invitation.code.clone();
    let server = pairing.challenge("certificate-A", &nonce).unwrap();
    assert!(
        drift_engine::pairing::verify(&code, "server", "certificate-B", &nonce, "", &server)
            .is_err()
    );
    let request = PairRequest {
        nonce: nonce.clone(),
        name: "Laptop".into(),
        proof: proof(&code, "client", "certificate-A", &nonce, "Laptop"),
    };
    pairing.consume("certificate-A", &request).unwrap();
    assert!(pairing.consume("certificate-A", &request).is_err());
    assert!(pairing.challenge("certificate-A", &secret(32)).is_err());
    let mut expired = Pairing::new("127.0.0.1".into(), 1);
    expired.invitation.expires = 0;
    assert!(expired.challenge("certificate", &secret(32)).is_err());
}

#[test]
fn unsafe_filenames_and_conflicts_are_rejected() {
    for path in [
        "../secret",
        "/etc/passwd",
        "a/../../b",
        "a\\b",
        "a:b",
        "a//b",
        "CON",
        "x/NUL.txt",
        "COM9",
        "a.",
        "x/.",
        "x/..",
        "x/\0",
        "a/hello?",
        "LPT1.txt",
    ] {
        assert!(safe_path(path).is_err(), "accepted {path}");
    }
    assert!(validate(&[entry("A.txt", b"a"), entry("a.txt", b"b")]).is_err());
    assert!(validate(&[entry("folder", b"a"), entry("folder/file", b"b")]).is_err());
    assert!(
        validate(&[
            entry("folder/photo.jpg", b"a"),
            entry("other/photo.jpg", b"b")
        ])
        .is_ok()
    );
}

#[tokio::test]
async fn tls_pairing_requires_both_approvals_and_rejects_wrong_code() {
    let root = tempfile::tempdir().unwrap();
    let receiver = device(root.path()).await;
    let mut invitation = receiver.engine.pairing.lock().await.invitation.clone();
    invitation.code = secret(15);
    assert!(Connection::pair(&invitation, "Attacker").await.is_err());
    let invitation = receiver.engine.pairing.lock().await.invitation.clone();
    let connection = Connection::pair(&invitation, "Sender").await.unwrap();
    assert!(
        connection
            .post::<_, Transfer>("/offer", &vec![entry("file", b"x")])
            .await
            .is_err()
    );
    assert!(
        connection
            .post::<_, drift_engine::model::Session>("/confirm", &())
            .await
            .is_err()
    );
    receiver
        .engine
        .sessions
        .lock()
        .await
        .get_mut(&connection.session)
        .unwrap()
        .approved = true;
    assert!(
        connection
            .post::<_, Transfer>("/offer", &vec![entry("file", b"x")])
            .await
            .is_err()
    );
    connection
        .post::<_, drift_engine::model::Session>("/confirm", &())
        .await
        .unwrap();
    let transfer: Transfer = connection
        .post("/offer", &vec![entry("file", b"x")])
        .await
        .unwrap();
    assert!(
        connection
            .chunk(&transfer.id, 0, 0, vec![b'x'])
            .await
            .is_err()
    );
    assert!(Connection::pair(&invitation, "Replay").await.is_err());
}

#[tokio::test]
async fn real_tls_folder_transfer_verifies_hash_and_rejects_bad_offsets() {
    let root = tempfile::tempdir().unwrap();
    let receiver = device(root.path()).await;
    let connection = paired(&receiver).await;
    let bytes = vec![0xA5; CHUNK * 3 + 7];
    let entries = vec![
        entry("photos/holiday.bin", &bytes),
        entry("empty.txt", b""),
        entry("other/holiday.bin", b"different"),
    ];
    let transfer = offer(&receiver, &connection, &entries).await;
    assert!(connection.chunk(&transfer.id, 0, 1, vec![1]).await.is_err());
    for (index, chunk) in bytes.chunks(CHUNK).enumerate() {
        connection
            .chunk(&transfer.id, 0, (index * CHUNK) as u64, chunk.to_vec())
            .await
            .unwrap();
    }
    connection
        .chunk(&transfer.id, 2, 0, b"different".to_vec())
        .await
        .unwrap();
    let finished: Transfer = connection
        .post(&format!("/transfer/{}/finish", transfer.id), &())
        .await
        .unwrap();
    assert_eq!(finished.status, "complete");
    let path = root
        .path()
        .join("received")
        .join(&transfer.id)
        .join("photos/holiday.bin");
    assert_eq!(hash_file(&path).await.unwrap(), entries[0].sha256);
    assert_eq!(
        std::fs::metadata(
            root.path()
                .join("received")
                .join(&transfer.id)
                .join("empty.txt")
        )
        .unwrap()
        .len(),
        0
    );
    assert!(connection.chunk(&transfer.id, 0, 0, vec![1]).await.is_err());
}

#[tokio::test]
async fn integrity_failure_never_publishes_files() {
    let root = tempfile::tempdir().unwrap();
    let receiver = device(root.path()).await;
    let connection = paired(&receiver).await;
    let transfer = offer(&receiver, &connection, &[entry("safe.bin", b"correct")]).await;
    connection
        .chunk(&transfer.id, 0, 0, b"corrupt".to_vec())
        .await
        .unwrap();
    assert!(
        connection
            .post::<_, Transfer>(&format!("/transfer/{}/finish", transfer.id), &())
            .await
            .is_err()
    );
    assert!(!root.path().join("received").join(&transfer.id).exists());
}

#[tokio::test]
async fn clearing_history_and_resending_never_truncates_published_hard_links() {
    let root = tempfile::tempdir().unwrap();
    let receiver = device(root.path()).await;
    let connection = paired(&receiver).await;
    let bytes = b"keep the original bytes";
    let entries = vec![entry("original.txt", bytes)];
    let transfer = offer(&receiver, &connection, &entries).await;
    connection
        .chunk(&transfer.id, 0, 0, bytes.to_vec())
        .await
        .unwrap();
    connection
        .post::<_, Transfer>(&format!("/transfer/{}/finish", transfer.id), &())
        .await
        .unwrap();
    receiver.engine.store.clear_history().unwrap();
    // Sending the same files again after the first copy finished delivers a
    // separate second copy; it never writes into the already-published files.
    let again = offer(&receiver, &connection, &entries).await;
    assert_eq!(again.id, format!("{}-2", transfer.id));
    assert_eq!(again.offsets[0], 0);
    connection
        .chunk(&again.id, 0, 0, bytes.to_vec())
        .await
        .unwrap();
    let finished: Transfer = connection
        .post(&format!("/transfer/{}/finish", again.id), &())
        .await
        .unwrap();
    assert_eq!(finished.status, "complete");
    assert_eq!(
        tokio::fs::read(
            receiver
                .engine
                .store
                .root
                .join("received")
                .join(&again.id)
                .join("original.txt")
        )
        .await
        .unwrap(),
        bytes
    );
    assert_eq!(
        tokio::fs::read(
            root.path()
                .join("received")
                .join(&transfer.id)
                .join("original.txt")
        )
        .await
        .unwrap(),
        bytes
    );
}

#[tokio::test]
async fn pause_retry_and_session_isolation() {
    let root = tempfile::tempdir().unwrap();
    let receiver = device(root.path()).await;
    let connection = paired(&receiver).await;
    let data = vec![23; CHUNK + 16];
    let entries = vec![entry("file", &data)];
    let transfer = offer(&receiver, &connection, &entries).await;
    connection
        .chunk(&transfer.id, 0, 0, data[..CHUNK].to_vec())
        .await
        .unwrap();
    let mut record = receiver.engine.store.get(&transfer.id).unwrap();
    record.status = "paused".into();
    receiver.engine.store.save(&record).unwrap();
    assert!(
        connection
            .chunk(&transfer.id, 0, CHUNK as u64, data[CHUNK..].to_vec())
            .await
            .is_err()
    );
    let resumed: Transfer = connection.post("/offer", &entries).await.unwrap();
    assert_eq!(resumed.offsets[0], CHUNK as u64);
    receiver
        .engine
        .receive_approval(&transfer.id, true)
        .await
        .unwrap();
    connection
        .chunk(&transfer.id, 0, CHUNK as u64, data[CHUNK..].to_vec())
        .await
        .unwrap();
    connection
        .post::<_, Transfer>(&format!("/transfer/{}/finish", transfer.id), &())
        .await
        .unwrap();
    *receiver.engine.pairing.lock().await =
        Pairing::new("127.0.0.1".into(), receiver.address.port());
    let other = paired(&receiver).await;
    assert!(
        other
            .get::<Transfer>(&format!("/transfer/{}", transfer.id))
            .await
            .is_err()
    );
    // The finished transfer stays private to its session, but the same files
    // offered again arrive as a separate new copy.
    let copy: Transfer = other.post("/offer", &entries).await.unwrap();
    assert_ne!(copy.id, transfer.id);
    assert!(copy.offsets.iter().all(|offset| *offset == 0));
}

#[tokio::test]
async fn another_session_cannot_take_over_an_unfinished_transfer() {
    let root = tempfile::tempdir().unwrap();
    let receiver = device(root.path()).await;
    let connection = paired(&receiver).await;
    let bytes = b"half of this arrives before the sender disappears";
    let entries = vec![entry("half.txt", bytes)];
    let transfer = offer(&receiver, &connection, &entries).await;
    connection
        .chunk(&transfer.id, 0, 0, bytes[..4].to_vec())
        .await
        .unwrap();
    let other = paired(&receiver).await;
    let error = other
        .post::<_, Transfer>("/offer", &entries)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("belongs to another session"),
        "{error}"
    );
}

#[tokio::test]
async fn persistent_offsets_survive_storage_restart() {
    let root = tempfile::tempdir().unwrap();
    let data = b"persistent streaming payload";
    let entries = vec![entry("folder/file", data)];
    let id = manifest_id(&entries);
    {
        let store = Store::open(root.path().to_owned()).unwrap();
        let mut transfer = Transfer {
            id: id.clone(),
            entries: entries.clone(),
            offsets: vec![0],
            status: "receiving".into(),
            peer: "peer".into(),
            updated: 0,
            error: None,
            direction: "receive".into(),
        };
        store
            .write_chunk(&mut transfer, 0, 0, &data[..10])
            .await
            .unwrap();
    }
    let store = Store::open(root.path().to_owned()).unwrap();
    let mut transfer = store.get(&id).unwrap();
    assert_eq!(transfer.offsets[0], 10);
    store
        .write_chunk(&mut transfer, 0, 10, &data[10..])
        .await
        .unwrap();
    store.finish(&mut transfer).await.unwrap();
    assert_eq!(
        tokio::fs::read(root.path().join("received").join(id).join("folder/file"))
            .await
            .unwrap(),
        data
    );
}

#[tokio::test]
async fn unauthenticated_network_and_browser_requests_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let receiver = device(root.path()).await;
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .build()
        .unwrap();
    let response = client
        .get(format!("https://{}/session", receiver.address))
        .send()
        .await
        .unwrap();
    assert!(!response.status().is_success());
    let (address, task) = drift_engine::web::serve(
        receiver.engine.clone(),
        SocketAddr::from(([127, 0, 0, 1], 0)),
        root.path().to_owned(),
    )
    .await
    .unwrap();
    assert_eq!(
        reqwest::get(format!("http://{address}/api/state"))
            .await
            .unwrap()
            .status(),
        401
    );
    let response = reqwest::Client::new()
        .get(format!("http://{address}/api/state"))
        .bearer_auth(&receiver.engine.ui_token)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    task.abort();
}

#[tokio::test]
async fn send_engine_streams_to_a_second_device() {
    let sender_dir = tempfile::tempdir().unwrap();
    let receiver_dir = tempfile::tempdir().unwrap();
    let sender = device(sender_dir.path()).await;
    let receiver = device(receiver_dir.path()).await;
    let connection = paired(&receiver).await;
    let connection_id = connection.session.clone();
    sender
        .engine
        .connections
        .lock()
        .await
        .insert(connection_id.clone(), connection);
    let batch = uuid::Uuid::new_v4().to_string();
    let bytes = vec![42; CHUNK * 8 + 1];
    let path = sender_dir.path().join("outbox").join(&batch);
    tokio::fs::create_dir_all(&path).await.unwrap();
    tokio::fs::write(path.join("0"), &bytes).await.unwrap();
    sender.engine.uploads.lock().await.insert(
        batch.clone(),
        vec![drift_engine::Upload {
            path: "large/file.bin".into(),
            size: bytes.len() as u64,
        }],
    );
    let engine = sender.engine.clone();
    let start = std::time::Instant::now();
    let sending = tokio::spawn(async move { engine.send(&connection_id, &batch).await });
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Some(transfer) = receiver
                .engine
                .store
                .list()
                .unwrap()
                .into_iter()
                .find(|t| t.status == "pending")
            {
                receiver
                    .engine
                    .receive_approval(&transfer.id, true)
                    .await
                    .unwrap();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), sending)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(sender.engine.store.get(&result).unwrap().status, "complete");
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "Loopback debug build, including hashing/approval/verification: {} bytes in {:.3}s ({:.2} MiB/s); NOT a Wi-Fi or AirDrop comparison",
        bytes.len(),
        elapsed,
        bytes.len() as f64 / elapsed / 1048576.0
    );
}

#[test]
fn invitation_qr_round_trip_and_rate_limit() {
    let mut pairing = Pairing::new("192.168.1.5".into(), 9518);
    let serialized = serde_json::to_string(&pairing.invitation).unwrap();
    let decoded: Invitation = serde_json::from_str(&serialized).unwrap();
    assert_eq!(decoded.code, pairing.invitation.code);
    for _ in 0..60 {
        pairing.challenge("fingerprint", &secret(32)).unwrap();
    }
    assert!(pairing.challenge("fingerprint", &secret(32)).is_err());
}

#[tokio::test]
async fn native_pipeline_resumes_from_receiver_without_sender_staging() {
    let sender_dir = tempfile::tempdir().unwrap();
    let receiver_dir = tempfile::tempdir().unwrap();
    let sender = device(sender_dir.path()).await;
    let receiver = device(receiver_dir.path()).await;
    let connection = paired(&receiver).await;
    let bytes = vec![0x73; CHUNK + 37];
    let manifest = vec![entry("resume.bin", &bytes)];
    let partial = offer(&receiver, &connection, &manifest).await;
    connection
        .chunk(&partial.id, 0, 0, bytes[..CHUNK].to_vec())
        .await
        .unwrap();
    assert_eq!(
        receiver.engine.store.get(&partial.id).unwrap().offsets,
        vec![CHUNK as u64]
    );
    // A fresh sender has no progress record. Only the receiver's durable offset
    // is authoritative; source reads must begin there without an outbox copy.
    let source = sender_dir.path().join("original.bin");
    tokio::fs::write(&source, &bytes).await.unwrap();
    let batch = uuid::Uuid::new_v4().to_string();
    sender
        .engine
        .native_sources
        .lock()
        .await
        .insert(batch.clone(), vec![source]);
    sender.engine.uploads.lock().await.insert(
        batch.clone(),
        vec![drift_engine::Upload {
            path: "resume.bin".into(),
            size: bytes.len() as u64,
        }],
    );
    let id = connection.session.clone();
    sender
        .engine
        .connections
        .lock()
        .await
        .insert(id.clone(), connection);
    let engine = sender.engine.clone();
    let sending = tokio::spawn(async move { engine.send(&id, &batch).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if receiver.engine.store.get(&partial.id).unwrap().status == "pending" {
                receiver
                    .engine
                    .receive_approval(&partial.id, true)
                    .await
                    .unwrap();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), sending)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        tokio::fs::read(
            receiver_dir
                .path()
                .join("received")
                .join(partial.id)
                .join("resume.bin")
        )
        .await
        .unwrap(),
        bytes
    );
    assert!(!sender_dir.path().join("outbox").exists());
}

#[tokio::test]
async fn lan_pairing_rejects_public_addresses_without_a_relay_request() {
    let pairing = Pairing::new("8.8.8.8".into(), 9518);
    let error = Connection::pair(&pairing.invitation, "LAN test")
        .await
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("LAN mode accepts local addresses only")
    );
}

#[tokio::test]
async fn durable_windows_enforce_approval_offsets_limits_and_legacy_compatibility() {
    let root = tempfile::tempdir().unwrap();
    let receiver = device(root.path()).await;
    let connection = paired(&receiver).await;
    let window = drift_engine::model::WINDOW;
    let payload = axum::body::Bytes::from(vec![0x62; window]);
    let entries = vec![entry("window.bin", &payload)];
    let transfer: Transfer = connection.post("/offer", &entries).await.unwrap();
    assert!(
        connection
            .window_chunk(&transfer.id, 0, 0, payload.clone())
            .await
            .is_err()
    );
    receiver
        .engine
        .receive_approval(&transfer.id, true)
        .await
        .unwrap();
    // The legacy route must still reject bodies beyond its original 8 MiB limit.
    assert!(
        connection
            .chunk(&transfer.id, 0, 0, payload.clone())
            .await
            .is_err()
    );
    assert!(
        connection
            .window_chunk(
                &transfer.id,
                0,
                0,
                axum::body::Bytes::from(vec![0; window + 1])
            )
            .await
            .is_err()
    );
    assert_eq!(
        receiver.engine.store.get(&transfer.id).unwrap().offsets,
        vec![0]
    );
    connection
        .window_chunk(&transfer.id, 0, 0, payload.clone())
        .await
        .unwrap();
    assert_eq!(
        receiver.engine.store.get(&transfer.id).unwrap().offsets,
        vec![window as u64]
    );
    assert!(
        connection
            .window_chunk(&transfer.id, 0, 0, payload.clone())
            .await
            .is_err()
    );
    {
        let mut pairing = receiver.engine.pairing.lock().await;
        *pairing = Pairing::new(pairing.invitation.host.clone(), pairing.invitation.port);
    }
    let stranger = paired(&receiver).await;
    assert!(
        stranger
            .window_chunk(&transfer.id, 0, 0, payload.clone())
            .await
            .is_err()
    );
    let finished: Transfer = connection
        .post(&format!("/transfer/{}/finish", transfer.id), &())
        .await
        .unwrap();
    assert_eq!(finished.status, "complete");
    assert_eq!(
        tokio::fs::read(
            root.path()
                .join("received")
                .join(transfer.id)
                .join("window.bin")
        )
        .await
        .unwrap(),
        payload.as_ref()
    );
}

#[tokio::test]
async fn pipelined_send_pauses_mid_file_and_resumes_from_durable_receiver() {
    let source_root = tempfile::tempdir().unwrap();
    let destination_root = tempfile::tempdir().unwrap();
    let sender = device(source_root.path()).await;
    let receiver = device(destination_root.path()).await;
    let connection = paired(&receiver).await;
    let connection_id = connection.session.clone();
    sender
        .engine
        .connections
        .lock()
        .await
        .insert(connection_id.clone(), connection);
    let payload = vec![0x59; CHUNK * 8 + 19];
    let source = source_root.path().join("source.bin");
    tokio::fs::write(&source, &payload).await.unwrap();
    let batch = uuid::Uuid::new_v4().to_string();
    sender
        .engine
        .native_sources
        .lock()
        .await
        .insert(batch.clone(), vec![source]);
    sender.engine.uploads.lock().await.insert(
        batch.clone(),
        vec![drift_engine::Upload {
            path: "pipeline.bin".into(),
            size: payload.len() as u64,
        }],
    );
    let remote_id = manifest_id(&[entry("pipeline.bin", &payload)]);
    let engine = sender.engine.clone();
    let selected = connection_id.clone();
    let selected_batch = batch.clone();
    let sending = tokio::spawn(async move { engine.send(&selected, &selected_batch).await });
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if receiver
                .engine
                .store
                .get(&remote_id)
                .is_ok_and(|transfer| transfer.status == "pending")
            {
                receiver
                    .engine
                    .receive_approval(&remote_id, true)
                    .await
                    .unwrap();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        loop {
            let transfer = receiver.engine.store.get(&remote_id).unwrap();
            if transfer.offsets[0] >= CHUNK as u64 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        sender
            .engine
            .send_controls
            .lock()
            .unwrap()
            .insert(format!("send-{remote_id}"), "paused".into());
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(30), sending)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    let checkpoint = receiver.engine.store.get(&remote_id).unwrap();
    assert_eq!(checkpoint.status, "paused");
    assert!(checkpoint.offsets[0] < payload.len() as u64);
    assert!(
        !destination_root
            .path()
            .join("received")
            .join(&remote_id)
            .exists()
    );
    let engine = sender.engine.clone();
    let sending = tokio::spawn(async move { engine.send(&connection_id, &batch).await });
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if receiver.engine.store.get(&remote_id).unwrap().status == "pending" {
                receiver
                    .engine
                    .receive_approval(&remote_id, true)
                    .await
                    .unwrap();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), sending)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        tokio::fs::read(
            destination_root
                .path()
                .join("received")
                .join(remote_id)
                .join("pipeline.bin")
        )
        .await
        .unwrap(),
        payload
    );
}

#[tokio::test]
async fn simultaneous_transfers_remain_isolated() {
    let root = tempfile::tempdir().unwrap();
    let receiver = device(root.path()).await;
    let connection = paired(&receiver).await;
    let first = offer(&receiver, &connection, &[entry("one.txt", b"one")]).await;
    let second = offer(&receiver, &connection, &[entry("two.txt", b"two")]).await;
    let (a, b) = tokio::join!(
        connection.chunk(&first.id, 0, 0, b"one".to_vec()),
        connection.chunk(&second.id, 0, 0, b"two".to_vec())
    );
    a.unwrap();
    b.unwrap();
    let first_path = format!("/transfer/{}/finish", first.id);
    let second_path = format!("/transfer/{}/finish", second.id);
    let (a, b) = tokio::join!(
        connection.post::<_, Transfer>(&first_path, &()),
        connection.post::<_, Transfer>(&second_path, &())
    );
    assert_eq!(a.unwrap().status, "complete");
    assert_eq!(b.unwrap().status, "complete");
}

#[tokio::test]
async fn disconnection_repair_and_resume_after_receiver_restart() {
    let root = tempfile::tempdir().unwrap();
    let receiver = device(root.path()).await;
    let connection = paired(&receiver).await;
    let data = vec![45; CHUNK + 37];
    let entries = vec![entry("resume.bin", &data)];
    let transfer = offer(&receiver, &connection, &entries).await;
    connection
        .chunk(&transfer.id, 0, 0, data[..CHUNK].to_vec())
        .await
        .unwrap();
    receiver.shutdown();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        connection
            .get::<Transfer>(&format!("/transfer/{}", transfer.id))
            .await
            .is_err()
    );
    drop(connection);
    drop(receiver);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let receiver = device(root.path()).await;
    let connection = paired(&receiver).await;
    let resumed = offer(&receiver, &connection, &entries).await;
    assert_eq!(resumed.offsets[0], CHUNK as u64);
    connection
        .chunk(&resumed.id, 0, CHUNK as u64, data[CHUNK..].to_vec())
        .await
        .unwrap();
    let result: Transfer = connection
        .post(&format!("/transfer/{}/finish", resumed.id), &())
        .await
        .unwrap();
    assert_eq!(result.status, "complete");
}

#[tokio::test]
#[ignore = "Explicit release-mode performance experiment, not part of the fast suite"]
async fn benchmark_streaming() {
    let sender_dir = tempfile::tempdir().unwrap();
    let receiver_dir = tempfile::tempdir().unwrap();
    let sender = device(sender_dir.path()).await;
    let receiver = device(receiver_dir.path()).await;
    let connection = paired(&receiver).await;
    let id = connection.session.clone();
    sender
        .engine
        .connections
        .lock()
        .await
        .insert(id.clone(), connection);
    let batch = uuid::Uuid::new_v4().to_string();
    let size = std::env::var("DRIFT_BENCH_MIB")
        .unwrap_or("256".into())
        .parse::<u64>()
        .unwrap()
        * 1024
        * 1024;
    let source = sender_dir.path().join("outbox").join(&batch);
    tokio::fs::create_dir_all(&source).await.unwrap();
    tokio::fs::File::create(source.join("0"))
        .await
        .unwrap()
        .set_len(size)
        .await
        .unwrap();
    sender.engine.uploads.lock().await.insert(
        batch.clone(),
        vec![drift_engine::Upload {
            path: "benchmark.bin".into(),
            size,
        }],
    );
    let engine = sender.engine.clone();
    let start = std::time::Instant::now();
    let sending = tokio::spawn(async move { engine.send(&id, &batch).await });
    let approval = tokio::time::timeout(std::time::Duration::from_secs(120), async {
        loop {
            if let Some(transfer) = receiver
                .engine
                .store
                .list()
                .unwrap()
                .into_iter()
                .find(|t| t.status == "pending")
            {
                receiver
                    .engine
                    .receive_approval(&transfer.id, true)
                    .await
                    .unwrap();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    approval.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(600), sending)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "BENCHMARK: {size} bytes, {elapsed:.3} seconds, {:.2} MiB/s end-to-end. Sparse zero-filled fixture, TLS loopback, includes hashing and verification. No browser staging or Wi-Fi/AirDrop comparison.",
        size as f64 / elapsed / 1048576.0
    );
}

#[tokio::test]
async fn sender_cancel_is_preserved_and_propagated_to_receiver() {
    let sender_dir = tempfile::tempdir().unwrap();
    let receiver_dir = tempfile::tempdir().unwrap();
    let sender = device(sender_dir.path()).await;
    let receiver = device(receiver_dir.path()).await;
    let connection = paired(&receiver).await;
    let connection_id = connection.session.clone();
    sender
        .engine
        .connections
        .lock()
        .await
        .insert(connection_id.clone(), connection);
    let batch = uuid::Uuid::new_v4().to_string();
    let source = sender_dir.path().join("outbox").join(&batch);
    tokio::fs::create_dir_all(&source).await.unwrap();
    tokio::fs::write(source.join("0"), b"cancel me")
        .await
        .unwrap();
    sender.engine.uploads.lock().await.insert(
        batch.clone(),
        vec![drift_engine::Upload {
            path: "cancel.txt".into(),
            size: 9,
        }],
    );
    let engine = sender.engine.clone();
    let sending = tokio::spawn(async move { engine.send(&connection_id, &batch).await });
    let id = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(transfer) = sender
                .engine
                .store
                .list()
                .unwrap()
                .into_iter()
                .find(|t| t.status == "awaiting approval")
            {
                break transfer.id;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    sender
        .engine
        .send_controls
        .lock()
        .unwrap()
        .insert(id.clone(), "cancelled".into());
    let mut local = sender.engine.store.get(&id).unwrap();
    local.status = "cancelled".into();
    sender.engine.store.save(&local).unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(10), sending)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert_eq!(sender.engine.store.get(&id).unwrap().status, "cancelled");
    assert_eq!(
        receiver
            .engine
            .store
            .get(id.strip_prefix("send-").unwrap())
            .unwrap()
            .status,
        "cancelled"
    );
}

#[test]
#[ignore = "Requires a multicast-enabled local network; run separately"]
fn mdns_discovery_lifecycle() {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.connect("192.0.2.1:80").unwrap();
    let host = socket.local_addr().unwrap().ip().to_string();
    let id_a = uuid::Uuid::new_v4().to_string();
    let id_b = uuid::Uuid::new_v4().to_string();
    let a = drift_engine::discovery::Discovery::start(&id_a, "Test A", &host, 9518).unwrap();
    let b = drift_engine::discovery::Discovery::start(&id_b, "Test B", &host, 9519).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if a.peers.lock().unwrap().values().any(|p| p.id == id_b) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Multicast discovery did not resolve the test peer"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    drop(b);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while a.peers.lock().unwrap().values().any(|p| p.id == id_b) {
        assert!(
            std::time::Instant::now() < deadline,
            "Peer removal was not observed"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[tokio::test]
async fn finished_transfer_keeps_only_the_published_copy() {
    let root = tempfile::tempdir().unwrap();
    let receiver = device(root.path()).await;
    let connection = paired(&receiver).await;
    let bytes = b"only the published copy should remain on disk";
    let entries = vec![entry("kept.txt", bytes)];
    let transfer = offer(&receiver, &connection, &entries).await;
    connection
        .chunk(&transfer.id, 0, 0, bytes.to_vec())
        .await
        .unwrap();
    connection
        .post::<_, Transfer>(&format!("/transfer/{}/finish", transfer.id), &())
        .await
        .unwrap();
    let data = &receiver.engine.store.root;
    // Deleting a received file must free its space: no hidden hard link remains.
    assert!(!data.join("partial").join(&transfer.id).exists());
    assert_eq!(
        tokio::fs::read(data.join("received").join(&transfer.id).join("kept.txt"))
            .await
            .unwrap(),
        bytes
    );
}

#[test]
fn reopening_storage_clears_unreachable_prepared_copies() {
    let root = tempfile::tempdir().unwrap();
    let staged = root.path().join("outbox").join("batch").join("0");
    std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
    std::fs::write(&staged, b"staged before a restart").unwrap();
    Store::open(root.path().to_owned()).unwrap();
    assert!(!root.path().join("outbox").exists());
}
