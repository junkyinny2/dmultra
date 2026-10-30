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

## v0.035 … v0.038 (not started)
- Phase 5: browser parity (extension panel, Site Grabber depth, video sniffing).
- Phase 6: DASH + built-in fMP4 remux + FFmpeg hook.
- Phase 7: RTSP + scheduler/dial-up + MMS fallback.
- Phase 8: FTP/proxy/language audit + full parity audit.
- See `PARITY_PLAN.md`.
