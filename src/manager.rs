//! Background download manager for the GUI.
//!
//! The GUI process hosts the engine directly:
//! downloads run as tokio tasks; live progress is exposed through
//! `DownloadStats` (a `ProgressSink` implementation the GUI polls each
//! frame); pause/cancel is delivered via watch channels. The GUI
//! derives the displayed speed from byte deltas between frames.

use crate::engine;
use crate::limiter::{QuotaTracker, SpeedLimiter};
use crate::state::{DownloadRec, State, Status};
use crate::ui::ProgressSink;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{watch, Mutex};

/// Shared, process-wide tokio runtime (created once in main).
pub static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

pub fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get().expect("tokio runtime not initialised")
}

/// Live per-download statistics the GUI polls.
pub struct DownloadStats {
    total: AtomicU64,
    bytes: AtomicU64,
    restarted_msg: std::sync::Mutex<Option<String>>,
    segments: std::sync::Mutex<Vec<Segment>>,
    done: AtomicBool,
}

#[derive(Clone, Copy)]
pub struct Segment {
    /// Absolute byte offset where this segment starts in the final file.
    pub start: u64,
    /// Absolute byte offset where it ends (exclusive).
    pub end: u64,
    pub downloaded: u64,
    pub len: u64,
    pub conn: u32,
}

impl DownloadStats {
    fn new() -> Arc<Self> {
        Arc::new(DownloadStats {
            total: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            restarted_msg: std::sync::Mutex::new(None),
            segments: std::sync::Mutex::new(Vec::new()),
            done: AtomicBool::new(false),
        })
    }

    pub fn total(&self) -> u64 {
        self.total.load(Ordering::SeqCst)
    }
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::SeqCst)
    }
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::SeqCst);
        if total == 0 {
            0.0
        } else {
            (self.bytes.load(Ordering::SeqCst) as f32 / total as f32).min(1.0)
        }
    }
    pub fn segments(&self) -> Vec<Segment> {
        self.segments.lock().unwrap().clone()
    }
    pub fn restart_message(&self) -> Option<String> {
        self.restarted_msg.lock().unwrap().clone()
    }
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }
}

impl ProgressSink for DownloadStats {
    fn setup(&self, chunk_count: usize) {
        let mut segs = self.segments.lock().unwrap();
        segs.clear();
        segs.resize(chunk_count, Segment { start: 0, end: 0, downloaded: 0, len: 0, conn: 0 });
    }

    fn add_chunk_bar(&self) {
        self.segments
            .lock()
            .unwrap()
            .push(Segment { start: 0, end: 0, downloaded: 0, len: 0, conn: 0 });
    }

    fn on_bytes(&self, n: u64) {
        self.bytes.fetch_add(n, Ordering::SeqCst);
    }

    fn chunk_update(&self, idx: usize, start: u64, end: u64, downloaded: u64, len: u64, conn: u32) {
        let mut segs = self.segments.lock().unwrap();
        if let Some(s) = segs.get_mut(idx) {
            s.start = start;
            s.end = end;
            s.downloaded = downloaded;
            s.len = len;
            s.conn = conn;
        }
    }

    fn set_total(&self, total: u64) {
        self.total.store(total, Ordering::SeqCst);
    }

    /// No background ticker needed: the GUI samples `bytes()` between
    /// frames; the display layer applies a windowed rate with 1.1x
    /// headroom.
    fn begin(&self) {
        *self.restarted_msg.lock().unwrap() = None;
    }

    fn restart_message(&self, msg: &str) {
        *self.restarted_msg.lock().unwrap() = Some(msg.to_string());
    }

    fn finish(&self, _outcome: &crate::engine::Outcome) {
        self.done.store(true, Ordering::SeqCst);
    }
}

/// One actively-running download tracked by the manager.
struct LiveDownload {
    stats: Arc<DownloadStats>,
    /// Live connection limit (the GUI may raise/lower it mid-run).
    conn_limit: Arc<AtomicU32>,
    cancel: watch::Sender<bool>,
    finished: watch::Receiver<bool>,
    handle: tokio::task::JoinHandle<()>,
}

/// Coordinates background downloads and sequential queue processing.
pub struct Manager {
    pub state: Arc<Mutex<State>>,
    pub limiter: Arc<SpeedLimiter>,
    pub quota: Arc<QuotaTracker>,
    active: std::sync::Mutex<HashMap<u64, LiveDownload>>,
    queue_running: AtomicBool,
    queue_cancel: std::sync::Mutex<Option<watch::Sender<bool>>>,
}

impl Manager {
    pub fn new(state: Arc<Mutex<State>>, limiter: Arc<SpeedLimiter>, quota: Arc<QuotaTracker>) -> Arc<Self> {
        Arc::new(Manager {
            state,
            limiter,
            quota,
            active: std::sync::Mutex::new(HashMap::new()),
            queue_running: AtomicBool::new(false),
            queue_cancel: std::sync::Mutex::new(None),
        })
    }

    /// Start a download task. The engine persists state; the GUI polls
    /// `stats()` for progress.
    pub fn start(
        self: &Arc<Self>,
        rec: DownloadRec,
        auth: Option<(String, String)>,
        referer: Option<String>,
    ) -> Result<()> {
        if self.is_active(rec.id) {
            return Ok(()); // already running
        }
        // Site Login (IDM): when no credentials were given for this
        // download, auto-apply a saved login matching the URL host.
        let auth = auth.or_else(|| {
            let host = url::Url::parse(&rec.url).ok()?.host_str()?.to_string();
            let logins = self.lock_ui().site_logins.clone();
            for l in &logins {
                if site_login_matches(&l.pattern, &host) {
                    return Some((l.user.clone(), l.pass.clone()));
                }
            }
            None
        });
        let stats = DownloadStats::new();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (fin_tx, fin_rx) = watch::channel(false);
        let rec_id = rec.id;
        let state = self.state.clone();
        let limiter = self.limiter.clone();
        let quota = self.quota.clone();
        let sink: Arc<dyn ProgressSink> = stats.clone();
        let conn_limit = Arc::new(AtomicU32::new(
            rec.max_connections
                .unwrap_or(self.lock_ui().settings.max_connections)
                .clamp(1, 32),
        ));
        let conn_limit_task = conn_limit.clone();
        let handle = runtime().spawn(async move {
            crate::hooks::notify(&state, crate::hooks::EVENT_STARTED, &rec).await;
            let _ = engine::run(
                state, rec, limiter, Some(quota), cancel_rx, Some(sink), referer, auth,
                conn_limit_task,
            )
            .await;
            let _ = fin_tx.send(true);
        });
        self.active.lock().unwrap().insert(
            rec_id,
            LiveDownload {
                stats,
                conn_limit,
                cancel: cancel_tx,
                finished: fin_rx,
                handle,
            },
        );
        Ok(())
    }

    pub fn stop(&self, id: u64) {
        let map = self.active.lock().unwrap();
        if let Some(live) = map.get(&id) {
            let _ = live.cancel.send(true);
        }
    }

    /// Live-adjust the connection limit of a running download (the
    /// engine's supervisor reacts within ~150 ms). Returns the clamped
    /// value actually applied. Persisting the record is the caller's
    /// job (the GUI does it through `lock_ui`).
    pub fn set_connections(&self, id: u64, n: u32) -> u32 {
        let n = n.clamp(1, 32);
        let map = self.active.lock().unwrap();
        if let Some(live) = map.get(&id) {
            live.conn_limit.store(n, Ordering::SeqCst);
        }
        n
    }

    /// Current live connection limit of a running download, if any.
    pub fn connections(&self, id: u64) -> Option<u32> {
        let map = self.active.lock().unwrap();
        map.get(&id).map(|l| l.conn_limit.load(Ordering::SeqCst))
    }

    /// Acquire the state lock from the GUI thread. NEVER uses
    /// `blocking_lock` on the first attempt: tokio's blocking park can
    /// lose the unlock wakeup when called from a non-runtime thread,
    /// freezing the UI thread forever while a download is running.
    /// Instead spin on `try_lock_owned` (which cannot miss the unlock),
    /// with a tiny sleep between attempts, and log if it takes long.
    pub fn lock_ui(&self) -> tokio::sync::OwnedMutexGuard<State> {
        let state = self.state.clone();
        let start = std::time::Instant::now();
        let mut warned = false;
        loop {
            match state.clone().try_lock_owned() {
                Ok(guard) => return guard,
                Err(_) => {
                    let elapsed = start.elapsed();
                    if elapsed >= std::time::Duration::from_millis(250) && !warned {
                        warned = true;
                        crate::logfile::log(
                            "UILOCK",
                            &format!("state lock busy for {elapsed:?} (GUI thread spinning)"),
                        );
                    }
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
        }
    }

    pub fn is_active(&self, id: u64) -> bool {
        self.active.lock().unwrap().contains_key(&id)
    }

    pub fn stats(&self, id: u64) -> Option<Arc<DownloadStats>> {
        self.active.lock().unwrap().get(&id).map(|l| l.stats.clone())
    }

    /// Watch-channel that flips once the engine task fully finished
    /// (state already persisted). Used by task-edit flows.
    pub fn wait_finished(&self, id: u64) -> Option<watch::Receiver<bool>> {
        self.active.lock().unwrap().get(&id).map(|l| l.finished.clone())
    }

    /// Number of currently running downloads.
    pub fn active_count(&self) -> usize {
        self.active.lock().unwrap().len()
    }

    /// Drop finished tasks from the active map (the engine has already
    /// persisted their outcomes).
    pub fn reap(&self) {
        let mut map = self.active.lock().unwrap();
        let finished: Vec<u64> = map
            .iter()
            .filter(|(_, l)| l.handle.is_finished() || l.stats.is_done())
            .map(|(k, _)| *k)
            .collect();
        for id in finished {
            if let Some(l) = map.remove(&id) {
                l.handle.abort(); // already finished; this just detaches
            }
        }
    }

    pub fn queue_running(&self) -> bool {
        self.queue_running.load(Ordering::SeqCst)
    }

    /// Stop queue processing (cancels the current item).
    pub fn queue_stop(&self) {
        let guard = self.queue_cancel.lock().unwrap();
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(true);
        }
    }

    /// Stop every active download (toolbar "Stop All").
    pub fn stop_all(&self) {
        let map = self.active.lock().unwrap();
        for (_, live) in map.iter() {
            let _ = live.cancel.send(true);
        }
    }

    /// Start sequential queue processing (one file at a time).
    /// Runs as a background task; the GUI polls progress.
    pub fn start_queue(self: &Arc<Self>, name: String) {
        if self.queue_running() {
            return;
        }
        self.queue_running.store(true, Ordering::SeqCst);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        *self.queue_cancel.lock().unwrap() = Some(cancel_tx.clone());
        let manager = self.clone();
        let queue_name = name;
        runtime().spawn(async move {
            loop {
                if cancel_rx.borrow().to_owned() {
                    println!("Queue processing stopped");
                    break;
                }
                // Next queued item.
                let next = {
                    let st = manager.state.lock().await;
                    st.downloads
                        .iter()
                        .find(|d| {
                            d.status == Status::Queued && d.queue.as_deref() == Some(&queue_name)
                        })
                        .cloned()
                };
                let Some(rec) = next else {
                    println!("Queue processing finished");
                    break;
                };
                let id = rec.id;
                let stats = DownloadStats::new();
                let sink: Arc<dyn ProgressSink> = stats.clone();
                let (fin_tx, fin_rx) = watch::channel(false);
                let state = manager.state.clone();
                let limiter = manager.limiter.clone();
                let quota = manager.quota.clone();
                let item_cancel = cancel_rx.clone();
                let conn_limit = Arc::new(AtomicU32::new(
                    rec.max_connections
                        .unwrap_or(state.lock().await.settings.max_connections)
                        .clamp(1, 32),
                ));
                let conn_limit_task = conn_limit.clone();
                let handle = runtime().spawn(async move {
                    crate::hooks::notify(&state, crate::hooks::EVENT_STARTED, &rec).await;
                    let _ = engine::run(
                        state,
                        rec,
                        limiter,
                        Some(quota),
                        item_cancel,
                        Some(sink),
                        None,
                        None,
                        conn_limit_task,
                    )
                    .await;
                    let _ = fin_tx.send(true);
                });
                manager.active.lock().unwrap().insert(
                    id,
                    LiveDownload {
                        stats,
                        conn_limit,
                        cancel: cancel_tx.clone(),
                        finished: fin_rx,
                        handle,
                    },
                );
                // Wait for this item before starting the next
                // (sequential queue processing).
                let mut fin = manager
                    .active
                    .lock()
                    .unwrap()
                    .get(&id)
                    .map(|l| l.finished.clone())
                    .expect("just inserted");
                let _ = fin.changed().await;
                manager.active.lock().unwrap().remove(&id);
                if cancel_rx.borrow().to_owned() {
                    println!("Queue processing stopped");
                    break;
                }
            }
            manager.queue_running.store(false, Ordering::SeqCst);
            *manager.queue_cancel.lock().unwrap() = None;
        });
    }
}
/// Host match for a saved Site Login: exact host, or `*.domain` /
/// trailing-dot forms (leading dots optional).
pub fn site_login_matches(pattern: &str, host: &str) -> bool {
    let p = pattern.trim().to_lowercase();
    let h = host.trim().to_lowercase();
    if p.is_empty() {
        return false;
    }
    if p == h {
        return true;
    }
    if let Some(suffix) = p.strip_prefix("*.") {
        return h == suffix || h.ends_with(&format!(".{suffix}"));
    }
    if let Some(suffix) = p.strip_prefix('.') {
        return h == suffix || h.ends_with(&format!(".{suffix}"));
    }
    false
}