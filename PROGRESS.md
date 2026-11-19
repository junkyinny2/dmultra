# DM Ultra — Progress log

## v0.029 (released)
- Native title bars use app theme colours (caption/border/text).
- Confirm Delete window included.

## v0.030 (released, commit a1f7f24, tag v0.030)
- Sub-windows (Options, Add URL, Site Grabber, Scheduler, progress, delete confirm)
  open centred over the main window via `centered_popup_pos`/`POPOUT_CHROME_*`.
- Only user-moved positions persist; Options position now persists.
- Centring maths fixed (double-counted frame chrome removed).

## v0.031 (released)
- [x] Created `PARITY_PLAN.md` with full 8-phase plan and parity matrix.
- [x] Cleaned 7 corrupted lines in `src/state.rs` (770 KB of byte noise).
- [x] Phase 0: `Settings.schema`, `State::migrate()`, new defaulted fields.
- [x] Phase 0: `DownloadRec` fields (referer, cookies, user_agent, post_action,
  url_history, quality, mux, host_connections).
- [x] Phase 0: `engine::JobOptions` introduced; threaded through all callers.
- [x] Phase 1: `src/postdownload.rs` (program, antivirus, power actions,
  cancellable countdown, test guard).
- [x] Phase 1: Manager hooks engine outcome into `postdownload::on_complete`.
- [x] Phase 1: Main-window banner with countdown + Cancel.
- [x] Phase 1: Options → Download: "After download" + "Antivirus" sections.
- [x] Built, deployed, tagged v0.031, published release.

## v0.032 (released)
- Completion sound alias/path setting + per-host connection limits + refresh-address settings + UI.

## v0.033 (released)
- Import/export download list (JSON) via File menu.
- Batch download URL expansion (numeric ranges, alternation) in Add URL.
- Per-host connection limits applied by the manager.

## v0.034 (released)
- HTTP Cookie header from browser-captured cookies.
- Per-download User-Agent override applied to engine + client.
- Referer already threaded (confirmed).

## v0.035 (released)
- Browser parity pass (extension + native messaging host robustness).

## v0.036 (released)
- DASH (.mpd) download support + optional FFmpeg remux hook.

## v0.037 (released)
- RTSP recording (interleaved RTP) + MMS HTTP fallback.

## v0.038 (released)
- FTP/proxy/language audit: PAC URL, proxy-from-IE toggle, FFmpeg path UI,
  i18n loader + locales/en.json.
- Full parity audit pass (see PARITY_PLAN.md matrix).

## v0.039 (built, not published)
- Sub-window positions now persist after a real move (mid-drag save was
  being skipped, so the position was never recorded).
- Options window follows the same rule.

## v0.040 (built, not published)
- Video overlay is wired to the extension heartbeat: the GUI reads
  `pending/overlay.json` every frame and drives overlay::tick().

## v0.041 (built, not published)
- Overlay menu shows the video title plus the resolution/quality list.

## v0.042 (built, not published)
- Browser link clicks no longer open the Add-URL (File Info) window: the
  download starts immediately and only the progress window appears.
  Setting renamed to "Show File Info for browser downloads" (default off).
- Pending browser queue entries older than 60 s are discarded instead of
  being downloaded in a burst on the next launch.
- `dmultra-nmh` now starts DM Ultra when it is not running, so a click is
  not silently lost.

## v0.043 (built, not published)
- Fixed browser clicks still opening the Add-URL window: the old
  `browser_auto_download` flag had the opposite meaning and was carried
  over by a serde alias. The alias is gone and the option is now
  "Ask before downloading browser links" (default off).
- Logged when a capture opens File Info, to make this diagnosable.

## v0.044 (built, not published)
- Browser clicks now work while the main window is minimized: eframe
  0.27 skips scheduled repaints for minimized windows, so the queue was
  only read after a restore. A watcher thread posts a synthetic WM_SIZE,
  which winit/eframe turn into one immediate frame, so the download starts
  and its progress window opens with the main window left minimized.
- Progress/utility windows open centred on the desktop work area when the
  main window is minimized (previously they fell back to 120,120).
- Added `test-minimized-capture.ps1` (end-to-end check for this scenario).

## v0.045 (built, not published)
- Main window opens centred on the desktop work area on first run.
- With "Remember window positions" on, the last position *and* size are
  saved (debounced while dragging) and restored on launch; a saved spot
  that is no longer on any attached monitor falls back to centred.
- Turning the option off clears the stored spot, so re-enabling it does not
  jump to a stale position.
- Added `test-main-window-placement.ps1`.

## v0.046 (built, not published)
- Fixed browser captures started while the main window is minimized: the
  "Download status" window opened at 0% and never updated even though the
  transfer completed. Immediate child viewports are only painted as part of
  the parent window's frame, and eframe skips frames for minimized windows.
  A `busy-waker` thread now posts a synthetic `WM_SIZE` every 150 ms while a
  transfer is active or a child window is open, so progress repaints.
- Added `test-minimized-progress.ps1`: serves a slow 12 MB file locally,
  minimizes the window, queues the URL like the native host does, and checks
  that the progress bar is actually painted to 100% on screen.

## v0.051 (built, not published)
- The frame waker now drives frames whenever a child window is open, even
  when the main window is visible, so closing a dialog takes effect within
  one frame instead of waiting for the next idle pulse.
- Closing a child window (progress/dialog) via its title-bar X or in-app
  Close button now hides it instantly (`ViewportCommand::Visible(false)`)
  instead of waiting for the root frame to garbage-collect it.
- Closing a child window (progress/dialog) via its title-bar X or Close
  button no longer waits for the next 500 ms idle pulse: the root frame is
  woken immediately so the window is destroyed at once.
- Download status (progress) window now remembers the last position it was
  moved to and reuses it for the next download, instead of reverting to
  the centred default for every new download id.
- "Launch at Startup" now works: writes/removes the HKCU Run value
  `DM Ultra`, pointing at `<exe> --background`. The entry is repaired on
  every launch and removed when the option is turned off; it can also be
  driven from the CLI with `dmultra config auto-startup true|false`.
- Browser- and startup-launched instances use `--background`: the main
  window stays hidden, and only the download status window and tray icon
  appear.
- One GUI at a time: a second launch asks the running instance to show its
  window instead of starting a duplicate that would fight over state.json.
- New "Minimize behavior" option: minimize to taskbar, taskbar + tray icon
  (default, IDM-like), or tray only (the window leaves the taskbar). Close
  hides to the tray in the tray modes; a left/double click on the tray icon
  restores the window.
- Windows reports a minimize as a zero-sized resize, which eframe drops, so
  the app never saw the minimize and could not hide the window. The frame
  waker now also wakes the app while tray-only mode is waiting for the
  minimize.
- A hidden window also gets no paint from Windows, so eframe's scheduled
  repaints never run there either. The waker posts a slow (500 ms) synthetic
  resize to a hidden window, and a tray click or a second launch wakes it
  immediately; this is what lets the running instance notice the show
  request and reveal its window.
- Tray icon creation/visibility failures are logged instead of silently
  ignored.
- Added `test-startup-registry.ps1`, `test-minimize-modes.ps1`,
  `test-single-instance.ps1`, and a `-Background` mode for
  `test-minimized-progress.ps1`.
- Removed the temporary `DMULTRA_NO_BUSY_WAKER` diagnostics gate from the
  busy-frame waker.

## v0.064 (built, not published)
- Child windows now close instantly. The title-bar X used to take ~300 ms
  (worst case 3.4 s) before the window disappeared, because the close
  request had to travel through eframe's event loop first. Each popout
  procedure is now subclassed with `SetWindowLongPtrW` and hides itself
  with `ShowWindow(SW_HIDE)` inside `WM_CLOSE` before the message is
  forwarded, so winit still reports `CloseRequested` and everything else
  behaves exactly as before. Measured 30-80 ms after the change.
- Popouts are found with `FindWindowW` by exact title instead of an
  `EnumWindows` sweep. Enumerating cost 100-215 ms because
  `GetWindowTextW` blocks ~100 ms on driver/IME helper windows
  (`NVOGLDC invisible`, `Default IME`). The hook also only runs when
  the set of open popouts changes, with a retry while a window is still
  being created.
- Removed the FRAME/GL/HOOK timing instrumentation added while diagnosing
  the lag, and set `vsync: false` on the native options.

## v0.065 (built, not published)
- Fixed progress popouts going dead when the main window is hidden to the
  tray. The busy flag that drives the synthetic-wake pulse was published
  near the top of `update`, before the popouts were rendered, so a window
  opened in that frame (or right after a transfer finished) could leave the
  pulse off for the frame that mattered. The flag is now published at the
  end of the frame, after every viewport is rendered, and each progress
  popout also requests a 100 ms parent repaint while it is open.
- The busy-waker thread now caches the main window handle and validates it
  with `IsWindow` instead of running a full `EnumWindows` +
  `GetWindowTextW` sweep every 100 ms. The sweep burned ~50% of a core and
  could stall on foreign driver/IME windows, which delayed - or skipped -
  the wake that makes popout clicks run while the main window is hidden.
- Popout hook installation now scans this process' own top-level windows
  and hooks every match, so two progress windows that share a title
  (e.g. two downloads of `blob.bin`) both get the instant title-bar close.
  `FindWindowW` only ever returned the first one.

## v0.066 - fix progress popout freezing when the main window is hidden
- Root cause of "download finishes, then the progress window is stuck
  (cannot move, click, or close)": immediate viewports are painted from the
  parent frame, and on Windows eframe schedules no frames for a hidden
  window. The busy-waker forced frames by posting a synthetic WM_SIZE to
  the main window every 100 ms, but a full frame costs ~200 ms. The wake
  rate therefore outran the paint rate, the Win32 message queue grew
  without bound, and every queued input message - mouse move/click, and
  even WM_CLOSE - was starved for minutes. The window looked frozen and a
  core sat at ~45%.
- The busy-waker is now self-clocking: it re-posts a wake only after the
  app has consumed the previous one (a frame completed, counted by the new
  rame_seq atomic) with a 2-second fallback. Queue depth is bounded to
  one wake in flight; frames stay ~5/s and input latency drops from
  minutes to under 200 ms. Measured: WM_CLOSE handled in ~190 ms, CPU
  ~2% of one core (was ~45%) with the main window hidden.
- Safety: state.rs no longer uses println!. The release binary is a
  Windows GUI-subsystem program with no console, so a stderr write fails
  and println! panics, aborting the whole process; that path now logs
  to the rotating log file. A panic hook writes panic.log under the
  app data dir so a future crash is visible instead of silent.
- Diagnostics: optional [DEBUG_HOOK] / popout-button lines are written
  to the log file (never stderr) so the next report is actionable.

## v0.067 - finish the self-clocking busy-waker (popout close/move fix)
- v0.066 documented a self-clocking busy-waker but the tree only contained
  the GUI half: `gui.rs` passed a new `frame_seq` counter that
  `browser.rs::spawn_busy_waker` did not accept, so the source did not
  compile and the frozen-popout fix was not actually in the binary.
- `spawn_busy_waker` now takes `frame_seq` and keeps at most ONE
  synthetic wake in flight: it posts the next `WM_SIZE` only after the
  GUI completed the frame the previous wake requested (2 s fallback for a
  coalesced wake). This bounds the Win32 queue, so mouse input,
  title-bar drag and `WM_CLOSE` are no longer starved while the main
  window is hidden to the tray or launched with `--background`.
- The idle pulse for a hidden window is unchanged (500 ms) and is not part
  of the busy loop.
- `test-minimized-progress.ps1` now also posts `WM_CLOSE` to the
  status window and asserts it hides within 2 s. Measured on this machine:
  close latency 401 ms with the window hidden and a completed download.

## v0.074 - the status window "Open" button really opens the file
- The "Download status - <file>" popout's **Open** button ran
  `explorer.exe <save_path>`. Explorer never runs a file that way - it just
  opens a window on the folder holding it - so the button looked like a second
  "Open Folder" and never launched the download. It now uses the shell open
  verb (`ShellExecuteW` with `"open"`, declared with plain FFI in `gui.rs`, so
  no new crate is pulled in), which is exactly what a double-click in Explorer
  does.
- The list context menu's "Open File" had the same bug and goes through the new
  path too. "Open Folder", "Open Containing Folder" and File → Open Save Folder
  are untouched: `explorer.exe` is the right tool for a folder.
- Both call sites now share `DmApp::open_download_file`, which reports the
  outcome in the status bar and writes an `OPEN` line to the diagnostic log -
  the popout is its own OS window, so the main window's status bar is the place
  a failure can actually be seen.
- Open is disabled with an explanatory tooltip until the file is on disk
  (`open_action_blocker` covers an empty save path, a file still being written,
  and a folder), so a click during a download can no longer seem to do nothing.
- Tests: new `gui::open_action_tests` (3 tests) covers the openable /
  not-openable decision. `shell_open` itself is never called from a test because
  it would launch the registered application.
- Pre-existing test failure, unrelated to this change and left as-is:
  `browser::tests::generated_extension_manifests_use_the_application_version`.
  `browser.rs` pins `EXT_VERSION = "0.28.2"` while `Cargo.toml` is still
  `version = "0.28.0"`, and the test asserts the two match; the released
  `dmultra-v0.073` snapshot carries the same mismatch. Either bump `Cargo.toml`
  to 0.28.2 or set `EXT_VERSION` to `env!("CARGO_PKG_VERSION")` to make it
  permanent.
- Built with `.\build.ps1 build --release`, both `dmultra.exe` and
  `dmultra-nmh.exe` deployed to `C:\dmultra\`, source + binaries zipped as
  `unused\releases\dmultra-v0.074-<date>.zip`.