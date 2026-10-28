//! Minimal asynchronous FTP client (control + PASV data connections).
//!
//! Command flow: USER/PASS ("Sending user name...", "Sending
//! password...", "User name OK"), TYPE I, SIZE, PASV ("PASV OK,
//! extracting address...", "ftp server doesn't support PASV"), REST +
//! RETR per segment ("REST %I64d" + "REST OK"/"REST failed"). PASV is
//! the default mode; segmented FTP downloads use several data
//! connections each positioned with REST.

use anyhow::{anyhow, bail, Context, Result};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct FtpClient {
    ctrl: TcpStream,
    last_code: Option<u16>,
}

pub struct FtpProbe {
    pub total: u64,
    pub resumable: bool,
}

fn parse_pasv_port(reply: &str) -> Option<(String, u16)> {
    let start = reply.find('(')?;
    let end = reply[start..].find(')')? + start;
    let nums: Vec<u32> = reply[start + 1..end]
        .split(',')
        .filter_map(|n| n.trim().parse().ok())
        .collect();
    if nums.len() != 6 {
        return None;
    }
    let host = format!("{}.{}.{}.{}", nums[0], nums[1], nums[2], nums[3]);
    let port = ((nums[4] << 8) | nums[5]) as u16;
    Some((host, port))
}

impl FtpClient {
    pub async fn connect(host: &str, port: u16, user: &str, pass: &str) -> Result<FtpClient> {
        let mut stream = TcpStream::connect((host, port))
            .await
            .with_context(|| format!("connecting to ftp://{host}:{port}"))?;
        let mut client = FtpClient { ctrl: stream, last_code: None };
        let (code, _) = client.read_reply().await?;
        if code == 220 {
            // USER
            client.command_expect(&format!("USER {user}"), &[230, 331]).await?;
            // Some servers answer USER with 331; send PASS.
            if client.last_code == Some(331) {
                client.command_expect(&format!("PASS {pass}"), &[230, 202]).await?;
            }
        }
        // Binary mode.
        client.command_expect("TYPE I", &[200]).await?;
        Ok(client)
    }

    pub async fn command_expect(&mut self, cmd: &str, expect: &[u16]) -> Result<(u16, String)> {
        self.ctrl
            .write_all(format!("{cmd}\r\n").as_bytes())
            .await
            .with_context(|| format!("sending FTP command {cmd}"))?;
        let (code, text) = self.read_reply().await?;
        if !expect.contains(&code) {
            bail!("FTP command {cmd:?} failed: {code} {text}");
        }
        self.last_code = Some(code);
        Ok((code, text))
    }

    pub async fn read_reply(&mut self) -> Result<(u16, String)> {
        let mut buf = Vec::with_capacity(256);
        let mut byte = [0u8; 1];
        loop {
            let n = self.ctrl.read(&mut byte).await?;
            if n == 0 {
                bail!("FTP control connection closed");
            }
            buf.push(byte[0]);
            if buf.ends_with(b"\r\n") {
                let line = String::from_utf8_lossy(&buf).trim().to_string();
                buf.clear();
                // Multi-line replies: "123-..." continues until "123 ..."
                if line.len() >= 4 && line.as_bytes()[3] == b'-' {
                    continue;
                }
                if let Ok(code) = line[..3.min(line.len())].parse::<u16>() {
                    return Ok((code, line));
                }
                bail!("invalid FTP reply: {line}");
            }
        }
    }

    /// Resolve the working path for a URL like ftp://host/dir/file.
    pub async fn path_for(&mut self, path: &str) -> Result<()> {
        let dir = match path.rsplit_once('/') {
            Some((d, _)) if !d.is_empty() => d.to_string(),
            _ => "/".to_string(),
        };
        self.command_expect(&format!("CWD {dir}"), &[250, 200]).await?;
        Ok(())
    }

    pub async fn size(&mut self, file: &str) -> Result<Option<u64>> {
        let (code, text) = self.command_expect(&format!("SIZE {file}"), &[213, 550]).await?;
        if code == 550 {
            return Ok(None);
        }
        Ok(text
            .split_whitespace()
            .nth(1)
            .and_then(|n| n.parse().ok()))
    }

    /// Open a PASV data connection (PASV is the default; PORT only
    /// when PASV is disabled).
    pub async fn open_data(&mut self) -> Result<TcpStream> {
        let (_, text) = self.command_expect("PASV", &[227]).await?;
        let (host, port) =
            parse_pasv_port(&text).ok_or_else(|| anyhow!("cannot parse PASV reply: {text}"))?;
        TcpStream::connect((host.as_str(), port))
            .await
            .context("connecting PASV data socket")
    }

    /// Begin a RETR at `start` (REST resume).
    /// Returns the data stream. `end` is only used to size the request;
    /// FTP has no end offset, so the caller stops reading at the chunk
    /// boundary.
    pub async fn retr(
        &mut self,
        file: &str,
        start: u64,
        end: Option<u64>,
        host: &str,
    ) -> Result<(TcpStream, u64)> {
        let mut data = self.open_data().await?;
        if start > 0 {
            // REST must succeed to allow resume.
            self.command_expect(&format!("REST {start}"), &[350]).await?;
        }
        self.ctrl
            .write_all(format!("RETR {}\r\n", file).as_bytes())
            .await?;
        // Expect 150 on the control connection.
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let (code, text) = self.read_reply().await?;
                if code == 150 || code == 125 {
                    return Ok::<(), anyhow::Error>(());
                }
                if code >= 400 {
                    bail!("RETR failed: {code} {text}");
                }
            }
        })
        .await
        .map_err(|_| anyhow!("timeout waiting for RETR response"))??;

        let len = match end {
            Some(e) => e.saturating_sub(start),
            None => 0,
        };
        let _ = host;
        Ok((data, len))
    }

    pub async fn quit(&mut self) {
        let _ = self.command_expect("QUIT", &[221]).await;
    }
}

/// Probe an FTP URL: login, TYPE I, CWD, SIZE. Returns total size;
/// resumability is decided lazily by whether REST succeeds.
pub async fn probe_ftp(
    url: &url::Url,
    user: &str,
    pass: &str,
) -> Result<(Arc<tokio::sync::Mutex<FtpClient>>, FtpProbe)> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("FTP URL has no host"))?
        .to_string();
    let port = url.port().unwrap_or(21);
    let path = url.path().trim_start_matches('/');
    let mut client = FtpClient::connect(&host, port, user, pass).await?;
    client.path_for(&format!("/{path}")).await?;
    let file = path.rsplit('/').next().unwrap_or(path).to_string();
    let total = client.size(&file).await?.unwrap_or(0);
    // REST support is checked when a resume is actually attempted;
    // assume resumable when SIZE worked.
    let probe = FtpProbe { total, resumable: total > 0 };
    Ok((Arc::new(tokio::sync::Mutex::new(client)), probe))
}
