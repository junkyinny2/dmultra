//! Rotating diagnostic log file (<data>/dmultra/logs/dmultra.log).

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

static LOGGER: OnceLock<Mutex<Option<File>>> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);

const MAX_BYTES: u64 = 5 * 1024 * 1024;

pub fn dir() -> PathBuf {
    crate::state::state_dir().join("logs")
}

fn path() -> PathBuf {
    dir().join("dmultra.log")
}

pub fn init(enabled: bool) {
    ENABLED.store(enabled, Ordering::SeqCst);
    let cell = LOGGER.get_or_init(|| Mutex::new(None));
    if !enabled {
        return;
    }
    let _ = fs::create_dir_all(dir());
    let p = path();
    if fs::metadata(&p).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = fs::rename(&p, dir().join("dmultra.log.1"));
    }
    let file = OpenOptions::new().create(true).append(true).open(&p);
    if let Ok(f) = file {
        *cell.lock().unwrap() = Some(f);
    }
}

pub fn set_enabled(on: bool) {
    if on && !ENABLED.swap(true, Ordering::SeqCst) {
        init(true);
    } else if !on {
        ENABLED.store(false, Ordering::SeqCst);
        if let Some(cell) = LOGGER.get() {
            *cell.lock().unwrap() = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Frame profiler (opt-in: only active when `DMULTRA_PROFILE` is set)
// ---------------------------------------------------------------------------
//
// Window dragging is performed by Windows inside a modal message loop that
// runs on the GUI thread, so the only way to know whether a frame is too
// expensive to run there is to measure it. Each completed frame writes one
// `FRAME <mode> <ms>` line to `logs/profile.log`; `<mode>` is `drag` while the
// move/size loop owns the thread and `idle`/`ui` otherwise, which makes the
// cost of the drag fast path directly comparable to a normal frame.

static PROFILE_LOCK: Mutex<Option<File>> = Mutex::new(None);

/// True when the frame profiler is enabled (`DMULTRA_PROFILE=1`).
pub fn profile_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("DMULTRA_PROFILE").is_some())
}

/// Record one GUI frame. A single small append per frame is cheap enough not
/// to distort what it measures, and it survives a killed process (which is how
/// these tests end the app).
pub fn profile_frame(mode: &str, ms: f64) {
    if !profile_enabled() {
        return;
    }
    let mut guard = match PROFILE_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.is_none() {
        let _ = fs::create_dir_all(dir());
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir().join("profile.log"));
        *guard = file.ok();
    }
    if let Some(f) = guard.as_mut() {
        let line = format!(
            "{} FRAME {} {:.2}\n",
            chrono::Local::now().format("%H:%M:%S%.3f"),
            mode,
            ms
        );
        let _ = f.write_all(line.as_bytes());
        let _ = f.flush();
    }
}

pub fn log(tag: &str, msg: &str) {
    if !ENABLED.load(Ordering::SeqCst) {
        return;
    }
    let cell = LOGGER.get_or_init(|| Mutex::new(None));
    let mut guard = cell.lock().unwrap();
    if guard.is_none() {
        let _ = fs::create_dir_all(dir());
        if let Ok(f) = OpenOptions::new().create(true).append(true).open(path()) {
            *guard = Some(f);
        }
    }
    if let Some(f) = guard.as_mut() {
        let line = format!(
            "{} [{}] {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            tag,
            msg
        );
        let _ = f.write_all(line.as_bytes());
        let _ = f.flush();
    }
}
