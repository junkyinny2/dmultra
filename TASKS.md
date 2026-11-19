# DM Ultra - Tasks

Rule: tick a box only when its proof command has actually run. `PROJECT_STATUS.md`
holds the state snapshot, `CHANGELOG.md` the release index, `PROGRESS.md` the full
per-version detail. Add new work under **Todo**, newest P0 first.

## Done

### Persistent memory system (the `agents2.md` request) - built 2026-11-18

- [x] Inspected the whole project (25 modules in `src/`, extension, build, deploy, git, runtime state).
- [x] `AGENTS.md` - permanent rules (already existed; kept as-is, only the checkpoint-tool section was added).
- [x] `PROJECT_STATUS.md` - current state, architecture map, error/solution table, exact next steps.
- [x] `TASKS.md` - this file.
- [x] `DECISIONS.md` - locked decisions D-001 .. D-016 with the reason each exists.
- [x] `CHANGELOG.md` - one-line release index + an `Unreleased` section for v0.074.
- [x] `checkpoint.ps1` - the checkpoint system: `status` (resume briefing), `verify`
      (memory files complete and fresh), `save -Note "..."` (stamp a checkpoint).
- [x] Checkpoint discipline recorded in `AGENTS.md`: save at milestones and before compaction.
- [x] Repaired the git worktree pointer that the folder cleanup broke (`PROJECT_STATUS.md` 7.9).
- [ ] Proof not yet run by you: `cd C:\dmultra-base; .\checkpoint.ps1 verify` must exit 0.

### Application work shipped (v0.029 -> v0.074)

- [x] v0.029-0.031 - themed native title bars, centred sub-windows, `PARITY_PLAN.md`,
      repair of the 770 KB of byte noise in `src/state.rs`, `Settings.schema` +
      `State::migrate()`, `DownloadRec` extra fields, `engine::JobOptions` threaded
      through every caller, `src/postdownload.rs` (program / antivirus / power actions
      with cancellable countdown), Options "After download" + "Antivirus" sections.
      Tagged and released as `v0.031`.
- [x] v0.032-0.038 - parity phases 2-8: completion sound setting, per-host connection
      limits, refresh-address settings, JSON import/export of the download list, batch
      URL expansion, browser-captured cookies + per-download User-Agent + referer,
      extension/NMH robustness, DASH `.mpd` + optional FFmpeg remux, RTSP recording +
      MMS HTTP fallback, PAC URL + proxy-from-IE + FFmpeg path UI, i18n loader +
      `locales/en.json`.
- [x] v0.039-0.048 - sub-window position persistence after a real move, video overlay
      driven by the extension heartbeat, overlay menu with title + quality list,
      browser clicks start the download without the File Info window, repaints while
      minimized, main window centring + remembered position/size, launch-at-startup
      (HKCU Run), single-instance handoff, minimize-to-tray modes.
- [x] v0.060-0.067 - instant popout close (wndproc subclass hides on `WM_CLOSE`),
      hook install scans this process' own windows instead of `EnumWindows`, wake while
      any child window is open, self-clocking busy-waker (bounded to one wake in
      flight; measured `WM_CLOSE` in ~190 ms, CPU ~2% of a core, was ~45%).
- [x] v0.072-0.073 - child windows keep working while the main window is parked
      off-screen. Last tag in the repo: `v0.073` (`dff3be5`).
- [x] v0.074 (built + deployed, still uncommitted) - Open buttons use the shell open
      verb instead of `explorer.exe <path>`, `open_action_blocker` + status-bar
      reporting + `OPEN` log lines, `gui::open_action_tests` (3 tests), no `println!`
      left in `state.rs`, panic hook writes `panic.log`, v0.067 waker finished.

## Doing

- Nothing in flight. The memory system is the only new work and it is complete;
  verify it with `.\checkpoint.ps1 verify`.

## Todo

### P0 - make the tree green and land v0.074

- [ ] Kill the version drift so `cargo test` is green (decision needed, `DECISIONS.md` D-014):
      either `EXT_VERSION = env!("CARGO_PKG_VERSION")` in `src/browser.rs:19`, or set
      `Cargo.toml` version to `0.28.2`. Proof: `.\build.ps1 test` -> "29 passed; 0 failed".
- [ ] Fix the three failing extension tests in `browser-extension/content.test.js`:
      "waits for documentElement before observing early-injected frames",
      "labels the detected stream with the playing video resolution" (got `MP4`,
      expected `MP4 - 1920x1080`), "refreshes the native overlay before its stale
      heartbeat timeout". Proof: `node --test browser-extension/content.test.js`.
- [ ] Land v0.074 in git: `git checkout master` (HEAD is detached at `dff3be5`, which
      is already `master`), commit the v0.074 change set, tag `v0.074`, push.
      Proof: `git --no-pager tag --sort=-creatordate` shows `v0.074`.
- [ ] Push what was never published: `origin/master` is still at `9ed1e8a` (v0.040) and
      the remote has no tag past `v0.040`, while this repo holds 53 tags up to `v0.073`.
      Proof: `git --no-pager ls-remote --tags origin` lists a `v0.07x`.
      `dmultra-v0.074-<yyyymmdd>.zip` (older ones live in `C:\dmultra\unused\releases`).
- [ ] Decide what git should do with the untracked `unused/` and `reference/` folders
      and the six deleted `test-*.ps1` scripts (ignore them, or move the scripts back
      into the repo - they are the only GUI end-to-end tests).

### P1 - remaining IDM parity gaps (from `PARITY_PLAN.md` phases 2-8; that file's
checkboxes are stale, so this list is the live version)

- [ ] Notifications: toast parity (click focuses the download, actions in place).
- [ ] "Download complete" dialog parity beyond the current Open / Open Folder / Close.
- [ ] Refresh download address: re-probe changed URLs and keep `url_history` usable.
- [ ] Clipboard parity: per-site exclusion ("do not monitor this site"), paste-and-go.
- [ ] Queue export/import and queue ordering parity.
- [ ] Temp-directory control + "move temp files" maintenance (`engine.rs` part files).
- [ ] FTP: directory listing and active-mode fallback (resume + PASV already work).
- [ ] HTTP: HTTP/2 and keep-alive tuning, `Range` fallbacks, 416 recovery.
- [ ] Extension panel: per-link actions, "Download all links", media list.
- [ ] Video sniffing: MSE segment capture, per-site rules, quality/track selection UI.
- [ ] Native messaging: reconnect, multi-profile support, Firefox manifest install path.
- [ ] Options -> Doctor: browser-integration self-check (extension, NM host, registry).
- [ ] DASH: built-in fMP4 remux without FFmpeg; representation selection UI.
- [ ] Site Grabber: crawl depth, filter set, project save/resume.
- [ ] Scheduler: several schedule windows, queue chaining, richer on-finish actions.
- [ ] Dial-up: connection selection, auto-dial, hang-up after completion.
- [ ] `locales/en.json`: finish the string coverage (loader exists, English only).

### P2 - hygiene

- [ ] Reduce the 80-warning baseline (`cargo check` warnings are stable, not growing).
- [ ] `README.md` still says "RSS feeds" and lists RSS as a feature; there is no RSS
      code in `src/` (AGENTS.md forbids it). Fix the README or re-add RSS deliberately.
- [ ] Re-sync `PARITY_PLAN.md` checkboxes with what `PROGRESS.md` says shipped.
- [ ] Give `checkpoint.ps1` a `test` mode that runs `cargo test` + `node --test` and
      prints one pass/fail line, so a milestone check is one command.
- [ ] Restore the six regression scripts to the repo root (currently in `unused\`):
      `test-minimized-progress.ps1`, `test-minimized-capture.ps1`,
      `test-main-window-placement.ps1`, `test-minimize-modes.ps1`,
      `test-single-instance.ps1`, `test-startup-registry.ps1`.

## How to verify anything in this file

| Suite | Command | Today |
|---|---|---|
| Compile | `cd C:\dmultra-base; .\build.ps1 check` | 0 errors, 80 warnings |
| Rust tests | `cd C:\dmultra-base; .\build.ps1 test` | 28 passed, 1 failed |
| Extension tests | `cd C:\dmultra-base; node --test browser-extension/background.test.js browser-extension/content.test.js` | 9 tests, 6 passed, 3 failed |
| Release build | `cd C:\dmultra-base; .\build.ps1 build --release` | ok (kill any running dmultra first) |
| Memory files | `cd C:\dmultra-base; .\checkpoint.ps1 verify` | must exit 0 |
| GUI end-to-end | the six `test-*.ps1` scripts in `unused\` | not run since the cleanup |

Deploy after a release build: copy `target\release\dmultra.exe` AND
`target\release\dmultra-nmh.exe` to `C:\dmultra` (Defender exclusion lives there).

