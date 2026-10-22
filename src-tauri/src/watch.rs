//! System notifications for what needs the person's attention, sent from the
//! app process so they arrive while the window is closed or in the background.
use drift_engine::{Shared, model::Transfer};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

fn notify(app: &AppHandle, body: &str) {
    // Nobody needs a notification for the window they are looking at.
    let focused = app.get_webview_window("main").is_some_and(|window| {
        window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
    });
    if focused {
        return;
    }
    if let Err(error) = app
        .notification()
        .builder()
        .title("Drift")
        .body(body)
        .show()
    {
        eprintln!("Notification unavailable: {error}");
    }
}

fn describe(transfer: &Transfer) -> String {
    let first = transfer
        .entries
        .first()
        .map(|entry| {
            entry
                .path
                .split('/')
                .next()
                .unwrap_or(&entry.path)
                .to_owned()
        })
        .unwrap_or_default();
    match transfer.entries.len() {
        1 => first,
        n => format!("{first} and {} more", n - 1),
    }
}

/// Says once, the first time the window is closed, that Drift keeps running.
pub fn first_hide(app: &AppHandle) {
    static TOLD: AtomicBool = AtomicBool::new(false);
    if !TOLD.swap(true, Ordering::Relaxed) {
        let place = if cfg!(target_os = "macos") {
            "menu bar"
        } else {
            "system tray"
        };
        notify(
            app,
            &format!(
                "Drift is still running in the {place} so your devices can send to it. Quit it from there."
            ),
        );
    }
}

pub fn spawn(app: AppHandle, engine: Shared) {
    tauri::async_runtime::spawn(async move {
        let mut statuses: HashMap<String, String> = engine
            .store
            .list()
            .map(|transfers| transfers.into_iter().map(|t| (t.id, t.status)).collect())
            .unwrap_or_default();
        let mut requests: Vec<String> = Vec::new();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let pending: Vec<_> = engine
                .sessions
                .lock()
                .await
                .values()
                .filter(|session| !session.approved)
                .map(|session| (session.id.clone(), session.name.clone()))
                .collect();
            for (id, name) in &pending {
                if !requests.contains(id) {
                    notify(
                        &app,
                        &format!("{name} wants to connect. Open Drift to approve or decline."),
                    );
                }
            }
            requests = pending.into_iter().map(|(id, _)| id).collect();
            let Ok(transfers) = engine.store.list() else {
                continue;
            };
            for transfer in &transfers {
                let before = statuses.insert(transfer.id.clone(), transfer.status.clone());
                if before.as_deref() == Some(transfer.status.as_str())
                    || transfer.direction != "receive"
                {
                    continue;
                }
                match transfer.status.as_str() {
                    "pending" => notify(
                        &app,
                        &format!(
                            "{} wants to send you {}. Open Drift to review.",
                            transfer.peer,
                            describe(transfer)
                        ),
                    ),
                    "complete" => notify(
                        &app,
                        &format!("Received {} from {}.", describe(transfer), transfer.peer),
                    ),
                    _ => {}
                }
            }
        }
    });
}
