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

## How to add a decision

Append a heading `### D-0NN <title> - locked | open | superseded by D-0MM`, then the
context, the decision and the consequence in that order, then record it with
`checkpoint.ps1 save -Note "decision D-0NN"` (run from `C:\dmultra-base`).

