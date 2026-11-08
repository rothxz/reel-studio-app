//! The browser's Transitions tab: the transition picker aimed at the main
//! lane's cut nearest the playhead (named above the grid), its categories
//! folded under the view in the side nav. A pick puts it on that cut and
//! replays the cut; the picked tile again (or None) takes it off. Once one
//! is on, its length and "Apply to all" sit under the grid.

use std::rc::Rc;

use gpui::{
    AnyElement, Context, FontWeight, MouseButton, MouseDownEvent, ObjectFit, SharedString, div,
    img, prelude::*, px, rgba,
};

use crate::editor::{Editor, SliderFn, timecode};
use crate::i18n::{t, tf};
use crate::model::{Micros, SECOND};
use crate::theme;
use crate::transitions::{self, CATEGORIES, Preset, REPLAY_LEAD, TRENDING};
use crate::ui::browser::TRANSITIONS_TAB;
use crate::ui::icons::Icon;
use crate::ui::widgets::{panel_row, pill_button, value_box};

const TILE_MIN: f32 = 92.;
const TILE_GAP: f32 = 8.;
const GRID_INSET: f32 = 12.;
const GRID_KEY: &str = "tr-grid";
const DURATION_KEY: &str = "tr:duration";

/// The cut a pick applies to: its outgoing clip, the cut's time, what's on
/// it, the longest it takes, and how many cuts there are.
struct Cut {
    clip_id: String,
    at: Micros,
    current: Option<transitions::Transition>,
    max: Micros,
    count: usize,
}

impl Editor {
    fn transition_cut(&self) -> Option<Cut> {
        let seams = transitions::seams(&self.project);
        let id = transitions::nearest_seam(&self.project, self.playhead)?;
        let (o, i) = seams.iter().find(|(o, _)| o.id == id)?;
        Some(Cut {
            clip_id: id.clone(),
            at: o.end(),
            current: transitions::of(o),
            max: transitions::max_micros(o, i),
            count: seams.len(),
        })
    }

    pub fn render_transitions_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let ci = self.browser_categories[TRANSITIONS_TAB].min(CATEGORIES.len() - 1);
        let (key, label) = CATEGORIES[ci];
        let heading = div()
            .flex_none()
            .h(px(36.))
            .px(px(GRID_INSET))
            .flex()
            .items_end()
            .text_size(px(theme::SM))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::text())
            .child(t(label));
        let Some(cut) = self.transition_cut() else {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(heading)
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .px(px(16.))
                        .text_size(px(theme::XS))
                        .text_color(theme::text_tertiary())
                        .text_center()
                        .child(t(
                            "Add a second clip to the main track to put a transition between them",
                        )),
                )
                .into_any_element();
        };

        let active = cut
            .current
            .as_ref()
            .map(|c| transitions::get(&c.kind).id.as_str());
        let mut tiles: Vec<(Option<&'static Preset>, bool)> = vec![];
        if key == TRENDING {
            tiles.push((None, active.is_none()));
        }
        tiles.extend(
            transitions::in_category(key)
                .into_iter()
                .map(|p| (Some(p), active == Some(p.id.as_str()))),
        );

        let width = self
            .measured(GRID_KEY)
            .map_or(0., |b| f32::from(b.size.width) - GRID_INSET * 2.);
        let columns = ((width + TILE_GAP) / (TILE_MIN + TILE_GAP)).floor().max(1.);
        let size = if width > 0. {
            ((width - TILE_GAP * (columns - 1.)) / columns).floor()
        } else {
            TILE_MIN
        };
        let fps = self.fps();
        let grid = div()
            .id(GRID_KEY)
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(self.measure(GRID_KEY))
            .child(
                div()
                    .px(px(GRID_INSET))
                    .pt(px(4.))
                    .text_size(px(theme::XS))
                    .text_color(theme::text_secondary())
                    .child(tf(
                        "Applies to the cut at {time}",
                        &[("time", &timecode(cut.at, fps))],
                    )),
            )
            .child(
                div()
                    .p(px(GRID_INSET))
                    .flex()
                    .flex_wrap()
                    .gap(px(TILE_GAP))
                    .children(
                        tiles
                            .into_iter()
                            .map(|(p, on)| self.transition_tile(p, on, size, cx)),
                    ),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(heading)
            .child(grid)
            .children(
                cut.current
                    .as_ref()
                    .map(|c| self.transition_settings(&cut, c, cx)),
            )
            .into_any_element()
    }

    fn transition_tile(
        &self,
        preset: Option<&'static Preset>,
        on: bool,
        size: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = SharedString::from(format!("tr:{}", preset.map_or("none", |p| p.id.as_str())));
        let hover_id = id.to_string();
        let hovered = self.fx_hover.as_ref() == Some(&hover_id);
        // A reveal's tile animation (from the asset host): its first frame,
        // playing while the pointer is over it.
        let url = preset
            .and_then(|p| p.preview.as_deref())
            .and_then(|gif| crate::assets::url("animation-previews", gif));
        let still = url.as_deref().and_then(|k| {
            self.want_picture(k, false);
            self.stills.get(k).cloned().flatten()
        });
        let anim = url.as_deref().filter(|_| hovered).and_then(|k| {
            self.want_picture(k, true);
            self.anim(k)
        });
        let label: SharedString = match preset {
            Some(p) => p.label.clone().into(),
            None => t("None").into(),
        };

        let thumb = div()
            .relative()
            .size(px(size))
            .rounded(px(6.))
            .overflow_hidden()
            .bg(theme::button_bg())
            .border(px(1.5))
            .border_color(if on { theme::text() } else { rgba(0xffffff12) })
            .when(!on, |d| {
                d.group_hover("tr-tile", |s| s.border_color(rgba(0xffffff59)))
            })
            .flex()
            .items_center()
            .justify_center()
            .map(|d| match (anim, still, preset) {
                (Some(a), _, _) => d.child(
                    img(a)
                        .object_fit(ObjectFit::Cover)
                        .id(SharedString::from(format!("{id}:anim")))
                        .absolute()
                        .inset_0()
                        .size_full(),
                ),
                (None, Some(s), _) => d.child(
                    img(s)
                        .object_fit(ObjectFit::Cover)
                        .absolute()
                        .inset_0()
                        .size_full(),
                ),
                (None, None, None) => d.child(Icon::Ban.el(30., rgba(0xffffff99))),
                (None, None, Some(_)) => d.child(Icon::Transition.el(26., rgba(0xffffff80))),
            })
            .when(on, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(4.))
                        .right(px(4.))
                        .size(px(15.))
                        .rounded(px(4.))
                        .bg(theme::text())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::Checkmark.el(11., theme::app_bg())),
                )
            });

        let pick = preset.map(|p| p.id.as_str());
        div()
            .id(id)
            .w(px(size))
            .flex()
            .flex_col()
            .gap(px(5.))
            .cursor_pointer()
            .group("tr-tile")
            .on_hover(cx.listener(move |ed, over: &bool, _, cx| {
                if *over {
                    ed.fx_hover = Some(hover_id.clone());
                } else if ed.fx_hover.as_ref() == Some(&hover_id) {
                    ed.fx_hover = None;
                }
                cx.notify();
            }))
            .on_click(cx.listener(move |ed, _, _, cx| {
                // The picked tile again takes it off, like None.
                ed.pick_transition(if on { None } else { pick }, cx)
            }))
            .child(thumb)
            .child(
                div()
                    .truncate()
                    .text_size(px(theme::XS))
                    .text_color(if on {
                        theme::text()
                    } else {
                        theme::text_secondary()
                    })
                    .child(label),
            )
            .into_any_element()
    }

    /// Under the grid once a transition is on: its length (one undo step
    /// per drag) and Apply to all.
    fn transition_settings(
        &self,
        cut: &Cut,
        current: &transitions::Transition,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (min, max) = (transitions::MIN_MICROS, cut.max);
        let stored = current.micros.min(max);
        let (shown, dragging) = match self.live_value {
            Some((k, v)) if k == DURATION_KEY => (v as Micros, true),
            _ => (stored, false),
        };
        let fraction = if max > min {
            (shown - min) as f32 / (max - min) as f32
        } else {
            1.
        };
        let (clip, kind) = (cut.clip_id.clone(), current.kind.clone());
        let apply: SliderFn = Rc::new(move |ed, f, done, cx| {
            // Tenths of a second, like the readout.
            let raw = min as f32 + f.clamp(0., 1.) * (max - min) as f32;
            let v = ((raw / 100_000.).round() as Micros * 100_000).clamp(min, max);
            if done {
                ed.live_value = None;
                ed.set_transition(&clip, &kind, v, cx);
            } else {
                ed.live_value = Some((DURATION_KEY, v as f64));
                cx.notify();
            }
        });
        let all_kind = current.kind.clone();
        let all_micros = current.micros;
        div()
            .flex_none()
            .px(px(14.))
            .py(px(8.))
            .border_t_1()
            .border_color(theme::card_border())
            .flex()
            .flex_col()
            .gap_1()
            .child(
                panel_row(t("DURATION"))
                    .child(
                        div()
                            .id(DURATION_KEY)
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .cursor_pointer()
                            .child(self.measure(DURATION_KEY))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    ed.start_slider(DURATION_KEY, e.position.x, apply.clone(), cx);
                                }),
                            )
                            .child(crate::ui::widgets::slider_track(fraction, dragging)),
                    )
                    .child(value_box(format!("{:.1}s", shown as f64 / SECOND as f64))),
            )
            .when(cut.count > 1, |d| {
                d.child(div().flex().justify_end().child(
                    pill_button("tr-apply-all", "Apply to all", false).on_click(cx.listener(
                        move |ed, _, _, cx| {
                            let done = ed.edit(
                                "Apply to all",
                                true,
                                None,
                                |p| transitions::set_all(p, &all_kind, all_micros),
                                cx,
                            );
                            if done {
                                ed.toast(t("Transition applied to all clips"), cx);
                            }
                        },
                    )),
                ))
            })
            .into_any_element()
    }

    /// A tile's pick on the cut nearest the playhead: a new one takes its
    /// own length (a switch keeps the cut's), None takes it off.
    pub fn pick_transition(&mut self, id: Option<&str>, cx: &mut Context<Self>) {
        let Some(cut) = self.transition_cut() else {
            return;
        };
        match id {
            None => {
                self.edit(
                    "Set Transition",
                    true,
                    None,
                    |p| transitions::set(p, &cut.clip_id, None),
                    cx,
                );
            }
            Some(id) => {
                let wanted = cut
                    .current
                    .as_ref()
                    .map_or(transitions::get(id).default_micros, |c| c.micros);
                self.set_transition(&cut.clip_id, id, wanted.min(cut.max), cx);
            }
        }
    }

    /// Puts `kind` on the cut after `clip_id` for `micros`, then plays the
    /// cut from just before its window so it's seen straight away.
    fn set_transition(
        &mut self,
        clip_id: &str,
        kind: &str,
        micros: Micros,
        cx: &mut Context<Self>,
    ) {
        let done = self.edit(
            "Set Transition",
            true,
            None,
            |p| transitions::set(p, clip_id, Some((kind, micros))),
            cx,
        );
        if !done {
            return;
        }
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let window = clip.end() - clip.out_transition();
        let from = (window - REPLAY_LEAD).max(clip.timeline_start);
        self.pause();
        self.seek(from, cx);
        self.toggle_play(cx);
    }
}
