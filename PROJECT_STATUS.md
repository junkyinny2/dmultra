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

Last checkpoint: 2026-11-18 11:18 - memory system built: PROJECT_STATUS/TASKS/DECISIONS/CHANGELOG + checkpoint.ps1; git worktree pointer repaired [git dff3be5 detached, dirty=23, VERSION 0.074]
Refresh: `.\checkpoint.ps1 save -Note "what just changed"`
Briefing: `.\checkpoint.ps1 status`
Self-check: `.\checkpoint.ps1 verify` (fails if a memory file is missing, thin or stale)

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

## 3. Current state (measured on 2026-11-18, not remembered)

| Check | Result |
|---|---|
| `.\build.ps1 check` | `Finished`, 0 errors, 80 warnings (the accepted baseline). |
| `.\build.ps1 test` | 28 passed, **1 failed**: `browser::tests::generated_extension_manifests_use_the_application_version`, `src/browser.rs:1613` (0.28.2 vs 0.28.0). |
| `node --test browser-extension/background.test.js browser-extension/content.test.js` | 9 tests, 6 passed, **3 failed**, all three in `content.test.js`. |
| Release binaries | `target\release\dmultra.exe` 15,606,784 B, `target\release\dmultra-nmh.exe` 506,880 B (built 2026-11-18 09:37-09:40). Both already copied to `C:\dmultra`. |
| Version strings disagree | `VERSION` = 0.074 (title bar + About, `src/gui.rs:32`), `Cargo.toml` = 0.28.0 (exe FileVersion + the failing test), `EXT_VERSION` (`src/browser.rs:19`) = 0.28.2 (generated extension manifests). |
| Git | Worktree in **detached HEAD** at `dff3be5` = tag `v0.073` = `master`; remote `https://github.com/junkyinny2/dmultra.git`; other branch `tauri-gui` (the abandoned non-egui GUI). |
| Uncommitted (v0.074 work) | `M VERSION`, `M PROGRESS.md`, `M AGENTS.md`, `M src/gui.rs` (+212), `M src/browser.rs` (+46), `M src/logfile.rs` (+50). The six `test-*.ps1`, `dmultra-chrome.zip` and `DM_Ultra (1).ico` read as deleted (they were moved to `unused\`). Untracked: `UNUSED_FILES_LOG.md`, `agents2.md`, `reference/`, `unused/`. |
| Tag and zip | `v0.074` is **not tagged** and no `dmultra-v0.074-*.zip` exists, although AGENTS.md requires both on a version bump. Newest tag: `v0.073`. |

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

## 5. Just finished (v0.074 - built and deployed, but NOT committed/tagged/zipped)

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

## 6. Open work (details and priority in `TASKS.md`)

1. Two test suites are red (Rust version assertion; three `content.test.js` tests).
2. v0.074 is uncommitted, untagged and unzipped; HEAD is detached, so a commit here
   would land on a dangling HEAD instead of `master`.
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

## 8. Exact next steps

1. `cd C:\dmultra-base; .\checkpoint.ps1 verify` then `.\checkpoint.ps1 status` - the
   memory system must stay green.
2. Decide the version scheme (see `DECISIONS.md` D-014) so the Rust test goes green:
   either set `EXT_VERSION = env!("CARGO_PKG_VERSION")` in `src/browser.rs:19`, or bump
   `Cargo.toml` to `0.28.2`. Then `.\build.ps1 test` must report 29 passed, 0 failed.
3. Fix the three failing `content.test.js` tests (early-injection wait, stream label
   resolution, overlay heartbeat) - `node --test browser-extension/content.test.js`.
4. Bring v0.074 into git properly: HEAD is detached at `dff3be5`, so first
   `git checkout master` (it is already at `dff3be5`), then commit the v0.074 change
   set, tag `v0.074`, and create the missing `dmultra-v0.074-<date>.zip` (AGENTS.md
   hard rule). `unused/` and `reference/` are currently untracked - decide whether to
   commit or ignore them before the first commit.
5. Only then pick the next feature from `PARITY_PLAN.md` (see `TASKS.md` Todo).

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
- 2026-11-18 11:18 | git dff3be5 detached dirty=23 | memory system built: PROJECT_STATUS/TASKS/DECISIONS/CHANGELOG + checkpoint.ps1; git worktree pointer repaired
- 2026-11-18 | git dff3be5 (detached, v0.073) | 18 paths dirty | persistent memory system created: PROJECT_STATUS.md, TASKS.md, DECISIONS.md, CHANGELOG.md, checkpoint.ps1; git worktree pointer repaired
<!-- CHECKPOINT:END -->


