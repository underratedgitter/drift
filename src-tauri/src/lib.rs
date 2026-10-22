//! Drift's app for macOS, Windows, Linux, Android and iOS. The transfer engine
//! runs inside this process and the interface reaches its control API through
//! Tauri IPC: no local HTTP port, no secret in a URL, the same on every OS.
//!
//! File paths only ever come from the operating system (pickers, drag and
//! drop) on the Rust side. The web view cannot name files on disk, so even a
//! compromised page could not choose what gets sent.
mod device;
mod watch;

use axum::body::Body;
use drift_engine::{Options, Running, selection};
use serde::Serialize;
use serde_json::Value;
use std::{net::SocketAddr, path::PathBuf, sync::Mutex};
use tauri::{
    AppHandle, Emitter, Manager, RunEvent, State, WindowEvent,
    ipc::{InvokeBody, Request},
};
use tauri_plugin_deep_link::DeepLinkExt;
use tower::ServiceExt;

struct Engine {
    running: Running,
    router: axum::Router,
}

/// A `drift://pair?…` link opened before the interface was listening.
#[derive(Default)]
struct PendingInvitation(Mutex<Option<String>>);

#[derive(Serialize)]
struct Reply {
    status: u16,
    body: Value,
}

/// What became of files dropped on the window.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Dropped {
    Selected { selection: selection::Selection },
    Failed { error: String },
}

type CommandResult<T> = Result<T, String>;

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

async fn dispatch(
    engine: &Engine,
    method: &str,
    path: &str,
    body: Body,
    json: bool,
) -> CommandResult<Reply> {
    if !["GET", "POST", "PUT"].contains(&method) || !path.starts_with('/') || path.contains("..") {
        return Err("Unsupported request".into());
    }
    let mut request = axum::http::Request::builder()
        .method(method)
        .uri(format!("/api{path}"))
        .header(
            "authorization",
            format!("Bearer {}", engine.running.engine.ui_token),
        );
    if json {
        request = request.header("content-type", "application/json");
    }
    let response = engine
        .router
        .clone()
        .oneshot(request.body(body).map_err(text)?)
        .await
        .map_err(text)?;
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), drift_engine::model::MAX_METADATA * 8)
        .await
        .map_err(text)?;
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    Ok(Reply { status, body })
}

/// The control API, exactly as the loopback web interface sees it.
#[tauri::command]
async fn api(
    engine: State<'_, Engine>,
    method: String,
    path: String,
    body: Option<Value>,
) -> CommandResult<Reply> {
    let (body, json) = match body {
        Some(value) => (Body::from(value.to_string()), true),
        None => (Body::empty(), false),
    };
    dispatch(&engine, &method, &path, body, json).await
}

/// File bytes picked in the web view (phones), written to the outbox.
#[tauri::command]
async fn api_bytes(engine: State<'_, Engine>, request: Request<'_>) -> CommandResult<Reply> {
    let path = request
        .headers()
        .get("x-drift-path")
        .and_then(|value| value.to_str().ok())
        .ok_or("Missing upload path")?
        .to_owned();
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("Expected file bytes".into());
    };
    if !path.starts_with("/stage/") {
        return Err("Unsupported upload".into());
    }
    dispatch(&engine, "PUT", &path, Body::from(bytes.clone()), false).await
}

/// Opens the system file or folder picker and selects what was chosen.
#[tauri::command]
async fn choose(
    app: AppHandle,
    engine: State<'_, Engine>,
    folder: bool,
) -> CommandResult<Option<selection::Selection>> {
    use tauri_plugin_dialog::DialogExt;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let dialog = app.dialog().file();
    if folder {
        dialog
            .set_title("Choose a folder to send")
            .pick_folder(move |picked| drop(sender.send(picked.map(|path| vec![path]))));
    } else {
        dialog
            .set_title("Choose files to send")
            .pick_files(move |picked| drop(sender.send(picked)));
    }
    let Some(picked) = receiver.await.map_err(text)? else {
        return Ok(None);
    };
    let paths = picked
        .into_iter()
        .map(|path| path.into_path().map_err(text))
        .collect::<CommandResult<Vec<PathBuf>>>()?;
    select_paths(&engine, paths).await.map(Some)
}

async fn select_paths(engine: &Engine, paths: Vec<PathBuf>) -> CommandResult<selection::Selection> {
    let sources = tauri::async_runtime::spawn_blocking(move || selection::collect(&paths))
        .await
        .map_err(text)?
        .map_err(text)?;
    selection::select(&engine.running.engine, sources)
        .await
        .map_err(text)
}

/// Shows a received transfer's saved files in Finder / Explorer / Files.
#[tauri::command]
async fn reveal(app: AppHandle, engine: State<'_, Engine>, transfer: String) -> CommandResult<()> {
    let saved = engine.running.engine.store.saved(&transfer).map_err(text)?;
    let first = saved
        .first()
        .ok_or("These files were not saved to a folder")?;
    if !std::path::Path::new(first).exists() {
        return Err("The saved files were moved or deleted".into());
    }
    #[cfg(desktop)]
    {
        use tauri_plugin_opener::OpenerExt;
        app.opener().reveal_item_in_dir(first).map_err(text)
    }
    #[cfg(mobile)]
    {
        use tauri_plugin_opener::OpenerExt;
        app.opener().open_path(first, None::<&str>).map_err(text)
    }
}

#[tauri::command]
fn take_invitation(pending: State<'_, PendingInvitation>) -> Option<String> {
    pending.0.lock().unwrap().take()
}

fn invitation(app: &AppHandle, url: &str) {
    if !url.starts_with("drift://pair?") || url.len() > 8192 {
        return;
    }
    show(app);
    *app.state::<PendingInvitation>().0.lock().unwrap() = Some(url.to_owned());
    let _ = app.emit("invitation", url);
}

fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Where verified received files go: somewhere people look for them.
fn inbox(app: &AppHandle) -> tauri::Result<Option<PathBuf>> {
    #[cfg(target_os = "ios")]
    {
        // The app's Documents folder: "On My iPhone › Drift" in the Files app.
        return Ok(Some(app.path().document_dir()?));
    }
    #[cfg(target_os = "android")]
    {
        // Saved to Downloads/Drift through the system's media store; see the
        // Android plugin. Until then files stay in the app.
        let _ = app;
        return Ok(None);
    }
    #[cfg(desktop)]
    {
        Ok(Some(app.path().download_dir()?.join("Drift")))
    }
}

fn start_engine(app: &AppHandle) -> anyhow::Result<Engine> {
    let root = app.path().app_data_dir()?.join("engine");
    let inbox = inbox(app)?;
    let running = tauri::async_runtime::block_on(drift_engine::launch(Options {
        root,
        name: device::name(),
        host: drift_engine::network::lan_ipv4(),
        bind: SocketAddr::from(([0, 0, 0, 0], 0)),
        discover: true,
        inbox,
    }))?;
    let router = drift_engine::web::app_router(running.engine.clone());
    Ok(Engine { running, router })
}

#[cfg(desktop)]
fn tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::{
        menu::{Menu, MenuItem},
        tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    };
    let open = MenuItem::with_id(app, "open", "Open Drift", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Drift", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut builder = TrayIconBuilder::with_id("drift")
        .tooltip("Drift is ready to receive")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

fn window_event(window: &tauri::Window, event: &WindowEvent) {
    match event {
        // Closing the window keeps Drift receiving from the tray / menu bar.
        #[cfg(desktop)]
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = window.hide();
            watch::first_hide(window.app_handle());
        }
        WindowEvent::DragDrop(drop) => {
            use tauri::DragDropEvent;
            match drop {
                DragDropEvent::Enter { .. } | DragDropEvent::Over { .. } => {
                    let _ = window.emit("dragging", true);
                }
                DragDropEvent::Leave => {
                    let _ = window.emit("dragging", false);
                }
                DragDropEvent::Drop { paths, .. } => {
                    let _ = window.emit("dragging", false);
                    let (app, paths) = (window.app_handle().clone(), paths.clone());
                    tauri::async_runtime::spawn(async move {
                        let engine = app.state::<Engine>();
                        let dropped = match select_paths(&engine, paths).await {
                            Ok(selection) => Dropped::Selected { selection },
                            Err(error) => Dropped::Failed { error },
                        };
                        let _ = app.emit("selection", dropped);
                    });
                }
                _ => {}
            }
        }
        _ => {}
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();
    #[cfg(desktop)]
    {
        // Must be first: a second launch (or a drift:// link on Windows and
        // Linux) reaches the running app instead of starting a second engine.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show(app)
        }));
    }
    let app = builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .manage(PendingInvitation::default())
        .setup(|app| {
            let engine = match start_engine(app.handle()) {
                Ok(engine) => engine,
                Err(error) => {
                    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
                    app.dialog()
                        .message(format!(
                            "{error:#}\n\nClose other copies of Drift and try again."
                        ))
                        .title("Drift could not start")
                        .kind(MessageDialogKind::Error)
                        .blocking_show();
                    std::process::exit(1);
                }
            };
            watch::spawn(app.handle().clone(), engine.running.engine.clone());
            app.manage(engine);
            #[cfg(desktop)]
            tray(app)?;
            #[cfg(any(windows, target_os = "linux"))]
            {
                // Installers register drift:// themselves; this covers portable runs.
                let _ = app.deep_link().register_all();
            }
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    invitation(&handle, url.as_str());
                }
            });
            if let Some(urls) = app.deep_link().get_current()? {
                for url in urls {
                    invitation(app.handle(), url.as_str());
                }
            }
            Ok(())
        })
        .on_window_event(window_event)
        .invoke_handler(tauri::generate_handler![
            api,
            api_bytes,
            choose,
            reveal,
            take_invitation
        ])
        .build(tauri::generate_context!())
        .expect("Drift's app shell failed to build");
    app.run(|app, event| match event {
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show(app),
        RunEvent::Exit => {
            if let Some(engine) = app.try_state::<Engine>() {
                engine.running.shutdown();
            }
        }
        _ => {}
    });
}
