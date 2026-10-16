//! Thin Win32 layer: everything that talks to the OS directly lives here.

pub mod capture;
pub mod clipboard;
pub mod input;
pub mod uia;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::w;
use std::ffi::c_void;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_DONOTROUND,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, SetProcessWorkingSetSize};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowLongPtrW, GetWindowRect, IsZoomed, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, ShowWindowAsync,
    GWL_EXSTYLE, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, SW_MAXIMIZE, SW_RESTORE,
    WS_EX_NOACTIVATE, WS_EX_TOPMOST,
};

/// Returns false when another Shoter is already running.
pub fn single_instance() -> bool {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // The handle is intentionally leaked: it must live as long as the process.
        let _ = CreateMutexW(None, true, w!("Local\\ShoterSingleInstance"));
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// Drops every page Shoter isn't touching right now out of its working set. Pages fault back
/// in on demand, so the next capture pays a few milliseconds instead of idling at peak RAM.
pub fn trim_memory() {
    unsafe {
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

pub fn hwnd_of(window: &gpui::Window) -> Option<HWND> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut _)),
        _ => None,
    }
}

/// Maximizes the window, or restores it to its previous size and place if it already is
/// (gpui's `zoom_window` only ever maximizes on Windows).
pub fn toggle_maximize(window: &gpui::Window) {
    if let Some(hwnd) = hwnd_of(window) {
        unsafe {
            // Async, like gpui's own zoom: a synchronous resize inside its click handler is missed
            // by gpui, which then keeps laying out at the old size.
            let _ = ShowWindowAsync(hwnd, if IsZoomed(hwnd).as_bool() { SW_RESTORE } else { SW_MAXIMIZE });
        }
    }
}

/// Windows 11 rounds popup windows and draws a 1px outline around them; our windows draw their
/// own shape (or must cover the screen edge to edge), so both are turned off.
unsafe fn bare_frame(hwnd: HWND) {
    let corner = DWMWCP_DONOTROUND;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &corner as *const _ as *const c_void,
        std::mem::size_of_val(&corner) as u32,
    );
    let none = DWMWA_COLOR_NONE;
    let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &none as *const _ as *const c_void, 4);
}

/// Covers `rect` (physical pixels) and stays above everything else.
pub fn place_topmost(window: &gpui::Window, rect: RECT, activate: bool) {
    let Some(hwnd) = hwnd_of(window) else { return };
    unsafe {
        bare_frame(hwnd);
        let mut flags = SWP_SHOWWINDOW;
        if !activate {
            flags |= SWP_NOACTIVATE;
        }
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), rect.left, rect.top, w, h, flags);
        // gpui windows keep an invisible resize frame around the drawable area. Grow the window
        // by it so the drawable area itself covers `rect`; otherwise the frozen screen is drawn
        // a few pixels squeezed and the highlight drifts off the element it marks.
        let (mut wr, mut cr, mut o) = (RECT::default(), RECT::default(), POINT::default());
        if GetWindowRect(hwnd, &mut wr).is_ok()
            && GetClientRect(hwnd, &mut cr).is_ok()
            && ClientToScreen(hwnd, &mut o).as_bool()
        {
            let (l, t) = (o.x - wr.left, o.y - wr.top);
            let (r, b) = (wr.right - o.x - cr.right, wr.bottom - o.y - cr.bottom);
            if l | t | r | b != 0 {
                let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), rect.left - l, rect.top - t, w + l + r, h + t + b, flags);
            }
        }
        if activate {
            force_foreground(hwnd);
        }
    }
}

/// Floating, never-activated window (the corner thumbnail).
pub fn make_floating(window: &gpui::Window) {
    let Some(hwnd) = hwnd_of(window) else { return };
    unsafe {
        bare_frame(hwnd);
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            ex | WS_EX_NOACTIVATE.0 as isize | WS_EX_TOPMOST.0 as isize,
        );
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

pub fn hide(window: &gpui::Window) {
    if let Some(hwnd) = hwnd_of(window) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

/// Windows only lets the foreground app hand over focus; a synthetic Alt tap lifts the lock.
unsafe fn force_foreground(hwnd: HWND) {
    keybd_event(VK_MENU.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
    keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
    let _ = SetForegroundWindow(hwnd);
}

pub fn save_dir() -> std::path::PathBuf {
    let base = std::env::var_os("USERPROFILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("Pictures").join("Shoter")
}

pub fn timestamp() -> String {
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}
