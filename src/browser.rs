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
const EXT_VERSION: &str = "0.28.2";
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
        EXT_VERSION
    )
}

const CONTENT_JS: &str = include_str!("../browser-extension/content.js");

const BACKGROUND_JS: &str = include_str!("../browser-extension/background.js");

// ---------------------------------------------------------------------------
// Packaging: .xpi (Firefox) and .crx (Chrome) ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Â¦Ãƒâ€šÃ‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â minimal ZIP + CRX3 writers
// ---------------------------------------------------------------------------

/// Precomputed CRX3 header (public key + RSA signature over SignedData),
/// created with the project's dev signing key.
const CRX3_HEADER: &[u8] = &[0x12, 0xac, 0x04, 0x0a, 0xa6, 0x02, 0x30, 0x82, 0x01, 0x22, 0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00, 0x03, 0x82, 0x01, 0x0f, 0x00, 0x30, 0x82, 0x01, 0x0a, 0x02, 0x82, 0x01, 0x01, 0x00, 0xaf, 0x50, 0x5e, 0x14, 0x8f, 0x4f, 0x86, 0xc1, 0x8c, 0x4e, 0x94, 0xdf, 0xb7, 0x59, 0x0c, 0xc9, 0x98, 0xcd, 0xce, 0xb2, 0x98, 0x3c, 0xd0, 0x13, 0x7a, 0x7f, 0x83, 0xa1, 0x1e, 0xeb, 0x6f, 0x75, 0x06, 0x27, 0xc1, 0xd3, 0x8c, 0x68, 0x3c, 0xf7, 0x0e, 0xe1, 0xcd, 0x57, 0x56, 0x71, 0x65, 0xf2, 0x39, 0xa6, 0x25, 0x9a, 0x1d, 0x71, 0x7b, 0xb7, 0x19, 0xed, 0xe9, 0xbb, 0x5f, 0xc5, 0x23, 0xfa, 0xb1, 0x19, 0xa0, 0x71, 0xed, 0xe7, 0x41, 0x25, 0x07, 0xf8, 0x72, 0xd3, 0x9a, 0xfa, 0x0f, 0xca, 0xad, 0xe3, 0x43, 0x06, 0x76, 0xb9, 0x16, 0x20, 0xbf, 0x34, 0xd1, 0xe5, 0x64, 0x87, 0xc7, 0x7d, 0xdd, 0x47, 0x97, 0x28, 0x5c, 0x83, 0xa8, 0x7d, 0x83, 0xfa, 0x2e, 0xdc, 0x60, 0x36, 0x46, 0x22, 0xd8, 0x82, 0xcf, 0xc8, 0x3b, 0x83, 0x47, 0x57, 0xa3, 0x4e, 0x21, 0x72, 0x5c, 0xb5, 0x41, 0x6f, 0x13, 0xa1, 0xc3, 0xde, 0x17, 0x42, 0xe2, 0x45, 0x29, 0x15, 0x29, 0x8b, 0x17, 0x9a, 0xab, 0x34, 0x17, 0xcd, 0x7e, 0x4f, 0xe4, 0xe6, 0x63, 0x27, 0x6c, 0xd0, 0x29, 0xe7, 0xf0, 0x42, 0x55, 0x37, 0xd2, 0xcb, 0x6f, 0x79, 0x23, 0xa4, 0xc6, 0xa4, 0x96, 0x17, 0x78, 0x37, 0xdb, 0x55, 0xf5, 0x0e, 0xfd, 0x8f, 0xde, 0x7a, 0xfd, 0xcf, 0x99, 0x87, 0x1b, 0xe2, 0x5f, 0x07, 0xda, 0xea, 0x9c, 0x53, 0x5a, 0xb0, 0x76, 0x61, 0xc7, 0xb5, 0xdc, 0xe1, 0xf0, 0x86, 0x76, 0xd1, 0x12, 0x13, 0x28, 0x5c, 0xe5, 0xc5, 0xdf, 0x6d, 0xb8, 0xe9, 0xab, 0x0c, 0x32, 0x5b, 0xff, 0x48, 0x5e, 0x36, 0x1f, 0xcc, 0x29, 0x67, 0x76, 0xdd, 0xe9, 0x3e, 0xa7, 0xe0, 0x2d, 0x36, 0x8a, 0xe9, 0x69, 0xb9, 0xca, 0x6f, 0xc7, 0x8b, 0xac, 0xa0, 0xb5, 0xfa, 0xed, 0xff, 0x77, 0x81, 0x67, 0x11, 0x44, 0xd1, 0x30, 0x55, 0x02, 0x03, 0x01, 0x00, 0x01, 0x12, 0x80, 0x02, 0x03, 0x6e, 0xee, 0x1b, 0x34, 0xcc, 0xd6, 0x80, 0x45, 0xfe, 0xe2, 0x55, 0x3b, 0x26, 0xb2, 0xa3, 0x59, 0xf3, 0xe5, 0xf9, 0xa9, 0x72, 0xf1, 0x7d, 0xaa, 0xe6, 0xc0, 0xea, 0xed, 0xe8, 0x2f, 0x05, 0x52, 0xfa, 0x46, 0x32, 0xc2, 0xe4, 0x3c, 0xcf, 0xb4, 0x2a, 0xf6, 0xca, 0x7c, 0xd3, 0x59, 0x2b, 0x85, 0x69, 0xaa, 0x4b, 0x42, 0xcf, 0x5a, 0xc9, 0x1a, 0x55, 0xbc, 0x1b, 0x58, 0xf1, 0x34, 0x63, 0x4e, 0x58, 0x24, 0xac, 0x05, 0xcc, 0xcd, 0x8b, 0xd0, 0xbd, 0xe4, 0x9c, 0x2c, 0xdf, 0x8f, 0xbc, 0x74, 0xcc, 0x51, 0x32, 0x96, 0x8e, 0x8b, 0x62, 0xcd, 0xe4, 0x2e, 0x26, 0xff, 0x13, 0xf3, 0xb6, 0x45, 0x1c, 0x84, 0x82, 0x3c, 0xac, 0x79, 0xf6, 0xd8, 0x0c, 0x16, 0x7a, 0xa9, 0xa7, 0xe8, 0x98, 0x2f, 0x45, 0xbe, 0xb0, 0x29, 0x3d, 0x02, 0xda, 0xe3, 0x36, 0xe4, 0xc9, 0x70, 0x2d, 0x31, 0x1c, 0x3b, 0x16, 0x83, 0x95, 0x46, 0xe6, 0x4d, 0x36, 0xa8, 0x8a, 0xf9, 0xb1, 0x15, 0xd6, 0xc7, 0xe7, 0x88, 0x26, 0x31, 0xe1, 0xf5, 0xc4, 0x52, 0x2f, 0xe9, 0xec, 0x91, 0x7c, 0x48, 0xc2, 0x71, 0x95, 0x51, 0x21, 0x21, 0x03, 0x69, 0xcc, 0xdf, 0x9f, 0xfe, 0xe7, 0x7f, 0xf1, 0xe5, 0x82, 0x0b, 0xa8, 0xce, 0x0e, 0x8b, 0x20, 0xf9, 0xef, 0xd4, 0x2f, 0x69, 0x3d, 0x7f, 0xc3, 0x29, 0x4a, 0x3f, 0x2c, 0xb1, 0x2a, 0x86, 0xfb, 0x05, 0x91, 0xc6, 0xfd, 0x5a, 0x24, 0x82, 0x7d, 0x54, 0x19, 0x9a, 0xf8, 0x9d, 0xcc, 0xb9, 0x32, 0x41, 0x49, 0x3f, 0x18, 0x49, 0xb4, 0x98, 0xaa, 0xc5, 0xb5, 0x93, 0xea, 0x2d, 0x28, 0xfa, 0xab, 0xdf, 0xe4, 0x62, 0x6e, 0x22, 0x6c, 0xb1, 0xae, 0x31, 0xbf, 0x84, 0x44, 0xa0, 0xd5, 0x3a, 0x2b, 0x24, 0xdf, 0xda, 0xff, 0x3a, 0x32, 0xf2, 0x22, 0xac, 0x71, 0x4a, 0x29, 0x82, 0xf1, 0x04, 0x14, 0x10, 0x00, 0x00, 0x00, 0xf2, 0xdc, 0xa1, 0x1b, 0xe8, 0x09, 0xba, 0x45, 0x4f, 0xd0, 0x3f, 0x9a, 0x0c, 0x17, 0x49, 0x9a];

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
  "permissions": ["nativeMessaging", "contextMenus", "tabs", "webRequest", "<all_urls>"],
  "background": {{"scripts": ["background.js"]}},
  "content_scripts": [
    {{"matches": ["<all_urls>"], "js": ["content.js"], "run_at": "document_start", "all_frames": true}}
  ],
  "browser_specific_settings": {{
    "gecko": {{"id": "dmultra@dmultra.local", "strict_min_version": "115.0"}}
  }}
}}
"#,
        EXT_VERSION
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

/// Build the Chrome unpacked-install package (.zip) next to the extension folder.
fn build_zip(dir: &std::path::Path) -> Result<std::path::PathBuf> {
    let mut zip = ZipBuilder::new();
    zip.add("manifest.json", manifest_json().as_bytes());
    zip.add("content.js", CONTENT_JS.as_bytes());
    zip.add("background.js", BACKGROUND_JS.as_bytes());
    let path = dir.parent().unwrap_or(dir).join("dmultra-chrome.zip");
    std::fs::write(&path, zip.finish()).context("write .zip")?;
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
    let _ = build_zip(&dir)?;
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
///
/// The waker is **self-clocking**: it posts the next synthetic wake only after
/// `frame_seq` has advanced (the GUI finished the frame the previous wake
/// asked for), with a 2-second fallback. Posting at a fixed 100 ms rate while
/// a frame costs ~200 ms grows the Win32 queue without bound; every queued
/// input message - mouse move/click and even `WM_CLOSE` - is then starved for
/// minutes and the progress popout looks frozen. Keeping at most one wake in
/// flight bounds the queue while still driving frames at the paint rate.
#[cfg(windows)]
pub fn spawn_busy_waker(
    busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    watch_minimize: std::sync::Arc<std::sync::atomic::AtomicBool>,
    frame_seq: std::sync::Arc<std::sync::atomic::AtomicU64>,
) {
    use std::sync::atomic::Ordering;
    let _ = std::thread::Builder::new()
        .name("busy-waker".into())
        .spawn(move || {
            let mut last_wake = std::time::Instant::now();
            // Sequence value observed when the in-flight wake was posted.
            let mut awaiting_seq: Option<u64> = None;

            loop {
                std::thread::sleep(std::time::Duration::from_millis(100));

                let Some(hwnd) = wakeup::main_window() else {
                    continue;
                };
                let busy_now = busy.load(Ordering::Relaxed);
                let watch = watch_minimize.load(Ordering::Relaxed);
                let current_seq = frame_seq.load(Ordering::Relaxed);

                // Self-clocking: at most one synthetic wake is in flight. If
                // the previous frame has not finished yet we never post more
                // work, so the Win32 queue can never be flooded with resizes
                // that starve mouse input and `WM_CLOSE` (the "frozen popout"
                // bug). The 2 s fallback covers a window that genuinely does
                // not paint (eframe skips frames for iconic windows).
                if let Some(seq) = awaiting_seq {
                    if current_seq != seq || last_wake.elapsed() >= std::time::Duration::from_secs(2)
                    {
                        awaiting_seq = None;
                    }
                }

                // A visible, non-iconic main window schedules its own frames
                // (and paints the immediate child popouts). Poking it would
                // only add pointless work, so the waker stays silent.
                if !wakeup::needs_wake(hwnd) {
                    awaiting_seq = None;
                    continue;
                }

                let interval = if busy_now || watch {
                    std::time::Duration::from_millis(150)
                } else {
                    // Slow idle pulse so the tray click, clipboard monitor and
                    // a second-instance show request are still observed.
                    std::time::Duration::from_secs(1)
                };

                if awaiting_seq.is_none() && last_wake.elapsed() >= interval {
                    wakeup::post_resize(hwnd);
                    awaiting_seq = Some(current_seq);
                    last_wake = std::time::Instant::now();
                }
            }
        });
}

#[cfg(not(windows))]
pub fn spawn_busy_waker(
    _busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    _watch_minimize: std::sync::Arc<std::sync::atomic::AtomicBool>,
    _frame_seq: std::sync::Arc<std::sync::atomic::AtomicU64>,
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

/// Park this process' main window off-screen (tray "hidden" state).
///
/// The window is moved to `(-32000, -32000)` but stays `WS_VISIBLE` and
/// **non-iconic**. That is essential: eframe paints immediate child viewports
/// from the root frame, and it refuses to schedule frames for a minimized
/// window (`run.rs`). A visible off-screen root keeps the normal frame loop
/// running, so the progress popouts stay clickable, draggable and closable
/// even though the user cannot see the main window.
///
/// This variant also adds `WS_EX_TOOLWINDOW` so the window leaves the
/// taskbar (used by "tray only" and browser-background launches).
#[cfg(windows)]
pub fn hide_main_window() {
    wakeup::park(true);
}

#[cfg(not(windows))]
pub fn hide_main_window() {}

/// Park the main window while keeping its taskbar button (minimize to
/// "taskbar and tray"). Clicking the taskbar button or the tray icon restores
/// it.
#[cfg(windows)]
pub fn park_main_window_keeping_taskbar() {
    wakeup::park(false);
}

#[cfg(not(windows))]
pub fn park_main_window_keeping_taskbar() {}

/// If the main window is genuinely iconic while a popout is open, restore it
/// and park it off-screen. eframe cannot paint (and therefore cannot service)
/// the popout for an iconic root.
#[cfg(windows)]
pub fn wake_main_for_popouts() {
    wakeup::wake_for_popouts();
}

#[cfg(not(windows))]
pub fn wake_main_for_popouts() {}

/// Restore the main window after [`hide_main_window`] / a parked minimize.
#[cfg(windows)]
pub fn show_main_window() {
    wakeup::unpark();
}

#[cfg(not(windows))]
pub fn show_main_window() {}

/// True when the main window is deliberately parked off-screen.
#[cfg(windows)]
pub fn main_window_parked() -> bool {
    wakeup::is_parked()
}

#[cfg(not(windows))]
pub fn main_window_parked() -> bool {
    false
}

/// Install the Win32 hook on the main window (once). Detects the minimize
/// button so the root is parked instead of becoming iconic.
#[cfg(windows)]
pub fn install_main_hook() {
    wakeup::install_main_hook();
}

#[cfg(not(windows))]
pub fn install_main_hook() {}

/// Tell the hook what the configured minimize mode is:
/// `"taskbar"`, `"taskbar_tray"` or `"tray"`.
#[cfg(windows)]
pub fn set_main_window_mode(mode: &str) {
    wakeup::set_main_mode(mode);
}

#[cfg(not(windows))]
pub fn set_main_window_mode(_mode: &str) {}

/// Tell the hook whether a child (progress/dialog) window is currently open.
#[cfg(windows)]
pub fn set_popout_open(open: bool) {
    wakeup::set_popout_open(open);
}

#[cfg(not(windows))]
pub fn set_popout_open(_open: bool) {}

/// Hook and un-own every popout window that currently exists.
///
/// Matches download progress windows by their `Download status - ` title
/// prefix (the filename can change between frames) and the known dialogs by
/// exact title. Returns how many matching windows were found so the caller
/// can retry while one is still being created.
#[cfg(windows)]
pub fn refresh_popup_hooks() -> usize {
    wakeup::refresh_popup_hooks()
}

#[cfg(not(windows))]
pub fn refresh_popup_hooks() -> usize {
    0
}

#[cfg(windows)]
mod wakeup {
    use std::collections::HashMap;
    use std::ffi::c_int;
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU8, Ordering};
    use std::sync::{Mutex, OnceLock};

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
    const WM_ACTIVATE: u32 = 0x0006;
    const WM_SYSCOMMAND: u32 = 0x0112;
    const SIZE_RESTORED: usize = 0;
    const SIZE_MAXIMIZED: usize = 2;
    /// `WPF_RESTORETOMAXIMIZED`: the window was maximized before minimizing.
    const WPF_RESTORE_TO_MAXIMIZED: u32 = 0x0002;

    const SC_MINIMIZE: usize = 0xF020;
    const SC_RESTORE: usize = 0xF120;

    const SW_HIDE: i32 = 0;
    const SW_SHOW: i32 = 5;
    const SW_RESTORE: i32 = 9;

    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;

    /// Where the window currently is, so "show" puts it back exactly there.
    static PARK_RECT: Mutex<Option<(i32, i32)>> = Mutex::new(None);
    /// The main window is currently parked off-screen.
    static PARKED: AtomicBool = AtomicBool::new(false);
    /// We added `WS_EX_TOOLWINDOW` ourselves and must remove it on restore.
    static PARK_TOOL_ADDED: AtomicBool = AtomicBool::new(false);
    /// `0` = taskbar, `1` = taskbar+tray, `2` = tray only.
    static MAIN_MODE: AtomicU8 = AtomicU8::new(1);
    /// A progress/dialog child window is open right now.
    static POPOUT_OPEN: AtomicBool = AtomicBool::new(false);
    /// Original window procedure of the main window (0 until installed).
    static MAIN_PREV: AtomicIsize = AtomicIsize::new(0);

    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(cb: extern "system" fn(isize, isize) -> i32, lparam: isize) -> i32;
        fn GetWindowTextW(hwnd: isize, buf: *mut u16, max: i32) -> i32;
        fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
        pub(super) fn IsIconic(hwnd: isize) -> c_int;
        pub(super) fn IsWindowVisible(hwnd: isize) -> c_int;
        pub(super) fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        fn SetForegroundWindow(hwnd: isize) -> c_int;
        pub(super) fn SetWindowLongPtrW(hwnd: isize, index: i32, new_long: isize) -> isize;
        fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
        pub(super) fn CallWindowProcW(prev: isize, hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
        fn GetWindowPlacement(hwnd: isize, placement: *mut WindowPlacement) -> c_int;
        fn DefWindowProcW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
        fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> c_int;
        fn IsWindow(hwnd: isize) -> c_int;
        fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> c_int;
        fn SetWindowPos(
            hwnd: isize,
            insert_after: isize,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> c_int;
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
    ///
    /// The handle is cached: the busy waker asks for it several times per
    /// second, and a full `EnumWindows` + `GetWindowTextW` sweep is both
    /// expensive and can block on foreign driver/IME windows. The cache is
    /// validated with `IsWindow` and the owning process id.
    pub(super) fn main_window() -> Option<isize> {
        static CACHED: AtomicIsize = AtomicIsize::new(0);
        let cached = CACHED.load(Ordering::Relaxed);
        if cached != 0 && is_our_window(cached) {
            return Some(cached);
        }
        let mut found: isize = 0;
        unsafe {
            EnumWindows(find_main, &mut found as *mut isize as isize);
        }
        if found != 0 {
            CACHED.store(found, Ordering::Relaxed);
            Some(found)
        } else {
            None
        }
    }

    /// True when `hwnd` still belongs to this process.
    fn is_our_window(hwnd: isize) -> bool {
        if unsafe { IsWindow(hwnd) } == 0 {
            return false;
        }
        let mut pid: u32 = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        pid == std::process::id()
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

    /// Park the main window off-screen while keeping it `WS_VISIBLE` and
    /// non-iconic, so eframe keeps scheduling frames for the child popouts.
    ///
    /// `no_taskbar` adds `WS_EX_TOOLWINDOW` (tray-only / background launch).
    pub(super) fn park(no_taskbar: bool) {
        let Some(hwnd) = main_window() else {
            return;
        };
        let already = PARKED.load(Ordering::Relaxed);
        let iconic = unsafe { IsIconic(hwnd) != 0 };
        let has_tool = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) }
            & (WS_EX_TOOLWINDOW as isize)
            != 0;

        // Idempotent fast path: already parked exactly the way we want.
        if already
            && !iconic
            && has_tool == no_taskbar
            && unsafe { IsWindowVisible(hwnd) } != 0
            && PARK_RECT.lock().map(|slot| slot.is_some()).unwrap_or(false)
        {
            return;
        }

        // Restore first: an iconic window cannot be moved reliably.
        if iconic {
            unsafe {
                ShowWindow(hwnd, SW_RESTORE);
            }
        }

        if !already {
            let mut rect = Rect::default();
            if unsafe { GetWindowRect(hwnd, &mut rect) } != 0 && rect.left > -10_000 {
                if let Ok(mut slot) = PARK_RECT.lock() {
                    *slot = Some((rect.left, rect.top));
                }
            }
        }

        if no_taskbar && !has_tool {
            unsafe {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style(hwnd) | WS_EX_TOOLWINDOW as isize);
            }
            PARK_TOOL_ADDED.store(true, Ordering::Relaxed);
        } else if !no_taskbar && PARK_TOOL_ADDED.load(Ordering::Relaxed) {
            unsafe {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style(hwnd) & !(WS_EX_TOOLWINDOW as isize));
            }
            PARK_TOOL_ADDED.store(false, Ordering::Relaxed);
        }

        // A style change only reaches the taskbar after a show/hide cycle.
        // Both happen at the parked position, so nothing is ever visible.
        if no_taskbar != has_tool && !already {
            unsafe {
                ShowWindow(hwnd, SW_HIDE);
                ShowWindow(hwnd, SW_SHOW);
            }
        }

        unsafe {
            SetWindowPos(hwnd, 0, -32000, -32000, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        PARKED.store(true, Ordering::Relaxed);
        if !already {
            debug_log(&format!("main window parked (toolwindow={no_taskbar})"));
        }
    }

    /// Bring a parked (or genuinely minimized) main window back on screen.
    pub(super) fn unpark() {
        let Some(hwnd) = main_window() else {
            return;
        };
        let was_parked = PARKED.swap(false, Ordering::Relaxed);

        if PARK_TOOL_ADDED.swap(false, Ordering::Relaxed) {
            unsafe {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style(hwnd) & !(WS_EX_TOOLWINDOW as isize));
                ShowWindow(hwnd, SW_HIDE);
                ShowWindow(hwnd, SW_SHOW);
            }
        }
        if unsafe { IsIconic(hwnd) } != 0 {
            unsafe {
                ShowWindow(hwnd, SW_RESTORE);
            }
        }
        if was_parked {
            let saved = PARK_RECT.lock().ok().and_then(|slot| *slot);
            if let Some((x, y)) = saved {
                unsafe {
                    SetWindowPos(hwnd, 0, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                }
            }
            if let Ok(mut slot) = PARK_RECT.lock() {
                *slot = None;
            }
        }
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                ShowWindow(hwnd, SW_SHOW);
            }
            SetForegroundWindow(hwnd);
        }
        if was_parked {
            debug_log("main window restored from the park position");
        }
    }

    pub(super) fn is_parked() -> bool {
        PARKED.load(Ordering::Relaxed)
    }

    /// Restore+park a genuinely iconic main window (a popout was opened
    /// while it was minimized to the taskbar).
    pub(super) fn wake_for_popouts() {
        let Some(hwnd) = main_window() else {
            return;
        };
        if unsafe { IsIconic(hwnd) } == 0 {
            return;
        }
        debug_log("popout opened while the main window was iconic; parking it");
        park(false);
    }

    fn ex_style(hwnd: isize) -> isize {
        unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) }
    }

    /// Install the main-window hook once, then report state to it.
    pub(super) fn install_main_hook() {
        if MAIN_PREV.load(Ordering::Relaxed) != 0 {
            return;
        }
        let Some(hwnd) = main_window() else {
            return;
        };
        let prev =
            unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, main_subclass_proc as *const () as isize) };
        if prev != 0 {
            MAIN_PREV.store(prev, Ordering::Relaxed);
            debug_log("main window subclass installed");
        }
    }

    pub(super) fn set_main_mode(mode: &str) {
        let value = match mode {
            "taskbar" => 0,
            "tray" => 2,
            _ => 1,
        };
        MAIN_MODE.store(value, Ordering::Relaxed);
    }

    pub(super) fn set_popout_open(open: bool) {
        POPOUT_OPEN.store(open, Ordering::Relaxed);
    }

    /// The minimize button is intercepted before winit/eframe can make the
    /// root iconic (eframe does not schedule repaints for iconic windows, and
    /// immediate popouts are painted by the root frame).
    extern "system" fn main_subclass_proc(
        hwnd: isize,
        msg: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        let prev = MAIN_PREV.load(Ordering::Relaxed);
        if prev == 0 {
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }

        if msg == WM_SYSCOMMAND {
            let cmd = wparam & 0xFFF0;
            if cmd == SC_MINIMIZE {
                let mode = MAIN_MODE.load(Ordering::Relaxed);
                let child = POPOUT_OPEN.load(Ordering::Relaxed);
                // Tray modes always park. Plain taskbar mode parks only while
                // a popout is open, because the popout needs the root frame.
                if mode != 0 || child {
                    debug_log("minimize intercepted; parking the main window");
                    park(mode == 2);
                    return 0;
                }
            } else if cmd == SC_RESTORE && PARKED.load(Ordering::Relaxed) {
                unpark();
                return 0;
            }
        } else if msg == WM_ACTIVATE
            && PARKED.load(Ordering::Relaxed)
            && (wparam & 0xFFFF) != 0
        {
            // Taskbar button click, Alt+Tab, tray restore.
            unpark();
            return 0;
        }

        unsafe { CallWindowProcW(prev, hwnd, msg, wparam, lparam) }
    }

    fn debug_log(msg: &str) {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        if *ENABLED.get_or_init(|| std::env::var_os("DMULTRA_DEBUG").is_some()) {
            crate::logfile::log("WIN", msg);
        }
    }

    /// Subclass every popout window that currently exists so its title-bar X
    /// (and Alt+F4) hides it immediately, without waiting for eframe's frame
    /// scheduling, and un-own the download progress windows so they cannot be
    /// minimized together with the main window.
    ///
    /// Download windows are matched by title *prefix* (`Download status - `),
    /// not by exact title: the filename in the title can change (e.g.
    /// `blob (1).bin`) and a recreated window must be hooked again. The known
    /// dialogs are matched by exact title.
    ///
    /// Returns how many matching windows were found, so the caller can retry
    /// while one is still being created. The caller throttles this to a few
    /// times per second - enumerating windows every frame is far too costly.
    pub(super) fn refresh_popup_hooks() -> usize {
        struct HookScan {
            found: usize,
        }

        extern "system" fn hook_scan(hwnd: isize, lparam: isize) -> i32 {
            let scan = unsafe { &mut *(lparam as *mut HookScan) };
            let mut pid: u32 = 0;
            unsafe {
                GetWindowThreadProcessId(hwnd, &mut pid);
            }
            if pid != std::process::id() {
                return 1;
            }
            let mut buf = [0u16; 512];
            let len = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
            if len <= 0 {
                return 1;
            }
            let title = String::from_utf16_lossy(&buf[..len as usize]);
            let is_download = title.starts_with("Download status - ");
            let is_dialog = matches!(
                title.as_str(),
                "Options" | "Add new download..." | "Confirm Delete" | "Scheduler" | "Site Grabber"
            );
            if is_download {
                scan.found += 1;
                hook_window(hwnd);
                make_unowned(hwnd);
            } else if is_dialog {
                scan.found += 1;
                hook_window(hwnd);
            }
            1
        }

        let mut scan = HookScan { found: 0 };
        unsafe {
            EnumWindows(hook_scan, &mut scan as *mut HookScan as isize);
        }
        scan.found
    }

    /// Install the instant-close subclass on one window (once).
    ///
    /// A handle that was destroyed and re-created must be hooked again, so
    /// stale entries are pruned instead of trusted forever.
    fn hook_window(hwnd: isize) {
        if let Ok(mut map) = hooked().lock() {
            if let Some(previous) = map.get(&hwnd).copied() {
                if previous != 0 && unsafe { IsWindow(hwnd) } != 0 {
                    return;
                }
                map.remove(&hwnd);
            }
        } else {
            return;
        }
        let previous =
            unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, subclass_proc as *const () as isize) };
        if previous != 0 {
            if let Ok(mut map) = hooked().lock() {
                map.insert(hwnd, previous);
            }
            debug_log("popout hook installed");
        }
    }

    /// `GWLP_WNDPROC`: index of the window-procedure pointer.
    const GWLP_WNDPROC: i32 = -4;
const GWL_HWNDPARENT: i32 = -8;
const GWL_EXSTYLE: i32 = -20;
const WS_EX_TOOLWINDOW: i32 = 0x00000080;

    /// Make a window unowned (independent) so it doesn't get minimized
    /// when its owner window is minimized. Idempotent: the periodic refresh
    /// calls this several times per second.
    pub(super) fn make_unowned(hwnd: isize) {
        unsafe {
            let owner = GetWindowLongPtrW(hwnd, GWL_HWNDPARENT);
            let exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let has_tool = exstyle & WS_EX_TOOLWINDOW as isize != 0;
            if owner != 0 {
                SetWindowLongPtrW(hwnd, GWL_HWNDPARENT, 0);
            }
            if !has_tool {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, exstyle | WS_EX_TOOLWINDOW as isize);
            }
            // If the window is currently minimized, restore it
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
        }
    }

    /// Original window procedures, keyed by window.
    fn hooked() -> &'static Mutex<HashMap<isize, isize>> {
        static HOOKED: OnceLock<Mutex<HashMap<isize, isize>>> = OnceLock::new();
        HOOKED.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// Debug: how many more messages to trace for a just-closed window.

    extern "system" fn subclass_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
        const WM_CLOSE: u32 = 0x0010;
        const WM_NCDESTROY: u32 = 0x0082;
        const SW_HIDE: i32 = 0;

        let previous = hooked()
            .lock()
            .ok()
            .and_then(|map| map.get(&hwnd).copied());
        let Some(previous) = previous else {
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        };

        if msg == WM_CLOSE {
            // Hide first: this is the whole point (instant close). The
            // message is still forwarded so the close is fully processed.
            debug_log("popout WM_CLOSE -> hidden");
            unsafe {
                ShowWindow(hwnd, SW_HIDE);
            }
        }

        let result = unsafe { CallWindowProcW(previous, hwnd, msg, wparam, lparam) };

        if msg == WM_NCDESTROY {
            if let Ok(mut map) = hooked().lock() {
                map.remove(&hwnd);
            }
        }
        result
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
