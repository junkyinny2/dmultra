//! Global speed limiter and download quota.
//!
//!  - Speed limiter: an optional bytes/sec cap; the UI setting is KB,
//!    converted with `* 0x400` to bytes/sec; fallback default 10240
//!    bytes/sec.
//!  - Session speed calculation: accumulates a window until >40000
//!    bytes, computes speed = bytes/ms * 1000 with 1.1x headroom,
//!    updates the UI at >=100ms and fully refreshes at >=2500ms.
//!  - Download quotas: MB-limit per time window with automatic stop and
//!    auto-resume at window end ("Download limits exceeded!" messages).

use chrono::Timelike;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

/// Shared global byte-rate limiter (token bucket, no initial burst ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â
/// Shared global byte-rate limiter (token bucket, no initial burst;
/// reads are paced rather than allowing large bursts).
pub struct SpeedLimiter {
    enabled: AtomicBool,
    bytes_per_sec: AtomicU64,
    tokens: tokio::sync::Mutex<f64>,
    last_refill: tokio::sync::Mutex<Instant>,
}

impl SpeedLimiter {
    pub fn new(enabled: bool, bytes_per_sec: u64) -> Arc<Self> {
        Arc::new(SpeedLimiter {
            enabled: AtomicBool::new(enabled),
            bytes_per_sec: AtomicU64::new(bytes_per_sec.max(1)),
            tokens: tokio::sync::Mutex::new(0.0),
            last_refill: tokio::sync::Mutex::new(Instant::now()),
        })
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::SeqCst);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub fn set_limit(&self, bps: u64) {
        self.bytes_per_sec.store(bps.max(1), Ordering::SeqCst);
    }

    pub fn limit(&self) -> u64 {
        self.bytes_per_sec.load(Ordering::SeqCst)
    }

    /// Wait until `n` bytes may be transferred. Returns immediately when
    /// the limiter is disabled.
    pub async fn acquire(&self, mut n: u64) {
        if !self.enabled.load(Ordering::SeqCst) {
            return;
        }
        while n > 0 {
            let bps = self.bytes_per_sec.load(Ordering::SeqCst).max(1);
            // One grant never exceeds ~1 second of budget (smooth pacing).
            let grant = n.min(bps).max(1);
            let mut tokens = self.tokens.lock().await;
            let mut last = self.last_refill.lock().await;
            let now = Instant::now();
            let elapsed = now.duration_since(*last).as_secs_f64();
            if elapsed > 0.0 {
                // Burst capacity = one second of budget.
                *tokens = (*tokens + elapsed * bps as f64).min(bps as f64);
                *last = now;
            }
            if *tokens >= grant as f64 {
                *tokens -= grant as f64;
                n -= grant;
                if n == 0 {
                    return;
                }
            } else {
                let need = (grant as f64 - *tokens).max(1.0);
                let wait = (need / bps as f64).min(1.0);
                drop(tokens);
                drop(last);
                tokio::time::sleep(Duration::from_secs_f64(wait)).await;
            }
        }
    }
}

/// Rolling download-quota tracker (MB per hour window).
pub struct QuotaTracker {
    /// Bytes downloaded inside the current window.
    pub bytes: AtomicU64,
    /// Whether the quota tripped (downloads must stop).
    pub tripped: AtomicBool,
    config: std::sync::RwLock<crate::state::Quota>,
    window_start: tokio::sync::Mutex<Instant>,
}

impl QuotaTracker {
    pub fn new(config: crate::state::Quota) -> Arc<Self> {
        Arc::new(QuotaTracker {
            bytes: AtomicU64::new(0),
            tripped: AtomicBool::new(false),
            config: std::sync::RwLock::new(config),
            window_start: tokio::sync::Mutex::new(Instant::now()),
        })
    }

    pub fn update_config(&self, config: crate::state::Quota) {
        let mut current = self.config.write().unwrap();
        let changed = current.enabled != config.enabled
            || current.mb != config.mb
            || current.start_hour != config.start_hour
            || current.end_hour != config.end_hour;
        *current = config;
        if changed {
            self.bytes.store(0, Ordering::SeqCst);
            self.tripped.store(false, Ordering::SeqCst);
        }
    }

    fn in_window(config: &crate::state::Quota) -> bool {
        let now = chrono::Local::now().time().hour() as u32;
        let (s, e) = (config.start_hour, config.end_hour);
        if s <= e {
            now >= s && now < e
        } else {
            now >= s || now < e
        }
    }

    /// Record transferred bytes; returns true when the quota trips.
    pub async fn record(&self, n: u64) -> bool {
        let config = self.config.read().unwrap().clone();
        if !config.enabled {
            return false;
        }
        let mut start = self.window_start.lock().await;
        if !Self::in_window(&config) {
            // Window rolled over: auto-resume stopped downloads.
            self.bytes.store(0, Ordering::SeqCst);
            self.tripped.store(false, Ordering::SeqCst);
            *start = Instant::now();
            return false;
        }
        let used = self.bytes.fetch_add(n, Ordering::SeqCst) + n;
        let limit = config.mb * 1024 * 1024;
        if used >= limit {
            self.tripped.store(true, Ordering::SeqCst);
        }
        self.tripped.load(Ordering::SeqCst)
    }

    pub fn tripped(&self) -> bool {
        self.tripped.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Quota;

    #[tokio::test]
    async fn limiter_disabled_is_free() {
        let l = SpeedLimiter::new(false, 10240);
        let t = Instant::now();
        l.acquire(10 * 1024 * 1024).await;
        assert!(t.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn limiter_throttles() {
        let l = SpeedLimiter::new(true, 64 * 1024);
        let t = Instant::now();
        l.acquire(128 * 1024).await; // 128 KiB at 64 KiB/s ~= 2s
        assert!(t.elapsed() >= Duration::from_millis(1500));
    }

    #[tokio::test]
    async fn quota_config_changes_apply_to_live_tracker() {
        let hour = chrono::Local::now().time().hour() as u32;
        let quota = Quota {
            enabled: true,
            mb: 1,
            start_hour: hour,
            end_hour: (hour + 1) % 24,
        };
        let tracker = QuotaTracker::new(quota.clone());
        assert!(tracker.record(1024 * 1024).await);

        let mut disabled = quota;
        disabled.enabled = false;
        tracker.update_config(disabled);
        assert!(!tracker.record(1).await);
    }
}
