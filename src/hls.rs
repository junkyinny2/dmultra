//! HLS (m3u8) video downloads (IDM-style video grabbing):
//! fetch the playlist, pick the highest-bandwidth variant when it's a
//! master playlist, download all media segments (4 at a time), and
//! concatenate them into one file. VOD playlists only.

use crate::state::DownloadRec;
use crate::state::Status;
use anyhow::{anyhow, Context, Result};
use std::sync::atomic::Ordering;
#[allow(unused_imports)]
use std::sync::atomic::Ordering as _Ordering;
use std::sync::Arc;
use tokio::sync::watch;

/// Outcome of an HLS download, mirroring the segmented engine's.
pub enum HlsOutcome {
    Complete { total: u64 },
    Cancelled,
    Failed(String),
}

/// Run an HLS download for `rec` (URL must point at an .m3u8 playlist).
/// The assembled file is written to `rec.save_path`; segments are
/// fetched concurrently and appended in order.
pub async fn run_hls(
    state: Arc<tokio::sync::Mutex<crate::state::State>>,
    mut rec: DownloadRec,
    limiter: Arc<crate::limiter::SpeedLimiter>,
    quota: Option<Arc<crate::limiter::QuotaTracker>>,
    mut cancel: watch::Receiver<bool>,
    ui: Option<Arc<dyn crate::ui::ProgressSink>>,
    referer: Option<String>,
    auth: Option<(String, String)>,
) -> (DownloadRec, HlsOutcome) {
    let settings = { state.lock().await.settings.clone() };
    let base_url = match url::Url::parse(&rec.url) {
        Ok(u) => u,
        Err(_) => {
            rec.status = Status::Failed;
            rec.last_error = Some("invalid URL".into());
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, HlsOutcome::Failed(msg));
        }
    };
    let client = match crate::http::make_client(&settings, &base_url) {
        Ok(c) => c,
        Err(e) => {
            rec.status = Status::Failed;
            rec.last_error = Some(format!("client build failed: {e:#}"));
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, HlsOutcome::Failed(msg));
        }
    };
    let auth_hdr = auth.clone();
    let referer_hdr = referer.clone();

    // 1. Fetch the playlist.
    let playlist_text = match fetch_text(&client, &rec.url, &auth_hdr, &referer_hdr).await {
        Ok(t) => t,
        Err(e) => {
            rec.status = Status::Failed;
            rec.last_error = Some(format!("playlist fetch failed: {e:#}"));
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, HlsOutcome::Failed(msg));
        }
    };

    // 2. Master playlist -> pick the highest-bandwidth variant.
    let (playlist_url, playlist_text) =
        match pick_variant(&client, &rec.url, &playlist_text, &auth_hdr, &referer_hdr).await {
            Ok(v) => v,
            Err(e) => {
                rec.status = Status::Failed;
                rec.last_error = Some(format!("{e:#}"));
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, HlsOutcome::Failed(msg));
            }
        };

    // 3. Parse segment URLs.
    let segments = parse_segments(&playlist_url, &playlist_text);
    if segments.is_empty() {
        rec.status = Status::Failed;
        rec.last_error =
            Some("no media segments found in playlist (live streams are not supported)".into());
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, HlsOutcome::Failed(msg));
    }

    // 4. Download segments concurrently into memory-part files, in order.
    let out_path = rec.save_path.clone();
    if let Some(parent) = out_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let part_dir = settings.temp_path.join(format!("d{}_hls", rec.id));
    let _ = std::fs::create_dir_all(&part_dir);

    let mut total: u64 = 0;
    let mut _done_seg: usize = 0;
    let seg_count = segments.len();

    // Download in waves of 4; append each wave in order.
    const WAVE: usize = 4;
    let mut idx = 0usize;
    while idx < segments.len() {
        if cancel.borrow().to_owned() {
            rec.status = Status::Paused;
            rec.last_error = None;
            let _ = std::fs::remove_dir_all(&part_dir);
            return (rec, HlsOutcome::Cancelled);
        }
        if let Some(q) = &quota {
            if q.tripped() {
                rec.status = Status::Paused;
                rec.last_error = Some("Download limits exceeded!".into());
                let _ = std::fs::remove_dir_all(&part_dir);
                return (rec, HlsOutcome::Cancelled);
            }
        }
        let wave: Vec<String> = segments[idx..(idx + WAVE).min(segments.len())].to_vec();
        let mut joins = Vec::new();
        for (k, seg) in wave.iter().enumerate() {
            let client = client.clone();
            let seg = seg.clone();
            let limiter = limiter.clone();
            let part = part_dir.join(format!("seg_{:06}.part", idx + k));
            let a = auth_hdr.clone();
            let r = referer_hdr.clone();
            joins.push(tokio::spawn(async move {
                fetch_segment(&client, &seg, &part, &limiter, &a, &r).await
            }));
        }
        for j in joins {
            match j.await {
                Ok(Ok(sz)) => total += sz,
                Ok(Err(e)) => {
                    let _ = std::fs::remove_dir_all(&part_dir);
                    rec.status = Status::Failed;
                    rec.last_error = Some(format!("segment download failed: {e:#}"));
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, HlsOutcome::Failed(msg));
                }
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&part_dir);
                    rec.status = Status::Failed;
                    rec.last_error = Some(format!("segment worker panicked: {e}"));
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, HlsOutcome::Failed(msg));
                }
            }
            _done_seg += 1;
            if let Some(u) = &ui {
                // Progress reporting: one chunk covering everything.
                u.on_bytes(total);
            }
        }
        idx += WAVE;
    }

    // 5. Concatenate all parts into the output file.
    let mut out = match tokio::fs::File::create(&out_path).await {
        Ok(f) => f,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&part_dir);
            rec.status = Status::Failed;
            rec.last_error = Some(format!("cannot create output: {e}"));
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, HlsOutcome::Failed(msg));
        }
    };
    use tokio::io::AsyncWriteExt;
    for i in 0..seg_count {
        let part = part_dir.join(format!("seg_{i:06}.part"));
        match tokio::fs::read(&part).await {
            Ok(bytes) => {
                if out.write_all(&bytes).await.is_err() {
                    let _ = std::fs::remove_dir_all(&part_dir);
                    rec.status = Status::Failed;
                    rec.last_error = Some("write failed during assembly".into());
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, HlsOutcome::Failed(msg));
                }
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&part_dir);
                rec.status = Status::Failed;
                rec.last_error = Some(format!("missing segment part: {e}"));
            let msg = rec.last_error.clone().unwrap_or_default();
            return (rec, HlsOutcome::Failed(msg));
            }
        }
    }
    let _ = out.flush().await;
    let _ = out.sync_all().await;
    let _ = std::fs::remove_dir_all(&part_dir);

    rec.file_size = total;
    rec.status = Status::Complete;
    rec.last_error = None;
    (rec, HlsOutcome::Complete { total })
}

async fn fetch_text(
    client: &reqwest::Client,
    url: &str,
    auth: &Option<(String, String)>,
    referer: &Option<String>,
) -> Result<String> {
    let mut req = client
        .get(url)
        .header("Accept", "*/*")
        .header("Accept-Encoding", "identity");
    if let Some(r) = referer.as_deref().filter(|s| !s.is_empty()) {
        req = req.header("Referer", r);
    }
    if let Some((u, p)) = auth {
        req = req.basic_auth(u, Some(p));
    }
    let resp = req.send().await.context("request failed")?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {}", resp.status().as_u16());
    }
    Ok(resp.text().await.context("read body failed")?)
}

/// If `text` is a master playlist (contains #EXT-X-STREAM-INF), fetch
/// the highest-bandwidth variant and return its URL + text.
#[allow(clippy::too_many_arguments)]
async fn pick_variant(
    client: &reqwest::Client,
    base_url: &str,
    text: &str,
    auth: &Option<(String, String)>,
    referer: &Option<String>,
) -> Result<(String, String)> {
    if !text.contains("#EXT-X-STREAM-INF") {
        return Ok((base_url.to_string(), text.to_string()));
    }
    let base = url::Url::parse(base_url).context("bad playlist URL")?;
    let mut best: Option<(u64, String)> = None;
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if line.starts_with("#EXT-X-STREAM-INF") {
            let bw = line
                .split(',')
                .find_map(|p| {
                    p.trim()
                        .strip_prefix("BANDWIDTH=")
                        .and_then(|v| v.parse::<u64>().ok())
                })
                .unwrap_or(0);
            if let Some(next) = lines[i + 1..].iter().find(|l| !l.trim().is_empty() && !l.trim().starts_with('#')) {
                let next_url = next.trim();
                if let Ok(u) = base.join(next_url) {
                    if best.as_ref().map(|b| bw > b.0).unwrap_or(true) {
                        best = Some((bw, u.to_string()));
                    }
                }
            }
        }
        i += 1;
    }
    let (_, variant_url) = best.ok_or_else(|| anyhow!("master playlist had no variants"))?;
    let text = fetch_text(client, &variant_url, auth, referer).await?;
    Ok((variant_url, text))
}

/// Parse media segment URLs (lines that are not tags), resolved against
/// the playlist URL.
fn parse_segments(playlist_url: &str, text: &str) -> Vec<String> {
    let base = match url::Url::parse(playlist_url) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    let mut segs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Ok(u) = base.join(line) {
            segs.push(u.to_string());
        }
    }
    segs
}

async fn fetch_segment(
    client: &reqwest::Client,
    seg: &str,
    part: &std::path::PathBuf,
    limiter: &Arc<crate::limiter::SpeedLimiter>,
    auth: &Option<(String, String)>,
    referer: &Option<String>,
) -> Result<u64> {
    // Retry once for transient failures.
    let mut attempt = 0;
    loop {
        attempt += 1;
        let mut req = client
            .get(seg)
            .header("Accept", "*/*")
            .header("Accept-Encoding", "identity");
        if let Some(r) = referer.as_deref().filter(|s| !s.is_empty()) {
            req = req.header("Referer", r);
        }
        if let Some((u, p)) = auth {
            req = req.basic_auth(u, Some(p));
        }
        match req
            .send()
            .await
        {
            Ok(resp) => {
                if !resp.status().is_success() {
                    if attempt <= 2 {
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                        continue;
                    }
                    anyhow::bail!("segment HTTP {}", resp.status().as_u16());
                }
                let bytes = resp.bytes().await.context("segment read failed")?;
                let len = bytes.len() as u64;
                // Throttle: acquire tokens for this chunk.
                limiter.acquire(len).await;
                std::fs::write(part, &bytes)?;
                return Ok(len);
            }
            Err(e) => {
                if attempt <= 2 {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    continue;
                }
                anyhow::bail!("segment request failed: {e}");
            }
        }
    }
}

/// True when a URL points at an HLS playlist.
pub fn is_hls_url(url: &str) -> bool {
    let lower = url.split(['#', '?']).next().unwrap_or("").to_lowercase();
    lower.ends_with(".m3u8")
}
