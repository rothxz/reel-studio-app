//! Speed › Curve: a speed ramp across a video clip, stored on the clip as
//! `speedCurve: { preset, points: [{ x, y, inSlope, outSlope, beat? }] }`
//! (the app's JSON, which the engine plays).
//!
//! x is how far through the clip's source segment (0–1); y is the speed on
//! a split axis: −1 = 0.1×, 0 = 1×, +1 = 10×, each half linear. Between
//! two points the curve is a cubic Hermite piece using the points' slopes.
//! The engine maps time through it; this side builds curves (presets,
//! dragged beats), works out what the clip's length becomes (its average
//! speed) and samples the shape for drawing.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::model::{Clip, Micros, Project};

/// Speed at the axis ends.
pub const MIN: f64 = 0.1;
pub const MAX: f64 = 10.;
/// Steps when integrating the time a curve takes.
const SAMPLES: usize = 256;
/// Beats keep this far apart (fraction of the clip).
pub const MIN_GAP: f64 = 0.02;
/// The preset id of a hand-shaped curve.
pub const CUSTOM: &str = "custom";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphPoint {
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub in_slope: f64,
    #[serde(default)]
    pub out_slope: f64,
    /// An inner point the user can drag.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub beat: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpeedCurve {
    pub preset: String,
    pub points: Vec<GraphPoint>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Speed multiplier at axis height `y`.
pub fn speed_from_axis(y: f64) -> f64 {
    let y = y.clamp(-1., 1.);
    if y >= 0. {
        1. + (MAX - 1.) * y
    } else {
        1. + (1. - MIN) * y
    }
}

/// Axis height of a speed (clamped to the axis).
#[cfg(test)]
fn axis_from_speed(speed: f64) -> f64 {
    let s = speed.clamp(MIN, MAX);
    if s >= 1. {
        (s - 1.) / (MAX - 1.)
    } else {
        (s - 1.) / (1. - MIN)
    }
}

/// The curve's height at `x` (held flat past its ends).
pub fn evaluate(points: &[GraphPoint], x: f64) -> f64 {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return 0.;
    };
    if points.len() < 2 || x <= first.x {
        return first.y;
    }
    if x >= last.x {
        return last.y;
    }
    let i = points.windows(2).position(|w| x < w[1].x).unwrap_or(0);
    let (a, b) = (&points[i], &points[i + 1]);
    let h = b.x - a.x;
    if h <= 0. {
        return b.y;
    }
    let t = (x - a.x) / h;
    let (t2, t3) = (t * t, t * t * t);
    (2. * t3 - 3. * t2 + 1.) * a.y
        + (t3 - 2. * t2 + t) * h * a.out_slope
        + (-2. * t3 + 3. * t2) * b.y
        + (t3 - t2) * h * b.in_slope
}

/// A smooth slope through `p` that never overshoots its neighbours: flat
/// at a peak or dip, else the neighbours' secant held under three times
/// the gentler side (the monotone-cubic limit).
fn smooth_slope(prev: &GraphPoint, p: &GraphPoint, next: &GraphPoint) -> f64 {
    let left = (p.y - prev.y) / (p.x - prev.x).max(1e-6);
    let right = (next.y - p.y) / (next.x - p.x).max(1e-6);
    if left * right <= 0. {
        return 0.;
    }
    let secant = (next.y - prev.y) / (next.x - prev.x).max(1e-6);
    secant.signum() * secant.abs().min(3. * left.abs().min(right.abs()))
}

/// Points through `(x, axis y)` anchors: flat ends, inner ones draggable
/// beats with smooth slopes.
fn through(anchors: &[(f64, f64)]) -> Vec<GraphPoint> {
    let mut pts: Vec<GraphPoint> = anchors
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| GraphPoint {
            x,
            y,
            in_slope: 0.,
            out_slope: 0.,
            beat: i > 0 && i + 1 < anchors.len(),
            extra: Map::new(),
        })
        .collect();
    refit_slopes(&mut pts);
    pts
}

/// Re-fits every inner point's slope (after a beat moved).
fn refit_slopes(pts: &mut [GraphPoint]) {
    for i in 1..pts.len().saturating_sub(1) {
        let s = smooth_slope(&pts[i - 1], &pts[i], &pts[i + 1]);
        pts[i].in_slope = s;
        pts[i].out_slope = s;
    }
}

/// A curve the engine can play: at least two points, x rising strictly
/// from 0 to 1, every number finite.
pub fn is_valid(points: &[GraphPoint]) -> bool {
    points.len() >= 2
        && points[0].x == 0.
        && points[points.len() - 1].x == 1.
        && points.iter().all(|p| {
            [p.x, p.y, p.in_slope, p.out_slope]
                .iter()
                .all(|v| v.is_finite())
        })
        && points.windows(2).all(|w| w[1].x > w[0].x)
}

/// The curve's average speed: source length ÷ the time it takes to play
/// (∫ dx / speed, trapezoids).
pub fn mean_speed(points: &[GraphPoint]) -> f64 {
    let inv = |i: usize| 1. / speed_from_axis(evaluate(points, i as f64 / SAMPLES as f64));
    let mut time = 0.;
    let mut prev = inv(0);
    for i in 1..=SAMPLES {
        let next = inv(i);
        time += (prev + next) / (2. * SAMPLES as f64);
        prev = next;
    }
    if time > 0. { 1. / time } else { 1. }
}

/// How far through the clip's timeline length each source step is
/// reached: `table[i]` for source fraction `i / SAMPLES`, 0 → 1, rising.
pub fn timing_table(points: &[GraphPoint]) -> Vec<f64> {
    let inv = |i: usize| 1. / speed_from_axis(evaluate(points, i as f64 / SAMPLES as f64));
    let mut table = Vec::with_capacity(SAMPLES + 1);
    table.push(0.);
    let mut prev = inv(0);
    let mut sum = 0.;
    for i in 1..=SAMPLES {
        let next = inv(i);
        sum += (prev + next) / (2. * SAMPLES as f64);
        table.push(sum);
        prev = next;
    }
    if sum > 0. {
        table.iter_mut().for_each(|v| *v /= sum);
    }
    table
}

/// The source fraction played at timeline fraction `u`.
pub fn source_fraction(table: &[f64], u: f64) -> f64 {
    let n = table.len().saturating_sub(1);
    if n == 0 || u <= 0. {
        return 0.;
    }
    if u >= 1. {
        return 1.;
    }
    let hi = table.partition_point(|&v| v <= u).clamp(1, n);
    let lo = hi - 1;
    let span = table[hi] - table[lo];
    let f = if span > 0. {
        (u - table[lo]) / span
    } else {
        0.
    };
    (lo as f64 + f) / n as f64
}

/// The beat within `eps` of `x` (not an end).
pub fn beat_near(points: &[GraphPoint], x: f64, eps: f64) -> Option<usize> {
    (1..points.len().saturating_sub(1))
        .filter(|&i| (points[i].x - x).abs() <= eps)
        .min_by(|&a, &b| (points[a].x - x).abs().total_cmp(&(points[b].x - x).abs()))
}

/// A new beat on the curve at `x` (None when it would crowd another).
pub fn add_beat(points: &[GraphPoint], x: f64) -> Option<Vec<GraphPoint>> {
    let at = points.iter().position(|p| p.x > x)?;
    if at == 0 || x - points[at - 1].x < MIN_GAP || points[at].x - x < MIN_GAP {
        return None;
    }
    let mut pts = points.to_vec();
    pts.insert(
        at,
        GraphPoint {
            x,
            y: evaluate(points, x),
            in_slope: 0.,
            out_slope: 0.,
            beat: true,
            extra: Map::new(),
        },
    );
    refit_slopes(&mut pts);
    Some(pts)
}

/// The curve without beat `index` (ends stay).
pub fn delete_beat(points: &[GraphPoint], index: usize) -> Vec<GraphPoint> {
    let mut pts = points.to_vec();
    if index > 0 && index + 1 < pts.len() {
        pts.remove(index);
        refit_slopes(&mut pts);
    }
    pts
}

/// Sets an end point's height (ends slide up and down only).
pub fn move_end(points: &[GraphPoint], index: usize, y: f64) -> Vec<GraphPoint> {
    let mut pts = points.to_vec();
    if let Some(p) = pts.get_mut(index) {
        p.y = y.clamp(-1., 1.);
    }
    refit_slopes(&mut pts);
    pts
}

/// The preset's tile / card label.
pub fn label(id: &str) -> &'static str {
    PRESETS
        .iter()
        .find(|p| p.id == id)
        .map_or("Custom", |p| p.label)
}

/// Moves beat `index` to `(x, y)`: x kept between its neighbours, y on
/// the axis, slopes re-fitted. Ends and unknown indices don't move.
pub fn move_beat(points: &[GraphPoint], index: usize, x: f64, y: f64) -> Vec<GraphPoint> {
    let mut pts = points.to_vec();
    if index == 0 || index + 1 >= pts.len() {
        return pts;
    }
    let (lo, hi) = (pts[index - 1].x + MIN_GAP, pts[index + 1].x - MIN_GAP);
    pts[index].x = if lo <= hi {
        x.clamp(lo, hi)
    } else {
        pts[index].x
    };
    pts[index].y = y.clamp(-1., 1.);
    refit_slopes(&mut pts);
    pts
}

/// A ready-made ramp.
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    anchors: &'static [(f64, f64)],
}

impl Preset {
    pub fn points(&self) -> Vec<GraphPoint> {
        through(self.anchors)
    }
}

/// The ramps on offer, in tile order. Shapes: speed up / slow down around
/// beats on the 0.1×–10× axis (0 = normal speed).
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "montage",
        label: "Montage",
        anchors: &[(0., 0.), (0.3, 0.6), (0.6, -0.6), (0.78, 0.), (1., 0.)],
    },
    Preset {
        id: "hero",
        label: "Hero",
        anchors: &[
            (0., 0.),
            (0.28, 0.5),
            (0.45, -0.5),
            (0.55, -0.5),
            (0.72, 0.5),
            (1., 0.),
        ],
    },
    Preset {
        id: "bullet",
        label: "Bullet",
        anchors: &[
            (0., 0.5),
            (0.38, 0.5),
            (0.46, -0.6),
            (0.54, -0.6),
            (0.62, 0.5),
            (1., 0.5),
        ],
    },
    Preset {
        id: "jumpCut",
        label: "Jump Cut",
        anchors: &[
            (0., -0.5),
            (0.35, -0.5),
            (0.5, 0.55),
            (0.65, -0.5),
            (1., -0.5),
        ],
    },
    Preset {
        id: "flashIn",
        label: "Flash In",
        anchors: &[(0., 0.5), (0.32, 0.5), (0.62, 0.), (1., 0.)],
    },
    Preset {
        id: "flashOut",
        label: "Flash Out",
        anchors: &[(0., 0.), (0.38, 0.), (0.68, 0.5), (1., 0.5)],
    },
];

/// Custom starts flat at 1× with three beats to drag.
const CUSTOM_ANCHORS: &[(f64, f64)] = &[(0., 0.), (0.25, 0.), (0.5, 0.), (0.75, 0.), (1., 0.)];

/// A fresh curve for a preset id (or Custom); None for an unknown id.
pub fn preset_curve(id: &str) -> Option<SpeedCurve> {
    let points = if id == CUSTOM {
        through(CUSTOM_ANCHORS)
    } else {
        PRESETS.iter().find(|p| p.id == id)?.points()
    };
    Some(SpeedCurve {
        preset: id.to_string(),
        points,
        extra: Map::new(),
    })
}

impl Clip {
    /// The clip's speed ramp, when it has one the engine can play.
    pub fn speed_curve(&self) -> Option<SpeedCurve> {
        let curve: SpeedCurve =
            serde_json::from_value(self.extra.get("speedCurve")?.clone()).ok()?;
        is_valid(&curve.points).then_some(curve)
    }
}

/// A sped clip is never shorter than this.
const MIN_DURATION: Micros = 1_000;

impl Project {
    /// Re-times a clip to `speed` (the curve's average when `curve` is
    /// set; a constant speed drops any curve), keeping its source segment:
    /// its timeline length becomes segment ÷ speed. The caller runs it
    /// through the magnet on the main lane. False when nothing changed.
    pub fn retime_clip(&mut self, clip_id: &str, speed: f64, curve: Option<SpeedCurve>) -> bool {
        let Some(clip) = self.clip_mut(clip_id) else {
            return false;
        };
        let curve = curve.filter(|c| clip.kind == "video" && is_valid(&c.points));
        let speed = match &curve {
            Some(c) => mean_speed(&c.points),
            None => speed,
        }
        .clamp(
            crate::ui::inspector::MIN_SPEED,
            crate::ui::inspector::MAX_SPEED,
        );
        let old_speed = if clip.speed > 0. { clip.speed } else { 1. };
        let source = clip.timeline_duration as f64 * old_speed;
        let duration = ((source / speed).round() as Micros).max(MIN_DURATION);
        let curve_value = curve.and_then(|c| serde_json::to_value(c).ok());
        if clip.speed == speed
            && clip.timeline_duration == duration
            && clip.extra.get("speedCurve") == curve_value.as_ref()
        {
            return false;
        }
        clip.speed = speed;
        clip.timeline_duration = duration;
        match curve_value {
            Some(v) => clip.extra.insert("speedCurve".into(), v),
            None => clip.extra.remove("speedCurve"),
        };
        self.recompute_duration();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_round_trips() {
        for s in [0.1, 0.5, 1., 2.6, 10.] {
            assert!((speed_from_axis(axis_from_speed(s)) - s).abs() < 1e-9);
        }
        assert_eq!(speed_from_axis(0.), 1.);
        assert!((speed_from_axis(-1.) - MIN).abs() < 1e-12);
        assert_eq!(speed_from_axis(1.), MAX);
    }

    #[test]
    fn flat_curve_is_normal_speed() {
        let c = preset_curve(CUSTOM).unwrap();
        assert!(is_valid(&c.points));
        assert!((mean_speed(&c.points) - 1.).abs() < 1e-9);
        assert_eq!(c.points.iter().filter(|p| p.beat).count(), 3);
    }

    #[test]
    fn every_preset_is_playable() {
        for p in PRESETS {
            let pts = p.points();
            assert!(is_valid(&pts), "{}", p.id);
            let m = mean_speed(&pts);
            assert!(m > MIN && m < MAX, "{} {m}", p.id);
            // Hermite pieces through the anchors.
            for &(x, y) in p.anchors {
                assert!((evaluate(&pts, x) - y).abs() < 1e-9);
            }
        }
        // Fast all the way: faster than normal on average.
        assert!(mean_speed(&preset_curve("flashIn").unwrap().points) > 1.);
        assert!(mean_speed(&preset_curve("jumpCut").unwrap().points) < 1.);
    }

    #[test]
    fn beats_stay_between_neighbours() {
        let pts = preset_curve(CUSTOM).unwrap().points;
        let moved = move_beat(&pts, 1, 0.9, 3.);
        assert!(moved[1].x <= moved[2].x - MIN_GAP + 1e-12);
        assert_eq!(moved[1].y, 1.);
        assert_eq!(move_beat(&pts, 0, 0.5, 0.5), pts);
    }

    #[test]
    fn timing_inverts_and_beats_add_and_go() {
        let pts = preset_curve("flashIn").unwrap().points;
        let table = timing_table(&pts);
        assert_eq!(table[0], 0.);
        assert!((table[SAMPLES] - 1.).abs() < 1e-12);
        assert!(table.windows(2).all(|w| w[1] > w[0]));
        // Fast first: half the source has played before half the time.
        assert!(source_fraction(&table, 0.5) > 0.5);
        for u in [0.1, 0.37, 0.9] {
            let x = source_fraction(&table, u);
            let i = (x * SAMPLES as f64).floor() as usize;
            assert!(table[i] <= u + 1e-12 && u <= table[i + 1] + 1e-12);
        }
        let flat = preset_curve(CUSTOM).unwrap().points;
        assert!(add_beat(&flat, 0.26).is_none());
        let more = add_beat(&flat, 0.4).unwrap();
        assert_eq!(more.len(), 6);
        assert_eq!(beat_near(&more, 0.41, 0.02), Some(2));
        assert_eq!(delete_beat(&more, 2), flat);
        assert_eq!(delete_beat(&more, 0).len(), 6);
        assert_eq!(move_end(&flat, 0, 2.)[0].y, 1.);
    }

    #[test]
    fn json_is_the_apps() {
        let v = serde_json::json!({
            "preset": "custom",
            "points": [
                { "x": 0.0, "y": 0.0, "inSlope": 0.0, "outSlope": 0.0 },
                { "x": 0.5, "y": 0.4, "inSlope": 0.1, "outSlope": 0.1, "beat": true, "ease": 2 },
                { "x": 1.0, "y": 0.0, "inSlope": 0.0, "outSlope": 0.0 }
            ]
        });
        let c: SpeedCurve = serde_json::from_value(v.clone()).unwrap();
        assert!(c.points[1].beat);
        assert_eq!(serde_json::to_value(&c).unwrap(), v);
    }
}
