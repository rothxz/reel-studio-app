//! The timeline: a ruler with the playhead's timecode at the top left,
//! lanes stacked overlays-above / main / audio-below, each with a header
//! (name, lock, eye, speaker), clips coloured by kind. Click or drag on the
//! ruler or empty lane space scrubs; a click on a clip selects it (or, with
//! the Split tool, cuts it there); the wheel scrolls, ⌘/ctrl + wheel zooms.

use gpui::{
    Context, CursorStyle, FontWeight, MouseButton, MouseDownEvent, Rgba, ScrollWheelEvent,
    SharedString, div, prelude::*, px,
};

use crate::editor::{Drag, Editor, Tool};
use crate::model::{Micros, SECOND, Track, TrackType};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets::{icon_button, mono};

const MAIN_LANE_HEIGHT: f32 = 56.;
const LANE_HEIGHT: f32 = 32.;
const LANE_GAP: f32 = 6.;
const RULER_HEIGHT: f32 = 20.;
const LANES_TOP: f32 = 8.;
pub const LANE_HEADER_WIDTH: f32 = 160.;

/// Which flag a lane header toggles.
#[derive(Clone, Copy)]
pub enum LaneFlag {
    Locked,
    Hidden,
    Muted,
}

fn clip_color(kind: &str) -> Rgba {
    match kind {
        "video" | "image" => theme::clip_video(),
        "audio" => theme::clip_audio(),
        "text" => theme::clip_text(),
        "sticker" | "drawing" => theme::clip_sticker(),
        "effect" => theme::clip_effect(),
        _ => theme::clip_sticker(),
    }
}

/// "MM:SS" on the ruler.
fn ruler_label(t: Micros) -> String {
    let s = t.max(0) / SECOND;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Lanes top to bottom: lanes above the main one (highest order first),
/// the main lane, then sound lanes.
pub fn lane_order(tracks: &[Track]) -> Vec<&Track> {
    let main_id = tracks
        .iter()
        .find(|t| t.kind == TrackType::Video)
        .map(|t| t.id.as_str());
    let mut above: Vec<&Track> = tracks
        .iter()
        .filter(|t| Some(t.id.as_str()) != main_id && t.kind != TrackType::Audio)
        .collect();
    above.sort_by_key(|t| -t.order);
    let mut below: Vec<&Track> = tracks
        .iter()
        .filter(|t| t.kind == TrackType::Audio)
        .collect();
    below.sort_by_key(|t| t.order);
    above
        .into_iter()
        .chain(tracks.iter().filter(|t| Some(t.id.as_str()) == main_id))
        .chain(below)
        .collect()
}

impl Editor {
    pub fn render_timeline(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let fps = self.fps();
        let main_id = self.project.main_track().map(|t| t.id.clone());
        let lanes_width = self
            .measured("lanes")
            .map_or(2000., |b| f32::from(b.size.width));

        // Ruler ticks: a step that keeps labels ~80 px apart.
        let steps = [0.5, 1., 2., 5., 10., 15., 30., 60., 120., 300., 600.];
        let step = steps
            .iter()
            .copied()
            .find(|s| s * self.pps as f64 >= 80.)
            .unwrap_or(1200.);
        let first = (self.scroll_x as f64 / self.pps as f64 / step).floor() * step;
        let last = first + (lanes_width as f64 / self.pps as f64) + step;
        let mut ticks = vec![];
        let mut s = first;
        while s <= last {
            let t = (s * SECOND as f64) as Micros;
            ticks.push(
                div()
                    .absolute()
                    .left(px(self.x_of(t)))
                    .top_0()
                    .h_full()
                    .border_l_1()
                    .border_color(theme::chip_border())
                    .pl_1()
                    .child(mono(ruler_label(t), theme::XXS, theme::text_tertiary())),
            );
            s += step;
        }

        let split_tool = self.tool == Tool::Split;
        let order = lane_order(&self.project.tracks);
        let mut headers = vec![];
        let mut lanes = vec![];
        for (li, track) in order.iter().enumerate() {
            let main = Some(&track.id) == main_id.as_ref();
            let height = if main { MAIN_LANE_HEIGHT } else { LANE_HEIGHT };
            headers.push(self.lane_header(track, main, height, cx).into_any_element());

            let clips = track.clips.iter().map(|clip| {
                let selected = self.selected.as_deref() == Some(clip.id.as_str());
                let color = clip_color(&clip.kind);
                let x = self.x_of(clip.timeline_start);
                let w = clip.timeline_duration as f32 / SECOND as f32 * self.pps;
                let id = clip.id.clone();
                let locked = track.locked;
                div()
                    .id(SharedString::from(format!("clip-{}", clip.id)))
                    .absolute()
                    .left(px(x))
                    .top_0()
                    .w(px((w - 1.).max(2.)))
                    .h(px(height))
                    .rounded(px(5.))
                    .bg(color)
                    .border_2()
                    .border_color(if selected { theme::selected() } else { color })
                    .overflow_hidden()
                    .px_1()
                    .when(clip.hidden || track.hidden, |d| d.opacity(0.4))
                    .when(split_tool, |d| d.cursor(CursorStyle::Crosshair))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let t = ed.time_at(e.position.x);
                            if ed.tool == Tool::Split && !locked {
                                ed.split_clip_at(&id, t, cx);
                                return;
                            }
                            ed.selected = Some(id.clone());
                            ed.drag = Some(Drag::Scrub);
                            ed.scrub_to(e.position.x, cx);
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .h(px(if main { 18. } else { height - 4. }))
                            .text_size(px(theme::XS))
                            .text_color(theme::text())
                            .when(clip.speed != 1., |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .px_1()
                                        .rounded(px(3.))
                                        .bg(gpui::rgba(0x00000080))
                                        .font_family(theme::FONT_MONO)
                                        .text_size(px(theme::XXS))
                                        .child(format!("{}×", clip.speed)),
                                )
                            })
                            .child(div().truncate().child(clip.label())),
                    )
            });
            lanes.push(
                div()
                    .id(("lane", li))
                    .relative()
                    .flex_none()
                    .h(px(height))
                    .when(main, |d| d.bg(theme::lane_bg()))
                    .children(clips)
                    .into_any_element(),
            );
        }

        let playhead_x = self.x_of(self.playhead);
        let readout = format!(
            "{} / {}",
            crate::editor::timecode(self.playhead, fps),
            crate::editor::timecode(self.project.duration, fps)
        );

        div()
            .id("timeline")
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .bg(theme::timeline_bg())
            .overflow_hidden()
            .child(
                // Lane headers, pinned left.
                div()
                    .flex_none()
                    .w(px(LANE_HEADER_WIDTH))
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(theme::card_border())
                    .bg(theme::panel())
                    .child(
                        div()
                            .flex_none()
                            .h(px(RULER_HEIGHT + LANES_TOP))
                            .px_2()
                            .flex()
                            .items_center()
                            .child(mono(readout, theme::XXS, theme::text_secondary())),
                    )
                    .child(div().flex().flex_col().gap(px(LANE_GAP)).children(headers)),
            )
            .child(
                div()
                    .id("lanes")
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .cursor(if split_tool {
                        CursorStyle::Crosshair
                    } else {
                        CursorStyle::Arrow
                    })
                    .child(self.measure("lanes"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|ed, e: &MouseDownEvent, _, cx| {
                            ed.selected = None;
                            ed.drag = Some(Drag::Scrub);
                            ed.scrub_to(e.position.x, cx);
                        }),
                    )
                    .on_scroll_wheel(cx.listener(|ed, e: &ScrollWheelEvent, _, cx| {
                        let d = e.delta.pixel_delta(px(20.));
                        if e.modifiers.platform || e.modifiers.control {
                            let f = if f32::from(d.y) > 0. { 1.1 } else { 1. / 1.1 };
                            ed.set_zoom(ed.pps * f, cx);
                        } else {
                            let dx = if f32::from(d.x).abs() > f32::from(d.y).abs() {
                                d.x
                            } else {
                                d.y
                            };
                            ed.scroll_x = (ed.scroll_x - f32::from(dx)).max(0.);
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .relative()
                            .h(px(RULER_HEIGHT))
                            .border_b_1()
                            .border_color(theme::card_border())
                            .children(ticks),
                    )
                    .child(
                        div()
                            .pt(px(LANES_TOP))
                            .flex()
                            .flex_col()
                            .gap(px(LANE_GAP))
                            .children(lanes),
                    )
                    // The playhead: a line with a small head on the ruler.
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(px(playhead_x - 1.))
                            .w(px(2.))
                            .bg(theme::playhead()),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left(px(playhead_x - 5.))
                            .w(px(10.))
                            .h(px(10.))
                            .rounded_b(px(3.))
                            .bg(theme::playhead()),
                    ),
            )
    }

    fn lane_header(
        &self,
        track: &Track,
        main: bool,
        height: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = track.id.clone();
        let toggle =
            |flag: LaneFlag, on: bool, icon: crate::ui::icons::Icon, label: &'static str| {
                let id = id.clone();
                icon_button(SharedString::from(format!("{label}-{id}")), icon, label)
                    .size(14.)
                    .active(on)
                    .on_click(cx.listener(move |ed, _, _, cx| ed.toggle_lane_flag(&id, flag, cx)))
            };
        div()
            .flex_none()
            .h(px(height))
            .px_2()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(theme::XS))
                    .text_color(theme::text_tertiary())
                    .when(main, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(track.name.clone()),
            )
            .child(toggle(
                LaneFlag::Locked,
                track.locked,
                if track.locked {
                    Icon::LockClosed
                } else {
                    Icon::LockOpen
                },
                if track.locked {
                    "Unlock lane"
                } else {
                    "Lock lane"
                },
            ))
            .when(track.kind != TrackType::Audio, |d| {
                d.child(toggle(
                    LaneFlag::Hidden,
                    !track.hidden,
                    if track.hidden {
                        Icon::EyeOff
                    } else {
                        Icon::Eye
                    },
                    if track.hidden {
                        "Show lane"
                    } else {
                        "Hide lane"
                    },
                ))
            })
            .when(
                matches!(
                    track.kind,
                    TrackType::Video | TrackType::Audio | TrackType::Overlay
                ),
                |d| {
                    d.child(toggle(
                        LaneFlag::Muted,
                        !track.muted,
                        if track.muted {
                            Icon::VolumeMute
                        } else {
                            Icon::VolumeHigh
                        },
                        if track.muted {
                            "Unmute lane"
                        } else {
                            "Mute lane"
                        },
                    ))
                },
            )
    }

    pub fn toggle_lane_flag(&mut self, track_id: &str, flag: LaneFlag, cx: &mut Context<Self>) {
        let id = track_id.to_string();
        let label = match flag {
            LaneFlag::Locked => "Lock Lane",
            LaneFlag::Hidden => "Hide Lane",
            LaneFlag::Muted => "Mute Lane",
        };
        self.edit(
            label,
            false,
            None,
            |p| {
                let Some(t) = p.tracks.iter_mut().find(|t| t.id == id) else {
                    return false;
                };
                let f = match flag {
                    LaneFlag::Locked => &mut t.locked,
                    LaneFlag::Hidden => &mut t.hidden,
                    LaneFlag::Muted => &mut t.muted,
                };
                *f = !*f;
                true
            },
            cx,
        );
    }
}
