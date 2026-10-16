//! A tiny background thread that owns the tray icon and the PrtSc keyboard hook.
//! It blocks in GetMessageW, so it costs nothing while idle.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;

use futures::channel::mpsc::UnboundedSender;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_SNAPSHOT};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallNextHookEx, ClipCursor, CreateIconIndirect, CreatePopupMenu, CreateWindowExW,
    DefWindowProcW, DestroyIcon, DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW,
    GetSystemMetrics, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    SetForegroundWindow, SetWindowsHookExW, TrackPopupMenu, TranslateMessage, HICON, ICONINFO,
    KBDLLHOOKSTRUCT, MF_SEPARATOR, MF_STRING, MSG, SM_CXSMICON, TPM_BOTTOMALIGN, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, WH_KEYBOARD_LL, WINDOW_EX_STYLE, WM_APP, WM_KEYDOWN, WM_LBUTTONUP,
    WM_RBUTTONUP, WM_SETTINGCHANGE, WM_SYSKEYDOWN, WNDCLASSW, WS_POPUP,
};

#[derive(Debug, Clone, Copy)]
pub enum AppEvent {
    Capture,
    Cancel,
    Quit,
}

/// True while the capture overlay is open; set through `lock`.
pub static CAPTURING: AtomicBool = AtomicBool::new(false);

static SENDER: OnceLock<UnboundedSender<AppEvent>> = OnceLock::new();
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

const WM_TRAY: u32 = WM_APP + 1;
const CMD_CAPTURE: usize = 1;
const CMD_QUIT: usize = 2;

pub fn start(tx: UnboundedSender<AppEvent>) {
    let _ = SENDER.set(tx);
    std::thread::Builder::new()
        .name("shoter-input".into())
        .spawn(|| unsafe { run() })
        .expect("spawn input thread");
}

fn send(ev: AppEvent) {
    if let Some(tx) = SENDER.get() {
        let _ = tx.unbounded_send(ev);
    }
}

unsafe fn run() {
    let Ok(hinst) = GetModuleHandleW(None) else { return };
    let class = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: hinst.into(),
        lpszClassName: w!("ShoterTray"),
        ..Default::default()
    };
    RegisterClassW(&class);
    // A hidden top-level window (not message-only) so it also hears "TaskbarCreated".
    let Ok(hwnd) = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("ShoterTray"),
        w!("Shoter"),
        WS_POPUP,
        0,
        0,
        0,
        0,
        None,
        None,
        Some(hinst.into()),
        None,
    ) else {
        return;
    };
    TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
    tray(hwnd, NIM_ADD);

    let _hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(hinst.into()), 0);

    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let k = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let down = wparam.0 as u32 == WM_KEYDOWN || wparam.0 as u32 == WM_SYSKEYDOWN;
        if k.vkCode == VK_SNAPSHOT.0 as u32 {
            if down {
                send(AppEvent::Capture);
            }
            // Swallow both edges so Windows' own PrtSc action doesn't also run.
            return LRESULT(1);
        }
        // While capturing, the keyboard belongs to the capture: no Alt+Tab, Win, shortcuts or
        // typing into the app behind. Esc cancels. A key-up is only swallowed if we swallowed its
        // key-down, so keys held when the capture started are never left stuck.
        let vk = (k.vkCode & 0xff) as usize;
        let bit = 1u64 << (vk % 64);
        if down && CAPTURING.load(Ordering::Relaxed) {
            if vk == VK_ESCAPE.0 as usize {
                send(AppEvent::Cancel);
            }
            HELD[vk / 64].fetch_or(bit, Ordering::Relaxed);
            return LRESULT(1);
        }
        if !down && HELD[vk / 64].fetch_and(!bit, Ordering::Relaxed) & bit != 0 {
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// Keys whose key-down the capture swallowed, so their key-up is swallowed too.
static HELD: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// Locks input to the capture: the keyboard (see `keyboard_proc`) and the cursor, which can't
/// leave the captured monitor. `None` releases both.
pub fn lock(area: Option<RECT>) {
    CAPTURING.store(area.is_some(), Ordering::Relaxed);
    unsafe {
        let _ = ClipCursor(area.as_ref().map(|r| r as *const RECT));
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_TRAY {
        match lparam.0 as u32 {
            WM_LBUTTONUP => send(AppEvent::Capture),
            WM_RBUTTONUP => menu(hwnd),
            _ => {}
        }
        return LRESULT(0);
    }
    if msg == WM_SETTINGCHANGE {
        // Taskbar theme may have flipped; redraw the icon in the right color.
        tray(hwnd, NIM_MODIFY);
    }
    let created = TASKBAR_CREATED.load(Ordering::Relaxed);
    if created != 0 && msg == created {
        tray(hwnd, NIM_ADD);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe fn menu(hwnd: HWND) {
    let Ok(m) = CreatePopupMenu() else { return };
    let _ = AppendMenuW(m, MF_STRING, CMD_CAPTURE, w!("Capture\tPrtSc"));
    let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(m, MF_STRING, CMD_QUIT, w!("Quit Shoter"));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(
        m,
        TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
        pt.x,
        pt.y,
        None,
        hwnd,
        None,
    );
    let _ = DestroyMenu(m);
    match cmd.0 as usize {
        CMD_CAPTURE => send(AppEvent::Capture),
        CMD_QUIT => {
            tray(hwnd, NIM_DELETE);
            send(AppEvent::Quit);
            PostQuitMessage(0);
        }
        _ => {}
    }
}

unsafe fn tray(hwnd: HWND, op: windows::Win32::UI::Shell::NOTIFY_ICON_MESSAGE) {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        ..Default::default()
    };
    let tip: Vec<u16> = "Shoter · PrtSc to capture".encode_utf16().collect();
    nid.szTip[..tip.len()].copy_from_slice(&tip);
    let icon = if op == NIM_DELETE { None } else { make_icon(light_taskbar()) };
    if let Some(icon) = icon {
        nid.hIcon = icon;
    }
    let _ = Shell_NotifyIconW(op, &nid);
    if let Some(icon) = icon {
        let _ = DestroyIcon(icon);
    }
}

fn light_taskbar() -> bool {
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
        .is_ok()
            && value == 1
    }
}

/// Draws the Nest mark (thin square frame around a solid square) as a tray icon.
unsafe fn make_icon(light: bool) -> Option<HICON> {
    let s = GetSystemMetrics(SM_CXSMICON).max(16);
    let color: [u8; 4] = if light { [0x11, 0x11, 0x11, 0xff] } else { [0xff, 0xff, 0xff, 0xff] };
    let stroke = (s as f32 / 16.0).round().max(1.0) as i32;
    let inset = (s as f32 * 11.0 / 64.0).round() as i32 + stroke / 2;
    let mut px = vec![0u8; (s * s * 4) as usize];
    for y in 0..s {
        for x in 0..s {
            let frame = x < stroke || y < stroke || x >= s - stroke || y >= s - stroke;
            let core = x >= inset && y >= inset && x < s - inset && y < s - inset;
            if frame || core {
                let i = ((y * s + x) * 4) as usize;
                px[i] = color[2];
                px[i + 1] = color[1];
                px[i + 2] = color[0];
                px[i + 3] = color[3];
            }
        }
    }
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: s,
            biHeight: -s,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let color_bmp = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
    std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());
    let mask_bits = vec![0u8; ((s + 15) / 16 * 2 * s) as usize];
    let mask = CreateBitmap(s, s, 1, 1, Some(mask_bits.as_ptr() as *const c_void));
    let info = ICONINFO {
        fIcon: true.into(),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: color_bmp,
    };
    let icon = CreateIconIndirect(&info).ok();
    let _ = DeleteObject(color_bmp.into());
    let _ = DeleteObject(mask.into());
    icon
}
