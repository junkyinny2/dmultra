//! DM Ultra GUI - High-fidelity implementation matching IDM / DM Ultra dark theme.
//!
//! Architectural invariants (see AGENTS.md):
//! - Download progress window, Add-URL dialog, Settings dialog, and Scheduler
//!   are ALWAYS their own OS-level popout windows (`ctx.show_viewport_immediate`),
//!   never in-app panes.
//! - GUI-thread state access uses `manager.lock_ui()` (bounded spin on
//!   `try_lock_owned`), NEVER tokio's `blocking_lock`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Key, Margin, Rect, RichText, Rounding, Sense,
    Stroke, Ui, Vec2, ViewportBuilder, ViewportId, pos2,
};
use tray_icon::{
    icon::Icon as TrayIcon,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    TrayIconBuilder,
};

use crate::manager::Manager;
use crate::options::{self, SettingsDialog};
use crate::queue::{Schedule, MAIN_QUEUE};
use crate::state::{DownloadRec, State, Status};

const APP_VERSION: &str = include_str!("../VERSION");

static CURRENT_PALETTE: Mutex<Option<options::ThemePalette>> = Mutex::new(None);

fn current_palette() -> options::ThemePalette {
    CURRENT_PALETTE
        .lock()
        .ok()
        .and_then(|guard| *guard)
        .unwrap_or_else(|| {
            options::Appearance {
                mode: "dark".into(),
                light_theme: "defaultLight".into(),
                dark_theme: "dmUltra".into(),
                accent: "blue".into(),
                ui_scale: 100,
                font_size: 13.0,
                system_dark: true,
            }
            .palette()
        })
}

// ---------------------------------------------------------------------------
// Native (Win32) window helpers: dark title bars and window positioning.
// Declared with plain FFI like overlay.rs - no extra crate.
// ---------------------------------------------------------------------------
#[cfg(target_os = "windows")]
mod native_win {
    use std::ffi::c_int;
    use std::sync::Mutex;

    /// Titles of our own OS windows that must follow the app theme.
    pub const OUR_TITLES_EXACT: [&str; 6] = [
        "DM Ultra",
        "Add new download...",
        "Confirm Delete",
        "Scheduler",
        "Options",
        "Site Grabber",
    ];
    pub const OUR_TITLES_PREFIX: &str = "Download status - ";

    fn is_main_title(text: &str) -> bool {
        text == "DM Ultra" || text.starts_with("DM Ultra v")
    }

    /// Window-chrome attributes understood by DWM. The colour ones need
    /// Windows 11 22000+; older builds simply ignore those calls.
    const DWMWA_USE_IMMERSIVE_DARK_MODE: c_int = 20;
    const DWMWA_BORDER_COLOR: c_int = 34;
    const DWMWA_CAPTION_COLOR: c_int = 35;
    const DWMWA_TEXT_COLOR: c_int = 36;

    /// DWM wants colours as 0x00BBGGRR (`COLORREF`).
    fn colorref(c: [u8; 3]) -> u32 {
        (c[0] as u32) | ((c[1] as u32) << 8) | ((c[2] as u32) << 16)
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    pub struct RectL {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: isize,
            attr: c_int,
            value: *const c_int,
            size: u32,
        ) -> i32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(cb: extern "system" fn(isize, isize) -> i32, lparam: isize) -> i32;
        fn GetWindowTextW(hwnd: isize, buf: *mut u16, max: i32) -> i32;
        fn GetWindowRect(hwnd: isize, rect: *mut RectL) -> i32;
        fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
        fn SystemParametersInfoW(action: u32, param: u32, pv: *mut RectL, win_ini: u32) -> i32;
        fn GetSystemMetrics(index: i32) -> i32;
        fn GetDpiForSystem() -> u32;
    }

    /// Desktop work area (screen minus taskbar), in physical pixels.
    pub fn work_area() -> Option<RectL> {
        const SPI_GETWORKAREA: u32 = 0x0030;
        let mut rect = RectL::default();
        let ok = unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut rect, 0) };
        (ok != 0).then_some(rect)
    }

    /// Whole virtual desktop spanning every attached monitor, in physical
    /// pixels: `(x, y, width, height)`.
    pub fn virtual_screen() -> Option<(i32, i32, i32, i32)> {
        const SM_XVIRTUALSCREEN: i32 = 76;
        const SM_YVIRTUALSCREEN: i32 = 77;
        const SM_CXVIRTUALSCREEN: i32 = 78;
        const SM_CYVIRTUALSCREEN: i32 = 79;
        let (x, y, w, h) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        (w > 0 && h > 0).then_some((x, y, w, h))
    }

    /// System DPI scale (1.0 at 96 DPI). Used to turn the physical pixel
    /// metrics above into the logical points egui window builders expect.
    pub fn system_scale() -> f32 {
        let dpi = unsafe { GetDpiForSystem() };
        if dpi == 0 {
            1.0
        } else {
            dpi as f32 / 96.0
        }
    }

    static COLLECTED: Mutex<Vec<(isize, String)>> = Mutex::new(Vec::new());

    extern "system" fn collect_cb(hwnd: isize, _lparam: isize) -> i32 {
        // Only consider windows owned by this process: other applications
        // may legitimately have windows titled "Options" or "Scheduler".
        let mut pid: u32 = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        if pid != std::process::id() {
            return 1;
        }
        const MAX: i32 = 256;
        let mut buf = [0u16; MAX as usize];
        let len = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), MAX) };
        if len > 0 {
            let text = String::from_utf16_lossy(&buf[..len as usize]);
            if let Ok(mut list) = COLLECTED.lock() {
                list.push((hwnd, text));
            }
        }
        1
    }

    fn is_ours(text: &str) -> bool {
        OUR_TITLES_EXACT.contains(&text)
            || is_main_title(text)
            || text.starts_with(OUR_TITLES_PREFIX)
    }

    /// Refresh the snapshot of our top-level windows.
    fn enumerate() {
        if let Ok(mut list) = COLLECTED.lock() {
            list.clear();
        }
        unsafe {
            EnumWindows(collect_cb, 0);
        }
    }

    /// Paint every window we own with the app's theme colours: dark/light
    /// immersive mode plus an explicit caption/border/text colour, so the
    /// title bar matches the app chrome instead of DWM's plain black.
    /// Cheap enough to call every frame: DWM ignores attributes whose value
    /// has not changed, and enumerating our own windows is a few syscalls.
    /// Calling it every frame means a popout created during this frame is
    /// themed before it is ever composited with the default title bar.
    pub fn apply_titlebar_theme(
        dark: bool,
        caption: [u8; 3],
        text: [u8; 3],
        border: [u8; 3],
    ) {
        enumerate();
        let mut windows: Vec<isize> = Vec::new();
        if let Ok(list) = COLLECTED.lock() {
            for (hwnd, title) in list.iter() {
                if is_ours(title) {
                    windows.push(*hwnd);
                }
            }
        }
        if windows.is_empty() {
            return;
        }

        let immersive: i32 = if dark { 1 } else { 0 };
        let caption_ref = colorref(caption) as i32;
        let text_ref = colorref(text) as i32;
        let border_ref = colorref(border) as i32;
        let int_size = std::mem::size_of::<i32>() as u32;
        for hwnd in windows {
            unsafe {
                DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_USE_IMMERSIVE_DARK_MODE,
                    &immersive,
                    int_size,
                );
                DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR, &caption_ref, int_size);
                DwmSetWindowAttribute(hwnd, DWMWA_TEXT_COLOR, &text_ref, int_size);
                DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &border_ref, int_size);
            }
        }
    }

    /// Outer screen rect of the window with exactly `title`, if any.
    pub fn window_rect(title: &str) -> Option<(i32, i32, i32, i32)> {
        enumerate();
        if let Ok(list) = COLLECTED.lock() {
            for (hwnd, text) in list.iter() {
                let title_matches = text == title || (title == "DM Ultra" && is_main_title(text));
                if is_ours(text) && title_matches {
                    let mut rect = RectL::default();
                    if unsafe { GetWindowRect(*hwnd, &mut rect) } != 0 {
                        return Some((rect.left, rect.top, rect.right, rect.bottom));
                    }
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// "Open" actions: run a downloaded file with the program registered for its
// file type.
//
// `explorer.exe <file>` never launches a file - it only opens an Explorer
// window on the folder that holds it - which is why the status window's "Open"
// button behaved exactly like "Open Folder". Running a file needs the shell
// open verb (what a double-click in Explorer does). Declared with plain FFI
// like the Win32 code above, so no extra crate is pulled in.
// ---------------------------------------------------------------------------

// `ShellExecuteW` with the "open" verb. Returns an `HINSTANCE` (a value
// greater than 32) on success, or one of the `SE_ERR_*` codes on failure.
#[cfg(target_os = "windows")]
#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: *mut std::ffi::c_void,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show_cmd: std::ffi::c_int,
    ) -> isize;
}

/// Why the "Open" action cannot run for `path`, or `None` when the file is on
/// disk and can be launched. Kept free of any shell call so it is testable and
/// safe to ask about on every frame.
fn open_action_blocker(path: &std::path::Path) -> Option<String> {
    if path.as_os_str().is_empty() {
        return Some("nothing has been saved for this download yet".to_string());
    }
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => None,
        Ok(_) => Some(format!("{} is a folder, not a file", path.display())),
        Err(_) => Some(format!("{} is not on disk yet", path.display())),
    }
}

/// Launch `path` with the program Windows has registered for its extension.
#[cfg(target_os = "windows")]
fn shell_open(path: &std::path::Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    /// SW_SHOWNORMAL: let the associated application open its window normally.
    const SW_SHOWNORMAL: std::ffi::c_int = 1;

    let file: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let verb: Vec<u16> = std::ffi::OsStr::new("open")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let status = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if status > 32 {
        return Ok(());
    }
    Err(match status {
        2 => "the file was not found".to_string(),
        5 => "access to the file was denied".to_string(),
        31 => "no application is registered for this file type".to_string(),
        32 => "a shell component could not be loaded".to_string(),
        other => format!("the shell rejected the file (error {other})"),
    })
}

#[cfg(not(target_os = "windows"))]
fn shell_open(path: &std::path::Path) -> Result<(), String> {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    match std::process::Command::new(opener).arg(path).spawn() {
        Ok(_) => Ok(()),
        Err(error) => Err(format!("could not run {opener}: {error}")),
    }
}

/// Screen position for a freshly opened main window: centred in the
/// desktop work area. `inner` is the egui client size in points.
#[cfg(target_os = "windows")]
fn initial_center_pos(inner: [f32; 2]) -> Option<egui::Pos2> {
    let area = native_win::work_area()?;
    let scale = native_win::system_scale();
    let to_logical = |v: i32| v as f32 / scale;
    // `with_position` is an OUTER position, so account for the frame.
    let outer_w = inner[0] + POPOUT_CHROME_W;
    let outer_h = inner[1] + POPOUT_CHROME_H;
    let area_w = to_logical(area.right - area.left);
    let area_h = to_logical(area.bottom - area.top);
    Some(egui::pos2(
        to_logical(area.left) + ((area_w - outer_w) * 0.5).max(0.0),
        to_logical(area.top) + ((area_h - outer_h) * 0.5).max(0.0),
    ))
}

#[cfg(not(target_os = "windows"))]
fn initial_center_pos(_inner: [f32; 2]) -> Option<egui::Pos2> {
    None
}

/// Whether a saved outer position still lands on an attached display.
/// Used to avoid reopening completely off-screen after a monitor change.
#[cfg(target_os = "windows")]
fn monitor_bounds_ok(pos: [f32; 2], inner: [f32; 2]) -> bool {
    let Some((vx, vy, vw, vh)) = native_win::virtual_screen() else {
        return true;
    };
    let scale = native_win::system_scale();
    let to_logical = |v: i32| v as f32 / scale;
    let (vx, vy) = (to_logical(vx), to_logical(vy));
    let (vw, vh) = (to_logical(vw), to_logical(vh));
    let w = inner[0] + POPOUT_CHROME_W;
    let h = inner[1] + POPOUT_CHROME_H;
    // A title-bar-sized strip of the window must be reachable.
    pos[0] + w > vx
        && pos[1] + h > vy
        && pos[0] < vx + vw - 80.0
        && pos[1] < vy + vh - 40.0
}

#[cfg(not(target_os = "windows"))]
fn monitor_bounds_ok(_pos: [f32; 2], _inner: [f32; 2]) -> bool {
    true
}

/// Position for a centered OS window of the given OUTER size over the
/// main window. Coordinates are in egui logical points (the
/// `ViewportCommand::OuterPosition` command converts to physical pixels
/// using the context's pixels-per-point).
#[cfg(target_os = "windows")]
fn centered_on_main_pos(
    ctx: &egui::Context,
    outer_w: f32,
    outer_h: f32,
) -> Option<egui::Pos2> {
    let ppp = ctx.pixels_per_point();
    let to_logical = |v: i32| v as f32 / ppp;
    if let Some((left, top, right, bottom)) = native_win::window_rect("DM Ultra") {
        // A minimized window parks at (-32000, -32000); do not center
        // relative to that.
        if left > -10_000 && right > left && bottom > top {
            let main_w = to_logical(right - left);
            let main_h = to_logical(bottom - top);
            return Some(egui::pos2(
                to_logical(left) + ((main_w - outer_w) * 0.5).max(0.0),
                to_logical(top) + ((main_h - outer_h) * 0.5).max(0.0),
            ));
        }
    }
    // No usable main window (minimized, or not created yet). A progress
    // window opened from a browser click must still land on screen, so
    // centre it over the desktop work area instead.
    let area = native_win::work_area()?;
    let area_w = to_logical(area.right - area.left);
    let area_h = to_logical(area.bottom - area.top);
    Some(egui::pos2(
        to_logical(area.left) + ((area_w - outer_w) * 0.5).max(0.0),
        to_logical(area.top) + ((area_h - outer_h) * 0.5).max(0.0),
    ))
}

#[cfg(target_os = "windows")]
fn adjacent_progress_pos(
    ctx: &egui::Context,
    outer_size: [f32; 2],
    cascade_index: usize,
) -> Option<egui::Pos2> {
    let (main_left, main_top, main_right, _) = native_win::window_rect("DM Ultra")?;
    let area = native_win::work_area()?;
    let scale = ctx.pixels_per_point();
    let to_logical = |value: i32| value as f32 / scale;
    let (area_left, area_top, area_right, area_bottom) = (
        to_logical(area.left),
        to_logical(area.top),
        to_logical(area.right),
        to_logical(area.bottom),
    );
    let (main_left, main_top, main_right) = (
        to_logical(main_left),
        to_logical(main_top),
        to_logical(main_right),
    );
    let right_x = main_right + 8.0;
    let left_x = main_left - outer_size[0] - 8.0;
    let right_space = area_right - right_x;
    let left_space = left_x - area_left;
    let max_x = (area_right - outer_size[0]).max(area_left);
    let x = if right_space >= outer_size[0] {
        right_x
    } else if left_space >= 0.0 {
        left_x
    } else if right_space >= left_space {
        right_x.clamp(area_left, max_x)
    } else {
        left_x.clamp(area_left, max_x)
    };
    let max_y = (area_bottom - outer_size[1]).max(area_top);
    let y = (main_top + cascade_index as f32 * 28.0).clamp(area_top, max_y);
    Some(egui::pos2(x, y))
}

#[cfg(not(target_os = "windows"))]
fn adjacent_progress_pos(
    _ctx: &egui::Context,
    _outer_size: [f32; 2],
    _cascade_index: usize,
) -> Option<egui::Pos2> {
    None
}

/// Width/height a native window frame adds around a winit client area
/// (8 px border each side, 39 px caption). Used to turn an inner size into
/// the OUTER size `with_position` works in.
pub const POPOUT_CHROME_W: f32 = 16.0;
pub const POPOUT_CHROME_H: f32 = 39.0;

/// Screen position (logical points) where a popout opens when it has no
/// remembered spot of its own: centred over the main window. `outer_size`
/// includes the native frame (caption + borders).
pub fn centered_popup_pos(ctx: &egui::Context, outer_size: [f32; 2]) -> Option<egui::Pos2> {
    #[cfg(target_os = "windows")]
    {
        centered_on_main_pos(ctx, outer_size[0], outer_size[1])
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (ctx, outer_size);
        None
    }
}

const COLUMN_LABELS: [&str; 8] = [
    "File Name", "Queue", "Size", "Status", "Time left", "Transfer rate", "Last Try",
    "Description",
];
const DEFAULT_COLUMN_WIDTHS: [f32; 8] = [175.0, 56.0, 70.0, 72.0, 64.0, 78.0, 78.0, 100.0];
const MIN_COLUMN_WIDTHS: [f32; 8] = [120.0, 56.0, 70.0, 72.0, 64.0, 78.0, 78.0, 100.0];
const MAX_COLUMN_WIDTH: f32 = 1600.0;
const COLUMN_RESIZE_HIT_WIDTH: f32 = 8.0;
const COLUMN_CELL_PADDING: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct ColumnTrack {
    column: usize,
    width: f32,
}

fn column_tracks(widths: &[f32], visible: &[bool]) -> Vec<ColumnTrack> {
    (0..COLUMN_LABELS.len())
        .filter(|&column| visible.get(column).copied().unwrap_or(true))
        .map(|column| ColumnTrack {
            column,
            width: widths
                .get(column)
                .copied()
                .unwrap_or(DEFAULT_COLUMN_WIDTHS[column]),
        })
        .collect()
}

fn resized_column_width(current_width: f32, frame_delta: f32, column: usize) -> f32 {
    (current_width + frame_delta).clamp(MIN_COLUMN_WIDTHS[column], MAX_COLUMN_WIDTH)
}

// ---------------------------------------------------------------------------
// Embedded Lucide SVG Icons from assets/icons
// ---------------------------------------------------------------------------
const SVG_ADD: &[u8] = include_bytes!("../assets/icons/add.svg");
const SVG_START: &[u8] = include_bytes!("../assets/icons/resume.svg");
const SVG_PAUSE: &[u8] = include_bytes!("../assets/icons/pause.svg");
const SVG_STOP_ALL: &[u8] = include_bytes!("../assets/icons/queue-stop.svg");
const SVG_RESUME_ALL: &[u8] = include_bytes!("../assets/icons/queue-start.svg");
const SVG_DELETE: &[u8] = include_bytes!("../assets/icons/delete.svg");
const SVG_DELETE_ALL: &[u8] = include_bytes!("../assets/icons/delete-all.svg");
const SVG_OPTIONS: &[u8] = include_bytes!("../assets/icons/settings.svg");
const SVG_SCHEDULER: &[u8] = include_bytes!("../assets/icons/clock.svg");
const SVG_SITE_GRABBER: &[u8] = include_bytes!("../assets/icons/globe.svg");
const SVG_STOP: &[u8] = include_bytes!("../assets/icons/x-circle.svg");

const SVG_FOLDER: &[u8] = include_bytes!("../assets/icons/folder.svg");
const SVG_PROGRESS: &[u8] = include_bytes!("../assets/icons/activity.svg");
const SVG_CHECK: &[u8] = include_bytes!("../assets/icons/check-circle.svg");
const SVG_DOC: &[u8] = include_bytes!("../assets/icons/file-text.svg");
const SVG_MUSIC: &[u8] = include_bytes!("../assets/icons/music.svg");
const SVG_VIDEO: &[u8] = include_bytes!("../assets/icons/film.svg");
const SVG_PROGRAMS: &[u8] = include_bytes!("../assets/icons/monitor.svg");
const SVG_ARCHIVE: &[u8] = include_bytes!("../assets/icons/package.svg");
const SVG_QUEUE: &[u8] = include_bytes!("../assets/icons/list.svg");
const SVG_SEARCH: &[u8] = include_bytes!("../assets/icons/search.svg");

// ---------------------------------------------------------------------------
// Theme Palette (dynamic: follows the saved appearance settings)
// ---------------------------------------------------------------------------
fn bg_dark() -> Color32 { current_palette().background }
fn bg_panel() -> Color32 { current_palette().surface }
fn bg_header() -> Color32 { current_palette().chrome }
fn bg_row_even() -> Color32 { current_palette().surface }
fn bg_row_odd() -> Color32 { current_palette().surface2 }
fn bg_row_hover() -> Color32 { current_palette().row_hover }
fn bg_row_selected() -> Color32 { current_palette().nav_selected }
fn bg_pill_selected() -> Color32 { current_palette().nav_selected }
fn bg_badge() -> Color32 { current_palette().surface2 }
fn bg_badge_sel() -> Color32 { current_palette().nav_selected }

fn border_hairline() -> Color32 { current_palette().hairline }

fn grid_line() -> Color32 {
    // Between the hairline and the muted text tone so the cell rules are
    // clearly visible on the row backgrounds in both light and dark themes.
    let p = current_palette();
    let f = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * 0.55).round() as u8;
    Color32::from_rgb(f(p.border.r(), p.muted.r()), f(p.border.g(), p.muted.g()), f(p.border.b(), p.muted.b()))
}

fn text_main() -> Color32 { current_palette().text }
fn text_dim() -> Color32 { current_palette().muted }
fn text_badge() -> Color32 { current_palette().muted }
fn text_empty_title() -> Color32 { current_palette().muted }
fn text_empty_sub() -> Color32 { current_palette().text_tertiary }

const ACCENT_BLUE: Color32 = Color32::from_rgb(57, 133, 245);  // #3985F5 (Add URL)
const ACCENT_GREEN: Color32 = Color32::from_rgb(33, 196, 93);  // #21C45D (Resume All)
const ACCENT_RED: Color32 = Color32::from_rgb(216, 64, 66);    // #D84042 (Delete All)
fn status_complete() -> Color32 { current_palette().success }
fn status_downloading() -> Color32 { current_palette().accent }
fn status_paused() -> Color32 { current_palette().warning }
fn status_failed() -> Color32 { current_palette().destructive }

// ---------------------------------------------------------------------------
// Category definitions matching the sidebar
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NavCategory {
    AllDownloads,
    InProgress,
    Completed,
    Documents,
    Music,
    Video,
    Programs,
    Compressed,
    // Queue section
    Scheduled,
    Queued,
    Paused,
}

impl NavCategory {
    pub fn label(&self) -> &'static str {
        match self {
            NavCategory::AllDownloads => "All Downloads",
            NavCategory::InProgress   => "In Progress",
            NavCategory::Completed    => "Completed",
            NavCategory::Documents    => "Documents",
            NavCategory::Music        => "Music",
            NavCategory::Video        => "Video",
            NavCategory::Programs     => "Programs",
            NavCategory::Compressed   => "Compressed",
            NavCategory::Scheduled    => "Scheduled",
            NavCategory::Queued       => "Queued",
            NavCategory::Paused       => "Paused",
        }
    }
}

// ---------------------------------------------------------------------------
// Icon Cache using resvg
// ---------------------------------------------------------------------------
struct IconCache {
    textures: HashMap<String, egui::TextureHandle>,
}

impl IconCache {
    fn new() -> Self {
        Self {
            textures: HashMap::new(),
        }
    }

    fn get_or_render(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        svg_bytes: &[u8],
        color: Color32,
        size: u32,
    ) -> egui::TextureHandle {
        let lookup_key = format!("{key}_{:02X}{:02X}{:02X}_{size}", color.r(), color.g(), color.b());
        if let Some(tex) = self.textures.get(&lookup_key) {
            return tex.clone();
        }

        let hex = format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b());
        let mut svg_str = String::from_utf8_lossy(svg_bytes).to_string();
        svg_str = svg_str.replace("currentColor", &hex);

        let mut opt = resvg::usvg::Options::default();
        opt.fontdb_mut().load_system_fonts();

        let image = match resvg::usvg::Tree::from_data(svg_str.as_bytes(), &opt) {
            Ok(t) => {
                let int_size = t.size().to_int_size();
                let sx = size as f32 / int_size.width() as f32;
                let sy = size as f32 / int_size.height() as f32;
                let transform = resvg::tiny_skia::Transform::from_scale(sx, sy);
                if let Some(mut pixmap) = resvg::tiny_skia::Pixmap::new(size, size) {
                    resvg::render(&t, transform, &mut pixmap.as_mut());
                    egui::ColorImage::from_rgba_unmultiplied(
                        [size as usize, size as usize],
                        pixmap.data(),
                    )
                } else {
                    egui::ColorImage::new([size as usize, size as usize], color)
                }
            }
            Err(_) => {
                egui::ColorImage::new([size as usize, size as usize], color)
            }
        };

        let handle = ctx.load_texture(&lookup_key, image, egui::TextureOptions::LINEAR);
        self.textures.insert(lookup_key, handle.clone());
        handle
    }
}

// ---------------------------------------------------------------------------
// Popout State (Separate OS Viewports)
// ---------------------------------------------------------------------------
#[derive(Default)]
struct AddUrlDialogState {
    open: bool,
    auto_started: bool,
    url: String,
    referer: Option<String>,
    save_as: String,
    queue: String,
    description: String,
    connections: u32,
    error: String,
}

#[derive(Default)]
struct SchedulerDialogState {
    open: bool,
    queue: String,
    start_time: String,
    stop_time: String,
    days: Vec<u32>,
    once: bool,
    on_finish: String,
    error: String,
}

/// Confirmation for Delete / Delete All: removes records from the list
/// and, when the checkbox is ticked, also deletes the files from disk.
/// (There is no Trash category - deletes here are final.)
#[derive(Default)]
struct DeleteConfirmState {
    open: bool,
    all: bool,
    ids: Vec<u64>,
    from_disk: bool,
}

struct GrabberDialogState {
    open: bool,
    start_url: String,
    depth: u32,
    same_origin: bool,
    file_types: String,
    queue: String,
    running: bool,
    pages_crawled: Arc<AtomicU64>,
    result: Arc<Mutex<Option<std::result::Result<crate::grabber::CrawlResult, String>>>>,
    task: Option<tokio::task::JoinHandle<()>>,
    links: Vec<String>,
    selected: HashSet<String>,
    error: String,
    report: String,
}

impl Default for GrabberDialogState {
    fn default() -> Self {
        Self {
            open: false,
            start_url: String::new(),
            depth: 2,
            same_origin: true,
            file_types: crate::filetypes::DEFAULT_FILE_TYPES.to_string(),
            queue: MAIN_QUEUE.to_string(),
            running: false,
            pages_crawled: Arc::new(AtomicU64::new(0)),
            result: Arc::new(Mutex::new(None)),
            task: None,
            links: Vec::new(),
            selected: HashSet::new(),
            error: String::new(),
            report: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Main Application State
// ---------------------------------------------------------------------------
pub struct DmApp {
    manager: Arc<Manager>,
    show_flag: Arc<Mutex<bool>>,
    show_main_window: bool,
    /// One-shot: hide the main window after eframe's first-frame
    /// anti-flash `set_visible(true)` when starting in the background.
    hide_main_window: bool,
    tray_icon: Option<tray_icon::TrayIcon>,
    /// Last applied tray-icon visibility, to avoid re-registering it each frame.
    tray_visible: bool,
    /// Previous minimized state, so the tray-only mode reacts to the
    /// minimize button exactly once.
    was_minimized: bool,

    // Nav & filtering
    selected_nav: NavCategory,
    categories_visible: bool,
    search_query: String,
    search_open: bool,
    search_focus: bool,
    search_icon_rect: Rect,
    selected_download_id: Option<u64>,

    // Popouts & Child windows
    add_dialog: AddUrlDialogState,
    settings_dialog: SettingsDialog,
    scheduler_dialog: SchedulerDialogState,
    grabber_dialog: GrabberDialogState,
    active_details_popouts: Vec<u64>, // IDs of open progress popout viewports
    about_open: bool,
    delete_confirm: DeleteConfirmState,

    // Icons & accent
    icons: IconCache,
    accent: Color32,

    // Live transfer-rate sampling (per download id)
    speed_samples: HashMap<u64, (Instant, u64)>,
    speeds: HashMap<u64, f64>,

    /// Popout set the window-procedure hook was last installed for, and
    /// whether a hook pass is still owed (a window that was mid-creation).
    /// Scanning for popout windows is expensive, so it is throttled to a few
    /// passes per second while a child is open.
    hook_signature: (Vec<u64>, u8),
    hook_pending: bool,
    last_hook_refresh: Instant,
    /// A popout position changed in memory and still needs to be flushed to
    /// `state.json` (throttled so a drag does not rewrite the file on every
    /// frame, and written outside the GUI lock).
    popout_pos_dirty: bool,
    last_popout_save: Instant,

    // Scheduler runtime state
    scheduler_last_tick: Instant,
    scheduler_last_window: HashMap<String, bool>,
    scheduler_started: HashSet<String>,
    scheduler_suppressed: HashSet<String>,
    active_queue: Option<String>,

    // Status bar & Sort
    status_bar: String,
    sort_column: usize,
    sort_descending: bool,
    last_update: Instant,

    // Clipboard watcher (settings.monitor_clipboard); a link copied into
    // the clipboard opens the Add-URL window (IDM behaviour).
    last_clip_text: String,
    clip_last_poll: Instant,
    clip_last_error: String,
    clip_probe_tx: mpsc::Sender<String>,
    clip_probe_rx: mpsc::Receiver<String>,
    browser_last_poll: Instant,
    browser_pending: VecDeque<(String, String)>,
    browser_last_error: String,
    /// Throttles writing the main window position/size to state.json while
    /// the user drags or resizes the window.
    last_main_window_save: Instant,
    /// Throttles checking for a second-instance "show" request.
    last_show_poll: Instant,
    /// Set while the UI must keep painting even if the main window is
    /// minimized (active transfers or open child windows). Read by the
    /// frame waker in `browser::spawn_busy_waker`.
    busy_frames: Arc<std::sync::atomic::AtomicBool>,
    /// Monotonic count of completed frames. The busy waker reads it so it
    /// re-posts a synthetic wake only after the previous one has been
    /// consumed. Posting a wake every 100 ms while a frame costs ~200 ms
    /// grows the Win32 queue without bound and starves input (the frozen
    /// progress popout).
    frame_seq: Arc<AtomicU64>,
    /// Set while "minimize to tray only" is active and the window is still
    /// shown, so the waker can deliver the frame that hides it.
    minimize_watch: Arc<std::sync::atomic::AtomicBool>,

    // Resizable/reorderable download-list columns
    column_widths: Vec<f32>,
    column_visible: Vec<bool>,
}

impl DmApp {
    pub fn new(manager: Arc<Manager>, start_hidden: bool) -> Self {
        let (clip_probe_tx, clip_probe_rx) = mpsc::channel();
        let show_flag = Arc::new(Mutex::new(false));
        let minimize_watch = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tray_icon = match create_tray_icon(show_flag.clone()) {
            Ok(tray) => Some(tray),
            Err(error) => {
                crate::logfile::log("TRAY", &format!("tray icon unavailable: {error}"));
                None
            }
        };
        let (accent, column_widths, column_visible) = {
            let st = manager.lock_ui();
            (
                ui_accent(&st.settings.accent),
                column_widths_from_settings(&st.settings),
                column_visible_from_settings(&st.settings),
            )
        };
        {
            // The Trash category has been removed; drop any legacy
            // records it held (their files stay untouched).
            let mut st = manager.lock_ui();
            if !st.trash.is_empty() {
                st.trash.clear();
                let _ = st.save();
            }
        }

        // Keep the frame loop alive while the main window is hidden or
        // minimized (eframe paints the child popouts from the parent frame).
        // `frame_seq` makes the waker self-clocking so it cannot outrun
        // painting and starve input messages.
        let busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let frame_seq = Arc::new(AtomicU64::new(0));
        crate::browser::spawn_busy_waker(
            Arc::clone(&busy),
            Arc::clone(&minimize_watch),
            Arc::clone(&frame_seq),
        );

        Self {
            manager,
            show_flag,
            show_main_window: !start_hidden,
            hide_main_window: start_hidden,
            tray_icon,
            tray_visible: true,
            was_minimized: false,
            selected_nav: NavCategory::AllDownloads,
            categories_visible: true,
            search_query: String::new(),
            search_open: false,
            search_focus: false,
            search_icon_rect: Rect::NOTHING,
            selected_download_id: None,
            add_dialog: AddUrlDialogState {
                connections: 8,
                queue: MAIN_QUEUE.to_string(),
                ..Default::default()
            },
            settings_dialog: SettingsDialog::default(),
            scheduler_dialog: SchedulerDialogState {
                queue: MAIN_QUEUE.into(),
                start_time: "02:00".into(),
                stop_time: "07:00".into(),
                on_finish: "none".into(),
                ..Default::default()
            },
            grabber_dialog: GrabberDialogState::default(),
            active_details_popouts: Vec::new(),
            about_open: false,
            delete_confirm: DeleteConfirmState::default(),
            icons: IconCache::new(),
            accent,
            speed_samples: HashMap::new(),
            speeds: HashMap::new(),
            hook_signature: (Vec::new(), 0),
            hook_pending: false,
            last_hook_refresh: Instant::now() - Duration::from_secs(5),
            popout_pos_dirty: false,
            last_popout_save: Instant::now() - Duration::from_secs(5),
            scheduler_last_tick: Instant::now(),
            scheduler_last_window: HashMap::new(),
            scheduler_started: HashSet::new(),
            scheduler_suppressed: HashSet::new(),
            active_queue: None,
            status_bar: "Ready.".to_string(),
            // No sort arrow at launch: sorting only activates when the
            // user clicks a column header.
            sort_column: usize::MAX,
            sort_descending: true,
            last_update: Instant::now(),
            last_clip_text: {
                // Seed with the current clipboard so a link copied
                // before launch does not trigger the watcher at startup.
                arboard::Clipboard::new()
                    .and_then(|mut c| c.get_text())
                    .unwrap_or_default()
            },
            clip_last_poll: Instant::now() - Duration::from_secs(5),
            clip_last_error: String::new(),
            clip_probe_tx,
            clip_probe_rx,
            browser_last_poll: Instant::now() - Duration::from_secs(5),
            browser_pending: VecDeque::new(),
            browser_last_error: String::new(),
            last_main_window_save: Instant::now() - Duration::from_secs(5),
            last_show_poll: Instant::now() - Duration::from_secs(5),
            busy_frames: busy,
            frame_seq,
            minimize_watch,
            column_widths,
            column_visible,
        }
    }

    /// Fill the Add-URL fields with the current defaults. Does not show the
    /// window, so browser captures can create a download in the background.
    fn prepare_add_url(&mut self) {
        let (default_dir, default_queue, connections) = {
            let st = self.manager.lock_ui();
            (
                st.settings.save_path.clone(),
                if st.settings.default_queue.trim().is_empty() {
                    MAIN_QUEUE.to_string()
                } else {
                    st.settings.default_queue.clone()
                },
                st.settings.max_connections.clamp(1, 32),
            )
        };
        self.add_dialog.auto_started = false;
        self.add_dialog.save_as = default_dir.to_string_lossy().to_string();
        self.add_dialog.url.clear();
        self.add_dialog.referer = None;
        self.add_dialog.description.clear();
        self.add_dialog.queue = default_queue;
        self.add_dialog.connections = connections;
        self.add_dialog.error.clear();
    }

    fn open_add_url(&mut self) {
        self.prepare_add_url();
        self.add_dialog.open = true;
    }

    /// Open the Add-URL window prefilled with a URL (clipboard watcher
    /// hook, IDM-style "link copied -> download box").
    fn open_add_url_with(&mut self, url: String, referer: Option<String>) {
        self.open_add_url();
        self.add_dialog.url = url;
        self.add_dialog.referer = referer;
        self.status_bar = "Download link detected.".into();
    }

    /// Poll the clipboard at most twice per second. A newly seen
    /// http/https/ftp link opens the Add-URL window prefilled. The
    /// clipboard is not touched while the dialog is already open, and
    /// the launch-time clipboard is seeded so stale links do not pop
    /// the dialog on startup.
    fn tick_clipboard_watcher(&mut self, ctx: &egui::Context, settings: &crate::state::Settings) {
        if !self.add_dialog.open && settings.clipboard_probe {
            if let Ok(url) = self.clip_probe_rx.try_recv() {
                self.open_add_url_with(url, None);
            }
        }
        if self.add_dialog.open || self.clip_last_poll.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.clip_last_poll = Instant::now();
        let text = match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
            Ok(text) => {
                if !self.clip_last_error.is_empty() {
                    self.clip_last_error.clear();
                    self.status_bar = "Clipboard monitoring is working again.".into();
                }
                text
            }
            Err(error) => {
                let message = format!("Clipboard monitor unavailable: {error}");
                if self.clip_last_error != message {
                    self.status_bar = message.clone();
                    self.clip_last_error = message;
                }
                return;
            }
        };
        if text.is_empty() || text == self.last_clip_text {
            return;
        }
        self.last_clip_text = text.clone();
        for url in crate::clipboard::extract_urls(&text) {
            if crate::clipboard::is_download_candidate(&url, settings) {
                self.open_add_url_with(url, None);
                break;
            }
            if crate::clipboard::needs_extensionless_probe(&url, settings) {
                let sender = self.clip_probe_tx.clone();
                let repaint = ctx.clone();
                crate::manager::runtime().spawn(async move {
                    match crate::clipboard::probe_extensionless_url(&url).await {
                        Ok(true) => {
                            let _ = sender.send(url);
                            repaint.request_repaint();
                        }
                        Ok(false) => {}
                        Err(error) => {
                            crate::logfile::log(
                                "CLIPBOARD",
                                &format!("extensionless link probe failed: {error:#}"),
                            );
                        }
                    }
                });
            }
        }
    }

    fn poll_browser_pending(&mut self, ctx: &egui::Context) {
        ctx.request_repaint_after(Duration::from_millis(250));
        if self.browser_last_poll.elapsed() >= Duration::from_millis(250) {
            self.browser_last_poll = Instant::now();
            match crate::browser::take_pending() {
                Ok(Some((urls, referer))) => {
                    self.browser_last_error.clear();
                    for url in urls {
                        self.browser_pending.push_back((url, referer.clone()));
                    }
                }
                Ok(None) => self.browser_last_error.clear(),
                Err(error) => {
                    let message = format!("Browser integration could not read queued links: {error:#}");
                    if self.browser_last_error != message {
                        self.status_bar = message.clone();
                        crate::logfile::log("BROWSER", &message);
                        self.browser_last_error = message;
                    }
                }
            }
        }

        if self.add_dialog.open {
            return;
        }
        let Some((url, referer)) = self.browser_pending.pop_front() else {
            return;
        };
        // A browser click is an explicit "download this" instruction, so the
        // download starts straight away. The Add-URL window is only used when
        // the user asked for it, or when the capture could not be started
        // (bad URL, missing save folder) and needs their attention.
        let show_file_info = self.manager.lock_ui().settings.browser_show_file_info;
        if show_file_info {
            self.open_add_url();
            self.add_dialog.url = url;
            self.add_dialog.referer = (!referer.trim().is_empty()).then_some(referer);
            self.status_bar = "Browser download link received.".into();
            crate::logfile::log(
                "BROWSER",
                "capture opened File Info (\"Show File Info for browser downloads\" is enabled)",
            );
            return;
        }
        self.prepare_add_url();
        self.add_dialog.url = url;
        self.add_dialog.referer = (!referer.trim().is_empty()).then_some(referer);
        if self.create_download(true) {
            self.status_bar = "Download started from browser.".into();
        } else {
            let error = self.add_dialog.error.clone();
            self.add_dialog.open = true;
            self.status_bar = format!("Could not start browser download: {error}");
            crate::logfile::log("BROWSER", &format!("browser capture failed: {error}"));
        }
    }

    fn open_settings(&mut self) {
        let (settings, webhooks, site_logins) = {
            let st = self.manager.lock_ui();
            (
                st.settings.clone(),
                st.webhooks.clone(),
                st.site_logins.clone(),
            )
        };
        self.settings_dialog
            .open_with(
                &settings,
                options::system_prefers_dark(),
                webhooks,
                site_logins,
            );
    }

    fn open_scheduler(&mut self) {
        self.scheduler_dialog.open = true;
        self.scheduler_dialog.error.clear();
        let queue = self
            .scheduler_dialog
            .queue
            .clone()
            .trim()
            .to_string();
        self.load_scheduler_for_queue(if queue.is_empty() { MAIN_QUEUE } else { &queue });
    }

    fn load_scheduler_for_queue(&mut self, queue: &str) {
        self.scheduler_dialog.queue = queue.to_string();
        let schedule = {
            let st = self.manager.lock_ui();
            st.schedules.iter().find(|s| s.queue == queue).cloned()
        };
        if let Some(schedule) = schedule {
            self.scheduler_dialog.start_time = schedule
                .start
                .map(|(h, m)| format!("{h:02}:{m:02}"))
                .unwrap_or_default();
            self.scheduler_dialog.stop_time = schedule
                .stop
                .map(|(h, m)| format!("{h:02}:{m:02}"))
                .unwrap_or_default();
            self.scheduler_dialog.days = schedule.days.clone();
            self.scheduler_dialog.once = schedule.once;
            self.scheduler_dialog.on_finish = schedule.on_finish.clone();
        } else {
            self.scheduler_dialog.start_time = "02:00".into();
            self.scheduler_dialog.stop_time = "07:00".into();
            self.scheduler_dialog.days.clear();
            self.scheduler_dialog.once = false;
            self.scheduler_dialog.on_finish = "none".into();
        }
        self.scheduler_dialog.error.clear();
    }

    fn start_selected(&mut self) {
        match self.selected_download_id {
            Some(id) => self.start_download(id),
            None => self.status_bar = "Select a download first.".into(),
        }
    }

    fn start_download(&mut self, id: u64) {
        if self.manager.is_active(id) {
            self.status_bar = format!("Download #{id} is already running.");
            return;
        }
        let rec = {
            let st = self.manager.lock_ui();
            st.get(id).cloned()
        };
        if let Some(mut rec) = rec {
            if rec.status == Status::Downloading {
                self.status_bar = format!("Download #{id} is already running.");
                return;
            }
            rec.status = Status::Downloading;
            rec.last_try_date = chrono::Utc::now().timestamp();
            {
                let mut st = self.manager.lock_ui();
                if let Some(d) = st.get_mut(id) {
                    d.status = Status::Downloading;
                    d.last_try_date = rec.last_try_date;
                    let _ = st.save();
                }
            }
            match self.manager.start(rec, None, None) {
                Ok(()) => {
                    self.status_bar = format!("Starting download #{id}...");
                    let show_progress = self.manager.lock_ui().settings.show_progress_window;
                    if show_progress && !self.active_details_popouts.contains(&id) {
                        self.active_details_popouts.push(id);
                    }
                }
                Err(err) => self.status_bar = format!("Could not start download #{id}: {err}"),
            }
        } else {
            self.status_bar = format!("Download #{id} no longer exists.");
        }
    }

    fn pause_selected(&mut self) {
        match self.selected_download_id {
            Some(id) => self.pause_download(id),
            None => self.status_bar = "Select a download first.".into(),
        }
    }

    /// "Open" action for a download: run the saved file with the program
    /// registered for its file type (shell open verb). `explorer.exe` only
    /// *shows* a file, so it is used for folders only - by the separate
    /// "Open Folder" action. Failures are reported in the status bar and the
    /// diagnostic log because the popout that raised them has its own window.
    fn open_download_file(&mut self, id: u64, path: &std::path::Path) {
        let message = match open_action_blocker(path) {
            Some(reason) => {
                crate::logfile::log(
                    "OPEN",
                    &format!("download #{id} cannot open {}: {reason}", path.display()),
                );
                format!("Could not open: {reason}.")
            }
            None => match shell_open(path) {
                Ok(()) => {
                    crate::logfile::log(
                        "OPEN",
                        &format!("download #{id} launched {}", path.display()),
                    );
                    format!("Opened {}.", path.display())
                }
                Err(error) => {
                    crate::logfile::log(
                        "OPEN",
                        &format!("download #{id} failed to open {}: {error}", path.display()),
                    );
                    format!("Could not open {}: {error}", path.display())
                }
            },
        };
        self.status_bar = message;
    }

    fn pause_download(&mut self, id: u64) {
        self.manager.stop(id);
        let mut st = self.manager.lock_ui();
        if let Some(d) = st.get_mut(id) {
            d.status = Status::Paused;
            let _ = st.save();
            self.status_bar = format!("Paused download #{id}.");
        } else {
            self.status_bar = format!("Download #{id} no longer exists.");
        }
    }

    /// Raise/lower the connection limit for one download. Persists the
    /// record and, when the download is running, pushes the new limit
    /// into the live engine (supervisor reacts within ~150 ms).
    fn set_download_connections(&mut self, id: u64, n: u32) {
        let n = n.clamp(1, 32);
        {
            let mut st = self.manager.lock_ui();
            if let Some(d) = st.get_mut(id) {
                d.max_connections = Some(n);
                let _ = st.save();
            } else {
                self.status_bar = format!("Download #{id} no longer exists.");
                return;
            }
        }
        self.manager.set_connections(id, n);
        self.status_bar = format!("Connections for download #{id} set to {n}.");
    }

    fn resume_all(&mut self) {
        let to_resume = {
            let st = self.manager.lock_ui();
            st.downloads.iter()
                .filter(|d| d.status == Status::Paused || d.status == Status::Queued)
                .cloned()
                .collect::<Vec<_>>()
        };
        let count = to_resume.len();
        for rec in to_resume {
            let _ = self.manager.start(rec, None, None);
        }
        self.status_bar = if count == 0 {
            "No paused or queued downloads to resume.".into()
        } else {
            format!("Resumed {count} download(s).")
        };
    }

    fn stop_all(&mut self) {
        self.manager.stop_all();
        let mut st = self.manager.lock_ui();
        let mut count = 0usize;
        for d in &mut st.downloads {
            if d.status == Status::Downloading {
                d.status = Status::Paused;
                count += 1;
            }
        }
        let _ = st.save();
        self.status_bar = if count == 0 {
            "No active downloads to stop.".into()
        } else {
            format!("Stopped {count} download(s).")
        };
    }

    /// Open the delete confirmation window for the selected download.
    fn delete_selected(&mut self) {
        let Some(id) = self.selected_download_id else {
            self.status_bar = "Select a download first.".into();
            return;
        };
        if !self.manager.lock_ui().downloads.iter().any(|d| d.id == id) {
            self.status_bar = format!("Download #{id} no longer exists.");
            self.selected_download_id = None;
            return;
        }
        self.delete_confirm.open = true;
        self.delete_confirm.all = false;
        self.delete_confirm.ids = vec![id];
        self.delete_confirm.from_disk = false;
    }

    /// Open the delete confirmation window for every download in the
    /// list.
    fn delete_all(&mut self) {
        let ids: Vec<u64> = self
            .manager
            .lock_ui()
            .downloads
            .iter()
            .map(|d| d.id)
            .collect();
        if ids.is_empty() {
            self.status_bar = "No downloads to delete.".into();
            return;
        }
        self.delete_confirm.open = true;
        self.delete_confirm.all = true;
        self.delete_confirm.ids = ids;
        self.delete_confirm.from_disk = false;
    }

    /// Apply the confirmed delete: remove the records from the list and
    /// (optionally) delete the files from disk. Downloads that are still
    /// running are stopped first, and any engine still writing one of
    /// these files blocks the removal for this pass (the user simply
    /// presses Delete again).
    fn confirm_delete_now(&mut self) {
        self.delete_confirm.open = false;
        let ids: Vec<u64> = self.delete_confirm.ids.drain(..).collect();
        let all = self.delete_confirm.all;
        let from_disk = self.delete_confirm.from_disk;
        if ids.is_empty() {
            return;
        }

        if all {
            self.manager.stop_all();
        } else {
            for &id in &ids {
                self.manager.stop(id);
            }
        }
        // Engine tasks exit asynchronously; if one of the targets is
        // still running, ask the user to repeat (it will be stopped by
        // then). This also prevents the engine's periodic persist from
        // resurrecting a removed record.
        let still_running: Vec<u64> = ids
            .iter()
            .copied()
            .filter(|id| self.manager.is_active(*id))
            .collect();
        if !still_running.is_empty() {
            self.status_bar = format!(
                "Stopped {} download(s); press Delete again to remove them.",
                still_running.len()
            );
            self.delete_confirm.open = false;
            return;
        }

        let delete_set: std::collections::HashSet<u64> = ids.iter().copied().collect();
        let mut removed = 0usize;
        let mut files_deleted = 0usize;
        let mut files_failed = 0usize;
        let mut st = self.manager.lock_ui();
        let mut kept = Vec::new();
        for d in st.downloads.drain(..) {
            if delete_set.contains(&d.id) {
                removed += 1;
                if from_disk {
                    let part_dir = d.temp_dir.join(format!("d{}", d.id));
                    match std::fs::remove_file(&d.save_path) {
                        Ok(()) => files_deleted += 1,
                        Err(_) => files_failed += 1,
                    }
                    let _ = std::fs::remove_dir_all(part_dir);
                }
            } else {
                kept.push(d);
            }
        }
        st.downloads = kept;
        // The Trash category no longer exists; legacy records are gone.
        st.trash.clear();
        let _ = st.save();
        if from_disk {
            self.delete_confirm.from_disk = false;
        }
        drop(st);
        self.selected_download_id = None;
        if !from_disk {
            self.status_bar = format!("Removed {removed} download(s) from the list.");
        } else if files_failed == 0 {
            self.status_bar = format!(
                "Removed {removed} download(s); {files_deleted} file(s) deleted from disk."
            );
        } else {
            self.status_bar = format!(
                "Removed {removed} download(s); {files_deleted} file(s) deleted, {files_failed} could not be deleted (in use)."
            );
        }
    }

    fn start_queue_manual(&mut self, queue: &str) {
        if self.manager.queue_running() {
            self.status_bar = "Queue processing is already running.".into();
            return;
        }
        self.scheduler_suppressed.remove(queue);
        self.active_queue = Some(queue.to_string());
        self.manager.start_queue(queue.to_string());
        self.status_bar = format!("Started queue \"{queue}\".");
    }

    fn queue_for_toolbar_start(&self) -> String {
        let st = self.manager.lock_ui();
        self.selected_download_id
            .and_then(|id| {
                st.downloads
                    .iter()
                    .find(|download| download.id == id)
                    .and_then(|download| download.queue.clone())
            })
            .filter(|queue| !queue.trim().is_empty())
            .or_else(|| {
                let queue = st.settings.default_queue.trim();
                (!queue.is_empty()).then(|| queue.to_string())
            })
            .unwrap_or_else(|| MAIN_QUEUE.to_string())
    }

    fn stop_queue_manual(&mut self) {
        if !self.manager.queue_running() {
            self.status_bar = "Queue processing is not running.".into();
            return;
        }
        let queue = self
            .active_queue
            .clone()
            .unwrap_or_else(|| MAIN_QUEUE.to_string());
        self.manager.queue_stop();
        self.scheduler_suppressed.insert(queue.clone());
        self.scheduler_started.remove(&queue);
        self.active_queue = None;
        self.status_bar = format!("Stopped queue \"{queue}\".");
    }

    fn open_grabber(&mut self) {
        let st = self.manager.lock_ui();
        let default_queue = if st.settings.default_queue.trim().is_empty() {
            MAIN_QUEUE.to_string()
        } else {
            st.settings.default_queue.clone()
        };
        self.grabber_dialog.file_types = crate::grabber::default_types(&st);
        self.grabber_dialog.queue = default_queue;
        self.grabber_dialog.error.clear();
        self.grabber_dialog.report.clear();
        self.grabber_dialog.open = true;
    }

    fn start_grabber(&mut self) {
        if self.grabber_dialog.running {
            return;
        }
        let start = self.grabber_dialog.start_url.trim().to_string();
        match url::Url::parse(&start) {
            Ok(url) if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() => {}
            _ => {
                self.grabber_dialog.error =
                    "Enter a valid HTTP or HTTPS start address.".to_string();
                return;
            }
        }

        self.grabber_dialog.links.clear();
        self.grabber_dialog.selected.clear();
        self.grabber_dialog.error.clear();
        self.grabber_dialog.report.clear();
        self.grabber_dialog.pages_crawled.store(0, Ordering::Relaxed);
        if let Ok(mut result) = self.grabber_dialog.result.lock() {
            *result = None;
        } else {
            self.grabber_dialog.error =
                "Could not initialize the Site Grabber result state.".to_string();
            return;
        }

        let result = Arc::clone(&self.grabber_dialog.result);
        let pages_crawled = Arc::clone(&self.grabber_dialog.pages_crawled);
        let depth = self.grabber_dialog.depth.min(20);
        let same_origin = self.grabber_dialog.same_origin;
        let file_types = self.grabber_dialog.file_types.clone();
        let user_agent = self.manager.lock_ui().settings.user_agent.clone();
        self.grabber_dialog.running = true;
        self.grabber_dialog.task = Some(crate::manager::runtime().spawn(async move {
            let outcome = crate::grabber::crawl(
                &start,
                depth,
                same_origin,
                &file_types,
                &user_agent,
                pages_crawled,
            )
            .await
            .map_err(|error| format!("{error:#}"));
            if let Ok(mut slot) = result.lock() {
                *slot = Some(outcome);
            }
        }));
    }

    fn poll_grabber(&mut self) {
        if !self.grabber_dialog.running
            || !self.grabber_dialog.task.as_ref().is_some_and(|task| task.is_finished())
        {
            return;
        }
        self.grabber_dialog.task = None;
        self.grabber_dialog.running = false;
        let outcome = self
            .grabber_dialog
            .result
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        match outcome {
            Some(Ok(result)) => {
                let count = result.links.len();
                let failures = result.failed_pages.len();
                let failure_details = result
                    .failed_pages
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                self.grabber_dialog.links = result.links;
                self.grabber_dialog.report = if failures == 0 {
                    format!(
                        "Found {count} matching file(s) across {} page(s).",
                        result.pages_crawled
                    )
                } else {
                    format!(
                        "Found {count} matching file(s); {} of {} page(s) could not be read.\n{}",
                        failures, result.pages_crawled, failure_details
                    )
                };
                if failures > 0 {
                    crate::logfile::log(
                        "GRAB",
                        &format!(
                            "Site Grabber encountered {failures} page errors: {failure_details}"
                        ),
                    );
                }
            }
            Some(Err(error)) => self.grabber_dialog.error = error,
            None => self.grabber_dialog.error =
                "Site Grabber stopped without returning a result.".to_string(),
        }
    }

    fn cancel_grabber(&mut self) {
        if let Some(task) = self.grabber_dialog.task.take() {
            task.abort();
            self.grabber_dialog.running = false;
            self.grabber_dialog.report = "Crawl canceled.".to_string();
        }
    }

    fn add_grabber_selected(&mut self) {
        let selected: Vec<String> = self.grabber_dialog.selected.iter().cloned().collect();
        if selected.is_empty() {
            self.grabber_dialog.report = "Select at least one result to add.".to_string();
            return;
        }
        let queue = if self.grabber_dialog.queue.trim().is_empty() {
            MAIN_QUEUE.to_string()
        } else {
            self.grabber_dialog.queue.trim().to_string()
        };
        let mut st = self.manager.lock_ui();
        let old_max_id = st.max_id;
        let old_download_count = st.downloads.len();
        let queue_was_present = st.queues.iter().any(|name| name == &queue);
        if !queue_was_present {
            st.queues.push(queue.clone());
        }
        let mut added = 0;
        let mut skipped = 0;
        for url in selected {
            if st.downloads.iter().any(|download| download.url == url) {
                skipped += 1;
                continue;
            }
            let mut download = crate::new_rec(&mut st, &url, None, Some(&queue));
            download.description = "Added by Site Grabber".to_string();
            st.downloads.push(download);
            added += 1;
        }
        if let Err(error) = st.save() {
            st.downloads.truncate(old_download_count);
            st.max_id = old_max_id;
            if !queue_was_present {
                st.queues.retain(|name| name != &queue);
            }
            self.grabber_dialog.error = format!("Could not save grabbed links: {error}");
            return;
        }
        self.grabber_dialog.report = format!(
            "Added {added} download(s) to \"{queue}\"; skipped {skipped} duplicate(s)."
        );
    }

    fn queued_count_for(&self, queue: &str) -> usize {
        let st = self.manager.lock_ui();
        st.downloads
            .iter()
            .filter(|d| d.status == Status::Queued && d.queue.as_deref() == Some(queue))
            .count()
    }

    fn column_tracks(&self) -> Vec<ColumnTrack> {
        column_tracks(&self.column_widths, &self.column_visible)
    }

    fn save_column_settings(&mut self) {
        let mut state = self.manager.lock_ui();
        state.settings.list_col_widths = self.column_widths.clone();
        state.settings.list_col_visible = self.column_visible.clone();
        let _ = state.save();
    }

    /// Sample live transfer rates from the manager's byte counters. The
    /// display is smoothed over ~0.2 s samples so it stays readable.
    fn sample_speeds(&mut self) {
        let now = Instant::now();
        let active: Vec<u64> = {
            let st = self.manager.lock_ui();
            st.downloads
                .iter()
                .filter(|d| d.status == Status::Downloading)
                .map(|d| d.id)
                .collect()
        };
        let active_set: HashSet<u64> = active.iter().copied().collect();

        for id in active {
            let Some(stats) = self.manager.stats(id) else {
                continue;
            };
            let bytes = stats.bytes();
            match self.speed_samples.get(&id).copied() {
                Some((then, previous)) => {
                    let elapsed = now.duration_since(then);
                    if elapsed >= Duration::from_millis(200) {
                        let dt = elapsed.as_secs_f64().max(0.001);
                        let instant = bytes.saturating_sub(previous) as f64 / dt;
                        let smoothed = self
                            .speeds
                            .get(&id)
                            .copied()
                            .map(|value| value * 0.7 + instant * 0.3)
                            .unwrap_or(instant);
                        self.speeds.insert(id, smoothed.max(0.0));
                        self.speed_samples.insert(id, (now, bytes));
                    }
                }
                None => {
                    self.speed_samples.insert(id, (now, bytes));
                }
            }
        }

        self.speeds.retain(|id, _| active_set.contains(id));
        self.speed_samples.retain(|id, _| active_set.contains(id));
    }

    /// Drive scheduled queues from the GUI thread: start a queue while its
    /// schedule window is open and stop it once the window closes. A manual
    /// Stop suppresses the schedule until the next window transition.
    fn scheduler_tick(&mut self) {
        if !self.manager.queue_running() {
            if let Some(queue) = self.active_queue.take() {
                self.scheduler_started.remove(&queue);
            }
        }
        if self.scheduler_last_tick.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.scheduler_last_tick = Instant::now();

        let schedules = {
            let st = self.manager.lock_ui();
            st.schedules.clone()
        };
        if schedules.is_empty() {
            self.scheduler_last_window.clear();
            return;
        }

        let mut start_queue: Option<String> = None;
        for schedule in &schedules {
            let active = schedule.active_now();
            let was_active = self
                .scheduler_last_window
                .insert(schedule.queue.clone(), active)
                .unwrap_or(false);
            if active && !was_active {
                self.scheduler_suppressed.remove(&schedule.queue);
            }

            let already_ours =
                self.active_queue.as_deref() == Some(schedule.queue.as_str());
            if active
                && !self.scheduler_suppressed.contains(&schedule.queue)
                && !already_ours
                && !self.manager.queue_running()
                && self.queued_count_for(&schedule.queue) > 0
                && start_queue.is_none()
            {
                start_queue = Some(schedule.queue.clone());
            }

            if !active && already_ours {
                if self.manager.queue_running() {
                    self.manager.queue_stop();
                    self.status_bar = format!(
                        "Scheduler stopped queue \"{}\" (outside its start window).",
                        schedule.queue
                    );
                }
                self.active_queue = None;
            }
        }

        if let Some(queue) = start_queue {
            self.active_queue = Some(queue.clone());
            self.manager.start_queue(queue.clone());
            self.status_bar = format!("Scheduler started queue \"{queue}\".");
        }
    }

    /// Apply the active column sort to the filtered download list.
    fn sort_items(&self, items: &mut [DownloadRec]) {
        let column = self.sort_column;
        let descending = self.sort_descending;
        let remaining = |d: &DownloadRec| -> u64 {
            let speed = self.speeds.get(&d.id).copied().unwrap_or(0.0);
            if speed < 1.0 || d.file_size == 0 {
                return u64::MAX;
            }
            let left = d.file_size.saturating_sub(d.total_downloaded());
            (left as f64 / speed) as u64
        };
        let rate = |d: &DownloadRec| -> u64 {
            self.speeds.get(&d.id).copied().unwrap_or(0.0) as u64
        };
        items.sort_by(|a, b| {
            let order = match column {
                0 => file_name_of(a)
                    .to_lowercase()
                    .cmp(&file_name_of(b).to_lowercase()),
                1 => a
                    .queue
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .cmp(&b.queue.as_deref().unwrap_or("").to_lowercase()),
                2 => a.file_size.cmp(&b.file_size),
                3 => a.status.as_str().cmp(b.status.as_str()),
                4 => remaining(a).cmp(&remaining(b)),
                5 => rate(a).cmp(&rate(b)),
                6 => a.last_try_date.cmp(&b.last_try_date),
                7 => a
                    .description
                    .to_lowercase()
                    .cmp(&b.description.to_lowercase()),
                _ => a.id.cmp(&b.id),
            };
            if descending {
                order.reverse()
            } else {
                order
            }
        });
    }

    /// Small search popup anchored under the header's magnifier icon.
    fn render_search_popup(&mut self, ctx: &egui::Context) {
        if !self.search_open {
            return;
        }
        let mut open = true;
        let anchor = if self.search_icon_rect.width() > 0.0 {
            pos2(
                (self.search_icon_rect.left() - 170.0).max(8.0),
                self.search_icon_rect.bottom() + 4.0,
            )
        } else {
            pos2(180.0, 70.0)
        };

        egui::Area::new(egui::Id::new("dm-search-popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(bg_panel())
                    .stroke(Stroke::new(1.0, border_hairline()))
                    .rounding(Rounding::same(4.0))
                    .inner_margin(Margin::same(8.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let response = ui.add_sized(
                                [220.0, 22.0],
                                egui::TextEdit::singleline(&mut self.search_query)
                                    .hint_text("Search downloads"),
                            );
                            if self.search_focus {
                                response.request_focus();
                                self.search_focus = false;
                            }
                            if ui.button("X").clicked() {
                                open = false;
                            }
                        });
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("File name, URL, description or queue")
                                    .size(10.5)
                                    .color(text_dim()),
                            );
                            if !self.search_query.is_empty()
                                && ui.button("Clear").clicked()
                            {
                                self.search_query.clear();
                            }
                        });
                    });
            });

        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            open = false;
        }
        if !open {
            self.search_open = false;
        }
    }

    fn save_schedule(&mut self) {
        let queue = self.scheduler_dialog.queue.trim().to_string();
        let queue = if queue.is_empty() {
            MAIN_QUEUE.to_string()
        } else {
            queue
        };
        let start = match parse_optional_hm(&self.scheduler_dialog.start_time) {
            Ok(value) => value,
            Err(message) => {
                self.scheduler_dialog.error = message;
                return;
            }
        };
        let stop = match parse_optional_hm(&self.scheduler_dialog.stop_time) {
            Ok(value) => value,
            Err(message) => {
                self.scheduler_dialog.error = message;
                return;
            }
        };
        if start.is_some() && start == stop {
            self.scheduler_dialog.error =
                "Start and stop times cannot be the same.".into();
            return;
        }

        let mut days = self.scheduler_dialog.days.clone();
        days.sort_unstable();
        days.dedup();
        let on_finish = if self.scheduler_dialog.on_finish == "exit" {
            "exit"
        } else {
            "none"
        };
        let schedule = Schedule {
            queue: queue.clone(),
            start,
            stop,
            days,
            once: self.scheduler_dialog.once,
            on_finish: on_finish.into(),
        };

        let result = {
            let mut st = self.manager.lock_ui();
            st.schedules.retain(|s| s.queue != queue);
            st.schedules.push(schedule);
            if !st.queues.iter().any(|q| q == &queue) {
                st.queues.push(queue.clone());
            }
            st.save()
        };

        match result {
            Ok(()) => {
                self.scheduler_dialog.error.clear();
                self.scheduler_dialog.open = false;
                self.status_bar = format!("Saved schedule for queue \"{queue}\".");
            }
            Err(error) => {
                self.scheduler_dialog.error = format!("Could not save schedule: {error}");
            }
        }
    }

    fn clear_schedule(&mut self) {
        let queue = self.scheduler_dialog.queue.trim().to_string();
        let queue = if queue.is_empty() {
            MAIN_QUEUE.to_string()
        } else {
            queue
        };
        let result = {
            let mut st = self.manager.lock_ui();
            let before = st.schedules.len();
            st.schedules.retain(|s| s.queue != queue);
            let removed = before - st.schedules.len();
            let saved = st.save();
            (removed, saved)
        };

        match result {
            (removed, Ok(())) => {
                if self.active_queue.as_deref() == Some(queue.as_str()) {
                    if self.manager.queue_running() {
                        self.manager.queue_stop();
                    }
                    self.active_queue = None;
                }
                self.scheduler_dialog.start_time.clear();
                self.scheduler_dialog.stop_time.clear();
                self.scheduler_dialog.days.clear();
                self.scheduler_dialog.once = false;
                self.scheduler_dialog.on_finish = "none".into();
                self.scheduler_dialog.error.clear();
                self.status_bar = if removed == 0 {
                    format!("Queue \"{queue}\" has no schedule to clear.")
                } else {
                    format!("Cleared schedule for queue \"{queue}\".")
                };
            }
            (_, Err(error)) => {
                self.scheduler_dialog.error =
                    format!("Could not clear schedule: {error}");
            }
        }
    }

    fn show_about(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let mut open = true;
        let mut close_clicked = false;
        egui::Window::new("About DM Ultra")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(bg_panel())
                    .inner_margin(Margin::same(16.0)),
            )
            .show(ctx, |ui| {
                ui.set_min_width(300.0);
                ui.label(RichText::new("DM Ultra").size(20.0).strong().color(text_main()));
                ui.label(
                    RichText::new(format!("Version {}", APP_VERSION.trim()))
                        .size(12.0)
                        .color(text_dim()),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Multi-threaded download manager for Windows.")
                        .color(text_main()),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Local use only. No accounts, telemetry, or cloud services.")
                        .size(11.5)
                        .color(text_dim()),
                );
                ui.add_space(12.0);
                if ui.button("Close").clicked() {
                    close_clicked = true;
                }
            });
        self.about_open = open && !close_clicked;
    }

    fn count_for_category(&self, cat: NavCategory) -> usize {
        let st = self.manager.lock_ui();
        match cat {
            NavCategory::AllDownloads => st.downloads.len(),
            NavCategory::InProgress   => st.downloads.iter().filter(|d| d.status == Status::Downloading).count(),
            NavCategory::Completed    => st.downloads.iter().filter(|d| d.status == Status::Complete).count(),
            NavCategory::Queued       => st.downloads.iter().filter(|d| d.status == Status::Queued && !queue_has_schedule(&st, d.queue.as_deref())).count(),
            NavCategory::Paused       => st.downloads.iter().filter(|d| d.status == Status::Paused).count(),
            NavCategory::Documents    => st.downloads.iter().filter(|d| is_file_type(&d.save_path, &["pdf","doc","docx","txt","rtf","xls","xlsx","ppt","pptx"])).count(),
            NavCategory::Music        => st.downloads.iter().filter(|d| is_file_type(&d.save_path, &["mp3","wav","wma","m4a","aac","flac","ogg","aif"])).count(),
            NavCategory::Video        => st.downloads.iter().filter(|d| is_file_type(&d.save_path, &["mp4","mkv","avi","mov","wmv","flv","webm","3gp","mpeg","mpg"])).count(),
            NavCategory::Programs     => st.downloads.iter().filter(|d| is_file_type(&d.save_path, &["exe","msi","apk","iso","img","bin"])).count(),
            NavCategory::Compressed   => st.downloads.iter().filter(|d| is_file_type(&d.save_path, &["zip","rar","7z","tar","gz","bz2","xz"])).count(),
            NavCategory::Scheduled    => st.downloads.iter().filter(|d| d.status == Status::Queued && queue_has_schedule(&st, d.queue.as_deref())).count(),
        }
    }
}

// ---------------------------------------------------------------------------
// eframe::App implementation
// ---------------------------------------------------------------------------
impl eframe::App for DmApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // BASELINE PROFILER PATCH (measurement only, not part of the app):
        // time the whole frame so the drag fast path can be compared against
        // this build with identical instrumentation.
        let frame_start = Instant::now();
        let dragging = crate::browser::main_window_dragging();
        // A browser-launched instance must not show the main window. eframe
        // flips the window visible right after the first frame, so park it
        // again in that same frame (commands are applied before the event
        // loop returns, so there is no visible flash).
        //
        // "Park" means moving the window off-screen while keeping it
        // `WS_VISIBLE` and non-iconic - NOT `ShowWindow(SW_HIDE)` and NOT
        // eframe's `ViewportCommand::Visible(false)`. eframe paints the child
        // progress popouts from the root frame, and it stops scheduling
        // frames once the root is hidden or iconic, which freezes those
        // popouts. An off-screen but visible root keeps the normal frame
        // loop and the popouts fully interactive.
        if std::mem::take(&mut self.hide_main_window) {
            crate::browser::hide_main_window();
        }

        // Remember where the main window is (honours the setting).
        self.track_main_window(ctx);

        // Window behaviour: tray icon and the configured minimize action.
        let minimize_mode = {
            let mode = self.manager.lock_ui().settings.minimize_mode.clone();
            normalize_minimize_mode(&mode)
        };
        // Without a tray icon the tray modes would strand the window, so
        // fall back to a plain taskbar minimize.
        let effective_mode = if self.tray_icon.is_some() {
            minimize_mode
        } else {
            "taskbar"
        };

        // Hook the main window once. The hook turns the minimize button into
        // an off-screen park instead of an iconic window, because eframe
        // refuses to schedule frames for iconic windows and the progress
        // popouts are painted by the root frame.
        crate::browser::install_main_hook();
        crate::browser::set_main_window_mode(effective_mode);

        // The waker delivers the frame that notices a minimize; eframe cannot
        // schedule one by itself for an iconic window. With the park hook this
        // is only a safety net.
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        let has_child = !self.active_details_popouts.is_empty()
            || self.add_dialog.open
            || self.settings_dialog.open
            || self.scheduler_dialog.open
            || self.grabber_dialog.running
            || self.about_open
            || self.delete_confirm.open;
        crate::browser::set_popout_open(has_child);
        self.minimize_watch.store(
            (minimized || effective_mode != "taskbar") && self.show_main_window,
            std::sync::atomic::Ordering::Relaxed,
        );

        // Showing from the tray (left/double click or the menu item), or
        // when a second launch asks this instance to surface.
        let mut show_requested = self
            .show_flag
            .lock()
            .map(|mut flag| std::mem::take(&mut *flag))
            .unwrap_or(false);
        if !show_requested && self.last_show_poll.elapsed() > Duration::from_millis(300) {
            self.last_show_poll = Instant::now();
            let path = show_request_path();
            if path.exists() {
                let _ = std::fs::remove_file(&path);
                show_requested = true;
            }
        }
        if show_requested {
            self.show_main_window = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            // Undo the raw Win32 hide (eframe already believes it is visible).
            crate::browser::show_main_window();
            crate::logfile::log("TRAY", "main window restored from the tray");
        }

        // Close button: hide to the tray when one exists, otherwise quit.
        if ctx.input(|i| i.viewport().close_requested()) && effective_mode != "taskbar" {
            self.show_main_window = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            crate::browser::hide_main_window();
            crate::logfile::log("TRAY", "close button hid the main window to the tray");
        }

        // Minimize button: tray modes park the window instead of letting it
        // become iconic (the hook normally intercepts this before winit sees
        // it; this branch is the fallback). Plain taskbar mode parks as well
        // while a popout is open, because the popout needs the root frame.
        if minimized && !self.was_minimized && self.show_main_window {
            if effective_mode == "tray" {
                self.show_main_window = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                crate::browser::hide_main_window();
                crate::logfile::log("TRAY", "minimize button hid the main window to the tray");
            } else if effective_mode == "taskbar_tray" || has_child {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                crate::browser::park_main_window_keeping_taskbar();
                crate::logfile::log("TRAY", "minimize button parked the main window off-screen");
            }
        }
        self.was_minimized = minimized && self.show_main_window;

        // Safety net: whenever the app state says the main window is hidden,
        // make sure it really is Win32-hidden, while never letting eframe know
        // (see the note at the top of `update`). Idempotent and cheap.
        if !self.show_main_window {
            crate::browser::hide_main_window();
        }

        // The tray icon is present for both tray modes and whenever the
        // main window is hidden; "taskbar" mode keeps it out of the tray.
        let want_tray = !self.show_main_window || effective_mode != "taskbar";
        if self.tray_visible != want_tray {
            if let Some(tray) = &self.tray_icon {
                if let Err(error) = tray.set_visible(want_tray) {
                    crate::logfile::log(
                        "TRAY",
                        &format!("could not change tray icon visibility: {error}"),
                    );
                }
            }
            self.tray_visible = want_tray;
        }

        // A hidden window receives no paint from Windows, so scheduled
        // repaints cannot run there; `browser::spawn_busy_waker` posts a slow
        // synthetic resize to keep the tray click and a second-instance
        // request observable.

        // Reap finished tasks periodically; active transfers keep the UI
        // repainting frequently enough for progress to look continuous.
        if self.last_update.elapsed() > Duration::from_millis(500) {
            self.manager.reap();
            self.last_update = Instant::now();
        }
        if self.manager.active_count() > 0 {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.grabber_dialog.running {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        // Resolve the active appearance from settings and publish it for the
        // dynamic color helpers used throughout the main window.
        let settings_snapshot = self.manager.lock_ui().settings.clone();
        let system_dark = options::system_prefers_dark();
        let palette = options::Appearance::from_settings(&settings_snapshot, system_dark).palette();
        if let Ok(mut guard) = CURRENT_PALETTE.lock() {
            *guard = Some(palette);
        }
        self.accent = ui_accent(&settings_snapshot.accent);

        // Apply styling to egui context
        apply_app_style(ctx);

        self.poll_browser_pending(ctx);

        // The busy/child-window flag is published at the END of the frame,
        // after every popout has been rendered (see below). Computing it here
        // would miss a window opened during this frame, which is exactly when
        // the wake pulse is needed most.
        // Clipboard watcher: when enabled, copying a download link opens
        // the Add-URL window and remains active while the app is idle.
        if settings_snapshot.monitor_clipboard {
            ctx.request_repaint_after(Duration::from_millis(500));
            self.tick_clipboard_watcher(ctx, &settings_snapshot);
        } else {
            self.clip_last_poll = Instant::now() - Duration::from_secs(5);
        }

        // Live transfer-rate sampling and schedule enforcement run before
        // the frame is built so the table and status bar show current data.
        self.sample_speeds();
        self.scheduler_tick();

        // Poll the video overlay heartbeat written by the native messaging
        // host. The overlay module owns the Win32 button thread.
        #[cfg(windows)]
        {
            let overlay_path = crate::state::state_dir().join("pending").join("overlay.json");
            let mut fed = false;
            if let Ok(meta) = std::fs::metadata(&overlay_path) {
                if let Ok(modified) = meta.modified() {
                    if modified.elapsed().unwrap_or(std::time::Duration::from_secs(999))
                        < std::time::Duration::from_secs(6)
                    {
                        if let Ok(data) = std::fs::read_to_string(&overlay_path) {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                                let url = v.get("url").and_then(|s| s.as_str()).unwrap_or("");
                                let referer = v.get("referer").and_then(|s| s.as_str()).unwrap_or("");
                                let title = v.get("title").and_then(|s| s.as_str()).unwrap_or("");
                                let format = v.get("format").and_then(|s| s.as_str()).unwrap_or("");
                                let x = v.get("x").and_then(|n| n.as_i64()).unwrap_or(0) as i32;
                                let y = v.get("y").and_then(|n| n.as_i64()).unwrap_or(0) as i32;
                                let w = v.get("w").and_then(|n| n.as_i64()).unwrap_or(0) as i32;
                                let h = v.get("h").and_then(|n| n.as_i64()).unwrap_or(0) as i32;
                                let mut qualities = Vec::new();
                                if let Some(qs) = v.get("qualities").and_then(|q| q.as_array()) {
                                    for q in qs {
                                        let label = q.get("label").and_then(|s| s.as_str()).unwrap_or("");
                                        let u = q.get("url").and_then(|s| s.as_str()).unwrap_or("");
                                        if !u.is_empty() {
                                            qualities.push((label.to_string(), u.to_string()));
                                        }
                                    }
                                }
                                crate::overlay::update_overlay(
                                    url, referer, title, format, qualities, x, y, w, h,
                                );
                                fed = true;
                            }
                        }
                    }
                }
            }
            if !fed {
                crate::overlay::hide_overlay();
            }
            crate::overlay::tick();
        }

        // -------------------------------------------------------------------
        // 1. Menu Bar (Tasks, File, Downloads, View, Help)
        // -------------------------------------------------------------------
        egui::TopBottomPanel::top("menubar")
            .exact_height(27.0)
            .frame(egui::Frame::none().fill(bg_dark()).inner_margin(Margin::symmetric(8.0, 3.0)))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                egui::menu::bar(ui, |ui| {
                    ui.menu_button(RichText::new("Tasks").color(text_main()), |ui| {
                        ui.menu_button("Start Queue Processing", |ui| {
                            let mut queues = self.manager.lock_ui().queues.clone();
                            if !queues.iter().any(|name| name == MAIN_QUEUE) {
                                queues.push(MAIN_QUEUE.to_string());
                            }
                            for queue in queues {
                                if ui.button(&queue).clicked() {
                                    self.start_queue_manual(&queue);
                                    ui.close_menu();
                                }
                            }
                        });
                        if ui
                            .add_enabled(
                                self.manager.queue_running(),
                                egui::Button::new("Stop Queue Processing"),
                            )
                            .clicked()
                        {
                            self.stop_queue_manual();
                        }
                        ui.separator();
                        if ui.button("Site Grabber...").clicked() {
                            self.open_grabber();
                        }
                        ui.separator();
                        if ui.button("Exit").clicked() {
                            std::process::exit(0);
                        }
                    });

                    ui.menu_button(RichText::new("File").color(text_main()), |ui| {
                        if ui.button("Open Save Folder").clicked() {
                            let save_dir = self.manager.lock_ui().settings.save_path.clone();
                            let _ = std::process::Command::new("explorer").arg(save_dir).spawn();
                        }
                        ui.separator();
                        if ui.button("Export Download List...").clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .set_file_name("downloads.json")
                                .save_file()
                            {
                                let st = self.manager.lock_ui();
                                let data = serde_json::to_string_pretty(&st.downloads).unwrap_or_default();
                                drop(st);
                                let _ = std::fs::write(&path, data);
                                self.status_bar = format!("Exported download list to {}.", path.display());
                            }
                            ui.close_menu();
                        }
                        if ui.button("Import Download List...").clicked() {
                            if let Some(path) = rfd::FileDialog::new().pick_file() {
                                if let Ok(data) = std::fs::read_to_string(&path) {
                                    if let Ok(list) = serde_json::from_str::<Vec<crate::state::DownloadRec>>(&data) {
                                        let mut st = self.manager.lock_ui();
                                        let mut imported = 0usize;
                                        for mut rec in list {
                                            rec.id = st.next_id();
                                            rec.status = crate::state::Status::Paused;
                                            st.downloads.push(rec);
                                            imported += 1;
                                        }
                                        let _ = st.save();
                                        self.status_bar = format!("Imported {imported} download(s).");
                                    } else {
                                        self.status_bar = "Could not parse download list.".into();
                                    }
                                }
                            }
                            ui.close_menu();
                        }
                    });

                    ui.menu_button(RichText::new("Downloads").color(text_main()), |ui| {
                        if ui.button("Add URL...").clicked() {
                            self.open_add_url();
                        }
                        ui.separator();
                        if ui.button("Start / Resume").clicked() {
                            self.start_selected();
                        }
                        if ui.button("Pause").clicked() {
                            self.pause_selected();
                        }
                        if ui.button("Stop All").clicked() {
                            self.stop_all();
                        }
                        if ui.button("Resume All").clicked() {
                            self.resume_all();
                        }
                        ui.separator();
                        if ui.button("Delete").clicked() {
                            self.delete_selected();
                        }
                        if ui.button("Delete All").clicked() {
                            self.delete_all();
                        }
                        ui.separator();
                        if ui.button("Options...").clicked() {
                            self.open_settings();
                        }
                        if ui.button("Scheduler...").clicked() {
                            self.open_scheduler();
                        }
                    });

                    ui.menu_button(RichText::new("View").color(text_main()), |ui| {
                        let cat_label = if self.categories_visible { "Hide Categories" } else { "Show Categories" };
                        if ui.button(cat_label).clicked() {
                            self.categories_visible = !self.categories_visible;
                        }
                        ui.separator();
                        ui.menu_button("Columns", |ui| {
                            for (index, label) in COLUMN_LABELS.iter().enumerate() {
                                let mut visible = self
                                    .column_visible
                                    .get(index)
                                    .copied()
                                    .unwrap_or(true);
                                if ui.checkbox(&mut visible, *label).changed() {
                                    if !visible
                                        && self.column_visible.iter().filter(|v| **v).count() <= 1
                                    {
                                        visible = true;
                                    }
                                    if let Some(slot) = self.column_visible.get_mut(index) {
                                        *slot = visible;
                                    }
                                    self.save_column_settings();
                                }
                            }
                        });
                    });

                    ui.menu_button(RichText::new("Help").color(text_main()), |ui| {
                        if ui.button("About DM Ultra").clicked() {
                            self.about_open = true;
                        }
                    });
                });
            });

        // -------------------------------------------------------------------
        // 2. Main Large Toolbar (Add URL, Start, Pause, Stop All, Resume All...)
        // -------------------------------------------------------------------
        egui::TopBottomPanel::top("toolbar")
            .exact_height(64.0)
            .frame(
                egui::Frame::none()
                    .fill(bg_panel())
                    .inner_margin(Margin::symmetric(6.0, 0.0)),
            )
            .show(ctx, |ui| {
                // Twelve command buttons share one compact left-aligned row,
                // split into command groups by two hairline separators.
                let button_w = ((ui.available_width() - 72.0) / 12.0).clamp(56.0, 72.0);
                ui.spacing_mut().item_spacing = Vec2::ZERO;

                ui.horizontal_centered(|ui| {
                    ui.add_space(4.0);

                    // Add URL (accent plus)
                    let icon_add = self.icons.get_or_render(ctx, "add", SVG_ADD, self.accent, 34);
                    if render_toolbar_button(ui, &icon_add, "Add URL", self.accent, button_w) {
                        self.open_add_url();
                    }

                    toolbar_separator(ui);

                    // Start
                    let icon_start = self.icons.get_or_render(ctx, "start", SVG_START, text_dim(), 34);
                    if render_toolbar_button(ui, &icon_start, "Start", text_dim(), button_w) {
                        self.start_selected();
                    }

                    // Pause
                    let icon_pause = self.icons.get_or_render(ctx, "pause", SVG_PAUSE, text_dim(), 34);
                    if render_toolbar_button(ui, &icon_pause, "Pause", text_dim(), button_w) {
                        self.pause_selected();
                    }

                    // Stop All
                    let icon_stop = self.icons.get_or_render(ctx, "stop", SVG_STOP, text_dim(), 34);
                    if render_toolbar_button(ui, &icon_stop, "Stop All", text_dim(), button_w) {
                        self.stop_all();
                    }

                    // Resume All (green play)
                    let icon_resume = self.icons.get_or_render(ctx, "resume", SVG_RESUME_ALL, ACCENT_GREEN, 34);
                    if render_toolbar_button(ui, &icon_resume, "Resume All", ACCENT_GREEN, button_w) {
                        self.resume_all();
                    }

                    // Delete
                    let icon_del = self.icons.get_or_render(ctx, "delete", SVG_DELETE, text_dim(), 34);
                    if render_toolbar_button(ui, &icon_del, "Delete", text_dim(), button_w) {
                        self.delete_selected();
                    }

                    // Delete All
                    let icon_del_all = self.icons.get_or_render(ctx, "delete_all", SVG_DELETE_ALL, ACCENT_RED, 34);
                    if render_toolbar_button(ui, &icon_del_all, "Delete All", ACCENT_RED, button_w) {
                        self.delete_all();
                    }

                    toolbar_separator(ui);

                    // Options
                    let icon_opt = self.icons.get_or_render(ctx, "options", SVG_OPTIONS, text_dim(), 34);
                    if render_toolbar_button(ui, &icon_opt, "Options", text_dim(), button_w) {
                        self.open_settings();
                    }

                    // Scheduler
                    let icon_sched = self.icons.get_or_render(ctx, "scheduler", SVG_SCHEDULER, text_dim(), 34);
                    if render_toolbar_button(ui, &icon_sched, "Scheduler", text_dim(), button_w) {
                        self.open_scheduler();
                    }

                    // IDM places queue controls and the site grabber after Scheduler.
                    let queue = self.queue_for_toolbar_start();
                    let icon_queue_start = self.icons.get_or_render(
                        ctx,
                        "queue_start",
                        SVG_RESUME_ALL,
                        ACCENT_GREEN,
                        34,
                    );
                    if render_toolbar_button(
                        ui,
                        &icon_queue_start,
                        "Start Queue",
                        ACCENT_GREEN,
                        button_w,
                    ) {
                        self.start_queue_manual(&queue);
                    }

                    let icon_queue_stop = self.icons.get_or_render(
                        ctx,
                        "queue_stop",
                        SVG_STOP_ALL,
                        text_dim(),
                        34,
                    );
                    if render_toolbar_button_enabled(
                        ui,
                        &icon_queue_stop,
                        "Stop Queue",
                        text_dim(),
                        button_w,
                        self.manager.queue_running(),
                    ) {
                        self.stop_queue_manual();
                    }

                    let icon_grabber = self.icons.get_or_render(
                        ctx,
                        "site_grabber",
                        SVG_SITE_GRABBER,
                        text_dim(),
                        34,
                    );
                    if render_toolbar_button(
                        ui,
                        &icon_grabber,
                        "Site Grabber",
                        text_dim(),
                        button_w,
                    ) {
                        self.open_grabber();
                    }
                });

                // Subtle bottom rule instead of a full frame border.
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.max_rect().bottom(),
                    Stroke::new(1.0, border_hairline()),
                );
            });

        // -------------------------------------------------------------------
        // 2b. Post-download power-action banner (cancellable countdown)
        // -------------------------------------------------------------------
        if let Some((action, rem)) = crate::postdownload::power_pending() {
            egui::TopBottomPanel::bottom("postaction_banner")
                .exact_height(28.0)
                .frame(
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgb(0x3a, 0x1f, 0x1f))
                        .inner_margin(Margin::symmetric(7.0, 0.0)),
                )
                .show(ctx, |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "Power action '{action}' in {rem}s"
                            ))
                            .size(11.5)
                            .color(egui::Color32::from_rgb(0xff, 0xcc, 0xcc)),
                        );
                        if ui.button("Cancel").clicked() {
                            crate::postdownload::cancel_power();
                        }
                    });
                });
            ctx.request_repaint_after(Duration::from_millis(250));
        }

        // -------------------------------------------------------------------
        // 3. Status Bar (Bottom)
        // -------------------------------------------------------------------
        egui::TopBottomPanel::bottom("statusbar")
            .exact_height(25.0)
            .frame(
                egui::Frame::none()
                    .fill(bg_dark())
                    .inner_margin(Margin::symmetric(7.0, 0.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new(&self.status_bar).size(11.5).color(text_dim()));
                });
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.max_rect().top(),
                    Stroke::new(1.0, border_hairline()),
                );
            });

        // -------------------------------------------------------------------
        // 4. Left Categories Sidebar
        // -------------------------------------------------------------------
        if self.categories_visible {
            egui::SidePanel::left("categories_sidebar")
                .exact_width(146.0)
                .resizable(false)
                .frame(
                    egui::Frame::none()
                        .fill(bg_panel())
                        .inner_margin(Margin::symmetric(6.0, 6.0)),
                )
                .show(ctx, |ui| {
                    // Sidebar rows are 28 px; the default 3 px inter-row
                    // gap pushed into the Paused row of the queue section.
                    ui.spacing_mut().item_spacing.y = 0.0;

                    // Header: Categories with [x] close button
                    ui.horizontal(|ui| {
                        ui.add_space(6.0);
                        ui.label(RichText::new("Categories").size(12.5).strong().color(text_dim()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let close = egui::Button::new(RichText::new("X").size(11.0).color(text_dim()))
                                .fill(bg_badge())
                                .rounding(Rounding::same(2.0))
                                .min_size(Vec2::new(18.0, 18.0));
                            if ui.add(close).on_hover_text("Hide categories").clicked() {
                                self.categories_visible = false;
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(2.0);

                    // Category items list
                    let main_cats = [
                        (NavCategory::AllDownloads, SVG_FOLDER, "folder"),
                        (NavCategory::InProgress,   SVG_PROGRESS, "progress"),
                        (NavCategory::Completed,    SVG_CHECK, "check"),
                        (NavCategory::Documents,    SVG_DOC, "doc"),
                        (NavCategory::Music,        SVG_MUSIC, "music"),
                        (NavCategory::Video,        SVG_VIDEO, "video"),
                        (NavCategory::Programs,     SVG_PROGRAMS, "programs"),
                        (NavCategory::Compressed,   SVG_ARCHIVE, "compressed"),
                    ];

                    for (cat, svg, key) in main_cats {
                        let selected = self.selected_nav == cat;
                        let count = self.count_for_category(cat);
                        let icon_color = if selected { self.accent } else { text_dim() };
                        let icon = self.icons.get_or_render(ctx, key, svg, icon_color, 16);

                        if render_sidebar_item(ui, &icon, cat.label(), count, selected, self.accent) {
                            self.selected_nav = cat;
                        }
                    }

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // Queue section header
                    ui.horizontal(|ui| {
                        ui.add_space(6.0);
                        ui.label(RichText::new("Queue").size(12.0).strong().color(text_dim()));
                    });
                    ui.add_space(4.0);

                    let queue_cats = [
                        (NavCategory::Scheduled, SVG_SCHEDULER, "sched_cat"),
                        (NavCategory::Queued,    SVG_QUEUE,     "queue_cat"),
                        (NavCategory::Paused,    SVG_PAUSE,     "pause_cat"),
                    ];

                    for (cat, svg, key) in queue_cats {
                        let selected = self.selected_nav == cat;
                        let count = self.count_for_category(cat);
                        let icon_color = if selected { self.accent } else { text_dim() };
                        let icon = self.icons.get_or_render(ctx, key, svg, icon_color, 16);

                        if render_sidebar_item(ui, &icon, cat.label(), count, selected, self.accent) {
                            self.selected_nav = cat;
                        }
                    }

                    // Thin divider between sidebar and download list (the
                    // panel frame contributes 6 px of inner margin).
                    ui.painter().vline(
                        ui.max_rect().right() + 6.0,
                        ui.max_rect().y_range(),
                        Stroke::new(1.0, border_hairline()),
                    );
                });
        }

        // -------------------------------------------------------------------
        // 5. Central Table (Downloads list or Empty state)
        // -------------------------------------------------------------------
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(bg_dark()))
            .show(ctx, |ui| {
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Header and rows share fixed tracks; the list scrolls
                        // horizontally whenever their total width exceeds the viewport.
                        // Snapshot one layout for the entire frame so header
                        // and body boundaries stay identical during a drag.
                        let column_tracks = self.column_tracks();
                        let table_inner_width: f32 =
                            column_tracks.iter().map(|track| track.width).sum();

                // Table header (single compact 27 px row)
                egui::Frame::none()
                    .fill(bg_header())
                    .show(ui, |ui| {
                        ui.set_height(27.0);
                        ui.set_min_width(table_inner_width);
                        ui.spacing_mut().item_spacing.x = 0.0;
                        // Column-boundary rules: (x edge, hovered, dragging).
                        let mut bounds: Vec<(f32, bool, bool)> = Vec::new();
                        ui.horizontal(|ui| {
                            let mut header_cells = Vec::with_capacity(column_tracks.len());
                            for track in &column_tracks {
                                let has_search = track.column == 0;
                                let (clicked, cell_rect) = self.table_header_cell(
                                    ui,
                                    COLUMN_LABELS[track.column],
                                    track.column,
                                    track.width,
                                    has_search,
                                );
                                if clicked {
                                    self.toggle_sort(track.column);
                                }
                                header_cells.push((*track, cell_rect, has_search));
                            }

                            let search_icon = self.icons.get_or_render(
                                ctx,
                                "search",
                                SVG_SEARCH,
                                text_dim(),
                                14,
                            );
                            for (track, cell_rect, has_search) in header_cells {
                                if has_search {
                                    let icon_rect = Rect::from_center_size(
                                        pos2(cell_rect.right() - 17.0, cell_rect.center().y),
                                        Vec2::splat(14.0),
                                    );
                                    ui.painter().image(
                                        search_icon.id(),
                                        icon_rect,
                                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                        Color32::WHITE,
                                    );
                                    self.search_icon_rect = icon_rect;
                                    let search_rect = Rect::from_center_size(
                                        icon_rect.center(),
                                        Vec2::new(22.0, 24.0),
                                    );
                                    let search_response = ui
                                        .interact(
                                            search_rect,
                                            egui::Id::new("download-list-search"),
                                            Sense::click(),
                                        )
                                        .on_hover_text("Search downloads")
                                        .on_hover_cursor(CursorIcon::Default);
                                    if search_response.clicked() {
                                        self.search_open = !self.search_open;
                                        self.search_focus = self.search_open;
                                    }
                                }

                                // Register resize hit-zones after header
                                // interactions so the handles win at edges.
                                let boundary_x = cell_rect.right();
                                let handle_rect = Rect::from_center_size(
                                    pos2(boundary_x, cell_rect.center().y),
                                    Vec2::new(COLUMN_RESIZE_HIT_WIDTH, 27.0),
                                );
                                let handle_resp = ui
                                    .interact(
                                        handle_rect,
                                        egui::Id::new(("download-column-resize", track.column)),
                                        Sense::click_and_drag(),
                                    )
                                    .on_hover_cursor(CursorIcon::ResizeHorizontal);
                                bounds.push((
                                    boundary_x,
                                    handle_resp.hovered(),
                                    handle_resp.dragged(),
                                ));
                                if handle_resp.dragged() {
                                    self.column_widths[track.column] = resized_column_width(
                                        self.column_widths[track.column],
                                        handle_resp.drag_delta().x,
                                        track.column,
                                    );
                                    ctx.request_repaint();
                                }
                                if handle_resp.drag_stopped() {
                                    self.save_column_settings();
                                }
                            }
                        });

                        // Grid rules between columns, spanning the full header
                        // height. Hovering/dragging a boundary highlights it.
                        for &(x, hovered, dragging) in &bounds {
                            let active = hovered || dragging;
                            let stroke = if active {
                                Stroke::new(2.0, text_main())
                            } else {
                                Stroke::new(1.0, grid_line())
                            };
                            ui.painter().vline(x, ui.max_rect().y_range(), stroke);
                        }

                        ui.painter().hline(
                            ui.min_rect().x_range(),
                            ui.max_rect().bottom(),
                            Stroke::new(1.0, border_hairline()),
                        );
                    });

                // Get visible downloads based on selected category & search filter
                let query = self.search_query.trim().to_lowercase();
                let mut items: Vec<DownloadRec> = {
                    let st = self.manager.lock_ui();
                    let scheduled_queues: HashSet<&str> = st
                        .schedules
                        .iter()
                        .map(|schedule| schedule.queue.as_str())
                        .collect();

                    st.downloads.iter().filter(|d| {
                        let is_scheduled = d
                            .queue
                            .as_deref()
                            .map(|queue| scheduled_queues.contains(queue))
                            .unwrap_or(false);
                        // Category filter
                        match self.selected_nav {
                            NavCategory::AllDownloads => true,
                            NavCategory::InProgress   => d.status == Status::Downloading,
                            NavCategory::Completed    => d.status == Status::Complete,
                            NavCategory::Queued       => d.status == Status::Queued && !is_scheduled,
                            NavCategory::Paused       => d.status == Status::Paused,
                            NavCategory::Documents    => is_file_type(&d.save_path, &["pdf","doc","docx","txt","rtf","xls","xlsx","ppt","pptx"]),
                            NavCategory::Music        => is_file_type(&d.save_path, &["mp3","wav","wma","m4a","aac","flac","ogg","aif"]),
                            NavCategory::Video        => is_file_type(&d.save_path, &["mp4","mkv","avi","mov","wmv","flv","webm","3gp","mpeg","mpg"]),
                            NavCategory::Programs     => is_file_type(&d.save_path, &["exe","msi","apk","iso","img","bin"]),
                            NavCategory::Compressed   => is_file_type(&d.save_path, &["zip","rar","7z","tar","gz","bz2","xz"]),
                            NavCategory::Scheduled    => d.status == Status::Queued && is_scheduled,
                        }
                    }).filter(|d| {
                        // Search filter: file name, URL, description, queue.
                        if query.is_empty() {
                            return true;
                        }
                        let haystack = format!(
                            "{} {} {} {}",
                            file_name_of(d),
                            d.url,
                            d.description,
                            d.queue.as_deref().unwrap_or("")
                        )
                        .to_lowercase();
                        haystack.contains(&query)
                    }).cloned().collect()
                };

                self.sort_items(&mut items);

                if items.is_empty() {
                    let visible_rect = ui.clip_rect();
                    ui.allocate_ui_at_rect(visible_rect, |ui| {
                        let block_height = 46.0;
                        let top_gap = ((ui.available_height() - block_height) * 0.5).max(0.0);
                        ui.add_space(top_gap);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("No downloads").size(18.0).color(text_empty_title()));
                            ui.add_space(5.0);
                            let subtitle = if query.is_empty() {
                                "Click Add URL on the toolbar to start downloading."
                            } else {
                                "No downloads match your search."
                            };
                            ui.label(RichText::new(subtitle).size(13.0).color(text_empty_sub()));
                        });
                    });
                } else {
                    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                        // Flush rows so the horizontal rules connect into a grid.
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (row_idx, d) in items.iter().enumerate() {
                            let is_selected = self.selected_download_id == Some(d.id);
                            let bg_color = if is_selected {
                                bg_row_selected()
                            } else if row_idx % 2 == 0 {
                                bg_row_even()
                            } else {
                                bg_row_odd()
                            };

                            let live_stats = self.manager.stats(d.id);
                            let total_bytes = if d.file_size > 0 { d.file_size } else { live_stats.as_ref().map(|s| s.total()).unwrap_or(0) };
                            let downloaded_bytes = live_stats.as_ref().map(|s| s.bytes()).unwrap_or_else(|| d.total_downloaded());
                            let speed_bps = self
                                .speeds
                                .get(&d.id)
                                .copied()
                                .unwrap_or(0.0)
                                .round() as u64;

                            let mut row_bounds: Vec<f32> = Vec::new();
                            let row_resp = egui::Frame::none()
                                .fill(bg_color)
                                .inner_margin(Margin::symmetric(0.0, 5.0))
                                .show(ui, |ui| {
                                    ui.set_min_width(table_inner_width);
                                    ui.spacing_mut().item_spacing.x = 0.0;
                                    ui.horizontal(|ui| {
                                        for track in &column_tracks {
                                            let column = track.column;
                                            let width = track.width;
                                            let column_left = ui.cursor().min.x;
                                            match column {
                                                0 => {
                                                    let name = d
                                                        .save_path
                                                        .file_name()
                                                        .unwrap_or_default()
                                                        .to_string_lossy();
                                                    cell_label_left(
                                                        ui,
                                                        width,
                                                        egui::Label::new(
                                                            RichText::new(name.as_ref())
                                                                .size(12.5)
                                                                .color(text_main()),
                                                        )
                                                        .truncate(true),
                                                    );
                                                }
                                                1 => {
                                                    let q_str = d.queue.as_deref().unwrap_or("");
                                                    cell_label_left(
                                                        ui,
                                                        width,
                                                        egui::Label::new(
                                                            RichText::new(q_str).size(11.0).color(text_dim()),
                                                        )
                                                        .truncate(true),
                                                    );
                                                }
                                                2 => {
                                                    let size_str = if total_bytes > 0 {
                                                        human_bytes(total_bytes)
                                                    } else {
                                                        "Unknown".into()
                                                    };
                                                    cell_label_left(
                                                        ui,
                                                        width,
                                                        egui::Label::new(
                                                            RichText::new(size_str).size(12.0).color(text_dim()),
                                                        )
                                                        .truncate(true),
                                                    );
                                                }
                                                3 => {
                                                    let status_color = match d.status {
                                                        Status::Downloading => status_downloading(),
                                                        Status::Complete => status_complete(),
                                                        Status::Paused => status_paused(),
                                                        Status::Failed => status_failed(),
                                                        Status::Queued => text_dim(),
                                                    };
                                                    cell_label_left(
                                                        ui,
                                                        width,
                                                        egui::Label::new(
                                                            RichText::new(d.status.as_str())
                                                                .size(12.0)
                                                                .color(status_color),
                                                        )
                                                        .truncate(true),
                                                    );
                                                }
                                                4 => {
                                                    let time_left =
                                                        if d.status == Status::Downloading && speed_bps > 0 {
                                                            let rem = total_bytes.saturating_sub(downloaded_bytes);
                                                            format_eta(rem, speed_bps)
                                                        } else {
                                                            "--".into()
                                                        };
                                                    cell_label_left(
                                                        ui,
                                                        width,
                                                        egui::Label::new(
                                                            RichText::new(time_left).size(12.0).color(text_dim()),
                                                        )
                                                        .truncate(true),
                                                    );
                                                }
                                                5 => {
                                                    let rate_str =
                                                        if d.status == Status::Downloading && speed_bps > 0 {
                                                            format!("{}/s", human_bytes(speed_bps))
                                                        } else {
                                                            "--".into()
                                                        };
                                                    cell_label_left(
                                                        ui,
                                                        width,
                                                        egui::Label::new(
                                                            RichText::new(rate_str).size(12.0).color(text_dim()),
                                                        )
                                                        .truncate(true),
                                                    );
                                                }
                                                6 => {
                                                    let date_str = if d.last_try_date > 0 {
                                                        format_timestamp(d.last_try_date)
                                                    } else {
                                                        "".into()
                                                    };
                                                    cell_label_left(
                                                        ui,
                                                        width,
                                                        egui::Label::new(
                                                            RichText::new(date_str).size(12.0).color(text_dim()),
                                                        )
                                                        .truncate(true),
                                                    );
                                                }
                                                7 => {
                                                    cell_label_left(
                                                        ui,
                                                        width,
                                                        egui::Label::new(
                                                            RichText::new(&d.description)
                                                                .size(12.0)
                                                                .color(text_dim()),
                                                        )
                                                        .truncate(true),
                                                    );
                                                }
                                                _ => {}
                                            }

                                            row_bounds.push(column_left + width);
                                        }
                                    });
                                });

                            // IDM-style grid rules: vertical cell separators for this
                            // row and a rule under it, spanning the full row height.
                            let rect = row_resp.response.rect;
                            for &x in &row_bounds {
                                ui.painter().vline(x, rect.y_range(), Stroke::new(1.0, grid_line()));
                            }
                            ui.painter().hline(
                                rect.x_range(),
                                rect.bottom(),
                                Stroke::new(1.0, grid_line()),
                            );

                            // Handle selection and double-click to open popout.
                            // Use an interact on the full row rect for reliable click/double-click.
                            let row_interact = ui.interact(rect, egui::Id::new(("download_row", d.id)), Sense::click());
                            if row_interact.clicked() {
                                self.selected_download_id = Some(d.id);
                            }
                            if row_interact.double_clicked() {
                                if !self.active_details_popouts.contains(&d.id) {
                                    self.active_details_popouts.push(d.id);
                                }
                            }

                            // Context menu for the row.
                            row_interact.context_menu(|ui| {
                                if ui.button("Start / Resume").clicked() {
                                    self.selected_download_id = Some(d.id);
                                    self.start_selected();
                                    ui.close_menu();
                                }
                                if ui.button("Pause / Stop").clicked() {
                                    self.selected_download_id = Some(d.id);
                                    self.pause_selected();
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui.button("Open File").clicked() {
                                    self.open_download_file(d.id, &d.save_path);
                                    ui.close_menu();
                                }
                                if ui.button("Open Containing Folder").clicked() {
                                    if let Some(parent) = d.save_path.parent() {
                                        let _ = std::process::Command::new("explorer").arg(parent).spawn();
                                    }
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui.button("Copy Download URL").clicked() {
                                    ctx.output_mut(|o| o.copied_text = d.url.clone());
                                    self.status_bar = "URL copied to clipboard.".into();
                                    ui.close_menu();
                                }
                                if ui.button("Progress Window").clicked() {
                                    if !self.active_details_popouts.contains(&d.id) {
                                        self.active_details_popouts.push(d.id);
                                    }
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui.button("Remove from List").clicked() {
                                    self.selected_download_id = Some(d.id);
                                    self.delete_selected();
                                    ui.close_menu();
                                }
                            });
                        }
                    });
                }
            });
            });

        // -------------------------------------------------------------------
        // 6. POPUPS & OS VIEWPORTS (Enforces AGENTS.md Hard Rules)
        // -------------------------------------------------------------------

        // A. Add URL Dialog (Separate OS Window)
        if self.add_dialog.open {
            self.show_add_url_viewport(ctx);
        }

        // A2. Delete Confirmation (Separate OS Window)
        if self.delete_confirm.open {
            self.show_delete_confirm_viewport(ctx);
        }

        // B. Settings Dialog (options::SettingsDialog)
        if self.settings_dialog.open {
            self.settings_dialog.show(ctx);
            let outcome = self.settings_dialog.take_outcome();
            if let Some(new_settings) = outcome.settings {
                self.accent = ui_accent(&new_settings.accent);
                self.column_widths = column_widths_from_settings(&new_settings);
                self.column_visible = column_visible_from_settings(&new_settings);
                self.manager
                    .quota
                    .update_config(new_settings.quota.clone());
                let auto_startup = new_settings.auto_startup;
                let mut st = self.manager.lock_ui();
                st.settings = new_settings;
                let _ = st.save();
                drop(st);
                match crate::startup::sync(auto_startup) {
                    Ok(()) => {
                        self.status_bar = "Settings saved.".into();
                    }
                    Err(error) => {
                        crate::logfile::log(
                            "STARTUP",
                            &format!("could not update the launch at startup entry: {error}"),
                        );
                        self.status_bar =
                            format!("Settings saved, but the startup entry failed: {error}");
                    }
                }
            }
            if let Some(webhooks) = outcome.webhooks {
                let mut st = self.manager.lock_ui();
                st.webhooks = webhooks;
                let _ = st.save();
            }
            if let Some(site_logins) = outcome.site_logins {
                let mut st = self.manager.lock_ui();
                let previous = std::mem::replace(&mut st.site_logins, site_logins);
                if let Err(error) = st.save() {
                    st.site_logins = previous;
                    self.status_bar = format!("Could not save Site Logins: {error}");
                } else {
                    self.status_bar = "Site Logins saved.".into();
                }
            }
            if outcome.open_log_dir {
                let dir = crate::state::state_dir().join("logs");
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::process::Command::new("explorer").arg(dir).spawn();
            }
            if outcome.clean_temp {
                if self.manager.active_count() > 0 {
                    self.status_bar =
                        "Stop active downloads before cleaning temporary files.".into();
                } else {
                    let temp_dir = self.manager.lock_ui().settings.temp_path.clone();
                    if temp_dir.exists() {
                        if let Ok(entries) = std::fs::read_dir(&temp_dir) {
                            for entry in entries.flatten() {
                                let path = entry.path();
                                if path.is_file() {
                                    let _ = std::fs::remove_file(path);
                                }
                            }
                        }
                    }
                    self.status_bar = "Temporary files cleaned.".into();
                }
            }
            // Persist the Options window position after the settings above
            // have been applied, so an OK press cannot overwrite it with the
            // snapshot the dialog opened with.
            if let Some(pos) = self.settings_dialog.pending_pos.take() {
                self.save_position("settings", pos);
            }
        }

        // C. Scheduler Dialog (Separate OS Window)
        if self.scheduler_dialog.open {
            self.show_scheduler_viewport(ctx);
        }

        // C2. Site Grabber is a separate OS window because it creates downloads.
        self.poll_grabber();
        if self.grabber_dialog.open {
            self.show_grabber_viewport(ctx);
        }

        // D. Download Progress Popout Windows (Separate OS Windows)
        let popout_ids = self.active_details_popouts.clone();
        for id in popout_ids {
            self.show_progress_viewport(ctx, id);
        }

        // E. In-app overlays that are not download flows.
        self.render_search_popup(ctx);
        self.show_about(ctx);

        // Instant close: hook the popouts' window procedures so the
        // title-bar X hides them immediately instead of waiting for the
        // parent to paint the close. The scan matches download windows by
        // title prefix (the filename can change) and re-hooks windows that
        // were recreated; it is throttled to a few passes per second because
        // enumerating windows is expensive.
        let dialog_bits = (self.add_dialog.open as u8)
            | ((self.delete_confirm.open as u8) << 1)
            | ((self.settings_dialog.open as u8) << 2)
            | ((self.scheduler_dialog.open as u8) << 3)
            | ((self.grabber_dialog.open as u8) << 4);
        let dialog_count = [
            self.add_dialog.open,
            self.delete_confirm.open,
            self.settings_dialog.open,
            self.scheduler_dialog.open,
            self.grabber_dialog.open,
        ]
        .iter()
        .filter(|open| **open)
        .count();
        let signature = (self.active_details_popouts.clone(), dialog_bits);
        let expected = self.active_details_popouts.len() + dialog_count;
        let signature_changed = signature != self.hook_signature;
        self.hook_signature = signature;
        if signature_changed
            || self.hook_pending
            || (expected > 0 && self.last_hook_refresh.elapsed() >= Duration::from_millis(250))
        {
            self.last_hook_refresh = Instant::now();
            let found = crate::browser::refresh_popup_hooks();
            self.hook_pending = found < expected;
        }

        // Immediate viewports (progress windows, dialogs) are painted by the
        // parent window's frame. The park hook keeps the root visible and
        // non-iconic, so eframe schedules these frames normally. The waker in
        // `browser::spawn_busy_waker` remains only a safety net for an
        // unexpectedly hidden/iconic window.
        let child_window_open = !self.active_details_popouts.is_empty()
            || self.add_dialog.open
            || self.settings_dialog.open
            || self.scheduler_dialog.open
            || self.grabber_dialog.running
            || self.about_open
            || self.delete_confirm.open;
        crate::browser::set_popout_open(child_window_open);
        if child_window_open {
            // A popout that opened while the main window was truly minimized
            // would never be painted: eframe skips frames for iconic windows.
            crate::browser::wake_main_for_popouts();
        }
        let busy_val = self.manager.active_count() > 0 || child_window_open;
        self.busy_frames.store(busy_val, Ordering::Relaxed);
        // Tell the busy waker this frame is done so it only queues the next
        // synthetic wake once we are ready for it.
        self.frame_seq.fetch_add(1, Ordering::Relaxed);
        // Immediate viewports are driven by the parent frame. A timed repaint
        // keeps them responsive without spinning the GUI thread continuously.
        // A pending Win32 hook also needs another frame to attach to a window
        // that did not exist yet.
        if child_window_open || self.hook_pending {
            ctx.request_repaint_after(Duration::from_millis(50));
        } else {
            // Keep the event loop alive even when idle so the tray, clipboard
            // monitor, and pending queue continue to work.
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        // Native window chrome (title bar, border, caption text) follows the
        // app theme. Done last so a popout created during THIS frame is
        // themed before it is composited - otherwise the OS shows its
        // default title bar (white, or plain black in dark mode) until the
        // next themed pass.
        #[cfg(target_os = "windows")]
        {
            native_win::apply_titlebar_theme(
                palette.dark,
                [
                    palette.background.r(),
                    palette.background.g(),
                    palette.background.b(),
                ],
                [palette.text.r(), palette.text.g(), palette.text.b()],
                [
                    palette.border.r(),
                    palette.border.g(),
                    palette.border.b(),
                ],
            );
        }

        // BASELINE PROFILER PATCH: one line per completed frame.
        if crate::logfile::profile_enabled() {
            let mode = if dragging {
                "drag"
            } else if self.busy_frames.load(Ordering::Relaxed) {
                "ui"
            } else {
                "idle"
            };
            crate::logfile::profile_frame(mode, frame_start.elapsed().as_secs_f64() * 1000.0);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Flush the final window position/size, even if the debounce timer
        // had not fired yet.
        self.flush_popout_positions();
        self.last_main_window_save = Instant::now() - Duration::from_secs(5);
        let mut st = self.manager.lock_ui();
        let _ = st.save();
        self.tray_icon = None;
    }

    /// Paint any area the UI does not cover (e.g. during a viewport resize)
    /// in the active theme background instead of a foreign color.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        bg_dark().to_normalized_gamma_f32()
    }
}

impl DmApp {
    /// Remember where the main window is and how big it is, so the next
    /// launch can restore it when "Remember window positions" is enabled.
    ///
    /// A maximized/minimized window is ignored: restoring the *normal*
    /// rectangle is what users expect, not the parked one.
    fn track_main_window(&mut self, ctx: &egui::Context) {
        // A parked (off-screen) window has no meaningful on-screen rectangle;
        // saving its -32000 position would move the window there on the next
        // launch.
        if !self.show_main_window || crate::browser::main_window_parked() {
            return;
        }
        let (outer, inner, maximized, minimized) = ctx.input(|i| {
            let viewport = i.viewport();
            (
                viewport.outer_rect,
                viewport.inner_rect,
                viewport.maximized.unwrap_or(false),
                viewport.minimized.unwrap_or(false),
            )
        });
        let Some(outer) = outer else {
            return;
        };
        if maximized || minimized || outer.width() < 200.0 || outer.height() < 150.0 {
            return;
        }
        let pos = [outer.min.x.round(), outer.min.y.round()];
        let size = inner.map(|r| [r.width().round(), r.height().round()]);

        let mut changed = false;
        {
            let mut st = self.manager.lock_ui();
            if st.settings.remember_window_pos {
                if st.settings.window_pos != Some(pos) {
                    st.settings.window_pos = Some(pos);
                    changed = true;
                }
                if let Some(size) = size {
                    if st.settings.window_size != Some(size) {
                        st.settings.window_size = Some(size);
                        changed = true;
                    }
                }
            } else if st.settings.window_pos.is_some() || st.settings.window_size.is_some() {
                // Option is off: forget the stored spot so re-enabling it
                // does not jump the window somewhere stale.
                st.settings.window_pos = None;
                st.settings.window_size = None;
                changed = true;
            }
        }
        if changed && self.last_main_window_save.elapsed() >= Duration::from_millis(700) {
            self.last_main_window_save = Instant::now();
            let mut st = self.manager.lock_ui();
            let _ = st.save();
        }
    }
}

// ---------------------------------------------------------------------------
// Separate Viewports (OS Windows) - AGENTS.md Conformance
// ---------------------------------------------------------------------------
impl DmApp {
    fn show_add_url_viewport(&mut self, ctx: &egui::Context) {
        let viewport_id = ViewportId::from_hash_of("dm-add");
        let mut close_requested = false;

        // Compute initial position: prefer saved position, otherwise center on main window.
        let initial_pos = self.saved_position(ctx, "add-url", [500.0, 340.0]);

        ctx.show_viewport_immediate(
            viewport_id,
            ViewportBuilder::default()
                .with_title("Add new download...")
                .with_inner_size([500.0, 340.0])
                .with_min_inner_size([440.0, 300.0])
                .with_always_on_top()
                .with_position(initial_pos),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    close_requested = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                // Remember the spot only if the user moved it.
                self.record_popout_move(ctx, "add-url", initial_pos);
                egui::CentralPanel::default().frame(egui::Frame::none().fill(bg_panel()).inner_margin(Margin::same(14.0))).show(ctx, |ui| {
                    ui.add_enabled_ui(!self.add_dialog.auto_started, |ui| {
                    let heading = if self.add_dialog.auto_started {
                        "Download File Info"
                    } else {
                        "Enter new address to download"
                    };
                    ui.label(RichText::new(heading).size(14.0).strong().color(text_main()));
                    ui.add_space(8.0);

                    // URL row
                    ui.horizontal(|ui| {
                        ui.add_sized([78.0, 20.0], egui::Label::new(RichText::new("Address:").color(text_dim())));
                        ui.add_sized(Vec2::new(370.0, 24.0), egui::TextEdit::singleline(&mut self.add_dialog.url).hint_text("https://example.com/file.zip"));
                    });
                    ui.add_space(6.0);

                    // Save As row
                    ui.horizontal(|ui| {
                        ui.add_sized([78.0, 20.0], egui::Label::new(RichText::new("Save To:").color(text_dim())));
                        ui.add_sized(Vec2::new(330.0, 24.0), egui::TextEdit::singleline(&mut self.add_dialog.save_as));
                        if ui.button("...").clicked() {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                self.add_dialog.save_as = folder.to_string_lossy().to_string();
                            }
                        }
                    });
                    ui.add_space(6.0);

                    // Queue row
                    let queues = {
                        let st = self.manager.lock_ui();
                        st.queues.clone()
                    };
                    ui.horizontal(|ui| {
                        ui.add_sized([78.0, 20.0], egui::Label::new(RichText::new("Queue:").color(text_dim())));
                        let mut queue = self.add_dialog.queue.clone();
                        egui::ComboBox::from_id_source("add_url_queue")
                            .width(260.0)
                            .selected_text(if queue.is_empty() { MAIN_QUEUE } else { queue.as_str() })
                            .show_ui(ui, |ui| {
                                for name in &queues {
                                    ui.selectable_value(&mut queue, name.clone(), name);
                                }
                                if !queues.iter().any(|name| name == MAIN_QUEUE) {
                                    ui.selectable_value(&mut queue, MAIN_QUEUE.to_string(), MAIN_QUEUE);
                                }
                            });
                        if queue != self.add_dialog.queue {
                            self.add_dialog.queue = queue;
                        }
                    });
                    ui.add_space(6.0);

                    // Description row
                    ui.horizontal(|ui| {
                        ui.add_sized([78.0, 20.0], egui::Label::new(RichText::new("Description:").color(text_dim())));
                        ui.add_sized(Vec2::new(370.0, 24.0), egui::TextEdit::singleline(&mut self.add_dialog.description).hint_text("Optional note"));
                    });
                    ui.add_space(6.0);

                    // Connections row
                    ui.horizontal(|ui| {
                        ui.add_sized([78.0, 20.0], egui::Label::new(RichText::new("Connections:").color(text_dim())));
                        ui.add(egui::DragValue::new(&mut self.add_dialog.connections).clamp_range(1..=32));
                        ui.label(RichText::new("simultaneous connections").size(11.0).color(text_dim()));
                    });

                    if !self.add_dialog.error.is_empty() {
                        ui.add_space(6.0);
                        ui.label(RichText::new(&self.add_dialog.error).size(11.5).color(status_failed()));
                    }
                    });
                    ui.add_space(12.0);

                    if self.add_dialog.auto_started {
                        ui.label(RichText::new("Download started automatically. The displayed file information was used.").size(11.5).color(text_dim()));
                        if ui.button(RichText::new("  Close  ").color(text_main())).clicked() {
                            self.add_dialog.open = false;
                        }
                    } else {
                        // Action buttons
                        ui.horizontal(|ui| {
                            if ui.button(RichText::new("  OK (Download Now)  ").color(text_main())).clicked() {
                                if self.create_download(true) {
                                    self.add_dialog.open = false;
                                }
                            }
                            if ui.button(RichText::new(" Download Later ").color(text_dim())).clicked() {
                                if self.create_download(false) {
                                    self.add_dialog.open = false;
                                }
                            }
                            if ui.button(RichText::new(" Cancel ").color(text_dim())).clicked() {
                                self.add_dialog.open = false;
                            }
                        });
                    }
                });
            },
        );

        if close_requested {
            self.add_dialog.open = false;
            crate::browser::wake_main_window_now();
        }
    }

    /// Delete-confirmation window (a separate OS window like the other
    /// download dialogs). Offers the checkbox that also removes the
    /// file(s) from the disk, not just from the download list.
    fn show_delete_confirm_viewport(&mut self, ctx: &egui::Context) {
        let viewport_id = ViewportId::from_hash_of("dm-delete-confirm");
        let mut close_requested = false;

        let initial_pos = self.saved_position(ctx, "delete-confirm", [440.0, 196.0]);

        ctx.show_viewport_immediate(
            viewport_id,
            ViewportBuilder::default()
                .with_title("Confirm Delete")
                .with_inner_size([440.0, 196.0])
                .with_min_inner_size([440.0, 196.0])
                .with_always_on_top()
                .with_position(initial_pos),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    close_requested = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                self.record_popout_move(ctx, "delete-confirm", initial_pos);
                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(bg_panel()).inner_margin(Margin::same(16.0)))
                    .show(ctx, |ui| {
                        let count = self.delete_confirm.ids.len();
                        let subject = if self.delete_confirm.all {
                            format!("all {count} download(s)")
                        } else {
                            let first = self.delete_confirm.ids.first().copied().unwrap_or(0);
                            let name = {
                                let st = self.manager.lock_ui();
                                st.downloads
                                    .iter()
                                    .find(|d| d.id == first)
                                    .map(|d| file_name_of(d))
                                    .unwrap_or_else(|| format!("download #{first}"))
                            };
                            format!("\"{name}\"")
                        };
                        ui.label(
                            RichText::new(format!("Delete {subject} from the list?"))
                                .size(14.0)
                                .strong()
                                .color(text_main()),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("This removes the download record. It cannot be undone.")
                                .size(12.0)
                                .color(text_dim()),
                        );
                        ui.add_space(10.0);
                        if ui
                            .checkbox(
                                &mut self.delete_confirm.from_disk,
                                RichText::new("Also delete the file(s) from the disk")
                                    .color(text_main()),
                            )
                            .changed()
                        {
                            // No extra action needed; the flag is read on confirm.
                        }
                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui
                                .button(RichText::new("  Delete  ").color(status_failed()))
                                .clicked()
                            {
                                self.confirm_delete_now();
                            }
                            if ui.button(RichText::new("  Cancel  ").color(text_main())).clicked() {
                                self.delete_confirm.open = false;
                            }
                        });
                    });
            },
        );

        if close_requested {
            self.delete_confirm.open = false;
            crate::browser::wake_main_window_now();
        }
    }

    fn create_download(&mut self, start_now: bool) -> bool {
        self.add_dialog.error.clear();
        let url_str0 = self.add_dialog.url.trim().to_string();
        if url_str0.contains('[') || url_str0.contains('{') {
            let urls = crate::batch::expand_urls(&url_str0);
            if urls.len() > 1 {
                let mut all_ok = true;
                for u in urls {
                    self.add_dialog.url = u;
                    if !self.create_download(start_now) {
                        all_ok = false;
                    }
                }
                self.add_dialog.url = url_str0;
                return all_ok;
            }
        }
        let mut url_str = self.add_dialog.url.trim().to_string();
        if url_str.is_empty() {
            self.add_dialog.error = "Enter a URL to download.".into();
            return false;
        }
        if !url_str.contains("://") {
            url_str = format!("https://{url_str}");
        }
        let parsed = match url::Url::parse(&url_str) {
            Ok(url) => url,
            Err(error) => {
                self.add_dialog.error = format!("Invalid URL: {error}");
                return false;
            }
        };
        if !matches!(parsed.scheme(), "http" | "https" | "ftp") {
            self.add_dialog.error = "Only HTTP, HTTPS and FTP URLs are supported.".into();
            return false;
        }
        if parsed.host_str().unwrap_or("").is_empty() {
            self.add_dialog.error = "The URL must include a host name.".into();
            return false;
        }

        let save_dir = PathBuf::from(self.add_dialog.save_as.trim());
        if self.add_dialog.save_as.trim().is_empty() {
            self.add_dialog.error = "Choose a folder to save the file in.".into();
            return false;
        }
        if !save_dir.is_dir() {
            self.add_dialog.error = "The selected save folder does not exist.".into();
            return false;
        }

        let raw_name = parsed
            .path_segments()
            .and_then(|mut segments| segments.next_back().map(str::to_string))
            .unwrap_or_default();
        let decoded = percent_encoding::percent_decode_str(&raw_name)
            .decode_utf8_lossy()
            .to_string();
        let filename = sanitize_filename(&decoded);
        let save_path = save_dir.join(filename);

        let queue = {
            let queue = self.add_dialog.queue.trim().to_string();
            if queue.is_empty() {
                MAIN_QUEUE.to_string()
            } else {
                queue
            }
        };
        let connections = self.add_dialog.connections.clamp(1, 32);
        let description = self.add_dialog.description.trim().to_string();

        let mut st = self.manager.lock_ui();
        let id = st.max_id;
        st.max_id += 1;

        let rec = DownloadRec {
            id,
            url: url_str.clone(),
            save_path,
            file_size: 0,
            resumable: true,
            etag: None,
            last_modified: None,
            chunks: Vec::new(),
            status: if start_now {
                Status::Downloading
            } else {
                Status::Queued
            },
            last_error: None,
            downloaded_secs: 0,
            date_added: chrono::Utc::now().timestamp(),
            last_try_date: chrono::Utc::now().timestamp(),
            queue: Some(queue.clone()),
            group: None,
            description,
            max_connections: Some(connections),
            host_connections: None,
            referer: self.add_dialog.referer.clone(),
            cookies: None,
            user_agent: None,
            post_action: None,
            url_history: Vec::new(),
            quality: None,
            mux: None,
            mirrors: Vec::new(),
            temp_dir: st.settings.temp_path.clone(),
            restarted: false,
        };

        st.downloads.push(rec.clone());
        if !st.queues.iter().any(|name| name == &queue) {
            st.queues.push(queue.clone());
        }
        let show_progress = st.settings.show_progress_window;
        let saved = st.save();
        drop(st);

        if start_now && show_progress && !self.active_details_popouts.contains(&id) {
            self.active_details_popouts.push(id);
        }

        if let Err(error) = saved {
            let mut st = self.manager.lock_ui();
            st.downloads.retain(|download| download.id != id);
            let _ = st.save();
            self.add_dialog.error = format!("Could not save download: {error}");
            return false;
        }

        if start_now {
            if let Err(error) = self
                .manager
                .start(rec, None, self.add_dialog.referer.clone())
            {
                let message = format!("Added download #{id}; could not start: {error}");
                self.status_bar = message.clone();
                self.add_dialog.error = message.clone();
                let mut st = self.manager.lock_ui();
                if let Some(download) = st.downloads.iter_mut().find(|download| download.id == id) {
                    download.status = Status::Failed;
                    download.last_error = Some(message);
                }
                if let Err(save_error) = st.save() {
                    crate::logfile::log(
                        "GUI",
                        &format!("could not persist failed download start: {save_error:#}"),
                    );
                }
                return false;
            } else {
                self.status_bar = format!("Added and started download #{id}.");
            }
        } else {
            self.status_bar = format!("Queued download #{id}.");
        }
        true
    }

    fn show_grabber_viewport(&mut self, ctx: &egui::Context) {
        let viewport_id = ViewportId::from_hash_of("dm-grabber");
        let mut close_requested = false;
        let initial_pos = self.saved_position(ctx, "site-grabber", [680.0, 540.0]);

        ctx.show_viewport_immediate(
            viewport_id,
            ViewportBuilder::default()
                .with_title("Site Grabber")
                .with_inner_size([680.0, 540.0])
                .with_min_inner_size([540.0, 400.0])
                .with_always_on_top()
                .with_position(initial_pos),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    close_requested = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                self.record_popout_move(ctx, "site-grabber", initial_pos);

                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::none()
                            .fill(bg_panel())
                            .inner_margin(Margin::same(14.0)),
                    )
                    .show(ctx, |ui| {
                        ui.label(
                            RichText::new("Collect downloadable links from a website")
                                .size(14.0)
                                .strong()
                                .color(text_main()),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Start address:").color(text_dim()));
                            ui.add_sized(
                                Vec2::new(ui.available_width(), 24.0),
                                egui::TextEdit::singleline(&mut self.grabber_dialog.start_url)
                                    .hint_text("https://example.com/downloads/"),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Depth:").color(text_dim()));
                            ui.add(
                                egui::DragValue::new(&mut self.grabber_dialog.depth)
                                    .clamp_range(0..=20),
                            );
                            ui.checkbox(
                                &mut self.grabber_dialog.same_origin,
                                "Stay on this site",
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("File types:").color(text_dim()));
                            ui.add_sized(
                                Vec2::new(ui.available_width(), 24.0),
                                egui::TextEdit::singleline(&mut self.grabber_dialog.file_types)
                                    .hint_text("ZIP EXE R0*"),
                            );
                        });

                        let queues = {
                            let st = self.manager.lock_ui();
                            st.queues.clone()
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Add results to queue:").color(text_dim()));
                            egui::ComboBox::from_id_source("grabber_queue")
                                .width(230.0)
                                .selected_text(self.grabber_dialog.queue.as_str())
                                .show_ui(ui, |ui| {
                                    for name in &queues {
                                        ui.selectable_value(
                                            &mut self.grabber_dialog.queue,
                                            name.clone(),
                                            name,
                                        );
                                    }
                                    if !queues.iter().any(|name| name == MAIN_QUEUE) {
                                        ui.selectable_value(
                                            &mut self.grabber_dialog.queue,
                                            MAIN_QUEUE.to_string(),
                                            MAIN_QUEUE,
                                        );
                                    }
                                });
                        });

                        if self.grabber_dialog.running {
                            ui.label(
                                RichText::new(format!(
                                    "Crawled {} page(s)...",
                                    self.grabber_dialog.pages_crawled.load(Ordering::Relaxed)
                                ))
                                .size(11.5)
                                .color(text_dim()),
                            );
                        }
                        if !self.grabber_dialog.error.is_empty() {
                            ui.label(
                                RichText::new(&self.grabber_dialog.error)
                                    .size(11.5)
                                    .color(status_failed()),
                            );
                        }
                        if !self.grabber_dialog.report.is_empty() {
                            ui.label(
                                RichText::new(&self.grabber_dialog.report)
                                    .size(11.5)
                                    .color(text_dim()),
                            );
                        }

                        ui.add_space(6.0);
                        ui.separator();
                        ui.label(
                            RichText::new(format!(
                                "Results: {} link(s)",
                                self.grabber_dialog.links.len()
                            ))
                            .strong()
                            .color(text_main()),
                        );
                        egui::ScrollArea::vertical()
                            .id_source("grabber-results")
                            .max_height(300.0)
                            .show(ui, |ui| {
                                for link in self.grabber_dialog.links.clone() {
                                    ui.horizontal(|ui| {
                                        let mut selected =
                                            self.grabber_dialog.selected.contains(&link);
                                        if ui.checkbox(&mut selected, "").changed() {
                                            if selected {
                                                self.grabber_dialog
                                                    .selected
                                                    .insert(link.clone());
                                            } else {
                                                self.grabber_dialog.selected.remove(&link);
                                            }
                                        }
                                        ui.add_sized(
                                            Vec2::new(
                                                ui.available_width().max(0.0),
                                                20.0,
                                            ),
                                            egui::Label::new(
                                                RichText::new(&link)
                                                    .size(11.0)
                                                    .color(text_dim()),
                                            )
                                            .truncate(true),
                                        );
                                    });
                                }
                            });

                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if self.grabber_dialog.running {
                                if ui.button("Cancel Crawl").clicked() {
                                    self.cancel_grabber();
                                }
                            } else if ui.button("Start Crawl").clicked() {
                                self.start_grabber();
                            }
                            if ui
                                .add_enabled(
                                    !self.grabber_dialog.running
                                        && !self.grabber_dialog.links.is_empty(),
                                    egui::Button::new("Select All"),
                                )
                                .clicked()
                            {
                                self.grabber_dialog.selected =
                                    self.grabber_dialog.links.iter().cloned().collect();
                            }
                            if ui
                                .add_enabled(
                                    !self.grabber_dialog.running
                                        && !self.grabber_dialog.selected.is_empty(),
                                    egui::Button::new("Clear Selection"),
                                )
                                .clicked()
                            {
                                self.grabber_dialog.selected.clear();
                            }
                            if ui
                                .add_enabled(
                                    !self.grabber_dialog.running
                                        && !self.grabber_dialog.selected.is_empty(),
                                    egui::Button::new("Add Selected to Queue"),
                                )
                                .clicked()
                            {
                                self.add_grabber_selected();
                            }
                            if ui.button("Close").clicked() {
                                close_requested = true;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                            }
                        });
                    });
            },
        );

        if close_requested {
            self.cancel_grabber();
            self.grabber_dialog.open = false;
            crate::browser::wake_main_window_now();
        }
    }

    fn show_scheduler_viewport(&mut self, ctx: &egui::Context) {
        let viewport_id = ViewportId::from_hash_of("dm-scheduler");
        let mut close_requested = false;

        let initial_pos = self.saved_position(ctx, "scheduler", [480.0, 430.0]);

        ctx.show_viewport_immediate(
            viewport_id,
            ViewportBuilder::default()
                .with_title("Scheduler")
                .with_inner_size([480.0, 430.0])
                .with_min_inner_size([420.0, 380.0])
                .with_always_on_top()
                .with_position(initial_pos),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    close_requested = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                self.record_popout_move(ctx, "scheduler", initial_pos);
                let queues = {
                    let st = self.manager.lock_ui();
                    st.queues.clone()
                };
                let mut chosen_queue = self.scheduler_dialog.queue.clone();
                egui::CentralPanel::default().frame(egui::Frame::none().fill(bg_panel()).inner_margin(Margin::same(14.0))).show(ctx, |ui| {
                    ui.label(RichText::new("Schedule Queued Downloads").size(14.0).strong().color(text_main()));
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Queue:").color(text_dim()));
                        egui::ComboBox::from_id_source("scheduler_queue")
                            .width(250.0)
                            .selected_text(if chosen_queue.is_empty() { MAIN_QUEUE } else { chosen_queue.as_str() })
                            .show_ui(ui, |ui| {
                                for name in &queues {
                                    ui.selectable_value(&mut chosen_queue, name.clone(), name);
                                }
                                if !queues.iter().any(|name| name == MAIN_QUEUE) {
                                    ui.selectable_value(&mut chosen_queue, MAIN_QUEUE.to_string(), MAIN_QUEUE);
                                }
                            });
                    });
                    if chosen_queue != self.scheduler_dialog.queue {
                        let queue = chosen_queue.clone();
                        self.load_scheduler_for_queue(&queue);
                    }
                    ui.add_space(6.0);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Start time:").color(text_dim()));
                        ui.add_sized(Vec2::new(120.0, 24.0), egui::TextEdit::singleline(&mut self.scheduler_dialog.start_time).hint_text("HH:MM, blank = disabled"));
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Stop time:").color(text_dim()));
                        ui.add_sized(Vec2::new(120.0, 24.0), egui::TextEdit::singleline(&mut self.scheduler_dialog.stop_time).hint_text("HH:MM, blank = disabled"));
                    });
                    ui.add_space(4.0);
                    ui.label(RichText::new("Times are local. Overnight windows (start after stop) are supported.").size(10.5).color(text_dim()));
                    ui.add_space(8.0);

                    ui.label(RichText::new("Days:").color(text_dim()));
                    ui.horizontal_wrapped(|ui| {
                        let labels = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
                        for (index, label) in labels.iter().enumerate() {
                            let mut active = self.scheduler_dialog.days.contains(&(index as u32));
                            if ui.checkbox(&mut active, *label).changed() {
                                if active {
                                    if !self.scheduler_dialog.days.contains(&(index as u32)) {
                                        self.scheduler_dialog.days.push(index as u32);
                                    }
                                } else {
                                    self.scheduler_dialog.days.retain(|day| *day != index as u32);
                                }
                            }
                        }
                    });
                    ui.label(RichText::new("Leave all days unselected to run every day.").size(10.5).color(text_dim()));
                    ui.add_space(8.0);
                    ui.checkbox(&mut self.scheduler_dialog.once, "Run once only");
                    ui.add_space(6.0);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("When finished:").color(text_dim()));
                        egui::ComboBox::from_id_source("scheduler_on_finish")
                            .selected_text(match self.scheduler_dialog.on_finish.as_str() {
                                "exit" => "Exit DM Ultra",
                                _ => "Do nothing",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.scheduler_dialog.on_finish, "none".into(), "Do nothing");
                                ui.selectable_value(&mut self.scheduler_dialog.on_finish, "exit".into(), "Exit DM Ultra");
                            });
                    });

                    if !self.scheduler_dialog.error.is_empty() {
                        ui.add_space(6.0);
                        ui.label(RichText::new(&self.scheduler_dialog.error).size(11.5).color(status_failed()));
                    }

                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button(" Save Schedule ").clicked() {
                            self.save_schedule();
                        }
                        if ui.button(" Clear Schedule ").clicked() {
                            self.clear_schedule();
                        }
                        if ui.button(" Close ").clicked() {
                            self.scheduler_dialog.open = false;
                        }
                    });
                });
            },
        );

        if close_requested {
            self.scheduler_dialog.open = false;
            crate::browser::wake_main_window_now();
        }
    }

    fn show_progress_viewport(&mut self, ctx: &egui::Context, id: u64) {
        let viewport_id = ViewportId::from_hash_of(("dl-progress", id));
        let mut close_requested = false;

        let rec = {
            let st = self.manager.lock_ui();
            st.get(id).cloned()
        };

        if let Some(d) = rec {
            let stats = self.manager.stats(id);
            let speed_bps = self
                .speeds
                .get(&id)
                .copied()
                .unwrap_or(0.0)
                .round() as u64;
            let filename = d.save_path.file_name().unwrap_or_default().to_string_lossy().to_string();

            // A unique key per download ID so each progress window remembers
            // its own position independently. ALSO track a shared "last
            // moved" key so NEW downloads open at the last position any
            // download window was moved to (instead of upper-left cascade).
            let key = format!("dl-progress-{}", id);
            let shared_key = "dl-progress-last";
            let cascade_index = self
                .active_details_popouts
                .iter()
                .position(|&open_id| open_id == id)
                .unwrap_or(0);
            let saved_pos = {
                let st = self.manager.lock_ui();
                // First try the per-ID position, then fall back to the shared
                // "last moved" position so new downloads open where the user
                // last put a progress window.
                let per_id = st.settings
                    .remember_window_pos
                    .then(|| st.settings.window_positions.get(&key).copied())
                    .flatten();
                let shared = st.settings
                    .remember_window_pos
                    .then(|| st.settings.window_positions.get(shared_key).copied())
                    .flatten();
                per_id.or(shared)
            };
            let initial_pos = if let Some([x, y]) = saved_pos {
                egui::pos2(x, y)
            } else {
                let outer_size = [
                    500.0 + POPOUT_CHROME_W,
                    360.0 + POPOUT_CHROME_H,
                ];
                adjacent_progress_pos(ctx, outer_size, cascade_index)
                    .or_else(|| centered_popup_pos(ctx, outer_size))
                    .unwrap_or_else(|| egui::pos2(120.0, 120.0))
            };

            ctx.show_viewport_immediate(
                viewport_id,
                ViewportBuilder::default()
                    .with_title(format!("Download status - {}", filename))
                    .with_inner_size([500.0, 360.0])
                    .with_min_inner_size([440.0, 300.0])
                    .with_always_on_top()
                    .with_position(initial_pos),
                |ctx, _class| {
                    if ctx.input(|i| i.viewport().close_requested()) {
                        close_requested = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    }
                    self.record_popout_move(ctx, &key, initial_pos);
                    egui::CentralPanel::default().frame(egui::Frame::none().fill(bg_panel()).inner_margin(Margin::same(14.0))).show(ctx, |ui| {
                        ui.label(RichText::new(&filename).size(15.0).strong().color(text_main()));
                        ui.add(egui::Label::new(RichText::new(&d.url).size(11.0).color(text_dim())).truncate(true));
                        ui.add_space(4.0);
                        ui.label(RichText::new(format!("Save to: {}", d.save_path.display())).size(11.0).color(text_dim()));
                        ui.add_space(8.0);

                        let total = if d.file_size > 0 { d.file_size } else { stats.as_ref().map(|s| s.total()).unwrap_or(0) };
                        let downloaded = stats.as_ref().map(|s| s.bytes()).unwrap_or_else(|| d.total_downloaded());
                        render_segmented_progress(
                            ui,
                            total,
                            downloaded,
                            stats.as_ref().map(|s| s.segments()).unwrap_or_default().as_slice(),
                        );
                        ui.add_space(8.0);

                        egui::Grid::new(("progress_grid", id))
                            .num_columns(2)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                ui.label(RichText::new("Downloaded:").color(text_dim()));
                                ui.label(RichText::new(format!("{} / {}", human_bytes(downloaded), if total > 0 { human_bytes(total) } else { "Unknown".to_string() })).color(text_main()));
                                ui.end_row();

                                ui.label(RichText::new("Transfer rate:").color(text_dim()));
                                ui.label(RichText::new(if d.status == Status::Downloading && speed_bps > 0 {
                                    format!("{}/s", human_bytes(speed_bps))
                                } else {
                                    "--".to_string()
                                }).color(text_main()));
                                ui.end_row();

                                let eta = if d.status == Status::Downloading && speed_bps > 0 && total > downloaded {
                                    format_eta(total.saturating_sub(downloaded), speed_bps)
                                } else {
                                    "--".to_string()
                                };
                                ui.label(RichText::new("Time left:").color(text_dim()));
                                ui.label(RichText::new(eta).color(text_main()));
                                ui.end_row();

                                ui.label(RichText::new("Status:").color(text_dim()));
                                ui.label(RichText::new(d.status.as_str()).color(text_main()));
                                ui.end_row();

                                ui.label(RichText::new("Connections:").color(text_dim()));
                                // Live connections = parts currently
                                // transferring (the engine marks finished
                                // parts conn = 0). The value used to show
                                // raw bar segments, which includes splits
                                // and finished parts - that is why 8
                                // connections could display 26.
                                let segs = stats.as_ref().map(|s| s.segments()).unwrap_or_default();
                                let active_conns =
                                    segs.iter().filter(|s| s.conn != 0 && s.downloaded < s.len).count();
                                let limit = d.max_connections.unwrap_or(1).clamp(1, 32);
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(if stats.is_some() {
                                        format!("{active_conns} / {limit}")
                                    } else {
                                        limit.to_string()
                                    })
                                    .color(text_main()));
                                    if ui
                                        .small_button("  -  ")
                                        .on_hover_text("Fewer connections")
                                        .clicked()
                                    {
                                        self.set_download_connections(id, limit.saturating_sub(1).max(1));
                                    }
                                    if ui
                                        .small_button(" + ")
                                        .on_hover_text("More connections")
                                        .clicked()
                                    {
                                        self.set_download_connections(id, (limit + 1).min(32));
                                    }
                                });
                                ui.end_row();

                                ui.label(RichText::new("Resumable:").color(text_dim()));
                                ui.label(RichText::new(if d.resumable { "Yes" } else { "No" }).color(text_main()));
                                ui.end_row();
                            });

                        ui.add_space(12.0);
                        // Checked once per frame here so the button can explain
                        // itself while the file is still arriving.
                        let open_note = open_action_blocker(&d.save_path);
                        ui.horizontal(|ui| {
                            if d.status == Status::Downloading {
                                if ui.button(" Pause ").clicked() {
                                    self.pause_download(id);
                                }
                            } else {
                                if ui.button(" Resume ").clicked() {
                                    self.start_download(id);
                                }
                            }
                            // " Open " must RUN the file. The old
                            // `explorer <save_path>` call only opened a window
                            // on the folder holding it, so the button looked
                            // like a second "Open Folder".
                            let open_btn = ui.add_enabled(
                                open_note.is_none(),
                                egui::Button::new(" Open "),
                            );
                            let open_btn = match open_note.as_deref() {
                                Some(reason) => {
                                    open_btn.on_disabled_hover_text(format!("Cannot open: {reason}."))
                                }
                                None => open_btn.on_hover_text(
                                    "Open the file with the program registered for its type.",
                                ),
                            };
                            if open_btn.clicked() {
                                self.open_download_file(id, &d.save_path);
                            }
                            if ui.button(" Open Folder ").clicked() {
                                if let Some(parent) = d.save_path.parent() {
                                    let _ = std::process::Command::new("explorer").arg(parent).spawn();
                                }
                            }
                            if ui.button(" Close ").clicked() {
                                close_requested = true;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                            }
                        });
                    });
                },
            );
        } else {
            close_requested = true;
        }

        // The popout is painted by the root frame. Keep a light repaint pulse
        // while it exists so a click is processed even if the main window is
        // hidden to the tray (a hidden window gets no OS paint of its own).
        ctx.request_repaint_after(Duration::from_millis(100));

        if close_requested {
            self.active_details_popouts.retain(|&pop_id| pop_id != id);
            self.flush_popout_positions();
            crate::browser::wake_main_window_now();
        }
    }

    fn table_header_cell(
        &self,
        ui: &mut Ui,
        label: &str,
        col: usize,
        width: f32,
        has_search: bool,
    ) -> (bool, Rect) {
        let is_sorted = self.sort_column == col;
        let arrow = if is_sorted {
            if self.sort_descending { " ▼" } else { " ▲" }
        } else {
            ""
        };
        let text = format!("{}{}", label, arrow);
        let color = if is_sorted { text_main() } else { text_dim() };

        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 27.0), Sense::hover());
        let text_rect = Rect::from_min_max(
            rect.min + Vec2::new(COLUMN_CELL_PADDING, 0.0),
            pos2(
                rect.right() - COLUMN_CELL_PADDING - if has_search { 30.0 } else { 0.0 },
                rect.bottom(),
            ),
        );
        let click_rect = Rect::from_min_max(
            text_rect.min,
            pos2(
                (text_rect.right() - COLUMN_RESIZE_HIT_WIDTH * 0.5).max(text_rect.left()),
                rect.bottom(),
            ),
        );
        let response = ui.interact(
            click_rect,
            egui::Id::new(("download-column-header", col)),
            Sense::click(),
        );
        ui.painter().with_clip_rect(text_rect).text(
            pos2(text_rect.left(), rect.center().y),
            Align2::LEFT_CENTER,
            text,
            FontId::proportional(12.0),
            color,
        );
        (
            response.on_hover_cursor(CursorIcon::Default).clicked(),
            rect,
        )
    }

    fn toggle_sort(&mut self, col: usize) {
        if self.sort_column == col {
            self.sort_descending = !self.sort_descending;
        } else {
            self.sort_column = col;
            self.sort_descending = true;
        }
    }

    /// Open position for a popout: the spot the user last left it in, or -
    /// when the user has never moved it - centred over the main window.
    ///
    /// Only positions the user actually chose are stored (see
    /// `record_popout_move`), so an auto-centred dialog keeps following the
    /// main window instead of freezing wherever it happened to be opened
    /// once.
    fn saved_position(&self, ctx: &egui::Context, key: &str, inner_size: [f32; 2]) -> egui::Pos2 {
        let saved = {
            let st = self.manager.lock_ui();
            if st.settings.remember_window_pos {
                st.settings.window_positions.get(key).copied()
            } else {
                None
            }
        };
        if let Some(pos) = saved {
            return egui::pos2(pos[0], pos[1]);
        }
        // Default: centred over the main window. `with_position` works in
        // OUTER coordinates, so grow the requested client size by the frame.
        centered_popup_pos(
            ctx,
            [
                inner_size[0] + POPOUT_CHROME_W,
                inner_size[1] + POPOUT_CHROME_H,
            ],
        )
        .unwrap_or_else(|| egui::pos2(120.0, 120.0))
    }

    /// Watch a popout for a user move. The position is stored only once the
    /// window is genuinely away from where it was opened; a window that is
    /// still where we put it stays "auto-centred" for the next open.
    fn record_popout_move(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        open_pos: egui::Pos2,
    ) {
        let current = ctx
            .input(|i| i.viewport().outer_rect)
            .map(|r| [r.min.x, r.min.y]);
        if let Some(pos) = current {
            // A few pixels of slack: DPI rounding and OS placement adjust the
            // window by a pixel or two without the user having moved it.
            let moved = (pos[0] - open_pos.x).abs() > 4.0
                || (pos[1] - open_pos.y).abs() > 4.0;
            if moved {
                self.save_position(key, pos);
                // Also update the shared "last moved" key so NEW downloads
                // open at this position.
                self.save_position("dl-progress-last", pos);
            }
        }
    }

    /// Remember a popout position. The in-memory value is updated at once;
    /// the state file is written at most twice per second, outside the GUI
    /// lock, so dragging never blocks the UI thread on disk I/O.
    fn save_position(&mut self, key: &str, pos: [f32; 2]) {
        let changed = {
            let mut st = self.manager.lock_ui();
            if !st.settings.remember_window_pos {
                return;
            }
            let changed = st
                .settings
                .window_positions
                .get(key)
                .map(|old| (old[0] - pos[0]).abs() > 0.5 || (old[1] - pos[1]).abs() > 0.5)
                .unwrap_or(true);
            if changed {
                st.settings.window_positions.insert(key.to_string(), pos);
            }
            changed
        };
        if changed {
            self.popout_pos_dirty = true;
        }
        if self.popout_pos_dirty && self.last_popout_save.elapsed() >= Duration::from_millis(500) {
            self.flush_popout_positions();
        }
    }

    /// Write the pending popout positions to disk (outside the GUI lock).
    fn flush_popout_positions(&mut self) {
        if !self.popout_pos_dirty {
            return;
        }
        self.popout_pos_dirty = false;
        self.last_popout_save = Instant::now();
        // Clone under the lock, save after releasing it: serializing and
        // writing state.json must never block the UI thread.
        let snapshot = {
            let st = self.manager.lock_ui();
            st.clone()
        };
        let _ = snapshot.save();
    }
}

// ---------------------------------------------------------------------------
// Helpers & Components
// ---------------------------------------------------------------------------

/// A data cell always reserves its full column width, even when its text is
/// short. The child UI clips and truncates content without changing the row
/// layout's next column position.
fn cell_label_left(ui: &mut Ui, width: f32, label: egui::Label) {
    let (track_rect, _) = ui.allocate_exact_size(Vec2::new(width, 18.0), Sense::hover());
    let cell_rect = track_rect.shrink2(Vec2::new(COLUMN_CELL_PADDING, 0.0));
    let mut layout = egui::Layout::left_to_right(egui::Align::Center);
    layout.main_align = egui::Align::LEFT;
    layout.cross_align = egui::Align::Center;
    let mut cell_ui = ui.child_ui(cell_rect, layout);
    cell_ui.set_max_width(width);
    cell_ui.add(label);
}

#[cfg(test)]
mod table_cell_tests {
    use super::*;

    fn next_track_offset(text: &str) -> f32 {
        let mut offset = 0.0;
        egui::__run_test_ui(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.horizontal(|ui| {
                let first_cell_left = ui.cursor().min.x;
                cell_label_left(ui, 120.0, egui::Label::new(text).truncate(true));
                let (next_cell, _) =
                    ui.allocate_exact_size(Vec2::new(72.0, 18.0), Sense::hover());
                offset = next_cell.left() - first_cell_left;
            });
        });
        offset
    }

    #[test]
    fn short_and_long_cell_text_preserve_the_next_column_boundary() {
        assert_eq!(next_track_offset("x"), 120.0);
        let long_name = "long-download-filename-".repeat(16);
        assert_eq!(next_track_offset(&long_name), 120.0);
    }

    #[test]
    fn visible_columns_share_one_contiguous_track_layout() {
        let tracks = column_tracks(
            &[120.0, 60.0, 80.0, 90.0, 70.0, 100.0, 100.0, 140.0],
            &[true, false, true, true, true, true, true, true],
        );

        assert_eq!(
            tracks.iter().map(|track| track.column).collect::<Vec<_>>(),
            vec![0, 2, 3, 4, 5, 6, 7]
        );
        let boundaries: Vec<f32> = tracks
            .iter()
            .scan(0.0, |edge, track| {
                *edge += track.width;
                Some(*edge)
            })
            .collect();
        assert_eq!(boundaries, vec![120.0, 200.0, 290.0, 360.0, 460.0, 560.0, 700.0]);
    }

    #[test]
    fn saved_column_widths_are_clamped_to_sensible_limits() {
        let mut settings = crate::state::Settings::default();
        settings.list_col_widths = vec![1.0, f32::NAN, 80.0, 90.0, 80.0, 90.0, 90.0, 5000.0];

        let widths = column_widths_from_settings(&settings);

        assert_eq!(widths[0], MIN_COLUMN_WIDTHS[0]);
        assert_eq!(widths[1], DEFAULT_COLUMN_WIDTHS[1]);
        assert_eq!(widths[2], 80.0);
        assert_eq!(widths[7], MAX_COLUMN_WIDTH);
    }

    #[test]
    fn column_resize_accumulates_frame_deltas_and_clamps_at_limits() {
        let column = 0;
        let width = resized_column_width(
            resized_column_width(200.0, 12.0, column),
            9.0,
            column,
        );
        assert_eq!(width, 221.0);
        assert_eq!(resized_column_width(width, -1000.0, column), MIN_COLUMN_WIDTHS[column]);
        assert_eq!(
            resized_column_width(MAX_COLUMN_WIDTH - 1.0, 10.0, column),
            MAX_COLUMN_WIDTH
        );
    }
}

fn render_toolbar_button(
    ui: &mut Ui,
    icon: &egui::TextureHandle,
    label: &str,
    text_color: Color32,
    width: f32,
) -> bool {
    render_toolbar_button_enabled(ui, icon, label, text_color, width, true)
}

fn render_toolbar_button_enabled(
    ui: &mut Ui,
    icon: &egui::TextureHandle,
    label: &str,
    text_color: Color32,
    width: f32,
    enabled: bool,
) -> bool {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 62.0), sense);

    if enabled && response.hovered() {
        ui.painter().rect_filled(rect.shrink(2.0), Rounding::same(4.0), bg_row_hover());
    }

    // Icon sits above its label, with the whole cell clickable.
    let icon_rect = Rect::from_center_size(
        pos2(rect.center().x, rect.top() + 22.0),
        Vec2::splat(34.0),
    );
    let icon_tint = if enabled { Color32::WHITE } else { text_dim() };
    ui.painter().image(
        icon.id(),
        icon_rect,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        icon_tint,
    );
    ui.painter().text(
        pos2(rect.center().x, rect.bottom() - 12.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(if width < 60.0 { 9.0 } else { 11.0 }),
        if enabled { text_color } else { text_dim() },
    );

    response
        .on_hover_text(label)
        .on_hover_cursor(CursorIcon::Default)
        .clicked()
}

/// Hairline divider between toolbar command groups.
fn toolbar_separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(12.0, 62.0), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range().shrink(9.0),
        Stroke::new(1.0, border_hairline()),
    );
}

fn render_sidebar_item(
    ui: &mut Ui,
    icon: &egui::TextureHandle,
    label: &str,
    count: usize,
    selected: bool,
    accent: Color32,
) -> bool {
    let bg_color = if selected { bg_pill_selected() } else { Color32::TRANSPARENT };
    let text_color = if selected { text_main() } else { text_dim() };
    let badge_bg = if selected { bg_badge_sel() } else { bg_badge() };
    let badge_text = if selected { accent } else { text_badge() };

    let resp = egui::Frame::none()
        .fill(bg_color)
        .rounding(Rounding::same(4.0))
        .inner_margin(Margin::symmetric(8.0, 4.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(20.0);
                ui.set_min_width(ui.available_width());
                ui.image(egui::load::SizedTexture::new(icon.id(), Vec2::new(15.0, 15.0)));
                ui.add_space(4.0);
                ui.label(RichText::new(label).size(12.5).color(text_color));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::Frame::none()
                        .fill(badge_bg)
                        .rounding(Rounding::same(8.0))
                        .inner_margin(Margin::symmetric(6.0, 1.5))
                        .show(ui, |ui| {
                            ui.label(RichText::new(count.to_string()).size(11.0).color(badge_text));
                        });
                });
            });
        }).response;

    resp.interact(Sense::click())
        .on_hover_cursor(CursorIcon::Default)
        .clicked()
}

/// One progress line divided into colored connection segments, like the
/// original DM Ultra progress window. Each segment owns a byte range of the
/// final file and is painted in its own color; together they form the full
/// progress bar.
fn render_segmented_progress(
    ui: &mut Ui,
    total: u64,
    downloaded: u64,
    segments: &[crate::manager::Segment],
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, Rounding::same(4.0), Color32::from_rgb(30, 40, 59));

    if total > 0 {
        let palette = [
            Color32::from_rgb(57, 133, 245),
            Color32::from_rgb(33, 196, 93),
            Color32::from_rgb(230, 165, 50),
            Color32::from_rgb(139, 92, 246),
            Color32::from_rgb(244, 63, 94),
            Color32::from_rgb(6, 182, 212),
            Color32::from_rgb(234, 179, 8),
            Color32::from_rgb(236, 72, 153),
        ];

        if segments.is_empty() {
            let filled = rect.left() + (downloaded as f32 / total as f32) * rect.width();
            painter.rect_filled(
                Rect::from_min_max(pos2(rect.left(), rect.top()), pos2(filled, rect.bottom())),
                Rounding::same(4.0),
                palette[0],
            );
        } else {
            for (index, segment) in segments.iter().enumerate() {
                let segment_len = if segment.len > 0 {
                    segment.len
                } else {
                    segment.end.saturating_sub(segment.start).max(1)
                };
                let segment_downloaded = segment.downloaded.min(segment_len);
                let x0 = rect.left() + (segment.start as f32 / total as f32) * rect.width();
                let x1 = rect.left()
                    + (segment.end.min(total) as f32 / total as f32) * rect.width();
                let filled_x = x0 + (segment_downloaded as f32 / segment_len as f32) * (x1 - x0);
                let color = palette[index % palette.len()];
                if filled_x > x0 {
                    painter.rect_filled(
                        Rect::from_min_max(pos2(x0, rect.top()), pos2(filled_x, rect.bottom())),
                        Rounding::same(0.0),
                        color,
                    );
                }
            }

            // Thin separators between connection ranges.
            for segment in segments {
                let x = rect.left() + (segment.start as f32 / total as f32) * rect.width();
                painter.vline(x, rect.y_range(), Stroke::new(1.0, bg_dark()));
            }
        }
    } else if downloaded > 0 {
        // Unknown total size: show an indeterminate filled bar.
        painter.rect_filled(rect.shrink(1.0), Rounding::same(4.0), Color32::from_rgb(57, 133, 245));
    }

    let percent = if total > 0 {
        (downloaded as f32 / total as f32 * 100.0).min(100.0)
    } else {
        0.0
    };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        format!("{percent:.0}%"),
        FontId::proportional(12.0),
        Color32::WHITE,
    );
    painter.rect_stroke(rect, Rounding::same(4.0), Stroke::new(1.0, border_hairline()));

    response
}

fn apply_app_style(ctx: &egui::Context) {
    // One complete, palette-driven override for the whole context (see
    // options::apply_global_theme_style).
    options::apply_global_theme_style(ctx, &current_palette());
    style_spacing(ctx);

    // Install native Windows Segoe UI fonts
    options::install_fonts(ctx);
}

/// Small spacing tweaks kept separate from the color theme.
fn style_spacing(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(6.0, 4.0);
    ctx.set_style(style);
}

fn is_file_type(path: &std::path::Path, exts: &[&str]) -> bool {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_lowercase();
        exts.iter().any(|&e| e == ext_lower)
    } else {
        false
    }
}

fn column_widths_from_settings(settings: &crate::state::Settings) -> Vec<f32> {
    let saved: &[f32] = if settings.list_col_widths.len() == COLUMN_LABELS.len() {
        &settings.list_col_widths
    } else {
        &DEFAULT_COLUMN_WIDTHS
    };
    saved
        .iter()
        .enumerate()
        .map(|(index, width)| {
            if width.is_finite() {
                width.clamp(MIN_COLUMN_WIDTHS[index], MAX_COLUMN_WIDTH)
            } else {
                DEFAULT_COLUMN_WIDTHS[index]
            }
        })
        .collect()
}

fn column_visible_from_settings(settings: &crate::state::Settings) -> Vec<bool> {
    if settings.list_col_visible.len() == 8 {
        settings.list_col_visible.clone()
    } else {
        vec![true; 8]
    }
}

fn file_name_of(download: &DownloadRec) -> String {
    download
        .save_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| download.url.clone())
}

fn sanitize_filename(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_control() || "<>:\"/\\|?*".contains(ch) {
            result.push('_');
        } else {
            result.push(ch);
        }
    }
    let trimmed = result.trim_matches(|ch| ch == ' ' || ch == '.');
    if trimmed.is_empty() {
        "download.bin".to_string()
    } else {
        trimmed.to_string()
    }
}

fn parse_optional_hm(input: &str) -> Result<Option<(u32, u32)>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let (hour, minute) = trimmed
        .split_once(':')
        .ok_or_else(|| "Use HH:MM for times.".to_string())?;
    let hour: u32 = hour
        .trim()
        .parse()
        .map_err(|_| "Hour must be a number.".to_string())?;
    let minute: u32 = minute
        .trim()
        .parse()
        .map_err(|_| "Minute must be a number.".to_string())?;
    if hour > 23 || minute > 59 {
        return Err("Time must be between 00:00 and 23:59.".to_string());
    }
    Ok(Some((hour, minute)))
}

/// Normalise the store's minimize-mode string. Unknown values (older state
/// files, hand edits) fall back to the IDM-like "taskbar + tray" behaviour.
fn normalize_minimize_mode(mode: &str) -> &'static str {
    match mode {
        "taskbar" => "taskbar",
        "tray" => "tray",
        _ => "taskbar_tray",
    }
}

fn ui_accent(name: &str) -> Color32 {
    if name == "blue" || name.is_empty() {
        return ACCENT_BLUE;
    }
    options::accent_color(name)
}

fn queue_has_schedule(state: &State, queue: Option<&str>) -> bool {
    match queue {
        Some(queue) => state.schedules.iter().any(|schedule| schedule.queue == queue),
        None => false,
    }
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n as f64;
    let mut unit = 0usize;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

fn format_eta(remaining: u64, speed: u64) -> String {
    if speed == 0 {
        return "--".into();
    }
    let secs = remaining / speed;
    if secs > 24 * 3600 {
        return ">1 day".into();
    }
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
}

fn format_timestamp(ts: i64) -> String {
    if let Some(dt) = chrono::DateTime::from_timestamp(ts, 0) {
        let local: chrono::DateTime<chrono::Local> = chrono::DateTime::from(dt);
        local.format("%m/%d %-I:%M %p").to_string()
    } else {
        "".into()
    }
}

// ---------------------------------------------------------------------------
// System tray integration
// ---------------------------------------------------------------------------
fn create_tray_icon(show_flag: Arc<Mutex<bool>>) -> Result<tray_icon::TrayIcon> {
    let icon = generate_tray_icon()?;

    let show_item = MenuItem::new("Show DM Ultra", true, None);
    let quit_item = MenuItem::new("Quit", true, None);

    let menu = Menu::new();
    menu.append(&show_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit_item)?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .with_tooltip("DM Ultra")
        .build()?;
    crate::logfile::log("TRAY", "tray icon created");

    let show_id = show_item.id().clone();
    let quit_id = quit_item.id().clone();

    let menu_flag = show_flag.clone();
    std::thread::spawn(move || {
        let receiver = tray_icon::menu::MenuEvent::receiver();
        while let Ok(event) = receiver.recv() {
            if event.id == show_id {
                if let Ok(mut flag) = menu_flag.lock() {
                    *flag = true;
                }
                crate::browser::wake_main_window();
            } else if event.id == quit_id {
                std::process::exit(0);
            }
        }
    });

    // A left click (or double click) on the icon itself restores the main
    // window, the way IDM does; the menu remains available on right click.
    std::thread::spawn(move || {
        let receiver = tray_icon::TrayIconEvent::receiver();
        while let Ok(event) = receiver.recv() {
            if matches!(
                event.click_type,
                tray_icon::ClickType::Left | tray_icon::ClickType::Double
            ) {
                if let Ok(mut flag) = show_flag.lock() {
                    *flag = true;
                }
                crate::browser::wake_main_window();
            }
        }
    });

    Ok(tray)
}

fn generate_tray_icon() -> Result<TrayIcon> {
    const SIZE: u32 = 64;
    Ok(TrayIcon::from_rgba(application_icon_rgba(SIZE)?, SIZE, SIZE)?)
}

// ---------------------------------------------------------------------------
// Entry point called by main.rs
// ---------------------------------------------------------------------------

/// Native title-bar icon.  Without this eframe falls back to its default
/// egui logo, which is not the DM Ultra application icon.
fn application_icon_rgba(size: u32) -> Result<Vec<u8>> {
    const ICON_BYTES: &[u8] = include_bytes!("../assets/icons/icon.ico");
    let decoder = image::codecs::ico::IcoDecoder::new(std::io::Cursor::new(ICON_BYTES))?;
    let decoded = image::DynamicImage::from_decoder(decoder)?.to_rgba8();
    let resized = image::imageops::resize(
        &decoded,
        size,
        size,
        image::imageops::FilterType::Lanczos3,
    );
    Ok(resized.into_raw())
}

fn window_icon() -> Result<Arc<egui::IconData>> {
    const SIZE: u32 = 64;
    Ok(Arc::new(egui::IconData {
        rgba: application_icon_rgba(SIZE)?,
        width: SIZE,
        height: SIZE,
    }))
}

/// Marker file used to ask the running instance to show its main window.
/// A second `dmultra.exe` launch writes it and exits instead of starting a
/// duplicate GUI (two instances would fight over state.json).
fn show_request_path() -> std::path::PathBuf {
    crate::state::state_dir().join("show.request")
}

/// Ask an already-running instance to reveal its main window.
pub fn request_show() {
    let path = show_request_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, b"1");
}

/// Start the GUI.
///
/// `start_hidden` is used when the browser launches DM Ultra to service a
/// download link: the main window must not pop up, only the download
/// status window (and the tray icon) should appear.
pub fn launch(manager: Arc<Manager>, start_hidden: bool) -> Result<()> {
    // A request left behind by a previous (crashed) instance must not
    // pop the window open on this launch.
    let _ = std::fs::remove_file(show_request_path());
    let title = format!("DM Ultra v{}", APP_VERSION.trim());
    const DEFAULT_INNER: [f32; 2] = [840.0, 520.0];
    const MIN_INNER: [f32; 2] = [640.0, 420.0];

    // Where the window opens:
    //   * "Remember window positions" on + a saved spot -> exactly there
    //     (unless that spot is on a monitor that is no longer attached, in
    //     which case fall back to the centre of the screen);
    //   * otherwise -> centred on the screen's work area.
    let (remember, saved_pos, saved_size) = {
        let st = manager.lock_ui();
        (
            st.settings.remember_window_pos,
            st.settings.window_pos,
            st.settings.window_size,
        )
    };
    let inner_size = if remember {
        saved_size
            .filter(|size| size[0] >= MIN_INNER[0] && size[1] >= MIN_INNER[1])
            .unwrap_or(DEFAULT_INNER)
    } else {
        DEFAULT_INNER
    };
    let initial_pos = if remember {
        saved_pos
            .filter(|pos| monitor_bounds_ok(*pos, inner_size))
            .map(|pos| egui::pos2(pos[0], pos[1]))
            .or_else(|| initial_center_pos(inner_size))
    } else {
        initial_center_pos(inner_size)
    };

    let mut viewport = ViewportBuilder::default()
        .with_inner_size(inner_size)
        .with_min_inner_size(MIN_INNER)
        .with_title(title.clone())
        // The main window is ALWAYS created "visible" from eframe's point of
        // view, even for a browser-launched (background) instance. A
        // background launch hides it on the first frame with a raw Win32
        // `ShowWindow(SW_HIDE)` instead (see `update`). Letting eframe believe
        // the window is invisible stops it pumping queued messages for the
        // progress popouts, which freezes them (unclickable/unclosable).
        .with_visible(true)
        .with_icon(window_icon()?);
    if let Some(pos) = initial_pos {
        viewport = viewport.with_position(pos);
    }

    let options = eframe::NativeOptions {
        viewport,
        // Child popout windows are painted from the parent frame and each
        // swap waits for vblank when vsync is on, which multiplies the frame
        // cost by the number of open windows. The UI is not animated, so
        // painting freely is both faster and perfectly smooth.
        vsync: false,
        ..Default::default()
    };

    let app = DmApp::new(manager, start_hidden);
    // Browser clicks must be picked up even while the window is minimized:
    // the watcher nudges the (otherwise idle) event loop. See browser.rs.
    crate::browser::spawn_pending_waker();
    eframe::run_native(&title, options, Box::new(move |_cc| Box::new(app)))
    .map_err(|e| anyhow::anyhow!("GUI error: {e}"))
}

// ---------------------------------------------------------------------------
// "Open" action tests. `shell_open` is deliberately not called here: it hands
// the file to the registered application, which would spawn a real window from
// a test run.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod open_action_tests {
    use super::*;

    fn temp_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("dmultra-open-{label}-{}", std::process::id()))
    }

    #[test]
    fn a_download_with_no_saved_file_has_nothing_to_open() {
        assert!(
            open_action_blocker(std::path::Path::new("")).is_some(),
            "an empty save path must block the open action"
        );
    }

    #[test]
    fn a_file_that_is_not_on_disk_yet_cannot_be_opened() {
        let path = temp_path("missing");
        let _ = std::fs::remove_file(&path);
        let reason = open_action_blocker(&path).expect("a missing file must block the open action");
        assert!(reason.contains("not on disk yet"), "unexpected reason: {reason}");
    }

    #[test]
    fn a_saved_file_is_openable_while_a_folder_is_not() {
        let dir = temp_path("dir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the temp folder");
        let file = dir.join("download.bin");
        std::fs::write(&file, b"payload").expect("write the temp file");

        assert!(
            open_action_blocker(&file).is_none(),
            "a file on disk must be launchable"
        );
        let reason = open_action_blocker(&dir).expect("a folder is not a launchable file");
        assert!(reason.contains("folder"), "unexpected reason: {reason}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
