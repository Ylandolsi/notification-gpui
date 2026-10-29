//! The island: one always-present window at the top-centre of the screen, shaped like a notch (ported from
//! the tour-guide app's pill). Idle it is a small pill with a dot; a notification grows it into a toast and it
//! shrinks back when the toast ends; a click on the pill opens the history list.
//!
//! The drawn shape springs inside a fixed-size window. The window is clipped to the shape (grown first, shrunk
//! once settled), so its transparent area is neither drawn nor clickable. Resizing the window instead blanks
//! it once it shrinks.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext, Bounds, BoxShadow, Context, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, Render, StatefulInteractiveElement, Styled, Window, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions, div, hsla, linear_color_stop, linear_gradient, point,
    px, rgb, size,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::depth::{ButtonTone, button};
use crate::geom::PhysicalRect;
use crate::model::{Cmd, Notification, Store, Urgency};
use crate::monitors::Monitor;
use crate::spring::Spring;
use crate::winapi::{self as window, Hwnd};

/// Shape sizes in logical pixels. The shape is flush with the top edge of the screen.
const IDLE: (f32, f32) = (150.0, 10.0);
const HOVER: (f32, f32) = (330.0, 40.0);
const TOAST: (f32, f32) = (440.0, 68.0);
const LIST_W: f32 = 440.0;
const LIST_HEADER: f32 = 44.0;
const LIST_ROW: f32 = 54.0;
const LIST_EMPTY: f32 = 44.0;
/// Rows the list shows; the rest stay in history.
const LIST_ROWS: usize = 5;
/// The window's own size: the biggest shape. It never changes; the clip follows the shape.
/// Room kept around the shape, on the sides and below, for its shadow.
const MARGIN: f32 = 20.0;
const WINDOW: (f32, f32) = (
    LIST_W + 2.0 * MARGIN,
    LIST_HEADER + LIST_ROWS as f32 * LIST_ROW + 8.0 + MARGIN,
);
/// How long a pointer may leave before the hover shape collapses: GPUI can report a short leave while the
/// shape changes size under the pointer.
const HOVER_LEAVE: Duration = Duration::from_millis(120);

/// Rubik (OFL, see `assets/`), a variable-weight font bundled so the island looks the same on every machine.
pub const FONT_DATA: &[u8] = include_bytes!("../assets/Rubik.ttf");
const FONT: &str = "Rubik";

const IDLE_DOT: u32 = 0x8A8A93;
const DND_DOT: u32 = 0xF5A623;
const MUTED: u32 = 0x9A9A9A;

fn urgency_colour(urgency: Urgency) -> Hsla {
    match urgency {
        Urgency::Low => rgb(0x8A8A93),
        Urgency::Normal => rgb(0xE447A8),
        Urgency::Critical => rgb(0xE5484D),
    }
    .into()
}

enum View {
    /// Nothing is happening; `hover` shows the count.
    Idle {
        hover: bool,
    },
    Toast(Notification),
    List,
}

pub struct PanelView {
    view: View,
    /// Toasts waiting behind the one showing.
    pending: VecDeque<Notification>,
    store: Arc<Store>,
    hwnd: Hwnd,
    /// The drawn shape's width and height, both started at `start`.
    w: Spring,
    h: Spring,
    start: Instant,
    /// The display the island sits on, and the logical size the window is currently clipped to.
    monitor: Monitor,
    window_size: (f32, f32),
    /// Bumped on every hover change, so a leave is ignored if the pointer comes back first.
    hover_generation: u32,
    /// Bumped on every toast, so an old toast's timer cannot end a newer one.
    toast_generation: u32,
}

impl PanelView {
    fn size(&self) -> (f32, f32) {
        match &self.view {
            View::Idle { hover: false } => IDLE,
            View::Idle { hover: true } => HOVER,
            View::Toast(_) => TOAST,
            View::List => {
                let rows = self.store.history().len().min(LIST_ROWS);
                let body = if rows == 0 {
                    LIST_EMPTY
                } else {
                    rows as f32 * LIST_ROW + 8.0
                };
                (LIST_W, LIST_HEADER + body)
            }
        }
    }

    /// Applies what the server sent.
    pub fn handle(&mut self, cmd: Cmd, cx: &mut Context<Self>) {
        match cmd {
            Cmd::Toast(n) => self.on_toast(n, cx),
            Cmd::Refresh => self.refresh(cx),
        }
    }

    fn on_toast(&mut self, n: Notification, cx: &mut Context<Self>) {
        match &self.view {
            // The list already shows it; it may need a row more.
            View::List => self.refresh(cx),
            View::Idle { .. } => self.show_toast(n, cx),
            View::Toast(current) if current.id == n.id => self.show_toast(n, cx),
            // A critical notification takes the island from a quieter one.
            View::Toast(current)
                if n.urgency == Urgency::Critical && current.urgency != Urgency::Critical =>
            {
                self.show_toast(n, cx)
            }
            View::Toast(_) => {
                match self.pending.iter_mut().find(|p| p.id == n.id) {
                    Some(slot) => *slot = n,
                    None => self.pending.push_back(n),
                }
                cx.notify();
            }
        }
    }

    /// History or Do Not Disturb changed: drop toasts whose notification is gone, and resize.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let history = self.store.history();
        self.pending
            .retain(|p| history.iter().any(|h| h.id == p.id));
        if let View::Toast(current) = &self.view
            && !history.iter().any(|h| h.id == current.id)
        {
            self.advance(cx);
            return;
        }
        self.animate_to_view();
        cx.notify();
    }

    fn show_toast(&mut self, n: Notification, cx: &mut Context<Self>) {
        self.toast_generation = self.toast_generation.wrapping_add(1);
        let generation = self.toast_generation;
        if let Some(duration) = n.toast_duration() {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(duration).await;
                let _ = this.update(cx, |view, cx| {
                    if view.toast_generation == generation {
                        view.advance(cx);
                    }
                });
            })
            .detach();
        }
        self.view = View::Toast(n);
        self.animate_to_view();
        cx.notify();
    }

    /// The toast ends: the next one, or back to the idle pill.
    fn advance(&mut self, cx: &mut Context<Self>) {
        match self.pending.pop_front() {
            Some(next) => self.show_toast(next, cx),
            None => {
                self.toast_generation = self.toast_generation.wrapping_add(1);
                self.view = View::Idle { hover: false };
                self.animate_to_view();
                cx.notify();
            }
        }
    }

    fn open_list(&mut self, cx: &mut Context<Self>) {
        self.toast_generation = self.toast_generation.wrapping_add(1);
        self.pending.clear();
        self.view = View::List;
        self.animate_to_view();
        cx.notify();
    }

    fn close_list(&mut self, cx: &mut Context<Self>) {
        self.view = View::Idle { hover: false };
        self.animate_to_view();
        cx.notify();
    }

    fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        self.store.remove(id);
        self.refresh(cx);
    }

    fn toggle_dnd(&mut self, cx: &mut Context<Self>) {
        self.store.set_dnd(!self.store.dnd());
        cx.notify();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.store.clear();
        self.refresh(cx);
    }

    /// Springs the shape from where it is now to the current view's size. The clip grows first, so the
    /// shape is never cut; `render` shrinks it once the shape has settled.
    fn animate_to_view(&mut self) {
        let now = Instant::now();
        let t = (now - self.start).as_secs_f32();
        let (w, h) = self.size();
        self.w = self.w.retarget(t, w);
        self.h = self.h.retarget(t, h);
        self.start = now;
        let grown = (self.window_size.0.max(w), self.window_size.1.max(h));
        if grown != self.window_size {
            self.clip(grown);
        }
    }

    /// Puts the window's client area at its fixed size, centred and flush with the top of its display.
    fn place(&mut self) {
        let scale = self.monitor.scale;
        let (w, h) = (
            (WINDOW.0 * scale).round() as i32,
            (WINDOW.1 * scale).round() as i32,
        );
        let x = self.monitor.rect.x + (self.monitor.rect.w - w) / 2;
        window::place_client(
            self.hwnd,
            PhysicalRect {
                x,
                y: self.monitor.rect.y,
                w,
                h,
            },
        );
    }

    /// Clips the window to a centred, top-flush rectangle of `size` (logical pixels).
    fn clip(&mut self, size: (f32, f32)) {
        self.window_size = size;
        let scale = self.monitor.scale;
        // The clip keeps room around the shape (the sides and the bottom) for its shadow.
        let (w, h) = (
            ((size.0 + 2.0 * MARGIN) * scale).round() as i32,
            ((size.1 + MARGIN) * scale).round() as i32,
        );
        // The region is in window coordinates; the shape is drawn in the client area.
        let (client, outer) = (
            window::client_rect(self.hwnd),
            window::window_rect(self.hwnd),
        );
        window::set_clip(
            self.hwnd,
            client.x - outer.x + (client.w - w) / 2,
            client.y - outer.y,
            w,
            h,
        );
    }

    fn on_hover(&mut self, hovered: bool, cx: &mut Context<Self>) {
        self.hover_generation = self.hover_generation.wrapping_add(1);
        if hovered {
            self.set_hover(true, cx);
            return;
        }
        let generation = self.hover_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_LEAVE).await;
            let _ = this.update(cx, |view, cx| {
                if view.hover_generation == generation {
                    view.set_hover(false, cx);
                }
            });
        })
        .detach();
    }

    /// Hover only changes the idle shape; a toast and the list ignore it.
    fn set_hover(&mut self, on: bool, cx: &mut Context<Self>) {
        if let View::Idle { hover } = &mut self.view
            && *hover != on
        {
            *hover = on;
            self.animate_to_view();
            cx.notify();
        }
    }
}

fn dot(colour: Hsla) -> impl IntoElement {
    div().flex_none().size(px(8.0)).rounded_full().bg(colour)
}

/// A 1px light line along the top of the shape, brightest in the middle, that fades out at both ends.
fn top_highlight() -> impl IntoElement {
    let clear = hsla(0.0, 0.0, 1.0, 0.0);
    let lit = hsla(0.0, 0.0, 1.0, 0.28);
    div()
        .absolute()
        .top_0()
        .left(px(14.0))
        .right(px(14.0))
        .h(px(1.0))
        .flex()
        .child(div().flex_1().h_full().bg(linear_gradient(
            90.0,
            linear_color_stop(clear, 0.0),
            linear_color_stop(lit, 1.0),
        )))
        .child(div().flex_1().h_full().bg(linear_gradient(
            90.0,
            linear_color_stop(lit, 0.0),
            linear_color_stop(clear, 1.0),
        )))
}

/// Soft light falling from the top: a white wash that fades out over the upper part of the shape.
fn sheen(shape_height: f32) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(px((shape_height * 0.55).min(48.0)))
        .bg(linear_gradient(
            180.0,
            linear_color_stop(hsla(0.0, 0.0, 1.0, 0.07), 0.0),
            linear_color_stop(hsla(0.0, 0.0, 1.0, 0.0), 1.0),
        ))
}

/// A faint light ring just inside the border, so the edge reads as a lit bevel (coss ui's `before:inset-0`).
fn rim(radius: f32) -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .rounded_b(px((radius - 1.0).max(0.0)))
        .border_1()
        .border_color(hsla(0.0, 0.0, 1.0, 0.09))
}

/// Title over a muted second line, as the toast and the list rows show a notification.
fn text_column(n: &Notification) -> impl IntoElement {
    let sub = if n.body.is_empty() {
        n.app.clone()
    } else {
        format!("{} · {}", n.app, n.body)
    };
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(14.0))
                .child(n.title.clone()),
        )
        .child(
            div()
                .truncate()
                .text_size(px(12.0))
                .text_color(Hsla::from(rgb(MUTED)))
                .child(sub),
        )
}

impl Render for PanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = (Instant::now() - self.start).as_secs_f32();
        if self.w.settled(t) && self.h.settled(t) {
            self.w = Spring::at_rest(self.w.target());
            self.h = Spring::at_rest(self.h.target());
            // Settled: the window no longer needs the room the shape grew through.
            let target = self.size();
            if self.window_size != target {
                self.clip(target);
            }
        } else {
            // Paced by GPUI's vsync thread, so one step per display refresh.
            window.request_animation_frame();
        }
        let (w, h) = (self.w.value(t), self.h.value(t));
        let radius = (h / 2.0).min(22.0);
        let (target_w, target_h) = self.size();
        let dnd = self.store.dnd();
        // The shape is flush with the top edge: square there, rounded below, like a notch. Its content is
        // laid out at the target size and clipped by the shape, so text does not reflow while it springs.
        let shape = div()
            .id("island")
            .flex_none()
            .w(px(w))
            .h(px(h))
            .flex()
            .justify_center()
            .items_start()
            .overflow_hidden()
            .rounded_b(px(radius))
            // Charcoal with a lit top edge, a dark hairline and a soft shadow underneath, like a raised key.
            .bg(linear_gradient(
                180.0,
                linear_color_stop(rgb(0x2C2C2F), 0.0),
                linear_color_stop(rgb(0x1B1B1D), 1.0),
            ))
            .border_1()
            .border_color(hsla(0.0, 0.0, 0.0, 0.6))
            .shadow(vec![BoxShadow {
                color: hsla(0.0, 0.0, 0.0, 0.35),
                offset: point(px(0.0), px(3.0)),
                blur_radius: px(8.0),
                spread_radius: px(0.0),
            }])
            .relative()
            .child(top_highlight())
            .child(sheen(h))
            .child(rim(radius))
            .text_color(gpui::white())
            .text_size(px(15.0))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| this.on_hover(*hovered, cx)));
        let content = div()
            .id("content")
            .flex_none()
            .w(px(target_w))
            .h(px(target_h))
            .flex()
            .items_center();
        let content = match &self.view {
            View::Idle { hover } => {
                // The click opens the list. It sits on the idle content, not the shape: a click on a list
                // button would bubble to the shape and reopen the list it just closed.
                let content = content.on_click(cx.listener(|this, _, _, cx| this.open_list(cx)));
                let colour = Hsla::from(rgb(if dnd { DND_DOT } else { IDLE_DOT }));
                if *hover {
                    let count = self.store.history().len();
                    let hint = if dnd {
                        format!("Do not disturb · {count} saved")
                    } else if count == 0 {
                        "No notifications".to_string()
                    } else {
                        format!("{count} notification{}", if count == 1 { "" } else { "s" })
                    };
                    content
                        .px(px(16.0))
                        .gap(px(10.0))
                        .child(dot(colour))
                        .child(div().text_size(px(13.0)).truncate().child(hint))
                } else {
                    // A bare sliver; only Do Not Disturb marks it, with a short amber line.
                    let sliver = content.justify_center();
                    if dnd {
                        sliver.child(
                            div()
                                .w(px(28.0))
                                .h(px(2.0))
                                .rounded_full()
                                .bg(Hsla::from(rgb(DND_DOT))),
                        )
                    } else {
                        sliver
                    }
                }
            }
            View::Toast(n) => {
                let more = self.pending.len();
                let mut row = content
                    .px(px(16.0))
                    .gap(px(12.0))
                    .child(dot(urgency_colour(n.urgency)))
                    .child(text_column(n));
                if more > 0 {
                    row = row.child(
                        div()
                            .flex_none()
                            .text_size(px(12.0))
                            .text_color(Hsla::from(rgb(MUTED)))
                            .child(format!("+{more}")),
                    );
                }
                // A click anywhere on the toast ends it; it stays in the history.
                row.on_click(cx.listener(|this, _, _, cx| this.advance(cx)))
            }
            View::List => {
                let history = self.store.history();
                let header = div()
                    .flex_none()
                    .w_full()
                    .h(px(LIST_HEADER))
                    .px(px(16.0))
                    .gap(px(8.0))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Notifications"),
                    )
                    .child(
                        button(
                            "dnd",
                            if dnd { "DND on" } else { "DND off" },
                            if dnd {
                                ButtonTone::Primary(0xB7791F)
                            } else {
                                ButtonTone::Outline
                            },
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_dnd(cx))),
                    )
                    .child(
                        button("clear", "Clear", ButtonTone::Outline)
                            .on_click(cx.listener(|this, _, _, cx| this.clear(cx))),
                    )
                    .child(
                        button("close", "×", ButtonTone::Outline)
                            .on_click(cx.listener(|this, _, _, cx| this.close_list(cx))),
                    );
                let mut rows = div().flex_none().w_full().flex().flex_col();
                if history.is_empty() {
                    rows = rows.child(
                        div()
                            .h(px(LIST_EMPTY))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(13.0))
                            .text_color(Hsla::from(rgb(MUTED)))
                            .child("No notifications"),
                    );
                }
                for (ix, n) in history.iter().take(LIST_ROWS).enumerate() {
                    let id = n.id.clone();
                    rows =
                        rows.child(
                            div()
                                .h(px(LIST_ROW))
                                .w_full()
                                .px(px(16.0))
                                .gap(px(12.0))
                                .flex()
                                .items_center()
                                .child(dot(urgency_colour(n.urgency)))
                                .child(text_column(n))
                                .child(button(("remove", ix), "×", ButtonTone::Ghost).on_click(
                                    cx.listener(move |this, _, _, cx| this.remove(&id, cx)),
                                )),
                        );
                }
                content.flex_col().items_start().child(header).child(rows)
            }
        };
        div()
            .id("panel")
            .font_family(FONT)
            .size_full()
            .flex()
            .justify_center()
            .child(shape.child(content))
    }
}

pub fn hwnd_of(window: &Window) -> Option<Hwnd> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

fn display_for(monitor: &Monitor, cx: &App) -> Option<gpui::DisplayId> {
    cx.displays().into_iter().find_map(|d| {
        let o = d.bounds().origin;
        let (x, y) = (
            f32::from(o.x) * monitor.scale,
            f32::from(o.y) * monitor.scale,
        );
        ((x - monitor.rect.x as f32).abs() < 2.0 && (y - monitor.rect.y as f32).abs() < 2.0)
            .then(|| d.id())
    })
}

/// Opens the island, idle, flush with the top of `monitor`. It never takes focus.
pub fn open_island(
    cx: &mut App,
    monitor: &Monitor,
    store: Arc<Store>,
) -> Option<gpui::WindowHandle<PanelView>> {
    let width_dip = monitor.rect.w as f32 / monitor.scale;
    let bounds = Bounds {
        origin: point(
            px(monitor.rect.x as f32 / monitor.scale + (width_dip - WINDOW.0) / 2.0),
            px(monitor.rect.y as f32 / monitor.scale),
        ),
        size: size(px(WINDOW.0), px(WINDOW.1)),
    };
    let monitor = *monitor;
    let handle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                display_id: display_for(&monitor, cx),
                titlebar: None,
                focus: false,
                show: true,
                kind: WindowKind::PopUp,
                is_movable: false,
                is_resizable: false,
                is_minimizable: false,
                window_background: WindowBackgroundAppearance::Transparent,
                ..Default::default()
            },
            move |window, cx| {
                let hwnd = hwnd_of(window).unwrap_or(0);
                if let Err(err) = window::style_clickable_no_focus(hwnd) {
                    eprintln!("panel: window style failed: {err}");
                }
                cx.new(|_| PanelView {
                    view: View::Idle { hover: false },
                    pending: VecDeque::new(),
                    store,
                    hwnd,
                    w: Spring::at_rest(IDLE.0),
                    h: Spring::at_rest(IDLE.1),
                    start: Instant::now(),
                    monitor,
                    window_size: IDLE,
                    hover_generation: 0,
                    toast_generation: 0,
                })
            },
        )
        .ok()?;
    // GPUI placed the window from logical bounds; pin it to exact physical pixels, flush with the top.
    let _ = handle.update(cx, |view, _, _| {
        view.place();
        view.clip(IDLE);
        // A window created without focus is not shown by GPUI; the island must be on screen.
        window::show_no_activate(view.hwnd);
    });
    Some(handle)
}
