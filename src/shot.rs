use std::sync::Arc;

use gpui::RenderImage;

/// A captured image. The pixels live once, inside the `RenderImage` GPUI uploads from; every
/// window and the exporter read that same buffer.
pub struct Shot {
    pub width: u32,
    pub height: u32,
    /// BGRA (what GPUI's renderer expects), opaque, top-down rows.
    pub image: Arc<RenderImage>,
    /// Display scale of the monitor it came from (1.0 = 96 DPI).
    pub scale: f32,
    /// What was captured, e.g. "Button" or "Region".
    pub kind: &'static str,
}

impl Shot {
    pub fn new(width: u32, height: u32, bgra: Vec<u8>, scale: f32, kind: &'static str) -> Shot {
        let buffer = image::RgbaImage::from_raw(width, height, bgra).expect("buffer size matches dimensions");
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
        Shot { width, height, image, scale, kind }
    }

    pub fn bgra(&self) -> &[u8] {
        self.image.as_bytes(0).unwrap_or_default()
    }

    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32, kind: &'static str) -> Shot {
        let x = x.min(self.width.saturating_sub(1));
        let y = y.min(self.height.saturating_sub(1));
        let w = w.clamp(1, self.width - x);
        let h = h.clamp(1, self.height - y);
        let stride = self.width as usize * 4;
        let src = self.bgra();
        let mut bgra = Vec::with_capacity(w as usize * h as usize * 4);
        for row in y..y + h {
            let start = row as usize * stride + x as usize * 4;
            bgra.extend_from_slice(&src[start..start + w as usize * 4]);
        }
        Shot::new(w, h, bgra, self.scale, kind)
    }

    pub fn dims(&self) -> String {
        format!("{} × {}", self.width, self.height)
    }
}
