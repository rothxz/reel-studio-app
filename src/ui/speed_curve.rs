//! Speed › Curve (a video): None, Custom and the preset ramps as tiles;
//! with a ramp on, its card under them: the name and the clip's length
//! before → after, the graph (0.1× at the bottom, 1× across the middle,
//! 10× at the top; x = how far through the clip's source) with the
//! playhead's line, the beats to drag (the ends slide up and down), and
//! Speed at the playhead, Reset and Add / Delete beat. Every pick and
//! every beat released is one undo step that re-times the clip (the main
//! lane closes up after it). A photo has no Speed: it says so.

use gpui::{
    AnyElement, Bounds, Context, Hsla, MouseButton, MouseDownEvent, PathBuilder, Pixels, Point,
    Window, canvas, div, fill, point, prelude::*, px,
};

use crate::editor::{Drag, Editor};
use crate::model::Clip;
use crate::speed_curve::{self as curve, CUSTOM, GraphPoint, PRESETS, SpeedCurve, speed_from_axis};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::inspector::secs;
use crate::ui::widgets::{icon_button, panel_note, pick_tile, pill_button, section};

const GRAPH: &str = "speed-curve-graph";
const GRAPH_HEIGHT: f32 = 150.;
/// Room left of the graph for the axis labels.
const LABELS_WIDTH: f32 = 34.;
/// A press this close to a beat picks it up.
const HIT_RADIUS: f32 = 12.;
const TILE: f32 = 56.;
/// Guide lines: 0.1×, halfway, 1×, halfway, 10×.
const GUIDES: [f64; 5] = [-1., -0.5, 0., 0.5, 1.];

/// A beat held on the graph.
pub struct BeatDrag {
    clip_id: String,
    preset: String,
    index: usize,
    /// The curve as dragged so far.
    pub points: Vec<GraphPoint>,
    moved: bool,
}

fn accent() -> Hsla {
    theme::text().into()
}

impl Editor {
    /// The image clip's Speed tab: it has no speed to set.
    pub fn render_image_speed_panel(&self) -> AnyElement {
        div()
            .px(px(14.))
            .pb(px(14.))
            .flex()
            .flex_col()
            .child(section("Speed", true))
            .child(panel_note("Select a video clip first"))
            .into_any_element()
    }

    pub fn render_speed_curve_panel(&self, clip: &Clip, cx: &mut Context<Self>) -> AnyElement {
        if clip.kind != "video" {
            return self.render_image_speed_panel();
        }
        let applied = clip.speed_curve();
        let active = applied.as_ref().map_or("none", |c| c.preset.as_str());
        let tile = |key: &'static str, label: &'static str, content: AnyElement| {
            let id = clip.id.clone();
            pick_tile(
                gpui::ElementId::Name(format!("speed-curve:{key}").into()),
                TILE,
                active == key,
                content,
                Some(label.into()),
            )
            .on_click(cx.listener(move |ed, _, _, cx| ed.pick_speed_curve(&id, key, cx)))
            .into_any_element()
        };
        let mut tiles = vec![
            tile(
                "none",
                "None",
                Icon::Close.el(20., theme::text()).into_any_element(),
            ),
            tile(
                CUSTOM,
                "Custom",
                Icon::Options.el(20., theme::text()).into_any_element(),
            ),
        ];
        for p in PRESETS {
            tiles.push(tile(p.id, p.label, thumb(p.points())));
        }

        div()
            .px(px(14.))
            .pb(px(14.))
            .flex()
            .flex_col()
            .child(section("Curve", true))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .pb(px(12.))
                    .children(tiles),
            )
            .child(match applied {
                Some(c) => self.render_curve_card(clip, c, cx),
                None => {
                    panel_note("Pick a curve to ramp the speed, then drag its beats in the graph")
                        .into_any_element()
                }
            })
            .into_any_element()
    }

    /// A tile picked: None removes the ramp; the one already on stays.
    fn pick_speed_curve(&mut self, clip_id: &str, key: &str, cx: &mut Context<Self>) {
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let current = clip.speed_curve().map(|c| c.preset);
        if current.as_deref().unwrap_or("none") == key {
            return;
        }
        let next = if key == "none" {
            None
        } else {
            let Some(c) = curve::preset_curve(key) else {
                return;
            };
            Some(c)
        };
        self.set_speed_curve(clip_id, next, cx);
    }

    /// The source fraction under the playhead (None off the clip).
    fn playhead_fraction(&self, clip: &Clip, points: &[GraphPoint]) -> Option<f64> {
        let local = self.playhead - clip.timeline_start;
        if local < 0 || local > clip.timeline_duration || clip.timeline_duration <= 0 {
            return None;
        }
        let u = local as f64 / clip.timeline_duration as f64;
        Some(curve::source_fraction(&curve::timing_table(points), u))
    }

    fn render_curve_card(&self, clip: &Clip, c: SpeedCurve, cx: &mut Context<Self>) -> AnyElement {
        // The held beat's curve while dragging.
        let points = match &self.drag {
            Some(Drag::Beat(d)) if d.clip_id == clip.id => d.points.clone(),
            _ => c.points.clone(),
        };
        let speed = if clip.speed > 0. { clip.speed } else { 1. };
        let source = clip.timeline_duration as f64 * speed;
        let here = self.playhead_fraction(clip, &points);
        let speed_here = here.map(|x| speed_from_axis(curve::evaluate(&points, x)));
        let beat_here = here.and_then(|x| curve::beat_near(&points, x, 0.02));
        let can_add = here.is_some_and(|x| curve::add_beat(&points, x).is_some());

        let header = div()
            .h(px(34.))
            .px(px(12.))
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .flex_1()
                    .text_size(px(theme::XS))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::text())
                    .child(curve::label(&c.preset)),
            )
            .child(mono(secs(source as i64), theme::text_tertiary()))
            .child(mono("→".into(), theme::text_tertiary()))
            .child(mono(secs(clip.timeline_duration), theme::text()));

        let labels = div()
            .flex_none()
            .w(px(LABELS_WIDTH))
            .h(px(GRAPH_HEIGHT))
            .flex()
            .flex_col()
            .justify_between()
            .font_family(theme::FONT_MONO)
            .text_size(px(9.))
            .text_color(theme::text_tertiary())
            .child("10×")
            .child("1×")
            .child("0.1×");

        let id = clip.id.clone();
        let preset = c.preset.clone();
        let drawn = points.clone();
        let held = match &self.drag {
            Some(Drag::Beat(d)) if d.clip_id == clip.id => Some(d.index),
            _ => None,
        };
        let graph = div()
            .id(GRAPH)
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(GRAPH_HEIGHT))
            .cursor_pointer()
            .child(self.measure(GRAPH))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        paint_graph(bounds, &drawn, here, held, window);
                    },
                )
                .absolute()
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    ed.start_beat_drag(&id, &preset, e.position, cx);
                }),
            );

        let (reset_id, reset_preset) = (clip.id.clone(), c.preset.clone());
        let (beat_id, beat_curve) = (clip.id.clone(), c.clone());
        let footer = div()
            .h(px(40.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap_2()
            .child(mono(
                speed_here
                    .map_or_else(|| "Speed –".into(), |s| format!("Speed {}", speed_label(s))),
                theme::text_secondary(),
            ))
            .child(div().flex_1())
            .child(
                pill_button("speed-curve-reset", "Reset", false).on_click(cx.listener(
                    move |ed, _, _, cx| {
                        if let Some(fresh) = curve::preset_curve(&reset_preset) {
                            ed.set_speed_curve(&reset_id, Some(fresh), cx);
                        }
                    },
                )),
            )
            .child(
                icon_button(
                    "speed-curve-beat",
                    if beat_here.is_some() {
                        Icon::Remove
                    } else {
                        Icon::Add
                    },
                    if beat_here.is_some() {
                        "Delete beat"
                    } else {
                        "Add beat"
                    },
                )
                .size(15.)
                .disabled(beat_here.is_none() && !can_add)
                .on_click(cx.listener(move |ed, _, _, cx| {
                    let pts = &beat_curve.points;
                    let next = match (beat_here, here) {
                        (Some(i), _) => Some(curve::delete_beat(pts, i)),
                        (None, Some(x)) => curve::add_beat(pts, x),
                        _ => None,
                    };
                    if let Some(points) = next {
                        let c = SpeedCurve {
                            points,
                            ..beat_curve.clone()
                        };
                        ed.set_speed_curve(&beat_id, Some(c), cx);
                    }
                })),
            );

        div()
            .rounded(px(8.))
            .bg(theme::fill_subtle())
            .border_1()
            .border_color(theme::card_border())
            .flex()
            .flex_col()
            .child(header)
            .child(div().px(px(10.)).flex().gap_1().child(labels).child(graph))
            .child(footer)
            .into_any_element()
    }

    /// A press on the graph: picks up the beat (or end) under it.
    fn start_beat_drag(
        &mut self,
        clip_id: &str,
        preset: &str,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(b) = self.measured(GRAPH) else {
            return;
        };
        let Some(points) = self
            .project
            .find_clip(clip_id)
            .and_then(|(_, c)| c.speed_curve())
            .map(|c| c.points)
        else {
            return;
        };
        let nearest = points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let q = to_px(b, p.x, p.y);
                let d = (f32::from(q.x - at.x)).hypot(f32::from(q.y - at.y));
                (i, d)
            })
            .filter(|&(_, d)| d <= HIT_RADIUS)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let Some((index, _)) = nearest else {
            return;
        };
        self.drag = Some(Drag::Beat(Box::new(BeatDrag {
            clip_id: clip_id.to_string(),
            preset: preset.to_string(),
            index,
            points,
            moved: false,
        })));
        cx.notify();
    }

    pub(crate) fn update_beat_drag(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(b) = self.measured(GRAPH) else {
            return;
        };
        let Some(Drag::Beat(d)) = &mut self.drag else {
            return;
        };
        let (x, y) = from_px(b, at);
        let last = d.points.len().saturating_sub(1);
        d.points = if d.index == 0 || d.index == last {
            curve::move_end(&d.points, d.index, y)
        } else {
            curve::move_beat(&d.points, d.index, x, y)
        };
        d.moved = true;
        cx.notify();
    }

    pub(crate) fn commit_beat_drag(&mut self, d: BeatDrag, cx: &mut Context<Self>) {
        if !d.moved {
            return;
        }
        let c = SpeedCurve {
            preset: d.preset,
            points: d.points,
            extra: Default::default(),
        };
        // Keep whatever else the stored curve carries.
        let c = match self
            .project
            .find_clip(&d.clip_id)
            .and_then(|(_, clip)| clip.speed_curve())
        {
            Some(old) => SpeedCurve {
                extra: old.extra,
                ..c
            },
            None => c,
        };
        self.set_speed_curve(&d.clip_id, Some(c), cx);
    }
}

/// "2.6×", "0.45×".
fn speed_label(s: f64) -> String {
    if s < 1. {
        format!("{s:.2}×")
    } else {
        format!("{s:.1}×")
    }
}

fn mono(s: String, color: impl Into<Hsla>) -> gpui::Div {
    div()
        .font_family(theme::FONT_MONO)
        .text_size(px(theme::XS))
        .text_color(color)
        .child(s)
}

/// Graph units → a point in `b` (y up, −1 at the bottom).
fn to_px(b: Bounds<Pixels>, x: f64, y: f64) -> Point<Pixels> {
    let w = f32::from(b.size.width);
    let h = f32::from(b.size.height);
    point(
        b.origin.x + px(x as f32 * w),
        b.origin.y + px((1. - (y as f32 + 1.) / 2.) * h),
    )
}

/// A point in `b` → graph units (clamped).
fn from_px(b: Bounds<Pixels>, p: Point<Pixels>) -> (f64, f64) {
    let w = f32::from(b.size.width).max(1.);
    let h = f32::from(b.size.height).max(1.);
    let x = (f32::from(p.x - b.origin.x) / w).clamp(0., 1.) as f64;
    let y = (1. - 2. * f32::from(p.y - b.origin.y) / h).clamp(-1., 1.) as f64;
    (x, y)
}

/// The curve's line through `b` (sampled finely enough to look smooth).
fn stroke_curve(b: Bounds<Pixels>, points: &[GraphPoint], width: f32, window: &mut Window) {
    let steps = (f32::from(b.size.width) / 2.).clamp(24., 160.) as usize;
    let mut path = PathBuilder::stroke(px(width));
    for i in 0..=steps {
        let x = i as f64 / steps as f64;
        let p = to_px(b, x, curve::evaluate(points, x));
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, accent());
    }
}

fn hline(b: Bounds<Pixels>, y: f64, color: Hsla, window: &mut Window) {
    // The bottom line sits inside the box, not just under it.
    let y = to_px(b, 0., y).y.min(b.origin.y + b.size.height - px(1.));
    window.paint_quad(fill(
        Bounds::new(point(b.origin.x, y), gpui::size(b.size.width, px(1.))),
        color,
    ));
}

fn paint_graph(
    b: Bounds<Pixels>,
    points: &[GraphPoint],
    playhead: Option<f64>,
    held: Option<usize>,
    window: &mut Window,
) {
    for g in GUIDES {
        let strong = g == 0.;
        let color: Hsla = if strong {
            gpui::hsla(0., 0., 1., 0.22)
        } else {
            gpui::hsla(0., 0., 1., 0.08)
        };
        hline(b, g, color, window);
    }
    stroke_curve(b, points, 2., window);
    if let Some(x) = playhead {
        let p = to_px(b, x, 0.);
        window.paint_quad(fill(
            Bounds::new(point(p.x, b.origin.y), gpui::size(px(1.), b.size.height)),
            gpui::hsla(0., 0., 1., 0.6),
        ));
    }
    for (i, p) in points.iter().enumerate() {
        let r = if held == Some(i) { 6. } else { 4.5 };
        let c = to_px(b, p.x, p.y);
        window.paint_quad(
            fill(
                Bounds::new(
                    point(c.x - px(r), c.y - px(r)),
                    gpui::size(px(2. * r), px(2. * r)),
                ),
                accent(),
            )
            .corner_radii(px(r)),
        );
    }
}

/// A preset's tile picture: its shape over the 1× line.
fn thumb(points: Vec<GraphPoint>) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let inset = px(6.);
            let b = Bounds::new(
                point(bounds.origin.x + inset, bounds.origin.y + inset),
                gpui::size(
                    bounds.size.width - inset * 2.,
                    bounds.size.height - inset * 2.,
                ),
            );
            hline(b, 0., gpui::hsla(0., 0., 1., 0.18), window);
            stroke_curve(b, &points, 1.5, window);
        },
    )
    .size_full()
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_mapping_round_trips() {
        let b = Bounds::new(point(px(10.), px(20.)), gpui::size(px(200.), px(100.)));
        for (x, y) in [(0., -1.), (0.5, 0.), (1., 1.), (0.25, 0.5)] {
            let (bx, by) = from_px(b, to_px(b, x, y));
            assert!((bx - x).abs() < 1e-6 && (by - y).abs() < 1e-6);
        }
        assert_eq!(speed_label(0.453), "0.45×");
        assert_eq!(speed_label(2.6), "2.6×");
    }
}
