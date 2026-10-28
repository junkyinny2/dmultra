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
    text.split_whitespace()
        .filter_map(|tok| url::Url::parse(tok).ok())
        .filter(|u| matches!(u.scheme(), "http" | "https" | "ftp"))
        .map(|u| u.to_string())
        .collect()
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
}
