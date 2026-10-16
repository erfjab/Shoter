#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod compose;
mod editor;
mod overlay;
mod shot;
mod theme;
mod toast;
mod ui;
mod vision;
mod win;

use std::borrow::Cow;
use std::time::Duration;

use futures::StreamExt;
use gpui::{
    div, point, px, size, App, AppContext, Application, Bounds, Context, Global, IntoElement,
    Render, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions,
};

use win::input::AppEvent;

/// Windows that exist at most once at a time.
#[derive(Default)]
pub struct AppState {
    pub overlay: Option<WindowHandle<overlay::Overlay>>,
    pub toast: Option<WindowHandle<toast::Toast>>,
}

impl Global for AppState {}

/// GPUI exits when its last window closes, so an invisible window keeps the app resident.
struct Anchor;

impl Render for Anchor {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn main() {
    if !win::single_instance() {
        return;
    }
    Application::new().run(|cx: &mut App| {
        let _ = cx.text_system().add_fonts(vec![
            Cow::Borrowed(include_bytes!("../assets/fonts/Geist-Regular.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/Geist-Medium.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/Geist-SemiBold.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/GeistMono-Regular.ttf").as_slice()),
        ]);
        cx.set_global(AppState::default());

        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1.), px(1.)),
                ))),
                titlebar: None,
                focus: false,
                show: false,
                kind: WindowKind::PopUp,
                ..Default::default()
            },
            |_, cx| cx.new(|_| Anchor),
        );

        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        win::input::start(tx);
        cx.spawn(async move |cx| {
            while let Some(event) = rx.next().await {
                if cx.update(|cx| handle(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        // Development check without capturing the screen: SHOTER_EDITOR opens the editor and
        // SHOTER_TOAST the corner thumbnail, on the PNG the variable names (or a plain test shot).
        for (var, toast) in [("SHOTER_EDITOR", false), ("SHOTER_TOAST", true)] {
            let Some(path) = std::env::var_os(var) else { continue };
            let shot = image::open(&path)
                .map(|img| {
                    let mut px = img.to_rgba8();
                    for p in px.pixels_mut() {
                        p.0.swap(0, 2);
                    }
                    shot::Shot::new(px.width(), px.height(), px.into_raw(), 1.0, "Element")
                })
                .unwrap_or_else(|_| shot::Shot::new(960, 600, [0xd0, 0xd0, 0xd0, 0xff].repeat(960 * 600), 1.0, "Test"));
            let shot = std::sync::Arc::new(shot);
            if toast {
                toast::open(shot, win::capture::monitor_at_cursor(), cx);
            } else {
                editor::open(shot, cx);
            }
        }
        trim_soon(cx);
    });
}

fn handle(event: AppEvent, cx: &mut App) {
    match event {
        AppEvent::Capture => start_capture(cx),
        AppEvent::Cancel => overlay::cancel(cx),
        AppEvent::Quit => cx.quit(),
    }
}

pub fn start_capture(cx: &mut App) {
    if cx.global::<AppState>().overlay.is_some() {
        return;
    }
    // Keep our own thumbnail out of the picture; give the compositor a frame to drop it.
    if close_toast(cx) {
        std::thread::sleep(Duration::from_millis(40));
    }
    let monitor = win::capture::monitor_at_cursor();
    let Some(shot) = win::capture::grab(monitor.rect, monitor.scale) else {
        return;
    };
    let windows = win::capture::top_windows();
    overlay::open(shot, monitor, windows, cx);
}

/// Closes the corner thumbnail if it is showing. Returns whether it was.
pub fn close_toast(cx: &mut App) -> bool {
    let Some(handle) = cx.global_mut::<AppState>().toast.take() else {
        return false;
    };
    let closed = handle
        .update(cx, |toast, window, _| {
            win::hide(window);
            let _ = window.drop_image(toast.image.clone());
            window.remove_window();
        })
        .is_ok();
    trim_soon(cx);
    closed
}

/// Once the capture windows are gone, hands their pages back to Windows so idle Shoter sits
/// at its baseline instead of its peak.
pub fn trim_soon(cx: &mut App) {
    cx.spawn(async |cx| {
        cx.background_executor().timer(Duration::from_millis(800)).await;
        let _ = cx.update(|cx| {
            let s = cx.global::<AppState>();
            if s.overlay.is_none() && s.toast.is_none() {
                win::trim_memory();
            }
        });
    })
    .detach();
}
