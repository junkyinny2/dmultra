# DM Ultra - Project status (READ THIS FIRST)

Resume order for a new conversation (do not ask the user to re-explain the project):

1. `AGENTS.md` - permanent rules: build, deploy, git, and the HARD rule that every
   download window is its own OS-level window.
2. `PROJECT_STATUS.md` - this file: exactly where things stand right now.
3. `TASKS.md` - finished / in progress / next, with the command that proves each item.
4. `DECISIONS.md` - why the code is shaped the way it is; do not re-litigate or
   "simplify" a locked decision.
5. `CHANGELOG.md` - one-line release index. `PROGRESS.md` - the full per-version
   history (it is the source of truth for what shipped; `PARITY_PLAN.md` checkboxes
   are stale for phases 2-8).

Last checkpoint: 2026-11-19 20:24 - v0.077 released: a taskbar-button click on a parked main window shows it again (D-023); test-single-instance.ps1 and test-window-drag-profile.ps1 proven wrong rather than the app and fixed; every suite re-measured green on the deployed 0.077; tagged, pushed and zipped [git 88191aa master, dirty=0, VERSION 0.077]
Refresh: `.\checkpoint.ps1 save -Note "what just changed"`
Briefing: `.\checkpoint.ps1 status`
Self-check: `.\checkpoint.ps1 verify` (fails if a memory file is missing, thin or stale)
Suites: `.\checkpoint.ps1 test -StopApp` (cargo + extension + drag harness, one line each,
        exit code = red suites; `-SkipGui` drops the cursor-dependent harness)

## 1. What we are building

DM Ultra: a from-scratch, multi-threaded download manager written in Rust (tokio
engine + eframe/egui GUI) for Windows, being brought to feature parity with Internet
Download Manager (`PARITY_PLAN.md`). Strictly local use: no IDM branding, no RSS,
no plugins, no BitTorrent, no eD2K, no accounts, no API service. Two binaries:

- `dmultra.exe` - Windows GUI-subsystem app (no console; `println!` panics here),
  hosts the download engine in-process. Run with no arguments to open the window.
- `dmultra-nmh.exe` - console-subsystem native messaging host spawned by Chrome/Edge.

## 2. Where everything lives

| Path | What it is |
|---|---|
| `C:\dmultra-base` | The only buildable tree (Rust + egui version). Working dir. |
| `C:\dmultra` | Deploy target: `dmultra.exe` + `dmultra-nmh.exe`. Has a Defender exclusion because ASR blocks new unsigned exes. |
| `C:\dmultra\unused\.git` | The real git directory. The cleanup moved it out of `C:\dmultra`, which broke this worktree - see 7.9. |
| `C:\dmultra\unused\releases` | Historical release zips (`dmultra-v0.064.zip` .. `dmultra-v0.073-20261105.zip`). |
| `C:\dmultra-base\reference` | IDM reverse-engineering material (ghidra output, `desktop_strings*.txt`, `browser-key.xml`). |
| `C:\dmultra-base\unused` | Items moved out of the clean tree, including the six `test-*.ps1` regression scripts. Explained in `UNUSED_FILES_LOG.md`. |
| `%APPDATA%\dmultra` | Runtime state: `state.json`, `logs\dmultra.log` (5 MB rotate), `pending\*.json` (browser queue + `overlay.json` heartbeat), `panic.log`. Migrated automatically from the legacy `idm-rs` folder. |

## 3. Current state (measured on 2026-11-24, not remembered)

| Check | Result |
|---|---|
| `.\build.ps1 check` | `Finished`, 0 errors, 80 warnings (the accepted baseline). |
| `.\build.ps1 test` | **31 passed, 0 failed** - re-run 2026-11-24 after the v0.078 taskbar-refresh test was added. |
| `node --test browser-extension/background.test.js browser-extension/content.test.js` | **9 pass, 0 fail**. |
| Frame cost by mode (`test-window-drag-profile.ps1`, v0.078) | 44 drag frames median 0.1 ms, idle median 0.6 ms, exit 0 - measured with VS Code maximized over the desktop, which the harness handles by escalating to the topmost band (7.23). v0.077 was 259 drag frames at 0.1 ms (max 1.4 ms), idle median 0.7 ms; v0.076 was 0.2 ms plain / 0.6 ms with a popout. |
| GUI end-to-end scripts | minimize-modes **16/16** and the new tray-only transition **8/8** on 2026-11-24 against the released v0.078 binaries (`taskbar=False` in tray-only mode, close keeps the process alive). placement 7/7, startup-registry 6/6, single-instance 4/4, minimized-progress 6/6, minimized-capture exit 0 were last re-measured 2026-11-19 on v0.077. Each runs in a fresh shell (`Add-Type` collides in a reused one) and needs the previous instance stopped **first**. |
| `.\checkpoint.ps1 test -StopApp` (v0.078) | exit 0: `rust unit tests 31 passed, 0 failed`, `extension tests 9 of 9 pass, 0 fail`, `44 drag frames median 0.1 ms`. `-SkipGui` runs the same two non-desktop suites. Measured 2026-11-24. |
| Release binaries | `C:\dmultra\dmultra.exe` 15,635,456 B (SHA256 `37DC3141...`) and `C:\dmultra\dmultra-nmh.exe` 506,880 B (SHA256 `1E6B6AAE...`), built 2026-11-24 from `VERSION` 0.078 and re-verified byte-for-byte against `C:\dmultra\unused\releases\dmultra-v0.078-20261124.zip`. The title bar reads "DM Ultra v0.078", confirmed by the GUI harness matching that title. File/ProductVersion 0.28.2.0 (package version, D-014). Both files and the folder still carry the `ALL APPLICATION PACKAGES` read-and-execute grant (D-018). Older binaries kept for A/B: `C:\dmultra\unused\dmultra-v0077.exe`, `C:\dmultra\unused\dmultra-v0078.exe`. |
| Version strings | `Cargo.toml` 0.28.2 = `EXT_VERSION` (`env!("CARGO_PKG_VERSION")`) = generated and on-disk extension manifest 0.28.2. `VERSION` 0.078 is the release counter (title bar, About, tags) and is deliberately independent of the package version - see D-014. |
| Git | On `master`, reached with `git checkout --ignore-other-worktrees master`: the branch was locked to the stale `C:/dmultra/unused` worktree, which is why HEAD was detached. Tags `v0.074` (`385cc3a`), `v0.075`, `v0.076`, `v0.077`, `v0.078`. `unused/` and `reference/` are ignored (D-019); the eight `test-*.ps1` scripts are tracked. Remote `https://github.com/junkyinny2/dmultra.git`; other branch `tauri-gui` (abandoned non-egui GUI). |
| Release zips | `C:\dmultra\unused\releases\dmultra-v0.074-20261119.zip`, `dmultra-v0.075-20261119.zip`, `dmultra-v0.076-20261119.zip`, `dmultra-v0.077-20261119.zip` and `dmultra-v0.078-20261124.zip` - each the source tree plus both exes under a `dmultra-vX.YYY/` folder (the v0.078 zip has 105 entries). |
| Deployed browser integration | `C:\dmultra\browser-extension\` (`content.js` 15,325 B - the fixed version) plus `dmultra-chrome.zip/.crx` and `dmultra-firefox.xpi`, regenerated on 2026-11-19 by running `C:\dmultra\dmultra.exe browser`. That folder had been archived away by the folder cleanup; the extension has to be reloaded in the browser (chrome://extensions -> Reload) to pick up the new `content.js`. |
| Published | Pushed 2026-11-24: `origin/master` is at the v0.078 commit (`958766f`) and every tag through `v0.078` is on GitHub (earlier pushes carried `v0.041` .. `v0.077`). Proof: `git --no-pager ls-remote --tags origin` lists `v0.078`. |

## 4. Architecture (25 modules, `src/`, line counts as of 2026-11-18)

| Module | Lines | Responsibility |
|---|---:|---|
| `engine.rs` | 1530 | Segmented multi-connection engine: chunk split (default 8, `MIN_PART_SIZE` 1 MiB), dynamic re-split of the largest remaining chunk, 64-byte overlap verification, `Range` resume with `If-Range`, part files + assembly with 49x200 ms sharing retries, `Outcome`/`JobOptions`/`Job`/`Shared`. |
| `gui.rs` | 4931 | eframe app: download list, toolbar, tray, popouts (`render_details`, `render_add_url`, delete confirm, grabber, scheduler), `centered_popup_pos`, `POPOUT_CHROME_W/H`, `frame_seq`, `open_download_file` (shell open verb). |
| `options.rs` | 3368 | Settings dialog as its own OS window (`dm-settings`), theme palette, per-host limits, proxy, post-download + antivirus sections. |
| `main.rs` | 879 | CLI (`add list resume pause remove queue schedule limits config monitor clean gui browser nm`), `--background`, single-instance handoff, panic hook, shared tokio runtime. |
| `browser.rs` | 1491 | Extension source + manifests embedded in the binary, NM host registration (`com.dmultra.nm`), pending queue (`take_pending`, 60 s max age), `spawn_busy_waker` (self-clocking), AppContainer ACL grants. |
| `state.rs` | 704 | `State`/`Settings`/`DownloadRec`/`ChunkRec`/`Webhook`/`Proxies`/`Quota`, `schema` + `migrate()`, atomic save via `state.json.tmp`, corrupt file -> `state.json.corrupt`, `derive_save_path`. |
| `http.rs` | 511 | Hand-rolled HTTP/1.x client, digest (Basic/Bearer) auth, proxies. |
| `manager.rs` | 437 | GUI-facing manager: `start/stop/set_connections/start_queue/reap`, `DownloadStats` (`ProgressSink`), `lock_ui()` (the ONLY safe GUI-side state lock). |
| `overlay.rs` | 913 | Native topmost "Download this video" button over the browser video; verifies foreground window is a browser and the rect still matches; `STALE_AFTER` heartbeat. |
| `limiter.rs` | 180 | Global token-bucket speed limiter (KB/s * 0x400; fallback 10240 B/s) + MB-per-hour quota. |
| `postdownload.rs` | 257 | After-download program (`%file%` `%folder%` `%filename%` `%url%`), antivirus hook, power actions with cancellable countdown, `DMULTRA_TEST_NO_POWER=1`. |
| `queue.rs` | 234 | Named queues + scheduler (start/stop times, days of week, one-shot, on-finish action). |
| `clipboard.rs` | 210 | Clipboard polling + prompt. |
| `startup.rs` | 220 | HKCU Run value `DM Ultra` -> `<exe> --background`, self-repair on launch. |
| `ftp.rs` | 183 | Hand-written async FTP (USER/PASS, TYPE I, SIZE, PASV, REST, RETR). |
| `grabber.rs` | 244 | Site grabber crawl + link filtering. |
| `batch.rs` | 70 | URL expansion: numeric ranges `file[01-20].zip`, `{a,b}` alternation. |
| `hls.rs` | 330 | HLS VOD m3u8 (highest variant, 4 segments at a time). |
| `dash.rs` | 252 | DASH `.mpd` + optional FFmpeg remux. |
| `rtsp.rs` | 340 | RTSP/RTP-over-TCP recording, MMS HTTP fallback. |
| `nmh.rs` | 196 | Native messaging host: read one message, write the pending file, launch DM Ultra if it is not running. |
| `ui.rs` | 208 | `ProgressSink` shared by CLI bars and the GUI. |
| `hooks.rs` | 87 | Task event fan-out: log line + webhook POST. |
| `logfile.rs` | 110 | Rotating log + `panic.log` hook plumbing. |
| `filetypes.rs` | 134 | Default/capture file-type lists. |
| `i18n.rs` | 37 | English-only string loader (`locales/en.json`). |

Data flow (browser click to bytes on disk):

`content.js` intercepts -> `background.js` -> `dmultra-nmh.exe` writes
`%APPDATA%\dmultra\pending\<id>.json` (write-then-rename) and starts the app if
needed -> the GUI reads the pending queue every frame (`browser::take_pending`) ->
`Manager::start` (per-host connection cap, `JobOptions`) -> `engine::run` ->
`DownloadStats` polled each frame -> list rows + the progress popout ->
`postdownload::on_complete` -> `hooks::notify` (log + webhooks) -> `State::save`.

## 5. Just finished (v0.074 + v0.075 - committed, tagged, zipped, deployed 2026-11-19)

- The "Download status" popout **Open** button and the list's "Open File" now use the
  shell open verb (`ShellExecuteW "open"`, plain FFI in `gui.rs`) instead of
  `explorer.exe <path>`, which only ever opened the containing folder. Shared helper
  `DmApp::open_download_file` reports the result in the status bar and logs an `OPEN`
  line; `open_action_blocker` disables Open with a tooltip until the file exists.
  New `gui::open_action_tests` (3 tests).
- `src/state.rs` no longer uses `println!`: this binary has no console, so a stderr
  write fails and `println!` panics, killing the process. A panic hook now writes
  `panic.log` under the app-data dir; diagnostics go to the log file, never stderr.
- v0.067's self-clocking busy-waker was completed (only the GUI half had landed, so
  v0.066 did not actually compile): `spawn_busy_waker` takes `frame_seq` and keeps at
  most one synthetic wake in flight (2 s fallback), so input and `WM_CLOSE` are no
  longer starved. Measured close latency 401 ms.
- v0.075 (browser extension): `content.js` attaches the video `MutationObserver` only
  once `document.documentElement` exists (it runs at `document_start`, and observing a
  null root threw); a stream with no declared quality is labelled with the resolution
  the player is actually playing (`MP4 - 1920x1080`); and while the button is visible
  the extension re-sends the overlay update every 2 s, the cadence `overlay.rs`
  (`STALE_AFTER` 6 s) always assumed but `content.js` never sent. `EXT_VERSION` is now
  `env!("CARGO_PKG_VERSION")`, which closes D-014. Extension 9/9, Rust 29/29.

## 6. Open work (details and priority in `TASKS.md`)

1. The six GUI end-to-end scripts are tracked again but have not been run since the
   folder cleanup.
2. The browser extension has to be reloaded (chrome://extensions -> Reload) before the
   v0.075 `content.js` is the one actually running in the browser.
3. Docs drift: `README.md` still advertises RSS (there is no RSS code in `src/`) and
   `PARITY_PLAN.md` checkboxes lag far behind `PROGRESS.md`.
4. 80 compiler warnings are the standing baseline.

## 7. Errors hit and how they were fixed (do not re-derive these)

| # | Symptom | Cause | Fix (where) |
|---|---|---|---|
| 7.1 | App died silently after a download | `println!` in `state.rs` panics: GUI-subsystem exe has no console, so the stderr write fails | Log to the rotating file + panic hook writing `panic.log` (`logfile.rs`, v0.074) |
| 7.2 | Whole app busy-spun at ~75% CPU and froze when the progress popout tracked a live download | `show_viewport_deferred` child paint scheduling in eframe 0.27 | Use `show_viewport_immediate` and paint the popout inside the root frame (AGENTS.md table) |
| 7.3 | UI thread deadlock when a download and the window both wanted state | tokio `blocking_lock` on the GUI thread | `Manager::lock_ui()` - bounded spin on `try_lock_owned` (`manager.rs:267`) |
| 7.4 | Progress popout stuck at 0% while the main window was minimized | Immediate viewports only paint with the parent frame, and eframe skips frames for minimized/hidden windows | Synthetic `WM_SIZE` wake from `browser::spawn_busy_waker` (500 ms idle pulse when hidden) |
| 7.5 | Popout frozen for minutes after a download finished; one core at ~45% | Waker posted a wake every 100 ms but a frame costs ~200 ms, so the Win32 queue grew unbounded and starved mouse input and `WM_CLOSE` | Self-clocking waker: re-post only after `frame_seq` advanced, 2 s fallback (v0.066/0.067) |
| 7.6 | 100-215 ms stalls installing popout hooks | `EnumWindows` + `GetWindowTextW` blocks ~100 ms on driver/IME windows (`NVOGLDC invisible`, `Default IME`) | Cache the handle and validate with `IsWindow`; scan only this process' own top-level windows (v0.064/0.065) |
| 7.7 | Title-bar X took 300 ms (worst 3.4 s) to close a popout | Close request had to travel through the eframe event loop | Subclass the popout wndproc and `ShowWindow(SW_HIDE)` inside `WM_CLOSE` before forwarding (v0.064) |
| 7.8 | Two progress windows with the same title: only one got instant close | `FindWindowW` returns only the first match | Hook every own-process window that matches the title (v0.065) |
| 7.9 | `git status` said "not a git repository" in `C:\dmultra-base` | The cleanup moved `C:\dmultra\.git` to `C:\dmultra\unused\.git`, so this worktree's `gitdir:` pointer file was gone | Recreated `C:\dmultra-base\.git` containing `gitdir: C:/dmultra/unused/.git/worktrees/dmultra-base` on 2026-11-18; `git status` works again. Revert by deleting that one file if you prefer no git here. |
| 7.10 | Browser clicks opened the Add-URL window even with auto-download on | The old `browser_auto_download` flag had the inverse meaning and a serde alias carried the old value through | Alias removed; option is now "Ask before downloading browser links", default off (v0.043) |
| 7.11 | Sub-window positions never persisted | Position was saved mid-drag and that save was skipped | Save only after a real move (v0.039) |
| 7.12 | `src/state.rs` was 1.7 MB and unparseable by tools | ~770 KB of corrupted byte noise inside doc comments | Repaired 7 lines, file is now 22 KB / 704 lines (v0.031) |
| 7.13 | Minimize to tray never took effect | Windows reports a minimize as a zero-sized resize, which eframe drops, so the app never saw the minimize | The frame waker also wakes the app while tray-only mode waits for the minimize (v0.051) |
| 7.14 | Native messaging silently did nothing | Registry written to the Firefox-style `NativeMessaging\Hosts` path | Must be `HKCU\Software\Google\Chrome\NativeMessagingHosts\com.dmultra.nm` (one word) |
| 7.15 | Browser integration blocked on this machine | Defender ASR + AppContainer for NM hosts; `HKCU\Software\Policies` is ACL-locked | Keep the `C:\dmultra` exclusion and the ALL APPLICATION PACKAGES grants on `C:\dmultra` and the pending dir |
| 7.16 | Dragging the window was choppy, clicks felt ignored, the app ran at ~5 fps | `apply_titlebar_theme` called `EnumWindows` once per frame; one sweep of ~340 top-level windows costs ~200 ms inside win32k on this machine, and Windows runs a title-bar drag in a modal loop on the GUI thread, so the drag could only advance one frame per step (~400 ms/step with a popout open) | Cache the sweep and skip it - plus the hook sweep and every file / registry / clipboard / state read - while `main_window_dragging()` is true (`gui.rs::native_win::ChromeCache`, v0.076, D-021). Measured 402 ms -> 0.6 ms per frame |
| 7.17 | `test-window-drag-profile.ps1` said "no drag frames" and the window never moved | Not an app bug: `GetWindowRect` includes the invisible resize border, and Excel's window was sitting over the caption, so the synthesized click landed in Excel | The harness raises our window first, finds the caption with `WM_NCHITTEST`, verifies with `WindowFromPoint` + `GetAncestor`, escalates to `SwitchToThisWindow` when `SetForegroundWindow` is refused, and throws if the window never moved |
| 7.18 | `test-single-instance.ps1` could not start: "cannot find C:\dmultra-base\dmultra.exe" | The script still pointed at the repo root, a pre-cleanup layout; the repo tree holds no binaries | Point it at the deployed `C:\dmultra\dmultra.exe` (v0.076) - 4/4 PASS after the change |
| 7.19 | `test-minimize-modes.ps1` failed 3 checks, `test-minimized-progress.ps1` 2, `test-minimized-capture.ps1` threw at its minimize step | The scripts asserted `IsIconic`. Since v0.073 (D-007) the tray modes park the window at (-32000,-32000) while keeping it `WS_VISIBLE` (and "tray only" adds `WS_EX_TOOLWINDOW`), so it is never iconic - the app was right, the assertion was stale | Assert "out of sight" (iconic OR hidden OR off-screen) plus the presence of the taskbar button: `IsOutOfSight` / `HasTaskbarButton` in each script. All three now exit 0 |
| 7.20 | After the fix, the "taskbar" check failed on one run and passed on the next with the same binary | Not a flake: `gui.rs:2348` parks the window in *any* mode while a popout is open, and a leftover download in `state.json` had its 'Download status' window open at startup on the first run | The script now counts the app's own titled top-level windows before minimizing and expects a real iconic minimize only when that count is 0 (`unused\probe-taskbar.ps1` proved it) |
| 7.21 | Clicking the taskbar icon of a hidden (parked) main window did nothing | A parked window stays `WS_VISIBLE` and non-iconic - that is what keeps its taskbar button and keeps eframe painting the popouts - so the shell believes it is open and its button click sends `SC_MINIMIZE`, never `SC_RESTORE`; the hook intercepted that as a minimize and parked it again, and even a hook restore was undone by the GUI's own "a hidden window must really be hidden" safety net on the next frame | While parked, `SC_MINIMIZE` **or** `SC_RESTORE` (and any `WM_ACTIVATE`) means "show it": `wakeup::restore_from_park` unparks, sets `RESTORE_REQUESTED` and posts the synthetic resize, and `DmApp::update` consumes `browser::main_window_restore_requested()` in the same frame (v0.077, D-023). Proved through the shell's own UIA taskbar button and asserted by two new checks in `test-minimize-modes.ps1` |
| 7.22 | `test-single-instance.ps1` failed its first check on v0.077 after passing 4/4 on v0.076 | Not an app change. The script asserted `NOT IsWindowVisible` for a `--background` launch - the pre-v0.073 hide semantics - while a background launch parks as `visible=True at=(-32000,-32000)`. `unused\probe-bg.ps1` run against the deployed 0.077 **and** the saved `C:\dmultra\unused\dmultra-v0076.exe` gives both binaries the identical settled state, and the not-visible state exists only 230-350 ms after the window is created (eframe's anti-flash), so the old check passed only when its 150 ms poll happened to land in that transient | Assert the state *settled* (out of sight and holding) instead of one sample; wait for a genuine on-screen reveal, because a parked window is also `WS_VISIBLE`; and wait for the second process to exit instead of sampling it once. 4/4 exit 0, twice in a row |
| 7.23 | `test-window-drag-profile.ps1` threw "our window will not come to the top of the Z-order at its caption" and took `checkpoint.ps1 test` red with it | Not an app bug: VS Code was maximized and owned the caption - `WindowFromPoint` returned its hwnd while our own `WM_NCHITTEST` on the same point answered HTCAPTION - and a process that did not start in the foreground may legitimately be refused, so both `SetForegroundWindow` and `SwitchToThisWindow` failed | The harness escalates a third time: `HWND_TOPMOST` with `SWP_NOMOVE\|NOSIZE\|NOACTIVATE` for the duration of the drag, then `HWND_NOTOPMOST` before anything is reported, so no lasting change is left. Measured 259 drag frames at 0.1 ms median with VS Code still maximized |
| 7.24 | `Tray only` selected in Options still left a DM Ultra taskbar button | `park()` added `WS_EX_TOOLWINDOW` but skipped the `SW_HIDE`/`SW_SHOW` cycle that tells the shell about the style change whenever the window was *already* parked (`&& !already`). A fresh close was always correct; only the mode-change transition failed. `wake_for_popouts` also hard-coded `park(false)` | Removed the guard, added the pure `needs_taskbar_refresh` helper + unit test, and made the popout path honour `MAIN_MODE` (v0.078, D-024). Both exes rebuilt and deployed; `build.ps1 check` 0 errors / 80 warnings |

## 8. Exact next steps

1. Reload the browser extension so the deployed `content.js` in
   `C:\dmultra\browser-extension` is the running one, then confirm the "Download this
   video" button disappears within ~6 s of pausing or hiding the video (the 2 s heartbeat
   plus `STALE_AFTER`). The deployed file is already byte-identical to the repo copy
   (both SHA256 `5E3827C2...FFFC36`, verified 2026-11-19); only the browser's own
   chrome://extensions -> Reload is outstanding, and that needs a human at the browser.
2. Only then pick the next feature from `PARITY_PLAN.md` (the live list is `TASKS.md` P1).
3. Measure, never eyeball, any change to frame production:
   `pwsh -NoProfile -File .\test-window-drag-profile.ps1` (add `-ChildUrl <url>` to drag
   with a progress window open), or `.\checkpoint.ps1 test -StopApp` for the whole tree at
   once. v0.078 must stay at <= 1 ms per dragging frame (D-021) - it measured 0.1 ms on
   2026-11-24.
4. When a GUI script or harness disagrees with the app, prove which of the two is wrong
   before touching code. Four failures in a row were the harness, not the app: 7.19, 7.20,
   7.22 (stale pre-D-007 assertions) and 7.23 (the desktop, not the binary). The template
   is an A/B probe against a saved binary - e.g.
   `pwsh -NoProfile -File unused\probe-bg.ps1 -Exe C:\dmultra\unused\dmultra-v0076.exe` -
   and a probe that drives the real thing (the shell's own UIA taskbar button in
   `unused\probe-uia-taskbar.ps1`) beats a synthesized message.
5. At every milestone run `.\checkpoint.ps1 save -Note "..."`, keep `TASKS.md` and
   `CHANGELOG.md` current, and `.\checkpoint.ps1 verify` must stay at exit 0.

Done since the previous checkpoint: v0.078 shipped - switching to `Tray only` while the main
window was already parked now removes the taskbar button (D-024), popouts honour the
selected minimize mode, and the new `test-tray-only-transition.ps1` proves the reported
transition 8/8 against the released binaries while `test-minimize-modes.ps1` stays 16/16.
Both exes were re-verified against the release zip; no new version was needed for the
post-tag test. Proof commands are in `TASKS.md`.

## 9. Standing rules that override convenience

- Download progress window, Add-URL/File Info, delete confirm, Site Grabber, Scheduler
  and Options are ALWAYS separate OS windows (`show_viewport_immediate`), never
  `egui::Window` panes inside the main window. See AGENTS.md - the user has required
  this repeatedly.
- Build with `.\build.ps1 check` (0 errors), then `.\build.ps1 build --release`; deploy BOTH
  `dmultra.exe` and `dmultra-nmh.exe` to `C:\dmultra`; on a version bump bump `VERSION`,
  commit, tag `vX.YYY` and zip source + binaries with the version in the zip name.
- After any meaningful milestone run `.\checkpoint.ps1 save -Note "..."` and update
  `TASKS.md` + `CHANGELOG.md`. Before context runs out, save a checkpoint even if the
  work is mid-flight.

<!-- CHECKPOINT:BEGIN -->
- 2026-11-19 20:24 | git 88191aa master dirty=0 | v0.077 released: a taskbar-button click on a parked main window shows it again (D-023); test-single-instance.ps1 and test-window-drag-profile.ps1 proven wrong rather than the app and fixed; every suite re-measured green on the deployed 0.077; tagged, pushed and zipped
- 2026-11-19 17:41 | git 98b35cd master dirty=8 | the three stale GUI scripts now assert the D-007 park contract (13/13, 6/6, exit 0) and checkpoint.ps1 gained a test mode (cargo + node + drag green in 14 s, drag median 0.1 ms)
- 2026-11-19 15:41 | git 6162660 master dirty=4 | drag-profile harness hardened (SwitchToThisWindow escalation + throws when the drag never engaged); re-measured on the shipped v0.076: 40 drag frames at 0.2 ms, idle median 0.3 ms
- 2026-11-19 15:26 | git 74ad8ad master dirty=5 | v0.076 released: cached window sweep + drag fast path (402 ms -> 0.6 ms/frame), double-click via GetDoubleClickTime, drag-profile harness proven
- 2026-11-19 10:44 | git 0a3764a master dirty=2 | checkpoint.ps1 now resolves the derived extension version
- 2026-11-19 10:36 | git de04574 master dirty=2 | post-release bookkeeping: publish and browser-integration reinstall recorded
- 2026-11-19 10:26 | git 385cc3a master dirty=8 | v0.075: extension observer attach + resolution labels + 2s overlay heartbeat; EXT_VERSION from CARGO_PKG_VERSION (D-014 closed); v0.074 committed, tagged and zipped
- 2026-11-18 11:18 | git dff3be5 detached dirty=23 | memory system built: PROJECT_STATUS/TASKS/DECISIONS/CHANGELOG + checkpoint.ps1; git worktree pointer repaired
- 2026-11-18 | git dff3be5 (detached, v0.073) | 18 paths dirty | persistent memory system created: PROJECT_STATUS.md, TASKS.md, DECISIONS.md, CHANGELOG.md, checkpoint.ps1; git worktree pointer repaired
<!-- CHECKPOINT:END -->


