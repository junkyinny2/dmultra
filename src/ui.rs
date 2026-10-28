//! Progress reporting abstractions.
//!
//! The engine reports through the `ProgressSink` trait so the same
//! download core can drive the CLI bars (`ProgressUi`) and the GUI's
//! live stats.

use crate::engine::Outcome;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Engine-facing progress interface.
pub trait ProgressSink: Send + Sync {
    /// (Re)create per-segment state after the chunk plan is known.
    fn setup(&self, chunk_count: usize);
    /// A dynamic split created one more segment.
    fn add_chunk_bar(&self);
    /// Bytes written.
    fn on_bytes(&self, n: u64);
    /// Segment `idx` progress. `start`/`end` are the segment's absolute
    /// byte range in the final file (for segment-map visualization);
    /// `len` may be u64::MAX for unknown.
    fn chunk_update(&self, idx: usize, start: u64, end: u64, downloaded: u64, len: u64, conn: u32);
    /// Total size became known.
    fn set_total(&self, total: u64);
    /// Start speed sampling / ticking.
    fn begin(&self);
    /// Download restarts (corruption / no-resume paths).
    fn restart_message(&self, msg: &str);
    /// Final state transition.
    fn finish(&self, outcome: &Outcome);
}

const MAX_BARS: usize = 16;
const UNKNOWN_TOTAL: u64 = u64::MAX;

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n as f64;
    let mut unit = 0usize;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

fn eta_string(remaining: u64, speed: u64) -> String {
    if speed == 0 {
        return "unknown".into();
    }
    let secs = remaining / speed;
    if secs > 24 * 3600 {
        return "unknown".into();
    }
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
}

/// CLI progress display ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â overall bar plus one line per connection,
/// mirroring the download dialog. Speed sampling follows the
/// Session::CalcSpeed shape:
/// 40000-byte window reset, 1.1x headroom, >=100ms UI updates.
pub struct ProgressUi {
    mp: MultiProgress,
    main: ProgressBar,
    chunks: std::sync::Mutex<Vec<ProgressBar>>,
    bytes: Arc<AtomicU64>,
    total_known: Arc<AtomicU64>,
    last_speed: Arc<AtomicU64>,
    label: std::sync::Mutex<String>,
}

impl ProgressSink for ProgressUi {
    fn setup(&self, chunk_count: usize) {
        let mut chunks = self.chunks.lock().unwrap();
        chunks.clear();
        for _ in 0..chunk_count.min(MAX_BARS) {
            let bar = self.mp.add(ProgressBar::new(100));
            bar.set_style(ProgressStyle::with_template("  {msg}").unwrap());
            chunks.push(bar);
        }
    }

    fn add_chunk_bar(&self) {
        let mut chunks = self.chunks.lock().unwrap();
        if chunks.len() < MAX_BARS {
            let bar = self.mp.add(ProgressBar::new(100));
            bar.set_style(ProgressStyle::with_template("  {msg}").unwrap());
            chunks.push(bar);
        }
    }

    fn on_bytes(&self, n: u64) {
        self.bytes.fetch_add(n, Ordering::SeqCst);
        self.main.inc(n);
    }

    fn chunk_update(&self, idx: usize, _start: u64, _end: u64, downloaded: u64, len: u64, conn: u32) {
        let msg = if len == u64::MAX {
            format!("conn#{:>2} streaming {}", conn, human_bytes(downloaded))
        } else if len == 0 {
            format!("conn#{:>2} done", conn)
        } else {
            let pct = (downloaded * 100 / len).min(100);
            format!(
                "conn#{:>2} {:>3}% [{} / {}]",
                conn,
                pct,
                human_bytes(downloaded),
                human_bytes(len)
            )
        };
        let chunks = self.chunks.lock().unwrap();
        if let Some(bar) = chunks.get(idx) {
            bar.set_message(msg);
        }
    }

    fn set_total(&self, total: u64) {
        self.total_known.store(total, Ordering::SeqCst);
        if total > 0 {
            self.main.set_length(total);
        }
    }

    fn begin(&self) {
        let ui_bytes = self.bytes.clone();
        let ui_speed = self.last_speed.clone();
        let ui_total = self.total_known.clone();
        let main = self.main.clone();
        let label = self.label.lock().unwrap().clone();
        tokio::spawn(async move {
            let started = Instant::now();
            let mut last_bytes = 0u64;
            let mut window_start = Instant::now();
            let mut window_bytes = 0u64;
            let mut last_ui = Instant::now() - Duration::from_secs(1);
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let now = Instant::now();
                let cur = ui_bytes.load(Ordering::SeqCst);
                window_bytes += cur - last_bytes;
                last_bytes = cur;
                // Evidence FUN_005bb600: window reset past 40000 bytes,
                // reported speed has a 1.1x headroom factor.
                if window_bytes > 40000 {
                    let elapsed_ms = window_start.elapsed().as_millis().max(1);
                    let speed = (window_bytes as u128 * 1000 / elapsed_ms) as u64;
                    let speed = speed + speed / 10;
                    ui_speed.store(speed, Ordering::SeqCst);
                    window_start = now;
                    window_bytes = 0;
                }
                if now.duration_since(last_ui).as_millis() >= 100 {
                    last_ui = now;
                    let speed = ui_speed.load(Ordering::SeqCst);
                    let total = ui_total.load(Ordering::SeqCst);
                    let eta = if total > 0 && cur <= total {
                        eta_string(total - cur, speed)
                    } else {
                        "unknown".into()
                    };
                    main.set_message(format!(
                        "Downloading {} ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â {}/s, {} of {}, ETA {} (elapsed {}s)",
                        label,
                        human_bytes(speed),
                        human_bytes(cur),
                        if total > 0 { human_bytes(total) } else { "unknown".into() },
                        eta,
                        started.elapsed().as_secs()
                    ));
                }
            }
        });
    }

    fn restart_message(&self, msg: &str) {
        self.main.set_message(msg.to_string());
        self.main.tick();
    }

    fn finish(&self, outcome: &Outcome) {
        match outcome {
            Outcome::Complete { total } => {
                self.main.finish_with_message(format!("Download complete. {total} Bytes Total"))
            }
            Outcome::Paused => self.main.abandon_with_message("Paused".to_string()),
            Outcome::Cancelled => self.main.abandon_with_message("Cancelled by user".to_string()),
            Outcome::Failed(m) => self.main.abandon_with_message(format!("Download failed. {m}")),
        }
        self.mp.clear().ok();
    }
}

impl ProgressUi {
    /// Create the CLI UI for a download; `total` may be 0 (unknown).
    pub fn new(total: u64, file_label: &str) -> Arc<Self> {
        let mp = MultiProgress::new();
        let main = mp.add(ProgressBar::new(if total > 0 { total } else { UNKNOWN_TOTAL }));
        main.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} {msg}\n  [{elapsed_precise}] {bar:38.cyan/blue} {bytes}/{total_bytes} ({bytes_per_sec}, ETA {eta})",
            )
            .unwrap()
            .progress_chars("=>-"),
        );
        main.set_message(format!("Downloading {file_label}"));
        Arc::new(ProgressUi {
            mp,
            main,
            chunks: std::sync::Mutex::new(Vec::new()),
            bytes: Arc::new(AtomicU64::new(0)),
            total_known: Arc::new(AtomicU64::new(total)),
            last_speed: Arc::new(AtomicU64::new(0)),
            label: std::sync::Mutex::new(file_label.to_string()),
        })
    }
}
