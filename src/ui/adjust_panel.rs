//! Adjust (a video or photo): the clip's filter strength when it has one
//! (the look itself is picked in the browser's Filters), then the seven
//! Adjust sliders in Color / Lightness groups with one Reset. Each release
//! is one undo step; on a keyframed clip it keys the playhead and the
//! sliders show the values there. The player follows a held slider.

use std::rc::Rc;

use gpui::{AnyElement, Context, MouseButton, MouseDownEvent, div, prelude::*, px};

use crate::adjust::{self, GROUPS};
use crate::catalog::FILTER_PREFIX;
use crate::editor::{Editor, SliderFn};
use crate::effects::stack::LiveParam;
use crate::model::{Clip, Micros};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets::{
    icon_button, panel_note, panel_row, section_with, slider_track, value_box,
};

const FILTER_SLIDER: &str = "adjust:filter";

impl Editor {
    /// The Adjust matrix of `clip` at timeline time `t` for the player:
    /// keyframed sliders evaluated, a held slider's live value on the
    /// selected clip.
    pub(crate) fn adjust_matrix_at(&self, clip: &Clip, t: Micros) -> Option<[f32; 20]> {
        let selected = self.selected.primary() == Some(clip.id.as_str());
        let local = t - clip.timeline_start;
        adjust::matrix(clip, |c| match self.live_value {
            Some((k, v)) if selected && k == c.slider => v,
            _ => self.property_at(clip, c.property, local).unwrap_or(0.),
        })
    }

    pub fn render_adjust_panel(&self, clip: &Clip, cx: &mut Context<Self>) -> AnyElement {
        let filter = self.render_filter_strength(clip, cx);
        let first = filter.is_none();
        let id = clip.id.clone();
        let head = section_with(
            "Adjust",
            first,
            [self.reset_button(
                "adjust-reset",
                clip.has_adjust(),
                move |ed, cx| {
                    let id = id.clone();
                    ed.edit_clip(&id, "Reset Adjust", |c| c.reset_adjust(), cx);
                    ed.toast("Adjustments reset", cx);
                },
                cx,
            )],
        );
        let groups = GROUPS.iter().map(|(label, keys)| {
            div()
                .flex()
                .flex_col()
                .child(group_label(label))
                .children(keys.iter().filter_map(|key| {
                    let c = adjust::control(key)?;
                    let value = self.shown_property(clip, c.property).unwrap_or(0.).round();
                    let id = clip.id.clone();
                    let property = c.property;
                    Some(self.fx_slider(
                        c.slider,
                        c.label,
                        value,
                        (c.min, c.max),
                        true,
                        "",
                        move |ed, v, cx| {
                            ed.set_clip_properties(&id, "Adjust", &[(property, v)], cx)
                        },
                        cx,
                    ))
                }))
        });
        div()
            .px(px(14.))
            .pb(px(14.))
            .flex()
            .flex_col()
            .children(filter)
            .child(head)
            .children(groups)
            .when(clip.is_animated(), |d| {
                d.child(panel_note("Keyframed · edits apply at the playhead"))
            })
            .into_any_element()
    }

    /// "Filter · <name>" with a remove, and its intensity (the player
    /// follows the held slider; one edit on release).
    fn render_filter_strength(&self, clip: &Clip, cx: &mut Context<Self>) -> Option<AnyElement> {
        let filter = clip.effect_with(FILTER_PREFIX)?;
        let stored = filter.param("intensity").unwrap_or(0.);
        let (shown, dragging) = match self.live_value {
            Some((k, v)) if k == FILTER_SLIDER => (v, true),
            _ => (stored, false),
        };
        let remove_id = clip.id.clone();
        let (clip_id, effect_id) = (clip.id.clone(), filter.id.clone());
        let apply: SliderFn = Rc::new(move |ed, f, done, cx| {
            let v = (f as f64 * 100.).round();
            if done {
                ed.live_value = None;
                ed.live_fx = None;
                if v != stored {
                    ed.set_fx_params(&clip_id, &effect_id, vec![("intensity".into(), v)], cx);
                }
                cx.notify();
            } else {
                ed.live_value = Some((FILTER_SLIDER, v));
                ed.live_fx = Some(LiveParam {
                    clip_id: clip_id.clone(),
                    effect_id: effect_id.clone(),
                    param: "intensity".into(),
                    value: v,
                });
                cx.notify();
            }
        });
        let head = section_with(
            format!("Filter · {}", filter.name()),
            true,
            [icon_button("adjust-filter-remove", Icon::Trash, "Delete")
                .size(14.)
                .on_click(cx.listener(move |ed, _, _, cx| {
                    let id = remove_id.clone();
                    if ed.edit(
                        "Remove Filter",
                        false,
                        None,
                        |p| p.set_clip_effect(&id, FILTER_PREFIX, None),
                        cx,
                    ) {
                        ed.toast("Filter removed", cx);
                    }
                }))
                .into_any_element()],
        );
        let row = panel_row("Intensity")
            .child(
                div()
                    .id(FILTER_SLIDER)
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .cursor_pointer()
                    .child(self.measure(FILTER_SLIDER))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            ed.start_slider(FILTER_SLIDER, e.position.x, apply.clone(), cx);
                        }),
                    )
                    .child(slider_track((shown / 100.) as f32, dragging)),
            )
            .child(value_box(format!("{}", shown.round() as i64)));
        Some(
            div()
                .flex()
                .flex_col()
                .child(head)
                .child(row)
                .into_any_element(),
        )
    }
}

/// A group's small uppercase heading ("COLOR").
fn group_label(label: &str) -> impl IntoElement {
    div()
        .h(px(26.))
        .flex()
        .items_end()
        .pb(px(4.))
        .font_family(theme::font_mono())
        .text_size(px(10.))
        .text_color(theme::text_tertiary())
        .child(crate::i18n::ts(label.to_string()).to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adjust::CONTROLS;

    #[test]
    fn every_grouped_slider_exists_once() {
        let mut keys: Vec<&str> = GROUPS.iter().flat_map(|(_, k)| k.iter().copied()).collect();
        keys.sort_unstable();
        let mut all: Vec<&str> = CONTROLS.iter().map(|c| c.key).collect();
        all.sort_unstable();
        assert_eq!(keys, all);
    }
}
