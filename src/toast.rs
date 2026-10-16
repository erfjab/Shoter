//! The floating card that slides into the screen corner after a capture.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    div, ease_out_quint, hsla, img, point, prelude::*, px, relative, size, Animation, AnimationExt,
    App, Bounds, BoxShadow, Context, CursorStyle, FontWeight, RenderImage, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
};

use crate::shot::Shot;
use crate::theme::{Theme, ICONS, MONO, SANS};
use crate::win::capture::Monitor;
use crate::AppState;

const CARD_W: f32 = 320.0;
/// Every capture, tiny or full-screen, sits in the same fixed box.
const STAGE_H: f32 = 176.0;
const STAGE_PAD: f32 = 12.0;
const FOOTER_H: f32 = 44.0;
const BAR_H: f32 = 3.0;
/// How long the card lives; hovering pauses it.
const LIFETIME: Duration = Duration::from_secs(5);

pub struct Toast {
    shot: Arc<Shot>,
    pub image: Arc<RenderImage>,
    hovered: bool,
    /// Time lived so far (hover excluded) and when it was last counted.
    lived: Duration,
    counted: Instant,
}

pub fn open(shot: Arc<Shot>, monitor: Monitor, cx: &mut App) {
    crate::close_toast(cx);
    let s = monitor.scale;
    // One size for every capture; the window is exactly the card.
    let (w, h) = (CARD_W, 10.0 + STAGE_H + 6.0 + FOOTER_H + 6.0 + BAR_H);
    let right = monitor.work.right as f32 / s - 16.0;
    let bottom = monitor.work.bottom as f32 / s - 16.0;
    let bounds = Bounds::new(point(px(right - w), px(bottom - h)), size(px(w), px(h)));
    let display_id = cx
        .displays()
        .iter()
        .find(|d| d.bounds().contains(&bounds.center()))
        .map(|d| d.id());

    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            display_id,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        move |window, cx| {
            crate::win::make_floating(window);
            cx.new(|_| Toast::new(shot))
        },
    );
    if let Ok(handle) = handle {
        cx.global_mut::<AppState>().toast = Some(handle);
    }
}

impl Toast {
    fn new(shot: Arc<Shot>) -> Self {
        Self { image: shot.image.clone(), shot, hovered: false, lived: Duration::ZERO, counted: Instant::now() }
    }

    fn edit(&mut self, cx: &mut Context<Self>) {
        let shot = self.shot.clone();
        cx.defer(move |cx| {
            crate::close_toast(cx);
            crate::editor::open(shot, cx);
        });
    }
}

impl Render for Toast {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::from(window.appearance());

        // Lifetime: counted per frame while not hovered; the bar below shows what is left.
        let now = Instant::now();
        if !self.hovered {
            self.lived += now - self.counted;
        }
        self.counted = now;
        if self.lived >= LIFETIME {
            cx.defer(|cx| {
                crate::close_toast(cx);
            });
        } else if !self.hovered {
            window.request_animation_frame();
        }
        let left = 1.0 - self.lived.as_secs_f32() / LIFETIME.as_secs_f32();

        // The capture is fitted (contain, centered) into the fixed themed box.
        let (iw, ih) = (CARD_W - 20.0 - 2.0 * STAGE_PAD, STAGE_H - 2.0 * STAGE_PAD);
        let k = (iw / self.shot.width as f32).min(ih / self.shot.height as f32);
        let (fw, fh) = (self.shot.width as f32 * k, self.shot.height as f32 * k);
        let stage = div()
            .id("stage")
            .h(px(STAGE_H))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(12.))
            .bg(t.stage)
            .border_1()
            .border_color(t.border)
            .cursor(CursorStyle::PointingHand)
            .on_click(cx.listener(|this, _, _, cx| this.edit(cx)))
            .child(
                div()
                    .w(px(fw))
                    .h(px(fh))
                    .rounded(px(6.))
                    .shadow(vec![BoxShadow {
                        color: hsla(0., 0., 0., if t.dark { 0.5 } else { 0.18 }),
                        offset: point(px(0.), px(4.)),
                        blur_radius: px(14.),
                        spread_radius: px(0.),
                    }])
                    .child(img(self.image.clone()).size_full().rounded(px(6.))),
            );

        let square = |id: &'static str, glyph: &'static str| {
            div()
                .id(id)
                .size(px(32.))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(9.))
                .text_color(t.text2)
                .font_family(ICONS)
                .text_size(px(10.))
                .cursor(CursorStyle::PointingHand)
                .hover(|d| d.bg(t.field).text_color(t.text))
                .child(glyph)
        };

        let footer = div()
            .h(px(FOOTER_H))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(4.))
            .child(
                div()
                    .size(px(18.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(t.success)
                    .text_color(t.on_accent)
                    .font_family(ICONS)
                    .text_size(px(9.))
                    .child("\u{E73E}"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .flex_1()
                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child("Copied"))
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(11.))
                            .text_color(t.text2)
                            .child(format!("{} · {}", self.shot.kind, self.shot.dims())),
                    ),
            )
            .child(square("edit", "\u{E70F}").on_click(cx.listener(|this, _, _, cx| this.edit(cx))))
            .child(square("close", "\u{E8BB}").on_click(cx.listener(|_, _, _, cx| {
                cx.defer(|cx| {
                    crate::close_toast(cx);
                });
            })));

        let card = div()
            .id("card")
            .absolute()
            .left_0()
            .top_0()
            // Exactly the window: a card taller than its window had its bottom edge cut off.
            .size_full()
            .p(px(10.))
            .pb(px(6.0 + BAR_H))
            .flex()
            .flex_col()
            .gap(px(6.))
            .overflow_hidden()
            // Round on top, flat along the bottom where the lifetime bar runs (a quiet shade of
            // the theme: a little lighter on dark, a little darker on light).
            .rounded_t(px(18.))
            .bg(t.panel)
            .border_1()
            .border_color(t.hud_border)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                // Settle the clock at the switch so time spent hovering is never counted.
                let now = Instant::now();
                if !this.hovered {
                    this.lived += now - this.counted;
                }
                this.counted = now;
                this.hovered = *hovered;
                cx.notify();
            }))
            .child(stage)
            .child(footer)
            .child(div().absolute().left_0().bottom_0().h(px(BAR_H)).w(relative(left.max(0.0))).bg(t.text.opacity(0.12)))
            .with_animation(
                "enter",
                Animation::new(Duration::from_millis(460)).with_easing(ease_out_quint()),
                |card, k| card.top(px((1.0 - k) * 16.0)).opacity(k),
            );

        div().size_full().relative().font_family(SANS).text_color(t.text).child(card)
    }
}
