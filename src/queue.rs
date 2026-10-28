//! Download queues and the scheduler.
//!
//!  - Queues: named queues persisted in state.json; the "Main download
//!    queue" is the default; queue processing runs one file at a time.
//!  - Scheduler: start/stop windows, day-of-week schedules, one-time
//!    runs, and post-completion actions.
//!  - Download limits (quotas): MB per hours-window with automatic
//!    stop and auto-resume at window end; hours validated to 1..=23.
//!
//! Queue items run strictly sequentially, start/stop windows and quotas
//! are enforced, and only safe post-completion actions ("exit";
//! shutdown/hibernate/hang-up are reported but NOT executed
//! automatically - see README) run.

use crate::engine::{self, Outcome};
use crate::state::{State, Status};
use anyhow::{bail, Context, Result};
use chrono::{Datelike, Timelike};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{watch, Mutex};

pub const MAIN_QUEUE: &str = "Main download queue";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Schedule {
    /// Queue this schedule drives.
    pub queue: String,
    /// Start time (hour, minute) ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â `startTime` / `isStartEnabled`.
    pub start: Option<(u32, u32)>,
    /// Stop time ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â `stopTime` / `isStopEnabled`.
    pub stop: Option<(u32, u32)>,
    /// Days of week when the schedule fires (`startDaysOfWeek`), 0=Sunday.
    pub days: Vec<u32>,
    /// One-time schedule (deleted after firing).
    pub once: bool,
    /// Post-completion action: "none" | "exit"
    /// (Hang-up/shutdown/hibernate are reported but not auto-executed
    /// here for safety - see README).
    pub on_finish: String,
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule {
            queue: MAIN_QUEUE.into(),
            start: None,
            stop: None,
            days: vec![],
            once: false,
            on_finish: "none".into(),
        }
    }
}

impl Schedule {
    fn now_hm() -> (u32, u32) {
        let now = chrono::Local::now();
        (now.time().hour(), now.time().minute())
    }

    fn day_matches(&self) -> bool {
        if self.days.is_empty() {
            return true;
        }
        let dow = chrono::Local::now().weekday().num_days_from_sunday();
        self.days.contains(&dow)
    }

    fn within_start_window(&self, hm: (u32, u32)) -> bool {
        match (self.start, self.stop) {
            (Some(s), Some(e)) => {
                let cur = hm.0 * 60 + hm.1;
                let s = s.0 * 60 + s.1;
                let e = e.0 * 60 + e.1;
                if s <= e {
                    cur >= s && cur < e
                } else {
                    cur >= s || cur < e
                }
            }
            (Some(s), None) => hm >= s,
            (None, _) => true,
        }
    }

    fn past_stop(&self, hm: (u32, u32)) -> bool {
        match self.stop {
            Some(e) => hm >= e,
            None => false,
        }
    }

    /// True when today is a scheduled day and the local clock is inside the
    /// configured start/stop window. Used by the GUI scheduler tick; the
    /// queue worker applies the same rules while it runs.
    pub fn active_now(&self) -> bool {
        self.day_matches() && self.within_start_window(Self::now_hm())
    }
}

/// Wait until the schedule's start window opens (polling every 20s).
async fn wait_for_window(schedule: &Option<Schedule>) -> Result<()> {
    if let Some(s) = schedule {
        if s.start.is_some() && !s.within_start_window(Schedule::now_hm()) {
            println!(
                "Scheduler: waiting for start time {:02}:{:02} for queue {}",
                s.start.unwrap().0,
                s.start.unwrap().1,
                s.queue
            );
            loop {
                if s.day_matches() && s.within_start_window(Schedule::now_hm()) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            }
            println!("Queue processing started");
        }
    }
    Ok(())
}

/// Run a queue: process its downloads one at a time, enforcing the
/// schedule windows, speed limiter and download quota.
pub async fn run_queue(
    state: Arc<Mutex<State>>,
    name: &str,
    limiter: Arc<crate::limiter::SpeedLimiter>,
    cancel: watch::Receiver<bool>,
) -> Result<()> {
    let (quota_cfg, schedule) = {
        let st = state.lock().await;
        let quota = st.settings.quota.clone();
        let sched = st.schedules.iter().find(|s| s.queue == name).cloned();
        (quota, sched)
    };
    let quota = crate::limiter::QuotaTracker::new(quota_cfg);

    wait_for_window(&schedule).await?;

    loop {
        if cancel.borrow().to_owned() {
            println!("Queue processing stopped");
            return Ok(());
        }

        // Quota gate: wait for the window to roll over (auto-resume).
        while quota.tripped() {
            println!(
                "Download limits exceeded! All downloads have been stopped. \
                 They will be resumed automatically at the end of the limits time window."
            );
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let _ = quota.record(0).await; // re-checks window rollover
        }

        // Next pending item.
        let next = {
            let st = state.lock().await;
            st.downloads
                .iter()
                .find(|d| d.status == Status::Queued && d.queue.as_deref() == Some(name))
                .cloned()
        };

        let Some(mut rec) = next else {
            println!("Queue processing finished");
            break;
        };

        // Schedule stop time reached => stop and wait for next window.
        if let Some(s) = &schedule {
            if s.past_stop(Schedule::now_hm()) && !s.within_start_window(Schedule::now_hm()) {
                println!("Queue processing stopped at the scheduled stop time");
                wait_for_window(&schedule).await?;
                continue;
            }
        }

        println!("Queue: starting #{} {}", rec.id, rec.url);
        let (updated, outcome) = engine::run(
            state.clone(),
            rec.clone(),
            limiter.clone(),
            Some(quota.clone()),
            cancel.clone(),
            None,
            None,
            None,
        )
        .await;

        rec = updated;
        match outcome {
            Outcome::Complete { total } => {
                println!(
                    "Download complete. Downloaded {total} Bytes Total\nFile saved as {}",
                    rec.save_path.display()
                );
            }
            Outcome::Paused | Outcome::Cancelled => {
                println!("Queue processing stopped");
                return Ok(());
            }
            Outcome::Failed(msg) => {
                println!("Download failed. {msg}");
            }
        }
    }

    // Post-completion action.
    if let Some(s) = &schedule {
        if s.on_finish == "exit" {
            println!("DM Ultra is going to exit (scheduler on-finish action).");
            let _ = state.lock().await; // final flush happens in main
            std::process::exit(0);
        }
        if s.once {
            let mut st = state.lock().await;
            st.schedules.retain(|x| x.queue != s.queue || !x.once);
            st.save().context("saving schedule state")?;
        }
    }
    Ok(())
}

/// Validate quota hour bounds (hours must be 1..=23).
pub fn validate_quota(start_hour: u32, end_hour: u32) -> Result<()> {
    let span = if start_hour <= end_hour {
        end_hour - start_hour
    } else {
        24 - start_hour + end_hour
    };
    if !(1..=23).contains(&span) {
        bail!("The value of hours in \"Download limits\" must be between 1 and 23.");
    }
    Ok(())
}
