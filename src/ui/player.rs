//! The Player panel: the project canvas fitted into the panel (letterboxed
//! on near-black), and the transport bar under it: timecode | go to start,
//! previous frame, play, next frame, go to end | ratio, full screen.

use gpui::{
    AnyElement, Context, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, Rgba, div,
    prelude::*, px, rgb,
};

use crate::editor::{Editor, PopupItem, timecode};
use crate::shortcuts::Shortcut;
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets::{icon_button, mono, panel_header};

/// The canvas ratios offered (label, width / height). `None` is Original:
/// the first clip's own shape.
pub const ASPECT_PRESETS: &[(&str, Option<f64>)] = &[
    ("Original", None),
    ("9:16", Some(9. / 16.)),
    ("1:1", Some(1.)),
    ("16:9", Some(16. / 9.)),
    ("4:3", Some(4. / 3.)),
    ("3:4", Some(3. / 4.)),
    ("5.8\"", Some(1125. / 2436.)),
    ("2:1", Some(2.)),
    ("2.35:1", Some(2.35)),
    ("1.85:1", Some(1.85)),
];

/// Canvas size for a ratio: the short side is 1080, the long side follows
/// (rounded to even, as encoders need).
pub fn canvas_size_for_ratio(ratio: f64) -> (u32, u32) {
    let even = |n: f64| ((n / 2.).round() * 2.).max(2.) as u32;
    if ratio >= 1. {
        (even(1080. * ratio), 1080)
    } else {
        (1080, even(1080. / ratio))
    }
}

/// "9:16" for a canvas matching a preset, else "1.33:1".
pub fn aspect_label(width: u32, height: u32) -> String {
    if height == 0 {
        return String::new();
    }
    let r = width as f64 / height as f64;
    ASPECT_PRESETS
        .iter()
        .find(|(_, p)| p.is_some_and(|p| (p - r).abs() < 0.006))
        .map(|(l, _)| l.to_string())
        .unwrap_or_else(|| format!("{r:.2}:1"))
}

/// "#rrggbb" → colour (black when it doesn't parse).
fn hex_color(s: &str) -> Rgba {
    u32::from_str_radix(s.trim_start_matches('#'), 16)
        .ok()
        .filter(|_| s.trim_start_matches('#').len() == 6)
        .map(rgb)
        .unwrap_or(rgb(0x000000))
}

impl Editor {
    pub fn render_player(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // The canvas, fitted into the area measured on the last frame.
        let area = self.measured("player-area");
        let (cw, ch) = (
            self.project.canvas.width.max(1) as f32,
            self.project.canvas.height.max(1) as f32,
        );
        let canvas_size = area.map(|b| {
            let (aw, ah) = (f32::from(b.size.width), f32::from(b.size.height));
            let scale = (aw / cw).min(ah / ch);
            (cw * scale, ch * scale)
        });

        let picture: Option<AnyElement> = self.frame.as_ref().map(|frame| frame.element());

        let canvas = div()
            .relative()
            .overflow_hidden()
            .bg(hex_color(&self.project.canvas.background_color))
            .map(|d| match canvas_size {
                Some((w, h)) => d.w(px(w)).h(px(h)),
                None => d.size_full(),
            })
            .children(picture);

        let empty = self.project.duration == 0;

        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(8.))
            .border_1()
            .border_color(theme::card_border())
            .bg(theme::panel())
            .when(!self.fullscreen, |d| d.child(panel_header("Player", 30.)))
            .child(
                div()
                    .id("player-area")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .bg(theme::player_bg())
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .cursor(if self.color_pick.is_some() {
                        gpui::CursorStyle::Crosshair
                    } else {
                        self.gizmo_cursor()
                    })
                    // Layers are picked, moved, scaled and turned here
                    // (`ui/gizmo.rs`).
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|ed, e: &MouseDownEvent, _, cx| {
                            // The eyedropper takes the press when it waits.
                            if !ed.eyedrop_at(e.position, cx) {
                                ed.gizmo_mouse_down(e, cx)
                            }
                        }),
                    )
                    .on_mouse_move(cx.listener(|ed, e: &MouseMoveEvent, _, cx| {
                        if ed.drag.is_none() {
                            ed.gizmo_hover_at(e.position, cx);
                        }
                    }))
                    .on_hover(cx.listener(|ed, hovered: &bool, _, cx| {
                        if !*hovered && ed.gizmo_hover.take().is_some() {
                            cx.notify();
                        }
                    }))
                    .child(self.measure("player-area"))
                    .child(canvas)
                    .when(!empty, |d| d.children(self.render_gizmo(cx)))
                    .when(empty, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(theme::SM))
                                .text_color(theme::text_tertiary())
                                .child("Import a video to start"),
                        )
                    }),
            )
            .child(self.render_transport(cx))
    }

    fn render_transport(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let fps = self.fps();
        let playing = self.engine.is_playing();
        let ratio = aspect_label(self.project.canvas.width, self.project.canvas.height);

        div()
            .flex_none()
            .h(px(40.))
            .px(px(12.))
            .flex()
            .items_center()
            .border_t_1()
            .border_color(theme::card_border())
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(mono(timecode(self.playhead, fps), theme::SM, theme::text()))
                    .child(mono(
                        format!(" | {}", timecode(self.project.duration, fps)),
                        theme::SM,
                        theme::text_secondary(),
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        icon_button("go-start", Icon::SkipBack, "Go to start")
                            .shortcut(Shortcut::GoStart)
                            .on_click(cx.listener(|ed, _, w, cx| ed.run(Shortcut::GoStart, w, cx))),
                    )
                    .child(
                        icon_button("prev-frame", Icon::CaretBack, "Previous frame")
                            .size(17.)
                            .shortcut(Shortcut::PrevFrame)
                            .on_click(
                                cx.listener(|ed, _, w, cx| ed.run(Shortcut::PrevFrame, w, cx)),
                            ),
                    )
                    .child(
                        icon_button(
                            "play",
                            if playing { Icon::Pause } else { Icon::Play },
                            if playing { "Pause" } else { "Play" },
                        )
                        .size(18.)
                        .shortcut(Shortcut::PlayPause)
                        .on_click(cx.listener(|ed, _, w, cx| ed.run(Shortcut::PlayPause, w, cx))),
                    )
                    .child(
                        icon_button("next-frame", Icon::CaretForward, "Next frame")
                            .size(17.)
                            .shortcut(Shortcut::NextFrame)
                            .on_click(
                                cx.listener(|ed, _, w, cx| ed.run(Shortcut::NextFrame, w, cx)),
                            ),
                    )
                    .child(
                        icon_button("go-end", Icon::SkipForward, "Go to end")
                            .shortcut(Shortcut::GoEnd)
                            .on_click(cx.listener(|ed, _, w, cx| ed.run(Shortcut::GoEnd, w, cx))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .child(
                        div()
                            .id("ratio")
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(theme::text_secondary())
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::button_hover()))
                            .font_family(theme::font_mono())
                            .text_size(px(theme::SM))
                            .font_weight(FontWeight::NORMAL)
                            .on_click(cx.listener(|ed, e: &gpui::ClickEvent, _, cx| {
                                ed.open_ratio_menu(e.position(), cx)
                            }))
                            .child(ratio),
                    )
                    .child(
                        icon_button(
                            "fullscreen",
                            if self.fullscreen {
                                Icon::Contract
                            } else {
                                Icon::Expand
                            },
                            if self.fullscreen {
                                "Exit full screen"
                            } else {
                                "Full screen preview"
                            },
                        )
                        .shortcut(Shortcut::Fullscreen)
                        .on_click(cx.listener(|ed, _, w, cx| ed.run(Shortcut::Fullscreen, w, cx))),
                    ),
            )
    }

    /// The Ratio picker (transport badge, Details › Modify).
    pub fn open_ratio_menu(&mut self, at: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let current = aspect_label(self.project.canvas.width, self.project.canvas.height);
        let items = ASPECT_PRESETS
            .iter()
            .map(|&(label, ratio)| {
                PopupItem::new(label, move |ed: &mut Editor, _, cx| {
                    ed.set_canvas_ratio(ratio, cx)
                })
                .checked(label == current)
            })
            .collect();
        self.open_popup(at, items, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios() {
        assert_eq!(canvas_size_for_ratio(9. / 16.), (1080, 1920));
        assert_eq!(canvas_size_for_ratio(16. / 9.), (1920, 1080));
        assert_eq!(aspect_label(1080, 1920), "9:16");
        assert_eq!(aspect_label(1000, 750), "4:3");
        assert_eq!(aspect_label(1000, 700), "1.43:1");
    }
}
