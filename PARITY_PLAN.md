# DM Ultra — Internet Download Manager parity plan

Goal: every user-visible IDM setting and feature exists in DM Ultra, implemented
mostly in Rust, with one GitHub release per phase. Reference material lives in
`idm-ghidra-decompile/` (registry layout, per-host `MCN` limits, command-line
tables, proxy settings, speed limiter keys).

Legend: `[x]` done · `[~]` partial · `[ ]` missing.

## Decisions locked in

| Question | Decision |
|---|---|
| Video muxing | Built-in fMP4 remux by default; optional user-supplied FFmpeg for TS/audio (`Settings.ffmpeg_path`); FFmpeg is never bundled |
| `mms://` | Try HTTP/HTTPS fallback, otherwise a clear "not supported" error (no true MMS stack) |
| Languages | English only; localisation loader + `locales/en.json` shipped |
| Power actions | Included (exit / shutdown / restart / sleep / hibernate), cancellable countdown, `DMULTRA_TEST_NO_POWER=1` test guard |
| Releases | One release per phase, starting at `v0.031` |

## Phase map

| Phase | Release | Theme |
|---|---|---|
| 0 | (with 1) | Foundation: schema migration, `JobOptions`, record fields, logging |
| 1 | v0.031 | Post-download actions: program, antivirus, power actions, Options UI |
| 2 | v0.032 | Finishing touches: sounds, notifications, completion window, refresh address, clipboard parity |
| 3 | v0.033 | Data + batch: import/export, batch wildcards, per-host connection limits, temp dir |
| 4 | v0.034 | Connection parity: cookies, referer, UA per download, FTP/proxy refinements |
| 5 | v0.035 | Browser parity: extension panel, site grabber parity, video sniffing, link capture |
| 6 | v0.036 | Video/DASH: fMP4 remux, DASH manifest, FFmpeg hook, quality selection |
| 7 | v0.037 | RTSP / live streams, scheduler parity, dial-up |
| 8 | v0.038 | FTP/proxy/language audit + full parity audit |

---

## Phase 0 — Foundation (shipped inside v0.031)

- [x] `Settings.schema: u32` + `State::migrate()` called from `State::load`
- [x] New `Settings` fields (all `#[serde(default)]`-safe, non-breaking):
  `post_action`, `post_program`, `post_program_args`, `post_wait_queue`,
  `virus_enabled`, `virus_program`, `virus_args`, `power_delay_secs`,
  `ffmpeg_path`, `refresh_address_enabled`, `refresh_address_secs`
- [x] New `DownloadRec` fields (per-record overrides, all defaulted):
  `referer`, `cookies`, `user_agent`, `post_action`, `url_history`,
  `quality`, `mux`, `host_connections`
- [x] `engine::JobOptions` introduced; `Manager::start`, queue processing,
  CLI download and scheduler all thread it through to `engine::run`
- [x] Cleaning: 7 lines in `src/state.rs` had ~770 KB of corrupted byte
  noise embedded inside doc comments; repaired (file 1.7 MB → 22 KB)

## Phase 1 — Post-download actions (v0.031)

- [x] `src/postdownload.rs`
  - Program launch with `%file%`, `%folder%`, `%filename%`, `%url%` tokens
  - Antivirus scan launch (independent of the program hook)
  - Power actions: exit / shutdown / restart / sleep / hibernate
  - Cancellable countdown (`power_pending`, `cancel_power`), default 60 s
  - `DMULTRA_TEST_NO_POWER=1` suppresses real execution but keeps the flow
  - Every outcome written to the diagnostic log (`POSTDL` tag)
- [x] Hook in `Manager::start` and queue processing: engine outcome is captured
  and handed to `postdownload::on_complete`
- [x] Main-window banner with live countdown + Cancel while a power action waits
- [x] Options → Download: new "After download" and "Antivirus" sections
- [x] `Settings` defaults: `post_action = "none"`, `post_wait_queue = true`

## Phase 2 — Finishing touches (v0.032)

- [ ] Completion sound selection (system scheme + custom `.wav` path)
- [ ] Toast/notification parity: click focuses the download, actions in-place
- [ ] "Download complete" dialog parity (open file / open folder / close)
- [ ] Refresh download address (re-probe changed URLs, `url_history`)
- [ ] Clipboard parity: exclusions, "do not monitor this host", paste-and-go
- [ ] Auto-shutdown queue drain ("wait for queue" semantics for power actions)

## Phase 3 — Data, batch, per-host limits (v0.033)

- [ ] Import/export download list (`.dlc`-style text format + JSON)
- [ ] Batch download with wildcards (`file[01-20].zip`, `{a,b}` alternation)
- [ ] Per-host connection limits (`HKCU\...\MCN\<host>` → `host_connections`)
- [ ] Global/per-download temp directory, "move temp files" maintenance
- [ ] Queue export/import parity, queue ordering parity

## Phase 4 — Connection + protocol parity (v0.034)

- [ ] Cookie handoff from the browser extension (`DownloadRec.cookies`)
- [ ] Referer + per-download User-Agent applied by the engine
- [ ] Proxy: PAC URL, proxy-from-IE (`CSystemProxySettings2`), auth persistence
- [ ] FTP parity: resume, directory listing, passive/active switch
- [ ] HTTP/2, keep-alive tuning, `Range` fallbacks, 416 recovery parity

## Phase 5 — Browser parity (v0.035)

- [ ] Extension panel: per-link actions, "Download all links", media list
- [ ] Site Grabber parity: depth, filters, project save/resume
- [ ] Video sniffing parity: MSE segment capture, per-site rules
- [ ] Native messaging robustness: reconnect, multi-profile, Firefox manifest
- [ ] Browser integration self-check in Options → Doctor

## Phase 6 — Video / DASH (v0.036)

- [ ] DASH (`.mpd`) manifest parsing, representation selection, quality list
- [ ] Built-in fMP4 remux (separate audio+video → single MP4)
- [ ] Optional FFmpeg path for TS/audio conversion (`Settings.ffmpeg_path`)
- [ ] Quality/mux fields surfaced in the File Info dialog and progress window
- [ ] Subtitle/audio track selection parity

## Phase 7 — RTSP / live / scheduler / dial-up (v0.037)

- [ ] RTSP (`rtsp://`, `rtsps://`) receive + write (RTP/RTCP, SDP parsing)
- [ ] Live stream recording with stop conditions (size/time/manual)
- [ ] Scheduler parity: multiple windows, queue chaining, on-finish actions
- [ ] Dial-up: connection selection, auto-dial, hang-up after completion
- [ ] `mms://` → HTTP fallback with the documented "not supported" error

## Phase 8 — FTP/proxy/language + audit (v0.038)

- [ ] Localisation loader finalised; `locales/en.json` complete, English only
- [ ] Full settings audit: every IDM registry key mapped or explicitly declined
- [ ] Parity matrix verification pass, screenshots, regression sweep
- [ ] Documentation: user guide + settings reference generated from code

---

## Parity matrix (current)

| IDM feature | DM Ultra today | Target phase |
|---|---|---|
| Multi-connection download | Yes (up to 32) | — |
| Per-host connection limit | No | 3 |
| Speed limiter | Yes | — |
| Scheduler / queues | Yes (basic) | 7 |
| Post-download program | No | 1 |
| Antivirus integration | No | 1 |
| Shutdown/hibernate/sleep/exit | No | 1 |
| Completion sounds | Yes (fixed) | 2 |
| Import/export list | No | 3 |
| Batch wildcards | No | 3 |
| Cookies/referer/UA | Partial (referer only) | 4 |
| PAC / IE proxy | No | 4 |
| FTP download | Yes (basic) | 4 |
| Site Grabber | Yes (basic) | 5 |
| Browser extension | Yes (link + media) | 5 |
| HLS VOD | Yes | — |
| DASH | No | 6 |
| fMP4 remux | No | 6 |
| RTSP | No | 7 |
| MMS | No (fallback only) | 7 |
| Localisation | Loader only | 8 |

## Risks

1. **Power actions are destructive.** Mitigations: 60 s countdown, visible
   cancel, `DMULTRA_TEST_NO_POWER=1`, log every step, never fire from a test run.
2. **State schema drift.** Every new field is `#[serde(default)]`; `schema`
   gates explicit migrations so old `state.json` files keep loading.
3. **Engine is a single 300 KB file.** New behaviour goes into separate modules
   and is wired through `JobOptions`/manager hooks to avoid destabilising it.
4. **FTP/RTSP/DASH are large.** Each gets its own module and its own phase so a
   regression is isolated to one release.
5. **Defender ASR / AppContainer.** All new binaries and helpers stay under
   `C:\dmultra`; no new executables are bundled.

## Verification per release

1. `.\build.ps1 check` — zero errors.
2. `.\build.ps1 build --release` after killing any running `dmultra`.
3. Copy both `dmultra.exe` and `dmultra-nmh.exe` to `C:\dmultra\`.
4. Manual smoke test of the phase's UI, then commit, tag, publish release.
5. Update `PROGRESS.md`.
