use windows::core::w;
use windows::Win32::Foundation::{HANDLE, HGLOBAL};
use windows::Win32::Graphics::Gdi::{BITMAPINFOHEADER, BI_RGB};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_DIB;

/// Puts an RGBA image on the clipboard as both a DIB (every app) and PNG (keeps alpha).
pub fn set_image(w: u32, h: u32, rgba: &[u8]) -> bool {
    let png = crate::compose::encode_png(w, h, rgba);
    unsafe {
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(None).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        if !opened {
            return false;
        }
        let _ = EmptyClipboard();

        // CF_DIB: header followed by bottom-up BGRA rows.
        let header = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: h as i32,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: w * h * 4,
            ..Default::default()
        };
        let header_len = std::mem::size_of::<BITMAPINFOHEADER>();
        if let Some(mem) = alloc(header_len + (w * h * 4) as usize) {
            let p = GlobalLock(mem) as *mut u8;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(&header as *const _ as *const u8, p, header_len);
                let stride = w as usize * 4;
                for y in 0..h as usize {
                    let src = &rgba[(h as usize - 1 - y) * stride..][..stride];
                    let dst = std::slice::from_raw_parts_mut(p.add(header_len + y * stride), stride);
                    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                        d[0] = s[2];
                        d[1] = s[1];
                        d[2] = s[0];
                        d[3] = s[3];
                    }
                }
                let _ = GlobalUnlock(mem);
                let _ = SetClipboardData(CF_DIB.0 as u32, Some(HANDLE(mem.0)));
            }
        }

        let png_format = RegisterClipboardFormatW(w!("PNG"));
        if png_format != 0 {
            if let Some(mem) = alloc(png.len()) {
                let p = GlobalLock(mem) as *mut u8;
                if !p.is_null() {
                    std::ptr::copy_nonoverlapping(png.as_ptr(), p, png.len());
                    let _ = GlobalUnlock(mem);
                    let _ = SetClipboardData(png_format, Some(HANDLE(mem.0)));
                }
            }
        }
        let _ = CloseClipboard();
    }
    true
}

unsafe fn alloc(len: usize) -> Option<HGLOBAL> {
    GlobalAlloc(GMEM_MOVEABLE, len).ok()
}
