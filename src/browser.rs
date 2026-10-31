//! Browser integration (IDM-style link interception).
//!
//! A small Chromium extension (Chrome/Edge, MV3) watches link clicks and
//! forwards matching URLs through the browser's **native messaging**
//! facility to this executable running as `dmultra nm`. The host drops
//! the URL into a local pending queue; a running GUI picks it up within
//! ~0.4 s and opens the Add-URL dialog (if the GUI is not running, the
//! host launches it). Everything is local: no network listener.

use anyhow::{Context, Result};
use serde_json::json;
use std::io::{Read, Write};
use std::path::PathBuf;

/// Native messaging host name (registered in the browser).
pub const NM_NAME: &str = "com.dmultra.nm";
/// Fixed extension id (derived from EXT_KEY, so it never changes).
pub const EXT_ID: &str = "pcnmkbbloiajlkefepnadpjkambhejjk";
/// Public key pinning the extension id (manifest "key").
pub const EXT_KEY: &str = "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAr1BeFI9PhsGMTpTft1kMyZjNzrKYPNATen+DoR7rb3UGJ8HTjGg89w7hzVdWcWXyOaYlmh1xe7cZ7em7X8Uj+rEZoHHt50ElB/hy05r6D8qt40MGdrkWIL800eVkh8d93UeXKFyDqH2D+i7cYDZGItiCz8g7g0dXo04hcly1QW8TocPeF0LiRSkVKYsXmqs0F81+T+TmYyds0Cnn8EJVN9LLb3kjpMaklhd4N9tV9Q79j956/c+ZhxviXwfa6pxTWrB2Yce13OHwhnbREhMoXOXF32246asMMlv/SF42H8wpZ3bd6T6n4C02iulpucpvx4usoLX67f93gWcRRNEwVQIDAQAB";

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn extension_dir() -> PathBuf {
    exe_dir().join("browser-extension")
}

fn pending_dir() -> PathBuf {
    crate::state::state_dir().join("pending")
}

// ---------------------------------------------------------------------------
// Registry plumbing (HKCU; no admin rights needed)
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod reg {
    pub const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
    const KEY_SET_VALUE: u32 = 0x0002;
    const KEY_CREATE_SUB_KEY: u32 = 0x0004;
    const REG_SZ: u32 = 1;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegCreateKeyExW(
            hkey: isize,
            subkey: *const u16,
            reserved: u32,
            class: *const u16,
            options: u32,
            access: u32,
            security: *const core::ffi::c_void,
            result: *mut isize,
            disposition: *mut u32,
        ) -> i32;
        fn RegSetValueW(
            hkey: isize,
            name: *const u16,
            typ: u32,
            data: *const u8,
            size: u32,
        ) -> i32;
        fn RegDeleteKeyW(hkey: isize, subkey: *const u16) -> i32;
        fn RegCloseKey(hkey: isize) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Set an HKCU registry key's default value to a string.
    pub fn set(hive_path: &str, value: &str) -> anyhow::Result<()> {
        unsafe {
            let mut hkey = 0isize;
            let rc = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(hive_path).as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_SET_VALUE | KEY_CREATE_SUB_KEY,
                std::ptr::null(),
                &mut hkey,
                std::ptr::null_mut(),
            );
            if rc != 0 {
                anyhow::bail!("RegCreateKeyExW({hive_path}) failed: {rc}");
            }
            let mut data = wide(value);
            data.pop(); // REG_SZ uses a single NUL
            let mut bytes = Vec::with_capacity(data.len() * 2 + 2);
            for u in data {
                bytes.extend_from_slice(&u.to_le_bytes());
            }
            bytes.extend_from_slice(&0u16.to_le_bytes());
            let rc = RegSetValueW(
                hkey,
                std::ptr::null(),
                REG_SZ,
                bytes.as_ptr(),
                bytes.len() as u32,
            );
            RegCloseKey(hkey);
            if rc != 0 {
                anyhow::bail!("RegSetValueW({hive_path}) failed: {rc}");
            }
        }
        Ok(())
    }

    /// Delete an HKCU registry key.
    pub fn delete(hive_path: &str) -> anyhow::Result<()> {
        unsafe {
            let rc = RegDeleteKeyW(HKEY_CURRENT_USER, wide(hive_path).as_ptr());
            if rc != 0 && rc != 2 {
                // 2 = does not exist: fine.
                anyhow::bail!("RegDeleteKeyW({hive_path}) failed: {rc}");
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod reg {
    pub fn set(_hive_path: &str, _value: &str) -> Result<()> {
        Ok(())
    }
    pub fn delete(_hive_path: &str) -> Result<()> {
        Ok(())
    }
}

const NM_REGISTRY_PATHS: [&str; 4] = [
    r"Software\Google\Chrome\NativeMessagingHosts",
    r"Software\Chromium\NativeMessagingHosts",
    r"Software\Microsoft\Edge\NativeMessagingHosts",
    r"Software\Mozilla\NativeMessaging\Hosts",
];

// ---------------------------------------------------------------------------
// Extension files (Chromium MV3)
// ---------------------------------------------------------------------------

fn manifest_json() -> String {
    format!(
        r#"{{
  "manifest_version": 3,
  "key": "{EXT_KEY}",
  "name": "DM Ultra Integration",
  "version": "{}",
  "description": "Opens download links in DM Ultra",
  "permissions": ["nativeMessaging", "contextMenus", "tabs", "webRequest"],
  "host_permissions": ["<all_urls>"],
  "background": {{"service_worker": "background.js"}},
  "content_scripts": [
    {{"matches": ["<all_urls>"], "js": ["content.js"], "run_at": "document_start", "all_frames": true}}
  ],
  "browser_specific_settings": {{
    "gecko": {{"id": "dmultra@dmultra.local", "strict_min_version": "115.0"}}
  }}
}}
"#,
        env!("CARGO_PKG_VERSION")
    )
}

const CONTENT_JS: &str = include_str!("../browser-extension/content.js");













const BACKGROUND_JS: &str = r#"// Native messaging bridge to DM Ultra.
const HOST = 'com.dmultra.nm';
function send(msg) {
  chrome.runtime.sendNativeMessage(HOST, msg, () => {
    if (chrome.runtime.lastError) {
      console.warn('DM Ultra:', chrome.runtime.lastError.message);
    }
  });
}
chrome.runtime.onMessage.addListener((msg) => {
  // Forward video overlay updates directly to the native host.
  if (msg && (msg.action === 'videoOverlay' || msg.action === 'videoOverlayHide')) {
    send(msg);
    return;
  }
  if (msg && typeof msg.url === 'string' && msg.url) send(msg);
  if (msg && Array.isArray(msg.urls) && msg.urls.length > 0) {
    send({ urls: msg.urls, referer: msg.referer || '' });
  }
});
function requestFromPage(action, tab) {
  if (tab && tab.id != null) {
    chrome.tabs.sendMessage(tab.id, { action }, () => {
      void chrome.runtime.lastError;
    });
  }
}
// ---------------------------------------------------------------------------
// Network-level video detection (IDM-style): intercept media URLs from
// network requests, not just DOM attributes. This catches videos that
// use MSE, blob: URLs, or lazy-loaded players.
// ---------------------------------------------------------------------------
const NET_MEDIA_RE = /\.(m3u8|mpd|mp4|webm|mkv|flv|mp3|m4a|mpg|mpeg|wav|ogg|aac)(\?|#|$)/i;
// Track best video per tab: prefer playlists and direct files over segments.
const tabVideos = new Map(); // tabId -> {url, format, priority}
function videoPriority(fmt) {
  if (fmt === 'm3u8' || fmt === 'mp4') return 3;
  if (fmt === 'mpd' || fmt === 'webm' || fmt === 'mkv') return 2;
  return 1;
}
chrome.webRequest.onBeforeRequest.addListener(
  (details) => {
    if (details.tabId < 0) return;
    const url = details.url || '';
    if (!/^https?:/i.test(url)) return;
    const m = url.match(NET_MEDIA_RE);
    if (!m) return;
    const format = m[1].toLowerCase();
    const prio = videoPriority(format);
    if (prio < 1) return;
    const existing = tabVideos.get(details.tabId);
    if (!existing || prio > existing.priority) {
      tabVideos.set(details.tabId, { url, format, priority: prio });
      // Notify the content script in that tab.
      chrome.tabs.sendMessage(details.tabId, {
        action: 'videoUrlFound',
        url: url,
        format: format,
      }, () => { void chrome.runtime.lastError; });
    }
  },
  { urls: ['<all_urls>'] },
  []
);
// Clean up on tab close.
chrome.tabs.onRemoved.addListener((tabId) => {
  tabVideos.delete(tabId);
});
// Handle "what videos do you know about" queries from the content script.
chrome.runtime.onMessage.addListener((msg, sender) => {
  if (msg && msg.action === 'queryVideos' && sender.tab && sender.tab.id != null) {
    const v = tabVideos.get(sender.tab.id);
    if (v) {
      chrome.tabs.sendMessage(sender.tab.id, {
        action: 'videoUrlFound',
        url: v.url,
        format: v.format,
      }, () => { void chrome.runtime.lastError; });
    }
  }
});
chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: 'dmultra-link',
    title: 'Download with DM Ultra',
    contexts: ['link'],
  });
  chrome.contextMenus.create({
    id: 'dmultra-grab',
    title: 'Grab download links from this page',
    contexts: ['page'],
  });
  chrome.contextMenus.create({
    id: 'dmultra-video',
    title: 'Download videos from this page',
    contexts: ['page'],
  });
});
chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === 'dmultra-link' && info.linkUrl &&
      /^https?:/i.test(info.linkUrl)) {
    send({
      url: info.linkUrl,
      referer: (tab && tab.url) || '',
      filename: '',
    });
  }
  if (info.menuItemId === 'dmultra-grab') {
    requestFromPage('grabLinks', tab);
  }
  if (info.menuItemId === 'dmultra-video') {
    requestFromPage('grabVideos', tab);
  }
});
"#;

// ---------------------------------------------------------------------------
// Packaging: .xpi (Firefox) and .crx (Chrome) ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Â¦Ãƒâ€šÃ‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â minimal ZIP + CRX3 writers
// ---------------------------------------------------------------------------

/// Precomputed CRX3 header (public key + RSA signature over SignedData),
/// created with the project's dev signing key.
const CRX3_HEADER: &[u8] = &[0x12, 0x82, 0x02, 0x2e, 0x12, 0x82, 0x01, 0x26, 0x30, 0x82, 0x01, 0x22, 0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00, 0x03, 0x82, 0x01, 0x0f, 0x00, 0x30, 0x82, 0x01, 0x0a, 0x02, 0x82, 0x01, 0x01, 0x00, 0xaf, 0x50, 0x5e, 0x14, 0x8f, 0x4f, 0x86, 0xc1, 0x8c, 0x4e, 0x94, 0xdf, 0xb7, 0x59, 0x0c, 0xc9, 0x98, 0xcd, 0xce, 0xb2, 0x98, 0x3c, 0xd0, 0x13, 0x7a, 0x7f, 0x83, 0xa1, 0x1e, 0xeb, 0x6f, 0x75, 0x06, 0x27, 0xc1, 0xd3, 0x8c, 0x68, 0x3c, 0xf7, 0x0e, 0xe1, 0xcd, 0x57, 0x56, 0x71, 0x65, 0xf2, 0x39, 0xa6, 0x25, 0x9a, 0x1d, 0x71, 0x7b, 0xb7, 0x19, 0xed, 0xe9, 0xbb, 0x5f, 0xc5, 0x23, 0xfa, 0xb1, 0x19, 0xa0, 0x71, 0xed, 0xe7, 0x41, 0x25, 0x07, 0xf8, 0x72, 0xd3, 0x9a, 0xfa, 0x0f, 0xca, 0xad, 0xe3, 0x43, 0x06, 0x76, 0xb9, 0x16, 0x20, 0xbf, 0x34, 0xd1, 0xe5, 0x64, 0x87, 0xc7, 0x7d, 0xdd, 0x47, 0x97, 0x28, 0x5c, 0x83, 0xa8, 0x7d, 0x83, 0xfa, 0x2e, 0xdc, 0x60, 0x36, 0x46, 0x22, 0xd8, 0x82, 0xcf, 0xc8, 0x3b, 0x83, 0x47, 0x57, 0xa3, 0x4e, 0x21, 0x72, 0x5c, 0xb5, 0x41, 0x6f, 0x13, 0xa1, 0xc3, 0xde, 0x17, 0x42, 0xe2, 0x45, 0x29, 0x15, 0x29, 0x8b, 0x17, 0x9a, 0xab, 0x34, 0x17, 0xcd, 0x7e, 0x4f, 0xe4, 0xe6, 0x63, 0x27, 0x6c, 0xd0, 0x29, 0xe7, 0xf0, 0x42, 0x55, 0x37, 0xd2, 0xcb, 0x6f, 0x79, 0x23, 0xa4, 0xc6, 0xa4, 0x96, 0x17, 0x78, 0x37, 0xdb, 0x55, 0xf5, 0x0e, 0xfd, 0x8f, 0xde, 0x7a, 0xfd, 0xcf, 0x99, 0x87, 0x1b, 0xe2, 0x5f, 0x07, 0xda, 0xea, 0x9c, 0x53, 0x5a, 0xb0, 0x76, 0x61, 0xc7, 0xb5, 0xdc, 0xe1, 0xf0, 0x86, 0x76, 0xd1, 0x12, 0x13, 0x28, 0x5c, 0xe5, 0xc5, 0xdf, 0x6d, 0xb8, 0xe9, 0xab, 0x0c, 0x32, 0x5b, 0xff, 0x48, 0x5e, 0x36, 0x1f, 0xcc, 0x29, 0x67, 0x76, 0xdd, 0xe9, 0x3e, 0xa7, 0xe0, 0x2d, 0x36, 0x8a, 0xe9, 0x69, 0xb9, 0xca, 0x6f, 0xc7, 0x8b, 0xac, 0xa0, 0xb5, 0xfa, 0xed, 0xff, 0x77, 0x81, 0x67, 0x11, 0x44, 0xd1, 0x30, 0x55, 0x02, 0x03, 0x01, 0x00, 0x01, 0x1a, 0x82, 0x01, 0x00, 0x8d, 0x90, 0xb0, 0xfc, 0xcc, 0xb1, 0xb8, 0x95, 0x68, 0x83, 0x7c, 0xb9, 0x90, 0xad, 0xc9, 0x10, 0x35, 0x96, 0xd4, 0xbb, 0x6d, 0xad, 0xbb, 0xa6, 0x23, 0xd0, 0x4a, 0xe4, 0xd3, 0x41, 0xaf, 0xd0, 0x3c, 0x89, 0xb0, 0x91, 0xd4, 0x01, 0x44, 0xaf, 0x92, 0x1b, 0x49, 0x41, 0x38, 0x56, 0x18, 0x92, 0xe7, 0xe2, 0x67, 0xa0, 0x61, 0x07, 0xf0, 0x78, 0xde, 0xa6, 0xd5, 0x84, 0xc9, 0x21, 0x88, 0x2d, 0xa5, 0x1f, 0x34, 0xe4, 0x33, 0xa8, 0xfa, 0xe2, 0x34, 0x09, 0xad, 0x8e, 0x39, 0x83, 0x94, 0xf3, 0xea, 0x66, 0x31, 0xea, 0xbc, 0xc0, 0x36, 0xf0, 0xa0, 0xb1, 0xfb, 0xbe, 0x9a, 0xec, 0xb5, 0x26, 0xd9, 0x33, 0xc9, 0x98, 0xfa, 0x96, 0x7d, 0x24, 0x6e, 0x51, 0x6b, 0x8c, 0xa4, 0x6e, 0x35, 0x4c, 0xa7, 0x0f, 0x95, 0x3d, 0x72, 0x59, 0x80, 0xb5, 0x9e, 0x29, 0xf6, 0x9b, 0xa1, 0xb7, 0xa8, 0x28, 0x6d, 0xe5, 0x2f, 0x00, 0xb3, 0x46, 0x66, 0x25, 0xfc, 0x69, 0x88, 0xfb, 0x0e, 0xe5, 0x2e, 0xd5, 0xc9, 0x69, 0x50, 0xc0, 0x49, 0xfa, 0xc4, 0x5f, 0xce, 0x93, 0xc0, 0x4e, 0x6c, 0x52, 0x41, 0x5c, 0x9c, 0x00, 0x52, 0xb2, 0xdd, 0x0c, 0xf7, 0xee, 0x1d, 0x7c, 0x20, 0x3f, 0xc6, 0x5f, 0xb2, 0xf7, 0x4e, 0xa4, 0xf7, 0xaa, 0x38, 0xa1, 0x42, 0xcf, 0x82, 0x31, 0xd8, 0xd1, 0x43, 0x05, 0xe4, 0x8a, 0x2b, 0x86, 0xdc, 0x39, 0x51, 0x8a, 0xfc, 0x17, 0x20, 0x2e, 0x80, 0xc8, 0xce, 0x66, 0xa8, 0x6a, 0x4b, 0xad, 0xaa, 0x8d, 0x3a, 0xf2, 0x0d, 0x66, 0x11, 0xcf, 0x51, 0x10, 0xd5, 0x3d, 0x85, 0x83, 0xa9, 0x33, 0x6b, 0x28, 0x11, 0xfc, 0xb3, 0x0e, 0x73, 0xe5, 0x57, 0x83, 0x89, 0x46, 0xe9, 0xed, 0xca, 0xe1, 0x16, 0xce, 0x48, 0xc0, 0x4b, 0xcf, 0xac, 0xac, 0xb4, 0x21, 0xbe, 0xd3, 0x62, 0xfc, 0x82, 0xf1, 0x04, 0x12, 0x0a, 0x10, 0xf2, 0xdc, 0xa1, 0x1b, 0xe8, 0x09, 0xba, 0x45, 0x4f, 0xd0, 0x3f, 0x9a, 0x0c, 0x17, 0x49, 0x9a];

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Minimal ZIP writer (stored entries) ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Â¦Ãƒâ€šÃ‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â enough for .xpi/.crx payloads.
struct ZipBuilder {
    out: Vec<u8>,
    dir: Vec<u8>,
    count: u16,
}

impl ZipBuilder {
    fn new() -> Self {
        Self { out: Vec::new(), dir: Vec::new(), count: 0 }
    }
    fn add(&mut self, name: &str, data: &[u8]) {
        let crc = crc32(data);
        let offset = self.out.len() as u32;
        let name_b = name.as_bytes();
        // local file header
        self.out.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        self.out.extend_from_slice(&[20, 0, 0, 0, 0, 0]); // version, flags, method=stored
        self.out.extend_from_slice(&[0, 0, 0, 0]); // time, date
        self.out.extend_from_slice(&crc.to_le_bytes());
        self.out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.out.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes());
        self.out.extend_from_slice(name_b);
        self.out.extend_from_slice(data);
        // central directory entry
        self.dir.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
        self.dir.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0]); // made-by, needed, flags, method=stored
        self.dir.extend_from_slice(&[0, 0, 0, 0]);
        self.dir.extend_from_slice(&crc.to_le_bytes());
        self.dir.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.dir.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.dir.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
        self.dir.extend_from_slice(&[0u8; 12]); // extra/comment/disk/attrs
        self.dir.extend_from_slice(&offset.to_le_bytes());
        self.dir.extend_from_slice(name_b);
        self.count += 1;
    }
    fn finish(mut self) -> Vec<u8> {
        let dir_offset = self.out.len() as u32;
        let dir_size = self.dir.len() as u32;
        self.out.extend_from_slice(&self.dir);
        // end of central directory
        self.out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
        self.out.extend_from_slice(&[0u8; 4]); // disk numbers
        self.out.extend_from_slice(&self.count.to_le_bytes());
        self.out.extend_from_slice(&self.count.to_le_bytes());
        self.out.extend_from_slice(&dir_size.to_le_bytes());
        self.out.extend_from_slice(&dir_offset.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes());
        self.out
    }
}

/// Firefox MV2 variant of the manifest (background scripts instead of a
/// service worker; `browser_specific_settings` carries the add-on id).
fn manifest_firefox_json() -> String {
    format!(
        r#"{{
  "manifest_version": 2,
  "name": "DM Ultra Integration",
  "version": "{}",
  "description": "Opens download links in DM Ultra",
  "permissions": ["nativeMessaging", "contextMenus", "<all_urls>"],
  "background": {{"scripts": ["background.js"]}},
  "content_scripts": [
    {{"matches": ["<all_urls>"], "js": ["content.js"], "run_at": "document_start", "all_frames": true}}
  ],
  "browser_specific_settings": {{
    "gecko": {{"id": "dmultra@dmultra.local", "strict_min_version": "115.0"}}
  }}
}}
"#,
        env!("CARGO_PKG_VERSION")
    )
}

/// Build the Firefox package (.xpi) next to the extension folder.
fn build_xpi(dir: &std::path::Path) -> Result<std::path::PathBuf> {
    let mut zip = ZipBuilder::new();
    zip.add("manifest.json", manifest_firefox_json().as_bytes());
    zip.add("content.js", CONTENT_JS.as_bytes());
    zip.add("background.js", BACKGROUND_JS.as_bytes());
    let path = dir.parent().unwrap_or(dir).join("dmultra-firefox.xpi");
    std::fs::write(&path, zip.finish()).context("write .xpi")?;
    Ok(path)
}

/// Build the Chrome package (.crx, CRX3) next to the extension folder.
fn build_crx(dir: &std::path::Path) -> Result<std::path::PathBuf> {
    let mut zip = ZipBuilder::new();
    zip.add("manifest.json", manifest_json().as_bytes());
    zip.add("content.js", CONTENT_JS.as_bytes());
    zip.add("background.js", BACKGROUND_JS.as_bytes());
    let archive = zip.finish();
    let mut crx = Vec::with_capacity(12 + CRX3_HEADER.len() + archive.len());
    crx.extend_from_slice(b"Cr24");
    crx.extend_from_slice(&3u32.to_le_bytes());
    crx.extend_from_slice(&(CRX3_HEADER.len() as u32).to_le_bytes());
    crx.extend_from_slice(CRX3_HEADER);
    crx.extend_from_slice(&archive);
    let path = dir.parent().unwrap_or(dir).join("dmultra-chrome.crx");
    std::fs::write(&path, crx).context("write .crx")?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Install / uninstall
// ---------------------------------------------------------------------------

/// Install the native messaging host + extension files. Returns the
/// extension directory (for the user to load in the browser).
pub fn install() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("resolve current exe")?;
    // Prefer the dedicated native messaging host binary when it is
    // deployed next to the main exe (console subsystem -> the browser
    // launches it directly, no cmd.exe wrapper).
    let nmh = exe_dir().join("dmultra-nmh.exe");
    let host_binary = if nmh.exists() { nmh } else { exe };
    let dir = extension_dir();
    std::fs::create_dir_all(&dir).context("create extension dir")?;

    std::fs::write(dir.join("manifest.json"), manifest_json())
        .context("write extension manifest")?;
    std::fs::write(dir.join("content.js"), CONTENT_JS)
        .context("write content script")?;
    std::fs::write(dir.join("background.js"), BACKGROUND_JS)
        .context("write background script")?;

    // Native messaging host manifest.
    let native = crate::state::state_dir().join("native");
    std::fs::create_dir_all(&native).context("create native dir")?;
    let hosts = format!(
        r#"{{
  "name": "{NM_NAME}",
  "description": "DM Ultra browser integration",
  "path": {json_path},
  "type": "stdio",
  "allowed_origins": ["chrome-extension://{EXT_ID}/"],
  "allowed_extensions": ["dmultra@dmultra.local"]
}}
"#,
        json_path = serde_json::to_string(&host_binary.display().to_string()).unwrap_or_default(),
    );
    let host_path = native.join(format!("{NM_NAME}.json"));
    std::fs::write(&host_path, hosts).context("write host manifest")?;

    for base in NM_REGISTRY_PATHS {
        reg::set(&format!(r"{base}\{NM_NAME}"), &host_path.display().to_string())
            .with_context(|| format!("register {base}"))?;
    }
    // The browser launches the host inside an AppContainer sandbox, so
    // the host can only write to locations granting ALL APPLICATION
    // PACKAGES access. Grant that on the pending queue (and the native
    // manifest folder so the trace log works).
    {
        use std::os::windows::process::CommandExt;
        let grant = |path: &str| {
            let _ = std::process::Command::new("icacls.exe")
                .arg(path)
                .args(["/grant", "*S-1-15-2-1:(OI)(CI)F"])
                .creation_flags(0x0800_0000)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        };
        let pending = crate::state::state_dir().join("pending");
        let _ = std::fs::create_dir_all(&pending);
        grant(&pending.display().to_string());
        grant(&native.display().to_string());
    }
    let _ = build_xpi(&dir)?;
    let _ = build_crx(&dir)?;
    Ok(dir)
}

/// Remove the native messaging registration and the extension files.
pub fn uninstall() -> Result<()> {
    for base in NM_REGISTRY_PATHS {
        let _ = reg::delete(&format!(r"{base}\{NM_NAME}"));
    }
    let _ = std::fs::remove_dir_all(extension_dir());
    let _ = std::fs::remove_file(
        crate::state::state_dir()
            .join("native")
            .join(format!("{NM_NAME}.json")),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Pending URL queue (host -> GUI)
// ---------------------------------------------------------------------------

/// Queue files are named `<unix_nanos>.json`. Other JSON files in the
/// pending directory (e.g. overlay control messages) are not downloads.
fn is_pending_queue_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".json") else {
        return false;
    };
    !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit())
}

/// Write the video-overlay control message for the running GUI. Uses
/// write-then-rename so a poll can never observe a half-written payload.
fn write_overlay_file(msg: &serde_json::Value, action: &str) {
    let dir = pending_dir();
    let _ = std::fs::create_dir_all(&dir);
    let target = dir.join("overlay.json");
    let tmp = dir.join("overlay.tmp");
    let body = serde_json::to_string(msg).unwrap_or_default();
    let _ = std::fs::write(&tmp, body.as_bytes());
    if std::fs::rename(&tmp, &target).is_err() {
        // Fallback for filesystems that refuse the replace.
        let _ = std::fs::write(&target, body.as_bytes());
    }
    trace(&format!("host: overlay written ({action})"));
}

/// A queued browser request is only worth acting on while a GUI is around to
/// consume it. The GUI polls this queue every 250 ms, so anything older than
/// this was captured while DM Ultra was closed (or stalled) and must be
/// discarded instead of being downloaded en masse when the app starts.
const PENDING_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(60);

/// Pull the oldest pending browser message. Returns `(urls, referer)`:
/// `urls` has one entry for single-URL messages or many for batches
/// ("Grab links" / "Download videos" from the extension).
pub fn take_pending() -> Result<Option<(Vec<String>, String)>> {
    let dir = pending_dir();
    if !dir.exists() {
        return Ok(None);
    }
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .with_context(|| format!("read browser pending directory {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("enumerate browser pending directory {}", dir.display()))?
        .into_iter()
        .filter(|e| is_pending_queue_name(&e.file_name().to_string_lossy()))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        // Drop links captured while DM Ultra was not running. Without this
        // the whole backlog was downloaded in one burst on the next launch.
        let age = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok());
        if let Some(age) = age {
            if age > PENDING_MAX_AGE {
                let _ = std::fs::remove_file(&path);
                crate::logfile::log(
                    "BROWSER",
                    &format!(
                        "discarded stale browser request ({:.0}s old): {}",
                        age.as_secs_f32(),
                        path.display()
                    ),
                );
                continue;
            }
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("read browser request {}", path.display()))?;
        let v = match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(value) => value,
            Err(error) => {
                std::fs::remove_file(&path)
                    .with_context(|| format!("remove invalid browser request {}", path.display()))?;
                crate::logfile::log("BROWSER", &format!("discarded invalid request: {error}"));
                continue;
            }
        };
        // Control messages (e.g. video overlay) are handled separately and
        // must never enter the download queue.
        if v.get("action").is_some() {
            continue;
        }
        std::fs::remove_file(&path)
            .with_context(|| format!("consume browser request {}", path.display()))?;
        let referer = v
            .get("referer")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        // Batch message: {"urls": [...]}
        if let Some(list) = v.get("urls").and_then(|x| x.as_array()) {
            let urls: Vec<String> = list
                .iter()
                .filter_map(|u| u.as_str())
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .collect();
            if !urls.is_empty() {
                return Ok(Some((urls, referer)));
            }
        }
        let url = v.get("url").and_then(|x| x.as_str()).unwrap_or("");
        if !url.is_empty() {
            return Ok(Some((vec![url.to_string()], referer)));
        }
    }
    Ok(None)
}

#[cfg(windows)]
fn gui_running() -> bool {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(callback: extern "system" fn(isize, isize) -> i32, context: isize) -> i32;
        fn GetWindowTextW(hwnd: isize, text: *mut u16, max_count: i32) -> i32;
    }

    extern "system" fn find_main_window(hwnd: isize, context: isize) -> i32 {
        let mut title = [0u16; 256];
        let length = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
        if length > 0 {
            let title = String::from_utf16_lossy(&title[..length as usize]);
            if title == "DM Ultra" || title.starts_with("DM Ultra v") {
                unsafe {
                    *(context as *mut bool) = true;
                }
                return 0;
            }
        }
        1
    }

    let mut found = false;
    unsafe {
        EnumWindows(find_main_window, &mut found as *mut bool as isize);
    }
    found
}

#[cfg(not(windows))]
fn gui_running() -> bool {
    false
}

fn ensure_gui_running() {
    if gui_running() {
        return;
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("dmultra.exe"));
    #[cfg(windows)]
    let spawn = {
        use std::os::windows::process::CommandExt;
        std::process::Command::new(exe)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
    };
    #[cfg(not(windows))]
    let spawn = std::process::Command::new(exe)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let _ = spawn;
}

// ---------------------------------------------------------------------------
// Native messaging host (`dmultra nm`)
// ---------------------------------------------------------------------------

/// Write a best-effort trace line (native messaging host debugging). The
/// host runs inside the browser's AppContainer sandbox, so it tries
/// several locations and gives up silently.
fn trace(msg: &str) {
    let line = format!("{msg}\r\n");
    for dir in [
        crate::state::state_dir().join("pending"),
        std::env::temp_dir(),
        exe_dir(),
    ] {
        if std::fs::write(dir.join("nm-trace.log"), &line).is_ok() {
            return;
        }
    }
}

/// Length-prefixed JSON over stdio, per the native messaging spec.
/// Runs until the browser closes the pipe.
pub fn host_loop() -> Result<()> {
    trace("host: started");
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    loop {
        let mut len_buf = [0u8; 4];
        if stdin.read_exact(&mut len_buf).is_err() {
            trace("host: stdin closed");
            break; // pipe closed -> exit quietly
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 || len > 1_000_000 {
            trace("host: bad length");
            break;
        }
        let mut buf = vec![0u8; len];
        if stdin.read_exact(&mut buf).is_err() {
            break;
        }
        let msg: serde_json::Value = serde_json::from_slice(&buf).unwrap_or_default();
        // Video overlay messages: write to the dedicated overlay file that
        // the GUI watches. Never create the overlay window in the native
        // messaging host process - it is short-lived and sandboxed, and the
        // window would outlive its data (the "flashing button" bug).
        let action = msg.get("action").and_then(|v| v.as_str()).unwrap_or("");
        if action == "videoOverlay" || action == "videoOverlayHide" {
            write_overlay_file(&msg, action);
            let reply = br#"{"ok":true}"#;
            let _ = std::io::stdout().write_all(&(reply.len() as u32).to_le_bytes());
            let _ = std::io::stdout().write_all(reply);
            let _ = std::io::stdout().flush();
            trace("host: overlay reply sent");
            continue;
        }
        let urls: Vec<String> = if let Some(list) = msg.get("urls").and_then(|v| v.as_array()) {
            list.iter()
                .filter_map(|u| u.as_str())
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .collect()
        } else {
            let url = msg
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if url.is_empty() {
                Vec::new()
            } else {
                vec![url]
            }
        };
        let referer = msg
            .get("referer")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        trace(&format!("host: got {} url(s)", urls.len()));
        if !urls.is_empty() {
            let dir = pending_dir();
            if let Err(e) = std::fs::create_dir_all(&dir) {
                trace(&format!("host: create pending dir failed: {e}"));
            }
            let stamp = crate::state::unix_now() * 1_000_000_000
                + u64::try_from(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.subsec_nanos())
                        .unwrap_or(0),
                )
                .unwrap_or(0);
            let payload = if urls.len() == 1 {
                json!({"url": urls[0], "referer": referer}).to_string()
            } else {
                json!({"urls": urls, "referer": referer}).to_string()
            };
            match std::fs::write(dir.join(format!("{stamp}.json")), payload) {
                Ok(()) => trace("host: pending written"),
                Err(e) => trace(&format!("host: pending write failed: {e}")),
            }
            ensure_gui_running();
        }
        let reply = br#"{"ok":true}"#;
        let _ = std::io::stdout().write_all(&(reply.len() as u32).to_le_bytes());
        let _ = std::io::stdout().write_all(reply);
        let _ = std::io::stdout().flush();
        trace("host: replied");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Wake a minimized window when a browser file arrives
// ---------------------------------------------------------------------------

/// True when at least one queued download request is waiting.
fn has_pending_download() -> bool {
    let Ok(entries) = std::fs::read_dir(pending_dir()) else {
        return false;
    };
    entries.flatten().any(|entry| {
        is_pending_queue_name(&entry.file_name().to_string_lossy())
    })
}

/// eframe 0.27 deliberately skips scheduled repaints for a minimized window
/// (`run.rs`: `if is_minimized { false }`), so the pending queue was only
/// read after the user restored the main window. A browser click while
/// minimized therefore looked like it did nothing.
///
/// This watcher posts a synthetic `WM_SIZE` to the main window. winit turns
/// every `WM_SIZE` into `WindowEvent::Resized`, and eframe treats a resize as
/// an *immediate* repaint (`EventResult::RepaintNow`), which runs one frame
/// even while minimized. That frame reads the queue, starts the download and
/// opens its progress window; the main window stays minimized and is never
/// resized for real.
#[cfg(windows)]
pub fn spawn_pending_waker() {
    let _ = std::thread::Builder::new()
        .name("pending-waker".into())
        .spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_millis(300));
            if !has_pending_download() {
                continue;
            }
            let Some(hwnd) = wakeup::main_window() else {
                continue;
            };
            if wakeup::needs_wake(hwnd) {
                wakeup::post_resize(hwnd);
            }
        });
}

#[cfg(not(windows))]
pub fn spawn_pending_waker() {}

/// Keep the GUI able to paint while the main window is minimized or hidden.
///
/// Immediate viewports (the progress window, dialogs) are rendered as part
/// of the *parent* window's frame; eframe skips frame scheduling for
/// minimized windows, so a progress window opened from the tray would show
/// its first frame (0%) and then freeze even though the download keeps
/// running. While `busy` is set, post a synthetic `WM_SIZE` so winit keeps
/// producing frames. See [`wakeup::post_resize`].
///
/// The same thread also implements "minimize to tray only": Windows reports
/// a minimize as a zero-sized resize, and eframe drops those, so the app
/// never gets a frame to notice the iconified state. While `watch_minimize`
/// is set (tray-only mode with the window still shown) it wakes the app so
/// the frame can un-minimize and hide the window.
///
/// Finally, a hidden window is not minimized, but Windows still delivers no
/// paint there, so eframe's scheduled repaints never run. A slow pulse keeps
/// such a window alive for the scheduler, the clipboard monitor, a tray click
/// and a second-instance show request.
#[cfg(windows)]
pub fn spawn_busy_waker(
    busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    watch_minimize: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    let mut last_idle_wake = std::time::Instant::now();
    let _ = std::thread::Builder::new()
        .name("busy-waker".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(150));
            let busy_now = busy.load(Ordering::Relaxed);
            let watch = watch_minimize.load(Ordering::Relaxed);
            let Some(hwnd) = wakeup::main_window() else {
                continue;
            };
            if busy_now && wakeup::needs_wake(hwnd) {
                wakeup::post_resize(hwnd);
                last_idle_wake = std::time::Instant::now();
            } else if watch && wakeup::is_iconic(hwnd) {
                wakeup::post_resize(hwnd);
                last_idle_wake = std::time::Instant::now();
            } else if wakeup::needs_wake(hwnd)
                && last_idle_wake.elapsed() >= std::time::Duration::from_millis(500)
            {
                wakeup::post_resize(hwnd);
                last_idle_wake = std::time::Instant::now();
            }
        });
}

#[cfg(not(windows))]
pub fn spawn_busy_waker(
    _busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    _watch_minimize: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
}

/// Handle of a running DM Ultra main window (in any process), if one
/// exists. Used to avoid starting a second GUI instance.
#[cfg(windows)]
pub fn gui_window_handle() -> Option<isize> {
    wakeup::gui_window()
}

#[cfg(not(windows))]
pub fn gui_window_handle() -> Option<isize> {
    None
}

/// Wake a hidden or minimized DM Ultra window so it runs a frame now.
///
/// Tray clicks and a second-instance show request only set flags that are
/// read inside a frame, and Windows delivers no paint to a hidden window;
/// posting `WM_SIZE` is what makes winit/eframe produce that frame.
#[cfg(windows)]
pub fn wake_window(hwnd: isize) {
    if wakeup::needs_wake(hwnd) {
        wakeup::post_resize(hwnd);
    }
}

#[cfg(not(windows))]
pub fn wake_window(_hwnd: isize) {}

/// Wake this process' main window (used by the tray listeners).
#[cfg(windows)]
pub fn wake_main_window() {
    if let Some(hwnd) = wakeup::main_window() {
        wake_window(hwnd);
    }
}

/// Wake this process' main window unconditionally (used when a child window
/// is being closed, so the root frame runs and destroys it without delay).
#[cfg(windows)]
pub fn wake_main_window_now() {
    if let Some(hwnd) = wakeup::main_window() {
        wakeup::post_resize(hwnd);
    }
}

#[cfg(not(windows))]
pub fn wake_main_window_now() {}

/// Subclass popup dialog windows so their title-bar X hides them instantly.
#[cfg(windows)]
pub fn subclass_popups() {
    wakeup::subclass_popups();
}

#[cfg(not(windows))]
pub fn subclass_popups() {}

#[cfg(windows)]
mod wakeup {
    use std::ffi::c_int;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct WindowPlacement {
        length: u32,
        flags: u32,
        show_cmd: u32,
        pt_min: Point,
        pt_max: Point,
        rc_normal: Rect,
    }

    const WM_SIZE: u32 = 0x0005;
    const SIZE_RESTORED: usize = 0;
    const SIZE_MAXIMIZED: usize = 2;
    /// `WPF_RESTORETOMAXIMIZED`: the window was maximized before minimizing.
    const WPF_RESTORE_TO_MAXIMIZED: u32 = 0x0002;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(cb: extern "system" fn(isize, isize) -> i32, lparam: isize) -> i32;
        fn GetWindowTextW(hwnd: isize, buf: *mut u16, max: i32) -> i32;
        fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
        pub(super) fn IsIconic(hwnd: isize) -> c_int;
        pub(super) fn IsWindowVisible(hwnd: isize) -> c_int;
        pub(super) fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        pub(super) fn InvalidateRect(hwnd: isize, rect: isize, erase: i32) -> i32;
        pub(super) fn SetWindowLongPtrW(hwnd: isize, index: i32, new_long: isize) -> isize;
        pub(super) fn CallWindowProcW(prev: isize, hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
        fn GetWindowPlacement(hwnd: isize, placement: *mut WindowPlacement) -> c_int;
        fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> c_int;
    }

    #[link(name = "comctl32")]
    unsafe extern "system" {
        pub(super) fn SetWindowSubclass(
            hwnd: isize,
            proc: extern "system" fn(isize, u32, usize, isize, usize, usize) -> isize,
            subclass_id: usize,
            ref_data: usize,
        ) -> i32;
        pub(super) fn DefSubclassProc(
            hwnd: isize,
            msg: u32,
            wparam: usize,
            lparam: isize,
        ) -> isize;
    }

    extern "system" fn find_main(hwnd: isize, found: isize) -> i32 {
        let mut pid: u32 = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        if pid != std::process::id() {
            return 1;
        }
        let mut buf = [0u16; 256];
        let len = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        if len > 0 {
            let title = String::from_utf16_lossy(&buf[..len as usize]);
            if title == "DM Ultra" || title.starts_with("DM Ultra v") {
                unsafe {
                    *(found as *mut isize) = hwnd;
                }
                return 0;
            }
        }
        1
    }

    /// HWND of this process' main window, if it exists.
    pub(super) fn main_window() -> Option<isize> {
        let mut found: isize = 0;
        unsafe {
            EnumWindows(find_main, &mut found as *mut isize as isize);
        }
        (found != 0).then_some(found)
    }

    extern "system" fn find_gui(hwnd: isize, found: isize) -> i32 {
        let mut buf = [0u16; 256];
        let len = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        if len > 0 {
            let title = String::from_utf16_lossy(&buf[..len as usize]);
            if title == "DM Ultra" || title.starts_with("DM Ultra v") {
                unsafe {
                    *(found as *mut isize) = hwnd;
                }
                return 0;
            }
        }
        1
    }

    /// HWND of any running DM Ultra main window (any process).
    pub(super) fn gui_window() -> Option<isize> {
        let mut found: isize = 0;
        unsafe {
            EnumWindows(find_gui, &mut found as *mut isize as isize);
        }
        (found != 0).then_some(found)
    }

    /// True when the window is minimized or hidden (tray-only mode or a
    /// browser-launched background instance).
    pub(super) fn needs_wake(hwnd: isize) -> bool {
        unsafe { IsIconic(hwnd) != 0 || IsWindowVisible(hwnd) == 0 }
    }

    /// True when the window is iconic (minimized to the taskbar).
    pub(super) fn is_iconic(hwnd: isize) -> bool {
        unsafe { IsIconic(hwnd) != 0 }
    }

    /// Ask winit/eframe for one frame without changing the real window size.
    pub(super) fn post_resize(hwnd: isize) {
        let mut placement = WindowPlacement {
            length: std::mem::size_of::<WindowPlacement>() as u32,
            ..Default::default()
        };
        let known = unsafe { GetWindowPlacement(hwnd, &mut placement) } != 0;
        // The restored size, so the temporary surface resize is a no-op.
        let (width, height) = if known {
            (
                (placement.rc_normal.right - placement.rc_normal.left).max(320),
                (placement.rc_normal.bottom - placement.rc_normal.top).max(240),
            )
        } else {
            (900, 600)
        };
        // Keep a previously maximized window maximized.
        let wparam = if known && placement.flags & WPF_RESTORE_TO_MAXIMIZED != 0 {
            SIZE_MAXIMIZED
        } else {
            SIZE_RESTORED
        };
        let lparam = (((height as u32) & 0xFFFF) << 16 | ((width as u32) & 0xFFFF)) as isize;
        unsafe {
            PostMessageW(hwnd, WM_SIZE, wparam, lparam);
        }
    }

    /// Force a repaint of a window by invalidating its client area.
    pub(super) fn invalidate(hwnd: isize) {
        unsafe {
            InvalidateRect(hwnd, 0, 0);
        }
    }

    /// Subclass any popup dialog windows so their title-bar X hides them
    /// immediately, without waiting for eframe's frame scheduling.
    pub(super) fn subclass_popups() {
        // no-op: subclassing via SetWindowSubclass requires comctl32 v6
        // which is not available; close latency is handled by Visible(false)
        // plus the parent-frame repaint loop instead.
    }

    extern "system" fn collect_cb(hwnd: isize, lparam: isize) -> i32 {
        unsafe {
            let mut buf = [0u16; 256];
            let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            if len > 0 {
                let title = String::from_utf16_lossy(&buf[..len as usize]);
                if title.starts_with("Download status")
                    || title.starts_with("Add new download")
                    || title.starts_with("Confirm Delete")
                    || title.starts_with("Site Grabber")
                    || title.starts_with("Scheduler")
                {
                    let targets = &mut *(lparam as *mut Vec<isize>);
                    targets.push(hwnd);
                }
            }
        }
        1
    }

    extern "system" fn subclass_proc(
        hwnd: isize,
        msg: u32,
        wparam: usize,
        lparam: isize,
        _subclass_id: usize,
        _ref_data: usize,
    ) -> isize {
        const WM_CLOSE: u32 = 0x0010;
        const SW_HIDE: i32 = 0;
        unsafe {
            if msg == WM_CLOSE {
                ShowWindow(hwnd, SW_HIDE);
            }
        }
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_pending_queue_name, manifest_firefox_json, manifest_json};

    #[test]
    fn generated_extension_manifests_use_the_application_version() {
        let version = env!("CARGO_PKG_VERSION");
        let chromium: serde_json::Value =
            serde_json::from_str(&manifest_json()).expect("valid Chromium manifest");
        let firefox: serde_json::Value =
            serde_json::from_str(&manifest_firefox_json()).expect("valid Firefox manifest");

        assert_eq!(chromium["version"], version);
        assert_eq!(firefox["version"], version);
    }

    #[test]
    fn numeric_queue_names_are_accepted() {
        assert!(is_pending_queue_name("1760000000000000000.json"));
        assert!(is_pending_queue_name("1.json"));
    }

    #[test]
    fn control_files_are_not_downloads() {
        assert!(!is_pending_queue_name("overlay.json"));
        assert!(!is_pending_queue_name("nm-trace.log"));
        assert!(!is_pending_queue_name(".json"));
        assert!(!is_pending_queue_name("1760000000000000000-overlay.json"));
    }
}
