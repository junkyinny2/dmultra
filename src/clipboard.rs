//! Clipboard monitoring.
//!
//! Prompts:
//!   - "Download link was found in clipboard. Do you want to download
//!     it?"
//!   - "Several URLs found in clipboard. Do you want to download
//!     them?"
//!   - "Batch download from clipboard."
//!   - "Cannot find any web address in the clipboard!"

use crate::state::State;
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Mutex};

/// Extract download-candidate URLs from clipboard text
/// (http/https/ftp schemes; MMS/RTMP/HLS are out of scope for this
/// clone).
pub fn extract_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for token in text.split_whitespace() {
        let token = token.trim_matches(|c: char| {
            matches!(c, '<' | '>' | '"' | '\'' | ',' | ';' | '.')
        });
        let Ok(url) = url::Url::parse(token) else {
            continue;
        };
        if matches!(url.scheme(), "http" | "https" | "ftp") {
            let value = url.to_string();
            if !urls.contains(&value) {
                urls.push(value);
            }
        }
    }
    urls
}

pub fn is_download_candidate(url: &str, settings: &crate::state::Settings) -> bool {
    let Some(parsed) = url::Url::parse(url).ok() else {
        return false;
    };
    if parsed.scheme() == "ftp" {
        return true;
    }
    let mut patterns = crate::filetypes::parse_list(&settings.file_types);
    patterns.extend(crate::filetypes::parse_list(&settings.clipboard_extensions));
    crate::filetypes::matches(parsed.path(), &patterns)
}

pub fn needs_extensionless_probe(url: &str, settings: &crate::state::Settings) -> bool {
    settings.clipboard_probe
        && url::Url::parse(url).ok().is_some_and(|parsed| {
            matches!(parsed.scheme(), "http" | "https")
                && parsed
                    .path_segments()
                    .and_then(|mut segments| segments.next_back())
                    .is_some_and(|name| !name.is_empty() && !name.contains('.'))
        })
}

pub async fn probe_extensionless_url(url: &str) -> Result<bool> {
    let response = reqwest::Client::new()
        .head(url)
        .timeout(Duration::from_secs(10))
        .send()
        .await?;
    if !response.status().is_success() {
        return Ok(false);
    }
    let headers = response.headers();
    let has_filename = headers
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("filename="));
    let content_type_is_file = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let value = value.to_ascii_lowercase();
            value.starts_with("application/")
                || value.starts_with("audio/")
                || value.starts_with("image/")
                || value.starts_with("video/")
        });
    Ok(has_filename || content_type_is_file)
}

fn prompt(question: &str) -> bool {
    use std::io::{BufRead, Write};
    print!("{question} (y/n) ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Run the clipboard monitor until Ctrl+C.
pub async fn monitor_loop(
    state: Arc<Mutex<State>>,
    limiter: Arc<crate::limiter::SpeedLimiter>,
    accept_all_types: bool,
) -> Result<()> {
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let stop = Arc::new(AtomicBool::new(false));

    // Ctrl+C: cancels any active download and stops the monitor.
    {
        let stop = stop.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = cancel_tx.send(true);
            stop.store(true, Ordering::SeqCst);
        });
    }

    let st = state.lock().await;
    let patterns = crate::filetypes::parse_list(&st.settings.file_types);
    let _monitor_flag = st.settings.monitor_clipboard;
    drop(st);

    println!("Monitoring system clipboard for links to download. Press Ctrl+C to stop.");

    let mut last = String::new();
    loop {
        if stop.load(Ordering::SeqCst) {
            println!("Monitor stopped.");
            return Ok(());
        }

        let text = tokio::task::spawn_blocking(|| {
            arboard::Clipboard::new()
                .and_then(|mut c| c.get_text())
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();

        if !text.is_empty() && text != last {
            last = text.clone();
            let urls = extract_urls(&text);
            let candidates: Vec<String> = urls
                .iter()
                .filter(|u| {
                    accept_all_types
                        || u.starts_with("ftp://")
                        || crate::filetypes::matches(u, &patterns)
                })
                .cloned()
                .collect();

            if candidates.len() == 1 {
                println!();
                println!("Download link was found in clipboard. Do you want to download it?");
                println!("  {}", candidates[0]);
                if prompt("Download?") {
                    let (rec, outcome) =
                        crate::cli_download(state.clone(), &candidates[0], limiter.clone(), cancel_rx.clone()).await;
                    crate::report_outcome(&rec, &outcome);
                }
            } else if candidates.len() > 1 {
                println!();
                println!("Several URLs found in clipboard. Do you want to download them?");
                println!("Batch download from clipboard.");
                for (i, u) in candidates.iter().enumerate() {
                    println!("  {:>2}. {u}", i + 1);
                }
                if prompt("Download all?") {
                    for u in &candidates {
                        let (rec, outcome) =
                            crate::cli_download(state.clone(), u, limiter.clone(), cancel_rx.clone()).await;
                        crate::report_outcome(&rec, &outcome);
                    }
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_urls() {
        let text = "look at https://example.com/file.ZIP and ftp://host/dir/setup.exe\nignore plain text";
        let urls = extract_urls(text);
        assert_eq!(urls.len(), 2);
        assert!(urls[0].starts_with("https://example.com/file.ZIP"));
        assert!(urls[1].starts_with("ftp://host/dir/setup.exe"));
    }

    #[test]
    fn extracts_urls_with_wrapping_punctuation_and_deduplicates() {
        let urls = extract_urls(
            "Download <https://example.com/a.zip>, then https://example.com/a.zip.",
        );
        assert_eq!(urls, ["https://example.com/a.zip"]);
    }

    #[test]
    fn file_type_candidate_uses_general_and_custom_patterns() {
        let mut settings = crate::state::Settings::default();
        settings.file_types = "ZIP R0*".into();
        settings.clipboard_extensions = ".CBZ".into();
        assert!(is_download_candidate("https://example.test/a.zip?token=1", &settings));
        assert!(is_download_candidate("https://example.test/a.r03", &settings));
        assert!(is_download_candidate("https://example.test/a.cbz", &settings));
        assert!(!is_download_candidate("https://example.test/page.html", &settings));
        settings.clipboard_probe = true;
        assert!(!is_download_candidate(
            "https://example.test/download?id=3",
            &settings
        ));
        assert!(needs_extensionless_probe(
            "https://example.test/download?id=3",
            &settings
        ));
        assert!(!needs_extensionless_probe(
            "https://example.test/download/file.zip",
            &settings
        ));
    }
}
