//! Site Grabber: crawl a starting page and collect file-type-matched URLs.

use anyhow::{bail, Result};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use url::Url;

const MAX_PAGES: usize = 200;
const MAX_LINKS: usize = 2_000;

#[derive(Debug, Default)]
pub struct CrawlResult {
    pub links: Vec<String>,
    pub pages_crawled: usize,
    pub failed_pages: Vec<String>,
}

pub async fn crawl(
    start: &str,
    depth: u32,
    same_origin: bool,
    file_types: &str,
    user_agent: &str,
    pages_crawled: Arc<AtomicU64>,
) -> Result<CrawlResult> {
    let base = Url::parse(start)?;
    if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
        bail!("Start address must be an HTTP or HTTPS URL.");
    }

    let patterns = crate::filetypes::parse_list(file_types);
    let mut pending = VecDeque::from([(base.clone(), 0)]);
    let mut seen_pages = HashSet::new();
    let mut seen_links = HashSet::new();
    let mut result = CrawlResult::default();
    let client = reqwest::Client::builder()
        .user_agent(user_agent)
        .timeout(std::time::Duration::from_secs(20))
        .build()?;

    while let Some((page, level)) = pending.pop_front() {
        if seen_pages.len() >= MAX_PAGES {
            break;
        }
        if !seen_pages.insert(page.to_string()) {
            continue;
        }
        result.pages_crawled = seen_pages.len();
        pages_crawled.store(result.pages_crawled as u64, Ordering::Relaxed);

        let response = match client.get(page.clone()).send().await {
            Ok(response) => response,
            Err(error) => {
                result.failed_pages.push(format!("{page}: {error}"));
                continue;
            }
        };
        let response = match response.error_for_status() {
            Ok(response) => response,
            Err(error) => {
                result.failed_pages.push(format!("{page}: {error}"));
                continue;
            }
        };
        let resolved_page = response.url().clone();
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => {
                result
                    .failed_pages
                    .push(format!("{resolved_page}: {error}"));
                continue;
            }
        };

        for link in extract_links(&body, &resolved_page) {
            if !matches!(link.scheme(), "http" | "https")
                || (same_origin && link.origin() != base.origin())
            {
                continue;
            }
            let link_text = link.to_string();
            if crate::filetypes::matches(link.path(), &patterns) {
                if seen_links.insert(link_text.clone()) {
                    result.links.push(link_text);
                    if result.links.len() >= MAX_LINKS {
                        return Ok(result);
                    }
                }
            } else if level < depth && looks_like_page(&link_text) {
                pending.push_back((link, level + 1));
            }
        }
    }
    Ok(result)
}

fn extract_links(body: &str, base: &Url) -> Vec<Url> {
    let mut out = Vec::new();
    let lower = body.to_ascii_lowercase();
    let bytes = body.as_bytes();
    let lower_bytes = lower.as_bytes();
    let attrs: [&[u8]; 3] = [b"href=", b"src=", b"data-src="];
    let mut positions = Vec::new();
    for attr in attrs {
        let mut i = 0;
        while i + attr.len() <= lower_bytes.len() {
            if &lower_bytes[i..i + attr.len()] == attr {
                positions.push(i);
                i += attr.len();
            } else {
                i += 1;
            }
        }
    }
    positions.sort_unstable();
    positions.dedup();
    for pos in positions {
        let mut value_start = pos;
        while value_start < bytes.len() && bytes[value_start] != b'=' {
            value_start += 1;
        }
        value_start += 1;
        while value_start < bytes.len()
            && matches!(bytes[value_start], b' ' | b'\t' | b'\n' | b'\r')
        {
            value_start += 1;
        }
        if value_start >= bytes.len() || !matches!(bytes[value_start], b'"' | b'\'') {
            continue;
        }
        let quote = bytes[value_start];
        value_start += 1;
        let mut value_end = value_start;
        while value_end < bytes.len() && bytes[value_end] != quote {
            value_end += 1;
        }
        if value_end == bytes.len() {
            continue;
        }
        let value = body[value_start..value_end].trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        if let Ok(url) = base.join(value) {
            out.push(url);
        }
    }
    out
}

fn looks_like_page(link: &str) -> bool {
    let path = link
        .split(['#', '?'])
        .next()
        .unwrap_or("")
        .trim_end_matches('/')
        .to_ascii_lowercase();
    let last_segment = path.rsplit('/').next().unwrap_or("");
    if !last_segment.contains('.') {
        return true;
    }
    matches!(
        last_segment.rsplit('.').next().unwrap_or(""),
        "html"
            | "htm"
            | "xhtml"
            | "php"
            | "asp"
            | "aspx"
            | "jsp"
            | "py"
            | "rb"
            | "cgi"
            | "cfm"
            | "do"
            | "action"
    )
}

pub fn default_types(st: &crate::state::State) -> String {
    if st.settings.file_types.trim().is_empty() {
        crate::filetypes::DEFAULT_FILE_TYPES.to_string()
    } else {
        st.settings.file_types.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn resolves_quoted_links_and_filters_extensions() {
        let base = Url::parse("https://example.test/downloads/index.html").unwrap();
        let links = extract_links(
            r#"<a href="../5GB.zip">x</a><img src='/image.png'><a href='movie.R03?x=1'>y</a>"#,
            &base,
        );
        let patterns = crate::filetypes::parse_list("ZIP R0*");
        let files: Vec<_> = links
            .iter()
            .filter(|url| crate::filetypes::matches(url.path(), &patterns))
            .map(Url::as_str)
            .collect();
        assert_eq!(
            files,
            [
                "https://example.test/5GB.zip",
                "https://example.test/downloads/movie.R03?x=1"
            ]
        );
    }

    #[test]
    fn identifies_crawlable_pages_without_treating_files_as_pages() {
        assert!(looks_like_page("https://example.test/path/"));
        assert!(looks_like_page("https://example.test/index.php?id=1"));
        assert!(!looks_like_page("https://example.test/archive.zip"));
    }

    #[tokio::test]
    async fn crawl_collects_matching_links_and_respects_origin() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            let body = r#"<a href="/files/5GB.zip">download</a><a href="https://elsewhere.test/other.zip">external</a>"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let pages = Arc::new(AtomicU64::new(0));
        let result = crawl(
            &format!("http://{address}/"),
            1,
            true,
            "ZIP",
            "DM Ultra test",
            pages.clone(),
        )
        .await
        .unwrap();
        server.join().unwrap();

        assert_eq!(result.links, [format!("http://{address}/files/5GB.zip")]);
        assert_eq!(result.pages_crawled, 1);
        assert_eq!(pages.load(Ordering::Relaxed), 1);
        assert!(result.failed_pages.is_empty());
    }
}
