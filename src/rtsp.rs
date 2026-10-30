//! RTSP (rtsp://, rtsps://) recording support (IDM-style).
//!
//! Minimal implementation: interleaved RTP over RTSP (TCP). Connects,
//! DESCRIBEs, SETUPs the first media track, PLAYs, and writes RTP
//! payloads to the output file until cancelled or the stream ends.

use crate::state::{DownloadRec, Status};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

pub enum RtspOutcome {
    Complete { total: u64 },
    Cancelled,
    Failed(String),
}

pub fn is_rtsp_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("rtsp://") || lower.starts_with("rtsps://")
}

pub async fn run_rtsp(
    state: Arc<tokio::sync::Mutex<crate::state::State>>,
    mut rec: DownloadRec,
    _limiter: Arc<crate::limiter::SpeedLimiter>,
    _quota: Option<Arc<crate::limiter::QuotaTracker>>,
    mut cancel: watch::Receiver<bool>,
    ui: Option<Arc<dyn crate::ui::ProgressSink>>,
    referer: Option<String>,
    auth: Option<(String, String)>,
) -> (DownloadRec, RtspOutcome) {
    let settings = { state.lock().await.settings.clone() };
    let url = match url::Url::parse(&rec.url) {
        Ok(u) => u,
        Err(_) => {
            rec.status = Status::Failed;
            rec.last_error = Some("invalid URL".into());
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, RtspOutcome::Failed(msg));
        }
    };
    let host = url.host_str().unwrap_or("").to_string();
    let port = url.port().unwrap_or(554);
    let path = if url.path().is_empty() { "/" } else { url.path() };

    if rec.save_path.as_os_str().is_empty() {
        rec.save_path = crate::state::derive_save_path(&settings, &rec.url, None, None);
    }

    // Connect.
    let mut stream = match tokio::net::TcpStream::connect((host.as_str(), port)).await {
        Ok(s) => s,
        Err(e) => {
            rec.status = Status::Failed;
            rec.last_error = Some(format!("connect failed: {e}"));
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, RtspOutcome::Failed(msg));
        }
    };
    let _ = stream.set_nodelay(true);

    let mut cseq = 1u32;
    let session_url = format!("rtsp://{}:{}{}", host, port, path);

    // OPTIONS
    let _ = rtsp_request(&mut stream, "OPTIONS", &session_url, cseq, None, &auth, &referer).await;
    cseq += 1;

    // DESCRIBE
    let describe_resp = match rtsp_request(
        &mut stream,
        "DESCRIBE",
        &session_url,
        cseq,
        Some("application/sdp"),
        &auth,
        &referer,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            rec.status = Status::Failed;
            rec.last_error = Some(format!("describe failed: {e}"));
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, RtspOutcome::Failed(msg));
        }
    };
    cseq += 1;

    // SETUP
    let transport = "RTP/AVP/TCP;unicast;interleaved=0-1";
    let setup_resp = match rtsp_request_with_header(
        &mut stream,
        "SETUP",
        &session_url,
        cseq,
        None,
        &[("Transport", transport)],
        &auth,
        &referer,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            rec.status = Status::Failed;
            rec.last_error = Some(format!("setup failed: {e}"));
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, RtspOutcome::Failed(msg));
        }
    };
    cseq += 1;

    // Parse session id.
    let session_id = parse_session(&setup_resp).unwrap_or_default();
    let _ = describe_resp;

    // PLAY
    let play_headers: Vec<(&str, &str)> = if session_id.is_empty() {
        vec![]
    } else {
        vec![("Session", session_id.as_str())]
    };
    let _ = rtsp_request_with_header(
        &mut stream,
        "PLAY",
        &session_url,
        cseq,
        None,
        &play_headers,
        &auth,
        &referer,
    )
    .await;
    cseq += 1;

    // Read interleaved RTP.
    let mut out = match std::fs::File::create(&rec.save_path) {
        Ok(f) => f,
        Err(e) => {
            rec.status = Status::Failed;
            rec.last_error = Some(format!("file create failed: {e}"));
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, RtspOutcome::Failed(msg));
        }
    };
    use std::io::Write;
    let mut total: u64 = 0;
    let mut buf = [0u8; 4];
    loop {
        if *cancel.borrow() {
            break;
        }
        // read 4-byte interleaved header
        let mut hdr = [0u8; 4];
        let n = match stream.read(&mut hdr).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        if n < 4 {
            // partial; try to read rest
            let mut got = n;
            while got < 4 {
                match stream.read(&mut hdr[got..]).await {
                    Ok(0) => break,
                    Ok(k) => got += k,
                    Err(_) => break,
                }
            }
            if got < 4 {
                break;
            }
        }
        if hdr[0] != b'$' {
            // not interleaved; skip
            continue;
        }
        let len = u16::from_be_bytes([hdr[2], hdr[3]]) as usize;
        let mut pkt = vec![0u8; len];
        let mut got = 0usize;
        while got < len {
            match stream.read(&mut pkt[got..]).await {
                Ok(0) => break,
                Ok(k) => got += k,
                Err(_) => break,
            }
        }
        if got < len {
            break;
        }
        // RTP payload starts after 12-byte header (if RTP)
        if len >= 12 && (pkt[0] >> 6) == 2 {
            let payload_offset = 12usize;
            // account for CSRC
            let csrc_count = (pkt[0] & 0x0F) as usize;
            let ext = (pkt[0] & 0x10) != 0;
            let mut off = 12 + csrc_count * 4;
            if ext && off + 4 <= len {
                let ext_len = u16::from_be_bytes([pkt[off + 2], pkt[off + 3]]) as usize;
                off += 4 + ext_len * 4;
            }
            if off <= len {
                let payload = &pkt[off..];
                if !payload.is_empty() {
                    let _ = out.write_all(payload);
                    total += payload.len() as u64;
                    if let Some(ui) = &ui {
                        ui.on_bytes(payload.len() as u64);
                    }
                }
            }
        } else {
            let _ = out.write_all(&pkt);
            total += pkt.len() as u64;
        }
        let _ = buf;
    }

    // TEARDOWN
    let td_headers: Vec<(&str, &str)> = if session_id.is_empty() {
        vec![]
    } else {
        vec![("Session", session_id.as_str())]
    };
    let _ = rtsp_request_with_header(
        &mut stream,
        "TEARDOWN",
        &session_url,
        cseq,
        None,
        &td_headers,
        &auth,
        &referer,
    )
    .await;

    drop(out);
    if *cancel.borrow() {
        rec.status = Status::Paused;
        return (rec, RtspOutcome::Cancelled);
    }
    rec.file_size = total;
    rec.status = Status::Complete;
    rec.last_error = None;
    {
        let mut st = state.lock().await;
        if let Some(r) = st.get_mut(rec.id) {
            *r = rec.clone();
        } else {
            st.downloads.push(rec.clone());
        }
        let _ = st.save();
    }
    (rec, RtspOutcome::Complete { total })
}

async fn rtsp_request(
    stream: &mut tokio::net::TcpStream,
    method: &str,
    url: &str,
    cseq: u32,
    accept: Option<&str>,
    auth: &Option<(String, String)>,
    referer: &Option<String>,
) -> Result<String, String> {
    rtsp_request_with_header(stream, method, url, cseq, accept, &[], auth, referer).await
}

async fn rtsp_request_with_header(
    stream: &mut tokio::net::TcpStream,
    method: &str,
    url: &str,
    cseq: u32,
    accept: Option<&str>,
    extra_headers: &[(&str, &str)],
    auth: &Option<(String, String)>,
    referer: &Option<String>,
) -> Result<String, String> {
    let mut req = format!("{method} {url} RTSP/1.0\r\nCSeq: {cseq}\r\n");
    if let Some(a) = accept {
        req.push_str(&format!("Accept: {a}\r\n"));
    }
    for (k, v) in extra_headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(r) = referer {
        if !r.is_empty() {
            req.push_str(&format!("Referer: {r}\r\n"));
        }
    }
    if let Some((u, p)) = auth {
        // Basic auth.
        let cred = base64_encode(format!("{u}:{p}").as_bytes());
        req.push_str(&format!("Authorization: Basic {cred}\r\n"));
    }
    req.push_str("\r\n");
    stream
        .write_all(req.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    // Read response until blank line.
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let n = stream.read(&mut tmp).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(buf).map_err(|e| e.to_string())
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i + 2 < input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(TABLE[(n >> 18) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push(TABLE[(n & 63) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(TABLE[(n >> 18) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(TABLE[(n >> 18) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push('=');
    }
    out
}

fn parse_session(resp: &str) -> Option<String> {
    for line in resp.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("session:") {
            let val = line[line.find(':').unwrap() + 1..].trim();
            // strip timeout
            return Some(val.split(';').next().unwrap_or(val).trim().to_string());
        }
    }
    None
}
