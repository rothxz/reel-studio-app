//! The inspector's Audio › Beats for a music clip: Auto generate (one
//! analysis per song, run off the UI thread and kept for the session) with
//! its Detection level (Strong = bars, Normal = every beat, Dense = with
//! offbeats); at the playhead ‹ Add / Delete beat › and Move nearest beat
//! here; Shift all beats a frame earlier / later to line the grid up by
//! ear; Reset and Clear. Every change is one undo step; the timeline draws
//! the beats as dots and other clips' edges snap to them.
//!
//! Keys: M adds / deletes the beat at the playhead, ⌥← / ⌥→ step between
//! beats.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{AnyElement, Context, FontWeight, SharedString, div, prelude::*, px};

use crate::beats::{self, Analysis, BeatMap, Mode};
use crate::editor::Editor;
use crate::i18n::tk;
use crate::model::{Clip, Micros, SECOND, TrackType, new_id};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets::{
    choice_pill, icon_button, panel_note, panel_row, section_with, switch, switch_row,
};

/// Beats analysis in this session.
#[derive(Default)]
pub struct BeatJobs {
    /// Finished analyses by song file: a second clip of the song (a split,
    /// a copy) gets its beats at once.
    done: HashMap<String, Arc<Analysis>>,
    /// Analyses running by song file, with the clips waiting on each and
    /// the detection mode each asked for.
    running: HashMap<String, Vec<(String, Mode)>>,
    /// The detection mode picked on a clip with no beats yet.
    mode: Mode,
}

impl BeatJobs {
    fn analyzing(&self, clip: &Clip) -> bool {
        clip.source
            .as_ref()
            .and_then(|s| self.running.get(s))
            .is_some_and(|w| w.iter().any(|(id, _)| *id == clip.id))
    }
}

/// The playhead is on a beat within this many pixels of its dot.
const ON_BEAT_PX: f32 = 8.;

/// A full-width button: `primary` white, else grey; greyed when disabled.
fn wide_button(
    id: &'static str,
    icon: Option<Icon>,
    label: &str,
    primary: bool,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    let fg: gpui::Hsla = if !enabled {
        theme::text_disabled().into()
    } else if primary {
        theme::app_bg().into()
    } else {
        theme::text().into()
    };
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .h(px(30.))
        .px(px(10.))
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .text_size(px(theme::XS))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .map(|d| {
            if primary && enabled {
                d.bg(theme::text()).hover(|s| s.opacity(0.88))
            } else {
                d.bg(theme::button_bg())
                    .when(enabled, |d| d.hover(|s| s.bg(theme::button_hover())))
            }
        })
        .when(enabled, |d| d.cursor_pointer())
        .children(icon.map(|i| i.el(13., fg)))
        .child(div().truncate().child(crate::i18n::ts(label.to_string())))
}

impl Editor {
    /// Within how far of the playhead a beat counts as under it.
    fn on_beat_tolerance(&self) -> Micros {
        30_000.max((ON_BEAT_PX / self.pps.max(1.) * SECOND as f32) as Micros)
    }

    /// The beat clip the keys act on: the selected music clip, else the
    /// one under the playhead.
    fn beat_clip(&self) -> Option<&Clip> {
        if let Some((_, c)) = self.selected_clip() {
            return (c.kind == "audio").then_some(c);
        }
        self.project
            .tracks
            .iter()
            .filter(|t| t.kind == TrackType::Audio)
            .flat_map(|t| t.clips.iter())
            .find(|c| {
                c.kind == "audio" && c.timeline_start <= self.playhead && self.playhead <= c.end()
            })
    }

    /// Replaces the clip's beat map (None removes it): one undo step.
    fn set_beats(&mut self, clip_id: &str, map: Option<BeatMap>, cx: &mut Context<Self>) {
        self.edit_clip(clip_id, "Edit Beats", move |c| beats::store(c, map), cx);
    }

    /// Auto generate on / off: on re-uses the clip's analysis when it's
    /// from this analyzer, else analyzes the song.
    fn toggle_auto_beats(&mut self, clip_id: &str, cx: &mut Context<Self>) {
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        if self.beat_jobs.analyzing(clip) {
            return;
        }
        let map = beats::of(clip);
        match map {
            Some(m) if m.auto_generate => self.set_beats(clip_id, Some(m.with_auto(false)), cx),
            Some(m)
                if m.detected.is_some()
                    && m.analysis_version == crate::engine::BeatAnalyzer::version() =>
            {
                self.set_beats(clip_id, Some(m.with_auto(true)), cx)
            }
            m => {
                let mode = m.map_or(self.beat_jobs.mode, |m| m.mode);
                self.analyze_beats(clip_id, mode, cx);
            }
        }
    }

    /// Finds the song's beats off the UI thread, then gives them to every
    /// clip that asked (each one undo step).
    fn analyze_beats(&mut self, clip_id: &str, mode: Mode, cx: &mut Context<Self>) {
        let Some(source) = self
            .project
            .find_clip(clip_id)
            .and_then(|(_, c)| c.source.clone())
        else {
            return;
        };
        if let Some(a) = self.beat_jobs.done.get(&source).cloned() {
            self.apply_analysis(clip_id, mode, &a, cx);
            return;
        }
        let waiting = self.beat_jobs.running.entry(source.clone()).or_default();
        let first = waiting.is_empty();
        waiting.retain(|(id, _)| id != clip_id);
        waiting.push((clip_id.to_string(), mode));
        cx.notify();
        if !first {
            return;
        }
        let path = source.clone();
        let task = cx
            .background_executor()
            .spawn(async move { beats::analyze(&path) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |ed, cx| {
                let waiting = ed.beat_jobs.running.remove(&source).unwrap_or_default();
                match result {
                    Ok(a) => {
                        let a = Arc::new(a);
                        ed.beat_jobs.done.insert(source, a.clone());
                        for (id, mode) in waiting {
                            ed.apply_analysis(&id, mode, &a, cx);
                        }
                    }
                    Err(e) => {
                        eprintln!("beat analysis: {e}");
                        ed.toast(crate::i18n::t("Beat analysis failed"), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn apply_analysis(&mut self, clip_id: &str, mode: Mode, a: &Analysis, cx: &mut Context<Self>) {
        // The clip may be gone (deleted, undone) while it ran.
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let map = BeatMap::from_analysis(a, mode, beats::of(clip).as_ref());
        self.set_beats(clip_id, Some(map), cx);
        if a.beats.is_empty() {
            self.toast(crate::i18n::t("No reliable beats detected"), cx);
        }
    }

    fn pick_beat_mode(&mut self, clip_id: &str, mode: Mode, cx: &mut Context<Self>) {
        self.beat_jobs.mode = mode;
        let map = self
            .project
            .find_clip(clip_id)
            .and_then(|(_, c)| beats::of(c));
        match map {
            Some(m) if m.mode != mode => self.set_beats(clip_id, Some(m.with_mode(mode)), cx),
            _ => cx.notify(),
        }
    }

    /// Deletes the beat under the playhead, else adds one there.
    fn toggle_beat(&mut self, clip_id: &str, cx: &mut Context<Self>) {
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let t = self.playhead;
        if t < clip.timeline_start || t > clip.end() {
            self.toast(
                crate::i18n::t("Move the playhead over this clip to add a beat"),
                cx,
            );
            return;
        }
        let map = beats::of(clip);
        let under = map
            .as_ref()
            .and_then(|m| beats::nearest(clip, m, t, self.on_beat_tolerance()))
            .map(|(_, m)| m.id.clone());
        let next = match (map, under) {
            (Some(m), Some(id)) => m.remove(&id),
            (map, _) => {
                let song = clip.source_start.unwrap_or(0) + clip.source_duration.unwrap_or(0);
                map.unwrap_or_else(|| {
                    BeatMap::empty(
                        song,
                        self.beat_jobs.mode,
                        crate::engine::BeatAnalyzer::version(),
                    )
                })
                .add(beats::to_source(clip, t), new_id("beat"))
            }
        };
        self.set_beats(clip_id, Some(next), cx);
    }

    /// How far "Move nearest beat here" reaches: half a beat, so it never
    /// takes the next beat over.
    fn move_reach(map: &BeatMap) -> Micros {
        map.bpm
            .filter(|&b| b > 0.)
            .map_or(300_000, |b| (30. / b * SECOND as f64) as Micros)
    }

    fn move_nearest_beat_here(&mut self, clip_id: &str, cx: &mut Context<Self>) {
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let Some(map) = beats::of(clip) else { return };
        let t = self.playhead;
        let Some((_, m)) = beats::nearest(clip, &map, t, Self::move_reach(&map)) else {
            return;
        };
        let next = map.move_to(&m.id.clone(), beats::to_source(clip, t));
        self.set_beats(clip_id, Some(next), cx);
    }

    /// Every beat a frame earlier (-1) or later (1).
    fn shift_beats(&mut self, clip_id: &str, frames: i64, cx: &mut Context<Self>) {
        let Some(map) = self
            .project
            .find_clip(clip_id)
            .and_then(|(_, c)| beats::of(c))
        else {
            return;
        };
        let next = map.shift(frames * self.frame_micros());
        self.set_beats(clip_id, Some(next), cx);
    }

    /// The beats ⌥← / ⌥→ step between: the selected music clip's, else
    /// every music clip's.
    fn beat_stops(&self) -> Vec<Micros> {
        match self.selected_clip() {
            Some((_, c)) if !c.beats().is_empty() => {
                c.beats().into_iter().map(|(t, _)| t).collect()
            }
            _ => self.project.beat_times(""),
        }
    }

    /// The playhead onto the previous (-1) or next (1) beat.
    pub fn step_beat(&mut self, dir: i32, cx: &mut Context<Self>) {
        self.pause();
        // A millisecond's slack, so a playhead on a beat moves on.
        const EPS: Micros = 1_000;
        let now = self.playhead;
        let stops = self.beat_stops();
        let target = if dir > 0 {
            stops.into_iter().find(|&t| t > now + EPS)
        } else {
            stops.into_iter().rev().find(|&t| t < now - EPS)
        };
        if let Some(t) = target {
            self.seek(t, cx);
        }
    }

    /// The M key: add / delete a beat at the playhead.
    pub fn toggle_beat_at_playhead(&mut self, cx: &mut Context<Self>) {
        match self.beat_clip().map(|c| c.id.clone()) {
            Some(id) => self.toggle_beat(&id, cx),
            None => self.toast(crate::i18n::t("Select a music clip to mark its beats"), cx),
        }
    }

    /// Whether the selected clip's beat under the playhead is lit on the
    /// timeline (what Delete beat would remove).
    pub fn beat_under_playhead(&self, clip: &Clip) -> Option<Micros> {
        if self.selected.primary() != Some(clip.id.as_str()) {
            return None;
        }
        let map = beats::of(clip)?;
        beats::nearest(clip, &map, self.playhead, self.on_beat_tolerance()).map(|(t, _)| t)
    }

    /// Audio › Beats.
    pub fn render_beats_panel(&self, clip: &Clip, cx: &mut Context<Self>) -> AnyElement {
        let map = beats::of(clip);
        let analyzing = self.beat_jobs.analyzing(clip);
        let auto_on = analyzing || map.as_ref().is_some_and(|m| m.auto_generate);
        let mode = map.as_ref().map_or(self.beat_jobs.mode, |m| m.mode);
        let t = self.playhead;
        let inside = t >= clip.timeline_start && t <= clip.end();
        let on_timeline = map
            .as_ref()
            .map(|m| beats::on_timeline(clip, m))
            .unwrap_or_default();
        let count = on_timeline.len();
        let under = map
            .as_ref()
            .filter(|_| inside)
            .and_then(|m| beats::nearest(clip, m, t, self.on_beat_tolerance()));
        let nearby = map
            .as_ref()
            .filter(|_| inside && under.is_none())
            .and_then(|m| beats::nearest(clip, m, t, Self::move_reach(m)));
        let has_prev = on_timeline.iter().any(|(bt, _)| *bt < t - 1_000);
        let has_next = on_timeline.iter().any(|(bt, _)| *bt > t + 1_000);
        let edited = map.as_ref().is_some_and(|m| m.has_user_edits());
        let id = clip.id.clone();

        let status: SharedString = if analyzing {
            crate::i18n::t("Analyzing…").into()
        } else if map
            .as_ref()
            .is_some_and(|m| m.auto_generate && m.detected.as_ref().is_some_and(|d| d.is_empty()))
        {
            crate::i18n::t("No reliable beats detected").into()
        } else if count > 0 {
            match map.as_ref().and_then(|m| m.bpm.filter(|_| m.auto_generate)) {
                Some(bpm) => tk(
                    "beats.summary",
                    "{bpm} BPM · {count} beats",
                    &[("bpm", &(bpm.round() as i64)), ("count", &count)],
                ),
                None => tk("beats.manualSummary", "{count} beats", &[("count", &count)]),
            }
            .into()
        } else {
            crate::i18n::t("Mark the music's beats automatically").into()
        };

        let (a, b, c, d, e, f, g, h) = (
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
        );
        let head = section_with(
            "Beats",
            true,
            [
                self.reset_button(
                    "beats-reset",
                    edited,
                    move |ed, cx| {
                        if let Some(m) = ed.project.find_clip(&a).and_then(|(_, c)| beats::of(c)) {
                            ed.set_beats(&a, Some(m.reset()), cx)
                        }
                    },
                    cx,
                ),
                icon_button("beats-clear", Icon::Trash, "Clear beat markers")
                    .size(14.)
                    .disabled(map.is_none() || analyzing)
                    .on_click(cx.listener(move |ed, _, _, cx| ed.set_beats(&b, None, cx)))
                    .into_any_element(),
            ],
        );

        // A count reads as a mono read-out; a hint as a sentence.
        let readout = count > 0 && !analyzing;
        let status_row = div()
            .mb_1()
            .flex()
            .items_center()
            .gap(px(6.))
            .when(readout, |d| d.font_family(theme::font_mono()))
            .text_size(px(theme::XS))
            .text_color(if analyzing || count > 0 {
                theme::text()
            } else {
                theme::text_tertiary()
            })
            .child(Icon::MusicalNotes.el(12., theme::text_secondary()))
            .child(div().truncate().child(status));

        let auto_row = switch_row(
            "Auto generate",
            switch("beats-auto", auto_on, analyzing)
                .on_click(cx.listener(move |ed, _, _, cx| ed.toggle_auto_beats(&c, cx))),
        );
        let modes = panel_row("Detection").child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .gap_1()
                .when(!auto_on, |d| d.opacity(0.5))
                .children(Mode::ALL.into_iter().map(|m| {
                    let id = d.clone();
                    choice_pill(("beats-mode", m as usize), m.label(), m == mode, !analyzing)
                        .flex_1()
                        .min_w_0()
                        .when(!analyzing, |p| {
                            p.on_click(
                                cx.listener(move |ed, _, _, cx| ed.pick_beat_mode(&id, m, cx)),
                            )
                        })
                })),
        );

        let playhead_head = section_with("At the playhead", false, []);
        let step_row = div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                icon_button("beat-prev", Icon::ChevronBack, "Previous beat")
                    .size(13.)
                    .shortcut(crate::shortcuts::Shortcut::PrevBeat)
                    .disabled(!has_prev)
                    .on_click(cx.listener(|ed, _, _, cx| ed.step_beat(-1, cx))),
            )
            .child(
                wide_button(
                    "beat-toggle",
                    Some(if under.is_some() {
                        Icon::Remove
                    } else {
                        Icon::Add
                    }),
                    if under.is_some() {
                        "Delete beat"
                    } else {
                        "Add beat"
                    },
                    under.is_none(),
                    inside && !analyzing,
                )
                .tooltip(crate::ui::widgets::tooltip(
                    crate::i18n::ts("Add / delete beat"),
                    Some(crate::shortcuts::hint(crate::shortcuts::Shortcut::AddBeat)),
                ))
                .when(inside && !analyzing, |el| {
                    el.on_click(cx.listener(move |ed, _, _, cx| ed.toggle_beat(&e, cx)))
                }),
            )
            .child(
                icon_button("beat-next", Icon::ChevronForward, "Next beat")
                    .size(13.)
                    .shortcut(crate::shortcuts::Shortcut::NextBeat)
                    .disabled(!has_next)
                    .on_click(cx.listener(|ed, _, _, cx| ed.step_beat(1, cx))),
            );
        let can_move = nearby.is_some() && !analyzing;
        let move_row = div().mt(px(6.)).flex().child(
            wide_button("beat-move", None, "Move nearest beat here", false, can_move).when(
                can_move,
                |el| {
                    el.on_click(cx.listener(move |ed, _, _, cx| ed.move_nearest_beat_here(&f, cx)))
                },
            ),
        );
        let hint = (!inside).then(|| panel_note("Move the playhead over this clip to add a beat"));

        let can_shift = count > 0 && !analyzing;
        let shift_row = panel_row("Shift all").child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_end()
                .gap_1()
                .child(
                    icon_button("beats-earlier", Icon::CaretBack, "A frame earlier")
                        .size(12.)
                        .disabled(!can_shift)
                        .on_click(cx.listener(move |ed, _, _, cx| ed.shift_beats(&g, -1, cx))),
                )
                .child(
                    div()
                        .font_family(theme::font_mono())
                        .text_size(px(theme::XS))
                        .text_color(theme::text_tertiary())
                        .child(crate::i18n::t("1 frame")),
                )
                .child(
                    icon_button("beats-later", Icon::CaretForward, "A frame later")
                        .size(12.)
                        .disabled(!can_shift)
                        .on_click(cx.listener(move |ed, _, _, cx| ed.shift_beats(&h, 1, cx))),
                ),
        );
        div()
            .px(px(14.))
            .pb(px(14.))
            .flex()
            .flex_col()
            .child(head)
            .child(status_row)
            .child(auto_row)
            .child(modes)
            .child(playhead_head)
            .child(step_row)
            .child(move_row)
            .children(hint)
            .child(section_with("Align", false, []))
            .child(shift_row)
            .child(panel_note(
                "Shift the beats when the dots run a little ahead of or behind the music. Other clips snap to the beats while auto snapping is on.",
            ))
            .into_any_element()
    }
}
