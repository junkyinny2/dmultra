//! `dmultra` Ã¢â‚¬â€ DM Ultra, a multi-threaded download manager written in
//! Multi-threaded download manager written in Rust with webhooks.

// Release builds use the Windows GUI subsystem: double-clicking never
// flashes a console/terminal window. Debug builds keep the console for
// log output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod batch;
mod browser;
mod clipboard;
mod dash;
mod engine;
mod filetypes;
mod ftp;
mod grabber;
mod hls;
mod i18n;
mod gui;
mod hooks;
mod http;
mod limiter;
mod logfile;
mod manager;
#[cfg(windows)]
mod overlay;
mod options;
mod postdownload;
mod queue;
mod rtsp;
mod state;
mod ui;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use state::{DownloadRec, State, Status};
use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use tokio::sync::{watch, Mutex};

use crate::engine::Outcome;

#[derive(Parser)]
#[command(
    name = "dmultra",
    version,
    about = "DM Ultra - multi-threaded download manager",
    long_about = None
)]
struct Cli {
    /// With no subcommand the GUI launches (double-click friendly).
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Add a URL and start downloading
    Add {
        url: String,
        /// Save as this path (default derived from URL/Content-Disposition)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Max connections for this download (default: 8)
        #[arg(short = 'c', long)]
        connections: Option<u32>,
        /// HTTP auth user
        #[arg(short = 'u', long)]
        user: Option<String>,
        /// HTTP auth password
        #[arg(short = 'p', long)]
        pass: Option<String>,
        /// Referer header
        #[arg(long)]
        referer: Option<String>,
        /// Add to this queue instead of downloading now
        #[arg(long)]
        queue: Option<String>,
        /// Add but do not download now ("Download later")
        #[arg(long)]
        later: bool,
    },
    /// List downloads
    List,
    /// Resume a download by id
    Resume {
        id: u64,
        /// Only re-queue; do not start now
        #[arg(long)]
        later: bool,
    },
    /// Mark a download as paused
    Pause { id: u64 },
    /// Remove a download record (and its temp parts)
    Remove {
        id: u64,
        /// Also delete the downloaded file itself
        #[arg(long)]
        purge: bool,
    },
    /// Manage download queues
    Queue {
        #[command(subcommand)]
        cmd: QueueCmd,
    },
    /// Manage the scheduler (start/stop times, post-actions)
    Schedule {
        #[command(subcommand)]
        cmd: ScheduleCmd,
    },
    /// Speed limiter and download limits (quotas)
    Limits {
        #[command(subcommand)]
        cmd: LimitsCmd,
    },
    /// Show or change settings
    Config {
        key: Option<String>,
        value: Option<String>,
    },
    /// Monitor the clipboard for downloadable links
    Monitor {
        /// Match any URL, not just configured file types
        #[arg(long)]
        all: bool,
    },
    /// Delete orphaned part files from interrupted downloads
    Clean,
    /// Launch the GUI (also the default when run with no arguments)
    Gui,
    /// Install/remove the browser integration (Chrome/Edge extension +
    /// native messaging host)
    Browser {
        /// Remove the integration instead of installing it
        #[arg(short, long)]
        uninstall: bool,
    },
    /// (Internal) native messaging host loop, spawned by the browser
    #[command(hide = true)]
    Nm,
}

#[derive(Subcommand)]
enum QueueCmd {
    /// List queues and their pending items
    List,
    /// Create a new queue
    Create { name: String },
    /// Delete a queue (downloads are moved to the main queue)
    Remove { name: String },
    /// Run a queue: process its downloads one at a time
    Run { name: String },
}

#[derive(Subcommand)]
enum ScheduleCmd {
    /// Show schedules
    Show,
    /// Set a schedule for a queue
    Set {
        #[arg(long)]
        queue: String,
        /// Start time HH:MM
        #[arg(long)]
        start: Option<String>,
        /// Stop time HH:MM
        #[arg(long)]
        stop: Option<String>,
        /// Days of week (0=Sunday, comma separated; default: every day)
        #[arg(long)]
        days: Option<String>,
        /// Fire only once
        #[arg(long)]
        once: bool,
        /// Action on finish: exit | none
        #[arg(long)]
        on_finish: Option<String>,
    },
    /// Remove a queue's schedule
    Clear { queue: String },
}

#[derive(Subcommand)]
enum LimitsCmd {
    /// Show current limiter and quota settings
    Show,
    /// Turn on the speed limiter (KB/s)
    On { kb_per_sec: u64 },
    /// Turn off the speed limiter
    Off,
    /// Configure download quota: MB per hour window
    Quota {
        megabytes: u64,
        start_hour: u32,
        end_hour: u32,
    },
    /// Disable the quota
    QuotaOff,
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut unit = 0usize;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

fn parse_hm(s: &str) -> Result<(u32, u32)> {
    let (h, m) = s
        .split_once(':')
        .ok_or_else(|| anyhow!("expected HH:MM, got {s:?}"))?;
    Ok((h.parse()?, m.parse()?))
}

/// Create a new download record for `url`.
pub(crate) fn new_rec(st: &mut State, url: &str, output: Option<PathBuf>, queue: Option<&str>) -> DownloadRec {
    let id = st.next_id();
    let save_path = match output {
        Some(p) => p,
        None => {
            let seg = filetypes::url_last_segment(url);
            if seg.is_empty() {
                PathBuf::new()
            } else {
                state::derive_save_path(&st.settings, url, None, None)
            }
        }
    };
    let temp_dir = st.settings.temp_path.clone();
    let mut save_path = save_path;
    if st.settings.category_dirs {
        if let Some(name) = save_path.file_name().and_then(|n| n.to_str()).map(str::to_string) {
            if let Some(cat) = filetypes::category_of(&name) {
                let dir = PathBuf::from(&st.settings.save_path).join(cat);
                save_path = dir.join(name);
            }
        }
    }
    DownloadRec {
        id,
        url: url.to_string(),
        save_path,
        file_size: 0,
        resumable: false,
        etag: None,
        last_modified: None,
        chunks: vec![],
        status: Status::Queued,
        last_error: None,
        downloaded_secs: 0,
        date_added: state::unix_now() as i64,
        last_try_date: state::unix_now() as i64,
        queue: queue.map(|q| q.to_string()),
        group: None,
        description: String::new(),
        max_connections: None,
        host_connections: None,
        referer: None,
        cookies: None,
        user_agent: None,
        post_action: None,
        url_history: Vec::new(),
        quality: None,
        mux: None,
        mirrors: Vec::new(),
        temp_dir,
        restarted: false,
    }
}

/// Shared helper: download `rec` now with progress UI + Ctrl+C pause.
pub async fn download_rec(
    st: Arc<Mutex<State>>,
    rec: DownloadRec,
    limiter: Arc<limiter::SpeedLimiter>,
    cancel: watch::Receiver<bool>,
    auth: Option<(String, String)>,
    referer: Option<String>,
) -> (DownloadRec, Outcome) {
    let label = rec
        .save_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| rec.url.clone());
    let ui: Arc<dyn ui::ProgressSink> = ui::ProgressUi::new(rec.file_size, &label);
    let start_conn = rec
        .max_connections
        .unwrap_or(st.lock().await.settings.max_connections)
        .clamp(1, 32);
    let rec_cookies = rec.cookies.clone();
    let opts = engine::JobOptions {
        referer,
        auth,
        cookies: rec_cookies,
        conn_limit: Some(Arc::new(AtomicU32::new(start_conn))),
    };
    let result = engine::run(
        st,
        rec.clone(),
        limiter,
        None,
        cancel,
        Some(ui.clone()),
        opts,
    )
    .await;
    ui.finish(&result.1);
    result
}

/// Clipboard entry point: download a URL immediately.
pub async fn cli_download(
    st: Arc<Mutex<State>>,
    url: &str,
    limiter: Arc<limiter::SpeedLimiter>,
    cancel: watch::Receiver<bool>,
) -> (DownloadRec, Outcome) {
    let rec = {
        let mut s = st.lock().await;
        new_rec(&mut s, url, None, None)
    };
    download_rec(st, rec, limiter, cancel, None, None).await
}

/// Print the outcome report.
pub fn report_outcome(rec: &DownloadRec, outcome: &Outcome) {
    match outcome {
        Outcome::Complete { total } => {
            // Evidence 0x6be2e0: "File %s download complete.\n\n
            // Downloaded %s (%I64d Bytes)\nFile saved as %s"
            println!();
            println!("Download complete.");
            println!("Downloaded {} ({total} Bytes).", human_bytes(*total));
            println!("File saved as {}", rec.save_path.display());
        }
        Outcome::Paused => println!("\nDownload paused; resume with `dmultra resume {}`.", rec.id),
        Outcome::Cancelled => println!(
            "\nDownload has been cancelled by user. Progress saved; \
             resume with `dmultra resume {}`.",
            rec.id
        ),
        Outcome::Failed(msg) => println!("\nDownload failed. {msg}"),
    }
}

/// GUI-subsystem release builds start detached from any console (so
/// double-clicking shows no terminal window). Reattach to the parent
/// console before parsing so CLI paths (`dmultra list`, `--help`, errors)
/// still print when launched from a terminal. Skipped when stdout is
/// already a valid (e.g. redirected) handle ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â AttachConsole would
/// clobber it with the console handles.
#[cfg(all(windows, not(debug_assertions)))]
fn attach_parent_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(n_std_handle: u32) -> isize;
        fn SetStdHandle(n_std_handle: u32, handle: isize) -> i32;
        fn AttachConsole(dw_process_id: u32) -> i32;
    }
    const STD_INPUT_HANDLE: u32 = 0xFFFF_FFF6; // (DWORD)-10
    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5; // (DWORD)-11
    const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4; // (DWORD)-12
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        if out != 0 && out != -1 {
            return; // stdout redirected (pipe/file) or already valid
        }
        // Save any redirected stderr/stdin before attaching.
        let err = GetStdHandle(STD_ERROR_HANDLE);
        let input = GetStdHandle(STD_INPUT_HANDLE);
        if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            // AttachConsole replaced all std handles with console
            // handles; put back the redirected ones.
            if err != 0 && err != -1 {
                SetStdHandle(STD_ERROR_HANDLE, err);
            }
            if input != 0 && input != -1 {
                SetStdHandle(STD_INPUT_HANDLE, input);
            }
        }
    }
}

#[cfg(not(all(windows, not(debug_assertions))))]
fn attach_parent_console() {}

fn main() -> Result<()> {
    // The browser launches the native messaging host with the extension
    // origin (Chrome/Chromium: chrome-extension://<id>/) or the add-on id
    // (Firefox) as the sole argument. Route that straight to the host
    // loop before CLI parsing.
    let argv: Vec<String> = std::env::args().collect();
    if argv.len() == 2 {
        let a = &argv[1];
        if a.starts_with("chrome-extension://")
            || a.starts_with("moz-extension://")
            || a == "dmultra@dmultra.local"
            || a == browser::NM_NAME
        {
            return browser::host_loop();
        }
    }
    attach_parent_console();
    let cli = Cli::parse();
    crate::i18n::init("en");

    // One process-wide runtime: the CLI blocks on it; the GUI spawns
    // engine tasks onto it from the event thread.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    let _ = manager::RUNTIME.set(rt);
    let rt = manager::runtime();

    let st = Arc::new(Mutex::new(State::load().context("loading state")?));
    let settings_snapshot = rt.block_on(async { st.lock().await.settings.clone() });
    logfile::init(settings_snapshot.log_to_file);
    logfile::log("APP", &format!("DM Ultra {} starting", env!("CARGO_PKG_VERSION")));
    let limiter = limiter::SpeedLimiter::new(
        settings_snapshot.use_speed_limiter,
        settings_snapshot.speed_limit_bps,
    );
    let quota_cfg = settings_snapshot.quota.clone();

    match cli.cmd {
        None | Some(Cmd::Gui) => {
            // GUI mode: detach from the console so double-clicking does
            // not leave a stray terminal window behind. CLI output is
            // not needed on this path (the GUI has its own status bar).
            #[cfg(windows)]
            unsafe {
                extern "system" {
                    fn FreeConsole() -> i32;
                }
                FreeConsole();
            }
            // The GUI hosts the engine itself (single process).
            let quota = limiter::QuotaTracker::new(quota_cfg);
            let manager = manager::Manager::new(st, limiter, quota);
            gui::launch(manager)
        }
        Some(Cmd::Browser { uninstall }) => {
            if uninstall {
                browser::uninstall()?;
                println!("Browser integration removed.");
            } else {
                let dir = browser::install()?;
                println!("Browser integration installed.");
                println!("Extension folder: {}", dir.display());
                println!(
                    "In Chrome/Edge: enable Developer mode on the Extensions page, \
                     then 'Load unpacked' and pick that folder."
                );
            }
            Ok(())
        }
        Some(Cmd::Nm) => browser::host_loop(),
        Some(cmd) => rt.block_on(async { run_cmd(st, limiter, cmd).await }),
    }
}

async fn run_cmd(st: Arc<Mutex<State>>, limiter: Arc<limiter::SpeedLimiter>, cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Add {
            url,
            output,
            connections,
            user,
            pass,
            referer,
            queue: queue_name,
            later,
        } => {
            let queue_name = queue_name.as_deref();
            let mut rec = {
                let mut s = st.lock().await;
                if let Some(qn) = queue_name {
                    if !s.queues.iter().any(|q| q == qn) {
                        s.queues.push(qn.to_string());
                    }
                }
                new_rec(&mut s, &url, output, queue_name)
            };
            if let Some(c) = connections {
                if (1..=32).contains(&c) {
                    st.lock().await.settings.max_connections = c;
                } else {
                    return Err(anyhow!("connections must be between 1 and 32"));
                }
            }
            // Persist the record before starting (crash safety).
            {
                let mut s = st.lock().await;
                s.downloads.push(rec.clone());
                s.save()?;
            }
            if later || queue_name.is_some() {
                println!("Added download #{} ({})", rec.id, rec.url);
                println!("Queued{}.", if queue_name.is_some() { "" } else { " (download later)" });
                return Ok(());
            }
            let (cancel_tx, cancel_rx) = watch::channel(false);
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                let _ = cancel_tx.send(true);
            });
            let (updated, outcome) = download_rec(
                st.clone(),
                rec.clone(),
                limiter,
                cancel_rx,
                user.clone().zip(pass.clone()),
                referer.clone(),
            )
            .await;
            rec = updated;
            if let Outcome::Complete { .. } = outcome {
                let mut s = st.lock().await;
                if let Some(r) = s.get_mut(rec.id) {
                    r.status = Status::Complete;
                }
                s.save()?;
            }
            report_outcome(&rec, &outcome);
        }
        Cmd::List => {
            let s = st.lock().await;
            println!("{:<4} {:<11} {:<22} {:<22} {:<8} URL", "ID", "STATUS", "DOWNLOADED", "SIZE", "QUEUE");
            for d in &s.downloads {
                let dl = d.total_downloaded();
                let size = if d.file_size > 0 {
                    human_bytes(d.file_size)
                } else {
                    "unknown".into()
                };
                println!(
                    "{:<4} {:<11} {:<22} {:<22} {:<8} {}",
                    d.id,
                    d.status.as_str(),
                    format!("{}/{}", human_bytes(dl), human_bytes(d.file_size)),
                    size,
                    d.queue.as_deref().unwrap_or("-"),
                    d.url
                );
            }
            if s.downloads.is_empty() {
                println!("No downloads.");
            }
        }
        Cmd::Resume { id, later } => {
            let rec = {
                let mut s = st.lock().await;
                let rec = s
                    .get_mut(id)
                    .ok_or_else(|| anyhow!("no download with id {id}"))?;
                if rec.status == Status::Complete {
                    return Err(anyhow!("download #{id} is already complete"));
                }
                rec.status = Status::Queued;
                rec.last_try_date = state::unix_now() as i64;
                let cloned = rec.clone();
                s.save()?;
                cloned
            };
            if later {
                println!("Queued #{id}.");
                return Ok(());
            }
            println!("Resuming #{id} ({})", rec.url);
            let (cancel_tx, cancel_rx) = watch::channel(false);
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                let _ = cancel_tx.send(true);
            });
            let (updated, outcome) =
                download_rec(st.clone(), rec, limiter, cancel_rx, None, None).await;
            report_outcome(&updated, &outcome);
        }
        Cmd::Pause { id } => {
            let mut s = st.lock().await;
            let rec = s.get_mut(id).ok_or_else(|| anyhow!("no download with id {id}"))?;
            rec.status = Status::Paused;
            s.save()?;
            println!("Paused #{id}. (A running foreground download pauses on Ctrl+C.)");
        }
        Cmd::Remove { id, purge } => {
            let (rec, settings) = {
                let mut s = st.lock().await;
                let rec = s.downloads.iter().position(|d| d.id == id).map(|i| s.downloads.remove(i));
                let rec = rec.ok_or_else(|| anyhow!("no download with id {id}"))?;
                let settings = s.settings.clone();
                s.save()?;
                (rec, settings)
            };
            let parts = settings.temp_path.join(format!("d{}", rec.id));
            let _ = std::fs::remove_dir_all(parts);
            if purge {
                let _ = std::fs::remove_file(&rec.save_path);
            }
            println!("Removed #{}.", id);
        }
        Cmd::Queue { cmd } => match cmd {
            QueueCmd::List => {
                let s = st.lock().await;
                for q in &s.queues {
                    let pending = s
                        .downloads
                        .iter()
                        .filter(|d| d.queue.as_deref() == Some(q))
                        .count();
                    println!("- {q} ({pending} item(s))");
                }
            }
            QueueCmd::Create { name } => {
                let mut s = st.lock().await;
                if !s.queues.iter().any(|q| q == &name) {
                    s.queues.push(name.clone());
                }
                s.save()?;
                println!("Created queue {name}.");
            }
            QueueCmd::Remove { name } => {
                let mut s = st.lock().await;
                s.queues.retain(|q| q != &name);
                for d in &mut s.downloads {
                    if d.queue.as_deref() == Some(&name) {
                        d.queue = Some(queue::MAIN_QUEUE.to_string());
                    }
                }
                s.save()?;
                println!("Removed queue {name}; items moved to the main queue.");
            }
            QueueCmd::Run { name } => {
                let (cancel_tx, cancel_rx) = watch::channel(false);
                tokio::spawn(async move {
                    let _ = tokio::signal::ctrl_c().await;
                    let _ = cancel_tx.send(true);
                });
                queue::run_queue(st.clone(), &name, limiter, cancel_rx).await?;
            }
        },
        Cmd::Schedule { cmd } => match cmd {
            ScheduleCmd::Show => {
                let s = st.lock().await;
                if s.schedules.is_empty() {
                    println!("No schedules.");
                }
                for sc in &s.schedules {
                    let start = sc.start.map(|(h, m)| format!("{h:02}:{m:02}")).unwrap_or_else(|| "-".into());
                    let stop = sc.stop.map(|(h, m)| format!("{h:02}:{m:02}")).unwrap_or_else(|| "-".into());
                    let days = if sc.days.is_empty() {
                        "every day".into()
                    } else {
                        sc.days
                            .iter()
                            .map(|d| d.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    };
                    println!(
                        "- queue {:?}: start {start}, stop {stop}, days {days}, once={}, on_finish={}",
                        sc.queue, sc.once, sc.on_finish
                    );
                }
            }
            ScheduleCmd::Set {
                queue: qname,
                start,
                stop,
                days,
                once,
                on_finish,
            } => {
                let mut s = st.lock().await;
                if !s.queues.iter().any(|q| q == &qname) {
                    s.queues.push(qname.clone());
                }
                let sch = queue::Schedule {
                    queue: qname.clone(),
                    start: start.as_deref().map(parse_hm).transpose()?,
                    stop: stop.as_deref().map(parse_hm).transpose()?,
                    days: days
                        .as_deref()
                        .map(|d| d.split(',').filter_map(|x| x.trim().parse().ok()).collect())
                        .unwrap_or_default(),
                    once,
                    on_finish: on_finish.unwrap_or_else(|| "none".into()),
                };
                s.schedules.retain(|x| x.queue != qname);
                s.schedules.push(sch);
                s.save()?;
                println!("Schedule set for queue {qname:?}.");
            }
            ScheduleCmd::Clear { queue: qname } => {
                let mut s = st.lock().await;
                s.schedules.retain(|x| x.queue != qname);
                s.save()?;
                println!("Cleared schedule for {qname:?}.");
            }
        },
        Cmd::Limits { cmd } => match cmd {
            LimitsCmd::Show => {
                let s = st.lock().await;
                let lim = if s.settings.use_speed_limiter {
                    format!("ON ({} KB/s)", s.settings.speed_limit_bps / 1024)
                } else {
                    "OFF".to_string()
                };
                println!("Speed limiter: {lim}");
                let q = &s.settings.quota;
                if q.enabled {
                    println!(
                        "Download limits: {} MB between {:02}:00 and {:02}:00",
                        q.mb, q.start_hour, q.end_hour
                    );
                } else {
                    println!("Download limits: OFF");
                }
            }
            LimitsCmd::On { kb_per_sec } => {
                // Evidence: sls_maxSpeed is stored in KB and converted
                // with *0x400 to bytes/sec (FUN_00538100).
                let mut s = st.lock().await;
                s.settings.use_speed_limiter = true;
                s.settings.speed_limit_bps = kb_per_sec * 1024;
                s.save()?;
                println!("Speed limiter ON at {kb_per_sec} KB/s.");
            }
            LimitsCmd::Off => {
                let mut s = st.lock().await;
                s.settings.use_speed_limiter = false;
                s.save()?;
                // Evidence: both values are deleted when off (FUN_005baad0).
                println!("Speed limiter OFF.");
            }
            LimitsCmd::Quota { megabytes, start_hour, end_hour } => {
                queue::validate_quota(start_hour, end_hour)?;
                let mut s = st.lock().await;
                s.settings.quota = state::Quota {
                    enabled: true,
                    mb: megabytes,
                    start_hour,
                    end_hour,
                };
                s.save()?;
                println!("Download limits: {megabytes} MB between {start_hour:02}:00 and {end_hour:02}:00.");
            }
            LimitsCmd::QuotaOff => {
                let mut s = st.lock().await;
                s.settings.quota.enabled = false;
                s.save()?;
                println!("Download limits OFF.");
            }
        },
        Cmd::Config { key, value } => {
            let mut s = st.lock().await;
            match (key.as_deref(), value) {
                (None, _) => {
                    println!("max-connections  {}", s.settings.max_connections);
                    println!("temp-dir         {}", s.settings.temp_path.display());
                    println!("save-dir         {}", s.settings.save_path.display());
                    println!("file-types       {}", s.settings.file_types);
                    println!("user-agent       {}", s.settings.user_agent);
                    println!("monitor-clipboard {}", s.settings.monitor_clipboard);
                }
                (Some(k), Some(v)) => match k {
                    "max-connections" => {
                        let n: u32 = v.parse()?;
                        (1..=32).contains(&n).then_some(()).ok_or_else(|| anyhow!("1..=32"))?;
                        s.settings.max_connections = n;
                    }
                    "temp-dir" => s.settings.temp_path = PathBuf::from(&v),
                    "save-dir" => s.settings.save_path = PathBuf::from(&v),
                    "file-types" => s.settings.file_types = v,
                    "user-agent" => s.settings.user_agent = v,
                    "monitor-clipboard" => s.settings.monitor_clipboard = v == "1" || v.eq_ignore_ascii_case("true"),
                    _ => return Err(anyhow!("unknown setting {k:?}")),
                },
                (Some(k), None) => return Err(anyhow!("missing value for {k:?}")),
            }
            s.save()?;
        }
        Cmd::Monitor { all } => {
            clipboard::monitor_loop(st.clone(), limiter, all).await?;
        }
        Cmd::Clean => {
            // Offer to delete unreferenced parts.
            let s = st.lock().await;
            let temp = s.settings.temp_path.clone();
            drop(s);
            std::fs::create_dir_all(&temp)?;
            let mut removed = 0;
            for entry in std::fs::read_dir(&temp)? {
                let p = entry?.path();
                if p.is_dir() {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let id: Option<u64> = name.trim_start_matches('d').parse().ok();
                    let in_use = {
                        let s = st.lock().await;
                        id.map(|i| s.get(i).is_some()).unwrap_or(false)
                    };
                    if !in_use {
                        let _ = std::fs::remove_dir_all(&p);
                        removed += 1;
                    }
                }
            }
            println!("Removed {removed} orphaned part folder(s).");
        }
        Cmd::Gui => unreachable!("GUI is launched in main()"),
        Cmd::Browser { .. } | Cmd::Nm => unreachable!("handled in main()"),
    }
    Ok(())
}
