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
