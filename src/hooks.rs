//! Task event fan-out: diagnostic log lines + webhook HTTP POSTs.

use crate::state::{DownloadRec, State, Webhook};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

pub const EVENT_STARTED: &str = "started";
pub const EVENT_COMPLETED: &str = "completed";
pub const EVENT_FAILED: &str = "failed";
pub const EVENT_ADDED: &str = "added";
pub const EVENT_DELETED: &str = "deleted";

/// Log + fire webhooks for one task event. Async-safe (holds the state
/// lock only while cloning the webhook list).
pub async fn notify(state: &Arc<Mutex<State>>, kind: &str, rec: &DownloadRec) {
    let hooks = { state.lock().await.webhooks.clone() };
    notify_with(hooks, kind, rec);
}

/// Sync callers (GUI) hand us the webhook list they already cloned.
pub fn notify_with(hooks: Vec<Webhook>, kind: &str, rec: &DownloadRec) {
    log_task(kind, rec);
    dispatch(hooks, kind, rec);
}

/// Schedule a notify from a synchronous context (GUI/CLI) onto the
/// shared tokio runtime.
pub fn spawn_notify(state: Arc<Mutex<State>>, kind: &'static str, rec: DownloadRec) {
    let _ = crate::manager::runtime().spawn(async move {
        notify(&state, kind, &rec).await;
    });
}

pub fn log_task(kind: &str, rec: &DownloadRec) {
    let name = rec
        .save_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| rec.url.clone());
    crate::logfile::log(
        "TASK",
        &format!("#{} {} [{}] {} ({})", rec.id, kind, rec.status.as_str(), name, rec.url),
    );
}

pub fn dispatch(hooks: Vec<Webhook>, kind: &str, rec: &DownloadRec) {    if hooks.is_empty() {
        return;
    }
    let name = rec
        .save_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let body = json!({
        "event": kind,
        "id": rec.id,
        "name": name,
        "url": rec.url,
        "save_path": rec.save_path.to_string_lossy(),
        "size": rec.file_size,
        "downloaded": rec.total_downloaded(),
        "status": rec.status.as_str(),
        "error": rec.last_error,
        "time": crate::state::unix_now(),
    });
    for h in hooks {
        if !h.enabled || h.url.trim().is_empty() {
            continue;
        }
        if !h.events.is_empty() && !h.events.iter().any(|e| e == kind) {
            continue;
        }
        let url = h.url.trim().to_string();
        let body = body.clone();
        let label = if h.name.trim().is_empty() { url.clone() } else { h.name.clone() };
        let kind = kind.to_string();
        let _ = crate::manager::runtime().spawn(async move {
            let client = reqwest::Client::new();
            match client.post(&url).json(&body).timeout(Duration::from_secs(10)).send().await {
                Ok(resp) => crate::logfile::log(
                    "WEBHOOK",
                    &format!("{label} <- {} -> HTTP {}", kind, resp.status().as_u16()),
                ),
                Err(e) => crate::logfile::log(
                    "WEBHOOK",
                    &format!("{label} <- {} -> error {e:#}", kind),
                ),
            }
        });
    }
}
