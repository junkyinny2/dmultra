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
- [x] Proof run: `cd C:\dmultra-base; .\checkpoint.ps1 verify` -> exit 0
      ("PASS memory system is complete and current", measured 2026-11-19).

### v0.076 follow-up - the three stale GUI scripts + a one-command milestone check (done 2026-11-19)

- [x] `test-minimize-modes.ps1`, `test-minimized-progress.ps1` and
      `test-minimized-capture.ps1` now assert the D-007 contract instead of an
      iconic minimize. Each gained `IsParkedOffScreen` / `IsOutOfSight` /
      `HasTaskbarButton` helpers (built on `GetWindowRect` and
      `WS_EX_TOOLWINDOW`) and prints `iconic= visible= at= taskbar=` on every
      check. Proof run against the deployed v0.076, each in a fresh shell:
      minimize-modes 13 PASS / 0 FAIL (exit 0), minimized-progress 6 PASS /
      0 FAIL (exit 0, bar painted 94 %, popout close latency 2 ms),
      minimized-capture exit 0 reporting `main still out of sight: True`.
- [x] Proven that the scripts were wrong and not the app: before the change the
      same three scripts failed only their iconic checks (modes 6/8/12, progress
      2 of its 6, capture threw at its minimize step) while every real behaviour
      passed - popout opened while out of sight, transfer completed, progress bar
      painted 94 %, popout close latency 3 ms.
- [x] `test-minimize-modes.ps1` now counts the app's own popout windows before
      the minimize and expects a genuine iconic minimize only when none are open,
      because `gui.rs:2348` parks the window in *any* mode while a popout is
      open. That is why one run parked in "taskbar" mode and the next did not: a
      leftover download in `state.json` had its progress window open at startup.
      Proof: `unused\probe-taskbar.ps1` prints the app's titled top-level windows
      before and after the minimize (`Download status - minimized-progress-test.bin`
      was open; with the leftover removed, popouts=0 and the minimize is iconic).
- [x] `checkpoint.ps1` has a `test` mode: cargo test + the extension's
      `node --test` suite + `test-window-drag-profile.ps1`, one pass/fail line per
      suite, exit code = number of red suites, and the dragging median asserted
      against `-DragMaxMs` (default 1.0 ms) so the D-021 fast path cannot regress
      silently. `-SkipGui` drops the desktop-dependent harness, `-StopApp` stops a
      running instance first and restarts it afterwards, and a red suite dumps its
      full output to `unused\checkpoint-<suite>.txt`. Proof run:
      `.\checkpoint.ps1 test -StopApp` -> `ok rust unit tests - 29 passed, 0
      failed`, `ok extension tests - 9 of 9 pass, 0 fail`, `ok gui frame cost -
      34 idle frames median 0.6 ms, 40 drag frames median 0.1 ms`,
      `PASS every suite ran green (14 s)`, exit 0. `.\checkpoint.ps1 test -SkipGui`
      -> same two suites green in 3 s, exit 0.
- [x] No app code changed, so `VERSION` stays 0.076 and no tag or zip is due
      (D-015). `.\checkpoint.ps1 verify` still exits 0.

### v0.077 - the taskbar button of a parked window (done 2026-11-19)

- [x] Found the real cause of "clicking the taskbar icon does nothing": a parked window is
      still `WS_VISIBLE` and non-iconic (that is what keeps its taskbar button and keeps
      eframe painting the popouts), so the shell believes it is open and its click arrives as
      `SC_MINIMIZE`, not `SC_RESTORE` - which the hook intercepted as a minimize and parked
      again. Proof before the change: `unused\probe-uia-taskbar.ps1` invoking the shell's own
      UIA taskbar button left the window at `(-32000,-32000)` with only a `main window parked`
      log line (`unused\fix-winlines.txt`).
- [x] `src/browser.rs` + `src/gui.rs` fix: treat either syscommand (and any activation) while
      parked as "show it", restore it, wake the GUI in the same frame so its safety net stops
      re-parking it. Proof after the change: the same UIA click moved the window to
      `(589,231)` and the next click parked it again (`unused\probe-uia-after2-sum.txt`), and
      `test-minimize-modes.ps1` -> 16 PASS / 0 FAIL, exit 0 against the deployed 0.077,
      including its two new checks (`taskbar click shows the parked window`,
      `clicking the icon again parks it again`). Recorded as D-023.
- [x] Proved `test-single-instance.ps1` was wrong and not the app: `unused\probe-bg.ps1`
      samples the window state over 10 s against both the deployed 0.077 and the saved
      `C:\dmultra\unused\dmultra-v0076.exe`, and both binaries settle into the identical
      `visible=True iconic=False toolwindow=True at=(-32000,-32000)`; the not-visible state
      lasts only 230-350 ms after the window is created, so the old `NOT IsWindowVisible`
      check passed only by racing that transient. The script now asserts the D-007 contract
      (settled out-of-sight, genuine on-screen reveal, one process left, request consumed):
      4 PASS / 0 FAIL exit 0, run twice in a row. Proof:
      `pwsh -NoProfile -File .\test-single-instance.ps1`.
- [x] `test-window-drag-profile.ps1` now runs even when a maximized app owns the foreground
      (VS Code covers the work area here and both `SetForegroundWindow` and
      `SwitchToThisWindow` are refused): it escalates into the topmost band and leaves it
      again before reporting. Proof: it measured 259 drag frames at 0.1 ms median with VS
      Code maximized, exit 0, where the run before the change threw "our window will not come
      to the top of the Z-order".
- [x] Re-measured everything against the deployed 0.077: `.\build.ps1 check` 0 errors /
      80 warnings, `.\checkpoint.ps1 test -StopApp` exit 0 (29 rust, 9 extension, drag median
      0.1 ms), placement 7/7, startup-registry 6/6, single-instance 4/4, minimize-modes 16/16,
      minimized-progress 6/6, minimized-capture exit 0. Release build 0 errors, both exes
      deployed with their `ALL APPLICATION PACKAGES` grant verified afterwards (D-018),
      `VERSION` 0.077, tag `v0.077` and the release zip.

### P0 - tree green, v0.074 and v0.075 landed - done 2026-11-19

- [x] Version drift killed and made permanent: `EXT_VERSION = env!("CARGO_PKG_VERSION")`
      in `src/browser.rs`; `Cargo.toml` 0.28.2 = generated and on-disk extension manifest
      0.28.2. Proof run: `.\build.ps1 test` -> 29 passed; 0 failed (D-014 closed).
- [x] The three failing extension tests fixed in `browser-extension/content.js`: observer
      attached only once `documentElement` exists, stream labels carry the playing
      resolution, 2 s overlay heartbeat added. Proof run: `node --test
      browser-extension/background.test.js browser-extension/content.test.js` -> 9 pass, 0 fail.
- [x] v0.074 landed in git: `git checkout --ignore-other-worktrees master` (the branch was
      locked to the stale `C:/dmultra/unused` worktree - plain checkout refused), code
      commit `1fa7935`, memory/docs commit `385cc3a`, tag `v0.074`,
      `C:\dmultra\unused\releases\dmultra-v0.074-20261119.zip` (105 entries, both exes).
- [x] v0.075 released: extension fixes + `EXT_VERSION` change + `VERSION` 0.075, release
      build 0 errors / 80 warnings, both exes copied to `C:\dmultra` (new extension code
      verified inside the deployed exe), tag + zip.
- [x] `unused/` and `reference/` git-ignored; the six GUI end-to-end `test-*.ps1` scripts
      restored to the repo root and tracked again (D-019).
- [x] Published: `git push origin master` moved `9ed1e8a -> de04574` and the 16 tags that
      were missing (`v0.041` .. `v0.075`) are now on GitHub. Proof run:
      `git --no-pager ls-remote --tags origin` lists `v0.075`.
- [x] Re-created `C:\dmultra\browser-extension` (the folder cleanup had archived it away) by
      running `C:\dmultra\dmultra.exe browser`; the deployed `content.js` is 15,325 B -
      byte-identical to the fixed repo copy - and the generated manifest is 0.28.2.
- [x] `checkpoint.ps1` understands the derived version: `status` resolves
      `EXT_VERSION = env!("CARGO_PKG_VERSION")` to the package version and only complains
      when the two really differ (it used to print the literal macro name and claim the
      three versions disagreed). Proof run: `.\checkpoint.ps1 status` prints
      `version (extension): 0.28.2 (follows CARGO_PKG_VERSION)` and `.\checkpoint.ps1 verify`
      exits 0.

### v0.076 - window dragging and click latency (done 2026-11-19)

- [x] Found the real cause of the choppy dragging: `apply_titlebar_theme` ran
      `EnumWindows` once per frame (~200 ms per sweep on this machine), so every frame
      cost ~200 ms (~5 fps) and ~400 ms while a progress popout was open. Proof run:
      `pwsh -NoProfile -File .\test-window-drag-profile.ps1` against the deployed
      v0.075 - idle avg 207.9 ms (median 200.6), a 675 ms drag produced 5 frames at
      200.5 ms; with a progress window open 402 ms per frame and 3 drag frames.
- [x] Cached the sweep (`ChromeCache`: window list + per-window theme signature + last
      scan, pruned with `IsWindow`) and skipped it, the popout hook sweep and every
      file / registry / clipboard / state read while a window of ours is inside the
      modal move loop. Proof run: the same script on v0.076 - drag 40 frames @ 0.2 ms
      plain and 40 @ 0.6 ms with the progress window open, idle median 0.7 ms, live
      median 0.8 ms. Decision D-021.
- [x] Double-clicking a download row opens its status window at the user's own speed
      (`GetDoubleClickTime`, 500 ms default) instead of egui's 300 ms between-frames
      rule, which a 200-400 ms frame always rejected (D-022).
- [x] `test-window-drag-profile.ps1` is tracked and reliable: it finds the caption with
      `WM_NCHITTEST`, verifies with `WindowFromPoint` + `GetAncestor`, raises our window
      first, escalates to `SwitchToThisWindow` when `SetForegroundWindow` is refused, and
      throws if the window never moved instead of reporting zero drag frames. The earlier "no drag frames"
      runs were the harness, not the app - Excel's window was over the caption and took
      the synthesized click (proved with `unused\probe-hwnd.ps1`).
- [x] Popout close latency in the same run: 288 ms (v0.075) -> 4 ms (v0.076).
- [x] v0.076 released: `VERSION` 0.076, release build 0 errors / 80 warnings,
      `dmultra.exe` 16,652,288 B + `dmultra-nmh.exe` 519,680 B deployed to `C:\dmultra`,
      tagged and zipped with both exes.
- [x] Suites re-run after the change: `.\build.ps1 check` 0 errors / 80 warnings,
      `.\build.ps1 test` 29 passed / 0 failed, `node --test` 9 pass / 0 fail,
      `.\checkpoint.ps1 verify` exit 0.

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

- Nothing in flight.

## Todo

### P0 - verify the shipped fixes end-to-end

- [x] Ran the six GUI end-to-end scripts against the deployed v0.076 (2026-11-19):
      `test-main-window-placement.ps1` 7/7, `test-startup-registry.ps1` 6/6,
      `test-single-instance.ps1` 4/4 (after pointing it at `C:\dmultra\dmultra.exe` -
      it still looked in the repo root), `test-minimize-modes.ps1` 8 pass / 3 fail,
      `test-minimized-progress.ps1` 3 pass / 3 fail, `test-minimized-capture.ps1`
      throws at its minimize step. Proof: `pwsh -NoProfile -File .\<script>`.
- [x] Fix the three scripts that still assert an *iconic* minimize - done, see
      the "v0.076 follow-up" section above: they now assert out-of-sight plus the
      taskbar button, and all three exit 0 against the deployed v0.076.
- [ ] Reload the browser extension (chrome://extensions -> Reload) so the deployed
      `content.js` in `C:\dmultra\browser-extension` is the one actually running, then
      confirm the "Download this video" button disappears within ~6 s of pausing or
      hiding the video - that is the 2 s heartbeat plus `STALE_AFTER`.

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
- [x] Give `checkpoint.ps1` a `test` mode that runs `cargo test` + `node --test` and
      prints one pass/fail line, so a milestone check is one command - done, and it
      runs the drag harness too (`.\checkpoint.ps1 test -StopApp`, exit 0 = green).
- [x] The six regression scripts are back in the repo root and tracked (D-019);
      `test-window-drag-profile.ps1` was added as the seventh in v0.076.

## How to verify anything in this file

| Suite | Command | Today |
|---|---|---|
| Compile | `cd C:\dmultra-base; .\build.ps1 check` | 0 errors, 80 warnings |
| Rust tests | `cd C:\dmultra-base; .\build.ps1 test` | 29 passed, 0 failed (measured 2026-11-19) |
| Extension tests | `cd C:\dmultra-base; node --test browser-extension/background.test.js browser-extension/content.test.js` | 9 pass, 0 fail (measured 2026-11-19) |
| Release build | `cd C:\dmultra-base; .\build.ps1 build --release` | ok (kill any running dmultra first) |
| Memory files | `cd C:\dmultra-base; .\checkpoint.ps1 verify` | must exit 0 |
| Everything at once | `cd C:\dmultra-base; .\checkpoint.ps1 test -StopApp` | exit 0 in 17 s (measured 2026-11-19 on the deployed v0.077): rust 29/29, extension 9/9, 1458 idle frames median 0.3 ms, 673 drag frames median 0.1 ms. `-SkipGui` drops the cursor-dependent harness and finishes in 3 s |
| GUI end-to-end | `pwsh -NoProfile -File .\test-<name>.ps1` (fresh process each: `Add-Type` collides in a reused shell), and stop the running instance before **each** script - they start their own and several leave one running | measured 2026-11-19 on the deployed v0.077: placement 7/7, startup-registry 6/6, single-instance 4/4 (twice in a row), minimize-modes 16/16, minimized-progress 6/6, minimized-capture exit 0 |
| Frame cost by mode | `pwsh -NoProfile -File .\test-window-drag-profile.ps1` (add `-ChildUrl <url>` to drag with a progress window open) | v0.077: 259 drag frames median 0.1 ms (max 1.4), idle median 0.7 ms - and it now works with a maximized app on the foreground because it escalates to the topmost band. v0.076: drag 0.2 ms plain / 0.6 ms with a popout, idle median 0.7 ms, live median 0.8 ms. v0.075 measured 200.5 / 402.7 / 200.6 / 402.2 ms |

Deploy after a release build: copy `target\release\dmultra.exe` AND
`target\release\dmultra-nmh.exe` to `C:\dmultra` (Defender exclusion lives there).

