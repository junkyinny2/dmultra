# DM Ultra - Changelog

One line per release. `PROGRESS.md` holds the full per-version detail (it is the
source of truth); this file is the index and the place to look first for "what changed
in version X". Newest first. Status column: `released` = tagged and published,
`built` = built and deployed to `C:\dmultra` but never tagged, `tagged` = in git only.

## Unreleased

- Nothing pending. The newest release is v0.076.

## Released history

| Version | Status | Headline |
|---|---|---|
| v0.076 | tagged + zip | Window dragging is no longer capped at ~5 fps: the Win32 window sweep is cached and skipped inside the modal move loop. Measured with a progress window open: 402 ms -> 0.6 ms per frame, 3 -> 40 frames during one title-bar drag, popout close latency 288 ms -> 4 ms. |
| v0.075 | tagged + zip | Extension: attach the video observer only once `documentElement` exists, label streams with the playing resolution, send the 2 s overlay heartbeat `overlay.rs` expects; `EXT_VERSION` now comes from `CARGO_PKG_VERSION` (D-014 closed). |
| v0.074 | tagged + zip `385cc3a` | Status window "Open" uses the shell open verb; no stderr writes in a GUI exe; panic hook writes `panic.log`. |
| v0.073 | tagged `dff3be5` | Park the main window off-screen so popouts stay interactive. |
| v0.072 | tagged | Child windows keep working while the main window is out of the way. |
| v0.067 | built | Finish the self-clocking busy-waker (`spawn_busy_waker` takes `frame_seq`); popout close latency 401 ms. |
| v0.066 | tagged | Fix the progress popout freezing when the main window is hidden (unbounded wake queue). |
| v0.065 | built | Popout hooks scan this process' own windows; waker caches the window handle instead of `EnumWindows`. |
| v0.064 | tagged | Instant popout close: wndproc subclass hides on `WM_CLOSE` (30-80 ms, was 300 ms-3.4 s). |
| v0.063, v0.061, v0.060 | tagged | Tray / single-instance / startup work around the v0.051 feature set. |
| v0.051 | built | Wake while any child window is open; minimize-to-tray modes; launch at startup (HKCU Run); one GUI at a time; `--background`. |
| v0.046 | built | Progress repaints while the main window is minimized (synthetic `WM_SIZE`). |
| v0.045 | built | Main window centres on first run; remembers position and size; off-screen spot falls back to centred. |
| v0.044 | built | Browser captures work while minimized; utility windows centre on the work area. |
| v0.043 | built | Fix the inverted "ask before downloading browser links" option and its serde alias. |
| v0.042 | built | Browser clicks start immediately (no File Info window); stale pending entries dropped; NMH starts the app if needed. |
| v0.041 | built | Overlay menu shows the video title plus the resolution/quality list. |
| v0.040 | built | Video overlay driven by the extension heartbeat (`pending\overlay.json`). |
| v0.039 | tagged | Sub-window and Options positions persist after a real move. |
| v0.038 | tagged | FTP/proxy/language audit: PAC URL, proxy-from-IE, FFmpeg path UI, i18n loader + `locales/en.json`; full parity pass. |
| v0.037 | tagged | RTSP recording (interleaved RTP) + MMS HTTP fallback. |
| v0.036 | tagged | DASH (.mpd) download + optional FFmpeg remux. |
| v0.035 | tagged | Browser parity pass: extension + native messaging host robustness. |
| v0.034 | tagged | Cookie header from browser-captured cookies; per-download User-Agent; referer confirmed. |
| v0.033 | tagged | Import/export the download list (JSON); batch URL expansion; per-host connection limits applied. |
| v0.032 | tagged | Completion sound setting; per-host connection limits; refresh-address settings + UI. |
| v0.031 | tagged | Post-download actions (`postdownload.rs`), power actions with countdown, Options UI, `JobOptions`, `state.rs` corruption cleanup. |
| v0.030 | tagged `a1f7f24` | Sub-windows open centred over the main window; only user-moved positions persist. |
| v0.029 | tagged | Native title bars use the app theme colours; Confirm Delete window. |
| v0.001 - v0.028 | tagged | The build-out of the engine, GUI, queues, scheduler, FTP, HLS, clipboard, limiter and browser integration. |

## How to add an entry

When a version is built, add a `### vX.YYY - <headline>` block under **Unreleased**
with what changed, what was tested, and what is still owed (tag, zip, publish). When it
is tagged and published, move the block to the table as one row and set the status.
Then run `checkpoint.ps1 save -Note "vX.YYY <what>"` (from `C:\dmultra-base`).
