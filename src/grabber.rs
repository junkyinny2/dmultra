//! Site Grabber (IDM "grabber"): crawl a starting page up to a depth,
//! collect every downloadable link found (file-type filtered), and hand
//! them to the batch window. Same-origin restriction by default.

use crate::state::State;
use anyhow::Result;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Crawl `start` up to `depth` page hops; push downloadable links into
/// `found`. `done` flips when finished (or errored).
#[allow(clippy::too_many_arguments)]
pub async fn crawl(
    start: &str,
    depth: u32,
    same_origin: bool,
    file_types: &str,
    user_agent: &str,
    found: Arc<Mutex<Vec<String>>>,
    progress: Arc<AtomicU64>,
    pages_crawled: Arc<AtomicU64>,
    done: Arc<AtomicBool>,
) -> Result<()> {
    let base = url::Url::parse(start)?;
    let host = base.host_str().unwrap_or("").to_string();
    let types: Vec<String> = file_types
        .split_whitespace()
        .map(|t| t.trim_start_matches('.').to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    let mut queue: Vec<(url::Url, u32)> = vec![(base.clone(), 0)];
    let mut seen_pages: Vec<String> = Vec::new();
    let mut link_seen: Vec<String> = Vec::new();
    const MAX_PAGES: usize = 200;
    const MAX_LINKS: usize = 2000;

    let client = reqwest::Client::builder()
        .user_agent(user_agent)
        .timeout(std::time::Duration::from_secs(20))
        .build()?;

    while !queue.is_empty() {
        let (page, level) = queue.remove(0);
        if seen_pages.len() >= MAX_PAGES {
            break;
        }
        let page_s = page.to_string();
        if seen_pages.contains(&page_s) {
            continue;
        }
        seen_pages.push(page_s.clone());
        pages_crawled.store(seen_pages.len() as u64, Ordering::Relaxed);
        progress.store(seen_pages.len() as u64, Ordering::Relaxed);

        let body = match client.get(page.clone()).send().await {
            Ok(r) => match r.text().await {
                Ok(t) => t,
                Err(_) => continue,
            },
            Err(_) => continue,
        };

        for link in extract_links(&body, &page) {
            let ls = link.to_string();
            if same_origin && link.host_str().unwrap_or("") != host {
                continue;
            }
            // Filter out javascript:, mailto:, etc.
            if !matches!(link.scheme(), "http" | "https") {
                continue;
            }
            if is_downloadable(&ls, &types) {
                if !link_seen.contains(&ls) {
                    link_seen.push(ls.clone());
                    found.lock().await.push(ls);
                    if link_seen.len() >= MAX_LINKS {
                        done.store(true, Ordering::Relaxed);
                        return Ok(());
                    }
                }
            } else if level < depth && looks_like_page(&ls) {
                if !seen_pages.contains(&ls) {
                    queue.push((link, level + 1));
                }
            }
        }
    }
    done.store(true, Ordering::Relaxed);
    Ok(())
}

/// Extract absolute URLs from `href="..."` and `src="..."` attributes.
/// Scans for the attribute names specifically (not any quoted string).
fn extract_links(body: &str, base: &url::Url) -> Vec<url::Url> {
    let mut out = Vec::new();
    let lower = body.to_lowercase();
    let bytes = body.as_bytes();
    let lower_bytes = lower.as_bytes();
    // Scan for href= / src= / data-src= attributes
    let attrs: [&[u8]; 3] = [b"href=", b"src=", b"data-src="];
    let mut positions: Vec<usize> = Vec::new();
    for attr in &attrs {
        let attr_lower = attr.to_ascii_lowercase();
        let mut i = 0;
        while i + attr_lower.len() <= lower_bytes.len() {
            if &lower_bytes[i..i + attr_lower.len()] == attr_lower.as_slice() {
                positions.push(i);
                i += attr_lower.len();
            } else {
                i += 1;
            }
        }
    }
    positions.sort();
    positions.dedup();
    for pos in positions {
        // Skip past the attribute name and any whitespace
        let mut j = pos;
        // Find the = sign
        while j < bytes.len() && bytes[j] != b'=' {
            j += 1;
        }
        j += 1; // past =
        while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\n') {
            j += 1;
        }
        // Expect quote
        let quote = if j < bytes.len() && (bytes[j] == b'"' || bytes[j] == b'\'') {
            bytes[j]
        } else {
            continue;
        };
        j += 1;
        let start = j;
        while j < bytes.len() && bytes[j] != quote {
            j += 1;
        }
        if j >= bytes.len() {
            continue;
        }
        let candidate = &body[start..j];
        let cand = candidate.trim();
        if cand.is_empty() || cand.starts_with('#') || cand.starts_with("javascript:") {
            continue;
        }
        if cand.starts_with("http://") || cand.starts_with("https://") {
            if let Ok(u) = url::Url::parse(cand) {
                out.push(u);
            }
        } else if let Ok(u) = base.join(cand) {
            out.push(u);
        }
    }
    out
}

fn is_downloadable(link: &str, types: &[String]) -> bool {
    let path = link.split('#').next().unwrap_or("");
    let lower = path.split('?').next().unwrap_or("").to_lowercase();
    let name = lower.rsplit('/').next().unwrap_or("");
    let ext = name.rsplit('.').next().unwrap_or("");
    name.contains('.') && types.iter().any(|t| t == ext)
}

/// True when the URL looks like a browsable page (not a file download,
/// not an image, not a media asset).
fn looks_like_page(link: &str) -> bool {
    let lower = link
        .split(['#', '?'])
        .next()
        .unwrap_or("")
        .trim_end_matches('/')
        .to_lowercase();
    if lower.is_empty() {
        return false;
    }
    // Directory-style URLs (no extension) are pages.
    let last_seg = lower.rsplit('/').next().unwrap_or("");
    if !last_seg.contains('.') {
        return true;
    }
    let ext = last_seg.rsplit('.').next().unwrap_or("");
    matches!(
        ext,
        "" | "html" | "htm" | "xhtml" | "php" | "asp" | "aspx" | "jsp" | "py" | "rb"
            | "cgi" | "cfm" | "do" | "action" | "com" | "net" | "org" | "io" | "us"
            | "uk" | "de" | "fr" | "ru" | "jp" | "cn" | "info" | "edu" | "gov" | "mil"
            | "int" | "eu" | "au" | "ca" | "br" | "in" | "nl" | "se" | "no" | "fi"
            | "dk" | "it" | "es" | "pt" | "ch" | "at" | "be" | "pl" | "cz" | "mx"
    )
}

/// Default file types for the grabber (mirrors the settings default).
pub fn default_types(st: &State) -> String {
    if st.settings.file_types.trim().is_empty() {
        "3gp 7z aac ace aif apk arj asf avi bin bz2 exe gz gzip img iso lzh m4a m4v mkv mov mp3 mp4 mpa mpe mpeg mpg msi msu ogg ogv pdf plj pps ppt qt ra rar rm rmvb sea sit sitx tar tif tiff wav wma wmv z zip".to_string()
    } else {
        st.settings.file_types.clone()
    }
}
/// Fire-and-forget crawl from the GUI thread.
#[allow(clippy::too_many_arguments)]
pub fn spawn_crawl(
    start: String,
    depth: u32,
    same_origin: bool,
    types: String,
    ua: String,
    found: Arc<Mutex<Vec<String>>>,
    finished: Arc<AtomicBool>,
) {
    let _ = crate::manager::runtime().spawn(async move {
        let progress = Arc::new(AtomicU64::new(0));
        let pages = Arc::new(AtomicU64::new(0));
        found.lock().await.clear();
        finished.store(false, Ordering::Relaxed);
        // Share the page counter so the dialog can show live progress.
        // The pages counter is already an Arc; the caller's dialog shares
        // it via the AtomicU64 field on GrabDialog (set externally).
        match crawl(
            &start,
            depth,
            same_origin,
            &types,
            &ua,
            found.clone(),
            progress,
            pages.clone(),
            finished.clone(),
        )
        .await
        {
            Ok(()) => {}
            Err(e) => {
                crate::logfile::log("GRAB", &format!("crawl failed: {e:#}"));
            }
        }
        finished.store(true, Ordering::Relaxed);
    });
}