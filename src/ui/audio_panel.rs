//! The inspector's Audio › Volume for a video or audio clip (the old
//! desktop's): Volume — slider (100 % at its middle, up to 1000 %) and
//! read-out, mute — and Fade in / Fade out in seconds. Each control is
//! one undo step on release.

use std::rc::Rc;

use gpui::{AnyElement, Context, MouseButton, MouseDownEvent, div, prelude::*, px};
use serde_json::Value;

use crate::audio::{MAX_FADE, MAX_VOLUME};
use crate::editor::{Editor, SliderFn};
use crate::model::{Clip, Micros, SECOND};
use crate::ui::icons::Icon;
use crate::ui::widgets::{icon_button, panel_row, section_with, slider_track, value_box};

/// The volume slider: the first half 0–100 %, the second 100–1000 %.
pub fn volume_to_position(v: f64) -> f32 {
    let v = v.clamp(0., MAX_VOLUME);
    (if v <= 1. {
        v * 0.5
    } else {
        0.5 + (v - 1.) / (MAX_VOLUME - 1.) * 0.5
    }) as f32
}

pub fn position_to_volume(p: f32) -> f64 {
    let p = p.clamp(0., 1.) as f64;
    let v = if p <= 0.5 {
        p / 0.5
    } else {
        1. + (p - 0.5) / 0.5 * (MAX_VOLUME - 1.)
    };
    (v * 100.).round() / 100.
}

/// The longest fade one side can have, leaving the other side's.
fn max_fade(duration: Micros, other: Micros) -> Micros {
    (duration - other).clamp(0, MAX_FADE)
}

impl Editor {
    fn set_audio(
        &mut self,
        clip_id: &str,
        label: &'static str,
        patch: Vec<(&'static str, Value)>,
        cx: &mut Context<Self>,
    ) {
        self.edit_clip(
            clip_id,
            label,
            move |c| {
                let before = c.extra.clone();
                for (k, v) in patch {
                    c.extra.insert(k.into(), v);
                }
                c.extra != before
            },
            cx,
        );
    }

    fn set_fades(
        &mut self,
        clip_id: &str,
        fade_in: Micros,
        fade_out: Micros,
        cx: &mut Context<Self>,
    ) {
        let v = serde_json::json!({ "inDurationMicros": fade_in, "outDurationMicros": fade_out });
        self.set_audio(clip_id, "Fade", vec![("audioFade", v)], cx);
    }

    /// A slider with its own scale: `to_value` maps 0–1 to the value,
    /// `text` shows it; `commit` gets it on release.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn scaled_slider(
        &self,
        key: &'static str,
        label: &str,
        value: f64,
        fraction: impl Fn(f64) -> f32,
        to_value: impl Fn(f32) -> f64 + 'static,
        text: impl Fn(f64) -> String,
        disabled: bool,
        commit: impl Fn(&mut Editor, f64, &mut Context<Editor>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (shown, dragging) = match self.live_value {
            Some((k, v)) if k == key => (v, true),
            _ => (value, false),
        };
        let start = value;
        let apply: SliderFn = Rc::new(move |ed, f, done, cx| {
            let v = to_value(f);
            if done {
                ed.live_value = None;
                if v != start {
                    commit(ed, v, cx);
                }
            } else {
                ed.live_value = Some((key, v));
            }
            cx.notify();
        });
        panel_row(label.to_string())
            .child(
                div()
                    .id(key)
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .when(disabled, |d| d.opacity(0.4))
                    .when(!disabled, |d| d.cursor_pointer())
                    .child(self.measure(key))
                    .when(!disabled, |d| {
                        d.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                ed.start_slider(key, e.position.x, apply.clone(), cx);
                            }),
                        )
                    })
                    .child(slider_track(fraction(shown), dragging)),
            )
            .child(value_box(text(shown)))
            .into_any_element()
    }

    /// Audio › Volume.
    pub fn render_volume_panel(&self, clip: &Clip, cx: &mut Context<Self>) -> AnyElement {
        let muted = clip.muted();
        let volume = clip.volume();
        let shown_volume = if muted { 0. } else { volume };
        let (fade_in, fade_out) = clip.audio_fade();
        let duration = clip.timeline_duration;
        let max_in = max_fade(duration, fade_out);
        let max_out = max_fade(duration, fade_in);
        let id = clip.id.clone();

        let (a, b) = (id.clone(), id.clone());
        let volume_head = section_with(
            "Volume",
            true,
            [
                icon_button(
                    "volume-mute",
                    if muted {
                        Icon::VolumeMute
                    } else {
                        Icon::VolumeHigh
                    },
                    "Mute",
                )
                .size(14.)
                .active(muted)
                .on_click(cx.listener(move |ed, _, _, cx| {
                    ed.set_audio(
                        &a,
                        if muted { "Unmute" } else { "Mute" },
                        vec![("muted", (!muted).into())],
                        cx,
                    )
                }))
                .into_any_element(),
                self.reset_button(
                    "volume-reset",
                    volume != 1. || muted,
                    move |ed, cx| {
                        ed.set_audio(
                            &b,
                            "Volume",
                            vec![("volume", 1.into()), ("muted", false.into())],
                            cx,
                        )
                    },
                    cx,
                ),
            ],
        );
        let c = id.clone();
        let volume_row = self.scaled_slider(
            "audio:volume",
            "Volume",
            shown_volume,
            volume_to_position,
            position_to_volume,
            |v| format!("{}%", (v * 100.).round() as i64),
            false,
            move |ed, v, cx| {
                // Setting a level on a muted clip unmutes it.
                ed.set_audio(
                    &c,
                    "Volume",
                    vec![("volume", v.into()), ("muted", false.into())],
                    cx,
                )
            },
            cx,
        );

        let r = id.clone();
        let fade_head = section_with(
            "Fade",
            false,
            [self.reset_button(
                "fade-reset",
                fade_in > 0 || fade_out > 0,
                move |ed, cx| ed.set_fades(&r, 0, 0, cx),
                cx,
            )],
        );
        let secs = |t: Micros| t as f64 / SECOND as f64;
        let fade_slider = |key: &'static str,
                           label: &str,
                           value: Micros,
                           max: Micros,
                           inward: bool,
                           cx: &mut Context<Self>| {
            let id = id.clone();
            let max_s = secs(max).max(0.1);
            self.scaled_slider(
                key,
                label,
                secs(value),
                move |v| (v / max_s) as f32,
                move |f| ((f as f64 * max_s) * 10.).round() / 10.,
                |v| format!("{v:.1}s"),
                max <= 0,
                move |ed, v, cx| {
                    let t = ((v * SECOND as f64) as Micros).min(max);
                    let Some((_, c)) = ed.project.find_clip(&id) else {
                        return;
                    };
                    let (fi, fo) = c.audio_fade();
                    if inward {
                        ed.set_fades(&id, t, fo, cx);
                    } else {
                        ed.set_fades(&id, fi, t, cx);
                    }
                },
                cx,
            )
        };
        let fade_in_row = fade_slider("audio:fade-in", "Fade in", fade_in, max_in, true, cx);
        let fade_out_row = fade_slider("audio:fade-out", "Fade out", fade_out, max_out, false, cx);

        div()
            .px(px(14.))
            .pb(px(14.))
            .flex()
            .flex_col()
            .child(volume_head)
            .child(volume_row)
            .child(fade_head)
            .child(fade_in_row)
            .child(fade_out_row)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_scale_has_unity_in_the_middle() {
        assert_eq!(volume_to_position(1.), 0.5);
        assert_eq!(position_to_volume(0.5), 1.);
        assert_eq!(position_to_volume(1.), MAX_VOLUME);
        assert_eq!(position_to_volume(0.25), 0.5);
        assert!((volume_to_position(5.5) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn fades_leave_room_for_each_other() {
        assert_eq!(max_fade(4 * SECOND, SECOND), 3 * SECOND);
        assert_eq!(max_fade(30 * SECOND, 0), MAX_FADE);
        assert_eq!(max_fade(SECOND, 2 * SECOND), 0);
    }
}
