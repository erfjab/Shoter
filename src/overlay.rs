//! Full-screen capture overlay: shows the frozen screen and nothing else but a highlight on the
//! element under the cursor. Click captures that element, drag captures a free region.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures::channel::mpsc::unbounded;
use futures::StreamExt;
use gpui::{
    div, img, point, prelude::*, px, rgba, size, App, Bounds, Context, CursorStyle, FontWeight,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta,
    ScrollWheelEvent, Task, Window, WindowBackgroundAppearance, WindowBounds, WindowKind,
    WindowOptions,
};
use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::shot::Shot;
use crate::theme::{Theme, MONO, SANS};
use crate::vision;
use crate::win::capture::{Monitor, WinInfo};
use crate::win::uia::{self, Node, Reply};
use crate::AppState;

#[derive(Clone, Copy, PartialEq)]
struct Candidate {
    rect: RECT,
    kind: &'static str,
}

pub struct Overlay {
    shot: Arc<Shot>,
    monitor: Monitor,
    windows: Vec<WinInfo>,
    trees: HashMap<isize, Vec<Node>>,
    pending: HashSet<isize>,
    reply: Reply,
    /// Cursor in physical screen pixels.
    cursor: POINT,
    candidates: Vec<Candidate>,
    level: usize,
    scroll_acc: f32,
    drag_start: Option<POINT>,
    region: Option<RECT>,
    _replies: Task<()>,
}

pub fn open(shot: Shot, monitor: Monitor, windows: Vec<WinInfo>, cx: &mut App) {
    let s = monitor.scale;
    let r = monitor.rect;
    let bounds = Bounds::new(
        point(px(r.left as f32 / s), px(r.top as f32 / s)),
        size(px((r.right - r.left) as f32 / s), px((r.bottom - r.top) as f32 / s)),
    );
    let display_id = cx
        .displays()
        .iter()
        .find(|d| d.bounds().contains(&bounds.center()))
        .map(|d| d.id());
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: true,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            display_id,
            window_background: WindowBackgroundAppearance::Opaque,
            ..Default::default()
        },
        move |window, cx| {
            crate::win::place_topmost(window, r, true);
            cx.new(|cx| Overlay::new(shot, monitor, windows, cx))
        },
    );
    if let Ok(handle) = handle {
        cx.global_mut::<AppState>().overlay = Some(handle);
        crate::win::input::lock(Some(r));
    }
}

pub fn cancel(cx: &mut App) {
    crate::win::input::lock(None);
    if let Some(handle) = cx.global_mut::<AppState>().overlay.take() {
        let _ = handle.update(cx, |this, window, _| {
            let _ = window.drop_image(this.shot.image.clone());
            window.remove_window();
        });
    }
    crate::trim_soon(cx);
}

impl Overlay {
    fn new(shot: Shot, monitor: Monitor, windows: Vec<WinInfo>, cx: &mut Context<Self>) -> Self {
        let (reply, mut rx) = unbounded::<(isize, Vec<Node>)>();
        let replies = cx.spawn(async move |this, cx| {
            while let Some((hwnd, nodes)) = rx.next().await {
                let alive = this.update(cx, |this, cx| {
                    this.pending.remove(&hwnd);
                    this.trees.insert(hwnd, nodes);
                    if this.recompute() {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let mut cursor = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut cursor);
        }
        let mut this = Self {
            shot: Arc::new(shot),
            monitor,
            windows,
            trees: HashMap::new(),
            pending: HashSet::new(),
            reply,
            cursor,
            candidates: Vec::new(),
            level: 0,
            scroll_acc: 0.0,
            drag_start: None,
            region: None,
            _replies: replies,
        };
        this.recompute();
        this
    }

    fn to_physical(&self, p: Point<Pixels>, window: &Window) -> POINT {
        let s = window.scale_factor();
        POINT {
            x: self.monitor.rect.left + (f32::from(p.x) * s).round() as i32,
            y: self.monitor.rect.top + (f32::from(p.y) * s).round() as i32,
        }
    }

    /// Rebuilds the stack of things under the cursor. Returns true if it changed.
    fn recompute(&mut self) -> bool {
        let p = self.cursor;
        let m = self.monitor.rect;
        let mut list: Vec<Candidate> = Vec::with_capacity(16);
        // Windows come front to back, so the first hit is the one actually on top.
        if let Some(w) = self.windows.iter().find(|w| contains(&w.rect, p)).copied() {
            let wr = intersect(&w.rect, &m);
            // 1. What the app says it is made of (UI Automation), innermost first.
            let mut tree = Vec::new();
            match self.trees.get(&w.hwnd) {
                Some(nodes) => uia::stack_at(nodes, p, |n| tree.push(Candidate { rect: n.rect, kind: n.kind })),
                None => {
                    if self.pending.insert(w.hwnd) {
                        uia::request_tree(w.hwnd, self.reply.clone());
                    }
                }
            }
            // Nothing shows outside its container (a heading's box can be far wider than the
            // dialog that clips it).
            let mut clip = wr;
            for c in tree.iter_mut().rev() {
                c.rect = intersect(&c.rect, &clip);
                clip = c.rect;
            }
            list.extend(tree);
            // 2. What the pixels show: works for apps with no accessibility tree at all.
            let img = vision::Image { px: self.shot.bgra(), w: self.shot.width as i32, h: self.shot.height as i32 };
            let local = RECT { left: wr.left - m.left, top: wr.top - m.top, right: wr.right - m.left, bottom: wr.bottom - m.top };
            for f in vision::frames(&img, (p.x - m.left, p.y - m.top), local, 6) {
                let rect = RECT { left: f.left + m.left, top: f.top + m.top, right: f.right + m.left, bottom: f.bottom + m.top };
                list.push(Candidate { rect, kind: "Frame" });
            }
            list.push(Candidate { rect: wr, kind: "Window" });
        }
        list.push(Candidate { rect: m, kind: "Screen" });
        list.retain(|c| area(&c.rect) > 0);
        list.sort_by_key(|c| area(&c.rect));
        list.dedup_by(|a, b| same(&a.rect, &b.rect));

        if list != self.candidates {
            self.level = keep_level(&self.candidates, self.level, &list);
            self.candidates = list;
            true
        } else {
            false
        }
    }

    fn selection(&self) -> Option<(RECT, &'static str)> {
        if let Some(r) = self.region {
            return Some((r, "Region"));
        }
        self.candidates
            .get(self.level.min(self.candidates.len().saturating_sub(1)))
            .map(|c| (c.rect, c.kind))
    }

    fn on_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.to_physical(e.position, window);
        if let (Some(start), true) = (self.drag_start, e.pressed_button == Some(MouseButton::Left)) {
            let threshold = (4.0 * window.scale_factor()) as i32;
            if self.region.is_some()
                || (self.cursor.x - start.x).abs() > threshold
                || (self.cursor.y - start.y).abs() > threshold
            {
                self.region = Some(normalize(start, self.cursor));
                cx.notify();
                return;
            }
        }
        if self.recompute() {
            cx.notify();
        }
    }

    fn on_down(&mut self, e: &MouseDownEvent, window: &mut Window, _: &mut Context<Self>) {
        self.drag_start = Some(self.to_physical(e.position, window));
        self.region = None;
    }

    fn on_up(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag_start.take().is_none() {
            return;
        }
        if let Some((rect, kind)) = self.selection() {
            self.finish(rect, kind, window, cx);
        }
    }

    fn on_scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let dy = match e.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => f32::from(p.y) / 40.0,
        };
        self.scroll_acc += dy;
        let steps = self.scroll_acc.trunc() as i32;
        if steps == 0 {
            return;
        }
        self.scroll_acc -= steps as f32;
        let max = self.candidates.len().saturating_sub(1) as i32;
        // Scrolling up walks out to the parent, down walks back in.
        self.level = (self.level as i32 + steps).clamp(0, max) as usize;
        cx.notify();
    }

    fn finish(&mut self, rect: RECT, kind: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let m = self.monitor.rect;
        let r = intersect(&rect, &m);
        if area(&r) <= 0 {
            return;
        }
        let shot = Arc::new(self.shot.crop(
            (r.left - m.left) as u32,
            (r.top - m.top) as u32,
            (r.right - r.left) as u32,
            (r.bottom - r.top) as u32,
            kind,
        ));
        crate::win::input::lock(None);
        let _ = window.drop_image(self.shot.image.clone());
        window.remove_window();
        cx.global_mut::<AppState>().overlay = None;

        // Clipboard first, off the UI thread.
        let for_clipboard = shot.clone();
        std::thread::spawn(move || {
            let rgba = crate::compose::shot_rgba(&for_clipboard);
            crate::win::clipboard::set_image(for_clipboard.width, for_clipboard.height, &rgba);
        });
        let monitor = self.monitor;
        cx.defer(move |cx| crate::toast::open(shot, monitor, cx));
    }
}

impl Render for Overlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::from(window.appearance());
        let s = window.scale_factor();
        let vp = window.viewport_size();
        let (vw, vh) = (f32::from(vp.width), f32::from(vp.height));
        let m = self.monitor.rect;

        let mut root = div()
            .id("overlay")
            .relative()
            .size_full()
            .overflow_hidden()
            .font_family(SANS)
            .cursor(CursorStyle::Crosshair)
            .child(img(self.shot.image.clone()).absolute().top_0().left_0().size_full())
            .on_mouse_move(cx.listener(Self::on_move))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_up))
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.defer(cancel))
            .on_scroll_wheel(cx.listener(Self::on_scroll));

        if let Some((rect, kind)) = self.selection() {
            // Edges are snapped to whole device pixels so the frame sits exactly on the pixels
            // the capture will contain; the frame is drawn inside the selection, never outside.
            let snap = |v: i32| (v as f32).round() / s;
            let x = snap(rect.left - m.left);
            let y = snap(rect.top - m.top);
            let w = snap(rect.right - m.left) - x;
            let h = snap(rect.bottom - m.top) - y;
            let dim = rgba(0x00000070);
            let shade = |l: f32, tp: f32, w: f32, h: f32| {
                div().absolute().left(px(l)).top(px(tp)).w(px(w.max(0.))).h(px(h.max(0.))).bg(dim)
            };
            let label_y = if y > 34.0 { y - 30.0 } else { (y + h + 6.0).min(vh - 30.0) };
            root = root
                .child(shade(0., 0., vw, y))
                .child(shade(0., y, x, h))
                .child(shade(x + w, y, vw - x - w, h))
                .child(shade(0., y + h, vw, vh - y - h))
                .child(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(w))
                        .h(px(h))
                        .border(px((2. * s).round() / s))
                        .border_color(t.accent),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(x.max(4.)))
                        .top(px(label_y))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .h(px(24.))
                        .px(px(9.))
                        .rounded(px(7.))
                        .bg(t.accent)
                        .text_color(t.on_accent)
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(kind)
                        .child(
                            div()
                                .font_family(MONO)
                                .text_size(px(11.))
                                .child(format!("{} × {}", rect.right - rect.left, rect.bottom - rect.top)),
                        ),
                );
        }
        root
    }
}

fn contains(r: &RECT, p: POINT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

fn intersect(a: &RECT, b: &RECT) -> RECT {
    RECT {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    }
}

fn area(r: &RECT) -> i64 {
    let w = (r.right - r.left) as i64;
    let h = (r.bottom - r.top) as i64;
    if w <= 0 || h <= 0 {
        0
    } else {
        w * h
    }
}

/// Close enough to be the same box: a tree rect and a pixel frame of one element can differ by a
/// border's width.
fn same(a: &RECT, b: &RECT) -> bool {
    (a.left - b.left).abs() <= 2
        && (a.top - b.top).abs() <= 2
        && (a.right - b.right).abs() <= 2
        && (a.bottom - b.bottom).abs() <= 2
}

fn normalize(a: POINT, b: POINT) -> RECT {
    RECT { left: a.x.min(b.x), top: a.y.min(b.y), right: a.x.max(b.x), bottom: a.y.max(b.y) }
}

/// After the stack under the cursor changes, stays on the frame the user scrolled out to as long
/// as the cursor is still inside it; otherwise starts again from the innermost element.
fn keep_level(old: &[Candidate], level: usize, new: &[Candidate]) -> usize {
    if level == 0 {
        return 0;
    }
    old.get(level).and_then(|o| new.iter().position(|c| same(&c.rect, &o.rect))).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(l: i32, t: i32, r: i32, b: i32) -> Candidate {
        Candidate { rect: RECT { left: l, top: t, right: r, bottom: b }, kind: "Frame" }
    }

    #[test]
    fn scrolled_out_selection_survives_mouse_moves() {
        let table = c(100, 100, 900, 600);
        let (cell_a, cell_b) = (c(110, 110, 200, 140), c(300, 110, 390, 140));
        let screen = c(0, 0, 1920, 1080);
        let old = [cell_a, table, screen];
        // Scrolled out to the table (level 1), then moved to another cell: still the table.
        assert_eq!(keep_level(&old, 1, &[cell_b, table, screen]), 1);
        // Moved off the table entirely: back to the innermost element.
        assert_eq!(keep_level(&old, 1, &[c(950, 50, 990, 90), screen]), 0);
        // Never scrolled: always follows the innermost element.
        assert_eq!(keep_level(&old, 0, &[cell_b, table, screen]), 0);
    }
}
