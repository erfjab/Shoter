//! Frame detection from pixels alone, for apps that publish no accessibility tree (Electron apps
//! like VS Code, games, remote desktops, images of UI).
//!
//! UI is built from flat-colored boxes, so a frame shows up as four straight color boundaries.
//! From the cursor we walk outward in each direction to the first boundary that is a real line
//! (long enough to not be a glyph stroke), then check that the four lines actually close a box;
//! a side that doesn't is pushed further out and tried again. Repeating from just outside the
//! found box yields its parents. Everything works on the frozen capture, per hover, in a few
//! thousand pixel reads: no precomputed maps, no extra memory.

use windows::Win32::Foundation::RECT;

/// Smallest channel difference that counts as a boundary (VS Code's status bar line is just 6).
const THRESHOLD: u8 = 5;
/// Shortest line that can be a frame side rather than a glyph stroke.
const MIN_RUN: i32 = 14;
/// Pixels a line may skip (anti-aliasing, dashed borders) and still count as one line.
const MAX_GAP: i32 = 2;
const MIN_SIDE: i32 = 10;

pub struct Image<'a> {
    /// BGRA, top-down rows.
    pub px: &'a [u8],
    pub w: i32,
    pub h: i32,
}

impl Image<'_> {
    fn differs(&self, a: (i32, i32), b: (i32, i32)) -> bool {
        let i = ((a.1 * self.w + a.0) * 4) as usize;
        let j = ((b.1 * self.w + b.0) * 4) as usize;
        (0..3).any(|k| self.px[i + k].abs_diff(self.px[j + k]) >= THRESHOLD)
    }

    /// Boundary between columns x-1 and x on row y.
    fn v(&self, x: i32, y: i32) -> bool {
        x > 0 && x < self.w && y >= 0 && y < self.h && self.differs((x - 1, y), (x, y))
    }

    /// Boundary between rows y-1 and y on column x.
    fn h(&self, x: i32, y: i32) -> bool {
        y > 0 && y < self.h && x >= 0 && x < self.w && self.differs((x, y - 1), (x, y))
    }
}

/// How far a traced line may bend sideways: enough to follow a rounded corner or a pill's end.
const MAX_DRIFT: i32 = 12;

/// Length of the line through (`cross`, `at`) along one axis, tolerating small gaps and
/// following a gentle curve (one pixel sideways per step), so rounded corners and pill-shaped
/// badges still read as one line. `edge(cross, along)`.
fn run(edge: impl Fn(i32, i32) -> bool, cross: i32, at: i32, lo: i32, hi: i32) -> i32 {
    let walk = |step: i32| {
        let (mut end, mut gap, mut i, mut c) = (at, 0, at + step, cross);
        while i >= lo && i < hi {
            if let Some(nc) = [c, c - 1, c + 1].into_iter().find(|&nc| (nc - cross).abs() <= MAX_DRIFT && edge(nc, i)) {
                c = nc;
                end = i;
                gap = 0;
            } else {
                gap += 1;
                if gap > MAX_GAP {
                    break;
                }
            }
            i += step;
        }
        end
    };
    walk(1) - walk(-1) + 1
}

/// Whether a side covers `lo..hi`. Near its ends the boundary may curve inward by up to the
/// distance to the end (a rounded corner); `edge(along, inward)`.
fn covered(edge: impl Fn(i32, i32) -> bool, lo: i32, hi: i32) -> bool {
    let len = hi - lo;
    let zone = (len / 2).min(28);
    // The tightest part of a rounded corner fades in steps too small to count as edges on a
    // low-contrast box, so the ends of a side are excused; its middle must hold.
    let excused = (len / 8).min(14);
    let hits = (lo..hi)
        .filter(|&i| {
            let d = (i - lo).min(hi - 1 - i);
            let reach = if d < zone { zone - d } else { 1 };
            d < excused || (0..=reach).any(|k| edge(i, k))
        })
        .count() as i32;
    // Long sides may be interrupted here and there (a tab, an overlapping badge).
    len - hits <= len * 12 / 100 + 2
}

/// Whether a separator line runs unbroken from end to end. Table row lines do; a line of text
/// never does (there are spaces between words), however full the line is.
fn solid(edge: impl Fn(i32) -> bool, lo: i32, hi: i32) -> bool {
    let len = hi - lo;
    len - (lo..hi).filter(|&i| edge(i)).count() as i32 <= len * 4 / 100 + 2
}

/// Frames around `p`, innermost first, inside `clip` (image coordinates).
pub fn frames(img: &Image, p: (i32, i32), clip: RECT, max: usize) -> Vec<RECT> {
    let clip = RECT {
        left: clip.left.max(0),
        top: clip.top.max(0),
        right: clip.right.min(img.w),
        bottom: clip.bottom.min(img.h),
    };
    let mut out = Vec::new();
    // Where each side's search starts; parents start just outside the last frame.
    let mut start = RECT { left: p.0, top: p.1, right: p.0 + 1, bottom: p.1 + 1 };
    // Parents are probed through the middle of the last frame rather than the cursor, which
    // may sit in a corner where the parent's edge curves away.
    let mut probe = p;
    while out.len() < max {
        // The innermost frame may be a slice (a table row between separators); every parent
        // must be a real box with clean corners, or rows would stack into fake "parents".
        let Some(f) = frame(img, probe, start, clip, !out.is_empty()) else { break };
        out.push(f);
        start = RECT { left: f.left - 1, top: f.top - 1, right: f.right + 1, bottom: f.bottom + 1 };
        probe = ((f.left + f.right) / 2, (f.top + f.bottom) / 2);
    }
    out
}

/// Whether a line keeps going 3..11 px past a corner (a T-junction, not a corner).
fn continues(edge: impl Fn(i32) -> bool, from: i32, step: i32) -> bool {
    (3..11).filter(|k| edge(from + step * k)).count() >= 6
}

fn frame(img: &Image, p: (i32, i32), start: RECT, clip: RECT, corners: bool) -> Option<RECT> {
    let (px, py) = p;
    // A side is the first boundary from `from` (walking by `step`) whose line is long enough.
    // The window's own edge closes a frame too (status bars, sidebars docked to the edge).
    let side_v = |from: i32, step: i32, py: i32| -> Option<i32> {
        if from < clip.left || from > clip.right {
            return None;
        }
        let mut x = from;
        while x > clip.left && x < clip.right {
            if img.v(x, py) && run(|cx, y| img.v(cx, y), x, py, clip.top, clip.bottom) >= MIN_RUN {
                return Some(x);
            }
            x += step;
        }
        Some(x)
    };
    let side_h = |from: i32, step: i32, px: i32| -> Option<i32> {
        if from < clip.top || from > clip.bottom {
            return None;
        }
        let mut y = from;
        while y > clip.top && y < clip.bottom {
            if img.h(px, y) && run(|cy, x| img.h(x, cy), y, px, clip.left, clip.right) >= MIN_RUN {
                return Some(y);
            }
            y += step;
        }
        Some(y)
    };
    // The next line outward that covers the given span; the window edge if none does.
    let push_v = |from: i32, step: i32, t: i32, b: i32| -> Option<i32> {
        if from < clip.left || from > clip.right {
            return None;
        }
        let mut x = from;
        while x > clip.left && x < clip.right {
            // Probed mid-span: near a rounded corner the cursor row crosses the curve, not the side.
            if img.v(x, (t + b) / 2) && covered(|y, k| img.v(x - step * k, y), t, b) {
                return Some(x);
            }
            x += step;
        }
        Some(x)
    };
    let push_h = |from: i32, step: i32, l: i32, r: i32| -> Option<i32> {
        if from < clip.top || from > clip.bottom {
            return None;
        }
        let mut y = from;
        while y > clip.top && y < clip.bottom {
            if img.h((l + r) / 2, y) && covered(|x, k| img.h(x, y - step * k), l, r) {
                return Some(y);
            }
            y += step;
        }
        Some(y)
    };
    let v_side = |x: i32| x == clip.left || x == clip.right;
    let h_side = |y: i32| y == clip.top || y == clip.bottom;
    let (mut l, mut t) = (side_v(start.left, -1, py)?, side_h(start.top, -1, px)?);
    let (mut r, mut b) = (side_v(start.right, 1, py)?, side_h(start.bottom, 1, px)?);
    // Near a rounded corner the cursor's row (or column) may cross a curve too faint to register;
    // if the opposite pair of sides is real, look again through the middle of the box.
    if (v_side(l) || v_side(r)) && !h_side(t) && !h_side(b) {
        let mid = (t + b) / 2;
        if v_side(l) { l = side_v(start.left, -1, mid)?; }
        if v_side(r) { r = side_v(start.right, 1, mid)?; }
    }
    if (h_side(t) || h_side(b)) && !v_side(l) && !v_side(r) {
        let mid = (l + r) / 2;
        if h_side(t) { t = side_h(start.top, -1, mid)?; }
        if h_side(b) { b = side_h(start.bottom, 1, mid)?; }
    }
    // Code editors and lists are full of short lines (indent guides, row highlights) that look
    // like sides until checked, so allow plenty of retries; each one only moves outward.
    for _ in 0..96 {
        if r - l < MIN_SIDE {
            l = side_v(l - 1, -1, py)?;
            r = side_v(r + 1, 1, py)?;
            continue;
        }
        if b - t < MIN_SIDE {
            t = side_h(t - 1, -1, px)?;
            b = side_h(b + 1, 1, px)?;
            continue;
        }
        // Each side must actually span the box the other three describe.
        let mut ok_l = v_side(l) || covered(|y, k| img.v(l + k, y), t, b);
        let mut ok_r = v_side(r) || covered(|y, k| img.v(r - k, y), t, b);
        let mut ok_t = h_side(t) || covered(|x, k| img.h(x, t + k), l, r);
        let mut ok_b = h_side(b) || covered(|x, k| img.h(x, b - k), l, r);
        // A side whose neighbours run on past it is a separator inside a bigger box (a T-junction,
        // not a corner). Parents must not have any; the innermost frame may be such a slice (a
        // table row), but only between separators that are solid lines, never lines of text.
        let vl = |y| img.v(l, y);
        let vr = |y| img.v(r, y);
        let ht = |x| img.h(x, t);
        let hb = |x| img.h(x, b);
        let tee_t = !h_side(t) && ((!v_side(l) && continues(vl, t, -1)) || (!v_side(r) && continues(vr, t, -1)));
        let tee_b = !h_side(b) && ((!v_side(l) && continues(vl, b, 1)) || (!v_side(r) && continues(vr, b, 1)));
        let tee_l = !v_side(l) && ((!h_side(t) && continues(ht, l, -1)) || (!h_side(b) && continues(hb, l, -1)));
        let tee_r = !v_side(r) && ((!h_side(t) && continues(ht, r, 1)) || (!h_side(b) && continues(hb, r, 1)));
        if corners {
            ok_t &= !tee_t;
            ok_b &= !tee_b;
            ok_l &= !tee_l;
            ok_r &= !tee_r;
        } else {
            ok_t &= !tee_t || solid(ht, l, r);
            ok_b &= !tee_b || solid(hb, l, r);
            ok_l &= !tee_l || solid(vl, t, b);
            ok_r &= !tee_r || solid(vr, t, b);
        }
        if ok_l && ok_r && ok_t && ok_b {
            return Some(RECT { left: l, top: t, right: r, bottom: b });
        }
        // A failing side jumps straight to the next line that spans the box the other sides
        // describe, instead of stepping through every glyph boundary on the way.
        let (l0, t0, r0, b0) = (l, t, r, b);
        if !ok_l {
            l = push_v(l0 - 1, -1, t0, b0)?;
        }
        if !ok_r {
            r = push_v(r0 + 1, 1, t0, b0)?;
        }
        if !ok_t {
            t = push_h(t0 - 1, -1, l0, r0)?;
        }
        if !ok_b {
            b = push_h(b0 + 1, 1, l0, r0)?;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: i32 = 600;
    const H: i32 = 400;

    struct Canvas(Vec<u8>);

    impl Canvas {
        fn new(gray: u8) -> Self {
            Canvas(vec![gray; (W * H * 4) as usize])
        }

        /// A filled box with anti-aliased rounded corners, blended like a real renderer would.
        fn rect(&mut self, (l, t, r, b): (i32, i32, i32, i32), radius: f32, gray: u8) -> &mut Self {
            for y in t..b {
                for x in l..r {
                    // Signed distance to a rounded box, negative inside.
                    let (hw, hh) = ((r - l) as f32 / 2.0, (b - t) as f32 / 2.0);
                    let qx = (x as f32 + 0.5 - (l as f32 + hw)).abs() - (hw - radius);
                    let qy = (y as f32 + 0.5 - (t as f32 + hh)).abs() - (hh - radius);
                    let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - radius;
                    let a = (0.5 - d).clamp(0.0, 1.0);
                    let i = ((y * W + x) * 4) as usize;
                    for k in 0..3 {
                        let v = self.0[i + k] as f32 * (1.0 - a) + gray as f32 * a;
                        self.0[i + k] = v.round() as u8;
                    }
                }
            }
            self
        }

        /// A Latin word: glyph-sized strokes only.
        fn word(&mut self, x: i32, y: i32, chars: i32, gray: u8) -> &mut Self {
            for k in 0..chars {
                self.rect((x + k * 7, y, x + k * 7 + 2, y + 12), 0.0, gray);
            }
            self
        }

        /// A line of connected script: glyph strokes on a long baseline, with gaps between words.
        fn text(&mut self, x: i32, y: i32, chars: i32, gray: u8) -> &mut Self {
            self.word(x, y, chars, gray);
            let mut k = 0;
            while k < chars {
                let n = (chars - k).min(5);
                self.rect((x + k * 7, y + 10, x + (k + n) * 7 - 4, y + 12), 0.0, gray);
                k += n + 1;
            }
            self
        }

        fn frames(&self, p: (i32, i32)) -> Vec<(i32, i32, i32, i32)> {
            let img = Image { px: &self.0, w: W, h: H };
            let all = RECT { left: 0, top: 0, right: W, bottom: H };
            frames(&img, p, all, 8).iter().map(|r| (r.left, r.top, r.right, r.bottom)).collect()
        }
    }

    /// Two frames are the same element if they differ by at most a border.
    fn near(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> bool {
        (a.0 - b.0).abs() <= 2 && (a.1 - b.1).abs() <= 2 && (a.2 - b.2).abs() <= 2 && (a.3 - b.3).abs() <= 2
    }

    fn assert_chain(got: &[(i32, i32, i32, i32)], want: &[(i32, i32, i32, i32)]) {
        let mut chain: Vec<_> = Vec::new();
        for &f in got {
            if chain.last().map_or(true, |&l| !near(l, f)) {
                chain.push(f);
            }
        }
        assert!(chain.len() >= want.len(), "got {chain:?}, want {want:?}");
        for (g, w) in chain.iter().zip(want) {
            assert!(near(*g, *w), "got {chain:?}, want {want:?}");
        }
    }

    #[test]
    fn button_inside_panel_and_text_is_not_a_frame() {
        let mut c = Canvas::new(0x18);
        c.rect((100, 50, 400, 300), 0.0, 0x1f).rect((150, 100, 220, 130), 6.0, 0x3a).text(120, 200, 12, 0xd0);
        assert_chain(&c.frames((170, 115)), &[(150, 100, 220, 130), (100, 50, 400, 300)]);
        assert_chain(&c.frames((150, 206)), &[(100, 50, 400, 300)]);
    }

    #[test]
    fn table_rows_lead_to_the_table_not_to_stacks_of_rows() {
        let mut c = Canvas::new(0x10);
        c.rect((20, 20, 580, 380), 12.0, 0x16); // panel
        c.rect((40, 60, 560, 360), 8.0, 0x2a).rect((41, 61, 559, 359), 7.0, 0x16); // bordered table
        for y in [100, 140, 180, 220, 260, 300] {
            c.rect((41, y, 559, y + 1), 0.0, 0x2a); // row separators
        }
        c.rect((300, 150, 340, 170), 10.0, 0x1e).word(306, 154, 4, 0x40); // pill badge in a row
        c.text(80, 154, 6, 0xd0);

        // In a row: the row, then straight to the table, then the panel.
        assert_chain(&c.frames((200, 160)), &[(41, 141, 559, 180), (40, 60, 560, 360), (20, 20, 580, 380)]);
        // A row at the table's rounded edge still leads to the table.
        assert_chain(&c.frames((50, 355)), &[(41, 301, 559, 359), (40, 60, 560, 360)]);
        // On the pill badge: the badge first.
        assert_chain(&c.frames((320, 160)), &[(300, 150, 340, 170)]);
        // Never a frame made of several rows.
        for f in c.frames((200, 160)) {
            let rows = (f.3 - f.1) / 40;
            assert!(!(rows >= 2 && f.3 - f.1 < 290), "stacked rows {f:?}");
        }
    }

    #[test]
    fn low_contrast_rounded_card_with_big_digits() {
        let mut c = Canvas::new(15);
        c.rect((100, 100, 400, 190), 24.0, 22); // 7 levels apart, like dark dashboard cards
        for k in 0..3 {
            c.rect((130 + k * 22, 125, 146 + k * 22, 165), 2.0, 240); // large digits
        }
        assert_chain(&c.frames((220, 145)), &[(100, 100, 400, 190)]);
        assert_chain(&c.frames((149, 110)), &[(100, 100, 400, 190)]);
    }

    #[test]
    fn chat_bubble_full_of_text_lines() {
        let mut c = Canvas::new(0x1e);
        c.rect((60, 40, 540, 300), 16.0, 0x24); // 6 levels above the page
        for line in 0..7 {
            c.text(90, 70 + line * 30, 58, 0xe0);
        }
        for p in [(300, 100), (300, 90), (80, 290), (530, 50)] {
            assert_chain(&c.frames(p), &[(60, 40, 540, 300)]);
        }
    }

    #[test]
    fn strip_docked_to_the_window_edge() {
        let mut c = Canvas::new(0x1f);
        c.rect((0, 370, W, 371), 0.0, 0x25).text(30, 380, 8, 0xc0); // status bar: one faint line
        assert_chain(&c.frames((300, 385)), &[(0, 371, W, H)]);
    }
}
