//! The right panel. Nothing selected: the project's Details. A clip
//! selected: its tabs (Visual / Audio / Speed / Animation / Adjust…) and
//! sub-tabs for that kind of clip, from one table like the old editor's,
//! with the chosen sub-tab's settings below.

use std::rc::Rc;

use gpui::{Context, FontWeight, MouseButton, MouseDownEvent, SharedString, div, prelude::*, px};

use crate::editor::{Editor, SliderFn, timecode};
use crate::model::{Clip, SECOND};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::player::aspect_label;
use crate::ui::widgets::{
    OnClick, icon_button, panel_header, panel_row, pill_button, section, segment_tabs,
    slider_track, value_box,
};

/// One sub-tab: a key the panel body matches on, and its label.
pub struct Entry {
    pub key: &'static str,
    pub label: &'static str,
    /// Only for a clip on the main lane (Background).
    pub main_lane: bool,
    /// Only for a clip on an overlay video lane (Splice blends with what's
    /// under it; the main lane has nothing under it).
    pub overlay: bool,
}

pub struct Tab {
    pub label: &'static str,
    pub entries: &'static [Entry],
}

const fn e(key: &'static str, label: &'static str) -> Entry {
    Entry {
        key,
        label,
        main_lane: false,
        overlay: false,
    }
}

const VISUAL_CLIP: Tab = Tab {
    label: "Visual",
    entries: &[
        e("basic", "Basic"),
        e("rotate", "Rotate"),
        e("removeBg", "Remove BG"),
        e("mask", "Mask"),
        Entry {
            overlay: true,
            ..e("splice", "Splice")
        },
        Entry {
            main_lane: true,
            ..e("background", "Background")
        },
    ],
};
// Video clips also get Face Swap (it follows a person through frames).
const VISUAL_VIDEO: Tab = Tab {
    label: "Visual",
    entries: &[
        e("basic", "Basic"),
        e("rotate", "Rotate"),
        e("removeBg", "Remove BG"),
        e("faceSwap", "Face Swap"),
        e("mask", "Mask"),
        Entry {
            overlay: true,
            ..e("splice", "Splice")
        },
        Entry {
            main_lane: true,
            ..e("background", "Background")
        },
    ],
};
const VISUAL_BASIC: Tab = Tab {
    label: "Visual",
    entries: &[e("basic", "Basic")],
};
const ANIMATE_CLIP: Tab = Tab {
    label: "Animation",
    entries: &[e("animation", "Animation")],
};
const ADJUST_CLIP: Tab = Tab {
    label: "Adjust",
    entries: &[e("adjust", "Adjust")],
};

const VIDEO_TABS: &[Tab] = &[
    VISUAL_VIDEO,
    Tab {
        label: "Audio",
        entries: &[e("volume", "Volume")],
    },
    Tab {
        label: "Speed",
        entries: &[e("speed", "Normal"), e("speedCurve", "Curve")],
    },
    ANIMATE_CLIP,
    ADJUST_CLIP,
];
const IMAGE_TABS: &[Tab] = &[
    VISUAL_CLIP,
    Tab {
        label: "Speed",
        entries: &[e("imageSpeed", "Speed")],
    },
    ANIMATE_CLIP,
    ADJUST_CLIP,
];
const TEXT_TABS: &[Tab] = &[
    Tab {
        label: "Text",
        entries: &[
            e("text:basic", "Basic"),
            e("text:templates", "Templates"),
            e("text:fonts", "Fonts"),
            e("text:effects", "Effects"),
            e("text:bubbles", "Bubbles"),
        ],
    },
    Tab {
        label: "Animation",
        entries: &[e("text:animations", "Animations")],
    },
    VISUAL_BASIC,
];
// A caption's look is edited on Text; its styles are picked in the
// browser (Captions › Style), so they aren't repeated here.
const CAPTION_TABS: &[Tab] = &[
    Tab {
        label: "Text",
        entries: &[
            e("text:basic", "Basic"),
            e("text:fonts", "Fonts"),
            e("text:effects", "Effects"),
            e("text:bubbles", "Bubbles"),
        ],
    },
    Tab {
        label: "Edit captions",
        entries: &[e("captionEdit", "Edit captions")],
    },
    Tab {
        label: "Animation",
        entries: &[e("text:animations", "Animations")],
    },
];
const AUDIO_TABS: &[Tab] = &[Tab {
    label: "Audio",
    entries: &[
        e("volume", "Volume"),
        e("beats", "Beats"),
        e("audioDuration", "Duration"),
    ],
}];
// Effects are picked in the browser (left); the inspector only holds the
// picked one's settings, never a second grid.
const EFFECT_TABS: &[Tab] = &[Tab {
    label: "Effects",
    entries: &[e("effects", "Adjust")],
}];
const STICKER_TABS: &[Tab] = &[VISUAL_BASIC];
const DRAWING_TABS: &[Tab] = &[Tab {
    label: "Animation",
    entries: &[e("drawAnim", "Animations")],
}];

/// The inspector's tabs for a kind of clip (the old editor's table).
pub fn tabs_for(kind: &str) -> &'static [Tab] {
    match kind {
        "video" => VIDEO_TABS,
        "image" => IMAGE_TABS,
        "text" => TEXT_TABS,
        "audio" => AUDIO_TABS,
        "effect" => EFFECT_TABS,
        "sticker" => STICKER_TABS,
        "drawing" => DRAWING_TABS,
        _ => &[],
    }
}

/// The speed slider runs in log10 space, 0.1×–100×.
pub const MIN_SPEED: f64 = 0.1;
pub const MAX_SPEED: f64 = 100.;

fn speed_to_fraction(speed: f64) -> f32 {
    let (lo, hi) = (MIN_SPEED.log10(), MAX_SPEED.log10());
    ((speed.clamp(MIN_SPEED, MAX_SPEED).log10() - lo) / (hi - lo)) as f32
}

/// Slider position → speed: readable precision, and exactly 1× near it.
fn fraction_to_speed(f: f32) -> f64 {
    let (lo, hi) = (MIN_SPEED.log10(), MAX_SPEED.log10());
    let raw = 10f64
        .powf(lo + f as f64 * (hi - lo))
        .clamp(MIN_SPEED, MAX_SPEED);
    if (raw - 1.).abs() < 0.03 {
        1.
    } else if raw < 1. {
        (raw * 100.).round() / 100.
    } else if raw < 10. {
        (raw * 10.).round() / 10.
    } else {
        raw.round()
    }
}

pub(crate) fn secs(t: i64) -> String {
    format!("{:.1}s", t as f64 / SECOND as f64)
}

impl Editor {
    pub fn render_inspector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        match self.selected_clip() {
            Some((track, clip)) => {
                let main = self.project.is_main_track(&track.id);
                let overlay = track.is_overlay_video(&self.project.id);
                self.render_clip_inspector(clip, main, overlay, cx)
                    .into_any_element()
            }
            None => self.render_details(cx).into_any_element(),
        }
    }

    /// Nothing selected: what the project is, with Modify opening the
    /// ratio picker.
    fn render_details(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &self.project;
        let clips: usize = p.tracks.iter().map(|t| t.clips.len()).sum();
        let rows: Vec<(&str, String)> = vec![
            ("Name", p.name.clone()),
            (
                "Aspect ratio",
                aspect_label(p.canvas.width, p.canvas.height),
            ),
            (
                "Resolution",
                format!("{} × {}", p.canvas.width, p.canvas.height),
            ),
            ("Frame rate", format!("{} fps", p.canvas.fps)),
            ("Duration", timecode(p.duration, self.fps())),
            ("Clips", clips.to_string()),
        ];
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(panel_header("Details", 34.))
            .child(
                div()
                    .id("details")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(16.))
                    .py(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .children(rows.into_iter().map(|(label, value)| {
                        div()
                            .flex()
                            .gap_3()
                            .text_size(px(theme::XS))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(104.))
                                    .text_color(theme::text_tertiary())
                                    .child(crate::i18n::ts(label)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_color(theme::text())
                                    .child(value),
                            )
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(44.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .justify_end()
                    .border_t_1()
                    .border_color(theme::card_border())
                    .child(
                        pill_button("modify", "Modify", false)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|ed, e: &gpui::ClickEvent, _, cx| {
                                ed.open_ratio_menu(e.position(), cx)
                            })),
                    ),
            )
    }

    fn render_clip_inspector(
        &self,
        clip: &Clip,
        main_lane: bool,
        overlay: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let table = if clip.is_caption() {
            CAPTION_TABS
        } else {
            tabs_for(&clip.kind)
        };
        let tabs: Vec<(&Tab, Vec<&Entry>)> = table
            .iter()
            .map(|t| {
                (
                    t,
                    t.entries
                        .iter()
                        .filter(|e| (!e.main_lane || main_lane) && (!e.overlay || overlay))
                        .collect::<Vec<_>>(),
                )
            })
            .filter(|(_, entries)| !entries.is_empty())
            .collect();
        // The open sub-tab, else this clip's first.
        let current = self
            .inspector_entry
            .filter(|k| tabs.iter().any(|(_, es)| es.iter().any(|e| e.key == *k)))
            .or_else(|| tabs.first().and_then(|(_, es)| es.first()).map(|e| e.key));
        // Arrange layers sits in the body instead: no tab is lit.
        let layers = self.layers_panel.is_some();
        let current_tab = tabs
            .iter()
            .filter(|_| !layers)
            .find(|(_, es)| es.iter().any(|e| Some(e.key) == current));

        // Tabs like a browser's: a darker strip, the open tab raised in the
        // panel's own fill with rounded top corners, its bottom corners
        // flaring out (concave) so it runs into the body below. The flare
        // is about the top corner's size, so each side reads as one S
        // curve; a tab under the pointer lights an inset pill instead.
        const TOP: f32 = 12.;
        const FOOT: f32 = 10.;
        let strip = theme::app_bg();
        let foot = |left: bool| {
            let cut = div().size_full().bg(strip);
            div()
                .absolute()
                .bottom_0()
                .size(px(FOOT))
                .bg(theme::sheet())
                .when(left, |d| d.left(px(-FOOT)))
                .when(!left, |d| d.right(px(-FOOT)))
                .child(if left {
                    cut.rounded_br(px(FOOT))
                } else {
                    cut.rounded_bl(px(FOOT))
                })
        };
        let tab_row = div()
            .flex_none()
            .h(px(40.))
            .pt(px(6.))
            .px(px(12.))
            .flex()
            .items_end()
            .gap(px(6.))
            .bg(strip)
            .children(tabs.iter().enumerate().map(|(i, (tab, entries))| {
                let on = current_tab.is_some_and(|(t, _)| std::ptr::eq(*t, *tab));
                let first = entries[0].key;
                let group = SharedString::from(format!("inspector-tab-{i}"));
                let label = div()
                    .h(px(26.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded(px(8.))
                    .child(crate::i18n::ts(tab.label));
                div()
                    .id(("inspector-tab", i))
                    .group(group.clone())
                    .relative()
                    .h_full()
                    .px(px(4.))
                    .flex()
                    .items_center()
                    .rounded_t(px(TOP))
                    .cursor_pointer()
                    .text_size(px(theme::SM))
                    .map(|d| {
                        if on {
                            d.bg(theme::sheet())
                                .text_color(theme::text())
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(foot(true))
                                .child(foot(false))
                                .child(label)
                        } else {
                            d.text_color(theme::text_secondary()).child(
                                label.group_hover(group, |s| {
                                    s.bg(theme::fill_subtle()).text_color(theme::text())
                                }),
                            )
                        }
                    })
                    .on_click(cx.listener(move |ed, _, _, cx| {
                        ed.inspector_entry = Some(first);
                        ed.layers_panel = None;
                        cx.notify();
                    }))
            }));

        let sub_tabs = current_tab
            .filter(|(_, es)| es.len() > 1)
            .map(|(_, entries)| {
                segment_tabs(
                    entries
                        .iter()
                        .map(|entry| {
                            let key = entry.key;
                            let f: OnClick = Box::new(cx.listener(move |ed, _, _, cx| {
                                ed.inspector_entry = Some(key);
                                cx.notify();
                            }));
                            (SharedString::from(entry.label), Some(key) == current, f)
                        })
                        .collect(),
                )
            });

        let body = match current {
            _ if layers => self.render_layers_panel(cx),
            Some("speed") => self.render_speed_panel(clip, cx).into_any_element(),
            Some("speedCurve") => self.render_speed_curve_panel(clip, cx),
            Some("imageSpeed") => self.render_image_speed_panel(),
            Some("adjust") => self.render_adjust_panel(clip, cx),
            Some("basic") => self.render_basic_panel(clip, cx),
            Some("rotate") => self.render_rotate_panel(clip, main_lane, cx),
            Some("removeBg") => self.render_cutout_panel(clip, cx),
            Some("faceSwap") => self.render_face_swap_panel(clip, cx),
            Some("mask") => self.render_mask_panel(clip, cx),
            Some("splice") => self.render_splice_panel(clip, cx),
            Some("background") => self.render_background_panel(clip, cx),
            Some("effects") => self.render_effect_settings(cx),
            Some("volume") => self.render_volume_panel(clip, cx),
            Some("text:basic") => self.render_text_panel(clip, cx),
            Some("captionEdit") => self.render_caption_edit(clip, cx),
            Some(key) => {
                let label = tabs
                    .iter()
                    .flat_map(|(_, es)| es.iter())
                    .find(|e| e.key == key)
                    .map_or("", |e| e.label);
                not_yet(label).into_any_element()
            }
            None => not_yet(&clip.kind).into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(tab_row)
            .children(sub_tabs)
            .child(
                div()
                    .id("inspector-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
    }

    /// Speed › Normal: one speed (log slider + read-out) and the clip's
    /// length before → after. Changing it rescales the clip and, on the
    /// main lane with the magnet on, closes up after it.
    fn render_speed_panel(&self, clip: &Clip, cx: &mut Context<Self>) -> impl IntoElement {
        let speed = if clip.speed > 0. { clip.speed } else { 1. };
        let shown = match self.live_value {
            Some(("speed", v)) => v,
            _ => speed,
        };
        let dragging = matches!(self.live_value, Some(("speed", _)));
        let original = (clip.timeline_duration as f64 * speed) as i64;
        let after = (original as f64 / shown) as i64;
        let id = clip.id.clone();
        let reset_id = clip.id.clone();
        let apply: SliderFn = Rc::new(move |ed, f, done, cx| {
            let v = fraction_to_speed(f);
            if done {
                ed.live_value = None;
                ed.set_speed(&id, v, cx);
            } else {
                ed.live_value = Some(("speed", v));
                cx.notify();
            }
        });

        div()
            .px(px(14.))
            .pb(px(14.))
            .flex()
            .flex_col()
            .child(
                section("Speed", true).child(
                    div().flex_1().flex().justify_end().child(
                        icon_button("speed-reset", Icon::Reload, "Reset")
                            .size(14.)
                            .disabled(speed == 1.)
                            .on_click(
                                cx.listener(move |ed, _, _, cx| ed.set_speed(&reset_id, 1., cx)),
                            ),
                    ),
                ),
            )
            .child(
                panel_row("Speed")
                    .child(
                        div()
                            .id("slider:speed")
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .cursor_pointer()
                            .child(self.measure("slider:speed"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    ed.start_slider(
                                        "slider:speed",
                                        e.position.x,
                                        apply.clone(),
                                        cx,
                                    );
                                }),
                            )
                            .child(slider_track(speed_to_fraction(shown), dragging)),
                    )
                    .child(value_box(format!("{shown:.2}×"))),
            )
            .child(
                panel_row("Duration").child(
                    div()
                        .flex()
                        .gap_1()
                        .font_family(theme::font_mono())
                        .text_size(px(theme::XS))
                        .child(
                            div()
                                .text_color(theme::text_tertiary())
                                .child(secs(original)),
                        )
                        .child(div().text_color(theme::text_tertiary()).child("→"))
                        .child(div().text_color(theme::text()).child(secs(after))),
                ),
            )
    }
}

/// A settings panel that isn't ported yet.
fn not_yet(label: &str) -> impl IntoElement {
    div()
        .p(px(16.))
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(px(theme::SM))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text_secondary())
                .child(crate::i18n::ts(label.to_string())),
        )
        .child(
            div()
                .text_size(px(theme::XS))
                .text_color(theme::text_tertiary())
                .child("Not in the native editor yet"),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_scale_snaps_and_round_trips() {
        assert_eq!(fraction_to_speed(speed_to_fraction(1.)), 1.);
        assert_eq!(fraction_to_speed(speed_to_fraction(2.)), 2.);
        assert_eq!(fraction_to_speed(speed_to_fraction(0.5)), 0.5);
        assert_eq!(fraction_to_speed(0.), MIN_SPEED);
        assert_eq!(fraction_to_speed(1.), MAX_SPEED);
    }

    #[test]
    fn every_kind_opens_somewhere() {
        for kind in [
            "video", "image", "text", "audio", "effect", "sticker", "drawing",
        ] {
            assert!(!tabs_for(kind).is_empty(), "{kind}");
        }
    }
}
