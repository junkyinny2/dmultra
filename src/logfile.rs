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
