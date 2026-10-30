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
    AnyElement, App, AppContext, Bounds, BoxShadow, Context, ElementId, Font, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, PathBuilder, Render,
    StatefulInteractiveElement, Styled, TextRun, Window, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, canvas, div, font, hsla, linear_color_stop, linear_gradient, point, px,
    rgb, size,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::depth::{ButtonTone, black, border, button, drop_shadow, white};
use crate::geom::PhysicalRect;
use crate::model::{Cmd, Notification, Store, Urgency};
use crate::winactivate;
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
/// The tallest the shape gets: an expanded notification scrolls past it.
const MAX_H: f32 = 460.0;
/// Space above and below the text of an expanded notification.
const EXPAND_PAD: f32 = 10.0;
/// Layout of a row, to work out how wide its text column is: padding on each side, the dot and the gaps and
/// buttons beside the text.
const ROW_PAD: f32 = 16.0;
const ROW_GAP: f32 = 12.0;
const DOT: f32 = 8.0;
const BUTTON_W: f32 = 32.0;
/// GPUI's default line height is the golden ratio times the text size.
const LINE_HEIGHT: f32 = 1.618_034;
/// The window's own size: the biggest shape. It never changes; the clip follows the shape.
/// Room kept around the shape, on the sides and below, for its shadow.
const MARGIN: f32 = 20.0;
const WINDOW: (f32, f32) = (LIST_W + 2.0 * MARGIN, MAX_H + MARGIN);
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
    /// The notification showing its full text, and the row height that text needs (before the cap).
    expanded: Option<(String, f32)>,
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
            View::Toast(n) => match &self.expanded {
                Some((id, height)) if *id == n.id => (TOAST.0, height.clamp(TOAST.1, MAX_H)),
                _ => TOAST,
            },
            View::List => {
                let history = self.store.history();
                let rows = history.len().min(LIST_ROWS);
                let body = if rows == 0 {
                    LIST_EMPTY
                } else {
                    history
                        .iter()
                        .take(rows)
                        .map(|n| self.row_height(&n.id, rows))
                        .sum::<f32>()
                        + 8.0
                };
                (LIST_W, LIST_HEADER + body)
            }
        }
    }

    /// A list row's height: expanded, it takes what the text needs, but the list never outgrows `MAX_H`.
    fn row_height(&self, id: &str, rows: usize) -> f32 {
        match &self.expanded {
            Some((open, height)) if open == id => height.clamp(LIST_ROW, expanded_row_cap(rows)),
            _ => LIST_ROW,
        }
    }

    /// Opens or closes the full text of notification `id`, which needs `height` as a row.
    fn toggle_expand(&mut self, id: &str, height: f32, cx: &mut Context<Self>) {
        if self.expanded.as_ref().is_some_and(|(open, _)| open == id) {
            self.expanded = None;
            // Reading stopped the toast's timer; a toast that is still up gets a fresh one.
            if let View::Toast(n) = &self.view {
                let n = n.clone();
                self.show_toast(n, cx);
                return;
            }
        } else {
            self.expanded = Some((id.to_string(), height));
            self.toast_generation = self.toast_generation.wrapping_add(1);
        }
        self.animate_to_view();
        cx.notify();
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
        if self
            .expanded
            .as_ref()
            .is_some_and(|(id, _)| !history.iter().any(|h| h.id == *id))
        {
            self.expanded = None;
        }
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
        self.expanded = None;
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
                self.expanded = None;
                self.toast_generation = self.toast_generation.wrapping_add(1);
                self.view = View::Idle { hover: false };
                self.animate_to_view();
                cx.notify();
            }
        }
    }

    fn open_list(&mut self, cx: &mut Context<Self>) {
        self.expanded = None;
        self.toast_generation = self.toast_generation.wrapping_add(1);
        self.pending.clear();
        self.view = View::List;
        self.animate_to_view();
        cx.notify();
    }

    fn close_list(&mut self, cx: &mut Context<Self>) {
        self.expanded = None;
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
    border(
        div()
            .absolute()
            .inset_0()
            .rounded_b(px((radius - 1.0).max(0.0)))
            .border_color(hsla(0.0, 0.0, 1.0, 0.09)),
    )
}

/// The tallest an expanded list row gets with `rows` rows showing: the others keep their height.
fn expanded_row_cap(rows: usize) -> f32 {
    MAX_H - LIST_HEADER - 8.0 - (rows - 1) as f32 * LIST_ROW
}

/// Width of the text column in a row `row_w` wide with `buttons` fixed-width controls beside the text.
fn text_width(row_w: f32, buttons: usize) -> f32 {
    row_w - 2.0 * ROW_PAD - DOT - ROW_GAP - buttons as f32 * (BUTTON_W + ROW_GAP)
}

fn sub_line(n: &Notification) -> String {
    if n.body.is_empty() {
        n.app.clone()
    } else {
        format!("{} · {}", n.app, n.body)
    }
}

fn text_run(len: usize, weight: FontWeight) -> TextRun {
    TextRun {
        len,
        font: Font {
            weight,
            ..font(FONT)
        },
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

/// `text` cut to `width` with an ellipsis. GPUI's own truncation depends on the order it is measured in, and
/// can leave the text clipped mid-word with no ellipsis.
fn ellipsize(window: &Window, text: String, size: f32, weight: FontWeight, width: f32) -> String {
    let mut runs = vec![text_run(text.len(), weight)];
    window
        .text_system()
        .line_wrapper(runs[0].font.clone(), px(size))
        .truncate_line(text.into(), px(width), "…", &mut runs)
        .to_string()
}

/// The row height `n` needs to show its text in full, or `None` when the collapsed row already shows it
/// all. `buttons` counts the controls beside the text, without the expand button.
fn full_height(window: &Window, n: &Notification, row_w: f32, buttons: usize) -> Option<f32> {
    let lines = |text: String, size: f32, weight: FontWeight, width: f32| -> f32 {
        let run = text_run(text.len(), weight);
        window
            .text_system()
            .shape_text(text.into(), px(size), &[run], Some(px(width)), None)
            .map_or(1, |lines| {
                lines.iter().map(|l| l.wrap_boundaries().len() + 1).sum()
            })
            .max(1) as f32
    };
    let (title, sub) = (n.title.clone(), sub_line(n));
    let fits = |width| {
        lines(title.clone(), 14.0, FontWeight::MEDIUM, width) == 1.0
            && lines(sub.clone(), 12.0, FontWeight::NORMAL, width) == 1.0
    };
    if fits(text_width(row_w, buttons)) {
        return None;
    }
    let width = text_width(row_w, buttons + 1);
    let text = lines(title.clone(), 14.0, FontWeight::MEDIUM, width) * 14.0 * LINE_HEIGHT
        + lines(sub.clone(), 12.0, FontWeight::NORMAL, width) * 12.0 * LINE_HEIGHT;
    Some(text.ceil() + 2.0 * EXPAND_PAD)
}

/// A ghost button with a down chevron, or an up one when `open`. Drawn as a stroke: Rubik has no chevron glyph.
fn expand_button(id: impl Into<ElementId>, open: bool) -> gpui::Stateful<gpui::Div> {
    let chevron = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let centre = bounds.center();
            let (half_w, tip) = (px(4.0), if open { px(-2.5) } else { px(2.5) });
            let mut path = PathBuilder::stroke(px(1.5));
            path.move_to(point(centre.x - half_w, centre.y - tip));
            path.line_to(point(centre.x, centre.y + tip));
            path.line_to(point(centre.x + half_w, centre.y - tip));
            if let Ok(path) = path.build() {
                window.paint_path(path, hsla(0.0, 0.0, 1.0, 0.85));
            }
        },
    )
    .absolute()
    .inset_0();
    // A space for a label keeps the button as tall as its neighbours.
    button(id, " ", ButtonTone::Ghost)
        .w(px(BUTTON_W))
        .child(chevron)
}

/// Title over a muted second line, as the toast and the list rows show a notification. Collapsed, each line
/// is cut off with an ellipsis to fit `width`; `expanded` (an id and the height available for the text) wraps
/// them and scrolls past that height.
fn text_column(
    window: &Window,
    n: &Notification,
    width: f32,
    expanded: Option<(ElementId, f32)>,
) -> AnyElement {
    let (title, sub) = (n.title.clone(), sub_line(n));
    let (title, sub) = if expanded.is_some() {
        (title, sub)
    } else {
        (
            ellipsize(window, title, 14.0, FontWeight::MEDIUM, width),
            ellipsize(window, sub, 12.0, FontWeight::NORMAL, width),
        )
    };
    let wrap = expanded.is_some();
    let line = |line: gpui::Div| {
        if wrap {
            line
        } else {
            line.whitespace_nowrap().overflow_hidden()
        }
    };
    let column = div()
        .flex()
        .flex_col()
        .child(
            line(div())
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(14.0))
                .child(title),
        )
        .child(
            line(div())
                .text_size(px(12.0))
                .text_color(Hsla::from(rgb(MUTED)))
                .child(sub),
        );
    match expanded {
        None => column.flex_1().min_w_0().into_any_element(),
        Some((id, max_h)) => div()
            .id(id)
            .flex_1()
            .min_w_0()
            .max_h(px(max_h))
            .overflow_y_scroll()
            .child(column)
            .into_any_element(),
    }
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
        let shape = border(shape);
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
                let full = full_height(window, n, TOAST.0, usize::from(more > 0));
                let open = self.expanded.as_ref().is_some_and(|(id, _)| *id == n.id);
                let mut row = content
                    .px(px(16.0))
                    .gap(px(12.0))
                    .child(dot(urgency_colour(n.urgency)))
                    .child(text_column(
                        window,
                        n,
                        text_width(TOAST.0, usize::from(more > 0) + usize::from(full.is_some())),
                        open.then(|| ("toast-text".into(), target_h - 2.0 * EXPAND_PAD)),
                    ));
                if more > 0 {
                    row = row.child(
                        div()
                            .flex_none()
                            .text_size(px(12.0))
                            .text_color(Hsla::from(rgb(MUTED)))
                            .child(format!("+{more}")),
                    );
                }
                if let Some(height) = full {
                    let toggle = n.id.clone();
                    row = row.child(expand_button("expand", open).on_click(cx.listener(
                        move |this, _, _, cx| {
                            // The toast's own click would open the notification.
                            cx.stop_propagation();
                            this.toggle_expand(&toggle, height, cx)
                        },
                    )));
                }
                // A click anywhere on the toast ends it, and does what the native one would; it stays in the history.
                let id = n.id.clone();
                // Open, the text is being read, so a click on it leaves the toast up.
                if open {
                    row
                } else {
                    row.on_click(cx.listener(move |this, _, _, cx| {
                        winactivate::activate(&id);
                        this.advance(cx)
                    }))
                }
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
                let shown = history.len().min(LIST_ROWS);
                for (ix, n) in history.iter().take(LIST_ROWS).enumerate() {
                    let id = n.id.clone();
                    let open_id = n.id.clone();
                    let toggle = n.id.clone();
                    let full = full_height(window, n, LIST_W, 1);
                    let open = self.expanded.as_ref().is_some_and(|(id, _)| *id == n.id);
                    let row_h = self.row_height(&n.id, shown);
                    // The highlight is inset from the island's edge; the padding keeps the content where it was.
                    // Hovered it is a translucent fill with a hairline and a small shadow, pressed it is pushed in.
                    let mut row = border(div().id(("row", ix)))
                        .h(px(row_h))
                        .mx(px(8.0))
                        .px(px(8.0))
                        .rounded(px(10.0))
                        .border_color(white(0.0))
                        .hover(|s| {
                            s.bg(white(0.07))
                                .border_color(white(0.09))
                                .shadow(drop_shadow(black(0.35), 1.0, 2.0))
                        })
                        .active(|s| s.bg(white(0.10)).shadow(Vec::new()))
                        .gap(px(12.0))
                        .flex()
                        .items_center()
                        .child(dot(urgency_colour(n.urgency)))
                        .child(text_column(
                            window,
                            n,
                            text_width(LIST_W, 1 + usize::from(full.is_some())),
                            open.then(|| (("row-text", ix).into(), row_h - 2.0 * EXPAND_PAD)),
                        ));
                    if let Some(height) = full {
                        row = row.child(expand_button(("expand", ix), open).on_click(cx.listener(
                            move |this, _, _, cx| {
                                // The row's own click would open the notification.
                                cx.stop_propagation();
                                this.toggle_expand(&toggle, height, cx)
                            },
                        )));
                    }
                    let row = row.child(button(("remove", ix), "×", ButtonTone::Ghost).on_click(
                        cx.listener(move |this, _, _, cx| {
                            // The row's own click would open the notification.
                            cx.stop_propagation();
                            this.remove(&id, cx)
                        }),
                    ));
                    // A click opens the notification and leaves it in the list; the × removes it.
                    rows = rows.child(row.on_click(move |_, _, _| winactivate::activate(&open_id)));
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
    crate::depth::set_scale(monitor.scale);
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
                    expanded: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_expanded_row_never_grows_the_list_past_the_cap() {
        for rows in 1..=LIST_ROWS {
            let list = LIST_HEADER + 8.0 + (rows - 1) as f32 * LIST_ROW + expanded_row_cap(rows);
            assert!(list <= MAX_H, "{rows} rows: {list}");
            assert!(expanded_row_cap(rows) >= LIST_ROW);
        }
    }
}
