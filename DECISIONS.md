# DM Ultra - Decisions

Locked technical decisions and the reason each exists. Each entry ends with what it
implies for future work. If you are about to "simplify" something on this list, stop
and read the consequence line. Status is `locked` unless marked otherwise.

## Interface

### D-001 egui/eframe 0.27 GUI, engine hosted in the same process - locked
The GUI process owns the tokio runtime (`manager::RUNTIME`) and runs downloads as
tasks inside it; there is no background service, no local API, no second process.
Consequence: anything that blocks the GUI thread blocks the app, and pause/resume
state must survive restarts through `state.json` rather than a daemon.

### D-002 Every download flow is its own OS window - locked (user requirement)
Progress popout `ViewportId::from_hash_of(("dl-progress", id))`, Add-URL `dm-add`,
delete confirm `dm-delete-confirm`, Site Grabber `dm-grabber`, Scheduler
`dm-scheduler`, Options `dm-settings` (in `options.rs`). All via
`ctx.show_viewport_immediate`. `ViewportClass::Embedded` branches exist only as a
non-desktop fallback and must never run on Windows.
Consequence: a new download-related dialog is a new viewport, not an `egui::Window`
pane. Rendering download UI inside the main window on Windows is a bug to fix at once.

### D-003 Immediate viewports, never deferred - locked
`show_viewport_deferred` in eframe 0.27 busy-spins (~75% CPU) and freezes the app
when the popout tracks a live download. Immediate children are painted inside the
root frame, which is also why the root frame must be kept awake (D-005).
Consequence: child windows only ever paint and receive input when the root window
produces a frame; any change to frame production must be tested with a minimized or
hidden main window.

### D-004 GUI state access goes through `Manager::lock_ui()` - locked
`manager.rs:267` - bounded spin on `try_lock_owned` returning an `OwnedMutexGuard`.
tokio's `blocking_lock` on the GUI thread deadlocks against the async engine.
Consequence: never call `blocking_lock`, and never hold the state lock across an await
on the GUI thread.

### D-005 Frames are driven by a self-clocking synthetic `WM_SIZE` waker - locked
`browser::spawn_busy_waker` (called from `gui.rs` with the `frame_seq` counter) posts
one synthetic resize at a time and only re-posts after the GUI finished the frame the
previous wake requested, with a 2 s fallback; a hidden window gets a 500 ms idle
pulse. `vsync: false`. A root-repaint watchdog runs while a popout exists.
Consequence: a fixed-rate wake (e.g. 100 ms) is wrong - a frame costs ~200 ms, so a
faster wake rate grows the Win32 queue without bound and starves mouse input and
`WM_CLOSE` (that was the "frozen progress window" bug).

### D-006 Popouts hide themselves inside `WM_CLOSE` - locked
Each popout wndproc is subclassed (`SetWindowLongPtrW`) and calls
`ShowWindow(SW_HIDE)` on `WM_CLOSE` before forwarding, so the title-bar X is honored
in 30-80 ms instead of 300 ms-3.4 s. Hook installation scans this process' own
top-level windows and hooks every title match.
Consequence: do not use `FindWindowW` (it returns only the first match, so two
progress windows with the same file name got one hook) and do not re-enumerate all
windows: `GetWindowTextW` blocks ~100 ms on driver/IME windows.

### D-007 A hidden main window is parked off-screen, not hidden - locked (v0.073)
Keeping the window alive but off-screen lets child windows keep painting and taking
input, which a truly hidden window does not.
Consequence: any "hide the window" work must preserve child-window interactivity.

## Engine and data

### D-008 `engine.rs` stays a single file; new features arrive as new modules - locked
New behaviour is wired in through `engine::JobOptions` and manager hooks rather than
by editing the 300 KB core.
Consequence: a new protocol or post-download behaviour gets its own module plus a
threading path, so a regression is confined to one release.

### D-009 One JSON state file, migrated forward, never repaired in place - locked
`%APPDATA%\dmultra\state.json`, written through `state.json.tmp` then renamed;
`Settings.schema` gates `State::migrate()`; every new field is `#[serde(default)]`;
an unreadable file is moved to `state.json.corrupt` and a fresh state starts; the
legacy `idm-rs` folder is migrated automatically.
Consequence: a field rename or a new required field is a breaking change - add a
schema bump and a migration, or default it. Do not silently delete the corrupt file.

### D-010 Strictly local, no IDM branding - locked
No IDM name or strings anywhere in code or binary; no RSS, plugins, BitTorrent, eD2K,
accounts or API service. Webhooks (`hooks.rs`) are the only outbound notification.
Consequence: `README.md` mentioning RSS is stale (see `TASKS.md` P2), not a feature to
re-add casually.

## Browser and video

### D-011 Browser handoff is a pending-file queue, not IPC - locked
Extension (MV3, Chrome/Edge) -> `dmultra-nmh.exe` (console subsystem) writes
`%APPDATA%\dmultra\pending\<id>.json` write-then-rename and starts DM Ultra if it is
not already running; the GUI drains the queue once per frame with
`browser::take_pending`, discarding entries older than 60 s (`PENDING_MAX_AGE`) so a
click from yesterday does not fire in a burst at startup.
Consequence: the queue must be polled every frame, which is part of why frames must be
produced even while minimized (D-005). The registry key is
`HKCU\Software\Google\Chrome\NativeMessagingHosts\com.dmultra.nm` - one word
`NativeMessagingHosts`; the Firefox-style two-key path silently does nothing.

### D-012 The video overlay is a heartbeat, not a setting - locked
`pending\overlay.json` is rewritten by the host while a video plays; the GUI acts only
on a changed file, deletes one older than ~5 s, and `overlay.rs` hides the button when
the heartbeat stops (`STALE_AFTER`). Before showing, `overlay.rs` verifies the
foreground window belongs to a browser process (`BROWSER_EXES`) and still contains the
reported video rect.
Consequence: never make the overlay persistent or relax the foreground/rect check -
that check is what stops the button flashing at stale coordinates, and the button must
never appear on launch, on the desktop, or over the DM Ultra window.

### D-013 Video and media scope - locked
HLS VOD (`hls.rs`) and DASH (`dash.rs`) are built in; FFmpeg is never bundled, only an
optional user-supplied path (`Settings.ffmpeg_path`) for TS/audio remux; `mms://` uses
an HTTP/HTTPS fallback and otherwise a clear "not supported" error; RTSP is
interleaved RTP over TCP only (`rtsp.rs`); languages are English-only with
`locales/en.json` plus a loader that falls back to the key.
Consequence: do not add a bundled binary (Defender ASR plus the AppContainer make new
executables on this machine expensive) and do not attempt a real MMS stack.

## Release, versioning, environment

### D-014 Extension version follows the package version; `VERSION` is the release counter - locked (resolved 2026-11-19)
`Cargo.toml` (`0.28.2`) is the exe FileVersion/ProductVersion (`build.rs`) and is now
also the single source of `EXT_VERSION` (`src/browser.rs`, `env!("CARGO_PKG_VERSION")`),
which is what gets written into the generated Chromium/Firefox manifests. An installed
extension therefore can never report a version different from the app that installed
it, and `browser::tests::generated_extension_manifests_use_the_application_version`
can no longer drift out of sync on its own.
`VERSION` (`0.075`) stays an independent release counter: it is what the title bar and
About show (`src/gui.rs`) and what the `vX.YYY` tags name. Historically `Cargo.toml`
sat at `0.28.0` across releases v0.031-v0.073 while `VERSION` advanced every release,
which is why the two numbers do not track each other.
Consequence: bump `Cargo.toml` to change the extension/exe version, bump `VERSION` and
tag `vX.YYY` to name a release. Never hardcode `EXT_VERSION` again.

### D-015 Release procedure - locked
Bump `VERSION` -> commit -> tag `vX.YYY` -> zip source + both exes with the version in
the zip name (history lives in `C:\dmultra\unused\releases`). Binaries and build output
stay git-ignored; only source, assets and docs are committed. Deploy means copying
`target\release\dmultra.exe` **and** `target\release\dmultra-nmh.exe` to `C:\dmultra`.
Consequence: a version bump without a tag and a zip is an unfinished release - which is
exactly the state v0.074 is in.

### D-016 Destructive power actions are guarded - locked
`postdownload.rs` runs exit / shutdown / restart / sleep / hibernate behind a visible
cancellable countdown (default 60 s) with a main-window banner; the
`DMULTRA_TEST_NO_POWER=1` environment variable keeps the flow testable without touching
the machine; every step is logged with the `POSTDL` tag.
Consequence: no test or automated run may power-cycle this machine.

### D-017 Memory-file split - locked
`AGENTS.md` = rules. `PROJECT_STATUS.md` = the single resume checkpoint (rewritten at
milestones, with a ledger of past checkpoints at the bottom). `TASKS.md` = the live
work list. `DECISIONS.md` = append-only rationale. `CHANGELOG.md` = release index.
`PROGRESS.md` = the detailed per-version history and the source of truth for what
shipped. `checkpoint.ps1` is the tool that keeps them honest.
Consequence: never rewrite history in `PROGRESS.md` or `DECISIONS.md`; add to them.
Read all of them before touching code, and re-check the code afterwards - the docs can
be stale (the `PARITY_PLAN.md` checkboxes, the README RSS claim and the missing v0.074
tag all are).

### D-018 Treat the machine's hardening as fixed constraints - locked
Defender ASR blocks new unsigned executables (hence the `C:\dmultra` exclusion), native
messaging hosts run inside an AppContainer (so `C:\dmultra` and the pending directory
need ALL APPLICATION PACKAGES grants), and `HKCU\Software\Policies` is ACL-locked so
policy edits from the app fail.
Consequence: do not "clean up" ACL grants, do not move the binaries out of
`C:\dmultra`, and do not plan on setting machine policy from the app.

### D-019 `unused/` and `reference/` are ignored, not deleted - locked (2026-11-19)
`unused\` holds the items moved out of the clean tree (catalogued in
`UNUSED_FILES_LOG.md`) plus scratch logs, and `reference\` holds the
reverse-engineering input (70 files / 1.2 MB, mostly the ghidra output and extracted
strings that `.gitignore` already excluded). Both folders are now ignored. The six GUI
end-to-end `test-*.ps1` scripts were moved back from `unused\` into the repo root,
because they are the only end-to-end GUI tests and a release snapshot must contain
them (the historical release zips all do).
Consequence: an untracked folder here is an archive, not junk - never delete it. Keep
the GUI test scripts tracked, and keep the release zips outside the tree.

### D-020 The extension owns the overlay heartbeat and must tolerate early injection - locked (2026-11-19)
`content.js` runs at `document_start` in every frame, so it can load before
`document.documentElement` exists; the video `MutationObserver` is now attached only
once that root is there (deferred to `DOMContentLoaded`) instead of observing a null
root. While the desktop button is visible the extension re-sends the overlay update on
a fixed 2 s beat, which is the cadence `overlay.rs` (`STALE_AFTER` 6 s) has always
documented. A stream with no declared quality is labelled with the playing element's
intrinsic resolution (`MP4 - 1920x1080`); declared HLS variant qualities always win.
Consequence: the button only disappears on time if the extension keeps sending - do not
fold the 2 s heartbeat into the 3 s scan throttle, and never observe a root that may be
null.

### D-021 The Win32 window sweep is cached, and no file/registry/state work runs inside a modal move loop - locked (2026-11-19)
`apply_titlebar_theme` and the popout hook refresh used to call `EnumWindows` once per frame. `EnumWindows` walks the top-level windows of every process and blocks inside win32k; measured on this machine one sweep of ~340 windows costs ~200 ms. Every frame therefore cost ~200 ms (~5 fps) and ~400 ms while a progress popout was open, and because Windows performs a title-bar drag in a modal message loop on the GUI thread, the drag could only advance once per frame - that was the choppy dragging and the "clicks feel delayed" report.
The sweep result is now cached in `gui.rs::native_win::ChromeCache` (window list, the theme signature last applied to each window, and the time of the last scan). A new sweep runs only when the live count of our windows disagrees with the count the app itself believes is open, the palette signature changed, or 100 ms have passed; dead windows are dropped with `IsWindow`, and the DWM attributes are sent only to windows that have not already received this exact theme. `window_rect` reads the same cache. While `browser::main_window_dragging()` is true (`WM_ENTERSIZEMOVE` .. `WM_EXITSIZEMOVE`/`WM_CAPTURECHANGED`) the frame paints only: the pending browser queue, clipboard watcher, speed sampling, scheduler tick, overlay heartbeat, main-window and popout position saves, the hook sweep and the theme sweep are deferred to the first frame after the drag, and a hook-set change seen during the drag is remembered in `hook_pending`.
Consequence: never put an `EnumWindows`/`FindWindowW` call back on the per-frame path. New per-frame work goes on the non-dragging side of that branch. Any change to frame production is measured with `test-window-drag-profile.ps1` (idle / ui / drag frame cost), not eyeballed.

### D-022 The download list pairs its own double-clicks with `GetDoubleClickTime` - locked (2026-11-19)
egui's `double_clicked()` needs the second press within 300 ms of the first release and times both against frame timestamps, so with 200-400 ms frames a normal Windows double-click (500 ms by default, user-settable) could never register and double-clicking a download silently failed to open its status window. `DmApp::last_row_click` now pairs two clicks on the same row within `native_win::double_click_interval()` (the Control Panel value); egui's own flag still counts when it fires.
Consequence: do not rely on egui's 300 ms rule for any user-facing double-click action while a frame can exceed it; use `double_click_interval()`.

### D-023 A parked window owns its taskbar button, so a click on it must be read as "show it" - locked (2026-11-19)
Since v0.073 (D-007) the hidden state is a *park*: the main window moves to `(-32000,-32000)` but stays `WS_VISIBLE` and non-iconic, because eframe only schedules the root frames that paint the progress popouts. In "taskbar and tray" mode that also keeps its taskbar button - deliberately. The consequence nobody followed: because the window is still visible, the shell believes it is open, so clicking its button sends `WM_SYSCOMMAND / SC_MINIMIZE` and never `SC_RESTORE`. The hook read that as a minimize and parked the window again, so the click appeared to do nothing.
While `PARKED` is set, **either** `SC_MINIMIZE` **or** `SC_RESTORE` (and any `WM_ACTIVATE`) is therefore treated as the user asking for the window back: `wakeup::restore_from_park` unparks it, sets `RESTORE_REQUESTED` and posts the synthetic resize, because a parked window otherwise only receives the slow idle pulse from `spawn_busy_waker` and the GUI would not have noticed for up to half a second. `browser::main_window_restore_requested()` hands the request to `DmApp::update`, which folds it into the existing `show_requested` path so `show_main_window` is cleared in the same frame - otherwise the "a hidden window must really be hidden" safety net parks it straight again, which is the other half of why the click looked dead.
Consequence: never condition the taskbar button on the window being iconic - a parked window never is. Any new hide/park mode must keep the toggle working (one click shows, the next hides) and `test-minimize-modes.ps1` asserts both halves in "taskbar and tray" mode. `unused\probe-uia-taskbar.ps1` proves it through the shell's own UIA button rather than a synthesized message.

### D-024 A taskbar-button style change must refresh the shell even while parked - locked (2026-11-24)
Windows only drops or creates a taskbar button when it sees a visibility change *after*
`WS_EX_TOOLWINDOW` changes. `wakeup::park` used to gate that `SW_HIDE`/`SW_SHOW` cycle on
`!already`, so changing Options from "Taskbar and tray" to "Tray only" while the main window
was already parked set the style bit but left the old button on screen - exactly the user's
"it still shows on taskbar" report. The cycle now runs whenever the desired style differs
from the current one, regardless of the parked flag, and `needs_taskbar_refresh` is unit
tested. `wakeup::wake_for_popouts` also parks with `MAIN_MODE == 2` instead of a hard-coded
`false`, because that path must not silently opt a tray-only user back into the taskbar.
Consequence: any future hide/park mode that changes extended window styles must notify the
shell; "already parked" is never a reason to skip the refresh. D-007's off-screen park and
D-023's taskbar-button toggle remain unchanged.

## How to add a decision

Append a heading `### D-0NN <title> - locked | open | superseded by D-0MM`, then the
context, the decision and the consequence in that order, then record it with
`checkpoint.ps1 save -Note "decision D-0NN"` (run from `C:\dmultra-base`).

