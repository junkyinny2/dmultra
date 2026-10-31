//! `dmultra-nmh` — DM Ultra's native messaging host.
//!
//! A tiny console-subsystem binary spawned by the browser (Chrome/Edge)
//! when a download link is intercepted by the extension. It reads one
//! length-prefixed JSON message, drops the URL into the pending queue
//! (a running DM Ultra GUI picks it up within ~0.4 s and opens the
//! Add-URL dialog), replies, and exits when the pipe closes.
//!
//! Deliberately dependency-light: the browser runs hosts inside an
//! AppContainer sandbox, so this binary only writes to the pending
//! queue, which carries an ALL APPLICATION PACKAGES grant.

use std::io::{Read, Write};

fn pending_dir() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("dmultra")
        .join("pending")
}

fn trace(msg: &str) {
    let line = format!("{msg}\r\n");
    let _ = std::fs::write(pending_dir().join("nm-trace.log"), line);
}

fn unix_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
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

/// Start DM Ultra if it is not already running. A browser click should start
/// the program and then the download, not just leave a file in the queue.
fn ensure_gui_running() {
    if gui_running() {
        trace("nmh: gui already running");
        return;
    }
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let exe = dir.join("dmultra.exe");
    if !exe.exists() {
        trace(&format!("nmh: gui not found at {}", exe.display()));
        return;
    }
    #[cfg(windows)]
    let spawn = {
        use std::os::windows::process::CommandExt;
        std::process::Command::new(&exe)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // CREATE_NO_WINDOW: never flash a console next to the browser.
            .creation_flags(0x0800_0000)
            .spawn()
    };
    #[cfg(not(windows))]
    let spawn = std::process::Command::new(&exe)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawn {
        Ok(_) => trace("nmh: launched gui"),
        Err(e) => trace(&format!("nmh: gui launch failed: {e}")),
    }
}

fn main() {
    trace("nmh: started");
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    let mut stdout = std::io::stdout();
    loop {
        let mut len_buf = [0u8; 4];
        if stdin.read_exact(&mut len_buf).is_err() {
            break; // pipe closed -> exit quietly
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 || len > 1_000_000 {
            break;
        }
        let mut buf = vec![0u8; len];
        if stdin.read_exact(&mut buf).is_err() {
            break;
        }
        let msg: serde_json::Value = serde_json::from_slice(&buf).unwrap_or_default();

        // Video overlay messages: write to a dedicated overlay file that the
        // GUI reads to create the Win32 floating button. The resolver only
        // treats numeric queue files as downloads, so it can never consume
        // this control file as a download request.
        let action = msg.get("action").and_then(|v| v.as_str()).unwrap_or("");
        if action == "videoOverlay" || action == "videoOverlayHide" {
            let dir = pending_dir();
            let _ = std::fs::create_dir_all(&dir);
            let target = dir.join("overlay.json");
            let tmp = dir.join("overlay.tmp");
            let body = serde_json::to_string(&msg).unwrap_or_default();
            trace(&format!("nmh: writing overlay data ({action})"));
            // Write-then-rename: the GUI must never read a half-written
            // payload (a truncated 12 KB read used to mean no update at all,
            // leaving a ghost button on screen).
            let _ = std::fs::write(&tmp, body.as_bytes());
            match std::fs::rename(&tmp, &target) {
                Ok(()) => {}
                Err(_) => {
                    // Fallback for filesystems that refuse the replace.
                    match std::fs::write(&target, body.as_bytes()) {
                        Ok(()) => trace("nmh: overlay written"),
                        Err(e) => trace(&format!("nmh: overlay write failed: {e}")),
                    }
                }
            }
            let reply = br#"{"ok":true}"#;
            let _ = stdout.write_all(&(reply.len() as u32).to_le_bytes());
            let _ = stdout.write_all(reply);
            let _ = stdout.flush();
            continue;
        }

        // Batch message: {"urls": [...]}
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
            if url.is_empty() { Vec::new() } else { vec![url] }
        };
        let referer = msg
            .get("referer")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        trace(&format!("nmh: got {} url(s)", urls.len()));
        if !urls.is_empty() {
            let dir = pending_dir();
            if let Err(e) = std::fs::create_dir_all(&dir) {
                trace(&format!("nmh: create pending dir failed: {e}"));
            }
            let payload = if urls.len() == 1 {
                serde_json::json!({"url": urls[0], "referer": referer}).to_string()
            } else {
                serde_json::json!({"urls": urls, "referer": referer}).to_string()
            };
            match std::fs::write(dir.join(format!("{}.json", unix_nanos())), payload) {
                Ok(()) => {
                    trace("nmh: pending written");
                    ensure_gui_running();
                }
                Err(e) => trace(&format!("nmh: pending write failed: {e}")),
            }
        }
        let reply = br#"{"ok":true}"#;
        let _ = stdout.write_all(&(reply.len() as u32).to_le_bytes());
        let _ = stdout.write_all(reply);
        let _ = stdout.flush();
        trace("nmh: replied");
    }
}
