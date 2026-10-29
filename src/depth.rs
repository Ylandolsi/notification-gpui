//! Depth helpers for the island's controls: the coss ui lighting recipe translated to GPUI (dark surfaces).
//! The recipe and its reasons are in the `gpui-depth` skill (`.claude/skills/gpui-depth`); keep this file and
//! the skill's `references/depth.rs` identical.

use std::sync::atomic::{AtomicU32, Ordering};

use gpui::{
    BoxShadow, Div, ElementId, Hsla, InteractiveElement, ParentElement, Pixels, Stateful,
    StatefulInteractiveElement, Styled, div, hsla, point, px, rgb,
};

/// One physical pixel, in logical pixels, as `f32` bits. A 1px logical border at a 150% display is 1.5
/// physical pixels, so it lands unevenly on the pixel grid and curves look ragged.
static HAIRLINE: AtomicU32 = AtomicU32::new(0x3F80_0000); // 1.0

/// Call once with the display's scale factor (1.0, 1.5, 2.0 ...) before building any element.
pub fn set_scale(scale: f32) {
    HAIRLINE.store((1.0 / scale.max(0.5)).to_bits(), Ordering::Relaxed);
}

/// The width of a border that is exactly one physical pixel.
pub fn hairline() -> Pixels {
    px(f32::from_bits(HAIRLINE.load(Ordering::Relaxed)))
}

/// A one-physical-pixel border on all four sides.
pub fn border<E: Styled>(mut el: E) -> E {
    let w = hairline();
    let b = &mut el.style().border_widths;
    b.top = Some(w.into());
    b.right = Some(w.into());
    b.bottom = Some(w.into());
    b.left = Some(w.into());
    el
}

/// A one-physical-pixel border on the top side only.
pub fn border_top<E: Styled>(mut el: E) -> E {
    el.style().border_widths.top = Some(hairline().into());
    el
}

pub fn white(alpha: f32) -> Hsla {
    hsla(0.0, 0.0, 1.0, alpha)
}

pub fn black(alpha: f32) -> Hsla {
    hsla(0.0, 0.0, 0.0, alpha)
}

/// A single outer shadow. GPUI has no inset shadow, so lit edges are `lit_edge` children instead.
pub fn drop_shadow(colour: Hsla, y: f32, blur: f32) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: colour,
        offset: point(px(0.0), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.0),
    }]
}

/// The lit inner edge: an absolute overlay inside the border (so it is inset by the border width) whose top
/// border catches the light. `radius` is the parent's corner radius; the overlay's is one pixel less.
pub fn lit_edge(radius: f32, alpha: f32) -> Div {
    border_top(
        div()
            .absolute()
            .inset_0()
            .rounded(px(radius - 1.0))
            .border_color(white(alpha)),
    )
}

#[derive(Clone, Copy)]
pub enum ButtonTone {
    /// Translucent white fill with a hairline: the default control.
    Outline,
    /// A solid accent fill (0xRRGGBB) with a bright lit top edge and a coloured shadow.
    Primary(u32),
    /// No fill or shadow until hovered.
    Ghost,
}

const RADIUS: f32 = 8.0;

pub fn button(id: impl Into<ElementId>, label: &str, tone: ButtonTone) -> Stateful<Div> {
    let base = div()
        .id(id)
        .relative()
        .flex_none()
        .px(px(11.0))
        .py(px(4.0))
        .rounded(px(RADIUS))
        .text_size(px(12.0))
        .child(lit_edge(
            RADIUS,
            match tone {
                ButtonTone::Primary(_) => 0.16,
                ButtonTone::Outline => 0.07,
                ButtonTone::Ghost => 0.0,
            },
        ))
        .child(label.to_string());
    let base = border(base);
    match tone {
        ButtonTone::Outline => base
            .border_color(white(0.10))
            .bg(white(0.07))
            .shadow(drop_shadow(black(0.35), 1.0, 2.0))
            .hover(|s| s.bg(white(0.12)))
            // Pressed: lighter fill, and the shadow goes, so the key reads as pushed in.
            .active(|s| s.bg(white(0.14)).shadow(Vec::new())),
        ButtonTone::Primary(fill) => {
            let fill = Hsla::from(rgb(fill));
            base.border_color(fill)
                .bg(fill)
                .shadow(drop_shadow(Hsla { a: 0.30, ..fill }, 1.0, 3.0))
                .hover(move |s| s.bg(Hsla { a: 0.9, ..fill }))
                .active(move |s| s.bg(Hsla { a: 0.8, ..fill }).shadow(Vec::new()))
        }
        ButtonTone::Ghost => base
            .border_color(white(0.0))
            .hover(|s| s.bg(white(0.08)).border_color(white(0.06)))
            .active(|s| s.bg(white(0.12))),
    }
}
