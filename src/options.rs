//! VoltDown-style Settings dialog (12 categories) for the DM Ultra GUI.
//!
//! Structure mirrors the GPUI Settings window in `voltdown/crates/settings`
//! (sidebar navigation, fixed page heading, scrollable section cards,
//! label+description rows with the control on the right, two-column layout
//! at >= 920 px of content width). Settings apply immediately; controls
//! that have no engine support in this clone are rendered disabled with an
//! explicit note.

use crate::state::{Settings, Webhook};
use eframe::egui::{self, Color32, FontFamily, FontId, RichText, Rounding, Stroke, Ui};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub const WINDOW_W: f32 = 800.0;
pub const WINDOW_H: f32 = 580.0;
pub const MIN_W: f32 = 660.0;
pub const MIN_H: f32 = 440.0;
const CONTENT_PAD_LEFT: f32 = 28.0;
const CONTENT_PAD_RIGHT: f32 = 28.0;
const TWO_COLUMN_MIN_W: f32 = 920.0;
const CONTROL_W: f32 = 340.0;
const ROW_MIN_H: f32 = 48.0;

static SEMIBOLD_FONT: AtomicBool = AtomicBool::new(false);

#[derive(Serialize, Deserialize)]
struct ThemeFile {
    theme_mode: String,
    light_theme: String,
    dark_theme: String,
    accent: String,
}

pub fn ui_font(size: f32, semibold: bool) -> FontId {
    if semibold && SEMIBOLD_FONT.load(Ordering::Relaxed) {
        FontId::new(size, FontFamily::Name("sd-semibold".into()))
    } else {
        FontId::new(size, FontFamily::Proportional)
    }
}

/// Install the Windows UI fonts (Segoe UI / Semibold) so the settings
/// window matches the desktop app; falls back to egui's bundled fonts.
pub fn install_fonts(ctx: &egui::Context) {
    // Called from the per-frame style pass; only read and register the
    // Windows fonts once.
    static FONTS_INSTALLED: AtomicBool = AtomicBool::new(false);
    if FONTS_INSTALLED.swap(true, Ordering::Relaxed) {
        return;
    }

    let mut fonts = egui::FontDefinitions::default();
    let regular = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf").ok();
    let semibold = std::fs::read(r"C:\Windows\Fonts\seguisb.ttf").ok();
    if regular.is_none() && semibold.is_none() {
        return;
    }
    if let Some(bytes) = regular {
        fonts.font_data.insert("sd-regular".into(), egui::FontData::from_owned(bytes));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "sd-regular".into());
    }
    let mut semis: Vec<String> = Vec::new();
    if let Some(bytes) = semibold {
        fonts.font_data.insert("sd-semibold".into(), egui::FontData::from_owned(bytes));
        semis.push("sd-semibold".into());
        SEMIBOLD_FONT.store(true, Ordering::Relaxed);
    }
    if fonts.font_data.contains_key("sd-regular") {
        semis.push("sd-regular".into());
    }
    if !semis.is_empty() {
        fonts
            .families
            .insert(FontFamily::Name("sd-semibold".into()), semis);
    }
    ctx.set_fonts(fonts);
}

fn c(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

fn mix(from: Color32, to: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgb(f(from.r(), to.r()), f(from.g(), to.g()), f(from.b(), to.b()))
}

fn relative_luminance(color: Color32) -> f32 {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.039_28 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(color.r()) + 0.7152 * lin(color.g()) + 0.0722 * lin(color.b())
}

fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let (l1, l2) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

fn with_min_contrast(mut color: Color32, against: Color32, min: f32) -> Color32 {
    let darken = relative_luminance(against) > 0.179_13;
    for _ in 0..64 {
        if contrast_ratio(color, against) >= min {
            break;
        }
        let step = if darken { -0.01 } else { 0.01 };
        let adjust = |v: u8| {
            let mut x = v as f32 / 255.0 + step;
            if x < 0.0 {
                x = 0.0;
            }
            if x > 1.0 {
                x = 1.0;
            }
            (x * 255.0).round() as u8
        };
        let next = Color32::from_rgb(adjust(color.r()), adjust(color.g()), adjust(color.b()));
        if next == color {
            break;
        }
        color = next;
    }
    color
}

/// Resolved theme for the settings dialog and main window.
#[derive(Clone, Copy)]
pub struct ThemePalette {
    pub background: Color32,
    pub surface: Color32,
    pub surface2: Color32,
    pub chrome: Color32,
    pub hairline: Color32,
    pub row_hover: Color32,
    pub nav_hover: Color32,
    pub nav_selected: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub text_tertiary: Color32,
    pub border: Color32,
    pub accent: Color32,
    pub accent_bg: Color32,
    pub accent_text: Color32,
    pub destructive: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub dark: bool,
}

struct Base {
    background: u32,
    surface: u32,
    surface2: u32,
    text: u32,
    muted: u32,
    border: u32,
    error: u32,
    accent_alpha: u8,
    dark: bool,
}

fn base_for(theme: &str) -> Base {
    match theme {
        "warmLight" => Base {
            background: 0xFFFBEB,
            surface: 0xFFFFFF,
            surface2: 0xFEF3C7,
            text: 0x1C1917,
            muted: 0x78716C,
            border: 0xE7E5E4,
            error: 0xDC2626,
            accent_alpha: 26,
            dark: false,
        },
        "midnightBlue" => Base {
            background: 0x0F172A,
            surface: 0x1E293B,
            surface2: 0x334155,
            text: 0xF1F5F9,
            muted: 0x94A3B8,
            border: 0x334155,
            error: 0xEF4444,
            accent_alpha: 38,
            dark: true,
        },
        "nord" => Base {
            background: 0x2E3440,
            surface: 0x3B4252,
            surface2: 0x434C5E,
            text: 0xECEFF4,
            muted: 0xD8DEE9,
            border: 0x4C566A,
            error: 0xBF616A,
            accent_alpha: 38,
            dark: true,
        },
        "dmUltra" => Base {
            background: 0x0F172A,
            surface: 0x141D30,
            surface2: 0x1E283B,
            text: 0xD6D9E0,
            muted: 0x9299A5,
            border: 0x2A364A,
            error: 0xEF4444,
            accent_alpha: 38,
            dark: true,
        },
        "defaultDark" => Base {
            background: 0x1C1C1E,
            surface: 0x2C2C2E,
            surface2: 0x3A3A3C,
            text: 0xF5F5F7,
            muted: 0xA1A1A6,
            border: 0x48484A,
            error: 0xEF4444,
            accent_alpha: 46,
            dark: true,
        },
        _ => Base {
            background: 0xF8F9FA,
            surface: 0xFFFFFF,
            surface2: 0xF1F3F5,
            text: 0x09090B,
            muted: 0x71717A,
            border: 0xE4E4E7,
            error: 0xEF4444,
            accent_alpha: 26,
            dark: false,
        },
    }
}

pub fn accent_color(name: &str) -> Color32 {
    let trimmed = name.trim();
    if let Some(hex) = trimmed.strip_prefix('#') {
        if hex.len() == 6 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&hex[0..2], 16),
                u8::from_str_radix(&hex[2..4], 16),
                u8::from_str_radix(&hex[4..6], 16),
            ) {
                return c(((r as u32) << 16) | ((g as u32) << 8) | b as u32);
            }
        }
    }
    match trimmed {
        "green" => c(0x22C55E),
        "violet" => c(0x8B5CF6),
        "rose" => c(0xF43F5E),
        "custom" => c(0x6366F1),
        _ => c(0x3B82F6),
    }
}

/// Apply the complete theme to `ctx`, overwriting every style field egui
/// widgets read. This includes all five widget states (noninteractive,
/// inactive, hovered, active, open) and their `weak_bg_fill`, which egui
/// uses to paint buttons and menu items. Overwriting everything every frame
/// guarantees no default or previously-applied light-mode value (e.g. a white
/// button background) can leak through in dark mode.
pub fn apply_global_theme_style(ctx: &egui::Context, pal: &ThemePalette) {
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;

    v.dark_mode = pal.dark;
    v.override_text_color = Some(pal.text);
    v.panel_fill = pal.background;
    v.window_fill = pal.background;
    v.faint_bg_color = pal.surface;
    v.extreme_bg_color = pal.surface2;
    v.hyperlink_color = pal.accent;
    v.warn_fg_color = pal.warning;
    v.error_fg_color = pal.destructive;
    v.window_stroke = egui::Stroke::new(1.0, pal.border);
    v.popup_shadow = egui::epaint::Shadow::NONE;

    let states = [
        (&mut v.widgets.noninteractive, (pal.background, pal.border, pal.text)),
        (&mut v.widgets.inactive, (pal.surface, pal.border, pal.text)),
        (&mut v.widgets.hovered, (pal.nav_hover, pal.border, pal.text)),
        (&mut v.widgets.active, (pal.nav_selected, pal.accent, pal.text)),
        (&mut v.widgets.open, (pal.nav_hover, pal.border, pal.text)),
    ];
    for (state, (bg, stroke, fg)) in states {
        state.bg_fill = bg;
        state.weak_bg_fill = bg;
        state.bg_stroke = egui::Stroke::new(1.0, stroke);
        state.fg_stroke = egui::Stroke::new(1.0, fg);
    }

    v.selection.bg_fill = pal.nav_selected;
    v.selection.stroke = egui::Stroke::new(1.0, pal.text);

    style.interaction.selectable_labels = false;
    ctx.set_style(style);
}

fn custom_accent_rgb(name: &str) -> [u8; 3] {
    let color = accent_color(name);
    [color.r(), color.g(), color.b()]
}

pub fn accent_name(color: Color32) -> &'static str {
    for name in ["blue", "green", "violet", "rose"] {
        if accent_color(name) == color {
            return name;
        }
    }
    "custom"
}

/// Runtime view of the appearance settings (mode + theme slots + accent).
#[derive(Clone)]
pub struct Appearance {
    pub mode: String,
    pub light_theme: String,
    pub dark_theme: String,
    pub accent: String,
    pub ui_scale: u32,
    pub font_size: f32,
    pub system_dark: bool,
}

impl Appearance {
    pub fn from_settings(s: &Settings, system_dark: bool) -> Self {
        Appearance {
            mode: s.theme_mode.clone(),
            light_theme: s.light_theme.clone(),
            dark_theme: s.dark_theme.clone(),
            accent: s.accent.clone(),
            ui_scale: s.ui_scale.clamp(80, 150),
            font_size: s.font_size.clamp(9.0, 24.0),
            system_dark,
        }
    }

    pub fn is_dark(&self) -> bool {
        match self.mode.as_str() {
            "dark" => true,
            "light" => false,
            _ => self.system_dark,
        }
    }

    fn active_theme(&self) -> &str {
        if self.is_dark() {
            &self.dark_theme
        } else {
            &self.light_theme
        }
    }

    pub fn palette(&self) -> ThemePalette {
        let base = base_for(self.active_theme());
        let background = c(base.background);
        let surface = c(base.surface);
        let surface2 = c(base.surface2);
        let border = c(base.border);
        let muted = c(base.muted);
        let text = c(base.text);
        let accent = accent_color(&self.accent);
        let (hairline, row_hover, chrome, nav_selected) = if base.dark {
            (
                mix(border, surface, 0.35),
                mix(surface2, Color32::WHITE, 0.16),
                mix(background, surface, 0.35),
                mix(surface2, Color32::WHITE, 0.22),
            )
        } else {
            (
                mix(border, surface, 0.4),
                mix(surface2, surface, 0.4),
                mix(background, surface2, 0.35),
                mix(surface2, border, 0.65),
            )
        };
        let nav_hover = mix(chrome, nav_selected, 0.55);
        let text_tertiary = mix(muted, surface, 0.38);
        let accent_bg = Color32::from_rgba_unmultiplied(
            accent.r(),
            accent.g(),
            accent.b(),
            base.accent_alpha,
        );
        let accent_text = with_min_contrast(accent, nav_selected, 3.0);
        ThemePalette {
            background,
            surface,
            surface2,
            chrome,
            hairline,
            row_hover,
            nav_hover,
            nav_selected,
            text,
            muted,
            text_tertiary,
            border,
            accent,
            accent_bg,
            accent_text,
            destructive: c(base.error),
            success: if base.dark { c(0x30D158) } else { c(0x1F9D55) },
            warning: if base.dark { c(0xFFB340) } else { c(0xC77C02) },
            dark: base.dark,
        }
    }
}

/// Minimal HKCU registry reader. Replaces `reg query` child processes,
/// which flashed a console window on every call (visible as a blank
/// popup on launch / while the Options proxy page rendered) and cost
/// tens of milliseconds per spawn.
#[cfg(windows)]
mod winreg_read {
    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            hkey: isize,
            subkey: *const u16,
            options: u32,
            access: u32,
            result: *mut isize,
        ) -> i32;
        fn RegQueryValueExW(
            hkey: isize,
            value: *const u16,
            reserved: *const u32,
            kind: *mut u32,
            data: *mut u8,
            size: *mut u32,
        ) -> i32;
        fn RegCloseKey(hkey: isize) -> i32;
    }
    const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
    const KEY_QUERY_VALUE: u32 = 0x0001;
    const REG_SZ: u32 = 1;
    const REG_DWORD: u32 = 4;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn open(subkey: &str) -> Option<isize> {
        let mut h = 0isize;
        let rc = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(subkey).as_ptr(),
                0,
                KEY_QUERY_VALUE,
                &mut h,
            )
        };
        (rc == 0).then_some(h)
    }

    pub fn query_dword(subkey: &str, value: &str) -> Option<u32> {
        unsafe {
            let h = open(subkey)?;
            let mut kind = 0u32;
            let mut data = 0u32;
            let mut size = 4u32;
            let rc = RegQueryValueExW(
                h,
                wide(value).as_ptr(),
                std::ptr::null(),
                &mut kind,
                &mut data as *mut u32 as *mut u8,
                &mut size,
            );
            RegCloseKey(h);
            (rc == 0 && kind == REG_DWORD).then_some(data)
        }
    }

    pub fn query_sz(subkey: &str, value: &str) -> Option<String> {
        unsafe {
            let h = open(subkey)?;
            let name = wide(value);
            let mut kind = 0u32;
            let mut size = 0u32;
            if RegQueryValueExW(
                h,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                std::ptr::null_mut(),
                &mut size,
            ) != 0
                || kind != REG_SZ
            {
                RegCloseKey(h);
                return None;
            }
            let mut buf = vec![0u16; (size as usize) / 2 + 1];
            let rc = RegQueryValueExW(
                h,
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut u8,
                &mut size,
            );
            RegCloseKey(h);
            if rc != 0 {
                return None;
            }
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..len]))
        }
    }
}

/// Best-effort read of the Windows "app mode" preference.
pub fn system_prefers_dark() -> bool {
    #[cfg(windows)]
    {
        winreg_read::query_dword(
            r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "AppsUseLightTheme",
        )
        .map(|v| v == 0)
        .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Read the WinINET system proxy (proxy server from the registry).
pub fn system_proxy_url() -> Option<String> {
    #[cfg(windows)]
    {
        let value = winreg_read::query_sz(
            r"Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            "ProxyServer",
        )?
        .trim()
        .to_string();
        if value.is_empty() {
            return None;
        }
        if let Some(eq) = value.find('=') {
            let rest = &value[eq + 1..];
            if let Some(eq2) = rest.find('=') {
                return Some(rest[..eq2].trim().to_string());
            }
            let key = &value[..eq];
            if key.eq_ignore_ascii_case("http") || key.eq_ignore_ascii_case("https") {
                return Some(rest.trim().to_string());
            }
            return None;
        }
        return Some(value);
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PageId {
    General,
    Appearance,
    Download,
    Proxy,
    Notify,
    Doctor,
    About,
}

impl PageId {
    pub const ALL: [PageId; 7] = [
        PageId::General,
        PageId::Appearance,
        PageId::Download,
        PageId::Proxy,
        PageId::Notify,
        PageId::Doctor,
        PageId::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PageId::General => "General",
            PageId::Appearance => "Appearance",
            PageId::Download => "Download",
            PageId::Proxy => "Proxy",
            PageId::Notify => "Notifications",
            PageId::Doctor => "Doctor",
            PageId::About => "About",
        }
    }

    pub fn desc(self) -> &'static str {
        match self {
            PageId::General => "Basic behavior settings",
            PageId::Appearance => "Theme & Colors",
            PageId::Download => "Download engine settings",
            PageId::Proxy => "Network proxy settings",
            PageId::Notify => "System notifications and download windows",
            PageId::Doctor => "Environment self-check & one-click repair",
            PageId::About => "Version info & Updates",
        }
    }

    fn rows(self) -> &'static [&'static str] {
        match self {
            PageId::General => &[
                "Launch at Startup",
                "Remember window position",
                "Clipboard Watcher",
                "Probe extensionless links online",
                "Custom watched extensions",
                "Browser integration",
                "Auto-download links",
                "Keep Awake While Downloading",
                "Show Status",
                "Show Queues",
                "Show Category",
                "Notifications",
            ],
            PageId::Appearance => &[
                "Language",
                "Theme Mode",
                "Theme",
                "Theme Color",
                "File Icons",
                "Interface Scale",
                "Font Size",
            ],
            PageId::Download => &[
                "Default Save Directory",
                "Remember Last Save Location",
                "Use Server File Time",
                "When file exists",
                "Periodically check completed files while idle",
                "Save each category to its own folder",
                "Default Queue",
                "Thread Count",
                "Download Limit",
                "Resume unfinished downloads on startup",
            ],
            PageId::Proxy => &[
                "Proxy Mode",
                "Proxy Type",
                "Server Address",
                "Port",
                "Bypass List",
                "Test Connection",
                "Username",
                "Password",
            ],
            PageId::Notify => &[
                "Completion Notifications",
                "Show download progress window",
                "Show download complete window",
            ],
            PageId::Doctor => &["Run diagnostics", "Copy report", "Open folder"],
            PageId::About => &[
                "Current Version",
            ],
        }
    }
}

/// Column selection used by pages to render one or two columns.
#[derive(Clone, Copy, PartialEq)]
pub enum Col {
    Left,
    Right,
    All,
}

impl Col {
    fn left(self) -> bool {
        matches!(self, Col::Left | Col::All)
    }
    fn right(self) -> bool {
        matches!(self, Col::Right | Col::All)
    }
}

type ProxyTest = Arc<Mutex<Option<Result<String, String>>>>;

pub struct SettingsDialog {
    pub open: bool,
    pub page: PageId,
    pub tab: usize,
    pub search: String,
    pub draft: Settings,
    /// Last observed outer position of the Settings OS window.
    pub last_pos: Option<[f32; 2]>,
    /// Whether the remembered position was applied for this open.
    pos_applied: bool,
    /// Snapshot of settings when the dialog opened (Cancel reverts to this).
    orig: Settings,
    dirty: bool,
    pub clean_temp: bool,
    pub open_data_dir: bool,
    pub open_log_dir: bool,
    pub system_dark: bool,
    pub webhooks: Vec<Webhook>,
    hook_bufs: Vec<String>,
    hooks_dirty: bool,
    notice: Option<(String, Instant)>,
    num: std::collections::HashMap<&'static str, String>,
    save_dir: String,
    temp_dir: String,
    file_types: String,
    ua: String,
    ext: String,
    proxy_host: String,
    proxy_port: String,
    proxy_user: String,
    proxy_pass: String,
    font_buf: String,
    proxy_testing: bool,
    proxy_test_rx: Option<ProxyTest>,
    doctor: Vec<(bool, String)>,
    doctor_ran: bool,
}

impl Default for SettingsDialog {
    fn default() -> Self {
        SettingsDialog {
            open: false,
            page: PageId::General,
            tab: 0,
            search: String::new(),
            draft: Settings::default(),
            last_pos: None,
            pos_applied: false,
            orig: Settings::default(),
            dirty: false,
            clean_temp: false,
            open_data_dir: false,
            open_log_dir: false,
            system_dark: false,
            webhooks: Vec::new(),
            hook_bufs: Vec::new(),
            hooks_dirty: false,
            notice: None,
            num: std::collections::HashMap::new(),
            save_dir: String::new(),
            temp_dir: String::new(),
            file_types: String::new(),
            ua: String::new(),
            ext: String::new(),
            proxy_host: String::new(),
            proxy_port: String::new(),
            proxy_user: String::new(),
            proxy_pass: String::new(),
            font_buf: String::new(),
            proxy_testing: false,
            proxy_test_rx: None,
            doctor: Vec::new(),
            doctor_ran: false,
        }
    }
}

pub struct Outcome {
    pub settings: Option<Settings>,
    pub clean_temp: bool,
    pub open_data_dir: bool,
    pub open_log_dir: bool,
    pub webhooks: Option<Vec<Webhook>>,
}

impl SettingsDialog {
    pub fn open_with(
        &mut self,
        settings: &Settings,
        system_dark: bool,
        webhooks: Vec<Webhook>,
    ) {
        self.orig = settings.clone();
        self.draft = settings.clone();
        self.last_pos = if settings.remember_window_pos {
            settings
                .window_positions
                .get("settings")
                .copied()
                .or(self.last_pos)
        } else {
            None
        };
        self.pos_applied = false;
        self.system_dark = system_dark;
        self.hook_bufs = webhooks
            .iter()
            .map(|h| h.events.join(", "))
            .collect();
        self.webhooks = webhooks;
        self.hooks_dirty = false;
        self.dirty = false;
        self.clean_temp = false;
        self.open_data_dir = false;
        self.search.clear();
        self.tab = 0;
        self.notice = None;
        self.sync_buffers();
        self.open = true;
    }

    pub fn take_outcome(&mut self) -> Outcome {
        let settings = if self.dirty {
            self.dirty = false;
            Some(self.draft.clone())
        } else {
            None
        };
        let clean_temp = std::mem::take(&mut self.clean_temp);
        let open_data_dir = std::mem::take(&mut self.open_data_dir);
        let open_log_dir = std::mem::take(&mut self.open_log_dir);
        let webhooks = if self.hooks_dirty {
            self.hooks_dirty = false;
            Some(self.webhooks.clone())
        } else {
            None
        };
        Outcome { settings, clean_temp, open_data_dir, open_log_dir, webhooks }
    }

    /// Smoke-test hook: simulate an edited dialog that has been closed.
    pub fn force_close_dirty(&mut self) {
        self.dirty = true;
        self.open = false;
    }

    fn sync_buffers(&mut self) {
        self.num.clear();
        self.num.insert("threads", self.draft.max_connections.to_string());
        self.num.insert("limit", (self.draft.speed_limit_bps / 1024).max(1).to_string());
        self.num.insert("quota_mb", self.draft.quota.mb.to_string());
        self.num.insert("quota_start", self.draft.quota.start_hour.to_string());
        self.num.insert("quota_end", self.draft.quota.end_hour.to_string());
        self.num.insert("scale", self.draft.ui_scale.to_string());
        self.save_dir = self.draft.save_path.display().to_string();
        self.temp_dir = self.draft.temp_path.display().to_string();
        self.file_types = self.draft.file_types.clone();
        self.ua = self.draft.user_agent.clone();
        self.ext = self.draft.clipboard_extensions.clone();
        self.proxy_user = self.draft.proxies.user.clone().unwrap_or_default();
        self.proxy_pass = self.draft.proxies.password.clone().unwrap_or_default();
        let raw = self
            .draft
            .proxies
            .https
            .clone()
            .or_else(|| self.draft.proxies.http.clone())
            .unwrap_or_default();
        let (kind, host, port) = parse_proxy_url(&raw);
        if !raw.is_empty() {
            self.draft.proxy_type = kind;
        }
        self.proxy_host = host;
        self.proxy_port = port;
        self.font_buf = format_size(self.draft.font_size);
    }

    fn palette(&self) -> ThemePalette {
        Appearance::from_settings(&self.draft, self.system_dark).palette()
    }

    fn matches(&self, label: &str, desc: &str) -> bool {
        let q = self.search.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        label.to_lowercase().contains(&q) || desc.to_lowercase().contains(&q)
    }

    fn section_hit(&self, title: &str, subtitle: Option<&str>) -> bool {
        let q = self.search.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        title.to_lowercase().contains(&q)
            || subtitle.is_some_and(|s| s.to_lowercase().contains(&q))
    }

    fn page_visible(&self, page: PageId) -> bool {
        let q = self.search.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        page.label().to_lowercase().contains(&q)
            || page.desc().to_lowercase().contains(&q)
            || page
                .rows()
                .iter()
                .any(|row| row.to_lowercase().contains(&q))
    }

    fn notice_text(&self) -> Option<&str> {
        self.notice
            .as_ref()
            .filter(|(_, at)| at.elapsed().as_secs() < 4)
            .map(|(text, _)| text.as_str())
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), Instant::now()));
    }

    fn set_dirty(&mut self) {
        self.dirty = true;
    }

    fn write_proxy(&mut self) {
        let host = self.proxy_host.trim().to_string();
        let port = self.proxy_port.trim().to_string();
        let value = if host.is_empty() {
            None
        } else if port.is_empty() {
            Some(format!("{}://{}", self.draft.proxy_type, host))
        } else {
            Some(format!("{}://{}:{}", self.draft.proxy_type, host, port))
        };
        self.draft.proxies.http = value.clone();
        self.draft.proxies.https = value.clone();
        self.draft.proxies.ftp = value;
        self.set_dirty();
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        let pal = self.palette();
        let mut builder = egui::ViewportBuilder::default()
            .with_title("Options")
            .with_inner_size([WINDOW_W, WINDOW_H])
            .with_min_inner_size([MIN_W, MIN_H]);
        if !self.pos_applied {
            if let Some(pos) = self.last_pos {
                builder = builder.with_position(pos);
            }
            self.pos_applied = true;
        }
        let mut keep_open = true;
        let mut last_pos = self.last_pos;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("dm-settings"),
            builder,
            |vctx, class| {
                if vctx.input(|i| i.viewport().close_requested()) {
                    keep_open = false;
                }
                match class {
                    egui::ViewportClass::Embedded => {
                        egui::Window::new("Settings")
                            .default_size([WINDOW_W, WINDOW_H])
                            .min_size([MIN_W, MIN_H])
                            .resizable(true)
                            .collapsible(false)
                            .show(vctx, |ui| self.body(ui, pal));
                    }
                    _ => {
                        egui::CentralPanel::default()
                            .frame(egui::Frame::none().fill(pal.background))
                            .show(vctx, |ui| self.body(ui, pal));
                    }
                }
                last_pos = vctx
                    .input(|i| i.viewport().outer_rect)
                    .map(|r| [r.min.x, r.min.y]);
            },
        );
        self.last_pos = last_pos;
        if !keep_open {
            self.open = false;
        }
    }

    fn body(&mut self, ui: &mut Ui, pal: ThemePalette) {
        self.check_proxy_test();
        let ctx = ui.ctx().clone();
        // One shared, complete theme override (also used by the main window).
        apply_global_theme_style(&ctx, &pal);

        ui.style_mut().spacing.button_padding = egui::vec2(10.0, 5.0);
        ui.style_mut().spacing.item_spacing = egui::vec2(8.0, 6.0);

        // IDM-style property sheet: tab strip across the top, page
        // content in the middle, OK/Cancel/Apply bottom-right.
        egui::TopBottomPanel::top("settings-tabs")
            .frame(
                egui::Frame::none()
                    .fill(pal.background)
                    .inner_margin(egui::Margin {
                        left: 14.0,
                        right: 14.0,
                        top: 10.0,
                        bottom: 8.0,
                    }),
            )
            .show_inside(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let visible: Vec<PageId> = PageId::ALL
                        .into_iter()
                        .filter(|page| self.page_visible(*page))
                        .collect();
                    if !visible.is_empty() && !visible.contains(&self.page) {
                        self.page = visible[0];
                    }
                    for page in &visible {
                        let sel = *page == self.page;
                        let label = RichText::new(page.label())
                            .font(ui_font(12.5, sel))
                            .color(if sel { pal.text } else { pal.muted });
                        if ui.selectable_label(sel, label).clicked() {
                            self.page = *page;
                            self.tab = 0;
                        }
                        ui.add_space(4.0);
                    }
                });
                let rect = ui.max_rect();
                ui.painter()
                    .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, pal.hairline));
            });

        egui::TopBottomPanel::bottom("settings-buttons")
            .frame(
                egui::Frame::none()
                    .fill(pal.background)
                    .inner_margin(egui::Margin {
                        left: 14.0,
                        right: 14.0,
                        top: 8.0,
                        bottom: 10.0,
                    }),
            )
            .show_inside(ui, |ui| {
                ui.set_width(ui.available_width());
                let rect = ui.max_rect();
                ui.painter()
                    .hline(rect.x_range(), rect.top(), Stroke::new(1.0, pal.hairline));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("OK").clicked() {
                        self.set_dirty();
                        self.open = false;
                    }
                    if ui.button("Cancel").clicked() {
                        self.draft = self.orig.clone();
                        self.sync_buffers();
                        self.set_dirty(); // persist the revert
                        self.open = false;
                    }
                    if ui.button("Apply").clicked() {
                        self.set_dirty();
                        self.toast("Settings applied.");
                    }
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(pal.surface)
                    .inner_margin(egui::Margin::same(0.0)),
            )
            .show_inside(ui, |ui| {
                self.content(ui, pal);
            });
    }


    fn content(&mut self, ui: &mut Ui, pal: ThemePalette) {
        let visible: Vec<PageId> = PageId::ALL
            .into_iter()
            .filter(|page| self.page_visible(*page))
            .collect();
        if visible.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("No matching settings")
                        .font(ui_font(13.0, false))
                        .color(pal.muted),
                );
            });
            return;
        }
        if !visible.contains(&self.page) {
            self.page = visible[0];
            self.tab = 0;
        }
        let page = self.page;
        let notice = self.notice_text().map(str::to_string);
        // IDM-style: the tab strip names the page; no in-content heading.
        // Only transient notices (proxy test results etc.) render here.
        if let Some(text) = &notice {
            egui::Frame::none()
                .inner_margin(egui::Margin {
                    left: CONTENT_PAD_LEFT,
                    right: CONTENT_PAD_RIGHT,
                    top: 10.0,
                    bottom: 2.0,
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new(text)
                            .font(ui_font(11.5, false))
                            .color(pal.accent_text),
                    );
                });
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(16.0);
                let wide = ui.available_width() >= TWO_COLUMN_MIN_W;
                match page {
                    PageId::General => split_cols(ui, wide, |ui, col| self.page_general(ui, pal, col)),
                    PageId::Appearance => {
                        split_cols(ui, wide, |ui, col| self.page_appearance(ui, pal, col))
                    }
                    PageId::Download => {
                        split_cols(ui, wide, |ui, col| self.page_download(ui, pal, col))
                    }
                    PageId::Proxy => {
                        split_cols(ui, wide, |ui, col| self.page_proxy(ui, pal, col))
                    }
                    PageId::Notify => {
                        split_cols(ui, wide, |ui, col| self.page_notify(ui, pal, col))
                    }
                    PageId::Doctor => self.page_doctor(ui, pal),
                    PageId::About => {
                        split_cols(ui, wide, |ui, col| self.page_about(ui, pal, col))
                    }
                }
                ui.add_space(24.0);
            });
    }

    fn check_proxy_test(&mut self) {
        let Some(slot) = self.proxy_test_rx.clone() else {
            return;
        };
        let result = slot.lock().ok().and_then(|mut g| g.take());
        if let Some(result) = result {
            self.proxy_testing = false;
            self.proxy_test_rx = None;
            match result {
                Ok(text) => self.toast(text),
                Err(err) => self.toast(format!("Test failed: {err}")),
            }
        }
    }

    fn run_proxy_test(&mut self) {
        let host = self.proxy_host.trim().to_string();
        let port = self.proxy_port.trim().to_string();
        if host.is_empty() || port.is_empty() {
            self.toast("Enter a server address and port first.");
            return;
        }
        let addr = format!("{host}:{port}");
        let slot: ProxyTest = Arc::new(Mutex::new(None));
        self.proxy_test_rx = Some(slot.clone());
        self.proxy_testing = true;
        std::thread::spawn(move || {
            let result = match addr.parse::<std::net::SocketAddr>() {
                Ok(sa) => {
                    match std::net::TcpStream::connect_timeout(&sa, std::time::Duration::from_secs(5))
                    {
                        Ok(_) => Ok(format!("Connected to {addr}.")),
                        Err(e) => Err(e.to_string()),
                    }
                }
                Err(_) => {
                    use std::net::ToSocketAddrs;
                    match addr.to_socket_addrs().ok().and_then(|mut it| it.next()) {
                        Some(sa) => match std::net::TcpStream::connect_timeout(
                            &sa,
                            std::time::Duration::from_secs(5),
                        ) {
                            Ok(_) => Ok(format!("Connected to {addr}.")),
                            Err(e) => Err(e.to_string()),
                        },
                        None => Err("could not resolve host".into()),
                    }
                }
            };
            if let Ok(mut g) = slot.lock() {
                *g = Some(result);
            }
        });
    }

    fn run_doctor(&mut self) {
        let mut report = Vec::new();
        let state_dir = crate::state::state_dir();
        report.push(fs_check("State folder", &state_dir, true));
        report.push(fs_check("Temp folder", &self.draft.temp_path, true));
        report.push(fs_check("Save folder", &self.draft.save_path, false));
        let proxy_ok = self.draft.proxy_mode != "manual"
            || (!self.proxy_host.trim().is_empty() && self.proxy_port.trim().parse::<u16>().is_ok());
        report.push((
            proxy_ok,
            if proxy_ok {
                "Proxy configuration".to_string()
            } else {
                "Proxy configuration incomplete".to_string()
            },
        ));
        let state_ok = crate::state::state_path().exists();
        report.push((
            state_ok,
            if state_ok { "state.json present" } else { "state.json missing" }.to_string(),
        ));
        report.push((
            true,
            format!("Version {}", env!("CARGO_PKG_VERSION")),
        ));
        self.doctor = report;
        self.doctor_ran = true;
        self.toast("Diagnostics finished.");
    }
}

fn fs_check(name: &str, path: &std::path::Path, create: bool) -> (bool, String) {
    if create {
        let _ = std::fs::create_dir_all(path);
    }
    if !path.exists() {
        return (false, format!("{name} missing: {}", path.display()));
    }
    let probe = path.join(".dmultra-probe");
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            (true, format!("{name} writable: {}", path.display()))
        }
        Err(e) => (false, format!("{name} not writable: {e}")),
    }
}

fn parse_proxy_url(raw: &str) -> (String, String, String) {
    if raw.trim().is_empty() {
        return ("http".into(), String::new(), String::new());
    }
    match url::Url::parse(raw) {
        Ok(u) => (
            u.scheme().to_string(),
            u.host_str().unwrap_or_default().to_string(),
            u.port().map(|p| p.to_string()).unwrap_or_default(),
        ),
        Err(_) => ("http".into(), raw.trim().to_string(), String::new()),
    }
}

fn format_size(v: f32) -> String {
    if (v.fract()).abs() < 0.001 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

fn split_cols(ui: &mut Ui, wide: bool, mut page: impl FnMut(&mut Ui, Col)) {
    if wide {
        ui.spacing_mut().item_spacing.x = 16.0;
        ui.columns(2, |cols| {
            let (left, right) = cols.split_at_mut(1);
            (page)(&mut left[0], Col::Left);
            (page)(&mut right[0], Col::Right);
        });
    } else {
        (page)(ui, Col::All);
    }
}

macro_rules! srow {
    ($self:ident, $ui:expr, $pal:expr, $first:ident, $drawn:ident, $hit:expr, $label:expr, $desc:expr, $body:expr) => {{
        if $hit || $self.matches($label, $desc) {
            row_frame($ui, $pal, &mut $first, CONTROL_W, $label, $desc, $body);
            *$drawn = true;
        }
    }};
}

macro_rules! srowv {
    ($self:ident, $ui:expr, $pal:expr, $first:ident, $drawn:ident, $hit:expr, $label:expr, $desc:expr, $body:expr) => {{
        if $hit || $self.matches($label, $desc) {
            row_frame_vertical($ui, $pal, &mut $first, $label, $desc, $body);
            *$drawn = true;
        }
    }};
}

fn section_card<F>(ui: &mut Ui, pal: ThemePalette, heading: Option<(&str, Option<&str>)>, body: F)
where
    F: FnOnce(&mut Ui, &mut bool),
{
    if let Some((title, subtitle)) = heading {
        ui.label(
            RichText::new(title)
                .font(ui_font(11.0, true))
                .color(pal.muted),
        );
        if let Some(subtitle) = subtitle {
            ui.add_space(2.0);
            ui.label(
                RichText::new(subtitle)
                    .font(ui_font(11.0, false))
                    .color(pal.text_tertiary),
            );
        }
        ui.add_space(8.0);
    }
    let idx = ui.painter().add(egui::Shape::Noop);
    let full_w = ui.available_width();
    let mut drawn = false;
    let inner = ui.allocate_ui_with_layout(
        egui::vec2(full_w, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(full_w);
            body(ui, &mut drawn);
        },
    );
    if drawn {
        let rect = inner.response.rect;
        ui.painter().set(
            idx,
            egui::Shape::Rect(egui::epaint::RectShape::new(
                rect,
                Rounding::same(8.0),
                pal.surface,
                Stroke::new(1.0, pal.hairline),
            )),
        );
    }
    ui.add_space(16.0);
}

fn row_frame<F>(ui: &mut Ui, pal: ThemePalette, first: &mut bool, ctl_w: f32, label: &str, desc: &str, body: F)
where
    F: FnOnce(&mut Ui),
{
    // Row stops 20px short of the content's right edge so the control
    // column never touches the window border.
    let full_w = ui.available_width() - 20.0;
    let divider = if *first {
        None
    } else {
        Some(ui.painter().add(egui::Shape::Noop))
    };
    let hover_idx = ui.painter().add(egui::Shape::Noop);
    let mut height = ROW_MIN_H;
    if !desc.is_empty() {
        let label_w = (full_w - 32.0 - ctl_w - 32.0).max(140.0);
        let galley = ui.painter().layout(
            desc.to_string(),
            ui_font(11.0, false),
            pal.muted,
            label_w,
        );
        height = height.max(20.0 + galley.size().y + 16.0);
    }
    let inner = ui.allocate_ui_with_layout(
        egui::vec2(full_w, height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(full_w);
            ui.add_space(10.0);
            ui.horizontal_top(|ui| {
                ui.add_space(16.0);
                let label_w = (full_w - 32.0 - ctl_w - 32.0).max(140.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(label_w, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(label_w);
                        ui.label(
                            RichText::new(label)
                                .font(ui_font(13.0, false))
                                .color(pal.text),
                        );
                        if !desc.is_empty() {
                            ui.add_space(2.0);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(desc)
                                        .font(ui_font(11.0, false))
                                        .color(pal.muted),
                                )
                                .wrap(true),
                            );
                        }
                    },
                );
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    body(ui);
                });
            });
        },
    );
    let hrect = egui::Rect::from_min_max(
        egui::pos2(inner.response.rect.left() + 1.0, inner.response.rect.top() + 1.0),
        egui::pos2(inner.response.rect.right() - 1.0, inner.response.rect.bottom() - 1.0),
    );
    if ui
        .interact(hrect, ui.id().with(("settings-row", label)), egui::Sense::hover())
        .hovered()
    {
        ui.painter().set(
            hover_idx,
            egui::Shape::Rect(egui::epaint::RectShape::new(
                hrect,
                Rounding::same(6.0),
                pal.row_hover,
                Stroke::NONE,
            )),
        );
    }
    if let Some(idx) = divider {
        let rect = inner.response.rect;
        let y = rect.top();
        ui.painter().set(
            idx,
            egui::Shape::LineSegment {
                points: [
                    egui::pos2(rect.left() + 16.0, y),
                    egui::pos2(rect.right() - 16.0, y),
                ],
                stroke: Stroke::new(1.0, pal.hairline),
            },
        );
    }
    *first = false;
}

fn row_frame_vertical<F>(
    ui: &mut Ui,
    pal: ThemePalette,
    first: &mut bool,
    label: &str,
    desc: &str,
    body: F,
) where
    F: FnOnce(&mut Ui),
{
    let full_w = ui.available_width() - 20.0;
    let divider = if *first {
        None
    } else {
        Some(ui.painter().add(egui::Shape::Noop))
    };
    let hover_idx = ui.painter().add(egui::Shape::Noop);
    let inner = ui.allocate_ui_with_layout(
        egui::vec2(full_w, ROW_MIN_H),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(full_w);
            ui.add_space(10.0);
            ui.allocate_ui_with_layout(
                egui::vec2(full_w - 32.0, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(full_w - 32.0);
                    ui.label(
                        RichText::new(label)
                            .font(ui_font(13.0, false))
                            .color(pal.text),
                    );
                    if !desc.is_empty() {
                        ui.add_space(2.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(desc)
                                    .font(ui_font(11.0, false))
                                    .color(pal.muted),
                            )
                            .wrap(true),
                        );
                    }
                    ui.add_space(8.0);
                    body(ui);
                },
            );
            ui.add_space(10.0);
        },
    );
    let hrect = egui::Rect::from_min_max(
        egui::pos2(inner.response.rect.left() + 1.0, inner.response.rect.top() + 1.0),
        egui::pos2(inner.response.rect.right() - 1.0, inner.response.rect.bottom() - 1.0),
    );
    if ui
        .interact(hrect, ui.id().with(("settings-row", label)), egui::Sense::hover())
        .hovered()
    {
        ui.painter().set(
            hover_idx,
            egui::Shape::Rect(egui::epaint::RectShape::new(
                hrect,
                Rounding::same(6.0),
                pal.row_hover,
                Stroke::NONE,
            )),
        );
    }
    if let Some(idx) = divider {
        let rect = inner.response.rect;
        let y = rect.top();
        ui.painter().set(
            idx,
            egui::Shape::LineSegment {
                points: [
                    egui::pos2(rect.left() + 16.0, y),
                    egui::pos2(rect.right() - 16.0, y),
                ],
                stroke: Stroke::new(1.0, pal.hairline),
            },
        );
    }
    *first = false;
}


#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

fn button(ui: &mut Ui, pal: ThemePalette, label: &str, kind: Btn) -> egui::Response {
    let (bg, fg, stroke) = match kind {
        Btn::Primary => (pal.accent, Color32::WHITE, Stroke::NONE),
        Btn::Secondary => (
            pal.surface,
            pal.text,
            Stroke::new(1.0, pal.border),
        ),
        Btn::Ghost => (Color32::TRANSPARENT, pal.text, Stroke::NONE),
        Btn::Danger => (Color32::TRANSPARENT, pal.destructive, Stroke::NONE),
    };
    ui.add(
        egui::Button::new(RichText::new(label).font(ui_font(12.5, false)).color(fg))
            .fill(bg)
            .stroke(stroke)
            .rounding(Rounding::same(6.0))
            .min_size(egui::vec2(0.0, 26.0)),
    )
}

fn disabled_button(ui: &mut Ui, pal: ThemePalette, label: &str) -> egui::Response {
    ui.add_enabled(
        false,
        egui::Button::new(
            RichText::new(label)
                .font(ui_font(12.5, false))
                .color(pal.text_tertiary),
        )
        .fill(pal.surface)
        .stroke(Stroke::new(1.0, pal.border))
        .rounding(Rounding::same(6.0))
        .min_size(egui::vec2(0.0, 26.0)),
    )
}

fn disabled_note(ui: &mut Ui, pal: ThemePalette, text: &str) {
    ui.label(
        RichText::new(text)
            .font(ui_font(11.0, false))
            .color(pal.text_tertiary),
    );
}

fn toggle(ui: &mut Ui, pal: ThemePalette, on: &mut bool) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(40.0, 22.0), egui::Sense::click());
    let t = ui.ctx().animate_bool(resp.id, *on);
    let track = if *on { pal.accent } else { pal.surface2 };
    let border = if *on {
        Stroke::NONE
    } else {
        Stroke::new(1.0, pal.border)
    };
    ui.painter()
        .rect(rect, 11.0, track, border);
    let knob_x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
    let knob = egui::Rect::from_center_size(
        egui::pos2(knob_x, rect.center().y),
        egui::vec2(16.0, 16.0),
    );
    ui.painter().circle_filled(knob.center(), 8.0, Color32::WHITE);
    if resp.clicked() {
        *on = !*on;
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}


fn value_chip(ui: &mut Ui, pal: ThemePalette, text: &str, width: f32) {
    let frame = egui::Frame::none()
        .fill(pal.surface2)
        .stroke(Stroke::new(1.0, pal.hairline))
        .rounding(Rounding::same(6.0))
        .inner_margin(egui::Margin { left: 10.0, right: 10.0, top: 5.0, bottom: 5.0 });
    frame.show(ui, |ui| {
        ui.set_width(width);
        ui.label(
            RichText::new(text)
                .font(ui_font(12.0, false))
                .color(pal.text_tertiary),
        );
    });
}

fn text_input(ui: &mut Ui, pal: ThemePalette, buf: &mut String, width: f32, hint: &str) -> egui::Response {
    let frame = egui::Frame::none()
        .fill(pal.surface)
        .stroke(Stroke::new(1.0, pal.border))
        .rounding(Rounding::same(6.0))
        .inner_margin(egui::Margin { left: 8.0, right: 8.0, top: 5.0, bottom: 5.0 });
    let inner = frame.show(ui, |ui| {
        ui.add(
            egui::TextEdit::singleline(buf)
                .frame(false)
                .font(ui_font(12.5, false))
                .text_color(pal.text)
                .hint_text(RichText::new(hint).font(ui_font(11.5, false)))
                .desired_width(width),
        )
    });
    if inner.inner.has_focus() {
        ui.painter().rect_stroke(
            inner.response.rect,
            Rounding::same(6.0),
            Stroke::new(1.0, pal.accent),
        );
    }
    inner.inner
}

fn text_input_pw(ui: &mut Ui, pal: ThemePalette, buf: &mut String, width: f32, hint: &str) -> egui::Response {
    let frame = egui::Frame::none()
        .fill(pal.surface)
        .stroke(Stroke::new(1.0, pal.border))
        .rounding(Rounding::same(6.0))
        .inner_margin(egui::Margin { left: 8.0, right: 8.0, top: 5.0, bottom: 5.0 });
    let inner = frame.show(ui, |ui| {
        ui.add(
            egui::TextEdit::singleline(buf)
                .frame(false)
                .password(true)
                .font(ui_font(12.5, false))
                .text_color(pal.text)
                .hint_text(RichText::new(hint).font(ui_font(11.5, false)))
                .desired_width(width),
        )
    });
    if inner.inner.has_focus() {
        ui.painter().rect_stroke(
            inner.response.rect,
            Rounding::same(6.0),
            Stroke::new(1.0, pal.accent),
        );
    }
    inner.inner
}

fn number_input(
    ui: &mut Ui,
    pal: ThemePalette,
    buf: &mut String,
    width: f32,
    suffix: &str,
) -> egui::Response {
    let mut resp = None;
    ui.horizontal(|ui| {
        let frame = egui::Frame::none()
            .fill(pal.surface)
            .stroke(Stroke::new(1.0, pal.border))
            .rounding(Rounding::same(6.0))
            .inner_margin(egui::Margin { left: 8.0, right: 8.0, top: 5.0, bottom: 5.0 });
        let inner = frame.show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(buf)
                    .frame(false)
                    .font(ui_font(12.5, false))
                    .text_color(pal.text)
                    .desired_width(width),
            )
        });
        if inner.inner.has_focus() {
            ui.painter().rect_stroke(
                inner.response.rect,
                Rounding::same(6.0),
                Stroke::new(1.0, pal.accent),
            );
        }
        if !suffix.is_empty() {
            ui.label(
                RichText::new(suffix)
                    .font(ui_font(11.0, false))
                    .color(pal.muted),
            );
        }
        resp = Some(inner.inner);
    });
    resp.expect("number input")
}

fn dropdown(
    ui: &mut Ui,
    pal: ThemePalette,
    id: &str,
    current: &mut String,
    width: f32,
    options: &[(&str, &str)],
) -> bool {
    let label = options
        .iter()
        .find(|(value, _)| value == current)
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| current.clone());
    let mut changed = false;
    let mut style = ui.style().as_ref().clone();
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.visuals.override_text_color = Some(pal.text);
    style.visuals.window_fill = pal.surface;
    style.visuals.window_stroke = Stroke::new(1.0, pal.border);
    style.visuals.window_rounding = Rounding::same(8.0);
    style.visuals.popup_shadow = egui::epaint::Shadow::NONE;
    style.visuals.selection.bg_fill = pal.accent_bg;
    style.visuals.selection.stroke = Stroke::new(1.0, pal.accent);
    {
        let w = &mut style.visuals.widgets;
        w.inactive.bg_fill = pal.surface;
        w.inactive.weak_bg_fill = pal.surface;
        w.inactive.bg_stroke = Stroke::new(1.0, pal.border);
        w.inactive.fg_stroke = Stroke::new(1.0, pal.text);
        w.inactive.rounding = Rounding::same(6.0);
        w.hovered.bg_fill = pal.nav_hover;
        w.hovered.weak_bg_fill = pal.nav_hover;
        w.hovered.bg_stroke = Stroke::new(1.0, pal.border);
        w.hovered.fg_stroke = Stroke::new(1.0, pal.text);
        w.hovered.rounding = Rounding::same(6.0);
        w.active.bg_fill = pal.nav_hover;
        w.active.weak_bg_fill = pal.nav_hover;
        w.active.bg_stroke = Stroke::new(1.0, pal.accent);
        w.active.fg_stroke = Stroke::new(1.0, pal.text);
        w.active.rounding = Rounding::same(6.0);
        w.open.bg_fill = pal.nav_hover;
        w.open.weak_bg_fill = pal.nav_hover;
        w.open.bg_stroke = Stroke::new(1.0, pal.border);
        w.open.fg_stroke = Stroke::new(1.0, pal.text);
    }
    ui.scope(|ui| {
        ui.set_style(style);
        egui::ComboBox::from_id_source(id)
            .width(width)
            .selected_text(RichText::new(label).font(ui_font(12.5, false)))
            .show_ui(ui, |ui| {
                for (value, label) in options {
                    if ui
                        .selectable_value(current, (*value).to_string(), *label)
                        .clicked()
                    {
                        changed = true;
                    }
                }
            });
    });
    changed
}

fn segmented(
    ui: &mut Ui,
    id: &str,
    labels: &[&str],
    selected: &mut usize,
    pal: ThemePalette,
) -> bool {
    let font = ui_font(12.0, false);
    let widths: Vec<f32> = labels
        .iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap((*label).to_string(), font.clone(), pal.text)
                .size()
                .x
                + 22.0
        })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(total, 30.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, Rounding::same(8.0), pal.nav_hover);
    let mut x = rect.left() + 2.0;
    let mut changed = false;
    for (idx, label) in labels.iter().enumerate() {
        let seg = egui::Rect::from_min_size(
            egui::pos2(x, rect.top() + 2.0),
            egui::vec2(widths[idx], 26.0),
        );
        let seg_resp = ui.interact(
            seg,
            ui.id().with((id, idx)),
            egui::Sense::click(),
        );
        if *selected == idx {
            ui.painter()
                .rect_filled(seg, Rounding::same(6.0), pal.surface);
        } else if seg_resp.hovered() {
            ui.painter()
                .rect_filled(seg, Rounding::same(6.0), pal.chrome);
        }
        ui.painter().text(
            seg.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            font.clone(),
            if *selected == idx { pal.text } else { pal.muted },
        );
        if seg_resp.clicked() && *selected != idx {
            *selected = idx;
            changed = true;
        }
        x += widths[idx];
    }
    let _ = resp;
    changed
}

fn theme_card(
    ui: &mut Ui,
    pal: ThemePalette,
    label: &str,
    preview: ThemePalette,
    selected: bool,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(150.0, 84.0), egui::Sense::click());
    ui.painter()
        .rect(rect, Rounding::same(8.0), preview.surface, Stroke::new(1.0, preview.hairline));
    let pad = 8.0;
    let area = rect.shrink(pad);
    let bar = |painter: &egui::Painter, y: f32, h: f32, color: Color32| {
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(area.left(), y), egui::vec2(area.width() - 24.0, h)),
            Rounding::same(2.0),
            color,
        );
    };
    bar(ui.painter(), area.top() + 4.0, 8.0, preview.text);
    bar(ui.painter(), area.top() + 18.0, 6.0, preview.muted);
    bar(ui.painter(), area.top() + 30.0, 6.0, preview.surface2);
    bar(ui.painter(), area.top() + 42.0, 6.0, preview.border);
    ui.painter().circle_filled(
        egui::pos2(area.right() - 8.0, area.top() + 10.0),
        6.0,
        preview.accent,
    );
    ui.painter().text(
        egui::pos2(area.left(), area.bottom() - 4.0),
        egui::Align2::LEFT_BOTTOM,
        label,
        ui_font(11.5, false),
        preview.text,
    );
    if selected {
        ui.painter()
            .rect_stroke(rect.shrink(1.0), Rounding::same(8.0), Stroke::new(2.0, pal.accent));
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn color_dot(ui: &mut Ui, color: Color32, selected: bool, pal: ThemePalette) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::click());
    let center = rect.center();
    if selected {
        ui.painter()
            .circle_stroke(center, 13.0, Stroke::new(2.0, pal.text));
    }
    ui.painter()
        .circle_stroke(center, 10.0, Stroke::new(1.0, pal.border));
    ui.painter().circle_filled(center, 10.0, color);
    if selected {
        let check = egui::Shape::line(
            vec![
                egui::pos2(center.x - 4.0, center.y),
                egui::pos2(center.x - 1.0, center.y + 3.0),
                egui::pos2(center.x + 4.5, center.y - 3.5),
            ],
            Stroke::new(1.8, Color32::WHITE),
        );
        ui.painter().extend(vec![check]);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn caption(ui: &mut Ui, pal: ThemePalette, text: &str) {
    ui.label(
        RichText::new(text)
            .font(ui_font(11.0, true))
            .color(pal.muted),
    );
}

fn beep() {
    std::thread::spawn(|| {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", "[console]::beep(880,150)"])
                .creation_flags(0x0800_0000)
                .output();
        }
    });
}

// ---------------------------------------------------------------------------
// Page renderers
// ---------------------------------------------------------------------------

impl SettingsDialog {
    fn rows_hit(&self, hit: bool, rows: &[(&str, &str)]) -> bool {
        hit || rows.iter().any(|(label, desc)| self.matches(label, desc))
    }

    fn number_row(
        &mut self,
        ui: &mut Ui,
        pal: ThemePalette,
        key: &'static str,
        width: f32,
        suffix: &str,
    ) -> Option<f64> {
        let changed = match self.num.get_mut(key) {
            Some(buf) => number_input(ui, pal, buf, width, suffix).changed(),
            None => false,
        };
        if !changed {
            return None;
        }
        self.num.get(key).and_then(|s| s.trim().parse::<f64>().ok())
    }

    fn report_text(&self) -> String {
        let mut out = format!("DM Ultra {} diagnostics\n", env!("CARGO_PKG_VERSION"));
        for (ok, text) in &self.doctor {
            out.push_str(if *ok { "[ok] " } else { "[warn] " });
            out.push_str(text);
            out.push('\n');
        }
        out
    }

    // ----- General ---------------------------------------------------------

    fn page_general(&mut self, ui: &mut Ui, pal: ThemePalette, col: Col) {
        if col.left() {
            let hit = self.section_hit("Startup", None);
            if self.rows_hit(
                hit,
                &[
                    ("Launch at Startup", "Start DM Ultra automatically when you sign in to Windows."),
                ],
            ) {
                section_card(ui, pal, Some(("Startup", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Launch at Startup", "Start DM Ultra automatically when you sign in to Windows.", |ui| {
                        if toggle(ui, pal, &mut self.draft.auto_startup).clicked() {
                            self.set_dirty();
                        }
                    });
                });
            }
            let hit = self.section_hit("Sidebar", None);
            if self.rows_hit(
                hit,
                &[
                    ("Show Status", "Show the status filter section in the main window."),
                    ("Show Queues", "Show the queues section in the main window."),
                    ("Show Category", "Show the category section in the main window."),
                ],
            ) {
                section_card(ui, pal, Some(("Sidebar", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Show Status", "Show the status filter section in the main window.", |ui| {
                        if toggle(ui, pal, &mut self.draft.show_sidebar_status).clicked() {
                            self.set_dirty();
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Show Queues", "Show the queues section in the main window.", |ui| {
                        if toggle(ui, pal, &mut self.draft.show_sidebar_queues).clicked() {
                            self.set_dirty();
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Show Category", "Show the category section in the main window.", |ui| {
                        if toggle(ui, pal, &mut self.draft.show_sidebar_category).clicked() {
                            self.set_dirty();
                        }
                    });
                });
            }
        }
        if col.right() {
            let hit = self.section_hit("Clipboard", None);
            if self.rows_hit(
                hit,
                &[
                    ("Clipboard Watcher", "Watch the clipboard for download links."),
                    ("Probe extensionless links online", "Probe links without a file extension with a HEAD request."),
                    ("Custom watched extensions", "Extra file extensions treated as downloads."),
                ],
            ) {
                section_card(ui, pal, Some(("Clipboard", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Clipboard Watcher", "Watch the clipboard for download links.", |ui| {
                        if toggle(ui, pal, &mut self.draft.monitor_clipboard).clicked() {
                            self.set_dirty();
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Probe extensionless links online", "Probe links without a file extension with a HEAD request.", |ui| {
                        if toggle(ui, pal, &mut self.draft.clipboard_probe).clicked() {
                            self.set_dirty();
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Custom watched extensions", "Extra file extensions treated as downloads.", |ui| {
                        if text_input(ui, pal, &mut self.ext, 180.0, ".mp4, .zip").changed() {
                            self.draft.clipboard_extensions = self.ext.trim().to_string();
                            self.set_dirty();
                        }
                    });
                });
            }
            let hit = self.section_hit("Browser", None);
            if self.rows_hit(
                hit,
                &[
                    ("Browser integration", "Click download links in Chrome/Edge to open them in DM Ultra (loads a one-time developer-mode extension)."),
                    ("Auto-download links", "Start downloads from browser link clicks immediately, without showing the File Info dialog."),
                ],
            ) {
                section_card(ui, pal, Some(("Browser", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Browser integration", "Click download links in Chrome/Edge to open them in DM Ultra (loads a one-time developer-mode extension).", |ui| {
                        if button(ui, pal, "Install", Btn::Secondary).clicked() {
                            match crate::browser::install() {
                                Ok(dir) => {
                                    self.toast("Integration installed. Load the extension folder in your browser's extensions page (developer mode).");
                                    let _ = std::process::Command::new("explorer")
                                        .arg(&dir)
                                        .spawn();
                                }
                                Err(e) => {
                                    self.toast(&format!("Install failed: {e}"));
                                }
                            }
                        }
                        if button(ui, pal, "Uninstall", Btn::Secondary).clicked() {
                            match crate::browser::uninstall() {
                                Ok(()) => self.toast("Browser integration removed."),
                                Err(e) => self.toast(&format!("Uninstall failed: {e}")),
                            }
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Auto-download links", "Start downloads from browser link clicks immediately, without showing the File Info dialog.", |ui| {
                        if toggle(ui, pal, &mut self.draft.browser_auto_download).clicked() {
                            self.set_dirty();
                        }
                    });
                });
            }
            let hit = self.section_hit("Behavior", None);
            if self.rows_hit(
                hit,
                &[
                    ("Remember window positions", "Restore the main window and dialogs to their last positions (and the main window size) on launch."),
                    ("Keep Awake While Downloading", "Prevent the system from sleeping while downloads are active."),
                ],
            ) {
                section_card(ui, pal, Some(("Behavior", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Remember window positions", "Restore the main window and dialogs to their last positions (and the main window size) on launch.", |ui| {
                        if toggle(ui, pal, &mut self.draft.remember_window_pos).clicked() {
                            self.set_dirty();
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Keep Awake While Downloading", "Prevent the system from sleeping while downloads are active.", |ui| {
                        if toggle(ui, pal, &mut self.draft.keep_awake).clicked() {
                            self.set_dirty();
                        }
                    });
                });
            }
        }
    }

    // ----- Appearance ------------------------------------------------------

    fn page_appearance(&mut self, ui: &mut Ui, pal: ThemePalette, col: Col) {
        if col.left() {
            let hit = self.section_hit("Language", None);
            if self.rows_hit(
                hit,
                &[("Language", "Interface language (English is the only shipped language).")],
            ) {
                section_card(ui, pal, Some(("Language", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Language", "Interface language (English is the only shipped language).", |ui| {
                        let mut value = self.draft.language.clone();
                        if dropdown(ui, pal, "language", &mut value, 180.0, &[("system", "System"), ("en", "English")]) {
                            self.draft.language = value;
                            self.set_dirty();
                        }
                    });
                });
            }
            let hit = self.section_hit("Theme", None);
            if self.rows_hit(hit, &[("Theme", "Light and dark theme styles for the active mode.")]) {
                section_card(ui, pal, Some(("Theme", None)), |ui, drawn| {
                    let mut first = true;
                    srowv!(self, ui, pal, first, drawn, hit, "Theme", "Light and dark theme styles for the active mode.", |ui| {
                        ui.horizontal(|ui| {
                            for (id, label) in [("defaultLight", "Default Light"), ("warmLight", "Warm Light")] {
                                let preview = Appearance {
                                    mode: "light".to_string(),
                                    light_theme: id.to_string(),
                                    dark_theme: self.draft.dark_theme.clone(),
                                    accent: self.draft.accent.clone(),
                                    ui_scale: 100,
                                    font_size: 13.0,
                                    system_dark: self.system_dark,
                                }
                                .palette();
                                let selected = self.draft.light_theme == id && !self.palette().dark;
                                if theme_card(ui, pal, label, preview, selected).clicked() {
                                    self.draft.light_theme = id.to_string();
                                    self.draft.theme_mode = "light".to_string();
                                    self.set_dirty();
                                }
                                ui.add_space(8.0);
                            }
                        });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            for (id, label) in [
                                ("dmUltra", "DM Ultra Dark"),
                                ("defaultDark", "Default Dark"),
                                ("midnightBlue", "Midnight Blue"),
                                ("nord", "Nord"),
                            ] {
                                let preview = Appearance {
                                    mode: "dark".to_string(),
                                    light_theme: self.draft.light_theme.clone(),
                                    dark_theme: id.to_string(),
                                    accent: self.draft.accent.clone(),
                                    ui_scale: 100,
                                    font_size: 13.0,
                                    system_dark: self.system_dark,
                                }
                                .palette();
                                let selected = self.draft.dark_theme == id && self.palette().dark;
                                if theme_card(ui, pal, label, preview, selected).clicked() {
                                    self.draft.dark_theme = id.to_string();
                                    self.draft.theme_mode = "dark".to_string();
                                    self.set_dirty();
                                }
                                ui.add_space(8.0);
                            }
                        });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if button(ui, pal, "Import\u{2026}", Btn::Secondary).clicked() {
                                if let Some(path) = rfd::FileDialog::new()
                                    .add_filter("DM Ultra theme", &["json"])
                                    .pick_file()
                                {
                                    match std::fs::read_to_string(&path) {
                                        Ok(text) => match serde_json::from_str::<ThemeFile>(&text) {
                                            Ok(theme) => {
                                                self.draft.theme_mode = theme.theme_mode;
                                                self.draft.light_theme = theme.light_theme;
                                                self.draft.dark_theme = theme.dark_theme;
                                                self.draft.accent = theme.accent;
                                                self.sync_buffers();
                                                self.set_dirty();
                                                self.toast("Theme settings imported.");
                                            }
                                            Err(error) => {
                                                self.toast(format!("Import failed: {error}"));
                                            }
                                        },
                                        Err(error) => {
                                            self.toast(format!("Import failed: {error}"));
                                        }
                                    }
                                }
                            }
                            if button(ui, pal, "Export\u{2026}", Btn::Secondary).clicked() {
                                if let Some(path) = rfd::FileDialog::new()
                                    .add_filter("DM Ultra theme", &["json"])
                                    .set_file_name("dm-ultra-theme.json")
                                    .save_file()
                                {
                                    let theme = ThemeFile {
                                        theme_mode: self.draft.theme_mode.clone(),
                                        light_theme: self.draft.light_theme.clone(),
                                        dark_theme: self.draft.dark_theme.clone(),
                                        accent: self.draft.accent.clone(),
                                    };
                                    match serde_json::to_string_pretty(&theme) {
                                        Ok(text) => match std::fs::write(&path, text) {
                                            Ok(()) => self.toast("Theme settings exported."),
                                            Err(error) => self.toast(format!("Export failed: {error}")),
                                        },
                                        Err(error) => self.toast(format!("Export failed: {error}")),
                                    }
                                }
                            }
                        });
                    });
                });
            }
        }
        if col.right() {
            let hit = self.section_hit("Theme Mode", None);
            if self.rows_hit(hit, &[("Theme Mode", "Follow the system, or force light/dark.")]) {
                section_card(ui, pal, Some(("Theme Mode", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Theme Mode", "Follow the system, or force light/dark.", |ui| {
                        let mut sel = match self.draft.theme_mode.as_str() {
                            "light" => 1,
                            "dark" => 2,
                            _ => 0,
                        };
                        if segmented(ui, "theme-mode", &["System", "Light", "Dark"], &mut sel, pal) {
                            self.draft.theme_mode = ["system", "light", "dark"][sel].to_string();
                            self.set_dirty();
                        }
                    });
                });
            }
            let hit = self.section_hit("Theme Color", None);
            let subtitle = accent_name(accent_color(&self.draft.accent));
            if self.rows_hit(hit, &[("Theme Color", "Accent color used for buttons and highlights.")]) {
                section_card(ui, pal, Some(("Theme Color", Some(subtitle))), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Theme Color", "Accent color used for buttons and highlights.", |ui| {
                        ui.horizontal(|ui| {
                            for name in ["blue", "green", "violet", "rose"] {
                                let color = accent_color(name);
                                let selected = self.draft.accent == name;
                                if color_dot(ui, color, selected, pal).clicked() {
                                    self.draft.accent = name.to_string();
                                    self.set_dirty();
                                }
                                ui.add_space(6.0);
                            }
                            let mut rgb = custom_accent_rgb(&self.draft.accent);
                            if ui
                                .color_edit_button_srgb(&mut rgb)
                                .on_hover_text("Custom accent color")
                                .changed()
                            {
                                self.draft.accent = format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]);
                                self.set_dirty();
                            }
                        });
                    });
                });
            }
            let hit = self.section_hit("Interface", None);
            if self.rows_hit(
                hit,
                &[
                    ("Interface Scale", "Scale the whole interface."),
                    ("Font Size", "Base font size for the windows."),
                ],
            ) {
                section_card(ui, pal, Some(("Interface", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Interface Scale", "Scale the whole interface.", |ui| {
                        if let Some(v) = self.number_row(ui, pal, "scale", 60.0, "%") {
                            self.draft.ui_scale = (v.round() as u32).clamp(80, 150);
                            self.set_dirty();
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Font Size", "Base font size for the windows.", |ui| {
                        if number_input(ui, pal, &mut self.font_buf, 60.0, "pt").changed() {
                            if let Ok(v) = self.font_buf.trim().parse::<f32>() {
                                self.draft.font_size = v.clamp(9.0, 24.0);
                                self.set_dirty();
                            }
                        }
                    });
                });
            }
        }
    }

    // ----- Download --------------------------------------------------------

    fn page_download(&mut self, ui: &mut Ui, pal: ThemePalette, col: Col) {
        if col.left() {
            let hit = self.section_hit("Location", None);
            if self.rows_hit(
                hit,
                &[
                    ("Default Save Directory", "Folder where finished downloads are stored."),
                    ("Remember Last Save Location", "Reuse the last used save folder for new downloads."),
                    ("Save each category to its own folder", "Create a subfolder per category (Video, Music, Documents, ...)."),
                ],
            ) {
                section_card(ui, pal, Some(("Location", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Default Save Directory", "Folder where finished downloads are stored.", |ui| {
                        if button(ui, pal, "Browse\u{2026}", Btn::Secondary).clicked() {
                            if let Some(dir) = rfd::FileDialog::new()
                                .set_directory(&self.draft.save_path)
                                .pick_folder()
                            {
                                self.draft.save_path = dir;
                                self.save_dir = self.draft.save_path.display().to_string();
                                self.set_dirty();
                            }
                        }
                        if text_input(ui, pal, &mut self.save_dir, 240.0, "").changed() {
                            let trimmed = self.save_dir.trim().to_string();
                            if !trimmed.is_empty() {
                                self.draft.save_path = std::path::PathBuf::from(trimmed);
                                self.set_dirty();
                            }
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Remember Last Save Location", "Reuse the last used save folder for new downloads.", |ui| {
                        if toggle(ui, pal, &mut self.draft.remember_last_save_dir).clicked() {
                            self.set_dirty();
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Save each category to its own folder", "Create a subfolder per category (Video, Music, Documents, ...).", |ui| {
                        if toggle(ui, pal, &mut self.draft.category_dirs).clicked() {
                            self.set_dirty();
                        }
                    });
                });
            }
            let hit = self.section_hit("Queue", None);
            if self.rows_hit(hit, &[("Default Queue", "Queue new downloads are added to.")]) {
                section_card(ui, pal, Some(("Queue", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Default Queue", "Queue new downloads are added to.", |ui| {
                        let mut value = self.draft.default_queue.clone();
                        if dropdown(ui, pal, "default-queue", &mut value, 180.0, &[("", "Main queue")]) {
                            self.draft.default_queue = value;
                            self.set_dirty();
                        }
                    });
                });
            }
            let hit = self.section_hit("Speed", None);
            if self.rows_hit(
                hit,
                &[
                    ("Download Limit", "Speed limit for all downloads (KB/s)."),
                ],
            ) {
                section_card(ui, pal, Some(("Speed", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Download Limit", "Speed limit for all downloads (KB/s).", |ui| {
                        if let Some(v) = self.number_row(ui, pal, "limit", 140.0, "KB/s") {
                            self.draft.speed_limit_bps =
                                (v.round() as u64).clamp(1, 10_000_000) * 1024;
                            self.set_dirty();
                        }
                        if toggle(ui, pal, &mut self.draft.use_speed_limiter).clicked() {
                            self.set_dirty();
                        }
                    });
                });
            }
        }
        if col.right() {
            let hit = self.section_hit("Behavior", None);
            if self.rows_hit(
                hit,
                &[
                    ("Use Server File Time", "Set the downloaded file's modification time from the server's Last-Modified header."),
                    ("When file exists", "What to do when the target file already exists."),
                    ("Periodically check completed files while idle", "Rescan completed downloads when the window regains focus."),
                ],
            ) {
                section_card(ui, pal, Some(("Behavior", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Use Server File Time", "Set the downloaded file's modification time from the server's Last-Modified header.", |ui| {
                        if toggle(ui, pal, &mut self.draft.use_server_file_time).clicked() {
                            self.set_dirty();
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "When file exists", "What to do when the target file already exists.", |ui| {
                        let mut value = self.draft.file_exists_mode.clone();
                        if dropdown(
                            ui,
                            pal,
                            "file-exists",
                            &mut value,
                            190.0,
                            &[
                                ("rename", "Save with a different name"),
                                ("overwrite", "Overwrite"),
                            ],
                        ) {
                            self.draft.file_exists_mode = value;
                            self.set_dirty();
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Periodically check completed files while idle", "Rescan completed downloads when the window regains focus.", |ui| {
                        if toggle(ui, pal, &mut self.draft.idle_file_scan).clicked() {
                            self.set_dirty();
                        }
                    });
                });
            }
            let hit = self.section_hit("Connections", None);
            if self.rows_hit(
                hit,
                &[
                    ("Thread Count", "Number of parallel connections per download."),
                ],
            ) {
                section_card(ui, pal, Some(("Connections", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Thread Count", "Number of parallel connections per download.", |ui| {
                        if let Some(v) = self.number_row(ui, pal, "threads", 70.0, "") {
                            self.draft.max_connections = (v.round() as u32).clamp(1, 64);
                            self.set_dirty();
                        }
                    });
                });
            }
            let hit = self.section_hit("Resume", None);
            if self.rows_hit(
                hit,
                &[
                    ("Resume unfinished downloads on startup", "Resume unfinished downloads when the app starts."),
                ],
            ) {
                section_card(ui, pal, Some(("Resume", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Resume unfinished downloads on startup", "Resume unfinished downloads when the app starts.", |ui| {
                        if toggle(ui, pal, &mut self.draft.auto_resume_on_start).clicked() {
                            self.set_dirty();
                        }
                    });
                });
            }
        }
    }

    // ----- Proxy -----------------------------------------------------------

    fn page_proxy(&mut self, ui: &mut Ui, pal: ThemePalette, col: Col) {
        if col.left() {
            let hit = self.section_hit("Mode", None);
            if self.rows_hit(hit, &[("Proxy Mode", "How downloads connect to the network.")]) {
                section_card(ui, pal, Some(("Mode", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Proxy Mode", "How downloads connect to the network.", |ui| {
                        let mut sel = match self.draft.proxy_mode.as_str() {
                            "system" => 1,
                            "manual" => 2,
                            _ => 0,
                        };
                        if segmented(ui, "proxy-mode", &["Direct", "System", "Manual"], &mut sel, pal) {
                            self.draft.proxy_mode = ["none", "system", "manual"][sel].to_string();
                            if self.draft.proxy_mode == "manual" {
                                self.write_proxy();
                            }
                            self.set_dirty();
                        }
                    });
                });
            }
            let hit = self.section_hit("Bypass", None);
            if self.rows_hit(hit, &[("Bypass List", "Hosts that never use the proxy (comma separated).")]) {
                section_card(ui, pal, Some(("Bypass", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Bypass List", "Hosts that never use the proxy (comma separated).", |ui| {
                        if text_input(ui, pal, &mut self.draft.proxy_no_list, 260.0, "localhost, 127.0.0.1").changed() {
                            self.set_dirty();
                        }
                    });
                });
            }
        }
        if col.right() {
            let hit = self.section_hit("Manual Proxy", None);
            if self.rows_hit(
                hit,
                &[
                    ("Proxy Type", "Scheme used by the proxy server."),
                    ("Server Address", "Host name or IP of the proxy server."),
                    ("Port", "TCP port of the proxy server."),
                    ("Username", "Proxy authentication username."),
                    ("Password", "Proxy authentication password."),
                ],
            ) {
                section_card(ui, pal, Some(("Manual Proxy", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Proxy Type", "Scheme used by the proxy server.", |ui| {
                        let mut value = self.draft.proxy_type.clone();
                        if dropdown(
                            ui,
                            pal,
                            "proxy-type",
                            &mut value,
                            140.0,
                            &[("http", "HTTP"), ("https", "HTTPS"), ("socks4", "SOCKS4"), ("socks5", "SOCKS5")],
                        ) {
                            self.draft.proxy_type = value;
                            self.write_proxy();
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Server Address", "Host name or IP of the proxy server.", |ui| {
                        if text_input(ui, pal, &mut self.proxy_host, 200.0, "127.0.0.1").changed() {
                            self.write_proxy();
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Port", "TCP port of the proxy server.", |ui| {
                        if number_input(ui, pal, &mut self.proxy_port, 70.0, "").changed() {
                            self.write_proxy();
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Username", "Proxy authentication username.", |ui| {
                        if text_input(ui, pal, &mut self.proxy_user, 180.0, "").changed() {
                            let user = self.proxy_user.trim();
                            self.draft.proxies.user =
                                if user.is_empty() { None } else { Some(user.to_string()) };
                            self.set_dirty();
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Password", "Proxy authentication password.", |ui| {
                        if text_input_pw(ui, pal, &mut self.proxy_pass, 180.0, "").changed() {
                            let pass = self.proxy_pass.trim();
                            self.draft.proxies.password =
                                if pass.is_empty() { None } else { Some(pass.to_string()) };
                            self.set_dirty();
                        }
                    });
                });
            }
            let hit = self.section_hit("Test", None);
            if self.rows_hit(hit, &[("Test Connection", "Check that the proxy accepts TCP connections.")]) {
                section_card(ui, pal, Some(("Test", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Test Connection", "Check that the proxy accepts TCP connections.", |ui| {
                        if self.proxy_testing {
                            caption(ui, pal, "Testing\u{2026}");
                        } else if button(ui, pal, "Test connection", Btn::Secondary).clicked() {
                            self.run_proxy_test();
                        }
                    });
                    if self.draft.proxy_mode == "system" {
                        let detected = system_proxy_url().unwrap_or_else(|| "Not detected".to_string());
                        srow!(self, ui, pal, first, drawn, hit, "System proxy", "Detected from the Windows Internet Settings.", |ui| {
                            value_chip(ui, pal, &detected, 200.0);
                        });
                    }
                });
            }
        }
        let hit = self.section_hit("Per-site proxy rules", None);
        if self.rows_hit(
            hit,
            &[("Per-site proxy rules", "Route specific hosts through a different proxy (Manual mode).")],
        ) {
            section_card(
                ui,
                pal,
                Some(("Per-site proxy rules", Some("First matching pattern wins \u{2014} use \"none\" to go direct"))),
                |ui, drawn| {
                    let mut first = true;
                    let mut remove: Option<usize> = None;
                    *drawn = true;
                    let count = self.draft.auto_proxy_rules.len();
                    for i in 0..count {
                        let label = format!("Rule {}", i + 1);
                        row_frame_vertical(ui, pal, &mut first, &label, "Wildcard host pattern and proxy URL.", |ui| {
                            let rule = &mut self.draft.auto_proxy_rules[i];
                            let mut changed = false;
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Host pattern").small().color(pal.muted));
                                changed |= ui
                                    .add(
                                        egui::TextEdit::singleline(&mut rule.pattern)
                                            .desired_width(180.0)
                                            .hint_text("*.example.com"),
                                    )
                                    .changed();
                                ui.label(RichText::new("Proxy").small().color(pal.muted));
                                changed |= ui
                                    .add(
                                        egui::TextEdit::singleline(&mut rule.proxy)
                                            .desired_width(220.0)
                                            .hint_text("socks5://127.0.0.1:1080 / none"),
                                    )
                                    .changed();
                                if button(ui, pal, "Remove", Btn::Secondary).clicked() {
                                    remove = Some(i);
                                }
                            });
                            if changed {
                                self.set_dirty();
                            }
                        });
                    }
                    if let Some(i) = remove {
                        self.draft.auto_proxy_rules.remove(i);
                        self.set_dirty();
                    }
                    row_frame_vertical(ui, pal, &mut first, "Add rule", "Add a per-site proxy override.", |ui| {
                        if button(ui, pal, "Add rule", Btn::Secondary).clicked() {
                            self.draft.auto_proxy_rules.push(crate::state::AutoProxyRule {
                                pattern: String::new(),
                                proxy: String::new(),
                            });
                            self.set_dirty();
                        }
                    });
                },
            );
        }
    }

    // ----- Notifications ---------------------------------------------------

    fn page_notify(&mut self, ui: &mut Ui, pal: ThemePalette, col: Col) {
        if col.left() {
            let hit = self.section_hit("Notifications", None);
            if self.rows_hit(
                hit,
                &[
                    ("Completion Notifications", "Show a system notification when a download finishes."),
                    ("Test Notification", "Preview the completion notification."),
                ],
            ) {
                section_card(ui, pal, Some(("Notifications", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Completion Notifications", "Show a system notification when a download finishes.", |ui| {
                        if toggle(ui, pal, &mut self.draft.notify_on_complete).clicked() {
                            self.set_dirty();
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Test Notification", "Preview the completion notification.", |ui| {
                        if button(ui, pal, "Send test", Btn::Secondary).clicked() {
                            beep();
                            self.toast("Test notification sent (system alert sound).");
                        }
                    });
                });
            }
        }
        if col.right() {
            let hit = self.section_hit("Windows", None);
            if self.rows_hit(
                hit,
                &[
                    ("Show download progress window", "Open the progress window when a download starts."),
                    ("Show download complete window", "Open the completion window when a download finishes."),
                ],
            ) {
                section_card(ui, pal, Some(("Windows", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Show download progress window", "Open the progress window when a download starts.", |ui| {
                        if toggle(ui, pal, &mut self.draft.show_progress_window).clicked() {
                            self.set_dirty();
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Show download complete window", "Open the completion window when a download finishes.", |ui| {
                        if toggle(ui, pal, &mut self.draft.show_completion_window).clicked() {
                            self.set_dirty();
                        }
                    });
                });
            }
            let hit = self.section_hit("Webhooks", None);
            if self.rows_hit(
                hit,
                &[("Webhooks", "POST a JSON event to external services when tasks change state.")],
            ) {
                section_card(
                    ui,
                    pal,
                    Some(("Webhooks", Some("HTTP POST \u{2014} started / completed / failed / added / deleted"))),
                    |ui, drawn| {
                        let mut first = true;
                        let mut remove: Option<usize> = None;
                        *drawn = true;
                        let count = self.webhooks.len();
                        for i in 0..count {
                            let name = self.webhooks[i].name.trim().to_string();
                            let label = if name.is_empty() { format!("Webhook {}", i + 1) } else { name };
                            row_frame_vertical(ui, pal, &mut first, &label, "POST JSON when tasks change state.", |ui| {
                                let (hooks, bufs) = (&mut self.webhooks, &mut self.hook_bufs);
                                let hook = &mut hooks[i];
                                let buf = &mut bufs[i];
                                let mut changed = false;
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("Name").small().color(pal.muted));
                                    changed |= ui
                                        .add(egui::TextEdit::singleline(&mut hook.name).desired_width(140.0))
                                        .changed();
                                    changed |= toggle(ui, pal, &mut hook.enabled).clicked();
                                    if button(ui, pal, "Remove", Btn::Secondary).clicked() {
                                        remove = Some(i);
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("URL").small().color(pal.muted));
                                    changed |= ui
                                        .add(
                                            egui::TextEdit::singleline(&mut hook.url)
                                                .desired_width(300.0)
                                                .hint_text("https://example.com/hook"),
                                        )
                                        .changed();
                                });
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("Events").small().color(pal.muted));
                                    if ui
                                        .add(
                                            egui::TextEdit::singleline(buf)
                                                .desired_width(240.0)
                                                .hint_text("completed, failed"),
                                        )
                                        .changed()
                                    {
                                        hook.events = buf
                                            .split(',')
                                            .map(|s| s.trim().to_string())
                                            .filter(|s| !s.is_empty())
                                            .collect();
                                        changed = true;
                                    }
                                });
                                if changed {
                                    self.hooks_dirty = true;
                                }
                            });
                        }
                        if let Some(i) = remove {
                            self.webhooks.remove(i);
                            self.hook_bufs.remove(i);
                            self.hooks_dirty = true;
                        }
                        row_frame_vertical(ui, pal, &mut first, "Add webhook", "Notify an external service (Discord/Slack-compatible JSON POST).", |ui| {
                            if button(ui, pal, "Add webhook", Btn::Secondary).clicked() {
                                let n = self.webhooks.len() + 1;
                                self.webhooks.push(Webhook {
                                    name: format!("Webhook {n}"),
                                    url: String::new(),
                                    events: vec!["completed".into(), "failed".into()],
                                    enabled: true,
                                });
                                self.hook_bufs.push("completed, failed".into());
                                self.hooks_dirty = true;
                            }
                        });
                    },
                );
            }
        }
    }

    // ----- Doctor ----------------------------------------------------------

    fn page_doctor(&mut self, ui: &mut Ui, pal: ThemePalette) {
        let hit = self.section_hit("Diagnostics", None);
        if self.rows_hit(
            hit,
            &[("Run diagnostics", "Run the environment self-check."), ("Diagnostics report", "Self-check results.")],
        ) {
            section_card(ui, pal, Some(("Diagnostics", None)), |ui, drawn| {
                let mut first = true;
                srow!(self, ui, pal, first, drawn, hit, "Run diagnostics", "Run the environment self-check.", |ui| {
                    if button(ui, pal, "Run diagnostics", Btn::Primary).clicked() {
                        self.run_doctor();
                    }
                });
                let results = self.doctor.clone();
                if !self.doctor_ran {
                    srow!(self, ui, pal, first, drawn, true, "Diagnostics report", "Press \"Run diagnostics\" to start.", |ui| {
                        disabled_note(ui, pal, "Not run yet");
                    });
                }
                for (ok, text) in results {
                    let color = if ok { pal.success } else { pal.warning };
                    srow!(self, ui, pal, first, drawn, true, &text, "", |ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().circle_filled(r.center(), 4.0, color);
                    });
                }
            });
        }
        let hit = self.section_hit("Maintenance", None);
        if self.rows_hit(
            hit,
            &[
                ("Clean temporary files", "Remove leftover partial files."),
                ("Copy report", "Copy the diagnostics report to the clipboard."),
                ("Open folder", "Open the data folder in Explorer."),
            ],
        ) {
            section_card(ui, pal, Some(("Maintenance", None)), |ui, drawn| {
                let mut first = true;
                srow!(self, ui, pal, first, drawn, hit, "Clean temporary files", "Remove leftover partial files.", |ui| {
                    if button(ui, pal, "Clean temp files", Btn::Danger).clicked() {
                        self.clean_temp = true;
                        self.toast("Temporary files will be cleaned when the dialog closes.");
                    }
                });
                srow!(self, ui, pal, first, drawn, hit, "Copy report", "Copy the diagnostics report to the clipboard.", |ui| {
                    if button(ui, pal, "Copy report", Btn::Secondary).clicked() {
                        self.run_doctor();
                        let report = self.report_text();
                        match arboard::Clipboard::new().and_then(|mut c| c.set_text(report)) {
                            Ok(()) => self.toast("Report copied to the clipboard."),
                            Err(e) => self.toast(format!("Copy failed: {e}")),
                        }
                    }
                });
                srow!(self, ui, pal, first, drawn, hit, "Open folder", "Open the data folder in Explorer.", |ui| {
                    if button(ui, pal, "Open folder", Btn::Ghost).clicked() {
                        let dir = crate::state::state_dir();
                        let _ = std::process::Command::new("explorer").arg(dir).spawn();
                        self.toast("Opened the data folder.");
                    }
                });
            });
        }
    }

    // ----- About -----------------------------------------------------------

    fn page_about(&mut self, ui: &mut Ui, pal: ThemePalette, col: Col) {
        if col.left() {
            let hit = self.section_hit("Version", None);
            if self.rows_hit(
                hit,
                &[
                    ("Current Version", "Version of DM Ultra."),
                ],
            ) {
                section_card(ui, pal, Some(("Version", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Current Version", "Version of DM Ultra.", |ui| {
                        value_chip(ui, pal, env!("CARGO_PKG_VERSION"), 120.0);
                    });
                });
            }
        }
        if col.right() {
            let hit = self.section_hit("Logs", None);
            if self.rows_hit(
                hit,
                &[
                    ("Write diagnostic log file", "Append diagnostic events to a rotating log file."),
                    ("Log File", "Location of the application log."),
                    ("Open Log Folder", "Reveal the log folder in Explorer."),
                ],
            ) {
                section_card(ui, pal, Some(("Logs", None)), |ui, drawn| {
                    let mut first = true;
                    srow!(self, ui, pal, first, drawn, hit, "Write diagnostic log file", "Append diagnostic events to a rotating log file.", |ui| {
                        if toggle(ui, pal, &mut self.draft.log_to_file).clicked() {
                            self.set_dirty();
                        }
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Log File", "Location of the application log.", |ui| {
                        let path = crate::state::state_dir().join("logs").join("dmultra.log");
                        value_chip(ui, pal, &path.display().to_string(), 240.0);
                    });
                    srow!(self, ui, pal, first, drawn, hit, "Open Log Folder", "Reveal the log folder in Explorer.", |ui| {
                        if button(ui, pal, "Open\u{2026}", Btn::Secondary).clicked() {
                            self.open_log_dir = true;
                        }
                    });
                });
            }
        }
    }
}
