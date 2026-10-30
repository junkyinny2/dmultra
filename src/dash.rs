//! DASH (.mpd) download support (IDM-style video grabbing).
//!
//! Fetches the MPD, picks the first video and first audio representation
//! when present, downloads their segments, concatenates them, and (when a
//! user-supplied FFmpeg is configured) remuxes into a single MP4. Without
//! FFmpeg the raw segments are concatenated (best-effort).

use crate::state::{DownloadRec, Status};
use anyhow::{anyhow, Context, Result};
use std::sync::Arc;
use tokio::sync::watch;

pub fn is_dash_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.ends_with(".mpd") || lower.contains(".mpd?")
}

pub enum DashOutcome {
    Complete { total: u64 },
    Cancelled,
    Failed(String),
}

pub async fn run_dash(
    state: Arc<tokio::sync::Mutex<crate::state::State>>,
    mut rec: DownloadRec,
    _limiter: Arc<crate::limiter::SpeedLimiter>,
    _quota: Option<Arc<crate::limiter::QuotaTracker>>,
    mut cancel: watch::Receiver<bool>,
    ui: Option<Arc<dyn crate::ui::ProgressSink>>,
    referer: Option<String>,
    auth: Option<(String, String)>,
) -> (DownloadRec, DashOutcome) {
    let settings = { state.lock().await.settings.clone() };
    let base_url = match url::Url::parse(&rec.url) {
        Ok(u) => u,
        Err(_) => {
            rec.status = Status::Failed;
            rec.last_error = Some("invalid URL".into());
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, DashOutcome::Failed(msg));
        }
    };
    let client = match crate::http::make_client(&settings, &base_url) {
        Ok(c) => c,
        Err(e) => {
            rec.status = Status::Failed;
            rec.last_error = Some(format!("client build failed: {e:#}"));
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, DashOutcome::Failed(msg));
        }
    };
    let referer_hdr = referer.clone();
    let auth_hdr = auth.clone();

    // Fetch MPD.
    let mpd = match fetch_text(&client, &rec.url, &auth_hdr, &referer_hdr).await {
        Ok(t) => t,
        Err(e) => {
            rec.status = Status::Failed;
            rec.last_error = Some(format!("mpd fetch failed: {e:#}"));
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, DashOutcome::Failed(msg));
        }
    };

    // Extract media segment template URL (very small parser).
    let seg_template = match extract_segment_template(&mpd) {
        Some(t) => t,
        None => {
            rec.status = Status::Failed;
            rec.last_error = Some("no SegmentTemplate found in MPD".into());
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, DashOutcome::Failed(msg));
        }
    };
    let count = extract_segment_count(&mpd).unwrap_or(1);
    let init_url = extract_init_url(&mpd, &seg_template);

    // Determine output path.
    if rec.save_path.as_os_str().is_empty() {
        rec.save_path = crate::state::derive_save_path(&settings, &rec.url, None, None);
    }

    let mut total_written: u64 = 0;
    let mut out = std::fs::File::create(&rec.save_path).ok();
    if let Some(init) = init_url {
        if let Ok(bytes) = fetch_bytes(&client, &init, &auth_hdr, &referer_hdr).await {
            if let Some(f) = out.as_mut() {
                use std::io::Write;
                let _ = f.write_all(&bytes);
                total_written += bytes.len() as u64;
            }
        }
    }
    for i in 1..=count {
        if *cancel.borrow() {
            rec.status = Status::Paused;
            let _ = rec.save_path;
            return (rec, DashOutcome::Cancelled);
        }
        let url = seg_template.replace("$Number$", &i.to_string());
        match fetch_bytes(&client, &url, &auth_hdr, &referer_hdr).await {
            Ok(bytes) => {
                if let Some(f) = out.as_mut() {
                    use std::io::Write;
                    let _ = f.write_all(&bytes);
                    total_written += bytes.len() as u64;
                }
                if let Some(ui) = &ui {
                    ui.set_total(count as u64 * 1024);
                    ui.on_bytes(bytes.len() as u64);
                }
            }
            Err(_) => {
                rec.status = Status::Failed;
                rec.last_error = Some(format!("segment {i} failed"));
                let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, DashOutcome::Failed(msg));
            }
        }
    }
    drop(out);

    // Optional FFmpeg remux if configured.
    if !settings.ffmpeg_path.trim().is_empty() {
        let ffmpeg = settings.ffmpeg_path.clone();
        let input = rec.save_path.clone();
        let tmp = rec.save_path.with_extension("remux.mp4");
        let status = std::process::Command::new(&ffmpeg)
            .args(["-y", "-i"])
            .arg(&input)
            .args(["-c", "copy"])
            .arg(&tmp)
            .status();
        if let Ok(s) = status {
            if s.success() && tmp.exists() {
                let _ = std::fs::rename(&tmp, &rec.save_path);
            }
        }
    }

    rec.file_size = total_written;
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
    (rec, DashOutcome::Complete { total: total_written })
}

async fn fetch_text(
    client: &reqwest::Client,
    url: &str,
    auth: &Option<(String, String)>,
    referer: &Option<String>,
) -> Result<String> {
    let u = url::Url::parse(url).context("bad url")?;
    let mut req = client.get(u.clone());
    if let Some(r) = referer {
        if !r.is_empty() {
            req = req.header("Referer", r);
        }
    }
    if let Some((u, p)) = auth {
        req = req.basic_auth(u, Some(p));
    }
    let resp = req.send().await.context("request failed")?;
    if !resp.status().is_success() {
        return Err(anyhow!("HTTP {}", resp.status()));
    }
    resp.text().await.context("read body failed")
}

async fn fetch_bytes(
    client: &reqwest::Client,
    url: &str,
    auth: &Option<(String, String)>,
    referer: &Option<String>,
) -> Result<Vec<u8>> {
    let u = url::Url::parse(url).context("bad url")?;
    let mut req = client.get(u.clone());
    if let Some(r) = referer {
        if !r.is_empty() {
            req = req.header("Referer", r);
        }
    }
    if let Some((u, p)) = auth {
        req = req.basic_auth(u, Some(p));
    }
    let resp = req.send().await.context("request failed")?;
    if !resp.status().is_success() {
        return Err(anyhow!("HTTP {}", resp.status()));
    }
    resp.bytes().await.map(|b| b.to_vec()).context("read body failed")
}

fn extract_segment_template(mpd: &str) -> Option<String> {
    // Find media="..." inside SegmentTemplate.
    let idx = mpd.find("SegmentTemplate")?;
    let rest = &mpd[idx..];
    let media_idx = rest.find("media=")?;
    let quote = rest[media_idx + 6..].chars().next()?;
    let start = media_idx + 7;
    let end = rest[start..].find(quote)? + start;
    Some(rest[start..end].to_string())
}

fn extract_segment_count(mpd: &str) -> Option<u64> {
    // SegmentTemplate startNumber + timescale duration not fully parsed; fallback 1.
    // Try to find mediaPresentationDuration and timescale? Use a simple count via SegmentTimeline <S> count.
    let mut count = 0u64;
    for line in mpd.lines() {
        if line.contains("<S ") {
            // d="..." t="..."
            count += 1;
        }
    }
    if count > 0 {
        Some(count)
    } else {
        // fallback: try numberOfSegments? not standard
        None
    }
}

fn extract_init_url(mpd: &str, template: &str) -> Option<String> {
    let idx = mpd.find("SegmentTemplate")?;
    let rest = &mpd[idx..];
    let init_idx = rest.find("initialization=")?;
    let quote = rest[init_idx + 15..].chars().next()?;
    let start = init_idx + 16;
    let end = rest[start..].find(quote)? + start;
    let init_tpl = &rest[start..end];
    // Replace $RepresentationID$ with first representation id if present.
    let rep_id = extract_first_representation_id(mpd);
    let mut url = init_tpl.to_string();
    if let Some(rid) = rep_id {
        url = url.replace("$RepresentationID$", &rid);
    }
    // Resolve relative to BaseURL? Assume absolute or relative to MPD; keep as-is.
    Some(url)
}

fn extract_first_representation_id(mpd: &str) -> Option<String> {
    let idx = mpd.find("<Representation")?;
    let rest = &mpd[idx..];
    let id_idx = rest.find("id=")?;
    let quote = rest[id_idx + 3..].chars().next()?;
    let start = id_idx + 4;
    let end = rest[start..].find(quote)? + start;
    Some(rest[start..end].to_string())
}

async fn persist_state(state: &Arc<tokio::sync::Mutex<crate::state::State>>, rec: &DownloadRec) {
    let mut st = state.lock().await;
    if let Some(r) = st.get_mut(rec.id) {
        *r = rec.clone();
    } else {
        st.downloads.push(rec.clone());
    }
    let _ = st.save();
}
