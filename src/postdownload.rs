//! Post-download actions: launch a program / antivirus scan and the
//! power actions (exit / shutdown / restart / sleep / hibernate) that
//! IDM runs after a download (or queue) finishes.
//!
//! Power actions are gated behind a cancellable countdown and never
//! execute when `DMULTRA_TEST_NO_POWER=1` is set, so automated/CI runs
//! and unit tests cannot shut the machine down.

use crate::engine::Outcome;
use crate::state::{DownloadRec, Settings};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct PowerPending {
    pub action: String,
    pub deadline: Instant,
}

static POWER: OnceLock<Mutex<Option<PowerPending>>> = OnceLock::new();

fn cell() -> &'static Mutex<Option<PowerPending>> {
    POWER.get_or_init(|| Mutex::new(None))
}

pub fn power_pending() -> Option<(String, u64)> {
    let g = cell().lock().ok()?;
    let p = g.as_ref()?;
    let rem = p.deadline.saturating_duration_since(Instant::now()).as_secs();
    Some((p.action.clone(), rem))
}

pub fn cancel_power() {
    if let Ok(mut g) = cell().lock() {
        *g = None;
    }
    crate::logfile::log("POSTDL", "power action cancelled by user");
}

fn is_power_action(a: &str) -> bool {
    matches!(a, "exit" | "shutdown" | "restart" | "sleep" | "hibernate")
}

fn expand_tokens(s: &str, rec: &DownloadRec) -> String {
    let folder = rec
        .save_path
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let filename = rec
        .save_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    s.replace("%file%", &rec.save_path.display().to_string())
        .replace("%folder%", &folder)
        .replace("%filename%", &filename)
        .replace("%url%", &rec.url)
}

fn launch_detached(program: &str, args: &str, rec: &DownloadRec) {
    if program.trim().is_empty() {
        return;
    }
    let prog = expand_tokens(program, rec);
    let argstr = expand_tokens(args, rec);
    let mut cmd = std::process::Command::new(&prog);
    if !argstr.trim().is_empty() {
        // Split on whitespace; quoted args are not supported (IDM-style).
        for a in argstr.split_whitespace() {
            cmd.arg(a);
        }
    }
    match cmd.spawn() {
        Ok(_) => crate::logfile::log("POSTDL", &format!("launched {prog}")),
        Err(e) => crate::logfile::log("POSTDL", &format!("failed to launch {prog}: {e}")),
    }
}

/// Run after a single download completes.
pub fn on_complete(settings: &Settings, rec: &DownloadRec, outcome: &Outcome, from_queue: bool) {
    if !matches!(outcome, Outcome::Complete { .. }) {
        crate::logfile::log(
            "POSTDL",
            &format!("download #{} not complete ({:?}); skipping", rec.id, outcome),
        );
        return;
    }
    crate::logfile::log("POSTDL", &format!("download #{} complete", rec.id));

    // Antivirus scan (independent of the program hook).
    if settings.virus_enabled && !settings.virus_program.trim().is_empty() {
        launch_detached(&settings.virus_program, &settings.virus_args, rec);
    }

    let action = rec
        .post_action
        .clone()
        .unwrap_or_else(|| settings.post_action.clone());

    match action.as_str() {
        "program" => {
            if !settings.post_program.trim().is_empty() {
                launch_detached(&settings.post_program, &settings.post_program_args, rec);
            }
        }
        a if is_power_action(a) => {
            // Defer power actions until the whole queue drains when asked.
            if from_queue && settings.post_wait_queue {
                crate::logfile::log(
                    "POSTDL",
                    "deferring power action until queue finishes",
                );
                return;
            }
            schedule_power(a, settings.power_delay_secs);
        }
        _ => {}
    }
}

/// Called when a queue finishes draining (deferred power actions).
pub fn on_queue_finished(settings: &Settings) {
    let action = settings.post_action.clone();
    if is_power_action(&action) && settings.post_wait_queue {
        schedule_power(&action, settings.power_delay_secs);
    }
    // Program hook for the queue as a whole is intentionally per-file
    // (already handled in on_complete); nothing extra here.
}

pub fn schedule_power(action: &str, delay_secs: u32) {
    let delay = if delay_secs == 0 { 60 } else { delay_secs };
    {
        let mut g = match cell().lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        *g = Some(PowerPending {
            action: action.to_string(),
            deadline: Instant::now() + Duration::from_secs(delay as u64),
        });
    }
    crate::logfile::log(
        "POSTDL",
        &format!("scheduling {action} in {delay}s (cancellable)"),
    );
    let action = action.to_string();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(250));
            match power_pending() {
                None => {
                    crate::logfile::log("POSTDL", "power action cancelled");
                    return;
                }
                Some((a, _)) if a != action => return,
                Some((_, 0)) => break,
                _ => {}
            }
        }
        if std::env::var("DMULTRA_TEST_NO_POWER").ok().as_deref() == Some("1") {
            crate::logfile::log(
                "POSTDL",
                &format!("DMULTRA_TEST_NO_POWER=1: would execute {action}"),
            );
            cancel_power();
            return;
        }
        execute_power(&action);
        cancel_power();
    });
}

fn execute_power(action: &str) {
    crate::logfile::log("POSTDL", &format!("executing power action: {action}"));
    match action {
        "exit" => std::process::exit(0),
        "shutdown" => {
            enable_shutdown_privilege();
            unsafe {
                const EWX_SHUTDOWN: u32 = 0x00000001;
                const EWX_FORCEIFHUNG: u32 = 0x00000010;
                ExitWindowsEx(EWX_SHUTDOWN | EWX_FORCEIFHUNG, 0);
            }
        }
        "restart" => {
            enable_shutdown_privilege();
            unsafe {
                const EWX_REBOOT: u32 = 0x00000002;
                const EWX_FORCEIFHUNG: u32 = 0x00000010;
                ExitWindowsEx(EWX_REBOOT | EWX_FORCEIFHUNG, 0);
            }
        }
        "sleep" => {
            unsafe {
                SetSuspendState(0, 0, 0);
            }
        }
        "hibernate" => {
            unsafe {
                SetSuspendState(1, 0, 0);
            }
        }
        _ => {}
    }
}

fn enable_shutdown_privilege() {
    unsafe {
        let mut token = std::ptr::null_mut();
        const TOKEN_ADJUST_PRIVILEGES: u32 = 0x0020;
        const TOKEN_QUERY: u32 = 0x0008;
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token)
            == 0
        {
            return;
        }
        let name: Vec<u16> = "SeShutdownPrivilege\0".encode_utf16().collect();
        let mut luid: i64 = 0;
        if LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) == 0 {
            return;
        }
        #[repr(C)]
        struct Tp {
            count: u32,
            luid: i64,
            attr: u32,
        }
        let tp = Tp {
            count: 1,
            luid,
            attr: 0x00000002, // SE_PRIVILEGE_ENABLED
        };
        AdjustTokenPrivileges(
            token,
            0,
            &tp as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<Tp>() as u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
    }
}

// FFI
#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> *mut std::ffi::c_void;
}
#[link(name = "advapi32")]
extern "system" {
    fn OpenProcessToken(
        process: *mut std::ffi::c_void,
        access: u32,
        token: *mut *mut std::ffi::c_void,
    ) -> i32;
    fn LookupPrivilegeValueW(system: *const u16, name: *const u16, luid: *mut i64) -> i32;
    fn AdjustTokenPrivileges(
        token: *mut std::ffi::c_void,
        disable_all: i32,
        new_state: *const std::ffi::c_void,
        buf_len: u32,
        prev_state: *mut std::ffi::c_void,
        ret_len: *mut u32,
    ) -> i32;
}
#[link(name = "user32")]
extern "system" {
    fn ExitWindowsEx(flags: u32, reason: u32) -> i32;
}
#[link(name = "powrprof")]
extern "system" {
    fn SetSuspendState(hibernate: u8, force: u8, disable_wake: u8) -> u8;
}
