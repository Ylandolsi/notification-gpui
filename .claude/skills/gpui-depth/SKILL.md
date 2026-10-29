---
name: gpui-depth
description: Give GPUI controls and surfaces (buttons, inputs, cards, popovers, the island itself) a lit, raised look instead of flat fills, using the coss ui lighting recipe translated to GPUI. Use when styling or restyling any GPUI element, when asked to make UI look less flat or more polished, or to add shadows, highlights, hover or pressed states.
---

# GPUI depth

Source recipe: coss ui (`cosscom/coss`, `packages/ui/src/components/*.tsx`, `styles/theme.css`), read as source, not as rendered pixels. This app is dark-only, so only the dark values are listed. Helpers: `references/depth.rs` (copy of `src/depth.rs`); use `button`, `lit_edge`, `drop_shadow` instead of rewriting them.

## The recipe

Light comes from above. Every raised thing is built from the same five layers:

1. **Translucent fill.** Dark controls are white at low alpha over the surface, not a grey: default `white 7%`, hover `12%`, pressed `14%`. A solid accent fill is only for the primary action.
2. **Hairline border.** 1px, white `8-10%` (never a solid grey). On a solid fill, the border is the fill colour.
3. **Lit edge.** A child overlay, `absolute inset-0`, corner radius = parent radius - 1px, with only a top border: white `16%` on a solid fill, `6-7%` on a translucent one. This replaces CSS `inset-shadow`, which GPUI lacks.
4. **Small outer shadow.** Controls: black `35%`, 1px down, 2px blur. Accent fills tint it with their own colour at `30%`. Floating surfaces (popover, menu): black `45%`, 3px down, 8px blur.
5. **Pressed means pushed in.** Active drops the outer shadow and moves the fill one step; disabled drops the shadow and goes to 64% opacity.

Steps up in dark mode are alpha steps (`/32`, `/64`), not new hues.

## Surfaces

| Surface | Radius | Border | Shadow | Fill |
|---|---|---|---|---|
| Control | 8 | white 8-10% | 1/2 black 35% | white 7% |
| Card | 16 | white 8% | 1/2 black 35% | gradient 2C2C2F -> 1B1B1D |
| Popover, menu | 8-10 | white 8% | 3/8 black 45% | charcoal gradient |
| Tooltip | 6 | white 8% | 2/6 black 40% | charcoal |

The island is a card flush with the screen's top: square on top, rounded below, lit edge along the top only.

## GPUI limits that shape the code

- `BoxShadow` is outer only. Fake any inset with a `lit_edge` child.
- `linear_gradient` takes exactly two stops. Build a three-stop fade (clear, lit, clear) from two half-width children.
- A shadow is clipped by the window's own bounds. This app clips the window with `SetWindowRgn`, so the clip must keep a margin around the shape (`MARGIN` in `panel.rs`) at least as big as the shadow's reach (about blur + offset + 4px). Too small a margin shows a hard grey box.
- Click-through: that margin swallows clicks. Keep it as small as the shadow allows.
- Hover and pressed need `.id(..)` and `StatefulInteractiveElement` in scope (`.hover(|s| ..)`, `.active(|s| ..)`).
- A parent's hover or pressed state cannot restyle a child overlay; keep the lit edge static.
- Text in controls: 12px, regular weight. Titles: 14px, `FontWeight::MEDIUM`.

## Checklist before saying it is done

- Is there a fill, a hairline, a lit edge and a shadow, and no solid grey borders?
- Does pressed lose its shadow?
- Is the shadow inside the window's clip margin? Look at the idle pill against a light page.
- Was it seen on screen, not only compiled? Say so if not.
