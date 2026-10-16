use gpui::{div, px, Div, Hsla, ParentElement, Styled};

/// The Nest mark: a thin square frame around a solid square, with a clear gap between them.
/// Every edge is snapped to whole device pixels (`scale` is the window's), so the mark stays
/// crisp at fractional display scales instead of smearing the frame into the core.
pub fn logo(size: f32, color: Hsla, scale: f32) -> Div {
    let device = (size * scale).round();
    let stroke = scale.round().max(1.0);
    let gap = (device * 0.16).round().max(stroke + 1.0);
    let core = device - 2.0 * (stroke + gap);
    let l = |v: f32| px(v / scale);
    div()
        .relative()
        .flex_none()
        .size(l(device))
        .border(l(stroke))
        .border_color(color)
        .child(div().absolute().top(l(gap)).left(l(gap)).size(l(core)).bg(color))
}
