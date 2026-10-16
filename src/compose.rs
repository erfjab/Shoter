//! CPU compositing for export: the gradient backdrop, its blur and grain under the padding, the
//! framed shot (edge, window header, rounded corners) and its shadow. The editor's preview draws
//! the same thing with the same numbers; this runs on a background thread.

use image::RgbaImage;

use crate::shot::Shot;

/// Backdrop gradients run along this CSS-style angle (degrees, 0 = to top, clockwise).
pub const ANGLE: f32 = 135.0;


/// The shot's shadow, in logical pixels.
#[derive(Clone, Copy, PartialEq)]
pub struct Shadow {
    /// Which way the shadow falls, in degrees (0 = right, 90 = down), and how far.
    pub angle: f32,
    pub size: f32,
    pub blur: f32,
    pub opacity: f32,
}

impl Shadow {
    /// The shadow's offset from the shot, in logical pixels.
    pub fn offset(&self) -> (f32, f32) {
        let a = self.angle.to_radians();
        (a.cos() * self.size, a.sin() * self.size)
    }
}

#[derive(Clone, Copy, PartialEq)]
pub struct Style {
    /// The backdrop: sRGB stops spread evenly along `ANGLE`.
    pub gradient: &'static [u32],
    /// Padding in output pixels.
    pub padding: u32,
    /// Corner radius in output pixels.
    pub radius: f32,
    /// Exact output size in pixels; None keeps the shot at its own size plus padding.
    pub size: Option<(u32, u32)>,
    /// Frame edge around the shot, an index into `EDGES`, and its width in logical pixels.
    pub edge: usize,
    pub edge_width: f32,
    /// Window header drawn on top of the shot, an index into `HEADERS`.
    pub header: usize,
    pub shadow: Shadow,
    /// Capture scale, to turn logical sizes into output pixels.
    pub scale: f32,
    /// How far the gradient melts into a soft mesh (0 plain ..= 1 most diffuse). Never touches the shot.
    pub blur: f32,
    /// Film grain over the backdrop (0..1).
    pub noise: f32,
    /// Where the shot sits: its shift from the centre as a fraction of the page (0 = centred).
    pub offset: (f32, f32),
    /// Extra scale of the shot inside the page (1 = fitted).
    pub zoom: f32,
}

pub struct Edge {
    pub name: &'static str,
    /// Logical pixels, drawn outside the shot; where the stroke bar starts for this look.
    pub width: f32,
    pub color: u32,
    pub alpha: f32,
}

pub const EDGES: [Edge; 8] = [
    Edge { name: "None", width: 0.0, color: 0x000000, alpha: 0.0 },
    Edge { name: "Hairline", width: 1.0, color: 0x000000, alpha: 0.22 },
    Edge { name: "Frost", width: 7.0, color: 0xffffff, alpha: 0.32 },
    Edge { name: "Smoke", width: 7.0, color: 0x000000, alpha: 0.35 },
    Edge { name: "Ink", width: 3.0, color: 0x0f0f0f, alpha: 1.0 },
    Edge { name: "Paper", width: 12.0, color: 0xffffff, alpha: 1.0 },
    Edge { name: "Silver", width: 4.0, color: 0xc7c7cc, alpha: 1.0 },
    Edge { name: "Ocean", width: 4.0, color: 0x2f6fed, alpha: 1.0 },
];

/// Window chrome drawn above the shot.
pub struct Header {
    pub name: &'static str,
    pub bar: u32,
    /// macOS traffic lights on the left.
    pub lights: bool,
    /// Windows minimize / maximize / close on the right, in this colour.
    pub controls: Option<u32>,
    /// A browser address pill in the middle, in this colour.
    pub address: Option<u32>,
}

pub const HEADERS: [Header; 5] = [
    Header { name: "None", bar: 0, lights: false, controls: None, address: None },
    Header { name: "macOS", bar: 0xececee, lights: true, controls: None, address: None },
    Header { name: "Windows", bar: 0xf3f3f3, lights: false, controls: Some(0x4a4a4a), address: None },
    Header { name: "Browser", bar: 0xe6e6e9, lights: true, controls: None, address: Some(0xffffff) },
    Header { name: "Browser dark", bar: 0x2a2a2d, lights: true, controls: None, address: Some(0x3c3c40) },
];

/// Header height in logical pixels, and the macOS light colours.
pub const HEADER_H: f32 = 30.0;
pub const LIGHTS: [u32; 3] = [0xff5f57, 0xfebc2e, 0x28c840];

impl Style {
    /// Edge width in output pixels.
    pub fn ring(&self) -> f32 {
        if self.edge == 0 {
            0.0
        } else {
            (self.edge_width * self.scale).round()
        }
    }

    /// Header height in output pixels (before fitting).
    pub fn header_h(&self) -> f32 {
        if self.header == 0 {
            0.0
        } else {
            (HEADER_H * self.scale).round()
        }
    }
}

/// The same grain everywhere: a cheap hash of the pixel position.
pub fn grain(x: u32, y: u32) -> u8 {
    let mut h = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    (h >> 24) as u8
}

/// Opacity of the grain at full strength.
pub const GRAIN_ALPHA: f32 = 0.2;

/// The backdrop alone at `w`×`h`, RGBA. At blur 0 it is the plain gradient. Blur blends it into
/// a soft mesh: each stop becomes two large glows set across the gradient's direction, mixed by
/// distance, and more blur makes them wider and more diffuse. Every pixel is a weighted average
/// of the stops, so nothing ever darkens or drifts off-palette; a sub-level dither keeps the
/// wide, slow blends free of banding. It depends only on the page's shape, so the preview and
/// the export match.
pub fn backdrop(stops: &[u32], w: u32, h: u32, blur: f32) -> RgbaImage {
    let (wf, hf) = (w.max(1) as f32, h.max(1) as f32);
    let a = ANGLE.to_radians();
    let (dx, dy) = (a.sin(), -a.cos());
    let (qx, qy) = (-dy, dx);
    let len = (wf * dx.abs() + hf * dy.abs()).max(1.0);
    let across = wf * qx.abs() + hf * qy.abs();
    let (cx, cy) = (wf / 2.0, hf / 2.0);
    let cols: Vec<[f32; 3]> = stops.iter().map(|&c| unpack(c)).collect();
    let last = cols.len().saturating_sub(1).max(1) as f32;
    let blur = blur.clamp(0.0, 1.0);
    // Two glows per stop, at the stop's place along the gradient, pushed to either side of the
    // axis and nudged along it in opposite directions, so the blend swirls a little.
    let glows: Vec<(f32, f32, [f32; 3])> = cols
        .iter()
        .enumerate()
        .flat_map(|(i, &c)| {
            let t = (i as f32 / last - 0.5) * len * 0.9;
            [(-1.0f32, 1.0f32), (1.0, -1.0)].map(|(side, nudge)| {
                let turn = if i % 2 == 0 { 1.0 } else { -1.0 };
                let (along, off) = (t + nudge * turn * 0.12 * len, side * 0.28 * across);
                (cx + dx * along + qx * off, cy + dy * along + qy * off, c)
            })
        })
        .collect();
    let spread = (0.16 + 0.34 * blur) * wf.max(hf);
    let k = -0.5 / (spread * spread);
    RgbaImage::from_fn(w.max(1), h.max(1), |x, y| {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let t = (((px - cx) * dx + (py - cy) * dy) / len + 0.5).clamp(0.0, 1.0) * last;
        let i = (t as usize).min(cols.len().saturating_sub(2));
        let f = t - i as f32;
        let (c0, c1) = (cols[i], cols[(i + 1).min(cols.len() - 1)]);
        let mut rgb = [0.0f32; 3];
        for ch in 0..3 {
            rgb[ch] = c0[ch] + (c1[ch] - c0[ch]) * f;
        }
        if blur > 0.0 {
            let mut mesh = [0.0f32; 3];
            let mut sum = 0.0;
            for &(gx, gy, c) in &glows {
                let wgt = (((px - gx).powi(2) + (py - gy).powi(2)) * k).exp().max(1e-12);
                sum += wgt;
                for ch in 0..3 {
                    mesh[ch] += c[ch] * wgt;
                }
            }
            for ch in 0..3 {
                rgb[ch] += (mesh[ch] / sum - rgb[ch]) * blur;
            }
        }
        let d = grain(x, y) as f32 / 255.0 - 0.5;
        let px = |v: f32| (v + d).round().clamp(0.0, 255.0) as u8;
        image::Rgba([px(rgb[0]), px(rgb[1]), px(rgb[2]), 255])
    })
}

/// The page size: the chosen size, or the shot plus padding. Edge, header, radius and shadow
/// never change it; the framed shot is fitted inside instead.
pub fn output_size(w: u32, h: u32, style: &Style) -> (u32, u32) {
    let pad = 2 * style.padding;
    style.size.unwrap_or((w + pad, h + pad))
}

/// How much the shot is scaled to sit inside the page with its padding (1.0 at its own size),
/// times the extra zoom. It depends on the shot and the page only: edge, header, radius and
/// shadow are drawn around the shot at that size, in the padding, and never resize it. The
/// preview and the export both use it.
pub fn fit(w: u32, h: u32, style: &Style) -> f32 {
    let (ow, oh) = output_size(w, h, style);
    let pad = 2.0 * style.padding as f32;
    ((ow as f32 - pad).max(1.0) / w.max(1) as f32).min((oh as f32 - pad).max(1.0) / h.max(1) as f32).max(0.01) * style.zoom
}

/// Where a block `w` wide goes in a page `ow` wide (one axis).
pub fn place(ow: f32, w: f32, offset: f32) -> f32 {
    // Centred, then shifted by `offset` pages, so the shot can be dragged anywhere, even
    // off the page.
    (ow - w) / 2.0 + offset * ow
}

/// Composites `shot` with `style` into a straight-alpha RGBA buffer.
pub fn compose(shot: &Shot, style: &Style) -> (u32, u32, Vec<u8>) {
    let (ow, oh) = output_size(shot.width, shot.height, style);
    // Fitting an exact size scales the shot and its frame together; Lanczos keeps text crisp.
    let f = fit(shot.width, shot.height, style);
    let (sw, sh) = (((shot.width as f32 * f).round() as u32).max(1), ((shot.height as f32 * f).round() as u32).max(1));
    let resized;
    let px: &[u8] = if (sw, sh) == (shot.width, shot.height) {
        shot.bgra()
    } else {
        let src = RgbaImage::from_raw(shot.width, shot.height, shot.bgra().to_vec()).expect("buffer size");
        resized = image::imageops::resize(&src, sw, sh, image::imageops::FilterType::Lanczos3).into_raw();
        &resized
    };
    let scale = shot.scale * f;
    let hh = (style.header_h() * f).round() as i64;
    let (w, h) = (sw as i64, sh as i64);
    let bh = h + hh;
    // The block is the header plus the shot; it is what gets placed, edged and shadowed.
    let ex = place(ow as f32, w as f32, style.offset.0).round() as i64;
    let ey = place(oh as f32, bh as f32, style.offset.1).round() as i64;
    let mut out = backdrop(style.gradient, ow, oh, style.blur).into_raw();

    // Grain over the backdrop, under the padding.
    if style.noise > 0.0 {
        let a = style.noise * GRAIN_ALPHA;
        for y in 0..oh {
            for x in 0..ow {
                let v = grain(x, y);
                let di = (y as usize * ow as usize + x as usize) * 4;
                over(&mut out[di..di + 4], [v, v, v, 255], a);
            }
        }
    }

    let rw = (style.ring() * f).round() as i64;
    let r = (style.radius * f).min(w.min(bh) as f32 / 2.0);
    let ro = if r > 0.0 { r + rw as f32 } else { 0.0 };
    let (fx, fy, fw, fh) = (ex - rw, ey - rw, w + 2 * rw, bh + 2 * rw);

    let s = style.shadow;
    if s.opacity > 0.0 {
        let (sx, sy) = s.offset();
        draw_shadow(&mut out, ow, oh, fx + (sx * scale).round() as i64, fy + (sy * scale).round() as i64, fw, fh, ro, (s.blur * scale / 2.0).max(0.5), s.opacity, 0x000000);
    }

    let edge = &EDGES[style.edge];
    if rw > 0 {
        let c = edge.color;
        let src = [(c >> 16) as u8, (c >> 8) as u8, c as u8, 255];
        for y in 0..fh {
            for x in 0..fw {
                let (oxp, oyp) = (fx + x, fy + y);
                if oxp < 0 || oyp < 0 || oxp >= ow as i64 || oyp >= oh as i64 {
                    continue;
                }
                let cov = rounded_coverage(x as f32 + 0.5, y as f32 + 0.5, fw as f32, fh as f32, ro);
                if cov > 0.0 {
                    let di = (oyp as usize * ow as usize + oxp as usize) * 4;
                    over(&mut out[di..di + 4], src, cov * edge.alpha);
                }
            }
        }
    }

    // The header, sharing the block's rounded top corners.
    if hh > 0 {
        let head = &HEADERS[style.header];
        let bar = rgb(head.bar);
        for y in 0..hh {
            for x in 0..w {
                let (oxp, oyp) = (ex + x, ey + y);
                if oxp < 0 || oyp < 0 || oxp >= ow as i64 || oyp >= oh as i64 {
                    continue;
                }
                let cov = rounded_coverage(x as f32 + 0.5, y as f32 + 0.5, w as f32, bh as f32, r);
                if cov > 0.0 {
                    let di = (oyp as usize * ow as usize + oxp as usize) * 4;
                    over(&mut out[di..di + 4], bar, cov);
                }
            }
        }
        let (x0, y0, u) = (ex as f32, ey as f32, scale);
        let mid = y0 + hh as f32 / 2.0;
        if head.lights {
            for (i, &c) in LIGHTS.iter().enumerate() {
                fill_circle(&mut out, ow, oh, x0 + (16.0 + 20.0 * i as f32) * u, mid, 6.0 * u, c);
            }
        }
        if let Some(c) = head.address {
            let pw = (w as f32 * 0.46).min(440.0 * u);
            let ph = hh as f32 * 0.58;
            fill_round_rect(&mut out, ow, oh, x0 + (w as f32 - pw) / 2.0, mid - ph / 2.0, pw, ph, ph / 2.0, c);
        }
        if let Some(c) = head.controls {
            let right = x0 + w as f32;
            let g = 5.0 * u;
            let (cx3, cx2, cx1) = (right - 22.0 * u, right - 62.0 * u, right - 102.0 * u);
            let segs = [
                ((cx1 - g, mid), (cx1 + g, mid)),
                ((cx2 - g, mid - g), (cx2 + g, mid - g)),
                ((cx2 + g, mid - g), (cx2 + g, mid + g)),
                ((cx2 + g, mid + g), (cx2 - g, mid + g)),
                ((cx2 - g, mid + g), (cx2 - g, mid - g)),
                ((cx3 - g, mid - g), (cx3 + g, mid + g)),
                ((cx3 - g, mid + g), (cx3 + g, mid - g)),
            ];
            stroke_segments(&mut out, ow, oh, &segs, 1.1 * u, c);
        }
    }

    // The capture itself, under the header, with the block's rounded bottom corners.
    for y in 0..h {
        let oy = ey + hh + y;
        if oy < 0 || oy >= oh as i64 {
            continue;
        }
        for x in 0..w {
            let ox = ex + x;
            if ox < 0 || ox >= ow as i64 {
                continue;
            }
            let cov = rounded_coverage(x as f32 + 0.5, (y + hh) as f32 + 0.5, w as f32, bh as f32, r);
            if cov <= 0.0 {
                continue;
            }
            let si = (y as usize * sw as usize + x as usize) * 4;
            let src = [px[si + 2], px[si + 1], px[si], 255];
            let di = (oy as usize * ow as usize + ox as usize) * 4;
            over(&mut out[di..di + 4], src, cov);
        }
    }
    (ow, oh, out)
}

fn rgb(c: u32) -> [u8; 4] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8, 255]
}

fn fill_circle(out: &mut [u8], ow: u32, oh: u32, cx: f32, cy: f32, radius: f32, color: u32) {
    let src = rgb(color);
    let (x0, x1) = ((cx - radius - 1.0).floor().max(0.0) as u32, ((cx + radius + 1.0).ceil() as u32).min(ow));
    let (y0, y1) = ((cy - radius - 1.0).floor().max(0.0) as u32, ((cy + radius + 1.0).ceil() as u32).min(oh));
    for y in y0..y1 {
        for x in x0..x1 {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let cov = (radius + 0.5 - d).clamp(0.0, 1.0);
            if cov > 0.0 {
                let di = (y as usize * ow as usize + x as usize) * 4;
                over(&mut out[di..di + 4], src, cov);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_round_rect(out: &mut [u8], ow: u32, oh: u32, x: f32, y: f32, w: f32, h: f32, r: f32, color: u32) {
    let src = rgb(color);
    let (x0, x1) = (x.floor().max(0.0) as u32, ((x + w).ceil() as u32).min(ow));
    let (y0, y1) = (y.floor().max(0.0) as u32, ((y + h).ceil() as u32).min(oh));
    for py in y0..y1 {
        for px in x0..x1 {
            let cov = rounded_coverage(px as f32 + 0.5 - x, py as f32 + 0.5 - y, w, h, r);
            if cov > 0.0 {
                let di = (py as usize * ow as usize + px as usize) * 4;
                over(&mut out[di..di + 4], src, cov);
            }
        }
    }
}

/// Anti-aliased strokes along `segs`, blended once where they cross.
fn stroke_segments(out: &mut [u8], ow: u32, oh: u32, segs: &[((f32, f32), (f32, f32))], width: f32, color: u32) {
    let src = rgb(color);
    let hw = width / 2.0;
    let pts = segs.iter().flat_map(|(a, b)| [*a, *b]);
    let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in pts {
        (l, t, r, b) = (l.min(x), t.min(y), r.max(x), b.max(y));
    }
    let x0 = (l - hw - 1.0).floor().max(0.0) as u32;
    let y0 = (t - hw - 1.0).floor().max(0.0) as u32;
    let x1 = ((r + hw + 1.0).ceil() as u32).min(ow);
    let y1 = ((b + hw + 1.0).ceil() as u32).min(oh);
    for y in y0..y1 {
        for x in x0..x1 {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let d = segs.iter().map(|&(a, b)| seg_dist(p, a, b)).fold(f32::MAX, f32::min);
            let cov = (hw + 0.5 - d).clamp(0.0, 1.0);
            if cov > 0.0 {
                let di = (y as usize * ow as usize + x as usize) * 4;
                over(&mut out[di..di + 4], src, cov);
            }
        }
    }
}

fn seg_dist(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
}

#[allow(clippy::too_many_arguments)]
fn draw_shadow(out: &mut [u8], ow: u32, oh: u32, ex: i64, ey: i64, w: i64, h: i64, radius: f32, sigma: f32, alpha: f32, color: u32) {
    let margin = (sigma * 3.0).ceil() as i64 + 1;
    let mw = (w + 2 * margin) as usize;
    let mh = (h + 2 * margin) as usize;
    let mut mask = vec![0f32; mw * mh];
    let r = radius.min(w.min(h) as f32 / 2.0);
    for y in 0..h {
        for x in 0..w {
            mask[(y + margin) as usize * mw + (x + margin) as usize] = rounded_coverage(x as f32 + 0.5, y as f32 + 0.5, w as f32, h as f32, r);
        }
    }
    // Three box blurs approximate a gaussian.
    let box_r = ((sigma * 0.87).round() as usize).max(1);
    let mut tmp = vec![0f32; mw * mh];
    for _ in 0..3 {
        box_blur_h(&mask, &mut tmp, mw, mh, box_r);
        box_blur_v(&tmp, &mut mask, mw, mh, box_r);
    }
    let src = rgb(color);
    for my in 0..mh {
        let oy = ey - margin + my as i64;
        if oy < 0 || oy >= oh as i64 {
            continue;
        }
        for mx in 0..mw {
            let ox = ex - margin + mx as i64;
            if ox < 0 || ox >= ow as i64 {
                continue;
            }
            let a = mask[my * mw + mx] * alpha;
            if a > 0.002 {
                let di = (oy as usize * ow as usize + ox as usize) * 4;
                over(&mut out[di..di + 4], src, a);
            }
        }
    }
}

fn box_blur_h(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize) {
    let norm = 1.0 / (2 * r + 1) as f32;
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        let mut acc = 0.0;
        for x in 0..=r.min(w - 1) {
            acc += row[x];
        }
        for x in 0..w {
            dst[y * w + x] = acc * norm;
            if x + r + 1 < w {
                acc += row[x + r + 1];
            }
            if x >= r {
                acc -= row[x - r];
            }
        }
    }
}

fn box_blur_v(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize) {
    let norm = 1.0 / (2 * r + 1) as f32;
    for x in 0..w {
        let mut acc = 0.0;
        for y in 0..=r.min(h - 1) {
            acc += src[y * w + x];
        }
        for y in 0..h {
            dst[y * w + x] = acc * norm;
            if y + r + 1 < h {
                acc += src[(y + r + 1) * w + x];
            }
            if y >= r {
                acc -= src[(y - r) * w + x];
            }
        }
    }
}

/// Coverage of a pixel center by a rounded rectangle at the origin.
fn rounded_coverage(px: f32, py: f32, w: f32, h: f32, r: f32) -> f32 {
    if px < 0.0 || py < 0.0 || px > w || py > h {
        return 0.0;
    }
    if r <= 0.0 {
        return 1.0;
    }
    let cx = if px < r { r } else if px > w - r { w - r } else { return 1.0 };
    let cy = if py < r { r } else if py > h - r { h - r } else { return 1.0 };
    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - r;
    (0.5 - d).clamp(0.0, 1.0)
}

/// Source-over blend of an opaque color with coverage `a` onto straight-alpha `dst`.
fn over(dst: &mut [u8], src: [u8; 4], a: f32) {
    let da = dst[3] as f32 / 255.0;
    let oa = a + da * (1.0 - a);
    if oa <= 0.0 {
        return;
    }
    for k in 0..3 {
        let c = (src[k] as f32 * a + dst[k] as f32 * da * (1.0 - a)) / oa;
        dst[k] = c.round().clamp(0.0, 255.0) as u8;
    }
    dst[3] = (oa * 255.0).round() as u8;
}

fn unpack(c: u32) -> [f32; 3] {
    [((c >> 16) & 0xff) as f32, ((c >> 8) & 0xff) as f32, (c & 0xff) as f32]
}

/// Encodes straight RGBA as PNG with fast compression.
pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::ImageEncoder;
    let mut buf = Vec::new();
    let encoder = PngEncoder::new_with_quality(&mut buf, CompressionType::Fast, FilterType::Adaptive);
    let _ = encoder.write_image(rgba, w, h, image::ExtendedColorType::Rgba8);
    buf
}

/// The raw capture as RGBA (for the first clipboard copy).
pub fn shot_rgba(shot: &Shot) -> Vec<u8> {
    let mut out = shot.bgra().to_vec();
    for px in out.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shot_placement_follows_the_position_bars() {
        assert_eq!(place(100.0, 20.0, 0.0), 40.0, "centred");
        assert_eq!(place(100.0, 20.0, 0.4), 80.0, "flush right");
        assert_eq!(place(100.0, 20.0, -0.5), -10.0, "centre on the left edge, half off the page");
        // A shot larger than the page (scaled up) is centred by default and may spill both ways.
        assert_eq!(place(100.0, 140.0, 0.0), -20.0);
        assert_eq!(grain(3, 7), grain(3, 7), "grain is the same in preview and export");
    }

    #[test]
    fn blur_never_spoils_the_backdrop_or_the_shot() {
        // Full blur keeps a flat backdrop exactly flat (no dark edges) and the shot sharp.
        let shot = Shot::new(4, 4, vec![0, 0, 255, 255].repeat(16), 1.0, "Test");
        let style = Style {
            gradient: &[0x00ff00, 0x00ff00],
            padding: 8,
            radius: 0.0,
            size: None,
            edge: 0,
            edge_width: 0.0,
            header: 0,
            shadow: Shadow { angle: 90.0, size: 0.0, blur: 0.0, opacity: 0.0 },
            scale: 1.0,
            blur: 1.0,
            noise: 0.0,
            offset: (0.0, 0.0),
            zoom: 1.0,
        };
        let (w, _, rgba) = compose(&shot, &style);
        assert_eq!(&rgba[0..4], &[0, 255, 0, 255], "corner is still the green backdrop");
        let centre = ((10 * w + 10) * 4) as usize;
        assert_eq!(&rgba[centre..centre + 3], &[255, 0, 0], "the shot is still the shot");

        // Blur melts a real gradient into a soft mesh: visibly different, yet smooth everywhere.
        let plain = backdrop(&[0x2f6fed, 0xff375f], 200, 100, 0.0);
        let soft = backdrop(&[0x2f6fed, 0xff375f], 200, 100, 1.0);
        let moved = plain.pixels().zip(soft.pixels()).map(|(a, b)| a.0[0].abs_diff(b.0[0]) as u32).max().unwrap();
        assert!(moved > 20, "blur changes the look ({moved})");
        for y in 0..100 {
            for x in 1..200 {
                let (a, b) = (soft.get_pixel(x - 1, y).0, soft.get_pixel(x, y).0);
                assert!((0..3).all(|c| a[c].abs_diff(b[c]) <= 4), "smooth at {x},{y}");
            }
        }
        let three = backdrop(&[0xff0000, 0x00ff00, 0x0000ff], 90, 90, 0.0);
        assert!(three.get_pixel(0, 0).0[0] > 248, "first stop at the start");
        assert!(three.get_pixel(45, 45).0[1] > 240, "middle stop in the middle");
    }

    #[test]
    fn edge_and_shadow_never_resize_the_page_or_the_shot() {
        let base = Style {
            gradient: &[0, 0],
            padding: 32,
            radius: 0.0,
            size: None,
            edge: 0,
            edge_width: 0.0,
            header: 0,
            shadow: Shadow { angle: 90.0, size: 0.0, blur: 0.0, opacity: 0.0 },
            scale: 1.5,
            blur: 0.0,
            noise: 0.0,
            offset: (0.0, 0.0),
            zoom: 1.0,
        };
        for size in [None, Some((1080, 1080)), Some((1500, 500))] {
            let page = output_size(800, 600, &Style { size, ..base });
            for edge in 0..EDGES.len() {
                for header in 0..HEADERS.len() {
                    for (radius, edge_width) in [(0.0, 0.0), (20.0, 12.0), (40.0, 24.0)] {
                        let shadow = Shadow { angle: 30.0, size: 40.0, blur: 90.0, opacity: 0.5 };
                        let s = Style { size, edge, header, radius, edge_width, shadow, ..base };
                        assert_eq!(output_size(800, 600, &s), page, "edge {edge} header {header} radius {radius}");
                        // And the shot itself keeps exactly the same size.
                        assert_eq!(fit(800, 600, &s), fit(800, 600, &Style { size, ..base }), "shot size, edge {edge} header {header}");
                    }
                }
            }
        }
        // With nothing around it, the shot keeps its own pixels.
        assert_eq!(fit(800, 600, &base), 1.0);
    }

    #[test]
    fn header_and_controls_are_painted_above_the_shot() {
        let mut out = vec![0u8; 200 * 40 * 4];
        stroke_segments(&mut out, 200, 40, &[((10.0, 20.0), (60.0, 20.0))], 2.0, 0xff0000);
        fill_circle(&mut out, 200, 40, 120.0, 20.0, 6.0, 0x00ff00);
        let alpha = |x: usize, y: usize| out[(y * 200 + x) * 4 + 3];
        assert_eq!(alpha(30, 20), 255, "on the stroke");
        assert_eq!(alpha(30, 30), 0, "away from it");
        assert_eq!(alpha(120, 20), 255, "inside the light");
        assert_eq!(alpha(140, 20), 0, "outside it");
    }
}
