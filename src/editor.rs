//! The editor: the preview fills the window, and one row of buttons under it opens the panels:
//! Page size, Background, Edge, Shadow, then Undo / Redo and Export. Every change to the picture
//! is undoable with Ctrl+Z / Ctrl+Y.
//!
//! Rendering stays light on purpose: a panel is only built while it is open and closed rows
//! build only what they show, so dragging a bar stays fluid.

use std::cell::{Cell, RefCell};

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    actions, anchored, canvas, deferred, div, ease_out_quint, fill, hsla, img, linear_color_stop, linear_gradient, point,
    prelude::*, px, relative, rgb, rgba, size, Animation, AnimationExt, AnyElement, App, Background as Fill, Bounds,
    BoxShadow, ClipboardItem, Context, Corner, CursorStyle, ElementId, FocusHandle, FontWeight, Hsla, KeyBinding,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, Pixels, RenderImage, ScrollHandle,
    SharedString, TitlebarOptions, Window, WindowBounds, WindowKind, WindowOptions,
};

use crate::compose::{self, Shadow, Style, EDGES, HEADERS, LIGHTS};
use crate::shot::Shot;
use crate::theme::{Theme, ICONS, MONO, SANS};
use crate::ui::logo;

actions!(shoter_editor, [Undo, Redo, Dismiss]);

const BAR_H: f32 = 40.0;
const GAP: f32 = 10.0;
/// Height of the row of buttons under the preview.
const DOCK_H: f32 = 52.0;

/// The backdrops, as sRGB stops: a soft white and a soft black (the light and dark themes'
/// defaults), then the first 48 gradients on coolors.co/gradients.
const GRADIENTS: [&[u32]; 50] = [
    &[0xffffff, 0xececef],
    &[0x1a1a1d, 0x000000],
    &[0x00ff87, 0x60efff],
    &[0x0061ff, 0x60efff],
    &[0xff1b6b, 0x45caff],
    &[0x40c9ff, 0xe81cff],
    &[0xff930f, 0xfff95b],
    &[0xff0f7b, 0xf89b29],
    &[0xbf0fff, 0xcbff49],
    &[0x696eff, 0xf8acff],
    &[0xa9ff68, 0xff8989],
    &[0xff5858, 0xffc8c8],
    &[0x595cff, 0xc6f8ff],
    &[0xffa585, 0xffeda0],
    &[0x84ffc9, 0xaab2ff, 0xeca0ff],
    &[0xef709b, 0xfa9372],
    &[0xb2ef91, 0xfa9372],
    &[0x9bf8f4, 0x6f7bf7],
    &[0xf9c58d, 0xf492f0],
    &[0xf492f0, 0xa18dce],
    &[0xf9b16e, 0xf68080],
    &[0x9bafd9, 0x103783],
    &[0xfbd07c, 0xf7f779],
    &[0xc5f9d7, 0xf7d486, 0xf27a7d],
    &[0xebf4f5, 0xb5c6e0],
    &[0xf6d5f7, 0xfbe9d7],
    &[0x432371, 0xfaae7b],
    &[0xe9b7ce, 0xd3f3f1],
    &[0x439cfb, 0xf187fb],
    &[0x1dbde6, 0xf1515e],
    &[0x57ebde, 0xaefb2a],
    &[0x42047e, 0x07f49e],
    &[0xf4f269, 0x5cb270],
    &[0xb190ba, 0xe8b595],
    &[0xb597f6, 0x96c6ea],
    &[0xc9def4, 0xf5ccd4, 0xb8a4c9],
    &[0x7c65a9, 0x96d4ca],
    &[0xf6cfbe, 0xb9dcf2],
    &[0xcaefd7, 0xf5bfd7, 0xabc9e9],
    &[0x9fccfa, 0x0974f1],
    &[0xffb88e, 0xea5753],
    &[0xd397fa, 0x8364e8],
    &[0x8de9d5, 0x32c4c0],
    &[0xf5e6ad, 0xf13c77],
    &[0x82f4b1, 0x30c67c],
    &[0xd4acfb, 0xb84fce],
    &[0xf7ba2c, 0xea5459],
    &[0x61f4de, 0x6e78ff],
    &[0xffcb6b, 0x3d8bff],
    &[0xa8f368, 0xf9035e],
];

/// Output sizes by where they're posted: (group, [(name, width, height)]).
const SIZE_GROUPS: [(&str, &[(&str, u32, u32)]); 10] = [
    ("Screen", &[("Desktop", 1920, 1080), ("4K UHD", 3840, 2160), ("HD", 1280, 720), ("Ultrawide", 2560, 1080)]),
    ("Instagram", &[("Square post", 1080, 1080), ("Portrait post", 1080, 1350), ("Landscape", 1080, 566), ("Story / Reel", 1080, 1920)]),
    ("TikTok", &[("Video / Photo", 1080, 1920), ("Square", 1080, 1080)]),
    ("X / Twitter", &[("Post", 1600, 900), ("Square post", 1080, 1080), ("Header", 1500, 500)]),
    ("YouTube", &[("Thumbnail", 1280, 720), ("Shorts", 1080, 1920), ("Channel banner", 2560, 1440), ("Community", 1080, 1080)]),
    ("LinkedIn", &[("Post", 1200, 627), ("Square post", 1080, 1080), ("Banner", 1584, 396)]),
    ("Facebook", &[("Post", 1200, 630), ("Story", 1080, 1920), ("Cover", 851, 315)]),
    ("Pinterest", &[("Pin", 1000, 1500)]),
    ("Web", &[("Open Graph", 1200, 630), ("Dribbble", 1600, 1200), ("Product Hunt", 1270, 760)]),
    ("Print · 300 dpi", &[("A4", 2480, 3508), ("US Letter", 2550, 3300)]),
];
const MIN_SIDE: u32 = 16;
const MAX_SIDE: u32 = 8000;

/// Corner radius presets (logical pixels).
const RADII: [(&str, f32); 5] = [("None", 0.0), ("Small", 6.0), ("Medium", 10.0), ("Large", 20.0), ("Round", 32.0)];

/// Shadow presets: (name, size, blur, opacity).
const SHADOWS: [(&str, f32, f32, f32); 5] =
    [("None", 0.0, 0.0, 0.0), ("Soft", 6.0, 24.0, 0.25), ("Lifted", 16.0, 44.0, 0.38), ("Deep", 36.0, 90.0, 0.55), ("Halo", 0.0, 60.0, 0.45)];

// Bar ids and ranges; all are picture values, undoable, in `Doc::values`.
const PADDING: usize = 0;
const RADIUS: usize = 1;
const EDGE_W: usize = 2;
const BLUR: usize = 3;
const NOISE: usize = 4;
const SCALE: usize = 5;
const POS_X: usize = 6;
const POS_Y: usize = 7;
const SH_SIZE: usize = 8;
const SH_BLUR: usize = 9;
const SH_OPACITY: usize = 10;
const SH_ANGLE: usize = 11;
const BARS: usize = 12;
const RANGES: [(f32, f32); BARS] = [
    (0.0, 160.0),
    (0.0, 40.0),
    (0.0, 24.0),
    (0.0, 1.0),
    (0.0, 1.0),
    (0.3, 1.6),
    (-1.0, 1.0),
    (-1.0, 1.0),
    (0.0, 60.0),
    (0.0, 120.0),
    (0.0, 1.0),
    (0.0, 360.0),
];

// Text fields.
const WIDTH: usize = 0;
const HEIGHT: usize = 1;

/// The position pad's size, centred in the Page size panel.
const PAD_W: f32 = 272.0;
const PAD_H: f32 = 150.0;
/// The shadow direction pad's height (as wide as the position pad).
const SHADOW_PAD_H: f32 = 120.0;
/// Panels stay short and scroll, so the preview stays in view.
const PANEL_MAX_H: f32 = 340.0;

// Panels.
const M_PAGE: u8 = 1;
const M_BG: u8 = 2;
const M_EDGE: u8 = 3;
const M_SHADOW: u8 = 4;
const M_EXPORT: u8 = 5;

/// Everything about the picture that undo covers.
#[derive(Clone, PartialEq)]
struct Doc {
    /// The backdrop, an index into `GRADIENTS`.
    backdrop: usize,
    values: [f32; BARS],
    edge: usize,
    header: usize,
    /// Exact output size; `None` is the shot at its own size plus padding.
    size: Option<(u32, u32)>,
    /// The size card that was picked (group, index). Presets can share a size, so this, not
    /// `size`, decides which one shows as selected; typing W/H clears it.
    preset: Option<(usize, usize)>,
}

/// The backdrop at preview size, rendered by the same code as the export; keyed by
/// (gradient, blur in hundredths, width, height).
struct Hero {
    key: (usize, u32, u32, u32),
    image: Arc<RenderImage>,
}
pub struct Editor {
    shot: Arc<Shot>,
    image: Arc<RenderImage>,
    /// A grain tile.
    grain: Arc<RenderImage>,
    /// Gradient tile thumbnails, rendered the first time they're shown.
    thumbs: RefCell<std::collections::HashMap<usize, Arc<RenderImage>>>,
    hero: Option<Hero>,
    /// Every GPU image this window uploaded, freed when it closes.
    gpu: Rc<RefCell<Vec<Arc<RenderImage>>>>,
    focus: FocusHandle,
    doc: Doc,
    /// The last committed state, and the undo / redo stacks around it.
    saved: Doc,
    past: Vec<Doc>,
    future: Vec<Doc>,
    dragging: Option<usize>,
    tracks: Rc<RefCell<[Bounds<Pixels>; BARS]>>,
    picker: Rc<Cell<Bounds<Pixels>>>,
    picking: bool,
    /// A drag of the shot on the position pad: where it started and the offset then.
    pad_drag: Option<(gpui::Point<Pixels>, (f32, f32))>,
    /// A drag of the panel's scrollbar thumb: the mouse y and the scroll offset at the start.
    bar_drag: Option<(f32, f32)>,
    fields: [FocusHandle; 2],
    drafts: [String; 2],
    menu: Option<u8>,
    /// The panel a press outside just closed, so its own button doesn't reopen it.
    just_closed: Option<u8>,
    panel_scroll: ScrollHandle,
    status: Option<SharedString>,
    busy: bool,
}

pub fn open(shot: Arc<Shot>, cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-z", Undo, Some("Editor")),
        KeyBinding::new("ctrl-y", Redo, Some("Editor")),
        KeyBinding::new("ctrl-shift-z", Redo, Some("Editor")),
        KeyBinding::new("escape", Dismiss, Some("Editor")),
    ]);
    let bounds = Bounds::centered(None, size(px(1240.), px(820.)), cx);
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Shoter".into()), appears_transparent: true, traffic_light_position: None }),
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            window_min_size: Some(size(px(980.), px(600.))),
            ..Default::default()
        },
        |window, cx| {
            let editor = cx.new(|cx| Editor::new(shot, Theme::from(window.appearance()).dark, cx));
            let gpu = editor.read(cx).gpu.clone();
            window.on_window_should_close(cx, move |window, cx| {
                for image in gpu.borrow().iter() {
                    let _ = window.drop_image(image.clone());
                }
                crate::trim_soon(cx);
                true
            });
            window.focus(&editor.read(cx).focus);
            editor
        },
    );
    cx.activate(true);
}

fn bgra(img: image::RgbaImage) -> Arc<RenderImage> {
    Arc::new(RenderImage::new(vec![image::Frame::new(img)]))
}

fn swap_rb(mut img: image::RgbaImage) -> image::RgbaImage {
    for p in img.pixels_mut() {
        p.0.swap(0, 2);
    }
    img
}

/// A rectangle with the proportions `w`:`h`, fitted into `max_w` × `max_h`.
fn aspect_box(w: u32, h: u32, max_w: f32, max_h: f32, border: Hsla, fill: Hsla) -> gpui::Div {
    let k = (max_w / w as f32).min(max_h / h as f32);
    div().w(px(w as f32 * k)).h(px(h as f32 * k)).rounded(px(3.)).border_1().border_color(border).bg(fill)
}

impl Editor {
    fn new(shot: Arc<Shot>, dark: bool, cx: &mut Context<Self>) -> Self {
        let doc = Doc {
            // The soft black in a dark theme, the soft white in a light one.
            backdrop: if dark { 1 } else { 0 },
            // New shots start with a 10 px radius and no stroke, header or shadow.
            values: [64.0, 10.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 90.0],
            edge: 0,
            header: 0,
            size: None,
            preset: None,
        };
        let grain = image::RgbaImage::from_fn(512, 512, |x, y| {
            let v = compose::grain(x, y);
            image::Rgba([v, v, v, (compose::GRAIN_ALPHA * 255.0) as u8])
        });
        let grain = bgra(grain);
        let gpu = vec![shot.image.clone(), grain.clone()];
        Self {
            image: shot.image.clone(),
            shot,
            grain,
            thumbs: RefCell::new(Default::default()),
            hero: None,
            gpu: Rc::new(RefCell::new(gpu)),
            focus: cx.focus_handle(),
            saved: doc.clone(),
            doc,
            past: Vec::new(),
            future: Vec::new(),
            dragging: None,
            tracks: Rc::new(RefCell::new([Bounds::default(); BARS])),
            picker: Rc::new(Cell::new(Bounds::default())),
            picking: false,
            pad_drag: None,
            bar_drag: None,
            fields: [cx.focus_handle(), cx.focus_handle()],
            drafts: Default::default(),
            menu: None,
            just_closed: None,
            panel_scroll: ScrollHandle::new(),
            status: None,
            busy: false,
        }
    }

    fn style(&self) -> Style {
        let s = self.shot.scale;
        let v = &self.doc.values;
        Style {
            gradient: GRADIENTS[self.doc.backdrop],
            padding: (v[PADDING] * s).round() as u32,
            radius: v[RADIUS] * s,
            size: self.doc.size,
            edge: self.doc.edge,
            edge_width: v[EDGE_W],
            header: self.doc.header,
            shadow: Shadow { angle: v[SH_ANGLE], size: v[SH_SIZE], blur: v[SH_BLUR], opacity: v[SH_OPACITY] },
            scale: s,
            blur: v[BLUR],
            noise: v[NOISE],
            offset: (v[POS_X], v[POS_Y]),
            zoom: v[SCALE],
        }
    }

    fn output(&self) -> (u32, u32) {
        compose::output_size(self.shot.width, self.shot.height, &self.style())
    }

    // ---- history ----------------------------------------------------------------------------

    /// Records the picture as one undo step if it changed since the last one. Called when a
    /// gesture ends (any mouse release) and when a field is applied, so a bar drag is one step.
    fn commit(&mut self) {
        if self.doc != self.saved {
            self.past.push(std::mem::replace(&mut self.saved, self.doc.clone()));
            self.future.clear();
        }
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        self.commit();
        if let Some(prev) = self.past.pop() {
            self.future.push(std::mem::replace(&mut self.doc, prev));
            self.saved = self.doc.clone();
            self.status = None;
            cx.notify();
        }
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = self.future.pop() {
            self.past.push(std::mem::replace(&mut self.doc, next));
            self.saved = self.doc.clone();
            self.status = None;
            cx.notify();
        }
    }

    // ---- values -----------------------------------------------------------------------------

    /// Bars move continuously with the mouse; the stored value is what the picture uses.
    fn set_from_x(&mut self, id: usize, x: Pixels) {
        let b = self.tracks.borrow()[id];
        let w = f32::from(b.size.width);
        if w <= 0.0 {
            return;
        }
        let f = ((f32::from(x) - f32::from(b.origin.x)) / w).clamp(0.0, 1.0);
        let (lo, hi) = RANGES[id];
        self.doc.values[id] = lo + (hi - lo) * f;
        self.status = None;
    }

    /// The shadow's direction and size from a point on the direction pad: the way from the
    /// middle is where it falls, the distance is how far.
    fn pick(&mut self, p: gpui::Point<Pixels>) {
        let b = self.picker.get();
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let (rx, ry) = (f32::from(p.x) - f32::from(b.origin.x) - w / 2.0, f32::from(p.y) - f32::from(b.origin.y) - h / 2.0);
        let dist = (rx * rx + ry * ry).sqrt();
        if dist > 1.0 {
            self.doc.values[SH_ANGLE] = ry.atan2(rx).to_degrees().rem_euclid(360.0);
        }
        self.doc.values[SH_SIZE] = (dist / (h / 2.0 - 10.0) * RANGES[SH_SIZE].1).clamp(0.0, RANGES[SH_SIZE].1);
    }

    // ---- fields -----------------------------------------------------------------------------

    fn field_text(&self, i: usize) -> String {
        let (w, h) = self.output();
        if i == WIDTH { w.to_string() } else { h.to_string() }
    }

    /// Applies a field's draft: one side of the output size.
    fn apply_field(&mut self, i: usize) {
        let draft = std::mem::take(&mut self.drafts[i]);
        if let Ok(v) = draft.parse::<u32>() {
            let v = v.clamp(MIN_SIDE, MAX_SIDE);
            let (w, h) = self.output();
            self.doc.size = Some(if i == WIDTH { (v, h) } else { (w, v) });
            self.doc.preset = None;
            self.commit();
        }
    }

    /// Applies whatever was typed in a focused field and hands the keyboard back.
    fn blur_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut typed = false;
        for i in [WIDTH, HEIGHT] {
            if self.fields[i].is_focused(window) {
                self.apply_field(i);
                typed = true;
            }
        }
        if typed {
            window.focus(&self.focus);
            cx.notify();
        }
    }

    fn on_field_key(&mut self, i: usize, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let ctrl = k.modifiers.control;
        match k.key.as_str() {
            "enter" => {
                self.apply_field(i);
                window.focus(&self.focus);
            }
            "escape" => {
                self.drafts[i].clear();
                window.focus(&self.focus);
            }
            "backspace" => {
                self.drafts[i].pop();
            }
            "c" if ctrl => {
                let text = if self.drafts[i].is_empty() { self.field_text(i) } else { self.drafts[i].clone() };
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            "v" if ctrl => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.drafts[i] = text.chars().filter(|c| c.is_ascii_digit()).take(4).collect();
                }
            }
            _ if !ctrl => {
                if let Some(ch) = k.key_char.as_ref().and_then(|s| s.chars().next()) {
                    if ch.is_ascii_digit() && self.drafts[i].len() < 4 {
                        self.drafts[i].push(ch);
                    }
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// A number field: a small label, the value, and its unit. Clicking it keeps the current
    /// number for editing.
    fn field(&self, i: usize, label: &'static str, unit: &'static str, t: &Theme, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.fields[i].is_focused(window);
        let text = if focused || !self.drafts[i].is_empty() { self.drafts[i].clone() } else { self.field_text(i) };
        let handle = self.fields[i].clone();
        div()
            .id(ElementId::NamedInteger("field".into(), i as u64))
            .track_focus(&self.fields[i])
            .flex_1()
            .min_w_0()
            .h(px(34.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(9.))
            .bg(t.field)
            .border_1()
            .border_color(if focused { t.accent } else { gpui::transparent_black() })
            .cursor(CursorStyle::IBeam)
            .child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text3).child(label))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .font_family(MONO)
                    .text_size(px(13.))
                    .text_color(t.text)
                    .child(text)
                    .when(focused, |d| d.child(div().ml(px(1.)).w(px(1.5)).h(px(16.)).bg(t.accent))),
            )
            .child(div().text_size(px(11.)).text_color(t.text3).child(unit))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.drafts[i] = this.field_text(i);
                    window.focus(&handle);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| this.on_field_key(i, e, window, cx)))
    }

    // ---- view -------------------------------------------------------------------------------

    /// The preview rectangle in window coordinates (mirrors the layout in `render`).
    fn stage_rect(window: &Window) -> Bounds<Pixels> {
        let vp = window.viewport_size();
        let w = f32::from(vp.width) - 2.0 * GAP;
        let h = f32::from(vp.height) - BAR_H - DOCK_H - 2.0 * GAP;
        Bounds::new(point(px(GAP), px(BAR_H)), size(px(w.max(1.)), px(h.max(1.))))
    }

    /// Logical pixels per output pixel: the whole page fits the preview, never past 1:1.
    fn scale(&self, window: &Window) -> f32 {
        let (ow, oh) = self.output();
        let st = Self::stage_rect(window);
        ((f32::from(st.size.width) - 64.0) / ow as f32)
            .min((f32::from(st.size.height) - 64.0) / oh as f32)
            .min(1.0 / window.scale_factor())
            .max(0.001)
    }

    fn on_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let held = e.pressed_button == Some(MouseButton::Left);
        if let Some(id) = self.dragging {
            if held {
                self.set_from_x(id, e.position.x);
            } else {
                self.dragging = None;
            }
            cx.notify();
        }
        if self.picking {
            if held {
                self.pick(e.position);
            } else {
                self.picking = false;
            }
            cx.notify();
        }
        if let Some((start, (x0, y0))) = self.pad_drag {
            if held {
                // Pad pixels to page fractions; the shot may leave the frame, up to a full page.
                let (_, _, fw, fh) = self.pad_frame();
                self.doc.values[POS_X] = (x0 + f32::from(e.position.x - start.x) / fw).clamp(-1.0, 1.0);
                self.doc.values[POS_Y] = (y0 + f32::from(e.position.y - start.y) / fh).clamp(-1.0, 1.0);
            } else {
                self.pad_drag = None;
            }
            cx.notify();
        }
        if let Some((y0, off0)) = self.bar_drag {
            if held {
                // The thumb follows the mouse: its travel maps onto the whole scroll range.
                let (track, thumb, max) = bar_metrics(&self.panel_scroll);
                let dy = f32::from(e.position.y) - y0;
                let off = (off0 + dy * max / (track - thumb).max(1.0)).clamp(0.0, max);
                self.panel_scroll.set_offset(point(px(0.), px(-off)));
            } else {
                self.bar_drag = None;
            }
            cx.notify();
        }
    }

    /// The page frame inside the position pad: (left, top, width, height) in pad pixels.
    fn pad_frame(&self) -> (f32, f32, f32, f32) {
        let (ow, oh) = self.output();
        let aspect = ow as f32 / oh.max(1) as f32;
        let fw = (PAD_W - 48.0).min((PAD_H - 36.0) * aspect);
        let fh = fw / aspect;
        ((PAD_W - fw) / 2.0, (PAD_H - fh) / 2.0, fw, fh)
    }

    /// The shot's box on the page (header included), in output pixels.
    fn shot_box(&self) -> (f32, f32) {
        let style = self.style();
        let f = compose::fit(self.shot.width, self.shot.height, &style);
        (self.shot.width as f32 * f, self.shot.height as f32 * f + (style.header_h() * f).round())
    }

    /// Every gesture ends here: drags stop and the result becomes one undo step.
    fn on_up(&mut self, cx: &mut Context<Self>) {
        self.dragging = None;
        self.picking = false;
        self.pad_drag = None;
        self.bar_drag = None;
        self.just_closed = None;
        self.commit();
        cx.notify();
    }

    /// Closes the editor, freeing its GPU images first (the native close path does the same).
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_hero(window);
        for image in self.gpu.borrow().iter() {
            let _ = window.drop_image(image.clone());
        }
        crate::trim_soon(cx);
        window.remove_window();
    }

    // ---- export -----------------------------------------------------------------------------

    fn export(&mut self, save: bool, cx: &mut Context<Self>) {
        self.menu = None;
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = Some(if save { "Saving…" } else { "Copying…" }.into());
        cx.notify();
        let shot = self.shot.clone();
        let style = self.style();
        cx.spawn(async move |this, cx| {
            let message = cx
                .background_executor()
                .spawn(async move {
                    let (w, h, rgba) = compose::compose(&shot, &style);
                    if save {
                        let dir = crate::win::save_dir();
                        let _ = std::fs::create_dir_all(&dir);
                        let path = dir.join(format!("Shoter_{}.png", crate::win::timestamp()));
                        match std::fs::write(&path, compose::encode_png(w, h, &rgba)) {
                            Ok(()) => "Saved to Pictures\\Shoter".to_string(),
                            Err(e) => format!("Couldn't save: {e}"),
                        }
                    } else if crate::win::clipboard::set_image(w, h, &rgba) {
                        "Copied to clipboard".to_string()
                    } else {
                        "Clipboard is busy, try again".to_string()
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.status = Some(message.into());
                cx.notify();
            });
        })
        .detach();
    }

    // ---- panels -----------------------------------------------------------------------------

    fn is_open(&self, id: u8) -> bool {
        self.menu == Some(id)
    }

    /// A bottom-row button that opens `panel` above it. Panels are only built while open, fade
    /// and rise in, and close on a press outside or Esc.
    fn panel_button(&self, id: u8, trigger: impl IntoElement, panel: Option<AnyElement>, primary: bool, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let open = self.is_open(id);
        let button = div()
            .id(ElementId::NamedInteger("panel-button".into(), id as u64))
            .size_full()
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(12.))
            .rounded(px(12.))
            .border_1()
            .cursor(CursorStyle::PointingHand)
            .map(|d| {
                if primary {
                    d.bg(t.accent).border_color(t.accent).text_color(t.on_accent).hover(|d| d.opacity(0.92))
                } else {
                    d.bg(if open { t.field } else { t.panel }).border_color(if open { t.accent } else { t.border }).hover(|d| d.bg(t.field))
                }
            })
            .child(trigger)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if this.just_closed.take() != Some(id) {
                        this.menu = if this.menu == Some(id) { None } else { Some(id) };
                        this.panel_scroll.set_offset(point(px(0.), px(0.)));
                    }
                    cx.notify();
                }),
            );
        let mut root = div().relative().size_full().child(button);
        if let (true, Some(panel)) = (open, panel) {
            let card = div()
                .id(ElementId::NamedInteger("panel".into(), id as u64))
                .p(px(12.))
                .pr(px(4.))
                .rounded(px(14.))
                .bg(t.panel)
                .border_1()
                .border_color(t.hud_border)
                .shadow(vec![BoxShadow {
                    color: hsla(0., 0., 0., if t.dark { 0.5 } else { 0.16 }),
                    offset: point(px(0.), px(12.)),
                    blur_radius: px(32.),
                    spread_radius: px(0.),
                }])
                .text_size(px(12.))
                .text_color(t.text)
                // Presses, drags and wheel turns inside the panel never reach the preview.
                .occlude()
                // The panel blocks the window's own handlers, so it follows drags and releases itself.
                .on_mouse_move(cx.listener(Self::on_move))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.on_up(cx)))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.blur_fields(window, cx)))
                .on_mouse_down_out(cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    if this.menu == Some(id) {
                        this.menu = None;
                        this.just_closed = Some(id);
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .relative()
                        .child(div().id("panel-scroll").max_h(px(PANEL_MAX_H)).overflow_y_scroll().track_scroll(&self.panel_scroll).pr(px(10.)).child(panel))
                        .child(scrollbar(&self.panel_scroll, t.text3.opacity(0.45), t.panel))
                        // A wider grip over the thumb: drag it, or press the track to jump there.
                        .child(div().id("panel-bar").absolute().top(px(8.)).bottom(px(8.)).right_0().w(px(10.)).cursor(CursorStyle::Arrow).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, e: &MouseDownEvent, _, cx| {
                                let (track, thumb, max) = bar_metrics(&this.panel_scroll);
                                if max <= 0.0 {
                                    return;
                                }
                                let top = f32::from(this.panel_scroll.bounds().origin.y) + 8.0;
                                let mut off = -f32::from(this.panel_scroll.offset().y);
                                let at = f32::from(e.position.y) - top;
                                let y = off / max * (track - thumb);
                                if at < y || at > y + thumb {
                                    off = ((at - thumb / 2.0) / (track - thumb).max(1.0) * max).clamp(0.0, max);
                                    this.panel_scroll.set_offset(point(px(0.), px(-off)));
                                }
                                this.bar_drag = Some((f32::from(e.position.y), off));
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )),
                )
                .with_animation(
                    ElementId::NamedInteger("panel-in".into(), id as u64),
                    Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint()),
                    |card, k| card.opacity(k).mb(px((1.0 - k) * 8.0)),
                );
            root = root.child(
                div().absolute().top_0().left_0().child(deferred(
                    anchored().anchor(Corner::BottomLeft).offset(point(px(0.), px(-10.))).snap_to_window_with_margin(px(10.)).child(card),
                )),
            );
        }
        root.into_any_element()
    }

    /// The ruler bar: name and value above, a hairline over fine tick marks, and a slim upright
    /// handle that follows the mouse exactly.
    fn slider(&self, id: usize, label: &'static str, shown: String, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let (lo, hi) = RANGES[id];
        let f = ((self.doc.values[id] - lo) / (hi - lo)).clamp(0.0, 1.0);
        let tracks = self.tracks.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_size(px(12.))
                    .child(div().text_color(t.text2).child(label))
                    .child(div().font_family(MONO).text_color(t.text).child(shown)),
            )
            .child(
                div()
                    .id(ElementId::NamedInteger("slider".into(), id as u64))
                    .relative()
                    .w_full()
                    .h(px(26.))
                    .cursor(CursorStyle::ResizeLeftRight)
                    // Everything runs on a track inset by half the handle, so the handle stays whole
                    // at both ends and the value under it is exactly the mouse's.
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(px(3.))
                            .right(px(3.))
                    .child(canvas(move |bounds, _, _| tracks.borrow_mut()[id] = bounds, |_, _, _, _| {}).absolute().size_full())
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(10.))
                            .h(px(2.))
                            .rounded(px(1.))
                            .bg(t.field)
                            .child(div().h_full().w(relative(f)).rounded(px(1.)).bg(t.accent)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(17.))
                            .flex()
                            .justify_between()
                            .children((0..=20).map(|k| div().w(px(1.)).h(px(if k % 5 == 0 { 7. } else { 4. })).bg(t.text3.opacity(0.6)))),
                    )
                    .child(div().absolute().top(px(2.)).left(relative(f)).ml(px(-3.)).w(px(6.)).h(px(18.)).rounded(px(3.)).bg(t.text)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                            this.dragging = Some(id);
                            this.set_from_x(id, e.position.x);
                            cx.notify();
                        }),
                    ),
            )
    }

    /// A selectable tile: all the same size and look; the picked one only gains an accent ring.
    fn tile(&self, id: &'static str, n: usize, on: bool, t: &Theme) -> gpui::Stateful<gpui::Div> {
        div()
            .id(ElementId::NamedInteger(id.into(), n as u64))
            .relative()
            .h(px(64.))
            .rounded(px(5.))
            .overflow_hidden()
            .border_2()
            .border_color(if on { t.accent } else { gpui::transparent_black() })
            .cursor(CursorStyle::PointingHand)
    }

    /// A tile labelled underneath.
    fn labelled(&self, tile: gpui::Stateful<gpui::Div>, label: &'static str, on: bool, t: &Theme) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(tile)
            .child(div().text_size(px(10.)).text_center().truncate().text_color(if on { t.text } else { t.text2 }).child(label))
    }

    /// A gradient tile's thumbnail, rendered the first time it's shown and kept for the window.
    fn thumb(&self, i: usize) -> Arc<RenderImage> {
        self.thumbs
            .borrow_mut()
            .entry(i)
            .or_insert_with(|| {
                let t = bgra(swap_rb(compose::backdrop(GRADIENTS[i], 272, 52, 0.0)));
                self.gpu.borrow_mut().push(t.clone());
                t
            })
            .clone()
    }

    /// Renders the backdrop at preview size with the export's own code (any number of stops,
    /// the same blur), freeing the previous one. Gradients are smooth, so a small render
    /// stretched to the preview looks the same and keeps Blur drags fluid.
    fn prepare_hero(&mut self, window: &mut Window) {
        let (ow, oh) = self.output();
        let k = (640.0 / ow.max(oh) as f32).min(1.0);
        let blur = (self.doc.values[BLUR] * 100.0).round() as u32;
        let key = (self.doc.backdrop, blur, ((ow as f32 * k) as u32).max(16), ((oh as f32 * k) as u32).max(16));
        if self.hero.as_ref().map(|h| h.key) != Some(key) {
            let image = bgra(swap_rb(compose::backdrop(GRADIENTS[key.0], key.2, key.3, blur as f32 / 100.0)));
            if let Some(old) = self.hero.replace(Hero { key, image }) {
                let _ = window.drop_image(old.image);
            }
        }
    }

    fn drop_hero(&mut self, window: &mut Window) {
        if let Some(old) = self.hero.take() {
            let _ = window.drop_image(old.image);
        }
    }
    /// The page as it will be exported, scaled by `k`.
    fn composition(&self, k: f32) -> impl IntoElement {
        let style = self.style();
        let (ow, oh) = self.output();
        let f = compose::fit(self.shot.width, self.shot.height, &style);
        let u = self.shot.scale * f;
        let edge = &EDGES[style.edge];
        let rw = style.ring() * f;
        let hh = (style.header_h() * f).round();
        let (sw, sh) = (self.shot.width as f32 * f, self.shot.height as f32 * f);
        let r = (style.radius * f).min(sw.min(sh + hh) / 2.0);
        let ro = if r > 0.0 { r + rw } else { 0.0 };
        let s = style.shadow;
        let (sx, sy) = s.offset();
        let shadow = (s.opacity > 0.0)
            .then(|| BoxShadow {
                color: hsla(0., 0., 0., s.opacity),
                offset: point(px(sx * u * k), px(sy * u * k)),
                blur_radius: px(s.blur * u * k),
                spread_radius: px(0.),
            })
            .into_iter()
            .collect::<Vec<_>>();
        // Placed exactly as the export places it, clipped to the page.
        let left = (compose::place(ow as f32, sw, style.offset.0) - rw) * k;
        let top = (compose::place(oh as f32, sh + hh, style.offset.1) - rw) * k;
        let head = &HEADERS[style.header];
        let header = (hh > 0.0).then(|| {
            let mid = hh * k / 2.0;
            let mut bar = div().relative().w_full().h(px(hh * k)).flex_none().rounded_t(px(r * k)).bg(rgb(head.bar));
            if head.lights {
                for (i, &col) in LIGHTS.iter().enumerate() {
                    let d = 12.0 * u * k;
                    bar = bar.child(div().absolute().left(px((16.0 + 20.0 * i as f32) * u * k - d / 2.0)).top(px(mid - d / 2.0)).size(px(d)).rounded_full().bg(rgb(col)));
                }
            }
            if let Some(col) = head.address {
                let pw = (sw * 0.46).min(440.0 * u) * k;
                let ph = hh * 0.58 * k;
                bar = bar.child(div().absolute().left(px((sw * k - pw) / 2.0)).top(px(mid - ph / 2.0)).w(px(pw)).h(px(ph)).rounded(px(ph / 2.0)).bg(rgb(col)));
            }
            if let Some(col) = head.controls {
                for (i, glyph) in ["\u{E921}", "\u{E922}", "\u{E8BB}"].into_iter().enumerate() {
                    let cx_ = sw * k - (102.0 - 40.0 * i as f32) * u * k;
                    bar = bar.child(
                        div()
                            .absolute()
                            .left(px(cx_ - 8.0 * u * k))
                            .top(px(mid - 8.0 * u * k))
                            .size(px(16.0 * u * k))
                            .flex()
                            .items_center()
                            .justify_center()
                            .font_family(ICONS)
                            .text_size(px(10.0 * u * k))
                            .text_color(rgb(col))
                            .child(glyph),
                    );
                }
            }
            bar
        });
        div()
            .relative()
            .w(px(ow as f32 * k))
            .h(px(oh as f32 * k))
            .flex_none()
            .overflow_hidden()
            // The backdrop, rendered by the export's own code (see `prepare_hero`).
            .when_some(self.hero.as_ref(), |d, h| d.child(img(h.image.clone()).absolute().top_0().left_0().size_full().object_fit(ObjectFit::Fill)))
            .when(style.noise > 0.0, |d| d.child(img(self.grain.clone()).absolute().top_0().left_0().size_full().object_fit(ObjectFit::Cover).opacity(style.noise)))
            .when(self.pad_drag.is_some(), |d| d.child(guides("grid-preview", guide_color(style.gradient), grid_cells(ow, oh))))
            .child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .p(px(rw * k))
                    .rounded(px(ro * k))
                    .bg(rgba((edge.color << 8) | (edge.alpha * 255.0) as u32))
                    .shadow(shadow)
                    .child(
                        div()
                            .w(px(sw * k))
                            .flex()
                            .flex_col()
                            .children(header)
                            .child(
                                img(self.image.clone())
                                    .w(px(sw * k))
                                    .h(px(sh * k))
                                    .map(|i| if hh > 0.0 { i.rounded_b(px(r * k)) } else { i.rounded(px(r * k)) }),
                            ),
                    ),
            )
    }

    // ---- the panels -------------------------------------------------------------------------

    fn page_panel(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let card = |id: usize, name: &'static str, w: u32, h: u32, on: bool| {
            div()
                .id(ElementId::NamedInteger("size-card".into(), id as u64))
                .flex()
                .flex_col()
                .gap(px(3.))
                .min_w_0()
                .cursor(CursorStyle::PointingHand)
                .child(
                    div()
                        .h(px(56.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(10.))
                        .bg(t.field)
                        .border_1()
                        .border_color(if on { t.accent } else { t.border })
                        .when(on, |d| d.border_2())
                        .hover(|d| d.border_color(t.accent))
                        .child(aspect_box(w, h, 64., 38., if on { t.accent } else { t.text3 }, t.panel)),
                )
                .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_center().truncate().child(name))
                .child(div().font_family(MONO).text_size(px(10.)).text_center().text_color(t.text3).truncate().child(format!("{w}×{h}")))
        };
        let (nw, nh) = compose::output_size(self.shot.width, self.shot.height, &Style { size: None, ..self.style() });
        let mut groups: Vec<AnyElement> = vec![div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(heading("Fit", t))
            .child(
                div().grid().grid_cols(3).gap(px(8.)).child(card(9999, "Fit to shot", nw, nh, self.doc.size.is_none()).on_click(cx.listener(|this, _, _, cx| {
                    this.doc.size = None;
                    this.doc.preset = None;
                    this.drafts = Default::default();
                    cx.notify();
                }))),
            )
            .into_any_element()];
        for (gi, (group, items)) in SIZE_GROUPS.iter().enumerate() {
            let cards: Vec<AnyElement> = items
                .iter()
                .enumerate()
                .map(|(i, &(name, w, h))| {
                    card(gi * 100 + i, name, w, h, self.doc.preset == Some((gi, i)))
                        // Stays open: trying sizes one after another is the point.
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.doc.size = Some((w, h));
                            this.doc.preset = Some((gi, i));
                            this.drafts = Default::default();
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect();
            groups.push(div().flex().flex_col().gap(px(8.)).child(heading(group, t)).child(div().grid().grid_cols(3).gap(px(8.)).children(cards)).into_any_element());
        }
        let v = &self.doc.values;
        div()
            .w(px(PAD_W + 8.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(div().flex().gap(px(6.)).child(self.field(WIDTH, "W", "px", t, window, cx)).child(self.field(HEIGHT, "H", "px", t, window, cx)))
            .child(self.slider(SCALE, "Scale", format!("{}%", (v[SCALE] * 100.0).round() as i32), t, cx))
            .child(self.position_pad(t, cx))
            .child(div().h(px(1.)).bg(t.border))
            .children(groups)
            .into_any_element()
    }

    fn background_panel(&self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let v = &self.doc.values;
        // Every tile is the same size and look; the picked one only gains a ring drawn inside it.
        let tiles: Vec<AnyElement> = (0..GRADIENTS.len())
            .map(|i| {
                div()
                    .id(ElementId::NamedInteger("bg".into(), i as u64))
                    .relative()
                    .h(px(52.))
                    .rounded(px(5.))
                    .overflow_hidden()
                    .cursor(CursorStyle::PointingHand)
                    .child(img(self.thumb(i)).size_full().rounded(px(5.)).object_fit(ObjectFit::Fill))
                    .when(self.doc.backdrop == i, |d| d.child(div().absolute().top_0().left_0().size_full().rounded(px(5.)).border_2().border_color(t.accent)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.doc.backdrop = i;
                        this.status = None;
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let percent = |x: f32| format!("{}%", (x * 100.0).round() as i32);
        div()
            .w(px(PAD_W + 8.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(self.slider(BLUR, "Blur", percent(v[BLUR]), t, cx))
            .child(self.slider(NOISE, "Noise", percent(v[NOISE]), t, cx))
            .child(div().h(px(1.)).bg(t.border))
            .child(div().flex().flex_col().gap(px(8.)).children(tiles))
            .into_any_element()
    }

    fn edge_panel(&self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let v = &self.doc.values;
        let radii: Vec<AnyElement> = RADII
            .iter()
            .enumerate()
            .map(|(i, &(name, r))| {
                let on = (v[RADIUS] - r).abs() < 0.5;
                self.labelled(
                    self.tile("radius", i, on, t).bg(corner_bg()).child(corner_frame(r, 0, 0, 0)).on_click(cx.listener(move |this, _, _, cx| {
                        this.doc.values[RADIUS] = r;
                        cx.notify();
                    })),
                    name,
                    on,
                    t,
                )
                .into_any_element()
            })
            .collect();
        let strokes: Vec<AnyElement> = EDGES
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let on = self.doc.edge == i;
                self.labelled(
                    self.tile("stroke", i, on, t).bg(corner_bg()).child(corner_frame(10.0, i, 0, 0)).on_click(cx.listener(move |this, _, _, cx| {
                        this.doc.edge = i;
                        this.doc.values[EDGE_W] = EDGES[i].width;
                        cx.notify();
                    })),
                    e.name,
                    on,
                    t,
                )
                .into_any_element()
            })
            .collect();
        let headers: Vec<AnyElement> = HEADERS
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let on = self.doc.header == i;
                self.labelled(
                    self.tile("header", i, on, t).bg(corner_bg()).child(corner_frame(10.0, 0, 0, i)).on_click(cx.listener(move |this, _, _, cx| {
                        this.doc.header = i;
                        cx.notify();
                    })),
                    h.name,
                    on,
                    t,
                )
                .into_any_element()
            })
            .collect();
        div()
            .w(px(PAD_W + 8.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(self.slider(RADIUS, "Radius", format!("{}", v[RADIUS].round() as i32), t, cx))
            .child(self.slider(EDGE_W, "Stroke", format!("{:.0} px", v[EDGE_W]), t, cx))
            .child(div().h(px(1.)).bg(t.border))
            .child(div().flex().flex_col().gap(px(6.)).child(heading("Radius", t)).child(div().grid().grid_cols(2).gap(px(8.)).children(radii)))
            .child(div().flex().flex_col().gap(px(6.)).child(heading("Stroke", t)).child(div().grid().grid_cols(2).gap(px(8.)).children(strokes)))
            .child(div().flex().flex_col().gap(px(6.)).child(heading("Header", t)).child(div().grid().grid_cols(2).gap(px(8.)).children(headers)))
            .into_any_element()
    }

    fn shadow_panel(&self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let v = &self.doc.values;
        let presets: Vec<AnyElement> = SHADOWS
            .iter()
            .enumerate()
            .map(|(i, &(name, sz, bl, op))| {
                let on = (v[SH_SIZE] - sz).abs() < 0.5 && (v[SH_BLUR] - bl).abs() < 0.5 && (v[SH_OPACITY] - op).abs() < 0.01;
                self.labelled(
                    self.tile("shadow", i, on, t).bg(corner_bg()).child(shadow_mini(v[SH_ANGLE], sz, bl, op, 1.0)).on_click(cx.listener(move |this, _, _, cx| {
                        this.doc.values[SH_SIZE] = sz;
                        this.doc.values[SH_BLUR] = bl;
                        this.doc.values[SH_OPACITY] = op;
                        cx.notify();
                    })),
                    name,
                    on,
                    t,
                )
                .into_any_element()
            })
            .collect();
        div()
            .w(px(PAD_W + 8.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(self.direction_pad(t, cx))
            .child(self.slider(SH_SIZE, "Size", format!("{:.0}", v[SH_SIZE]), t, cx))
            .child(self.slider(SH_BLUR, "Blur", format!("{:.0}", v[SH_BLUR]), t, cx))
            .child(self.slider(SH_OPACITY, "Opacity", format!("{}%", (v[SH_OPACITY] * 100.0).round() as i32), t, cx))
            .child(div().h(px(1.)).bg(t.border))
            .child(div().grid().grid_cols(2).gap(px(8.)).children(presets))
            .into_any_element()
    }

    /// Which way the shadow falls: drag the handle anywhere on the pad, and its distance from
    /// the middle is the shadow's size. The card in the middle shows the result; double-click
    /// resets to straight down.
    fn direction_pad(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let v = &self.doc.values;
        let bounds = self.picker.clone();
        let (mx, my) = (PAD_W / 2.0, SHADOW_PAD_H / 2.0);
        let reach = SHADOW_PAD_H / 2.0 - 10.0;
        // Pad pixels per logical pixel of shadow.
        let k = reach / RANGES[SH_SIZE].1;
        let a = v[SH_ANGLE].to_radians();
        let (hx, hy) = (mx + a.cos() * v[SH_SIZE] * k, my + a.sin() * v[SH_SIZE] * k);
        let card = div()
            .absolute()
            .left(px(mx - 32.))
            .top(px(my - 20.))
            .w(px(64.))
            .h(px(40.))
            .rounded(px(5.))
            .bg(rgb(0xf4f4f5))
            .when(v[SH_OPACITY] > 0.0, |d| {
                d.shadow(vec![BoxShadow {
                    // Distance is exaggerated so the direction reads; blur is kept tight so the
                    // shadow stays visible on a card this small.
                    color: hsla(0., 0., 0., (v[SH_OPACITY] * 1.6).min(1.0)),
                    offset: point(px(a.cos() * v[SH_SIZE] * k * 0.5), px(a.sin() * v[SH_SIZE] * k * 0.5)),
                    blur_radius: px(v[SH_BLUR] * 0.18),
                    spread_radius: px(0.),
                }])
            });
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(div().flex().justify_between().text_size(px(12.)).child(div().text_color(t.text2).child("Direction")).child(div().text_size(px(11.)).text_color(t.text3).child("double-click to reset")))
            .child(
                div()
                    .id("shadow-pad")
                    .relative()
                    .mx_auto()
                    .w(px(PAD_W))
                    .h(px(SHADOW_PAD_H))
                    .rounded(px(5.))
                    .overflow_hidden()
                    // A light backdrop, like the style tiles, so a black shadow shows in either theme.
                    .bg(corner_bg())
                    .border_1()
                    .border_color(t.border)
                    .cursor(CursorStyle::Crosshair)
                    .child(canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).absolute().size_full())
                    .child(guides("grid-shadow", hsla(0., 0., 0., 0.07), grid_cells(PAD_W as u32, SHADOW_PAD_H as u32)))
                    .child(card)
                    .child(div().absolute().left(px(hx - 5.)).top(px(hy - 5.)).size(px(10.)).rounded_full().bg(t.accent).border_2().border_color(rgb(0xffffff)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, e: &MouseDownEvent, _, cx| {
                            if e.click_count >= 2 {
                                this.doc.values[SH_ANGLE] = 90.0;
                                this.doc.values[SH_SIZE] = 16.0;
                                this.picking = false;
                            } else {
                                this.picking = true;
                                this.pick(e.position);
                            }
                            cx.notify();
                        }),
                    ),
            )
    }

    /// Where the shot sits: the page frame and the shot on a faint grid. Drag the shot anywhere,
    /// even partly or fully off the page; double-click puts it back in the middle.
    fn position_pad(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let style = self.style();
        let (ow, oh) = self.output();
        let (bw, bh) = self.shot_box();
        let (fl, ft, fw, fh) = self.pad_frame();
        let (sx, sy) = (fw / ow as f32, fh / oh as f32);
        let (w, h) = (bw * sx, bh * sy);
        let x = fl + compose::place(ow as f32, bw, style.offset.0) * sx;
        let y = ft + compose::place(oh as f32, bh, style.offset.1) * sy;
        let dragging = self.pad_drag.is_some();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(div().flex().justify_between().text_size(px(12.)).child(div().text_color(t.text2).child("Position")).child(div().text_size(px(11.)).text_color(t.text3).child("double-click to center")))
            .child(
                div()
                    .id("pad")
                    .relative()
                    .mx_auto()
                    .w(px(PAD_W))
                    .h(px(PAD_H))
                    .rounded(px(5.))
                    .overflow_hidden()
                    .bg(t.field)
                    .border_1()
                    .border_color(t.border)
                    .cursor(if dragging { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })

                    // The page: a plain sheet.
                    .child(
                        div()
                            .absolute()
                            .left(px(fl))
                            .top(px(ft))
                            .w(px(fw))
                            .h(px(fh))
                            .rounded(px(3.))
                            .bg(t.panel)
                            .border_1()
                            .border_color(t.text.opacity(0.12))
                            .when(dragging, |d| d.child(guides("grid-pad", t.text.opacity(0.1), grid_cells(ow, oh)))),
                    )
                    // The shot: a neutral block with a small accent handle.
                    .child(
                        div()
                            .absolute()
                            .left(px(x))
                            .top(px(y))
                            .w(px(w))
                            .h(px(h))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(2.))
                            .bg(t.text.opacity(if dragging { 0.24 } else { 0.16 }))
                            .child(div().size(px(10.)).flex_none().rounded_full().bg(t.accent).border_2().border_color(t.panel)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, e: &MouseDownEvent, _, cx| {
                            if e.click_count >= 2 {
                                this.doc.values[POS_X] = 0.0;
                                this.doc.values[POS_Y] = 0.0;
                                this.pad_drag = None;
                            } else {
                                this.pad_drag = Some((e.position, (this.doc.values[POS_X], this.doc.values[POS_Y])));
                            }
                            cx.notify();
                        }),
                    ),
            )
    }

    fn export_panel(&self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let row = |id: &'static str, glyph: &'static str, label: &'static str, hint: &'static str| {
            div()
                .id(id)
                .h(px(40.))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(10.))
                .rounded(px(9.))
                .cursor(CursorStyle::PointingHand)
                .hover(|d| d.bg(t.field))
                .child(div().w(px(16.)).font_family(ICONS).text_size(px(13.)).child(glyph))
                .child(div().flex_1().text_size(px(13.)).child(label))
                .child(div().text_size(px(11.)).text_color(t.text3).child(hint))
        };
        div()
            .w(px(250.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(row("copy", "\u{E8C8}", "Copy", "clipboard").on_click(cx.listener(|this, _, _, cx| this.export(false, cx))))
            .child(row("save", "\u{E74E}", "Save PNG", "Pictures\\Shoter").on_click(cx.listener(|this, _, _, cx| this.export(true, cx))))
            .into_any_element()
    }
}

/// The grey backdrop of the style previews.
fn corner_bg() -> Fill {
    linear_gradient(160., linear_color_stop(rgb(0xdcdcde), 0.), linear_color_stop(rgb(0x9e9ea4), 1.))
}

/// The corner of a framed card, as in a style preview: the card sits off to the bottom right so
/// its top-left corner shows the radius, the edge, the shadow depth and the window header.
fn corner_frame(radius: f32, edge: usize, shadow: usize, header: usize) -> gpui::Div {
    let e = &EDGES[edge];
    let ring = (e.width * 0.8).round();
    let r = radius * 0.9;
    let head = &HEADERS[header];
    div()
        .absolute()
        .left(relative(0.28))
        .top(relative(0.22))
        .w_full()
        .h_full()
        .p(px(ring))
        .rounded(px(if r > 0.0 { r + ring } else { 0.0 }))
        .bg(rgba((e.color << 8) | (e.alpha * 255.0) as u32))
        .when(shadow > 0, |d| {
            d.shadow(vec![BoxShadow {
                color: hsla(0., 0., 0., 0.12 + 0.1 * shadow as f32),
                offset: point(px(0.), px(shadow as f32 * 2.0)),
                blur_radius: px(shadow as f32 * 5.0),
                spread_radius: px(0.),
            }])
        })
        .child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .rounded(px(r))
                .overflow_hidden()
                .bg(rgb(0xf4f4f5))
                .when(header > 0, |d| {
                    d.child(
                        div()
                            .h(px(12.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(3.))
                            .px(px(6.))
                            .bg(rgb(head.bar))
                            .when(head.lights, |d| d.children(LIGHTS.iter().map(|&c| div().size(px(4.)).rounded_full().bg(rgb(c)))))
                            .when_some(head.address, |d, c| d.child(div().ml(px(4.)).w(px(28.)).h(px(5.)).rounded(px(3.)).bg(rgb(c))))
                            .when_some(head.controls, |d, c| d.child(div().flex_1()).children((0..3).map(move |_| div().size(px(4.)).bg(rgb(c))))),
                    )
                }),
        )
}

/// A shadow preview: a card casting the preset's shadow in the chosen direction, `scale` times
/// the size of a style tile's (the bottom button's chip is much smaller).
fn shadow_mini(angle: f32, size: f32, blur: f32, opacity: f32, scale: f32) -> gpui::Div {
    let a = angle.to_radians();
    div().absolute().top_0().left_0().size_full().flex().items_center().justify_center().child(
        div()
            .w(px(56. * scale))
            .h(px(34. * scale))
            .rounded(px(5. * scale))
            .bg(rgb(0xf4f4f5))
            .when(opacity > 0.0, |d| {
                d.shadow(vec![BoxShadow {
                    color: hsla(0., 0., 0., (opacity * 1.6).min(1.0)),
                    offset: point(px(a.cos() * size * 0.25 * scale), px(a.sin() * size * 0.25 * scale)),
                    blur_radius: px(blur * 0.12 * scale),
                    spread_radius: px(0.),
                }])
            }),
    )
}

/// Columns and rows that cut a `w`×`h` page into whole, equal squares: 4 to 16 across the long
/// side, preferring an exact fit (16:9 → 16×9, 1:1 → 8×8) and otherwise the squarest cells.
fn grid_cells(w: u32, h: u32) -> (u32, u32) {
    let (long, short) = (w.max(h).max(1) as f32, w.min(h).max(1) as f32);
    let (n, m) = (4..=16u32)
        .map(|n| {
            let exact = short * n as f32 / long;
            let m = exact.round().max(1.0);
            // Off-square error dominates; among equals, stay near 8 across.
            let score = (exact - m).abs() / m * 10.0 + n.abs_diff(8) as f32 * 0.01;
            (n, m as u32, score)
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(n, m, _)| (n, m))
        .unwrap_or((8, 8));
    if w >= h { (n, m) } else { (m, n) }
}

/// The positioning grid over a page: `cols`×`rows` equal squares drawn as hairlines exactly one
/// device pixel wide, in one quiet colour, and faded in so it never pops.
fn guides(id: &'static str, color: Hsla, (cols, rows): (u32, u32)) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .child(
            canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    let s = window.scale_factor();
                    let snap = |v: Pixels| px((f32::from(v) * s).round() / s);
                    let hair = px(1.0 / s);
                    for k in 1..cols {
                        let x = snap(b.origin.x + b.size.width * (k as f32 / cols as f32));
                        window.paint_quad(fill(Bounds::new(point(x, b.origin.y), size(hair, b.size.height)), color));
                    }
                    for k in 1..rows {
                        let y = snap(b.origin.y + b.size.height * (k as f32 / rows as f32));
                        window.paint_quad(fill(Bounds::new(point(b.origin.x, y), size(b.size.width, hair)), color));
                    }
                },
            )
            .size_full(),
        )
        .with_animation(id, Animation::new(Duration::from_millis(120)).with_easing(ease_out_quint()), |d, k| d.opacity(k))
}

/// Grid lines that read on a backdrop without shouting: dark on light gradients, light on dark.
fn guide_color(stops: &[u32]) -> Hsla {
    let l = stops.iter().map(|&c| (0.2126 * ((c >> 16) & 0xff) as f32 + 0.7152 * ((c >> 8) & 0xff) as f32 + 0.0722 * (c & 0xff) as f32) / 255.0).sum::<f32>() / stops.len().max(1) as f32;
    if l > 0.6 { hsla(0., 0., 0., 0.14) } else { hsla(0., 0., 1., 0.22) }
}

/// A group title: small, quiet capitals.
fn heading(text: &'static str, t: &Theme) -> gpui::Div {
    div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text3).child(text.to_uppercase())
}

/// The scrollbar's geometry for `handle`: track length (the view less 8 px at each end), thumb
/// length, and the scroll range. The range is 0 when everything fits.
fn bar_metrics(handle: &ScrollHandle) -> (f32, f32, f32) {
    let view = f32::from(handle.bounds().size.height);
    let max = f32::from(handle.max_offset().height);
    let track = (view - 16.0).max(0.0);
    if max <= 1.0 || track <= 0.0 {
        return (track, track, 0.0);
    }
    (track, (track * view / (view + max)).max(28.0).min(track), max)
}

/// A slim scrollbar that follows `handle` and only appears while there is more to see, and soft
/// fades at the top and bottom edges while there is more to scroll that way.
fn scrollbar(handle: &ScrollHandle, color: Hsla, bg: Hsla) -> impl IntoElement {
    let handle = handle.clone();
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let (track, thumb, max) = bar_metrics(&handle);
            if max <= 0.0 {
                return;
            }
            let off = -f32::from(handle.offset().y);
            let fade = px(28.);
            let clear = Hsla { a: 0., ..bg };
            if off > 1.0 {
                window.paint_quad(fill(Bounds::new(b.origin, size(b.size.width, fade)), linear_gradient(180., linear_color_stop(bg, 0.), linear_color_stop(clear, 1.))));
            }
            if off < max - 1.0 {
                window.paint_quad(fill(Bounds::new(point(b.origin.x, b.origin.y + b.size.height - fade), size(b.size.width, fade)), linear_gradient(0., linear_color_stop(bg, 0.), linear_color_stop(clear, 1.))));
            }
            let y = (off / max).clamp(0.0, 1.0) * (track - thumb);
            let rect = Bounds::new(point(b.origin.x + b.size.width - px(4.), b.origin.y + px(8. + y)), size(px(3.), px(thumb)));
            window.paint_quad(fill(rect, color).corner_radii(px(1.5)));
        },
    )
    .absolute()
    .top_0()
    .bottom_0()
    .left_0()
    .right_0()
}

/// Dragging the header moves the window; a double click maximizes or restores it.
fn drag_window(e: &MouseDownEvent, window: &mut Window) {
    if e.click_count >= 2 {
        crate::win::toggle_maximize(window);
    } else {
        window.start_window_move();
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prepare_hero(window);
        let t = Theme::from(window.appearance());
        let k = self.scale(window);
        let scale = window.scale_factor();

        // Square window controls, as tall as the bar; they act on click themselves.
        let control = |id: &'static str, glyph: &'static str, danger: bool| {
            div()
                .id(id)
                .size(px(BAR_H))
                .flex()
                .items_center()
                .justify_center()
                .font_family(ICONS)
                .text_size(px(9.))
                .text_color(t.text2)
                .cursor(CursorStyle::PointingHand)
                .when(danger, |d| d.hover(|d| d.bg(rgb(0xc42b1c)).text_color(rgb(0xffffff))))
                .when(!danger, |d| d.hover(|d| d.bg(t.field)))
                .child(glyph)
        };
        let top_bar = div()
            .h(px(BAR_H))
            .flex_none()
            .flex()
            .items_center()
            .child(
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    // Whole device pixels to the mark's left edge keep it crisp.
                    .pl(px((14. * scale).round() / scale))
                    .pr(px(10.))
                    .on_mouse_down(MouseButton::Left, |e, window, _| drag_window(e, window))
                    .child(logo(14., t.mark(), scale))
                    .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child("Shoter"))
                    .child(div().text_size(px(13.)).text_color(t.text3).child("/"))
                    .child(div().text_size(px(13.)).text_color(t.text2).child(format!("{} capture", self.shot.kind)))
                    .child(div().h(px(20.)).px(px(7.)).flex().items_center().rounded(px(6.)).bg(t.field).font_family(MONO).text_size(px(11.)).text_color(t.text2).child(self.shot.dims())),
            )
            .child(div().flex_1().h_full().on_mouse_down(MouseButton::Left, |e, window, _| drag_window(e, window)))
            .when_some(self.status.clone(), |d, status| d.child(div().pr(px(12.)).text_size(px(12.)).text_color(t.text2).child(status)))
            .child(
                div()
                    .h_full()
                    .flex()
                    .child(control("min", "\u{E921}", false).on_click(|_, window, _| window.minimize_window()))
                    .child(control("max", if window.is_maximized() { "\u{E923}" } else { "\u{E922}" }, false).on_click(|_, window, _| crate::win::toggle_maximize(window)))
                    .child(control("close", "\u{E8BB}", true).on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))),
            );

        let stage = div()
            .id("stage")
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .rounded(px(16.))
            .bg(t.stage)
            .border_1()
            .border_color(t.border)
            .child(self.composition(k));

        // The row of buttons: each shows what it holds, and opens its panel above.
        let (ow, oh) = self.output();
        let preset = self.doc.preset.map(|(g, i)| &SIZE_GROUPS[g].1[i]);
        // Each button: a preview chip, then a quiet title over the value (cut short if long).
        let label = |title: &'static str, value: String| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_size(px(10.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text3).child(title.to_uppercase()))
                .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(t.text).truncate().child(value))
        };
        let chip = || div().relative().w(px(36.)).h(px(26.)).flex_none().rounded(px(7.)).overflow_hidden().border_1().border_color(t.border);
        let page_panel = self.is_open(M_PAGE).then(|| self.page_panel(&t, window, cx));
        let bg_panel = self.is_open(M_BG).then(|| self.background_panel(&t, cx));
        let edge_panel = self.is_open(M_EDGE).then(|| self.edge_panel(&t, cx));
        let shadow_panel = self.is_open(M_SHADOW).then(|| self.shadow_panel(&t, cx));
        let export_panel = self.is_open(M_EXPORT).then(|| self.export_panel(&t, cx));
        let page_name = preset.map_or(if self.doc.size.is_none() { "Fit to shot" } else { "Custom" }, |p| p.0);
        let edge_name = format!("{} · {}", EDGES[self.doc.edge].name, HEADERS[self.doc.header].name);
        let shadow_name = SHADOWS
            .iter()
            .find(|s| (self.doc.values[SH_SIZE] - s.1).abs() < 0.5 && (self.doc.values[SH_BLUR] - s.2).abs() < 0.5 && (self.doc.values[SH_OPACITY] - s.3).abs() < 0.01)
            .map_or("Custom", |s| s.0);
        let icon = |id: &'static str, glyph: &'static str, enabled: bool| {
            div()
                .id(id)
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(10.))
                .font_family(ICONS)
                .text_size(px(13.))
                .text_color(if enabled { t.text } else { t.text3 })
                .when(enabled, |d| d.cursor(CursorStyle::PointingHand).hover(|d| d.bg(t.field)))
                .child(glyph)
        };
        let dock = div()
            .h(px(DOCK_H))
            .flex_none()
            .flex()
            .gap(px(GAP))
            .child(div().flex_1().min_w_0().child(self.panel_button(
                M_PAGE,
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(div().w(px(36.)).h(px(26.)).flex_none().flex().items_center().justify_center().child(aspect_box(ow, oh, 32., 22., t.text2, t.field)))
                    .child(label("Page size", format!("{page_name} · {ow} × {oh}"))),
                page_panel,
                false,
                &t,
                cx,
            )))
            .child(div().flex_1().min_w_0().child(self.panel_button(
                M_BG,
                div().flex_1().flex().items_center().gap(px(10.)).child(chip().child(img(self.thumb(self.doc.backdrop)).size_full().rounded(px(6.)).object_fit(ObjectFit::Fill))).child(label("Background", format!("Gradient {}", self.doc.backdrop + 1))),
                bg_panel,
                false,
                &t,
                cx,
            )))
            .child(div().flex_1().min_w_0().child(self.panel_button(
                M_EDGE,
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(chip().bg(corner_bg()).child(corner_frame(self.doc.values[RADIUS], self.doc.edge, 0, self.doc.header)))
                    .child(label("Edge", edge_name)),
                edge_panel,
                false,
                &t,
                cx,
            )))
            .child(div().flex_1().min_w_0().child(self.panel_button(
                M_SHADOW,
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(chip().bg(corner_bg()).child(shadow_mini(self.doc.values[SH_ANGLE], self.doc.values[SH_SIZE], self.doc.values[SH_BLUR], self.doc.values[SH_OPACITY], 0.4)))
                    .child(label("Shadow", shadow_name.to_string())),
                shadow_panel,
                false,
                &t,
                cx,
            )))
            .child(
                div()
                    .w(px(96.))
                    .flex_none()
                    .p(px(4.))
                    .flex()
                    .gap(px(2.))
                    .rounded(px(12.))
                    .bg(t.panel)
                    .border_1()
                    .border_color(t.border)
                    .child(icon("undo", "\u{E7A7}", !self.past.is_empty() || self.doc != self.saved).on_click(cx.listener(|this, _, _, cx| this.undo(cx))))
                    .child(icon("redo", "\u{E7A6}", !self.future.is_empty()).on_click(cx.listener(|this, _, _, cx| this.redo(cx)))),
            )
            .child(div().w(px(130.)).flex_none().child(self.panel_button(
                M_EXPORT,
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .child(div().font_family(ICONS).text_size(px(13.)).child("\u{EDE1}"))
                    .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child("Export")),
                export_panel,
                true,
                &t,
                cx,
            )));

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.app)
            .text_color(t.text)
            .font_family(SANS)
            .track_focus(&self.focus)
            .key_context("Editor")
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.undo(cx)))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.redo(cx)))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            // A press outside a text field applies what was typed and hands the keyboard back.
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.blur_fields(window, cx)))
            .on_mouse_move(cx.listener(Self::on_move))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.on_up(cx)))
            .child(top_bar)
            .child(div().flex_1().min_h_0().flex().flex_col().gap(px(GAP)).px(px(GAP)).pb(px(GAP)).child(stage).child(dock))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_cuts_pages_into_squares() {
        assert_eq!(grid_cells(1920, 1080), (16, 9));
        assert_eq!(grid_cells(1080, 1080), (8, 8));
        assert_eq!(grid_cells(1080, 1920), (9, 16));
        assert_eq!(grid_cells(1080, 1350), (8, 10));
        // No exact fit: the cells still come out within a few percent of square.
        let (c, r) = grid_cells(1200, 627);
        let cell = (1200.0 / c as f32) / (627.0 / r as f32);
        assert!((cell - 1.0).abs() < 0.04, "{c}×{r} -> {cell}");
    }
}
