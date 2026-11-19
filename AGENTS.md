# DM Ultra - Project Rules for Agents


# Persistent AI Memory Rules

You are working on an ongoing software project.
Your conversation history is NOT your permanent memory.

## Before starting work
1. Read PROJECT_STATUS.md.
2. Read TASKS.md.
3. Read DECISIONS.md when relevant.
4. Inspect the existing code before making changes.
5. Continue from the current project state. Do not restart or duplicate completed work.

## During development
1. Keep PROJECT_STATUS.md updated.
2. Record important architectural decisions in DECISIONS.md.
3. Update TASKS.md as tasks are completed.
4. Record important changes in CHANGELOG.md.
5. Do not rely on conversation history to remember important decisions.

## Before context runs out
Create a detailed checkpoint containing:
- What we are building.
- What has been completed.
- What files have been modified.
- What is currently being worked on.
- What remains unfinished.
- Errors encountered and their solutions.
- Important decisions and requirements.
- The exact next steps.

Save the checkpoint to PROJECT_STATUS.md.

Do not wait until the context window is completely full.
Update memory at meaningful milestones and before compaction.

## When continuing in a new conversation
Read the project memory files first.
Verify the current state of the code.
Resume the unfinished work without asking the user
to explain the project again.


## HARD RULE: Download windows are ALWAYS separate OS windows

Always recompile the binary when done, then zip the source code and binaries and put them in files with build version in the filename.

Everytime you increase a version, zip it so we can always revert back changes.

**Never render the download window, the download progress window, or the
Add-URL / "Download File Info" dialog as an egui::Window pane inside the
main window. They must always be their own OS-level popout windows,
separate from the main program window.** The user has required this
repeatedly; do not "simplify" it back into the main window under any
circumstances.

How each one is implemented (keep it this way):

| Window | Mechanism | Where |
|---|---|---|
| Download progress window | `ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(("dl-progress", id)), ...)` - separate OS window, painted inside the root frame, spawned beside the main window, Win32-owned by it so it always floats above it, reopens on demand. NOTE: do NOT switch this to `show_viewport_deferred` - eframe 0.27's deferred child paint scheduling busy-spins (75% CPU) and freezes the whole app when the popout tracks a live download | `render_details` / `render_progress_body` in `src/gui.rs` |
| Add-URL / File Info dialog | `ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("dm-add"), ...)` - separate OS window, spawned beside the main window on first open | `render_add_url` in `src/gui.rs` |
| Settings dialog | `ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("dm-settings"), ...)` | `SettingsDialog::show` in `src/options.rs` |

Additional invariants:

- The progress popout must be positioned next to the main window when it
  opens (never exactly underneath it), and is adopted as a Win32 owned
  window (`popout_win::adopt`, `GWL_HWNDPARENT`) so it never sinks behind
  the main window. The `root_repaint` watchdog keeps root frame processing
  alive while a popout exists (winit starvation workaround). GUI-thread
  state access must use `manager.lock_ui()` (bounded spin on
  `try_lock_owned`), NEVER tokio's `blocking_lock` - it deadlocks the UI
  thread when contended with the async engine.
- The `ViewportClass::Embedded` branches exist only as a technical
  fallback for non-desktop platforms; on Windows they must never run.
  If you ever see download UI rendered inside the main window on Windows,
  that is a bug - fix it immediately.
- Any new dialog that represents a download flow must follow the same
  rule: separate OS window, never an in-app pane.

## Video overlay rules

- The "Download this video" overlay must only appear over a **playing**
  video in a web browser. It is never shown on app launch, over the
  desktop, over the DM Ultra window, or over any non-browser app.
- `pending/overlay.json` is a heartbeat, not a persistent setting. The
  GUI only acts on a changed file, deletes one older than ~5 s, and
  `overlay.rs` hides the button when the heartbeat stops (`STALE_AFTER`).
  The nmh writes it with write-then-rename.
- `overlay.rs` verifies the foreground window belongs to a browser
  process (`BROWSER_EXES`) and still contains the reported video rect
  before showing. Do not relax that check; it is what stops the button
  flashing at stale coordinates.

## Other standing rules

- Build: `.\build.ps1 check` (0 errors; warning baseline), then
  `.\build.ps1 build --release`; deploy by copying
  `target\release\dmultra.exe` AND `target\release\dmultra-nmh.exe` to
  `C:\dmultra\`.
- Version control: this repository is under git (the user requested it).
  `VERSION` holds the current version (`0.001`), and each released
  version is tagged `vX.YYY` (first tag: `v0.001`). Binaries and build
  output stay ignored; commit only source, assets, and docs.
- State lives in `%APPDATA%\dmultra\state.json`; the app migrates the
  legacy `idm-rs` folder automatically - keep that migration.
- No IDM branding anywhere in code, strings, or the binary (see the
  2026-10-20 purge). No RSS, plugins, BitTorrent, eD2K, accounts, or API
  service - the app is strictly local use.
- Browser integration: extension in `browser-extension\`, native
  messaging host is `dmultra-nmh.exe` (console subsystem), registered
  under `HKCU\Software\Google\Chrome\NativeMessagingHosts\com.dmultra.nm`
  (one word - NOT `NativeMessaging\Hosts`, that Firefox-style path is
  wrong for Chromium and will silently break the integration).
- The machine is policy-hardened: Defender ASR blocks new unsigned
  exes (C:\dmultra has a Defender exclusion), and `HKCU\Software\Policies`
  is ACL-locked. Browser NM hosts run inside an AppContainer, so
  `C:\dmultra` and the pending queue carry ALL APPLICATION PACKAGES
  grants - do not remove them.
