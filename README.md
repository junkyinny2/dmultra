# DM Ultra

A from-scratch, multi-threaded download manager written in Rust, with
RSS feeds and webhooks. Original code; no license checks of any kind.

## GUI

Run `dmultra.exe` with **no arguments** (e.g. double-click it) to
launch the window:

- Download list with live progress, speed, ETA and per-segment detail
  (double-click a row for the progress popout)
- Whole-row selection, resizable/sortable columns, context menu
  (start, pause, open file/folder, copy URL, remove)
- Toolbar: Add URL, Resume, Pause, Remove, Remove + delete file
- Queue picker with Start/Stop queue processing (sequential, one file
  at a time)
- Speed limiter toggle + KB/s control
- Add URL dialog: address, save-as, max connections, authentication /
  referer, queue and "Download later"
- Progress popout: a separate window that floats above the main one
  (close it with the X; reopen from the list any time)

The GUI process hosts the engine directly: downloads keep running in
the background while you use the window; pause/resume state survives
restarts via `state.json`.

## Features

- **Segmented multi-connection downloads** â€” default 8 connections,
  equal initial split with **dynamic chunk splitting** when connections
  free up early
- **Full resume support** â€” `Range: bytes=N-M` / `N-` requests,
  If-Range validation via ETag/Last-Modified, restart-from-scratch when
  the file changed, chunk state persisted continuously (crash-safe)
- **64-byte cross-chunk data verification** â€” each chunk over-requests
  64 bytes past its end and the next chunk validates against them;
  mismatch restarts the download with a single connection
- **Part-file assembly** â€” per-chunk `.part` files under the temp dir,
  renamed/appended into the destination with 49x200ms sharing-violation
  retries, cross-drive copy fallback
- **Speed limiter** â€” global token bucket (bytes/sec), default
  fallback 10240 B/s
- **Download quotas** â€” MB per hour window; all downloads stop when
  exceeded and auto-resume at window end
- **Queues + scheduler** â€” named queues processed one file at a time,
  start/stop times, days-of-week, one-time schedules, post-finish
  actions (`exit`; shutdown/hibernate are modeled but not
  auto-executed)
- **FTP** â€” hand-written async client (USER/PASS, TYPE I, SIZE, PASV,
  REST resume, RETR) with segmented parallel data connections
- **HTTP(S) auth** â€” Basic, Bearer; proxies incl. SOCKS (via reqwest
  `socks` feature)
- **Clipboard monitoring** â€” polls for links matching the default
  file-type list (`3GP 7Z ... R0* R1* ... ZIP`) and prompts
- **RSS** â€” poll feeds and offer enclosures for download
- **Webhooks** â€” POST notifications on download events

## Browser and clipboard capture

The GUI clipboard watcher can be enabled in Options. It recognizes the
configured download file types and custom clipboard extensions; the optional
extensionless-link setting checks candidates with an HTTP HEAD request. The
watcher continues polling while the GUI is idle and reports clipboard access
errors in the status bar.

The Chrome/Edge browser extension forwards captured links to DM Ultra through
the native messaging host. With **Always auto-download browser links** enabled,
captured links start immediately; otherwise, the separate Add-URL window opens
with the link and its referer.

## Build

```powershell
# requires Rust (stable) and VS Build Tools (link.exe) on PATH, or use
# the included wrapper which wires the installed Build Tools:
powershell -ExecutionPolicy Bypass -File build.ps1 build --release
```

## CLI usage

```text
dmultra                  launch the GUI (same as double-clicking the exe)
dmultra gui              launch the GUI explicitly
dmultra add <URL> [-o FILE] [-c N] [-u USER] [-p PASS] [--referer URL]
     [--queue NAME] [--later]    add and download now (or queue)
dmultra list                        list downloads
dmultra resume <id> [--later]       resume a download
dmultra pause <id>                  mark paused
dmultra remove <id> [--purge]       remove record (+parts, or +file)
dmultra queue list|create|remove|run <name>
dmultra schedule show|set|clear      scheduler windows per queue
dmultra limits show|on KBPS|off|quota MB H1 H2|quota-off
dmultra config [key value]           settings (max-connections, temp-dir,
                                     save-dir, file-types, user-agent, ...)
dmultra monitor [--all]              clipboard watcher
dmultra clean                        delete orphaned part folders
```

Examples:

```powershell
dmultra add https://proof.ovh.net/files/10Mb.dat
dmultra add ftp://user:pass@host/big.iso -c 16
dmultra limits on 500                      # global 500 KB/s
dmultra add <url> --queue "Night queue"
dmultra schedule set --queue "Night queue" --start 02:00 --stop 07:00 --on-finish exit
dmultra queue run "Night queue"
dmultra monitor                            # clipboard capture
```

State lives in `%APPDATA%\dmultra\state.json` (records, settings,
trash). On first launch after upgrading, an existing legacy data
folder is migrated automatically.

## Not implemented

- MMS/RTMP/HLS media protocols, HTTP/2, kernel network monitors, PAC,
  dial-up, and cookie-store import.
- Scheduler post-actions `hangup`/`shutdown`/`hibernate` are accepted
  in the model but never executed automatically (safety).
- GUI credentials entered in Add URL are not persisted across
  restarts; URL-embedded `user:pass@host` credentials survive via the
  record URL.

## Tests

```powershell
powershell -ExecutionPolicy Bypass -File build.ps1 test
```

Covers the split planner, resume cover-checks, the token-bucket
limiter (throttle timing), file-type matching (incl. `R0*`
wildcards), URL segment extraction, clipboard URL extraction, and
list interaction routing (row clicks, column resize). End-to-end GUI
verification hooks: set `DM_GUI_SMOKE=<url>` and launch the GUI â€” it
auto-starts one download (used by the automated checks).
