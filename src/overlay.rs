//! Win32 video overlay button (IDM-style): a native topmost window
//! positioned over the browser's video element. The browser extension
//! sends the video's screen coordinates via the native messaging host;
//! the GUI creates this overlay window to show a "Download this video"
//! button that floats above the video, exactly like IDMan.exe does.
//!
//! The overlay is a borderless topmost window with:
//!   - A blue button face showing "Download this video (FORMAT)"
//!   - A dropdown menu on click showing available resolutions/qualities
//!   - Win32-painted (GDI), no egui dependency — matches IDM's native look
//!
//! Visibility rules (important):
//!   - The overlay only ever shows over a video that is playing in a web
//!     browser: the foreground window must belong to a known browser
//!     process and must still contain the reported video rectangle.
//!   - The reported geometry must be plausible; stale or zeroed
//!     coordinates are rejected instead of being clamped to (0, 0).
//!   - The extension heartbeat must be fresh. When updates stop (video
//!     paused/closed, browser closed, app relaunched with a leftover
//!     `overlay.json`) the overlay hides itself.
//!   - Repaints/repositions are skipped when nothing changed so the
//!     button cannot flicker.

#![cfg(windows)]
#![allow(dead_code, unused_variables, unused_imports)]

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// Shared state between the GUI thread and the overlay window thread.
pub struct OverlayState {
    /// Whether the overlay should be visible (data present + fresh).
    pub visible: AtomicBool,
    /// Screen-space rect of the video element: (x, y, width, height).
    pub x: AtomicI32,
    pub y: AtomicI32,
    pub w: AtomicI32,
    pub h: AtomicI32,
    /// Video URL to download when clicked.
    pub url: std::sync::Mutex<String>,
    /// Video format label (MP4, M3U8, WEBM, etc).
    pub format: std::sync::Mutex<String>,
    /// Available qualities (one per line: "MP4 - 1920x1080|https://...").
    pub qualities: std::sync::Mutex<Vec<(String, String)>>,
    /// Currently selected quality index (-1 = show main button).
    pub menu_open: AtomicBool,
    /// Overlay window handle.
    pub hwnd: AtomicI32,
    /// Stop signal for the overlay thread.
    pub shutdown: AtomicBool,
    /// Time of the last fresh video update from the extension.
    last_feed: Mutex<Option<Instant>>,
    /// Whether the native window is currently shown.
    shown: AtomicBool,
    /// Last geometry pushed to the window (avoids needless repaints).
    last_x: AtomicI32,
    last_y: AtomicI32,
    last_w: AtomicI32,
    last_h: AtomicI32,
}

impl Default for OverlayState {
    fn default() -> Self {
        Self {
            visible: AtomicBool::new(false),
            x: AtomicI32::new(0),
            y: AtomicI32::new(0),
            w: AtomicI32::new(0),
            h: AtomicI32::new(0),
            url: std::sync::Mutex::new(String::new()),
            format: std::sync::Mutex::new(String::new()),
            qualities: std::sync::Mutex::new(Vec::new()),
            menu_open: AtomicBool::new(false),
            hwnd: AtomicI32::new(0),
            shutdown: AtomicBool::new(false),
            last_feed: Mutex::new(None),
            shown: AtomicBool::new(false),
            last_x: AtomicI32::new(i32::MIN),
            last_y: AtomicI32::new(i32::MIN),
            last_w: AtomicI32::new(0),
            last_h: AtomicI32::new(0),
        }
    }
}

/// IDM-style compact button: 26x26 icon + tiny dropdown arrow.
/// Menu remains wider (220px) to show quality labels.
const BTN_W: i32 = 26;
const BTN_H: i32 = 26;
const MENU_W: i32 = 220;
const MENU_ITEM_H: i32 = 24;

/// A video smaller than this is not "playing in a browser window" - it is
/// usually a thumbnail, an ad slot or a bad coordinate report.
const MIN_VIDEO_W: i32 = 160;
const MIN_VIDEO_H: i32 = 90;
/// Hide when no heartbeat arrived for this long (extension sends every ~2 s).
const STALE_AFTER: Duration = Duration::from_millis(6000);
/// How often the overlay thread re-checks freshness and the foreground app.
const TIMER_MS: u32 = 250;

const COLOR_BTN_FACE: u32 = 0x00A94F1A; // #1A4FAC in BGR
const COLOR_BTN_FACE_HOVER: u32 = 0x00D96325; // #2563D9 in BGR
const COLOR_BTN_TEXT: u32 = 0x00FFFFFF;
const COLOR_MENU_FACE: u32 = 0x00F0F0F0;
const COLOR_MENU_ITEM_HOVER: u32 = 0x00D96325;

/// Executables that host a playing video. Anything else (the DM Ultra
/// window, Explorer, a media player, the desktop) never gets the button.
const BROWSER_EXES: &[&str] = &[
    "chrome.exe",
    "chrome_proxy.exe",
    "msedge.exe",
    "msedgewebview2.exe",
    "firefox.exe",
    "brave.exe",
    "opera.exe",
    "opera_gx.exe",
    "vivaldi.exe",
    "chromium.exe",
    "thorium.exe",
    "librewolf.exe",
    "waterfox.exe",
    "floorp.exe",
    "zen.exe",
    "arc.exe",
];

// Win32 FFI
#[link(name = "user32")]
extern "system" {
    fn CreateWindowExW(
        dw_ex_style: u32,
        lp_class_name: *const u16,
        lp_window_name: *const u16,
        dw_style: u32,
        x: i32,
        y: i32,
        n_width: i32,
        n_height: i32,
        hwnd_parent: isize,
        h_menu: isize,
        h_instance: isize,
        lp_param: *const core::ffi::c_void,
    ) -> isize;
    fn DefWindowProcW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn DestroyWindow(hwnd: isize) -> i32;
    fn RegisterClassExW(lp_wnd_class: *const WNDCLASSEXW) -> u16;
    fn ShowWindow(hwnd: isize, n_cmd_show: i32) -> i32;
    fn UpdateWindow(hwnd: isize) -> i32;
    fn GetDC(hwnd: isize) -> isize;
    fn ReleaseDC(hwnd: isize, hdc: isize) -> i32;
    fn SetWindowPos(
        hwnd: isize,
        hwnd_insert_after: isize,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        u_flags: u32,
    ) -> i32;
    fn SetLayeredWindowAttributes(hwnd: isize, cr_key: u32, b_alpha: u8, dw_flags: u32) -> i32;
    fn GetSystemMetrics(n_index: i32) -> i32;
    fn SetFocus(hwnd: isize) -> isize;
    fn InvalidateRect(hwnd: isize, lp_rect: *const core::ffi::c_void, b_erase: i32) -> i32;
    fn TrackMouseEvent(lp_event_track: *const TRACKMOUSEEVENT) -> i32;
    fn GetCursorPos(lp_point: *mut POINT) -> i32;
    fn ScreenToClient(hwnd: isize, lp_point: *mut POINT) -> i32;

    fn GetForegroundWindow() -> isize;
    fn GetWindowThreadProcessId(hwnd: isize, lpdw_process_id: *mut u32) -> u32;
    fn GetWindowRect(hwnd: isize, lp_rect: *mut RECT) -> i32;
    fn IsIconic(hwnd: isize) -> i32;
    fn MonitorFromRect(lprc: *const RECT, dw_flags: u32) -> isize;
    fn GetMonitorInfoW(hmonitor: isize, lpmi: *mut MONITORINFO) -> i32;

    fn GetMessageW(
        lp_msg: *mut MSG,
        hwnd: isize,
        w_msg_filter_min: u32,
        w_msg_filter_max: u32,
    ) -> isize;
    fn TranslateMessage(lp_msg: *const MSG) -> i32;
    fn DispatchMessageW(lp_msg: *const MSG) -> isize;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateSolidBrush(color: u32) -> isize;
    fn DeleteObject(hObject: isize) -> i32;
    fn FillRect(hdc: isize, lp_rect: *const RECT, h_brush: isize) -> i32;
    fn SetBkMode(hdc: isize, mode: i32) -> i32;
    fn SetTextColor(hdc: isize, color: u32) -> u32;
    fn TextOutW(hdc: isize, x: i32, y: i32, lp_string: *const u16, c: i32) -> i32;
    fn SelectObject(hdc: isize, hgdiobj: isize) -> isize;
    fn Rectangle(hdc: isize, left: i32, top: i32, right: i32, bottom: i32) -> i32;
    fn SetDCPenColor(hdc: isize, color: u32) -> u32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(lp_module_name: *const u16) -> isize;
    fn CreateThread(
        lp_thread_attributes: *const core::ffi::c_void,
        dw_stack_size: usize,
        lp_start_address: usize,
        lp_parameter: *const core::ffi::c_void,
        dw_creation_flags: u32,
        lp_thread_id: *mut u32,
    ) -> isize;
    fn GetCurrentThreadId() -> u32;
    fn OpenProcess(dw_desired_access: u32, b_inherit_handle: i32, dw_process_id: u32) -> isize;
    fn QueryFullProcessImageNameW(
        h_process: isize,
        dw_flags: u32,
        lp_exe_name: *mut u16,
        lpdw_size: *mut u32,
    ) -> i32;
    fn CloseHandle(h_object: isize) -> i32;
}

#[repr(C)]
struct WNDCLASSEXW {
    cb_size: u32,
    style: u32,
    lpfn_wnd_proc: Option<unsafe extern "system" fn(isize, u32, usize, isize) -> isize>,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    h_instance: isize,
    h_icon: isize,
    h_cursor: isize,
    hbr_background: isize,
    lpsz_menu_name: *const u16,
    lpsz_class_name: *const u16,
    h_icon_sm: isize,
}

#[repr(C)]
struct RECT {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct MONITORINFO {
    cb_size: u32,
    rc_monitor: RECT,
    rc_work: RECT,
    dw_flags: u32,
}

#[repr(C)]
struct POINT {
    x: i32,
    y: i32,
}

#[repr(C)]
struct MSG {
    hwnd: isize,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: POINT,
    l_private: u32,
}

#[repr(C)]
struct TRACKMOUSEEVENT {
    cb_size: u32,
    dw_flags: u32,
    hwnd_track: isize,
    dw_hover_time: u32,
}

const WM_PAINT: u32 = 0x000F;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_DESTROY: u32 = 0x0002;
const WM_TIMER: u32 = 0x0113;
const WS_POPUP: u32 = 0x8000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_EX_TOPMOST: u32 = 0x0000_0008;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const WS_EX_LAYERED: u32 = 0x0008_0000;
const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
const WS_EX_TRANSPARENT: u32 = 0x0000_0020;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_SHOWWINDOW: u32 = 0x0040;
const HWND_TOPMOST: isize = -1;
const LWA_ALPHA: u32 = 0x0000_0002;
const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;
const SM_XVIRTUALSCREEN: i32 = 76;
const SM_YVIRTUALSCREEN: i32 = 77;
const SM_CXVIRTUALSCREEN: i32 = 78;
const SM_CYVIRTUALSCREEN: i32 = 79;
const TRANSPARENT_BK: i32 = 1;
const TME_LEAVE: u32 = 0x0000_0002;
const SW_HIDE: i32 = 0;
const SW_SHOWNOACTIVATE: i32 = 4;
const MONITOR_DEFAULTTONEAREST: u32 = 2;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

static OVERLAY_STATE: std::sync::OnceLock<Arc<OverlayState>> = std::sync::OnceLock::new();
static HOVERED: AtomicBool = AtomicBool::new(false);

fn state() -> &'static Arc<OverlayState> {
    OVERLAY_STATE.get_or_init(|| Arc::new(OverlayState::default()))
}

/// Is the video rectangle plausible for a browser video? Rejects the
/// zeroed / tiny / off-screen coordinates that used to make the button
/// flash at the top-left of the screen.
fn geometry_ok(x: i32, y: i32, w: i32, h: i32) -> bool {
    if w < MIN_VIDEO_W || h < MIN_VIDEO_H {
        return false;
    }
    unsafe {
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if vw <= 0 || vh <= 0 {
            return true; // no virtual-screen info: fall back to the size check
        }
        // Require a meaningful part of the video to be on-screen.
        let ix = (x + w).min(vx + vw) - x.max(vx);
        let iy = (y + h).min(vy + vh) - y.max(vy);
        ix > 0 && iy > 0 && (ix as i64 * iy as i64) * 4 >= (w as i64 * h as i64)
    }
}

/// Update the overlay with new video info from the browser extension.
pub fn update_overlay(
    url: &str,
    format: &str,
    qualities: Vec<(String, String)>,
    screen_x: i32,
    screen_y: i32,
    width: i32,
    height: i32,
) {
    // Never show the button for an empty URL or nonsense geometry.
    if url.trim().is_empty() || !geometry_ok(screen_x, screen_y, width, height) {
        crate::logfile::log(
            "OVERLAY",
            &format!(
                "ignored update: url_empty={} rect=({screen_x},{screen_y}) {width}x{height}",
                url.trim().is_empty()
            ),
        );
        hide_overlay();
        return;
    }

    let st = state();
    let was_visible = st.visible.load(Ordering::Relaxed);
    let video_changed = {
        let mut u = st.url.lock().unwrap();
        let mut f = st.format.lock().unwrap();
        let changed = *u != url || *f != format;
        *u = url.to_string();
        *f = format.to_string();
        changed
    };
    *st.qualities.lock().unwrap() = qualities;
    st.x.store(screen_x, Ordering::Relaxed);
    st.y.store(screen_y, Ordering::Relaxed);
    st.w.store(width, Ordering::Relaxed);
    st.h.store(height, Ordering::Relaxed);
    st.visible.store(true, Ordering::Relaxed);
    // A heartbeat for the same video must not close an open quality menu.
    if video_changed {
        st.menu_open.store(false, Ordering::Relaxed);
    }
    *st.last_feed.lock().unwrap() = Some(Instant::now());
    if !was_visible {
        crate::logfile::log(
            "OVERLAY",
            &format!("show candidate: {} ({format}) at ({screen_x},{screen_y}) {width}x{height}", url),
        );
    }

    // If the overlay thread hasn't started yet, start it; otherwise
    // re-evaluate visibility (the foreground app may have changed).
    if st.hwnd.load(Ordering::Relaxed) == 0 {
        spawn_overlay_thread();
    } else {
        let hwnd = st.hwnd.load(Ordering::Relaxed) as isize;
        refresh_window(hwnd, video_changed);
    }
}

/// Hide the overlay (when the page changes or the video goes away).
pub fn hide_overlay() {
    let st = state();
    let was_visible = st.visible.swap(false, Ordering::Relaxed);
    st.menu_open.store(false, Ordering::Relaxed);
    *st.last_feed.lock().unwrap() = None;
    st.url.lock().unwrap().clear();
    st.format.lock().unwrap().clear();
    st.qualities.lock().unwrap().clear();
    let hwnd = st.hwnd.load(Ordering::Relaxed) as isize;
    if hwnd != 0 {
        hide_window(hwnd);
    }
    if was_visible {
        crate::logfile::log("OVERLAY", "hidden");
    }
}

/// Called periodically by the GUI while it is alive. Hides the button as
/// soon as the heartbeat goes stale or the browser is no longer in front.
pub fn tick() {
    let st = state();
    let hwnd = st.hwnd.load(Ordering::Relaxed) as isize;
    if hwnd != 0 {
        refresh_window(hwnd, false);
    }
}

fn feed_is_fresh(st: &OverlayState) -> bool {
    st.last_feed
        .lock()
        .map(|g| g.map_or(false, |t| t.elapsed() <= STALE_AFTER))
        .unwrap_or(false)
}

/// The overlay may only be shown over a playing video that lives in the
/// foreground browser window.
fn should_show(st: &OverlayState) -> bool {
    if !st.visible.load(Ordering::Relaxed) {
        return false;
    }
    if !feed_is_fresh(st) {
        st.visible.store(false, Ordering::Relaxed);
        return false;
    }
    foreground_is_browser_window(st)
}

/// True when the foreground window belongs to a browser process AND still
/// contains the reported video rectangle. This is what stops the button
/// from appearing over the desktop, the DM Ultra window, or another app.
fn foreground_is_browser_window(st: &OverlayState) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg == 0 || IsIconic(fg) != 0 {
            return false;
        }
        if !window_is_browser(fg) {
            return false;
        }
        let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(fg, &mut wr) == 0 {
            return false;
        }
        let vx = st.x.load(Ordering::Relaxed);
        let vy = st.y.load(Ordering::Relaxed);
        let vw = st.w.load(Ordering::Relaxed);
        let vh = st.h.load(Ordering::Relaxed);
        let ix = (vx + vw).min(wr.right) - vx.max(wr.left);
        let iy = (vy + vh).min(wr.bottom) - vy.max(wr.top);
        // At least half of the video must still be inside the window that
        // has focus (guards against stale coordinates after a window move
        // or a background tab reporting while another window is in front).
        ix > 0
            && iy > 0
            && (ix as i64 * iy as i64) * 2 >= (vw as i64 * vh as i64).max(1)
    }
}

fn window_is_browser(hwnd: isize) -> bool {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return false;
        }
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h == 0 {
            return false;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok == 0 || len == 0 {
            return false;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        let exe = path
            .rsplit(|c| c == '\\' || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        BROWSER_EXES.contains(&exe.as_str())
    }
}

fn hide_window(hwnd: isize) {
    let st = state();
    if st.shown.swap(false, Ordering::Relaxed) {
        unsafe {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

/// Re-evaluate visibility and only touch the window when something
/// actually changed (prevents the repaint/ShowWindow flicker).
fn refresh_window(hwnd: isize, force_paint: bool) {
    let st = state();
    if should_show(st) {
        reposition_window(hwnd, force_paint);
    } else {
        hide_window(hwnd);
    }
}

fn monitor_work_area(x: i32, y: i32, w: i32, h: i32, height: i32) -> Option<RECT> {
    unsafe {
        let probe = RECT {
            left: x,
            top: y,
            right: x + w.max(1),
            bottom: y + h.max(1),
        };
        let mon = MonitorFromRect(&probe, MONITOR_DEFAULTTONEAREST);
        if mon == 0 {
            return None;
        }
        let mut mi = MONITORINFO {
            cb_size: std::mem::size_of::<MONITORINFO>() as u32,
            rc_monitor: RECT { left: 0, top: 0, right: 0, bottom: 0 },
            rc_work: RECT { left: 0, top: 0, right: 0, bottom: 0 },
            dw_flags: 0,
        };
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return None;
        }
        Some(mi.rc_work)
    }
}

fn reposition_window(hwnd: isize, force: bool) {
    let st = state();
    let vx = st.x.load(Ordering::Relaxed);
    let vy = st.y.load(Ordering::Relaxed);
    let vw = st.w.load(Ordering::Relaxed);
    let vh = st.h.load(Ordering::Relaxed);

    // Position the button at the top-right of the video, inset by 6px.
    let mut bx = vx + vw - BTN_W - 6;
    let mut by = vy + 6;

    let menu_open = st.menu_open.load(Ordering::Relaxed);
    let height = if menu_open {
        BTN_H + MENU_ITEM_H * st.qualities.lock().unwrap().len().min(8) as i32
    } else {
        BTN_H
    };
    let width = if menu_open { MENU_W } else { BTN_W };

    // Keep the button inside the work area of the monitor that holds the
    // video (multi-monitor safe; never clamp to a phantom (0, 0)).
    if let Some(work) = monitor_work_area(vx, vy, vw, vh, height) {
        bx = bx.max(work.left).min((work.right - width).max(work.left));
        by = by.max(work.top).min((work.bottom - height).max(work.top));
    } else {
        let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        bx = bx.max(0).min((screen_w - width).max(0));
        by = by.max(0).min((screen_h - height).max(0));
    }

    let shown = st.shown.load(Ordering::Relaxed);
    let same_pos = st.last_x.load(Ordering::Relaxed) == bx
        && st.last_y.load(Ordering::Relaxed) == by;
    let same_size = st.last_w.load(Ordering::Relaxed) == width
        && st.last_h.load(Ordering::Relaxed) == height;
    if shown && same_pos && same_size && !force {
        return; // nothing moved -> do not repaint (no flicker)
    }

    st.last_x.store(bx, Ordering::Relaxed);
    st.last_y.store(by, Ordering::Relaxed);
    st.last_w.store(width, Ordering::Relaxed);
    st.last_h.store(height, Ordering::Relaxed);

    unsafe {
        let flags = SWP_NOACTIVATE | if shown { 0 } else { SWP_SHOWWINDOW };
        SetWindowPos(hwnd, HWND_TOPMOST, bx, by, width, height, flags);
        if !shown {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            st.shown.store(true, Ordering::Relaxed);
        }
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}

unsafe extern "system" fn overlay_wndproc(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let st = state();
    match msg {
        WM_PAINT => {
            paint_window(hwnd, st);
            0
        }
        WM_MOUSEMOVE => {
            // Track mouse leave.
            let mut tme = TRACKMOUSEEVENT {
                cb_size: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dw_flags: TME_LEAVE,
                hwnd_track: hwnd,
                dw_hover_time: 0,
            };
            unsafe { TrackMouseEvent(&tme) };
            if !HOVERED.load(Ordering::Relaxed) {
                HOVERED.store(true, Ordering::Relaxed);
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            }
            0
        }
        WM_MOUSELEAVE => {
            HOVERED.store(false, Ordering::Relaxed);
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            0
        }
        WM_LBUTTONDOWN => {
            let x = (lparam as u32 & 0xFFFF) as i32;
            let y = ((lparam as u32) >> 16) as i32;
            let menu_open = st.menu_open.load(Ordering::Relaxed);
            if menu_open {
                // Menu is open: window is MENU_W wide.
                // Button sits at right edge: x in [MENU_W - BTN_W, MENU_W)
                // Menu items are below button: y >= BTN_H
                let btn_left = MENU_W - BTN_W;
                if y < BTN_H && x >= btn_left {
                    // Clicked the button area -> close menu.
                    st.menu_open.store(false, Ordering::Relaxed);
                    reposition_window(hwnd, true);
                } else if y >= BTN_H {
                    // Clicked a menu item.
                    let idx = (y - BTN_H) / MENU_ITEM_H;
                    let quals = st.qualities.lock().unwrap();
                    if idx >= 0 && (idx as usize) < quals.len() {
                        let (label, url) = &quals[idx as usize];
                        crate::logfile::log("OVERLAY", &format!("clicked: {label}"));
                        // Send this URL to the pending queue (like a browser click).
                        let _ = std::fs::create_dir_all(crate::state::state_dir().join("pending"));
                        let stamp = crate::state::unix_now() * 1_000_000_000;
                        let payload = serde_json::json!({"url": url, "referer": ""}).to_string();
                        let _ = std::fs::write(
                            crate::state::state_dir()
                                .join("pending")
                                .join(format!("{stamp}.json")),
                            payload,
                        );
                    }
                    st.menu_open.store(false, Ordering::Relaxed);
                    reposition_window(hwnd, true);
                }
                // Click elsewhere (menu background) -> ignore, keep menu open.
            } else {
                // Menu closed: button fills window (0..BTN_W).
                if x < BTN_W && y < BTN_H {
                    st.menu_open.store(true, Ordering::Relaxed);
                    reposition_window(hwnd, true);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            // Menu selection is handled in LBUTTONDOWN; just ensure the menu
            // stays open after a click on the button area.
            0
        }
        WM_TIMER => {
            // Re-check freshness and that a browser is still in front.
            refresh_window(hwnd, false);
            0
        }
        WM_DESTROY => {
            st.hwnd.store(0, Ordering::Relaxed);
            st.shown.store(false, Ordering::Relaxed);
            unsafe { DestroyWindow(hwnd) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn paint_window(hwnd: isize, st: &OverlayState) {
    unsafe {
        let hdc = GetDC(hwnd);
        if hdc == 0 {
            return;
        }

        let menu_open = st.menu_open.load(Ordering::Relaxed);
        let qual_count = st.qualities.lock().unwrap().len().min(8) as i32;
        let total_h = if menu_open {
            BTN_H + MENU_ITEM_H * qual_count
        } else {
            BTN_H
        };
        // When menu is open, window is MENU_W wide; button sits at right edge.
        let win_w = if menu_open { MENU_W } else { BTN_W };
        let btn_left = win_w - BTN_W;
        let btn_right = win_w;

        // Draw button face.
        let face_color = if HOVERED.load(Ordering::Relaxed) {
            COLOR_BTN_FACE_HOVER
        } else {
            COLOR_BTN_FACE
        };
        let brush = CreateSolidBrush(face_color);
        let btn_rc = RECT { left: btn_left, top: 0, right: btn_right, bottom: BTN_H };
        FillRect(hdc, &btn_rc, brush);
        DeleteObject(brush);

        // Draw IDM-style download icon: a small downward arrow with a line under it.
        // Centered in the 26x26 button area.
        let cx = btn_left + BTN_W / 2;
        let cy = BTN_H / 2;
        let arrow_color = COLOR_BTN_TEXT;

        // Create a pen for the arrow.
        #[link(name = "gdi32")]
        extern "system" {
            fn CreatePen(fn_pen_style: i32, n_width: i32, cr_color: u32) -> isize;
            fn MoveToEx(hdc: isize, x: i32, y: i32, lp_point: *mut POINT) -> i32;
            fn LineTo(hdc: isize, x: i32, y: i32) -> i32;
        }
        const PS_SOLID: i32 = 0;

        let pen = CreatePen(PS_SOLID, 2, arrow_color);
        let old_pen = SelectObject(hdc, pen);

        // Draw downward arrow: stem + arrowhead
        // Stem: from (cx, cy-4) to (cx, cy+2)
        MoveToEx(hdc, cx, cy - 4, std::ptr::null_mut());
        LineTo(hdc, cx, cy + 2);
        // Arrowhead left: (cx, cy+2) to (cx-4, cy-1)
        MoveToEx(hdc, cx, cy + 2, std::ptr::null_mut());
        LineTo(hdc, cx - 4, cy - 1);
        // Arrowhead right: (cx, cy+2) to (cx+4, cy-1)
        MoveToEx(hdc, cx, cy + 2, std::ptr::null_mut());
        LineTo(hdc, cx + 4, cy - 1);
        // Small line under arrow (the "download to" line)
        MoveToEx(hdc, cx - 5, cy + 5, std::ptr::null_mut());
        LineTo(hdc, cx + 5, cy + 5);

        SelectObject(hdc, old_pen);
        DeleteObject(pen);

        // Draw dropdown menu if open.
        if menu_open {
            let menu_brush = CreateSolidBrush(COLOR_MENU_FACE);
            for i in 0..qual_count {
                let item_rc = RECT {
                    left: 0,
                    top: BTN_H + i * MENU_ITEM_H,
                    right: MENU_W,
                    bottom: BTN_H + (i + 1) * MENU_ITEM_H,
                };
                FillRect(hdc, &item_rc, menu_brush);
            }
            DeleteObject(menu_brush);

            let quals = st.qualities.lock().unwrap();
            for (i, (label, _)) in quals.iter().take(8).enumerate() {
                let y = BTN_H + i as i32 * MENU_ITEM_H;
                let item_text: Vec<u16> =
                    label.encode_utf16().chain(std::iter::once(0)).collect();
                SetBkMode(hdc, TRANSPARENT_BK);
                SetTextColor(hdc, 0x001A1A2E); // dark text
                TextOutW(hdc, 10, y + (MENU_ITEM_H - 14) / 2, item_text.as_ptr(), (item_text.len() - 1) as i32);
            }
        }

        ReleaseDC(hwnd, hdc);
    }
}

fn spawn_overlay_thread() {
    std::thread::spawn(|| {
        unsafe {
            let h_instance = GetModuleHandleW(std::ptr::null());

            let class_name: Vec<u16> = "DMUVideoOverlay"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let window_name: Vec<u16> = "DMU Video Overlay"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();

            let wc = WNDCLASSEXW {
                cb_size: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: 0,
                lpfn_wnd_proc: Some(overlay_wndproc),
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance,
                h_icon: 0,
                h_cursor: 0,
                hbr_background: 0,
                lpsz_menu_name: std::ptr::null(),
                lpsz_class_name: class_name.as_ptr(),
                h_icon_sm: 0,
            };

            let atom = RegisterClassExW(&wc);
            if atom == 0 {
                crate::logfile::log("OVERLAY", "RegisterClassExW failed");
                return;
            }

            // Create initially hidden.
            let st = state();
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class_name.as_ptr(),
                window_name.as_ptr(),
                WS_POPUP,
                0, 0, BTN_W, BTN_H,
                0, 0, h_instance,
                std::ptr::null(),
            );
            if hwnd == 0 {
                crate::logfile::log("OVERLAY", "CreateWindowExW failed");
                return;
            }
            st.hwnd.store(hwnd as i32, Ordering::Relaxed);

            // Set semi-transparency for a modern look.
            let _ = SetLayeredWindowAttributes(hwnd, 0, 230, LWA_ALPHA);

            // Timer to periodically re-check visibility and reposition.
            // Note: we use a WM_TIMER via SetTimer which is in user32.
            #[link(name = "user32")]
            extern "system" {
                fn SetTimer(hwnd: isize, n_id_event: usize, u_elapse: u32, lp_timer_func: *const core::ffi::c_void) -> usize;
            }
            SetTimer(hwnd, 1, TIMER_MS, std::ptr::null());

            // Only show now if the data is fresh AND a browser is in front.
            refresh_window(hwnd, true);

            // Message loop.
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            loop {
                let r = GetMessageW(&mut msg, 0, 0, 0);
                if r <= 0 {
                    break;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
}
