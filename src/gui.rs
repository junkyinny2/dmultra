//! DM Ultra GUI - High-fidelity implementation matching IDM / DM Ultra dark theme.
//!
//! Architectural invariants (see AGENTS.md):
//! - Download progress window, Add-URL dialog, Settings dialog, and Scheduler
//!   are ALWAYS their own OS-level popout windows (`ctx.show_viewport_immediate`),
//!   never in-app panes.
//! - GUI-thread state access uses `manager.lock_ui()` (bounded spin on
//!   `try_lock_owned`), NEVER tokio's `blocking_lock`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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
    pub const OUR_TITLES_EXACT: [&str; 4] = ["DM Ultra", "Add new download...", "Scheduler", "Options"];
    pub const OUR_TITLES_PREFIX: &str = "Download status - ";

    /// Dark title bar message ids (DWMWA_USE_IMMERSIVE_DARK_MODE).
    const DWMWA_USE_IMMERSIVE_DARK_MODE: c_int = 20;

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
        OUR_TITLES_EXACT.contains(&text) || text.starts_with(OUR_TITLES_PREFIX)
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

    /// Apply dark/light immersive title-bar mode to every window we own.
    /// Idempotent and cheap; call it whenever the palette changes (and
    /// periodically, so popouts opened since the last change are included).
    pub fn apply_titlebar_theme(dark: bool) {
        enumerate();
        let val: i32 = if dark { 1 } else { 0 };
        if let Ok(list) = COLLECTED.lock() {
            for (hwnd, text) in list.iter() {
                if !is_ours(text) {
                    continue;
                }
                unsafe {
                    DwmSetWindowAttribute(
                        *hwnd,
                        DWMWA_USE_IMMERSIVE_DARK_MODE,
                        &val,
                        std::mem::size_of::<i32>() as u32,
                    );
                }
            }
        }
    }

    /// Outer screen rect of the window with exactly `title`, if any.
    pub fn window_rect(title: &str) -> Option<(i32, i32, i32, i32)> {
        enumerate();
        if let Ok(list) = COLLECTED.lock() {
            for (hwnd, text) in list.iter() {
                if is_ours(text) && text == title {
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
    let (left, top, right, bottom) = native_win::window_rect("DM Ultra")?;
    // A minimized window parks at (-32000, -32000); do not center
    // relative to that.
    if left < -10_000 || right <= left || bottom <= top {
        return None;
    }
    let ppp = ctx.pixels_per_point();
    let to_logical = |v: i32| v as f32 / ppp;
    let main_w = to_logical(right - left);
    let main_h = to_logical(bottom - top);
    Some(egui::pos2(
        to_logical(left) + ((main_w - outer_w) * 0.5).max(0.0),
        to_logical(top) + ((main_h - outer_h) * 0.5).max(0.0),
    ))
}

const COLUMN_LABELS: [&str; 8] = [
    "File Name", "Queue", "Size", "Status", "Time left", "Transfer rate", "Last Try",
    "Description",
];
const DEFAULT_COLUMN_WIDTHS: [f32; 8] = [220.0, 28.0, 72.0, 90.0, 80.0, 90.0, 90.0, 120.0];

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
// Responsive download-list column widths
// ---------------------------------------------------------------------------
#[derive(Clone, Copy)]
struct ColumnWidths {
    name: f32,
    queue: f32,
    size: f32,
    status: f32,
    time_left: f32,
    transfer: f32,
    last_try: f32,
    description: f32,
}

impl ColumnWidths {
    fn new(content_width: f32) -> Self {
        // Fixed columns keep their reference proportions; the file-name
        // column absorbs all remaining space so the table always spans the
        // full width of the download-list region.
        let queue = 24.0;
        let size = 68.0;
        let status = 88.0;
        let time_left = 78.0;
        let transfer = 88.0;
        let last_try = 88.0;
        let description = 104.0;
        let fixed = queue + size + status + time_left + transfer + last_try + description;
        let name = (content_width - fixed).max(110.0);
        Self {
            name,
            queue,
            size,
            status,
            time_left,
            transfer,
            last_try,
            description,
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
    url: String,
    save_as: String,
    queue: String,
    description: String,
    connections: u32,
    error: String,
    /// Centers the OS window on the main window once per open.
    positioned: bool,
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
    /// Centers the OS window on the main window once per open.
    positioned: bool,
}

// ---------------------------------------------------------------------------
// Main Application State
// ---------------------------------------------------------------------------
pub struct DmApp {
    manager: Arc<Manager>,
    show_flag: Arc<Mutex<bool>>,
    show_main_window: bool,
    tray_icon: Option<tray_icon::TrayIcon>,

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
    active_details_popouts: Vec<u64>, // IDs of open progress popout viewports
    about_open: bool,
    delete_confirm: DeleteConfirmState,

    // Icons & accent
    icons: IconCache,
    accent: Color32,

    // Live transfer-rate sampling (per download id)
    speed_samples: HashMap<u64, (Instant, u64)>,
    speeds: HashMap<u64, f64>,

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

    // Native title-bar theming throttle.
    titlebar_last_apply: Instant,

    // Resizable/reorderable download-list columns
    column_widths: Vec<f32>,
    column_visible: Vec<bool>,
    /// Columns/widths used to lay out the table this frame (see
    /// update_column_geometry).
    display_columns: Vec<usize>,
    display_widths: Vec<f32>,
}

impl DmApp {
    pub fn new(manager: Arc<Manager>) -> Self {
        let show_flag = Arc::new(Mutex::new(false));
        let tray_icon = create_tray_icon(show_flag.clone()).ok();
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

        Self {
            manager,
            show_flag,
            show_main_window: true,
            tray_icon,
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
            active_details_popouts: Vec::new(),
            about_open: false,
            delete_confirm: DeleteConfirmState::default(),
            icons: IconCache::new(),
            accent,
            speed_samples: HashMap::new(),
            speeds: HashMap::new(),
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
            titlebar_last_apply: Instant::now() - Duration::from_secs(5),
            column_widths,
            column_visible,
            display_columns: (0..COLUMN_LABELS.len()).collect(),
            display_widths: DEFAULT_COLUMN_WIDTHS.to_vec(),
        }
    }

    fn open_add_url(&mut self) {
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
        self.add_dialog.open = true;
        self.add_dialog.save_as = default_dir.to_string_lossy().to_string();
        self.add_dialog.url.clear();
        self.add_dialog.description.clear();
        self.add_dialog.queue = default_queue;
        self.add_dialog.connections = connections;
        self.add_dialog.error.clear();
        self.add_dialog.positioned = false;
    }

    /// Open the Add-URL window prefilled with a URL (clipboard watcher
    /// hook, IDM-style "link copied -> download box").
    fn open_add_url_with(&mut self, url: String) {
        self.open_add_url();
        self.add_dialog.url = url;
        self.status_bar = "Download link found in the clipboard.".into();
    }

    /// Poll the clipboard at most twice per second. A newly seen
    /// http/https/ftp link opens the Add-URL window prefilled. The
    /// clipboard is not touched while the dialog is already open, and
    /// the launch-time clipboard is seeded so stale links do not pop
    /// the dialog on startup.
    fn tick_clipboard_watcher(&mut self) {
        if self.add_dialog.open || self.clip_last_poll.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.clip_last_poll = Instant::now();
        let text = arboard::Clipboard::new()
            .and_then(|mut c| c.get_text())
            .unwrap_or_default();
        if text.is_empty() || text == self.last_clip_text {
            return;
        }
        self.last_clip_text = text.clone();
        let urls = crate::clipboard::extract_urls(&text);
        if let Some(url) = urls.into_iter().next() {
            self.open_add_url_with(url);
        }
    }

    fn open_settings(&mut self) {
        let (settings, webhooks) = {
            let st = self.manager.lock_ui();
            (st.settings.clone(), st.webhooks.clone())
        };
        self.settings_dialog
            .open_with(&settings, options::system_prefers_dark(), webhooks);
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
        self.delete_confirm.positioned = false;
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
        self.delete_confirm.positioned = false;
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

    fn start_queue_manual(&mut self) {
        if self.manager.queue_running() {
            self.status_bar = "Queue processing is already running.".into();
            return;
        }
        self.scheduler_suppressed.remove(MAIN_QUEUE);
        self.active_queue = Some(MAIN_QUEUE.to_string());
        self.manager.start_queue(MAIN_QUEUE.to_string());
        self.status_bar = format!("Started queue \"{MAIN_QUEUE}\".");
    }

    fn stop_queue_manual(&mut self) {
        if !self.manager.queue_running() {
            self.status_bar = "Queue processing is not running.".into();
            return;
        }
        self.manager.queue_stop();
        self.scheduler_suppressed.insert(MAIN_QUEUE.to_string());
        self.scheduler_started.remove(MAIN_QUEUE);
        self.active_queue = None;
        self.status_bar = format!("Stopped queue \"{MAIN_QUEUE}\".");
    }

    fn queued_count_for(&self, queue: &str) -> usize {
        let st = self.manager.lock_ui();
        st.downloads
            .iter()
            .filter(|d| d.status == Status::Queued && d.queue.as_deref() == Some(queue))
            .count()
    }

    fn update_column_geometry(&mut self, available: f32) {
        // Columns/widths used to lay out the table this frame. User widths
        // are honored exactly (min 40 px). When they do not fill the window
        // the last visible column stretches; when they overflow the window
        // the table scrolls horizontally (IDM-style).
        let visible: Vec<usize> = (0..COLUMN_LABELS.len())
            .filter(|&index| self.column_visible.get(index).copied().unwrap_or(true))
            .collect();
        let mut widths: Vec<f32> = visible
            .iter()
            .map(|&index| {
                self.column_widths
                    .get(index)
                    .copied()
                    .unwrap_or(DEFAULT_COLUMN_WIDTHS[index])
            })
            .collect();
        let sum: f32 = widths.iter().sum();
        if sum < available {
            if let Some(last) = widths.last_mut() {
                *last = (*last + (available - sum)).max(40.0);
            }
        }
        self.display_columns = visible;
        self.display_widths = widths;
    }

    fn save_column_settings(&mut self) {
        let mut state = self.manager.lock_ui();
        state.settings.list_col_widths = self.column_widths.clone();
        state.settings.list_col_visible = self.column_visible.clone();
        let _ = state.save();
    }

    /// Sample live transfer rates from the manager's byte counters. The
    /// display is smoothed over ~0.4 s samples so it stays readable.
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
                    if elapsed >= Duration::from_millis(400) {
                        let dt = elapsed.as_secs_f64().max(0.001);
                        let instant = bytes.saturating_sub(previous) as f64 / dt;
                        let smoothed = self
                            .speeds
                            .get(&id)
                            .copied()
                            .map(|value| value * 0.55 + instant * 0.45)
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
        // Handle background tray icon click
        if let Ok(mut flag) = self.show_flag.lock() {
            if *flag {
                self.show_main_window = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                *flag = false;
            }
        }

        // Minimize to tray on close if user wants to keep running
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.show_main_window {
                self.show_main_window = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        // Periodic manager reap and repaint request
        if self.last_update.elapsed() > Duration::from_millis(500) {
            self.manager.reap();
            self.last_update = Instant::now();
            ctx.request_repaint();
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

        // Native title bars (main window + popout windows) follow the
        // dark-mode theme. Re-applied at 1 Hz so popouts opened since
        // the last pass are themed too.
        let dark = options::Appearance::from_settings(&settings_snapshot, system_dark).is_dark();
        if self.titlebar_last_apply.elapsed() > Duration::from_secs(1) {
            self.titlebar_last_apply = Instant::now();
            #[cfg(target_os = "windows")]
            {
                native_win::apply_titlebar_theme(dark);
            }
        }

        // Clipboard watcher: when enabled, copying a link opens the
        // Add-URL window (IDM behaviour).
        if settings_snapshot.monitor_clipboard {
            self.tick_clipboard_watcher();
        } else {
            self.clip_last_poll = Instant::now() - Duration::from_secs(5);
        }

        // Live transfer-rate sampling and schedule enforcement run before
        // the frame is built so the table and status bar show current data.
        self.sample_speeds();
        self.scheduler_tick();

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
                        if ui.button("Start Queue Processing").clicked() {
                            self.start_queue_manual();
                        }
                        if ui.button("Stop Queue Processing").clicked() {
                            self.stop_queue_manual();
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
                // Nine command buttons share one compact left-aligned row,
                // split into command groups by two hairline separators.
                let button_w = ((ui.available_width() - 32.0) / 9.0).clamp(64.0, 72.0);
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
                    let icon_stop = self.icons.get_or_render(ctx, "stop", SVG_STOP_ALL, text_dim(), 34);
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
                });

                // Subtle bottom rule instead of a full frame border.
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.max_rect().bottom(),
                    Stroke::new(1.0, border_hairline()),
                );
            });

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
                let viewport_width = ui.available_width();
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Column geometry, computed once per frame. Overflow
                        // behavior follows IDM: columns keep their set widths and
                        // the table scrolls horizontally; the last visible column
                        // absorbs leftover space when there is any.
                        let available = viewport_width - 16.0;
                        self.update_column_geometry(available);
                        let visible_columns = self.display_columns.clone();
                        let column_widths = self.display_widths.clone();

                // Table header (single compact 27 px row)
                egui::Frame::none()
                    .fill(bg_header())
                    .inner_margin(Margin::symmetric(8.0, 0.0))
                    .show(ui, |ui| {
                        ui.set_height(27.0);
                        ui.spacing_mut().item_spacing.x = 0.0;
                        // Column-boundary rules: (x center, hovered, dragging).
                        let mut bounds: Vec<(f32, bool, bool)> = Vec::new();
                        ui.horizontal_centered(|ui| {
                            for (position, &column) in visible_columns.iter().enumerate() {
                                let width = column_widths[position];
                                if column == 1 {
                                    let search_icon = self.icons.get_or_render(ctx, "search", SVG_SEARCH, text_dim(), 14);
                                    let (icon_rect, icon_resp) = ui.allocate_exact_size(
                                        Vec2::splat(16.0),
                                        Sense::click(),
                                    );
                                    ui.painter().image(
                                        search_icon.id(),
                                        icon_rect,
                                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                        Color32::WHITE,
                                    );
                                    self.search_icon_rect = icon_rect;
                                    if icon_resp
                                        .on_hover_text("Search downloads")
                                        .on_hover_cursor(CursorIcon::Default)
                                        .clicked()
                                    {
                                        self.search_open = !self.search_open;
                                        self.search_focus = self.search_open;
                                    }
                                    if width > 16.0 {
                                        ui.add_space(width - 16.0);
                                    }
                                } else {
                                    let label = COLUMN_LABELS[column];
                                    if self.table_header_cell(ui, label, column, width) {
                                        self.toggle_sort(column);
                                    }
                                }

                                // Draggable boundary between columns. Dragging moves
                                // the boundary itself (IDM-style): only the column to
                                // its left changes width and later columns shift.
                                if position + 1 < visible_columns.len() {
                                    let (handle_rect, handle_resp) = ui.allocate_exact_size(
                                        Vec2::new(6.0, 27.0),
                                        Sense::click_and_drag(),
                                    );
                                    bounds.push((
                                        handle_rect.center().x,
                                        handle_resp.hovered(),
                                        handle_resp.dragged(),
                                    ));
                                    if handle_resp.dragged() {
                                        let left = visible_columns[position];
                                        let old_left = self.column_widths[left];
                                        self.column_widths[left] =
                                            (old_left + handle_resp.drag_delta().x).max(40.0);
                                        self.update_column_geometry(available);
                                    }
                                    if handle_resp.drag_stopped() {
                                        self.save_column_settings();
                                    }
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
                            ui.max_rect().x_range(),
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
                    // Empty state centred inside the download-list body, not
                    // the whole window (the sidebar must not affect it).
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
                } else {
                    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
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
                                .inner_margin(Margin::symmetric(8.0, 5.0))
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.x = 0.0;
                                    ui.horizontal(|ui| {
                                        for (position, &column) in visible_columns.iter().enumerate() {
                                            let width = column_widths[position];
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

                                            if position + 1 < visible_columns.len() {
                                                let (handle_rect, _resp) = ui
                                                    .allocate_exact_size(
                                                        Vec2::new(6.0, 18.0),
                                                        Sense::hover(),
                                                    );
                                                row_bounds.push(handle_rect.center().x);
                                            }
                                        }
                                    });
                                })
                                .response
                                .on_hover_cursor(CursorIcon::Default);

                            // IDM-style grid rules: vertical cell separators for this
                            // row and a rule under it, spanning the full row height.
                            {
                                let rect = row_resp.rect;
                                for &x in &row_bounds {
                                    ui.painter().vline(x, rect.y_range(), Stroke::new(1.0, grid_line()));
                                }
                                ui.painter().hline(
                                    rect.x_range(),
                                    rect.bottom(),
                                    Stroke::new(1.0, grid_line()),
                                );
                            }

                            // Handle selection and double-click to open popout
                            if row_resp.interact(Sense::click()).clicked() {
                                self.selected_download_id = Some(d.id);
                            }
                            if row_resp.double_clicked() {
                                if !self.active_details_popouts.contains(&d.id) {
                                    self.active_details_popouts.push(d.id);
                                }
                            }

                            // Context menu for the row.
                            row_resp.context_menu(|ui| {
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
                                    let _ = std::process::Command::new("explorer").arg(&d.save_path).spawn();
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
                let mut st = self.manager.lock_ui();
                st.settings = new_settings;
                let _ = st.save();
                self.status_bar = "Settings saved.".into();
            }
            if let Some(webhooks) = outcome.webhooks {
                let mut st = self.manager.lock_ui();
                st.webhooks = webhooks;
                let _ = st.save();
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
        }

        // C. Scheduler Dialog (Separate OS Window)
        if self.scheduler_dialog.open {
            self.show_scheduler_viewport(ctx);
        }

        // D. Download Progress Popout Windows (Separate OS Windows)
        let popout_ids = self.active_details_popouts.clone();
        for id in popout_ids {
            self.show_progress_viewport(ctx, id);
        }

        // E. In-app overlays that are not download flows.
        self.render_search_popup(ctx);
        self.show_about(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.tray_icon = None;
    }

    /// Paint any area the UI does not cover (e.g. during a viewport resize)
    /// in the active theme background instead of a foreign color.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        bg_dark().to_normalized_gamma_f32()
    }
}

// ---------------------------------------------------------------------------
// Separate Viewports (OS Windows) - AGENTS.md Conformance
// ---------------------------------------------------------------------------
impl DmApp {
    fn show_add_url_viewport(&mut self, ctx: &egui::Context) {
        let viewport_id = ViewportId::from_hash_of("dm-add");
        let mut close_requested = false;

        ctx.show_viewport_immediate(
            viewport_id,
            ViewportBuilder::default()
                .with_title("Add new download...")
                .with_inner_size([500.0, 340.0])
                .with_min_inner_size([440.0, 300.0])
                .with_always_on_top(),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    close_requested = true;
                }
                // Center the OS window over the main program window
                // (once per open; `open_add_url` resets the flag).
                if !self.add_dialog.positioned {
                    #[cfg(target_os = "windows")]
                    {
                        if let Some(pos) = centered_on_main_pos(ctx, 506.0, 380.0) {
                            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
                            self.add_dialog.positioned = true;
                        }
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        self.add_dialog.positioned = true;
                    }
                }
                egui::CentralPanel::default().frame(egui::Frame::none().fill(bg_panel()).inner_margin(Margin::same(14.0))).show(ctx, |ui| {
                    ui.label(RichText::new("Enter new address to download").size(14.0).strong().color(text_main()));
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
                    ui.add_space(12.0);

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
                });
            },
        );

        if close_requested {
            self.add_dialog.open = false;
        }
    }

    /// Delete-confirmation window (a separate OS window like the other
    /// download dialogs). Offers the checkbox that also removes the
    /// file(s) from the disk, not just from the download list.
    fn show_delete_confirm_viewport(&mut self, ctx: &egui::Context) {
        let viewport_id = ViewportId::from_hash_of("dm-delete-confirm");
        let mut close_requested = false;

        ctx.show_viewport_immediate(
            viewport_id,
            ViewportBuilder::default()
                .with_title("Confirm Delete")
                .with_inner_size([440.0, 196.0])
                .with_min_inner_size([440.0, 196.0])
                .with_always_on_top(),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    close_requested = true;
                }
                // Center the OS window over the main program window
                // (once per open).
                if !self.delete_confirm.positioned {
                    #[cfg(target_os = "windows")]
                    {
                        if let Some(pos) = centered_on_main_pos(ctx, 446.0, 236.0) {
                            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
                            self.delete_confirm.positioned = true;
                        }
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        self.delete_confirm.positioned = true;
                    }
                }
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
        }
    }

    fn create_download(&mut self, start_now: bool) -> bool {
        self.add_dialog.error.clear();
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
            if let Err(error) = self.manager.start(rec, None, None) {
                self.status_bar = format!("Added download #{id}; could not start: {error}");
            } else {
                self.status_bar = format!("Added and started download #{id}.");
            }
        } else {
            self.status_bar = format!("Queued download #{id}.");
        }
        true
    }

    fn show_scheduler_viewport(&mut self, ctx: &egui::Context) {
        let viewport_id = ViewportId::from_hash_of("dm-scheduler");
        let mut close_requested = false;

        ctx.show_viewport_immediate(
            viewport_id,
            ViewportBuilder::default()
                .with_title("Scheduler")
                .with_inner_size([480.0, 430.0])
                .with_min_inner_size([420.0, 380.0])
                .with_always_on_top(),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    close_requested = true;
                }
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

            ctx.show_viewport_immediate(
                viewport_id,
                ViewportBuilder::default()
                    .with_title(format!("Download status - {}", filename))
                    .with_inner_size([500.0, 360.0])
                    .with_min_inner_size([440.0, 300.0])
                    .with_always_on_top(),
                |ctx, _class| {
                    if ctx.input(|i| i.viewport().close_requested()) {
                        close_requested = true;
                    }
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
                            if ui.button(" Open ").clicked() {
                                let _ = std::process::Command::new("explorer").arg(&d.save_path).spawn();
                            }
                            if ui.button(" Open Folder ").clicked() {
                                if let Some(parent) = d.save_path.parent() {
                                    let _ = std::process::Command::new("explorer").arg(parent).spawn();
                                }
                            }
                            if ui.button(" Close ").clicked() {
                                close_requested = true;
                            }
                        });
                    });
                },
            );
        } else {
            close_requested = true;
        }

        if close_requested {
            self.active_details_popouts.retain(|&pop_id| pop_id != id);
        }
    }

    fn table_header_cell(&self, ui: &mut Ui, label: &str, col: usize, width: f32) -> bool {
        let is_sorted = self.sort_column == col;
        let arrow = if is_sorted {
            if self.sort_descending { " ▼" } else { " ▲" }
        } else {
            ""
        };
        let text = format!("{}{}", label, arrow);
        let color = if is_sorted { text_main() } else { text_dim() };

        // Left-align each heading at the start of its column, like the
        // reference table (egui buttons would center the text instead).
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 20.0), Sense::click());
        ui.painter().text(
            pos2(rect.left(), rect.center().y),
            Align2::LEFT_CENTER,
            text,
            FontId::proportional(12.0),
            color,
        );
        response.on_hover_cursor(CursorIcon::Default).clicked()
    }

    fn toggle_sort(&mut self, col: usize) {
        if self.sort_column == col {
            self.sort_descending = !self.sort_descending;
        } else {
            self.sort_column = col;
            self.sort_descending = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers & Components
// ---------------------------------------------------------------------------

/// Left-aligned table-cell label. `ui.add_sized` uses a centered layout,
/// which pushed short cell text to the middle whenever a column was
/// stretched; IDM keeps all cell text flush left regardless of width.
fn cell_label_left(ui: &mut Ui, width: f32, label: egui::Label) {
    ui.allocate_ui_with_layout(
        Vec2::new(width, 18.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            // Hard cap so wide content truncates at the cell boundary
            // instead of spilling under the grid rule.
            ui.set_max_width(width);
            ui.add(label);
        },
    );
}

fn render_toolbar_button(
    ui: &mut Ui,
    icon: &egui::TextureHandle,
    label: &str,
    text_color: Color32,
    width: f32,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 62.0), Sense::click());

    if response.hovered() {
        ui.painter().rect_filled(rect.shrink(2.0), Rounding::same(4.0), bg_row_hover());
    }

    // Icon sits above its label, with the whole cell clickable.
    let icon_rect = Rect::from_center_size(
        pos2(rect.center().x, rect.top() + 22.0),
        Vec2::splat(34.0),
    );
    ui.painter().image(
        icon.id(),
        icon_rect,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    ui.painter().text(
        pos2(rect.center().x, rect.bottom() - 12.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        text_color,
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
    if settings.list_col_widths.len() == 8 {
        let mut widths = settings.list_col_widths.clone();
        for (index, width) in widths.iter_mut().enumerate() {
            if !width.is_finite() || *width < 40.0 {
                *width = DEFAULT_COLUMN_WIDTHS[index];
            }
        }
        widths
    } else {
        DEFAULT_COLUMN_WIDTHS.to_vec()
    }
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

    let show_id = show_item.id().clone();
    let quit_id = quit_item.id().clone();

    std::thread::spawn(move || {
        let receiver = tray_icon::menu::MenuEvent::receiver();
        while let Ok(event) = receiver.recv() {
            if event.id == show_id {
                if let Ok(mut flag) = show_flag.lock() {
                    *flag = true;
                }
            } else if event.id == quit_id {
                std::process::exit(0);
            }
        }
    });

    Ok(tray)
}

fn generate_tray_icon() -> Result<TrayIcon> {
    const S: u32 = 64;
    let mut rgba = vec![0u8; (S * S * 4) as usize];
    for y in 0..S {
        for x in 0..S {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let dx = (fx - 32.0).abs();
            let dy = (fy - 32.0).abs();
            let mut c = [0u8, 0, 0, 0];
            if dx <= 28.0 && dy <= 28.0 && (dx <= 22.0 || dy <= 22.0) {
                c = [37, 131, 247, 255]; // Blue
            }
            let bolt = |px: f32, py: f32| -> bool {
                let in_head = px >= 26.0 && px <= 38.0 && py >= 8.0 && py <= 24.0 && (px - 32.0).abs() <= (py - 8.0) * 0.8;
                let in_mid = px >= 20.0 && px <= 38.0 && py >= 24.0 && py <= 30.0;
                let in_tail = px >= 20.0 && px <= 34.0 && py >= 30.0 && py <= 56.0 && (px - 27.0).abs() <= (56.0 - py) * 0.55 + 1.2;
                in_head || in_mid || in_tail
            };
            if bolt(fx, fy) {
                c = [255, 255, 255, 255];
            }
            let i = ((y * S + x) * 4) as usize;
            rgba[i] = c[0];
            rgba[i + 1] = c[1];
            rgba[i + 2] = c[2];
            rgba[i + 3] = c[3];
        }
    }
    Ok(TrayIcon::from_rgba(rgba, S, S)?)
}

// ---------------------------------------------------------------------------
// Entry point called by main.rs
// ---------------------------------------------------------------------------

/// Native title-bar icon.  Without this eframe falls back to its default
/// egui logo, which is not the DM Ultra application icon.
fn window_icon() -> Arc<egui::IconData> {
    const ICON_BYTES: &[u8] = include_bytes!("../assets/icons/icon.ico");

    // `IcoDecoder::new` already selects the best (largest/highest-quality)
    // entry inside the ICO, so a single decode is enough.  Ask for 64 px so
    // the title bar has a crisp source at both 100% and 200% DPI.
    let decoded = image::codecs::ico::IcoDecoder::new(std::io::Cursor::new(ICON_BYTES))
        .ok()
        .and_then(|decoder| image::DynamicImage::from_decoder(decoder).ok())
        .map(|img| img.to_rgba8())
        .unwrap_or_else(|| {
            image::RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 0]))
        });
    let resized = image::imageops::resize(&decoded, 64, 64, image::imageops::FilterType::Lanczos3);

    Arc::new(egui::IconData {
        rgba: resized.into_raw(),
        width: 64,
        height: 64,
    })
}

pub fn launch(manager: Arc<Manager>) -> Result<()> {
    let viewport = ViewportBuilder::default()
        // Outer window lands at roughly the 916 x 605 reference size once
        // the native title bar and frame are added.
        .with_inner_size([900.0, 566.0])
        .with_min_inner_size([720.0, 480.0])
        .with_title("DM Ultra")
        .with_icon(window_icon());

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let app = DmApp::new(manager);
    eframe::run_native("DM Ultra", options, Box::new(move |_cc| Box::new(app)))
        .map_err(|e| anyhow::anyhow!("GUI error: {e}"))
}

