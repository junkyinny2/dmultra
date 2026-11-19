# Unused Files Log

This log documents the cleanup performed to keep **`C:\dmultra-base`** as the clean Rust **egui** version of DM Ultra (the only version to compile/run), while preserving reference materials for future feature work.

## Clean project (kept in place): `C:\dmultra-base`

The following items remain in `C:\dmultra-base` because they are needed to compile/run the Rust egui app or are kept as in-project reference/docs/assets:

| Item | Purpose |
|---|---|
| `Cargo.toml`, `Cargo.lock` | Rust package manifest and locked dependency versions. |
| `build.rs`, `build.ps1`, `resource.rc` | Build scripts/resources for Windows builds and icon/version embedding. |
| `VERSION` | Version string embedded/used by the application. |
| `src/` | Rust source code (egui GUI, engine, networking, native messaging host, etc.). |
| `assets/` | Application assets (icons/SVGs/ICO/JPG) used by the GUI/build. |
| `browser-extension/` | Browser extension sources/manifests embedded/used by browser integration code. |
| `locales/` | Localization resources. |
| `.gitignore` | Git ignore rules for the project. |
| `README.md`, `AGENTS.md`, `PROGRESS.md`, `PARITY_PLAN.md`, `guicorrect.md` | Project documentation/notes kept for reference and future feature work. |
| `DM_Ultra*.png/.ico`, `Glossy*.png`, `icon.jpg` | Graphic/image assets kept for reference/branding. |
| `reference/` | Reference materials copied in for future work (see below). |
| `unused/` | Items moved out of the clean project as not needed to compile/run (see log). |

## Reference materials added to `C:\dmultra-base\reference`

| Item | Description |
|---|---|
| `reference/idm-ghidra-decompile/` | Ghidra decompilation/reverse-engineering outputs copied from `C:\dmultra` for future feature parity work. |
| `reference/desktop_strings.txt`, `reference/desktop_strings_2.txt` | Extracted desktop UI/string references from reverse engineering. |
| `reference/browser-key.xml` | Browser/key-related reverse-engineering reference data. |
| `reference/from-dmultra-root/` | `.md` docs/notes from `C:\dmultra` root that differed from the base versions (preserved for comparison/reference). |

## Unused items moved from `C:\dmultra-base`

Destination: `C:\dmultra-base\unused\`

| Item | Type | Description |
|---|---|---|
| `.git` | file | Git metadata pointer file (gitdir reference); not required to compile or run the Rust egui app. |
| `build-before.log` | file | Previous build log output; transient artifact not needed to compile or run. |
| `dmultra-chrome.zip` | file | Prebuilt Chrome browser-extension package; can be regenerated from the browser-extension/ source directory. |
| `test-main-window-placement.ps1` | file | PowerShell test script for main window placement behavior; not required to compile or run. |
| `test-minimize-modes.ps1` | file | PowerShell test script for minimize mode behavior; not required to compile or run. |
| `test-minimized-capture.ps1` | file | PowerShell test script for minimized capture behavior; not required to compile or run. |
| `test-minimized-progress.ps1` | file | PowerShell test script for minimized progress behavior; not required to compile or run. |
| `test-single-instance.ps1` | file | PowerShell test script for single-instance behavior; not required to compile or run. |
| `test-startup-registry.ps1` | file | PowerShell test script for startup/registry behavior; not required to compile or run. |

## Unused items moved from `C:\dmultra`

Destination: `C:\dmultra\unused\` (all original contents preserved here after extracting needed reference materials to `dmultra-base\reference`).

| Item | Type | Files | Size | Description |
|---|---|---:|---:|---|
| `.git` | directory | 804 | 9.62 MB | Git repository metadata/history directory; not part of the Rust egui source needed to compile or run the selected version. |
| `assets` | directory | 39 | 233.38 KB | Asset files from the mixed dmultra workspace; graphics/icons preserved (also present in clean base where needed). |
| `browser-extension` | directory | 5 | 28.35 KB | Browser extension source from the mixed dmultra workspace; clean egui base retains its own required copy for compilation. |
| `idm-ghidra-decompile` | directory | 64 | 1.05 MB | Ghidra decompilation/reference outputs from reverse engineering; copied to dmultra-base\reference for future feature work, original moved here. |
| `locales` | directory | 1 | 158 B | Localization files from the mixed dmultra workspace; clean egui base retains its own copy. |
| `releases` | directory | 9 | 67.86 MB | Packaged release artifacts/binaries; distributables, not required to compile the source. |
| `src` | directory | 28 | 997.21 KB | Rust source tree variant from the mixed dmultra workspace (includes Tauri/app integration files not in the clean egui base); preserved as unused/reference copy. |
| `src-tauri` | directory | 8166 | 5.21 GB | Tauri application wrapper/project and its build artifacts; alternate desktop shell, not used by the selected Rust egui version. |
| `target` | directory | 9862 | 4.83 GB | Cargo/Rust build output directory for the root project; generated artifacts, not source and not needed to compile from clean state. |
| `ui` | directory | 2684 | 84.76 MB | React/Vite web frontend project (includes node_modules and build config); alternate UI, not used by the selected Rust egui version. |
| `.gitignore` | file | 1 | 105 B | Git ignore rules from mixed workspace; clean base has its own .gitignore. |
| `AGENTS.md` | file | 1 | 5.99 KB | Markdown documentation/notes; relevant docs also preserved in clean base/reference where applicable. |
| `browser-key.xml` | file | 1 | 1.64 KB | Browser/key-related reverse-engineering reference data; copied to dmultra-base\reference, original moved here. |
| `build.ps1` | file | 1 | 765 B | Build helper script from mixed workspace; clean base has its own build script. |
| `build.rs` | file | 1 | 972 B | Build script from mixed workspace root; clean base has its own build script. |
| `Cargo.lock` | file | 1 | 175.79 KB | Cargo lockfile from mixed workspace root; clean base has its own lockfile. |
| `Cargo.toml` | file | 1 | 1015 B | Cargo manifest from mixed workspace root; clean base has its own manifest for the selected egui version. |
| `desktop_strings_2.txt` | file | 1 | 29.30 KB | Reverse-engineering extracted desktop strings reference (variant); copied to dmultra-base\reference, original moved here. |
| `desktop_strings.txt` | file | 1 | 18.21 KB | Reverse-engineering extracted desktop strings reference; copied to dmultra-base\reference, original moved here. |
| `DM_Ultra (1).ico` | file | 1 | 127.40 KB | Graphic/image asset file; retained in clean base or reference where needed for the egui project and future work. |
| `DM_Ultra_1024.png` | file | 1 | 1.08 MB | Graphic/image asset file; retained in clean base or reference where needed for the egui project and future work. |
| `DM_Ultra_256.png` | file | 1 | 84.71 KB | Graphic/image asset file; retained in clean base or reference where needed for the egui project and future work. |
| `DM_Ultra.ico` | file | 1 | 127.40 KB | Graphic/image asset file; retained in clean base or reference where needed for the egui project and future work. |
| `dmultra-chrome.crx` | file | 1 | 20.83 KB | Prebuilt Chrome extension (CRX) package; distributable artifact, not needed to compile. |
| `dmultra-chrome.zip` | file | 1 | 20.25 KB | Prebuilt Chrome extension package; can be regenerated from browser-extension source. |
| `dmultra-firefox.xpi` | file | 1 | 19.83 KB | Prebuilt Firefox extension (XPI) package; distributable artifact, not needed to compile. |
| `dmultra-nmh-new.exe` | file | 1 | 231.50 KB | Alternate/prebuilt native messaging host executable variant; not needed to compile from source. |
| `dmultra-nmh.exe` | file | 1 | 318.50 KB | Prebuilt native messaging host executable; can be built from source (dmultra-nmh binary target). |
| `dmultra.exe` | file | 1 | 14.73 MB | Prebuilt DM Ultra executable binary; not source, not needed to compile. |
| `Glossy 3D Global Sync Emblem.png` | file | 1 | 1.70 MB | Graphic/image asset file; retained in clean base or reference where needed for the egui project and future work. |
| `Glossy Global Download Emblem.png` | file | 1 | 1.72 MB | Graphic/image asset file; retained in clean base or reference where needed for the egui project and future work. |
| `guicorrect.md` | file | 1 | 12.91 KB | Markdown documentation/notes; relevant docs also preserved in clean base/reference where applicable. |
| `icon.jpg` | file | 1 | 46.50 KB | Graphic/image asset file; retained in clean base or reference where needed for the egui project and future work. |
| `PARITY_PLAN.md` | file | 1 | 8.04 KB | Markdown documentation/notes; relevant docs also preserved in clean base/reference where applicable. |
| `PROGRESS.md` | file | 1 | 12.41 KB | Markdown documentation/notes; relevant docs also preserved in clean base/reference where applicable. |
| `README.md` | file | 1 | 6.07 KB | Markdown documentation/notes; relevant docs also preserved in clean base/reference where applicable. |
| `resource.rc` | file | 1 | 965 B | Windows resource script from mixed workspace; clean base has its own resource/build configuration. |
| `test-main-window-placement.ps1` | file | 1 | 5.78 KB | PowerShell test/diagnostic script; not required to compile or run the application. |
| `test-minimize-modes.ps1` | file | 1 | 6.00 KB | PowerShell test/diagnostic script; not required to compile or run the application. |
| `test-minimized-capture.ps1` | file | 1 | 3.60 KB | PowerShell test/diagnostic script; not required to compile or run the application. |
| `test-minimized-progress.ps1` | file | 1 | 13.48 KB | PowerShell test/diagnostic script; not required to compile or run the application. |
| `test-single-instance.ps1` | file | 1 | 3.70 KB | PowerShell test/diagnostic script; not required to compile or run the application. |
| `test-startup-registry.ps1` | file | 1 | 3.58 KB | PowerShell test/diagnostic script; not required to compile or run the application. |
| `VERSION` | file | 1 | 6 B | Version file from mixed workspace; clean base has its own VERSION file. |

## Notes

- The selected clean/compile-ready Rust egui project is `C:\dmultra-base`.
- No files were deleted; items not needed to compile/run the selected version were moved into `unused` folders for safekeeping.
- Reference materials requested (graphics, .md docs, Ghidra decompilation info, and related RE strings/data) were retained in `C:\dmultra-base` and/or `C:\dmultra-base\reference`.
- `C:\dmultra` was fully archived into `C:\dmultra\unused` after copying required reference materials into `C:\dmultra-base\reference`.
