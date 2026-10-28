//! DM Ultra GUI - High-fidelity implementation matching IDM / DM Ultra dark theme.
//!
//! Architectural invariants (see AGENTS.md):
//! - Download progress window, Add-URL dialog, Settings dialog, and Scheduler
//!   are ALWAYS their own OS-level popout windows (`ctx.show_viewport_immediate`),
//!   never in-app panes.
//! - GUI-thread state access uses `manager.lock_ui()` (bounded spin on
//!   `try_lock_owned`), NEVER tokio's `blocking_lock`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use eframe::egui::{
    self, Align2, Color32, FontId, Margin, Rect, RichText, Rounding, Sense, Stroke, Ui, Vec2,
    ViewportBuilder, ViewportId, pos2, vec2,
};
use tray_icon::{
    icon::Icon as TrayIcon,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    TrayIconBuilder,
};

use crate::manager::Manager;
use crate::options::{self, SettingsDialog};
use crate::queue::MAIN_QUEUE;
use crate::state::{DownloadRec, Status};

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
// Theme Palette (sampled directly from the reference dark UI)
// ---------------------------------------------------------------------------
const BG_DARK: Color32 = Color32::from_rgb(15, 23, 42);        // #0F172A main window bg
const BG_PANEL: Color32 = Color32::from_rgb(20, 29, 48);       // #141D30 toolbar & sidebar bg
const BG_HEADER: Color32 = Color32::from_rgb(20, 29, 48);      // #141D30 table header
const BG_ROW_EVEN: Color32 = Color32::from_rgb(30, 40, 59);    // #1E283B alternating row
const BG_ROW_ODD: Color32 = Color32::from_rgb(32, 43, 62);     // #202B3E alternating row
const BG_ROW_HOVER: Color32 = Color32::from_rgb(38, 51, 72);
const BG_ROW_SELECTED: Color32 = Color32::from_rgb(48, 61, 81); // #303D51
const BG_PILL_SELECTED: Color32 = Color32::from_rgb(48, 61, 81); // sidebar selection capsule
const BG_BADGE: Color32 = Color32::from_rgb(38, 50, 70);
const BG_BADGE_SEL: Color32 = Color32::from_rgb(48, 61, 81);

const BORDER_HAIRLINE: Color32 = Color32::from_rgb(42, 54, 74);

const TEXT_MAIN: Color32 = Color32::from_rgb(214, 217, 224);   // #D6D9E0
const TEXT_DIM: Color32 = Color32::from_rgb(146, 153, 165);    // #9299A5
const TEXT_BADGE: Color32 = Color32::from_rgb(154, 166, 184);
const TEXT_EMPTY_TITLE: Color32 = Color32::from_rgb(146, 153, 165);
const TEXT_EMPTY_SUB: Color32 = Color32::from_rgb(122, 132, 148);

const ACCENT_BLUE: Color32 = Color32::from_rgb(57, 133, 245);  // #3985F5 (Add URL)
const ACCENT_GREEN: Color32 = Color32::from_rgb(33, 196, 93);  // #21C45D (Resume All)
const ACCENT_RED: Color32 = Color32::from_rgb(216, 64, 66);    // #D84042 (Delete All)
const STATUS_COMPLETE: Color32 = Color32::from_rgb(33, 196, 93);
const STATUS_DOWNLOADING: Color32 = Color32::from_rgb(57, 133, 245);
const STATUS_PAUSED: Color32 = Color32::from_rgb(230, 165, 50);
const STATUS_FAILED: Color32 = Color32::from_rgb(235, 75, 75);

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
    // Bottom docked
    Trash,
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
            NavCategory::Trash        => "Trash",
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
}

#[derive(Default)]
struct SchedulerDialogState {
    open: bool,
    start_time: String,
    stop_time: String,
    once: bool,
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
    selected_download_id: Option<u64>,

    // Popouts & Child windows
    add_dialog: AddUrlDialogState,
    settings_dialog: SettingsDialog,
    scheduler_dialog: SchedulerDialogState,
    active_details_popouts: Vec<u64>, // IDs of open progress popout viewports

    // Icons
    icons: IconCache,

    // Status bar & Sort
    status_bar: String,
    sort_column: usize,
    sort_descending: bool,
    last_update: Instant,
}

impl DmApp {
    pub fn new(manager: Arc<Manager>) -> Self {
        let show_flag = Arc::new(Mutex::new(false));
        let tray_icon = create_tray_icon(show_flag.clone()).ok();

        Self {
            manager,
            show_flag,
            show_main_window: true,
            tray_icon,
            selected_nav: NavCategory::AllDownloads,
            categories_visible: true,
            search_query: String::new(),
            selected_download_id: None,
            add_dialog: AddUrlDialogState {
                connections: 8,
                queue: MAIN_QUEUE.to_string(),
                ..Default::default()
            },
            settings_dialog: SettingsDialog::default(),
            scheduler_dialog: SchedulerDialogState {
                start_time: "02:00".into(),
                stop_time: "07:00".into(),
                ..Default::default()
            },
            active_details_popouts: Vec::new(),
            icons: IconCache::new(),
            status_bar: "Ready.".to_string(),
            sort_column: 6, // Last Try
            sort_descending: true,
            last_update: Instant::now(),
        }
    }

    fn open_add_url(&mut self) {
        let default_dir = {
            let st = self.manager.lock_ui();
            st.settings.save_path.clone()
        };
        self.add_dialog.open = true;
        self.add_dialog.save_as = default_dir.to_string_lossy().to_string();
        self.add_dialog.url.clear();
        self.add_dialog.description.clear();
    }

    fn open_settings(&mut self) {
        let (settings, webhooks) = {
            let st = self.manager.lock_ui();
            (st.settings.clone(), st.webhooks.clone())
        };
        self.settings_dialog.open_with(&settings, true, webhooks);
    }

    fn open_scheduler(&mut self) {
        self.scheduler_dialog.open = true;
    }

    fn start_selected(&mut self) {
        if let Some(id) = self.selected_download_id {
            let rec = {
                let st = self.manager.lock_ui();
                st.get(id).cloned()
            };
            if let Some(mut rec) = rec {
                if rec.status != Status::Downloading {
                    rec.status = Status::Downloading;
                    let _ = self.manager.start(rec, None, None);
                    self.status_bar = format!("Starting download #{}", id);
                }
            }
        }
    }

    fn pause_selected(&mut self) {
        if let Some(id) = self.selected_download_id {
            self.manager.stop(id);
            let mut st = self.manager.lock_ui();
            if let Some(d) = st.get_mut(id) {
                d.status = Status::Paused;
                let _ = st.save();
            }
            self.status_bar = format!("Paused download #{}", id);
        }
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
        self.status_bar = format!("Resumed {} download(s).", count);
    }

    fn stop_all(&mut self) {
        self.manager.stop_all();
        let mut st = self.manager.lock_ui();
        for d in &mut st.downloads {
            if d.status == Status::Downloading {
                d.status = Status::Paused;
            }
        }
        let _ = st.save();
        self.status_bar = "All downloads stopped.".to_string();
    }

    fn delete_selected(&mut self) {
        if let Some(id) = self.selected_download_id {
            self.manager.stop(id);
            let mut st = self.manager.lock_ui();
            if let Some(idx) = st.downloads.iter().position(|d| d.id == id) {
                let rec = st.downloads.remove(idx);
                st.trash.push(rec);
                let _ = st.save();
                self.status_bar = format!("Moved download #{} to Trash.", id);
            }
            self.selected_download_id = None;
        }
    }

    fn delete_all(&mut self) {
        self.manager.stop_all();
        let mut st = self.manager.lock_ui();
        let count = st.downloads.len();
        while let Some(rec) = st.downloads.pop() {
            st.trash.push(rec);
        }
        let _ = st.save();
        self.selected_download_id = None;
        self.status_bar = format!("Moved all {} download(s) to Trash.", count);
    }

    fn count_for_category(&self, cat: NavCategory) -> usize {
        let st = self.manager.lock_ui();
        match cat {
            NavCategory::AllDownloads => st.downloads.len(),
            NavCategory::InProgress   => st.downloads.iter().filter(|d| d.status == Status::Downloading).count(),
            NavCategory::Completed    => st.downloads.iter().filter(|d| d.status == Status::Complete).count(),
            NavCategory::Queued       => st.downloads.iter().filter(|d| d.status == Status::Queued).count(),
            NavCategory::Paused       => st.downloads.iter().filter(|d| d.status == Status::Paused).count(),
            NavCategory::Documents    => st.downloads.iter().filter(|d| is_file_type(&d.save_path, &["pdf","doc","docx","txt","rtf","xls","xlsx","ppt","pptx"])).count(),
            NavCategory::Music        => st.downloads.iter().filter(|d| is_file_type(&d.save_path, &["mp3","wav","wma","m4a","aac","flac","ogg","aif"])).count(),
            NavCategory::Video        => st.downloads.iter().filter(|d| is_file_type(&d.save_path, &["mp4","mkv","avi","mov","wmv","flv","webm","3gp","mpeg","mpg"])).count(),
            NavCategory::Programs     => st.downloads.iter().filter(|d| is_file_type(&d.save_path, &["exe","msi","apk","iso","img","bin"])).count(),
            NavCategory::Compressed   => st.downloads.iter().filter(|d| is_file_type(&d.save_path, &["zip","rar","7z","tar","gz","bz2","xz"])).count(),
            NavCategory::Scheduled    => st.downloads.iter().filter(|d| d.status == Status::Queued && d.queue.is_some()).count(),
            NavCategory::Trash        => st.trash.len(),
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

        // Apply dark styling to egui context
        apply_app_style(ctx);

        // -------------------------------------------------------------------
        // 1. Menu Bar (Tasks, File, Downloads, View, Help)
        // -------------------------------------------------------------------
        egui::TopBottomPanel::top("menubar")
            .exact_height(27.0)
            .frame(egui::Frame::none().fill(BG_DARK).inner_margin(Margin::symmetric(8.0, 3.0)))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                egui::menu::bar(ui, |ui| {
                    ui.menu_button(RichText::new("Tasks").color(TEXT_MAIN), |ui| {
                        if ui.button("Exit").clicked() {
                            std::process::exit(0);
                        }
                    });

                    ui.menu_button(RichText::new("File").color(TEXT_MAIN), |ui| {
                        if ui.button("Open Save Folder").clicked() {
                            let save_dir = self.manager.lock_ui().settings.save_path.clone();
                            let _ = std::process::Command::new("explorer").arg(save_dir).spawn();
                        }
                    });

                    ui.menu_button(RichText::new("Downloads").color(TEXT_MAIN), |ui| {
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

                    ui.menu_button(RichText::new("View").color(TEXT_MAIN), |ui| {
                        let cat_label = if self.categories_visible { "Hide Categories" } else { "Show Categories" };
                        if ui.button(cat_label).clicked() {
                            self.categories_visible = !self.categories_visible;
                        }
                    });

                    ui.menu_button(RichText::new("Help").color(TEXT_MAIN), |ui| {
                        if ui.button("About DM Ultra").clicked() {
                            self.status_bar = "DM Ultra 1.0.0 (64-bit multi-threaded download manager)".to_string();
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
                    .fill(BG_PANEL)
                    .inner_margin(Margin::symmetric(6.0, 0.0)),
            )
            .show(ctx, |ui| {
                // Nine command buttons share one compact left-aligned row,
                // split into command groups by two hairline separators.
                let button_w = ((ui.available_width() - 32.0) / 9.0).clamp(64.0, 72.0);
                ui.spacing_mut().item_spacing = Vec2::ZERO;

                ui.horizontal_centered(|ui| {
                    ui.add_space(4.0);

                    // Add URL (blue plus)
                    let icon_add = self.icons.get_or_render(ctx, "add", SVG_ADD, ACCENT_BLUE, 30);
                    if render_toolbar_button(ui, &icon_add, "Add URL", ACCENT_BLUE, button_w) {
                        self.open_add_url();
                    }

                    toolbar_separator(ui);

                    // Start
                    let icon_start = self.icons.get_or_render(ctx, "start", SVG_START, TEXT_DIM, 27);
                    if render_toolbar_button(ui, &icon_start, "Start", TEXT_DIM, button_w) {
                        self.start_selected();
                    }

                    // Pause
                    let icon_pause = self.icons.get_or_render(ctx, "pause", SVG_PAUSE, TEXT_DIM, 27);
                    if render_toolbar_button(ui, &icon_pause, "Pause", TEXT_DIM, button_w) {
                        self.pause_selected();
                    }

                    // Stop All
                    let icon_stop = self.icons.get_or_render(ctx, "stop", SVG_STOP_ALL, TEXT_DIM, 27);
                    if render_toolbar_button(ui, &icon_stop, "Stop All", TEXT_DIM, button_w) {
                        self.stop_all();
                    }

                    // Resume All (green play)
                    let icon_resume = self.icons.get_or_render(ctx, "resume", SVG_RESUME_ALL, ACCENT_GREEN, 30);
                    if render_toolbar_button(ui, &icon_resume, "Resume All", ACCENT_GREEN, button_w) {
                        self.resume_all();
                    }

                    // Delete
                    let icon_del = self.icons.get_or_render(ctx, "delete", SVG_DELETE, TEXT_DIM, 27);
                    if render_toolbar_button(ui, &icon_del, "Delete", TEXT_DIM, button_w) {
                        self.delete_selected();
                    }

                    // Delete All
                    let icon_del_all = self.icons.get_or_render(ctx, "delete_all", SVG_DELETE_ALL, ACCENT_RED, 27);
                    if render_toolbar_button(ui, &icon_del_all, "Delete All", ACCENT_RED, button_w) {
                        self.delete_all();
                    }

                    toolbar_separator(ui);

                    // Options
                    let icon_opt = self.icons.get_or_render(ctx, "options", SVG_OPTIONS, TEXT_DIM, 27);
                    if render_toolbar_button(ui, &icon_opt, "Options", TEXT_DIM, button_w) {
                        self.open_settings();
                    }

                    // Scheduler
                    let icon_sched = self.icons.get_or_render(ctx, "scheduler", SVG_SCHEDULER, TEXT_DIM, 27);
                    if render_toolbar_button(ui, &icon_sched, "Scheduler", TEXT_DIM, button_w) {
                        self.open_scheduler();
                    }
                });

                // Subtle bottom rule instead of a full frame border.
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.max_rect().bottom(),
                    Stroke::new(1.0, BORDER_HAIRLINE),
                );
            });

        // -------------------------------------------------------------------
        // 3. Status Bar (Bottom)
        // -------------------------------------------------------------------
        egui::TopBottomPanel::bottom("statusbar")
            .exact_height(25.0)
            .frame(
                egui::Frame::none()
                    .fill(BG_DARK)
                    .inner_margin(Margin::symmetric(7.0, 0.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new(&self.status_bar).size(11.5).color(TEXT_DIM));
                });
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.max_rect().top(),
                    Stroke::new(1.0, BORDER_HAIRLINE),
                );
            });

        // -------------------------------------------------------------------
        // 4. Left Categories Sidebar
        // -------------------------------------------------------------------
        if self.categories_visible {
            egui::SidePanel::left("categories_sidebar")
                .exact_width(146.0)
                .frame(
                    egui::Frame::none()
                        .fill(BG_PANEL)
                        .inner_margin(Margin::symmetric(6.0, 6.0)),
                )
                .show(ctx, |ui| {
                    // Sidebar rows are 28 px; the default 3 px inter-row gap
                    // would push the bottom dock (Trash) into the Paused row.
                    ui.spacing_mut().item_spacing.y = 0.0;

                    // Header: Categories with [x] close button
                    ui.horizontal(|ui| {
                        ui.add_space(6.0);
                        ui.label(RichText::new("Categories").size(12.5).strong().color(TEXT_DIM));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let close = egui::Button::new(RichText::new("X").size(11.0).color(TEXT_DIM))
                                .fill(BG_BADGE)
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
                        let icon_color = if selected { ACCENT_BLUE } else { TEXT_DIM };
                        let icon = self.icons.get_or_render(ctx, key, svg, icon_color, 16);

                        if render_sidebar_item(ui, &icon, cat.label(), count, selected) {
                            self.selected_nav = cat;
                        }
                    }

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // Queue section header
                    ui.horizontal(|ui| {
                        ui.add_space(6.0);
                        ui.label(RichText::new("Queue").size(12.0).strong().color(TEXT_DIM));
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
                        let icon_color = if selected { ACCENT_BLUE } else { TEXT_DIM };
                        let icon = self.icons.get_or_render(ctx, key, svg, icon_color, 16);

                        if render_sidebar_item(ui, &icon, cat.label(), count, selected) {
                            self.selected_nav = cat;
                        }
                    }

                    // Trash is docked to the bottom of the sidebar.  Reserve
                    // the row, its divider, and a little breathing room, then
                    // push the cursor down to that point.
                    let reserved = 28.0 + 6.0 + 2.0;
                    let remaining = ui.available_height();
                    if remaining > reserved {
                        ui.add_space(remaining - reserved);
                    }
                    ui.separator();
                    ui.add_space(4.0);

                    let count = self.count_for_category(NavCategory::Trash);
                    let selected = self.selected_nav == NavCategory::Trash;
                    let icon = self.icons.get_or_render(ctx, "trash_cat", SVG_DELETE, if selected { ACCENT_BLUE } else { TEXT_DIM }, 16);
                    if render_sidebar_item(ui, &icon, "Trash", count, selected) {
                        self.selected_nav = NavCategory::Trash;
                    }

                    // Thin divider between sidebar and download list (the
                    // panel frame contributes 6 px of inner margin).
                    ui.painter().vline(
                        ui.max_rect().right() + 6.0,
                        ui.max_rect().y_range(),
                        Stroke::new(1.0, BORDER_HAIRLINE),
                    );
                });
        }

        // -------------------------------------------------------------------
        // 5. Central Table (Downloads list or Empty state)
        // -------------------------------------------------------------------
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BG_DARK))
            .show(ctx, |ui| {
                // Keep one shared column geometry for the header and rows so
                // the table lines up at every window width.
                let cols = ColumnWidths::new(ui.available_width() - 16.0);

                // Table header (single compact 27 px row)
                egui::Frame::none()
                    .fill(BG_HEADER)
                    .inner_margin(Margin::symmetric(8.0, 0.0))
                    .show(ui, |ui| {
                        ui.set_height(27.0);
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.horizontal_centered(|ui| {
                            // Column 0: File Name
                            if self.table_header_cell(ui, "File Name", 0, cols.name - 24.0) {
                                self.toggle_sort(0);
                            }
                            ui.add_space(4.0);

                            // Column 1: Search icon (the queue column below)
                            let search_icon = self.icons.get_or_render(ctx, "search", SVG_SEARCH, TEXT_DIM, 14);
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
                            icon_resp.on_hover_text("Search");
                            ui.add_space(4.0);

                            // Column 2: Size
                            if self.table_header_cell(ui, "Size", 2, cols.size) { self.toggle_sort(2); }
                            // Column 3: Status
                            if self.table_header_cell(ui, "Status", 3, cols.status) { self.toggle_sort(3); }
                            // Column 4: Time left
                            if self.table_header_cell(ui, "Time left", 4, cols.time_left) { self.toggle_sort(4); }
                            // Column 5: Transfer rate
                            if self.table_header_cell(ui, "Transfer rate", 5, cols.transfer) { self.toggle_sort(5); }
                            // Column 6: Last Try
                            if self.table_header_cell(ui, "Last Try", 6, cols.last_try) { self.toggle_sort(6); }
                            // Column 7: Description
                            if self.table_header_cell(ui, "Description", 7, cols.description) { self.toggle_sort(7); }
                        });

                        ui.painter().hline(
                            ui.max_rect().x_range(),
                            ui.max_rect().bottom(),
                            Stroke::new(1.0, BORDER_HAIRLINE),
                        );
                    });

                // Get visible downloads based on selected category & search filter
                let items: Vec<DownloadRec> = {
                    let st = self.manager.lock_ui();
                    let source = if self.selected_nav == NavCategory::Trash {
                        &st.trash
                    } else {
                        &st.downloads
                    };

                    source.iter().filter(|d| {
                        // Category filter
                        match self.selected_nav {
                            NavCategory::AllDownloads => true,
                            NavCategory::InProgress   => d.status == Status::Downloading,
                            NavCategory::Completed    => d.status == Status::Complete,
                            NavCategory::Queued       => d.status == Status::Queued,
                            NavCategory::Paused       => d.status == Status::Paused,
                            NavCategory::Documents    => is_file_type(&d.save_path, &["pdf","doc","docx","txt","rtf","xls","xlsx","ppt","pptx"]),
                            NavCategory::Music        => is_file_type(&d.save_path, &["mp3","wav","wma","m4a","aac","flac","ogg","aif"]),
                            NavCategory::Video        => is_file_type(&d.save_path, &["mp4","mkv","avi","mov","wmv","flv","webm","3gp","mpeg","mpg"]),
                            NavCategory::Programs     => is_file_type(&d.save_path, &["exe","msi","apk","iso","img","bin"]),
                            NavCategory::Compressed   => is_file_type(&d.save_path, &["zip","rar","7z","tar","gz","bz2","xz"]),
                            NavCategory::Scheduled    => d.status == Status::Queued && d.queue.is_some(),
                            NavCategory::Trash        => true,
                        }
                    }).filter(|d| {
                        // Search filter
                        self.search_query.is_empty()
                            || d.save_path.to_string_lossy().to_lowercase().contains(&self.search_query.to_lowercase())
                    }).cloned().collect()
                };

                if items.is_empty() {
                    // Empty state centred inside the download-list body, not
                    // the whole window (the sidebar must not affect it).
                    let block_height = 46.0;
                    let top_gap = ((ui.available_height() - block_height) * 0.5).max(0.0);
                    ui.add_space(top_gap);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("No downloads").size(18.0).color(TEXT_EMPTY_TITLE));
                        ui.add_space(5.0);
                        ui.label(RichText::new("Click Add URL on the toolbar to start downloading.").size(13.0).color(TEXT_EMPTY_SUB));
                    });
                } else {
                    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                        for (row_idx, d) in items.iter().enumerate() {
                            let is_selected = self.selected_download_id == Some(d.id);
                            let bg_color = if is_selected {
                                BG_ROW_SELECTED
                            } else if row_idx % 2 == 0 {
                                BG_ROW_EVEN
                            } else {
                                BG_ROW_ODD
                            };

                            let live_stats = self.manager.stats(d.id);
                            let total_bytes = if d.file_size > 0 { d.file_size } else { live_stats.as_ref().map(|s| s.total()).unwrap_or(0) };
                            let downloaded_bytes = live_stats.as_ref().map(|s| s.bytes()).unwrap_or_else(|| d.total_downloaded());
                            let speed_bps = 0u64; // can sample from manager

                            let row_resp = egui::Frame::none()
                                .fill(bg_color)
                                .inner_margin(Margin::symmetric(8.0, 5.0))
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.x = 0.0;
                                    ui.horizontal(|ui| {
                                        // 0. File Name
                                        let name = d.save_path.file_name().unwrap_or_default().to_string_lossy();
                                        ui.add_sized(Vec2::new(cols.name, 18.0), egui::Label::new(RichText::new(name.as_ref()).size(12.5).color(TEXT_MAIN)).truncate(true));

                                        // 1. Queue marker (search column in header)
                                        let q_str = d.queue.as_deref().unwrap_or("");
                                        ui.add_sized(Vec2::new(cols.queue, 18.0), egui::Label::new(RichText::new(q_str).size(11.0).color(TEXT_DIM)).truncate(true));

                                        // 2. Size
                                        let size_str = if total_bytes > 0 { human_bytes(total_bytes) } else { "Unknown".into() };
                                        ui.add_sized(Vec2::new(cols.size, 18.0), egui::Label::new(RichText::new(size_str).size(12.0).color(TEXT_DIM)));

                                        // 3. Status
                                        let status_color = match d.status {
                                            Status::Downloading => STATUS_DOWNLOADING,
                                            Status::Complete => STATUS_COMPLETE,
                                            Status::Paused => STATUS_PAUSED,
                                            Status::Failed => STATUS_FAILED,
                                            Status::Queued => TEXT_DIM,
                                        };
                                        ui.add_sized(Vec2::new(cols.status, 18.0), egui::Label::new(RichText::new(d.status.as_str()).size(12.0).color(status_color)));

                                        // 4. Time left
                                        let time_left = if d.status == Status::Downloading && speed_bps > 0 {
                                            let rem = total_bytes.saturating_sub(downloaded_bytes);
                                            format_eta(rem, speed_bps)
                                        } else {
                                            "--".into()
                                        };
                                        ui.add_sized(Vec2::new(cols.time_left, 18.0), egui::Label::new(RichText::new(time_left).size(12.0).color(TEXT_DIM)));

                                        // 5. Transfer rate
                                        let rate_str = if d.status == Status::Downloading && speed_bps > 0 {
                                            format!("{}/s", human_bytes(speed_bps))
                                        } else {
                                            "--".into()
                                        };
                                        ui.add_sized(Vec2::new(cols.transfer, 18.0), egui::Label::new(RichText::new(rate_str).size(12.0).color(TEXT_DIM)));

                                        // 6. Last Try
                                        let date_str = if d.last_try_date > 0 {
                                            format_timestamp(d.last_try_date)
                                        } else {
                                            "".into()
                                        };
                                        ui.add_sized(Vec2::new(cols.last_try, 18.0), egui::Label::new(RichText::new(date_str).size(12.0).color(TEXT_DIM)));

                                        // 7. Description
                                        ui.add_sized(Vec2::new(cols.description, 18.0), egui::Label::new(RichText::new(&d.description).size(12.0).color(TEXT_DIM)).truncate(true));
                                    });
                                }).response;

                            // Handle selection and double-click to open popout
                            if row_resp.interact(Sense::click()).clicked() {
                                self.selected_download_id = Some(d.id);
                            }
                            if row_resp.double_clicked() {
                                if !self.active_details_popouts.contains(&d.id) {
                                    self.active_details_popouts.push(d.id);
                                }
                            }

                            // Context menu
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

        // -------------------------------------------------------------------
        // 6. POPUPS & OS VIEWPORTS (Enforces AGENTS.md Hard Rules)
        // -------------------------------------------------------------------

        // A. Add URL Dialog (Separate OS Window)
        if self.add_dialog.open {
            self.show_add_url_viewport(ctx);
        }

        // B. Settings Dialog (options::SettingsDialog)
        if self.settings_dialog.open {
            self.settings_dialog.show(ctx);
            let outcome = self.settings_dialog.take_outcome();
            if let Some(new_settings) = outcome.settings {
                let mut st = self.manager.lock_ui();
                st.settings = new_settings;
                let _ = st.save();
                self.status_bar = "Settings saved.".into();
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
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.tray_icon = None;
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
                .with_inner_size([480.0, 240.0])
                .with_min_inner_size([420.0, 200.0])
                .with_always_on_top(),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    close_requested = true;
                }
                egui::CentralPanel::default().frame(egui::Frame::none().fill(BG_PANEL).inner_margin(Margin::same(14.0))).show(ctx, |ui| {
                    ui.label(RichText::new("Enter new address to download").size(14.0).strong().color(TEXT_MAIN));
                    ui.add_space(8.0);

                    // URL row
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Address:").color(TEXT_DIM));
                        ui.add_sized(Vec2::new(360.0, 24.0), egui::TextEdit::singleline(&mut self.add_dialog.url).hint_text("https://example.com/file.zip"));
                    });
                    ui.add_space(6.0);

                    // Save As row
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Save To:").color(TEXT_DIM));
                        ui.add_sized(Vec2::new(320.0, 24.0), egui::TextEdit::singleline(&mut self.add_dialog.save_as));
                        if ui.button("...").clicked() {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                self.add_dialog.save_as = folder.to_string_lossy().to_string();
                            }
                        }
                    });
                    ui.add_space(14.0);

                    // Action buttons
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("  OK (Download Now)  ").color(TEXT_MAIN)).clicked() {
                            if !self.add_dialog.url.trim().is_empty() {
                                self.create_download(true);
                                self.add_dialog.open = false;
                            }
                        }
                        if ui.button(RichText::new(" Download Later ").color(TEXT_DIM)).clicked() {
                            if !self.add_dialog.url.trim().is_empty() {
                                self.create_download(false);
                                self.add_dialog.open = false;
                            }
                        }
                        if ui.button(RichText::new(" Cancel ").color(TEXT_DIM)).clicked() {
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

    fn create_download(&mut self, start_now: bool) {
        let url_str = self.add_dialog.url.trim().to_string();
        let default_filename = url_str.split('?').next().unwrap_or("").split('/').last().unwrap_or("file.dat");
        let save_path = PathBuf::from(&self.add_dialog.save_as).join(default_filename);

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
            status: if start_now { Status::Downloading } else { Status::Queued },
            last_error: None,
            downloaded_secs: 0,
            date_added: chrono::Utc::now().timestamp(),
            last_try_date: chrono::Utc::now().timestamp(),
            queue: Some(self.add_dialog.queue.clone()),
            group: None,
            description: self.add_dialog.description.clone(),
            max_connections: Some(self.add_dialog.connections),
            mirrors: Vec::new(),
            temp_dir: st.settings.temp_path.clone(),
            restarted: false,
        };

        st.downloads.push(rec.clone());
        let _ = st.save();

        if start_now {
            let _ = self.manager.start(rec, None, None);
            self.status_bar = format!("Added and started download #{}", id);
        } else {
            self.status_bar = format!("Queued download #{}", id);
        }
    }

    fn show_scheduler_viewport(&mut self, ctx: &egui::Context) {
        let viewport_id = ViewportId::from_hash_of("dm-scheduler");
        let mut close_requested = false;

        ctx.show_viewport_immediate(
            viewport_id,
            ViewportBuilder::default()
                .with_title("Scheduler")
                .with_inner_size([460.0, 320.0])
                .with_min_inner_size([400.0, 260.0])
                .with_always_on_top(),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    close_requested = true;
                }
                egui::CentralPanel::default().frame(egui::Frame::none().fill(BG_PANEL).inner_margin(Margin::same(14.0))).show(ctx, |ui| {
                    ui.label(RichText::new("Schedule Queued Downloads").size(14.0).strong().color(TEXT_MAIN));
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Start time:").color(TEXT_DIM));
                        ui.text_edit_singleline(&mut self.scheduler_dialog.start_time);
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Stop time:").color(TEXT_DIM));
                        ui.text_edit_singleline(&mut self.scheduler_dialog.stop_time);
                    });
                    ui.add_space(10.0);
                    ui.checkbox(&mut self.scheduler_dialog.once, "Run once only");

                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if ui.button(" Save Schedule ").clicked() {
                            self.scheduler_dialog.open = false;
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
            let filename = d.save_path.file_name().unwrap_or_default().to_string_lossy().to_string();

            ctx.show_viewport_immediate(
                viewport_id,
                ViewportBuilder::default()
                    .with_title(format!("Download status - {}", filename))
                    .with_inner_size([500.0, 320.0])
                    .with_min_inner_size([440.0, 260.0])
                    .with_always_on_top(),
                |ctx, _class| {
                    if ctx.input(|i| i.viewport().close_requested()) {
                        close_requested = true;
                    }
                    egui::CentralPanel::default().frame(egui::Frame::none().fill(BG_PANEL).inner_margin(Margin::same(14.0))).show(ctx, |ui| {
                        ui.label(RichText::new(&filename).size(14.0).strong().color(TEXT_MAIN));
                        ui.add(egui::Label::new(RichText::new(&d.url).size(11.0).color(TEXT_DIM)).truncate(true));
                        ui.add_space(8.0);

                        let total = if d.file_size > 0 { d.file_size } else { stats.as_ref().map(|s| s.total()).unwrap_or(0) };
                        let downloaded = stats.as_ref().map(|s| s.bytes()).unwrap_or_else(|| d.total_downloaded());
                        let fraction = if total > 0 { (downloaded as f32 / total as f32).min(1.0) } else { 0.0 };

                        ui.add(egui::ProgressBar::new(fraction).show_percentage());
                        ui.add_space(6.0);

                        ui.label(RichText::new(format!("Downloaded: {} / {}", human_bytes(downloaded), if total > 0 { human_bytes(total) } else { "Unknown".into() })).color(TEXT_DIM));
                        ui.label(RichText::new(format!("Status: {}", d.status.as_str())).color(TEXT_MAIN));

                        // Segment bars
                        if let Some(ref st) = stats {
                            let segs = st.segments();
                            if !segs.is_empty() {
                                ui.add_space(6.0);
                                ui.label(RichText::new("Connections:").size(11.0).color(TEXT_DIM));
                                ui.horizontal_wrapped(|ui| {
                                    for (_i, seg) in segs.iter().enumerate() {
                                        let frac = if seg.len > 0 { (seg.downloaded as f32 / seg.len as f32).min(1.0) } else { 0.0 };
                                        ui.add_sized(Vec2::new(50.0, 10.0), egui::ProgressBar::new(frac));
                                    }
                                });
                            }
                        }

                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            if d.status == Status::Downloading {
                                if ui.button(" Pause ").clicked() {
                                    self.pause_selected();
                                }
                            } else {
                                if ui.button(" Resume ").clicked() {
                                    self.start_selected();
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
        let color = if is_sorted { TEXT_MAIN } else { TEXT_DIM };

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
        response.clicked()
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

fn render_toolbar_button(
    ui: &mut Ui,
    icon: &egui::TextureHandle,
    label: &str,
    text_color: Color32,
    width: f32,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 62.0), Sense::click());

    if response.hovered() {
        ui.painter().rect_filled(rect.shrink(2.0), Rounding::same(4.0), BG_ROW_HOVER);
    }

    // Icon sits above its label, with the whole cell clickable.
    let icon_rect = Rect::from_center_size(
        pos2(rect.center().x, rect.top() + 18.0),
        Vec2::splat(28.0),
    );
    ui.painter().image(
        icon.id(),
        icon_rect,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    ui.painter().text(
        pos2(rect.center().x, rect.bottom() - 6.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        text_color,
    );

    response.on_hover_text(label).clicked()
}

/// Hairline divider between toolbar command groups.
fn toolbar_separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(12.0, 62.0), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range().shrink(9.0),
        Stroke::new(1.0, BORDER_HAIRLINE),
    );
}

fn render_sidebar_item(ui: &mut Ui, icon: &egui::TextureHandle, label: &str, count: usize, selected: bool) -> bool {
    let bg_color = if selected { BG_PILL_SELECTED } else { Color32::TRANSPARENT };
    let text_color = if selected { TEXT_MAIN } else { TEXT_DIM };
    let badge_bg = if selected { BG_BADGE_SEL } else { BG_BADGE };
    let badge_text = if selected { ACCENT_BLUE } else { TEXT_BADGE };

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

    resp.interact(Sense::click()).clicked()
}

fn apply_app_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals.dark_mode = true;
    style.visuals.override_text_color = Some(TEXT_MAIN);
    style.visuals.panel_fill = BG_DARK;
    style.visuals.window_fill = BG_DARK;
    style.visuals.faint_bg_color = BG_PANEL;
    style.visuals.extreme_bg_color = Color32::from_rgb(22, 26, 33);
    style.visuals.widgets.inactive.bg_fill = BG_PANEL;
    style.visuals.widgets.hovered.bg_fill = BG_ROW_HOVER;
    style.visuals.widgets.active.bg_fill = BG_ROW_SELECTED;
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT_MAIN);
    style.visuals.selection.bg_fill = BG_ROW_SELECTED;
    style.visuals.window_stroke = Stroke::new(1.0, BORDER_HAIRLINE);
    style.spacing.item_spacing = Vec2::new(6.0, 4.0);
    ctx.set_style(style);

    // Install native Windows Segoe UI fonts
    options::install_fonts(ctx);
}

fn is_file_type(path: &std::path::Path, exts: &[&str]) -> bool {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_lowercase();
        exts.iter().any(|&e| e == ext_lower)
    } else {
        false
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
