//! Win32 window helpers: placement, z-order, visibility, DWM attributes and edge snapping.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::Window;
use player_core::geometry::Rect;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle, Win32WindowHandle,
    WindowHandle,
};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMSBT_NONE, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWINDOWATTRIBUTE,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CreateRectRgn, DeleteObject, GetMonitorInfoW, GetWindowRgnBox, HMONITOR,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, MonitorFromWindow, SetWindowRgn,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowRect, HTCLIENT, HTTOP, HWND_NOTOPMOST, HWND_TOPMOST, IsIconic, IsWindowVisible,
    SW_HIDE, SW_RESTORE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow,
    SetWindowPos, ShowWindow, WM_MOVING, WM_NCHITTEST, WM_SIZE,
};

/// Snap distance in logical pixels, as in the original mini player.
const SNAP_THRESHOLD: i32 = 20;

/// Whether dragging the window snaps it to monitor work-area edges (mini mode only).
static EDGE_SNAPPING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy)]
pub struct NativeWindow(isize);

impl NativeWindow {
    pub const SUPPORTS_PLACEMENT: bool = true;

    pub fn from_window(window: &Window) -> Option<Self> {
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(Self(handle.hwnd.get())),
            _ => None,
        }
    }

    fn hwnd(self) -> HWND {
        HWND(self.0 as *mut _)
    }

    /// The raw window handle, for system APIs tied to a window (media controls).
    pub fn raw_handle(self) -> Option<*mut std::ffi::c_void> {
        Some(self.0 as *mut _)
    }

    /// The drawable area in physical screen coordinates.
    pub fn client_rect(self) -> Option<Rect> {
        client_rect(self.hwnd())
    }

    /// Bounds of the monitor the window is on, in physical pixels.
    pub fn monitor_rect(self) -> Option<Rect> {
        unsafe { monitor_rect(MonitorFromWindow(self.hwnd(), MONITOR_DEFAULTTONEAREST)) }
    }

    /// Moves and resizes the window so its drawable area matches `rect` exactly.
    pub fn set_client_rect(self, rect: Rect) {
        let hwnd = self.hwnd();
        let Some((left, top, right, bottom)) = frame_insets(hwnd) else { return };
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                None,
                rect.x - left,
                rect.y - top,
                rect.width + left + right,
                rect.height + top + bottom,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    pub fn set_topmost(self, topmost: bool) {
        let after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
        unsafe {
            let _ =
                SetWindowPos(self.hwnd(), Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    pub fn show(self) {
        let hwnd = self.hwnd();
        unsafe {
            let _ = ShowWindow(hwnd, if IsIconic(hwnd).as_bool() { SW_RESTORE } else { SW_SHOW });
            let _ = SetForegroundWindow(hwnd);
        }
    }

    pub fn hide(self) {
        unsafe {
            let _ = ShowWindow(self.hwnd(), SW_HIDE);
        }
    }

    pub fn is_visible(self) -> bool {
        unsafe { IsWindowVisible(self.hwnd()).as_bool() && !IsIconic(self.hwnd()).as_bool() }
    }

    /// Turns off the system corner rounding, border and backdrop (the app draws its own
    /// rounded surface) and matches the dark-mode hint to the theme.
    pub fn apply_chrome(self, dark: bool) {
        let hwnd = self.hwnd();
        unsafe {
            set_attribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &DWMWCP_DONOTROUND);
            set_attribute(hwnd, DWMWA_BORDER_COLOR, &COLORREF(DWMWA_COLOR_NONE));
            set_attribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &DWMSBT_NONE);
            set_attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &windows::core::BOOL::from(dark));
        }
    }

    /// Installs the window subclass that adjusts hit testing, clipping and edge snapping.
    pub fn install_hooks(self) {
        unsafe {
            let _ = SetWindowSubclass(self.hwnd(), Some(subclass_proc), 1, 0);
        }
        clip_to_client(self.hwnd());
    }

    pub fn set_edge_snapping(enabled: bool) {
        EDGE_SNAPPING.store(enabled, Ordering::Relaxed);
    }

    /// An owner handle for native dialogs (e.g. rfd's `set_parent`).
    ///
    /// GPUI's `Window` can't be used for that directly: its `display_handle()` is
    /// `unimplemented!()` on Windows, and the resulting panic inside the window procedure
    /// aborts the process.
    pub fn dialog_parent(self) -> DialogParent {
        DialogParent(self.0)
    }
}

pub struct DialogParent(isize);

impl HasWindowHandle for DialogParent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = std::num::NonZeroIsize::new(self.0).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(hwnd));
        // SAFETY: the HWND belongs to the app's main window, which outlives any dialog.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for DialogParent {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
}

unsafe fn set_attribute<T>(hwnd: HWND, attribute: DWMWINDOWATTRIBUTE, value: &T) {
    let _ = unsafe {
        DwmSetWindowAttribute(hwnd, attribute, value as *const T as *const _, size_of::<T>() as u32)
    };
}

fn client_rect(hwnd: HWND) -> Option<Rect> {
    unsafe {
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client).ok()?;
        let mut origin = POINT::default();
        if !ClientToScreen(hwnd, &mut origin).as_bool() {
            return None;
        }
        Some(Rect::new(origin.x, origin.y, client.right - client.left, client.bottom - client.top))
    }
}

/// Distance from each edge of the window rect to the client rect (GPUI keeps an
/// invisible resize frame around frameless windows).
fn frame_insets(hwnd: HWND) -> Option<(i32, i32, i32, i32)> {
    let client = client_rect(hwnd)?;
    let mut window = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut window).ok()? };
    Some((
        client.x - window.left,
        client.y - window.top,
        window.right - client.right(),
        window.bottom - client.bottom(),
    ))
}

/// Restricts the window to its client area. GPUI leaves an invisible resize frame (and on
/// Windows 11 a 1px top strip that DWM paints) around frameless windows; clipping them
/// away also keeps clicks there from landing on the window.
fn clip_to_client(hwnd: HWND) {
    let (Some((left, top, _, _)), Some(client)) = (frame_insets(hwnd), client_rect(hwnd)) else {
        return;
    };
    let desired = RECT { left, top, right: left + client.width, bottom: top + client.height };
    unsafe {
        let mut current = RECT::default();
        if GetWindowRgnBox(hwnd, &mut current).0 != 0 && current == desired {
            return;
        }
        let region = CreateRectRgn(desired.left, desired.top, desired.right, desired.bottom);
        if SetWindowRgn(hwnd, Some(region), true) == 0 {
            let _ = DeleteObject(region.into());
        }
    }
}

fn to_rect(rect: &RECT) -> Rect {
    Rect::new(rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top)
}

fn monitor_info(monitor: HMONITOR) -> Option<MONITORINFO> {
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool().then_some(info)
}

fn monitor_rect(monitor: HMONITOR) -> Option<Rect> {
    monitor_info(monitor).map(|info| to_rect(&info.rcMonitor))
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        // GPUI reports the top few pixels of a frameless window as a resize edge. This window
        // is not resizable, so treat that strip as ordinary client area.
        WM_NCHITTEST => {
            let hit = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            if hit.0 == HTTOP as isize { LRESULT(HTCLIENT as isize) } else { hit }
        }
        WM_SIZE => {
            let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            clip_to_client(hwnd);
            result
        }
        WM_MOVING if EDGE_SNAPPING.load(Ordering::Relaxed) => {
            let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            if let Some(moving) = unsafe { (lparam.0 as *mut RECT).as_mut() } {
                snap_moving_rect(hwnd, moving);
            }
            result
        }
        _ => unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) },
    }
}

/// Adjusts the proposed window rect so the visible client area hugs nearby work-area edges.
fn snap_moving_rect(hwnd: HWND, moving: &mut RECT) {
    let Some((left, top, right, bottom)) = frame_insets(hwnd) else { return };
    let client = Rect::new(
        moving.left + left,
        moving.top + top,
        moving.right - moving.left - left - right,
        moving.bottom - moving.top - top - bottom,
    );
    let Some(info) = monitor_info(unsafe { MonitorFromRect(moving, MONITOR_DEFAULTTONEAREST) }) else {
        return;
    };
    let work = to_rect(&info.rcWork);
    let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
    let snapped = client.snapped_to(&work, (SNAP_THRESHOLD as f32 * scale).round() as i32);
    let (dx, dy) = (snapped.x - client.x, snapped.y - client.y);
    moving.left += dx;
    moving.right += dx;
    moving.top += dy;
    moving.bottom += dy;
}
