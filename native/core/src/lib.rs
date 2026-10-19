use anyhow::{Context, Result, ensure};
use drift_engine::{Running, Shared};
use include_dir::{Dir, include_dir};
use serde_json::{Value, json};
use std::{
    ffi::{CStr, CString, c_char},
    net::SocketAddr,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};
use tokio::runtime::Runtime;

static ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../dist/ui");
static INSTANCE: OnceLock<Mutex<Option<Instance>>> = OnceLock::new();
struct Instance {
    runtime: Runtime,
    running: Running,
    ui: tokio::task::JoinHandle<()>,
    root: PathBuf,
    url: String,
}
fn state() -> &'static Mutex<Option<Instance>> {
    INSTANCE.get_or_init(|| Mutex::new(None))
}
fn assets(path: &str) -> Option<&'static [u8]> {
    ASSETS.get_file(path).map(|file| file.contents())
}

fn start(root: PathBuf, name: String) -> Result<Value> {
    start_with_host(root, name, None)
}

#[cfg(any(target_os = "android", test))]
fn supplied_lan_host(value: &str) -> Result<Option<String>> {
    if value.is_empty() {
        return Ok(None);
    }
    let ip: std::net::Ipv4Addr = value.parse().context("Invalid local network address")?;
    ensure!(
        ip.is_private() || ip.is_link_local(),
        "Use a private Wi-Fi or Ethernet address"
    );
    Ok(Some(ip.to_string()))
}

fn start_with_host(root: PathBuf, name: String, host: Option<String>) -> Result<Value> {
    ensure!(
        root.is_absolute(),
        "Use an absolute app-private data directory"
    );
    let mut instance = state()
        .lock()
        .map_err(|_| anyhow::anyhow!("Engine state is unavailable"))?;
    if let Some(existing) = instance.as_ref() {
        ensure!(
            existing.root == root,
            "Another engine instance is already running"
        );
        return Ok(json!({"url":existing.url}));
    }
    let runtime = Runtime::new()?;
    let (running, address, ui) = runtime.block_on(async {
        let host = host.unwrap_or_else(drift_engine::network::lan_ipv4);
        let running = drift_engine::start(
            root.clone(),
            name,
            host,
            SocketAddr::from(([0, 0, 0, 0], 0)),
            true,
        )
        .await?;
        let (address, ui) = drift_engine::web::serve_embedded(
            running.engine.clone(),
            SocketAddr::from(([127, 0, 0, 1], 0)),
            assets,
        )
        .await?;
        Ok::<_, anyhow::Error>((running, address, ui))
    })?;
    let url = format!("http://{address}/#{}", running.engine.ui_token);
    *instance = Some(Instance {
        runtime,
        running,
        ui,
        root,
        url: url.clone(),
    });
    Ok(json!({"url":url}))
}
fn export(engine: &Shared, id: &str, index: usize) -> Result<Value> {
    ensure!(
        id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid received transfer ID"
    );
    let transfer = engine.store.get(id)?;
    ensure!(
        transfer.direction == "receive" && transfer.status == "complete",
        "Only verified completed received files can be exported"
    );
    let entry = transfer
        .entries
        .get(index)
        .context("File index not found")?;
    drift_engine::storage::safe_path(&entry.path)?;
    let path = engine
        .store
        .root
        .join("received")
        .join(id)
        .join(&entry.path);
    let metadata = std::fs::symlink_metadata(&path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() == entry.size,
        "Received file changed or is unavailable"
    );
    Ok(
        json!({"path":path,"name":path.file_name().and_then(|value| value.to_str()).context("Unsupported filename")?}),
    )
}
fn reply(action: impl FnOnce() -> Result<Value>) -> String {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)) {
        Ok(Ok(value)) => json!({"ok":true,"result":value}).to_string(),
        Ok(Err(error)) => json!({"ok":false,"error":error.to_string()}).to_string(),
        Err(_) => {
            json!({"ok":false,"error":"Native engine failed safely; restart Drift."}).to_string()
        }
    }
}
unsafe fn input(value: *const c_char) -> Result<String> {
    ensure!(!value.is_null(), "Missing native argument");
    // Caller contract: a valid, NUL-terminated UTF-8 C string for this call.
    Ok(unsafe { CStr::from_ptr(value) }.to_str()?.to_owned())
}
fn output(value: String) -> *mut c_char {
    CString::new(value)
        .expect("JSON escapes embedded NULs")
        .into_raw()
}

/// # Safety
/// Arguments must point to valid NUL-terminated strings for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn drift_start(root: *const c_char, name: *const c_char) -> *mut c_char {
    output(reply(|| {
        start(PathBuf::from(unsafe { input(root)? }), unsafe {
            input(name)?
        })
    }))
}
/// # Safety
/// ID must be a valid NUL-terminated string. The reply must be freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn drift_export_path(id: *const c_char, index: u32) -> *mut c_char {
    output(reply(|| {
        let id = unsafe { input(id)? };
        let instance = state()
            .lock()
            .map_err(|_| anyhow::anyhow!("Engine state unavailable"))?;
        export(
            &instance
                .as_ref()
                .context("Engine is not running")?
                .running
                .engine,
            &id,
            index as usize,
        )
    }))
}
#[unsafe(no_mangle)]
pub extern "C" fn drift_stop() {
    if let Ok(mut guard) = state().lock()
        && let Some(instance) = guard.take()
    {
        instance.ui.abort();
        instance.running.shutdown();
        drop(instance.running);
        instance
            .runtime
            .shutdown_timeout(std::time::Duration::from_secs(1));
    }
}
/// # Safety
/// Free only a non-null pointer returned by drift_start/drift_export_path, exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn drift_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}

#[cfg(target_os = "android")]
mod android {
    use super::*;
    use jni::{
        JNIEnv,
        objects::{JClass, JString},
        sys::{jint, jstring},
    };
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_drift_app_NativeBridge_start(
        mut env: JNIEnv<'_>,
        _class: JClass<'_>,
        root: JString<'_>,
        name: JString<'_>,
        host: JString<'_>,
    ) -> jstring {
        let result = reply(|| {
            let root: String = env.get_string(&root)?.into();
            let name: String = env.get_string(&name)?.into();
            let host: String = env.get_string(&host)?.into();
            start_with_host(PathBuf::from(root), name, supplied_lan_host(&host)?)
        });
        env.new_string(result)
            .map(|value| value.into_raw())
            .unwrap_or(std::ptr::null_mut())
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_drift_app_NativeBridge_exportPath(
        mut env: JNIEnv<'_>,
        _class: JClass<'_>,
        id: JString<'_>,
        index: jint,
    ) -> jstring {
        let result = reply(|| {
            ensure!(index >= 0, "Invalid file index");
            let id: String = env.get_string(&id)?.into();
            let guard = state()
                .lock()
                .map_err(|_| anyhow::anyhow!("Engine state unavailable"))?;
            export(
                &guard
                    .as_ref()
                    .context("Engine is not running")?
                    .running
                    .engine,
                &id,
                index as usize,
            )
        });
        env.new_string(result)
            .map(|value| value.into_raw())
            .unwrap_or(std::ptr::null_mut())
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_drift_app_NativeBridge_stop(
        _env: JNIEnv<'_>,
        _class: JClass<'_>,
    ) {
        drift_stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn platform_lan_address_requires_a_private_local_link() {
        assert_eq!(
            supplied_lan_host("10.0.2.16").unwrap(),
            Some("10.0.2.16".into())
        );
        assert_eq!(
            supplied_lan_host("192.168.43.1").unwrap(),
            Some("192.168.43.1".into())
        );
        assert_eq!(
            supplied_lan_host("169.254.1.2").unwrap(),
            Some("169.254.1.2".into())
        );
        assert_eq!(supplied_lan_host("").unwrap(), None);
        for value in [
            "127.0.0.1",
            "0.0.0.0",
            "8.8.8.8",
            "224.0.0.1",
            "example.com",
            "::1",
        ] {
            assert!(supplied_lan_host(value).is_err(), "Accepted {value}");
        }
    }

    #[test]
    fn embedded_assets_match_current_ui_build() {
        fn check(directory: &Dir<'_>, root: &std::path::Path) {
            for file in directory.files() {
                assert_eq!(
                    file.contents(),
                    std::fs::read(root.join(file.path())).unwrap(),
                    "Stale embedded UI asset: {}",
                    file.path().display()
                );
            }
            for child in directory.dirs() {
                check(child, root);
            }
        }
        check(
            &ASSETS,
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/ui"),
        );
    }

    #[test]
    fn native_lifecycle_assets_and_export_authority() {
        let root = tempfile::tempdir().unwrap();
        let response = start(root.path().into(), "Native test".into()).unwrap();
        assert_eq!(
            start(root.path().into(), "Recreated activity".into()).unwrap()["url"],
            response["url"]
        );
        let other = tempfile::tempdir().unwrap();
        assert!(start(other.path().into(), "Other".into()).is_err());
        let url = response["url"].as_str().unwrap().to_owned();
        let (base, token) = url.split_once('#').unwrap();
        let runtime = Runtime::new().unwrap();
        runtime.block_on(async {
            let client = reqwest::Client::new();
            let response = client.get(base).send().await.unwrap();
            assert!(response.status().is_success());
            assert!(response.text().await.unwrap().contains("Drift"));
            assert_eq!(
                client
                    .get(format!("{base}api/state"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                401
            );
            assert!(
                client
                    .get(format!("{base}api/state"))
                    .bearer_auth(token)
                    .send()
                    .await
                    .unwrap()
                    .status()
                    .is_success()
            );
            assert_eq!(
                client
                    .get(format!("{base}missing.js"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                404
            );
        });
        let guard = state().lock().unwrap();
        let engine = &guard.as_ref().unwrap().running.engine;
        assert!(export(engine, "../../private", 0).is_err());
        let id = "a".repeat(64);
        engine
            .store
            .save(&drift_engine::model::Transfer {
                id: id.clone(),
                entries: vec![drift_engine::model::Entry {
                    path: "file.txt".into(),
                    size: 3,
                    sha256: "0".repeat(64),
                }],
                offsets: vec![3],
                status: "receiving".into(),
                peer: "test".into(),
                updated: 0,
                error: None,
                direction: "receive".into(),
            })
            .unwrap();
        assert!(export(engine, &id, 0).is_err());
        let path = root.path().join("received").join(&id);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("file.txt"), b"abc").unwrap();
        let mut transfer = engine.store.get(&id).unwrap();
        transfer.status = "complete".into();
        engine.store.save(&transfer).unwrap();
        assert_eq!(export(engine, &id, 0).unwrap()["name"], "file.txt");
        assert!(export(engine, &id, 1).is_err());
        drop(guard);
        drift_stop();
        runtime.block_on(async {
            assert!(reqwest::get(base).await.is_err());
        });
        let invalid = reply(|| anyhow::bail!("expected failure"));
        assert!(invalid.contains("\"ok\":false"));
        assert!(unsafe { input(std::ptr::null()) }.is_err());
    }
}
