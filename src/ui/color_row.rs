//! The inspector's colour row: an eyedropper (a click on the player takes
//! the colour there), a custom colour (an inline spectrum field: hue across,
//! white at the top through the pure hue to black at the bottom), a divider,
//! then round swatches that scroll sideways, with arrows at the ends to page
//! them while there is more that way. Text styles, Background and the
//! Remove BG stroke share it.

use std::rc::Rc;

use gpui::{
    AnyElement, Context, Hsla, MouseButton, MouseDownEvent, Pixels, Point, div, hsla,
    linear_color_stop, linear_gradient, point, prelude::*, px,
};

use crate::editor::{Drag, Editor};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::visual::hex;
use crate::ui::widgets::tooltip;

/// The round swatches: greys first, then the hues.
pub(crate) const SWATCH_COLORS: [&str; 13] = [
    "#ffffff", "#b5b5b5", "#808080", "#454545", "#000000", "#f5c400", "#ff7a00", "#ff3b30",
    "#ff4fa3", "#a259ff", "#2f7bff", "#00c2ff", "#2fbf71",
];

/// Background's swatches: the shared ones, then pastels and deep tones.
pub(crate) const BACKGROUND_COLORS: [&str; 23] = [
    "#ffffff", "#b5b5b5", "#808080", "#454545", "#000000", "#f5c400", "#ff7a00", "#ff3b30",
    "#ff4fa3", "#a259ff", "#2f7bff", "#00c2ff", "#2fbf71", "#f7c6c7", "#fbd6b4", "#fff1b8",
    "#cdeccf", "#c9e4ff", "#e2d4ff", "#8b1e2d", "#5a3825", "#1f3b2c", "#1b2a4a",
];

/// A colour chosen in a row (`None`: the "none" swatch).
pub type ColorFn = Rc<dyn Fn(&mut Editor, Option<String>, &mut Context<Editor>)>;

/// Swatch size, the ring around it, and the gap between rings.
const SWATCH: f32 = 20.;
const RING: f32 = 1.5;
const GAP: f32 = 2.;
const SPECTRUM_H: f32 = 40.;
/// Where the open spectrum field is measured (one is open at a time).
const SPECTRUM_KEY: &str = "color-spectrum";

const HUES: [u32; 7] = [
    0xff0000, 0xffff00, 0x00ff00, 0x00ffff, 0x0000ff, 0xff00ff, 0xff0000,
];

/// The eyedropper waiting for a click on the player: the row that started
/// it, and where the colour goes.
pub struct ColorPick {
    pub owner: &'static str,
    /// The clip selected when it started (another selection drops it).
    pub clip: Option<String>,
    pub apply: ColorFn,
}

/// The open custom-colour field: its row, and the last point taken (0–1).
pub struct Spectrum {
    pub owner: &'static str,
    pub marker: Option<(f32, f32)>,
}

/// HSL (h in degrees, s and l 0–1) → "#rrggbb".
pub fn hsl_to_hex(h: f32, s: f32, l: f32) -> String {
    let h = h.rem_euclid(360.);
    let a = s * l.min(1. - l);
    let ch = |n: f32| {
        let k = (n + h / 30.) % 12.;
        let v = l - a * (k - 3.).min(9. - k).clamp(-1., 1.);
        (v * 255.).round().clamp(0., 255.) as u8
    };
    format!("#{:02x}{:02x}{:02x}", ch(0.), ch(8.), ch(4.))
}

fn ring(id: impl Into<gpui::ElementId>, on: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .p(px(RING))
        .border(px(RING))
        .rounded_full()
        .border_color(if on {
            theme::text().into()
        } else {
            gpui::transparent_black()
        })
        .cursor_pointer()
}

fn icon_swatch(icon: Icon, size: f32, color: gpui::Rgba) -> gpui::Div {
    div()
        .size(px(SWATCH))
        .rounded_full()
        .bg(theme::button_bg())
        .flex()
        .items_center()
        .justify_center()
        .child(icon.el(size, color))
}

fn clear_white() -> Hsla {
    hsla(0., 0., 1., 0.)
}

fn clear_black() -> Hsla {
    hsla(0., 0., 0., 0.)
}

impl Editor {
    /// A colour row keyed `key`: `colors` as swatches (a leading "none" one
    /// when `none`), `chosen` ringed. Every pick goes to `pick`.
    pub(crate) fn color_row(
        &self,
        key: &'static str,
        colors: &[&'static str],
        chosen: Option<&str>,
        none: bool,
        pick: impl Fn(&mut Editor, Option<String>, &mut Context<Editor>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pick: ColorFn = Rc::new(pick);
        let picking = self
            .color_pick
            .as_ref()
            .is_some_and(|p| p.owner == key && p.clip.as_deref() == self.selected.primary());
        let spectrum = self.spectrum.as_ref().filter(|s| s.owner == key);

        let p = pick.clone();
        let clip = self.selected.primary().map(str::to_string);
        let eyedropper = ring((key, usize::MAX - 1), picking)
            .tooltip(tooltip("Pick a color from the video", None))
            .child(icon_swatch(Icon::Eyedrop, 13., theme::text()))
            .on_click(cx.listener(move |ed, _, _, cx| {
                ed.color_pick = if picking {
                    None
                } else {
                    Some(ColorPick {
                        owner: key,
                        clip: clip.clone(),
                        apply: p.clone(),
                    })
                };
                cx.notify();
            }));

        let open = spectrum.is_some();
        let custom = ring((key, usize::MAX - 2), open)
            .tooltip(tooltip("Custom color", None))
            .child(icon_swatch(Icon::Palette, 13., theme::text()))
            .on_click(cx.listener(move |ed, _, _, cx| {
                ed.spectrum = (!open).then_some(Spectrum {
                    owner: key,
                    marker: None,
                });
                cx.notify();
            }));

        let divider = div()
            .flex_none()
            .mx(px(4.))
            .w(px(1.))
            .h(px(18.))
            .bg(gpui::rgba(0xffffff40));

        let rest = match spectrum {
            Some(s) => self.spectrum_field(s.marker, pick, cx),
            None => self.swatch_strip(key, colors, chosen, none, pick, cx),
        };

        div()
            .flex()
            .items_center()
            .gap(px(GAP))
            .min_h(px(SWATCH + 12.))
            .py(px(if open { 6. } else { 0. }))
            .child(eyedropper)
            .child(custom)
            .child(divider)
            .child(rest)
            .into_any_element()
    }

    /// The swatches, scrolled sideways; ‹ › page them.
    fn swatch_strip(
        &self,
        key: &'static str,
        colors: &[&'static str],
        chosen: Option<&str>,
        none: bool,
        pick: ColorFn,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scroll = self
            .color_scrolls
            .borrow_mut()
            .entry(key)
            .or_default()
            .clone();

        let mut items: Vec<AnyElement> = Vec::new();
        if none {
            let p = pick.clone();
            items.push(
                ring((key, usize::MAX), chosen.is_none())
                    .tooltip(tooltip("None", None))
                    .child(icon_swatch(Icon::BanOutline, 14., theme::text_secondary()))
                    .on_click(cx.listener(move |ed, _, _, cx| p(ed, None, cx)))
                    .into_any_element(),
            );
        }
        for (i, &c) in colors.iter().enumerate() {
            let p = pick.clone();
            let on = chosen.is_some_and(|x| x.eq_ignore_ascii_case(c));
            items.push(
                ring((key, i), on)
                    .child(div().size(px(SWATCH)).rounded_full().bg(hex(c)))
                    .on_click(cx.listener(move |ed, _, _, cx| p(ed, Some(c.to_string()), cx)))
                    .into_any_element(),
            );
        }

        // Read from the last layout (a scroll re-renders, so they follow).
        let x = -f32::from(scroll.offset().x);
        let max = f32::from(scroll.max_offset().width);
        let can_back = x > 1.;
        let can_forward = max - x > 1.;
        let arrow = |left: bool| {
            let s = scroll.clone();
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .when(left, |d| d.left_0().pr(px(2.)))
                .when(!left, |d| d.right_0().pl(px(2.)))
                .flex()
                .items_center()
                .bg(theme::panel())
                .child(
                    div()
                        .id((key, usize::MAX - if left { 3 } else { 4 }))
                        .w(px(SWATCH * 0.8))
                        .h(px(SWATCH + RING * 4.))
                        .rounded(px(4.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|d| d.bg(theme::button_bg()))
                        .tooltip(tooltip(
                            if left {
                                "Previous colors"
                            } else {
                                "More colors"
                            },
                            None,
                        ))
                        .child(
                            if left {
                                Icon::ChevronBack
                            } else {
                                Icon::ChevronForward
                            }
                            .el(12., theme::text()),
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            let view = f32::from(s.bounds().size.width);
                            let max = f32::from(s.max_offset().width);
                            let x = -f32::from(s.offset().x);
                            let dir = if left { -1. } else { 1. };
                            let to = (x + dir * view * 0.7).clamp(0., max);
                            s.set_offset(point(px(-to), px(0.)));
                            cx.notify();
                        })),
                )
        };

        div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .child(
                div()
                    .id((key, usize::MAX - 5))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .overflow_x_scroll()
                    .track_scroll(&scroll)
                    .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
                    .children(items),
            )
            .when(can_back, |d| d.child(arrow(true)))
            .when(can_forward, |d| d.child(arrow(false)))
            .into_any_element()
    }

    /// The custom-colour field: press or drag anywhere to take a colour.
    fn spectrum_field(
        &self,
        marker: Option<(f32, f32)>,
        pick: ColorFn,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let last = HUES.len() - 1;
        let hues = (0..last).map(|i| {
            div()
                .flex_1()
                .h_full()
                .when(i == 0, |d| d.rounded_l(px(8.)))
                .when(i == last - 1, |d| d.rounded_r(px(8.)))
                .bg(linear_gradient(
                    90.,
                    linear_color_stop(gpui::rgb(HUES[i]), 0.),
                    linear_color_stop(gpui::rgb(HUES[i + 1]), 1.),
                ))
        });
        let shade = |top: bool| {
            div()
                .absolute()
                .left_0()
                .right_0()
                .h_1_2()
                .when(top, |d| {
                    d.top_0().rounded_t(px(8.)).bg(linear_gradient(
                        180.,
                        linear_color_stop(gpui::white(), 0.),
                        linear_color_stop(clear_white(), 1.),
                    ))
                })
                .when(!top, |d| {
                    d.bottom_0().rounded_b(px(8.)).bg(linear_gradient(
                        180.,
                        linear_color_stop(clear_black(), 0.),
                        linear_color_stop(gpui::black(), 1.),
                    ))
                })
        };
        let dot = marker.and_then(|(fx, fy)| {
            let b = self.measured(SPECTRUM_KEY)?;
            Some(
                div()
                    .absolute()
                    .left(px(fx * f32::from(b.size.width) - 9.))
                    .top(px(fy * SPECTRUM_H - 9.))
                    .size(px(18.))
                    .rounded_full()
                    .border_2()
                    .border_color(gpui::white()),
            )
        });

        div()
            .id(SPECTRUM_KEY)
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(SPECTRUM_H))
            .flex()
            .cursor_crosshair()
            .child(self.measure(SPECTRUM_KEY))
            .children(hues)
            .child(shade(true))
            .child(shade(false))
            .children(dot)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    ed.drag = Some(Drag::Spectrum(pick.clone()));
                    ed.spectrum_to(e.position, cx);
                }),
            )
            .into_any_element()
    }

    /// The spectrum dragged to `at`: its colour, live (one undo step).
    pub(crate) fn spectrum_to(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let (Some(b), Some(Drag::Spectrum(apply))) = (self.measured(SPECTRUM_KEY), &self.drag)
        else {
            return;
        };
        let apply = apply.clone();
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        if w <= 0. || h <= 0. {
            return;
        }
        let fx = (f32::from(at.x - b.origin.x) / w).clamp(0., 1.);
        let fy = (f32::from(at.y - b.origin.y) / h).clamp(0., 1.);
        if let Some(s) = self.spectrum.as_mut() {
            s.marker = Some((fx, fy));
        }
        self.coalesce_edits = Some(SPECTRUM_KEY.into());
        apply(self, Some(hsl_to_hex(fx * 360., 1., 1. - fy)), cx);
        self.coalesce_edits = None;
        cx.notify();
    }

    /// A press on the player while the eyedropper waits: the colour of
    /// the picture there goes to its row, and picking ends. False when
    /// nothing was picking.
    pub(crate) fn eyedrop_at(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some(pick) = self.color_pick.take() else {
            return false;
        };
        if pick.clip.as_deref() != self.selected.primary() {
            // Its row went away with the selection.
            return false;
        }
        cx.notify();
        let (Some(area), Some(frame)) = (self.measured("player-area"), self.frame.clone()) else {
            return true;
        };
        let (cw, ch) = (
            self.project.canvas.width.max(1) as f32,
            self.project.canvas.height.max(1) as f32,
        );
        let (aw, ah) = (f32::from(area.size.width), f32::from(area.size.height));
        let k = (aw / cw).min(ah / ch);
        if k <= 0. {
            return true;
        }
        let left = f32::from(area.origin.x) + (aw - cw * k) / 2.;
        let top = f32::from(area.origin.y) + (ah - ch * k) / 2.;
        let fx = (f32::from(at.x) - left) / (cw * k);
        let fy = (f32::from(at.y) - top) / (ch * k);
        if !(0. ..1.).contains(&fx) || !(0. ..1.).contains(&fy) {
            // Off the picture: keep waiting for a press on it.
            self.color_pick = Some(pick);
            return true;
        }
        let (x, y) = (
            (fx * frame.width as f32) as u32,
            (fy * frame.height as f32) as u32,
        );
        // The player's pictures are BGRA.
        if let Some(px) = frame.pixel(x, y) {
            let c = format!("#{:02x}{:02x}{:02x}", px[2], px[1], px[0]);
            (pick.apply)(self, Some(c), cx);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::hsl_to_hex;

    #[test]
    fn spectrum_colors() {
        assert_eq!(hsl_to_hex(0., 1., 0.5), "#ff0000");
        assert_eq!(hsl_to_hex(120., 1., 0.5), "#00ff00");
        assert_eq!(hsl_to_hex(240., 1., 0.5), "#0000ff");
        assert_eq!(hsl_to_hex(360., 1., 0.5), "#ff0000");
        assert_eq!(hsl_to_hex(77., 1., 1.), "#ffffff");
        assert_eq!(hsl_to_hex(200., 1., 0.), "#000000");
    }
}
