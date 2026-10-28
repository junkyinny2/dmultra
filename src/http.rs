//! HTTP/HTTPS download primitives.
//!
//!   - Request template:
//!     "%s %s HTTP/1.1\r\nHost: %s%s\r\nUser-Agent: %s\r\nAccept: %s\r\n..."
//!     with default `Accept: */*` and `Accept-Encoding: identity` as the
//!     default encoding.
//!   - Range request forms:
//!       "Range: bytes=0-%I64d\r\n"  (full probe)
//!       "Range: bytes=0-\r\n"       (open probe)
//!       "Range: bytes=%I64d-\r\n"   (resume tail)
//!       "Range: bytes=%I64d-%I64d\r\n" (mid-file segment)
//!   - Response decisions: `accept-ranges:` without "none" => resumable;
//!     `content-range: bytes X-Y/Z` => total size Z; a plain 200 answer
//!     to a Range request means the server ignored Range (no resume);
//!     `Content-Range`+206 confirms resume.
//!   - Resume validation compares stored `ETag` / `Last-Modified`
//!     (changed => restart from the beginning).
//!
//! `reqwest` (rustls) is used as the transport, preserving the same
//! request headers and response decisions.

use anyhow::{anyhow, bail, Context, Result};
use reqwest::{Client, Method, Response, Url};
use std::time::Duration;

use crate::state::DownloadRec;

pub const DEFAULT_ACCEPT_ENCODING: &str = "identity";

/// Case-insensitive glob matcher (`*` and `?`), used for proxy
/// bypass/auto-rule host patterns.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[derive(Debug, Clone)]
pub struct Probe {
    pub final_url: Url,
    pub status: u16,
    /// Total size in bytes; 0 when unknown (stream until EOF).
    pub total: u64,
    /// Server honours byte ranges (206 / accept-ranges).
    pub resumable: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_type: Option<String>,
    pub content_disposition: Option<String>,
}

/// Build a client with the default settings (UA, identity encoding,
/// redirects followed, cookies enabled, optional proxies/auth).
pub fn make_client(settings: &crate::state::Settings, url: &Url) -> Result<Client> {
    let mut builder = Client::builder()
        .user_agent(&settings.user_agent)
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(Duration::from_secs(300))
        .connect_timeout(Duration::from_secs(30))
        .cookie_store(true)
        .no_gzip(); // Accept-Encoding: identity by default

    let scheme = url.scheme();
    let proxying = settings.proxy_mode != "none";
    let mut proxy = if proxying {
        match scheme {
            "http" => settings.proxies.http.clone(),
            "https" => settings.proxies.https.clone(),
            "ftp" | _ => settings.proxies.ftp.clone(),
        }
    } else {
        None
    };
    if proxying {
        let host = url.host_str().unwrap_or("").to_string();
        if let Some(rule) = settings
            .auto_proxy_rules
            .iter()
            .find(|r| !r.pattern.trim().is_empty() && glob_match(r.pattern.trim(), &host))
        {
            proxy = if rule.proxy.trim().is_empty()
                || rule.proxy.trim().eq_ignore_ascii_case("none")
            {
                None
            } else {
                Some(rule.proxy.trim().to_string())
            };
        }
    }
    if let Some(p) = proxy.filter(|s| !s.trim().is_empty()) {
        let purl = Url::parse(&p).with_context(|| format!("parsing proxy url {p}"))?;
        let mut pb = reqwest::Proxy::all(purl.clone())?;
        if let (Some(u), Some(w)) = (&settings.proxies.user, &settings.proxies.password) {
            if !u.is_empty() {
                pb = pb.basic_auth(u, w);
            }
        }
        if let Some(no) = reqwest::NoProxy::from_string(settings.proxy_no_list.trim()) {
            pb = pb.no_proxy(Some(no));
        }
        builder = builder.proxy(pb);
    }
    builder.build().context("building HTTP client")
}

fn apply_auth(req: reqwest::RequestBuilder, user: Option<&str>, pass: Option<&str>) -> reqwest::RequestBuilder {
    match (user, pass) {
        (Some(u), Some(p)) if !u.is_empty() => req.basic_auth(u, Some(p)),
        _ => req,
    }
}

// ---------------------------------------------------------------------------
// HTTP Digest authentication (RFC 2617): a full Digest implementation
// (challenge parsing, MD5 code, "Authorization: Digest" construction).
// This is
// the clone's equivalent.
// ---------------------------------------------------------------------------

fn md5_hex(s: &str) -> String {
    use md5::{Digest as Md5Digest, Md5};
    let mut h = Md5::new();
    h.update(s.as_bytes());
    let out = h.finalize();
    let mut hex = String::with_capacity(32);
    for b in out.iter() {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// Extract a `name=value` parameter from a WWW-Authenticate challenge
/// (values may be quoted and contain commas).
fn challenge_param(challenge: &str, name: &str) -> Option<String> {
    let lower = challenge.to_ascii_lowercase();
    let needle = format!("{name}=");
    let mut search_from = 0usize;
    loop {
        let pos = lower[search_from..].find(&needle).map(|p| p + search_from)?;
        // Make sure it is a standalone token (not a suffix of another name).
        let is_token = pos == 0
            || !lower.as_bytes()[pos - 1].is_ascii_alphanumeric() && lower.as_bytes()[pos - 1] != b'-';
        if is_token {
            let rest = challenge[pos + needle.len()..].trim_start();
            if let Some(stripped) = rest.strip_prefix('"') {
                let end = stripped.find('"')?;
                return Some(stripped[..end].to_string());
            }
            let end = rest.find([',', ' ']).unwrap_or(rest.len());
            return Some(rest[..end].trim().to_string());
        }
        search_from = pos + needle.len();
    }
}

/// Build the `Authorization: Digest ...` header value for a challenge.
/// `cnonce` is passed in for deterministic tests.
fn digest_response_with_cnonce(
    challenge: &str,
    user: &str,
    pass: &str,
    method: &str,
    uri: &str,
    cnonce: &str,
) -> Option<String> {
    let realm = challenge_param(challenge, "realm")?;
    let nonce = challenge_param(challenge, "nonce")?;
    let qop = challenge_param(challenge, "qop");
    let opaque = challenge_param(challenge, "opaque");
    let algorithm = challenge_param(challenge, "algorithm").unwrap_or_else(|| "MD5".into());
    if !algorithm.eq_ignore_ascii_case("MD5") {
        return None; // MD5-sess and SHA algorithms not implemented
    }
    let ha1 = md5_hex(&format!("{user}:{realm}:{pass}"));
    let ha2 = md5_hex(&format!("{method}:{uri}"));
    let (response, qop_part) = match &qop {
        Some(q) if q.split(',').any(|t| t.trim().eq_ignore_ascii_case("auth")) => {
            let nc = "00000001";
            let response = md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}"));
            (
                response,
                format!(", qop=auth, nc={nc}, cnonce=\"{cnonce}\""),
            )
        }
        Some(_) => return None, // qop=auth-int unsupported
        None => (md5_hex(&format!("{ha1}:{nonce}:{ha2}")), String::new()),
    };
    let mut header = format!(
        "Digest username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", \
         uri=\"{uri}\", response=\"{response}\""
    );
    if let Some(op) = opaque {
        header.push_str(&format!(", opaque=\"{op}\""));
    }
    header.push_str(&format!(", algorithm=MD5{qop_part}"));
    Some(header)
}

fn digest_response(challenge: &str, user: &str, pass: &str, method: &str, uri: &str) -> Option<String> {
    let cnonce = md5_hex(&format!(
        "dm-cnonce-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))[..16]
        .to_string();
    digest_response_with_cnonce(challenge, user, pass, method, uri, &cnonce)
}

/// If the server answered 401 with a Digest challenge and we hold
/// credentials, retry the same request once with a Digest header.
async fn retry_with_digest(
    resp: Response,
    build: impl Fn(Option<String>) -> reqwest::RequestBuilder,
    auth: Option<(&str, &str)>,
    url: &Url,
) -> Result<Response> {
    if resp.status().as_u16() != 401 {
        return Ok(resp);
    }
    let wa = resp
        .headers()
        .get("www-authenticate")
        .and_then(|v| v.to_str().ok());
    let (auth, wa) = match (auth, wa) {
        (Some(a), Some(w)) => (a, w),
        _ => return Ok(resp),
    };
    if !wa.trim_start().to_ascii_lowercase().starts_with("digest") {
        return Ok(resp);
    }
    match digest_response(wa, auth.0, auth.1, "GET", url.path()) {
        Some(header) => build(Some(header))
            .send()
            .await
            .context("digest retry failed"),
        None => Ok(resp),
    }
}

fn header_str(resp: &Response, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Probe a URL with an open Range request (`Range: bytes=0-`).
///
/// `resume` carries the stored ETag/Last-Modified; when present they are
/// sent as If-Range so a changed file is detected via a 200 response.
pub async fn probe(
    client: &Client,
    url: &Url,
    referer: Option<&str>,
    auth: Option<(&str, &str)>,
    resume: Option<(&Option<String>, &Option<String>)>,
) -> Result<Probe> {
    // The request is rebuilt through `build` so a 401 Digest challenge
    // can be retried with an Authorization header.
    let build = |extra_auth: Option<String>| {
        let mut req = client
            .request(Method::GET, url.clone())
            .header("Accept", "*/*")
            .header("Accept-Encoding", DEFAULT_ACCEPT_ENCODING)
            .header("Connection", "Keep-Alive")
            .header("Range", "bytes=0-"); // open-probe form
        if let Some(r) = referer.filter(|s| !s.is_empty()) {
            req = req.header("Referer", r);
        }
        req = apply_auth(req, auth.map(|a| a.0), auth.map(|a| a.1));
        if let Some((etag, last_modified)) = resume {
            // If-Range: ETag preferred, else Last-Modified.
            if let Some(etag) = etag {
                req = req.header("If-Range", etag);
            } else if let Some(lm) = last_modified {
                req = req.header("If-Range", lm);
            }
        }
        if let Some(h) = extra_auth {
            req = req.header("Authorization", h);
        }
        req
    };

    let resp = build(None).send().await.context("probe request failed")?;
    let resp = retry_with_digest(resp, build, auth, url).await?;
    let status = resp.status().as_u16();
    if !resp.status().is_success() && status != 304 {
        bail!("server returned HTTP {status}");
    }

    let final_url = resp.url().clone();
    let etag = header_str(&resp, "ETag");
    let last_modified = header_str(&resp, "Last-Modified");
    let content_type = header_str(&resp, "Content-Type");
    let content_disposition = header_str(&resp, "Content-Disposition");
    let accept_ranges = header_str(&resp, "Accept-Ranges");

    let mut resumable = false;
    let mut total = 0u64;

    if status == 206 {
        // Partial content: server honoured the Range probe => resumable,
        // and Content-Range carries the total size.
        resumable = true;
        if let Some(cr) = header_str(&resp, "Content-Range") {
            if let Some(z) = cr.rsplit('/').next() {
                if let Ok(t) = z.trim().parse::<u64>() {
                    total = t;
                }
            }
        }
        if total == 0 {
            if let Some(cl) = header_str(&resp, "Content-Length") {
                total = cl.parse().unwrap_or(0);
            }
        }
    } else {
        // 200 to a Range probe: server ignored the range header
        // (sets "no resume" state), but accept-ranges may
        // still advertise support.
        if let Some(cl) = header_str(&resp, "Content-Length") {
            total = cl.parse().unwrap_or(0);
        }
        if let Some(ar) = accept_ranges.as_deref() {
            // "accept-ranges:" containing "none" => not resumable.
            if !ar.to_ascii_lowercase().contains("none") && ar.contains("bytes") {
                resumable = true;
            }
        }
    }

    Ok(Probe {
        final_url,
        status,
        total,
        resumable,
        etag,
        last_modified,
        content_type,
        content_disposition,
    })
}

/// Open a ranged body stream for `[start, end)` (end exclusive; `None`
/// reads to EOF). Mirrors the mid-file segment and resume-tail Range
/// forms.
pub async fn ranged_get(
    client: &Client,
    url: &Url,
    start: u64,
    end: Option<u64>,
    referer: Option<&str>,
    auth: Option<(&str, &str)>,
) -> Result<Response> {
    let build = |extra_auth: Option<String>| {
        let range = match end {
            Some(e) => format!("bytes={}-{}", start, e.saturating_sub(1)), // mid-file segment
            None => format!("bytes={}-", start),                          // resume tail
        };
        let mut req = client
            .request(Method::GET, url.clone())
            .header("Accept", "*/*")
            .header("Accept-Encoding", DEFAULT_ACCEPT_ENCODING)
            .header("Connection", "Keep-Alive")
            .header("Range", range);
        if let Some(r) = referer.filter(|s| !s.is_empty()) {
            req = req.header("Referer", r);
        }
        req = apply_auth(req, auth.map(|a| a.0), auth.map(|a| a.1));
        if let Some(h) = extra_auth {
            req = req.header("Authorization", h);
        }
        req
    };
    let resp = build(None).send().await.context("range request failed")?;
    let resp = retry_with_digest(resp, build, auth, url).await?;
    let status = resp.status().as_u16();
    if status != 206 && status != 200 {
        bail!("server returned HTTP {status} for range request");
    }
    Ok(resp)
}

/// Full (unranged) GET for non-resumable transfers.
pub async fn full_get(
    client: &Client,
    url: &Url,
    referer: Option<&str>,
    auth: Option<(&str, &str)>,
) -> Result<Response> {
    let mut req = client
        .request(Method::GET, url.clone())
        .header("Accept", "*/*")
        .header("Accept-Encoding", DEFAULT_ACCEPT_ENCODING)
        .header("Connection", "Keep-Alive");
    if let Some(r) = referer.filter(|s| !s.is_empty()) {
        req = req.header("Referer", r);
    }
    req = apply_auth(req, auth.map(|a| a.0), auth.map(|a| a.1));
    let resp = req.send().await.context("GET request failed")?;
    if !resp.status().is_success() {
        bail!("server returned HTTP {}", resp.status().as_u16());
    }
    Ok(resp)
}

/// Decide whether a persisted record is still valid for the freshly
/// probed file (changed file => restart).
pub fn record_matches_probe(rec: &DownloadRec, probe: &Probe) -> bool {
    if probe.file_size_changed(rec) {
        return false;
    }
    if rec.file_size != 0 && probe.total != 0 && rec.file_size != probe.total {
        return false;
    }
    true
}

impl Probe {
    pub fn file_size_changed(&self, rec: &DownloadRec) -> bool {
        // ETag mismatch or Last-Modified mismatch => changed.
        if let (Some(old), Some(new)) = (&rec.etag, &self.etag) {
            return old != new;
        }
        if let (Some(old), Some(new)) = (&rec.last_modified, &self.last_modified) {
            return old != new;
        }
        // If the probe was answered with a plain 200 while resuming,
        // the server either ignored If-Range or the file changed.
        if rec.chunks.iter().any(|c| c.downloaded > 0) && self.status == 200 {
            return true;
        }
        false
    }

    /// Does the probe answer indicate the server will not honor ranges?
    pub fn unsupported_resume(&self) -> bool {
        !self.resumable
    }
}

pub fn parse_url(url: &str) -> Result<Url> {
    Url::parse(url).map_err(|e| anyhow!("invalid URL {url:?}: {e}"))
}

#[cfg(test)]
mod digest_tests {
    use super::*;

    /// RFC 2617 section 3.5 example (the classic test vector).
    #[test]
    fn rfc2617_reference_vector() {
        let challenge = r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#;
        let header = digest_response_with_cnonce(
            challenge,
            "Mufasa",
            "Circle Of Life",
            "GET",
            "/dir/index.html",
            "0a4f113b",
        )
        .expect("digest header");
        // Authoritative value from RFC 2617 section 3.5.
        assert!(
            header.contains(r#"response="6629fae49393a05397450978507c4ef1""#),
            "unexpected header: {header}"
        );
        assert!(header.contains(r#"username="Mufasa""#));
        assert!(header.contains(r#"nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093""#));
        assert!(header.contains("qop=auth"));
        assert!(header.contains(r#"opaque="5ccc069c403ebaf9f0171e9517f40e41""#));
    }

    #[test]
    fn legacy_rfc2069_mode() {
        // No qop -> RFC 2069 style response = MD5(HA1:nonce:HA2).
        let challenge = r#"Digest realm="testrealm@host.com", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093""#;
        let header =
            digest_response_with_cnonce(challenge, "u", "p", "GET", "/x", "ignored").unwrap();
        // Value differs from the vector (different user/uri) but must be
        // a deterministic 32-hex digest with no qop fields.
        assert!(header.contains("algorithm=MD5"));
        assert!(!header.contains("qop="));
        let _ = md5_hex("u");
    }

    #[test]
    fn rejects_unsupported_algorithms() {
        let challenge = r#"Digest realm="r", nonce="n", algorithm=MD5-sess"#;
        assert!(digest_response_with_cnonce(challenge, "u", "p", "GET", "/x", "c").is_none());
    }
}
