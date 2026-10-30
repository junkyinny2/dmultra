//! Minimal localisation loader (English only by decision).
//!
//! Loads `locales/<lang>.json` if present (a flat map of key -> string)
//! and falls back to the key itself when a translation is missing.
//! Only `en` is shipped; the infrastructure exists for future use.

use std::collections::BTreeMap;
use std::sync::OnceLock;

static STRINGS: OnceLock<BTreeMap<String, String>> = OnceLock::new();

pub fn init(lang: &str) {
    let _ = STRINGS.get_or_init(|| load(lang));
}

fn load(lang: &str) -> BTreeMap<String, String> {
    // Try next to the executable first, then the data dir.
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("locales").join(format!("{lang}.json")));
        }
    }
    if let Some(data) = dirs::data_dir() {
        candidates.push(data.join("dmultra").join("locales").join(format!("{lang}.json")));
    }
    for p in candidates {
        if let Ok(s) = std::fs::read_to_string(&p) {
            if let Ok(map) = serde_json::from_str::<BTreeMap<String, String>>(&s) {
                return map;
            }
        }
    }
    BTreeMap::new()
}

pub fn t(key: &str) -> String {
    STRINGS
        .get()
        .and_then(|m| m.get(key).cloned())
        .unwrap_or_else(|| key.to_string())
}
