//! What the player draws at a time, as render passes: the shown clip's own
//! looks, then the effect-lane clips over the whole canvas.
//!
//! Per clip (the order export shades a clip in): its video effect if it
//! distorts, its filter (a Classic look's colour matrix, or a packaged
//! filter's shader mixed by its intensity), then its video effect if it's
//! laid over the picture. Then, over the canvas (the clip fitted into it on
//! the background colour), every effect-lane clip under the playhead, lane
//! order. Effects and filters at intensity 0, switched off, on a
//! deactivated clip or a hidden lane are skipped.

use std::sync::Arc;

use crate::catalog::{self, EFFECT_PREFIX, FILTER_PREFIX};
use crate::model::{Clip, EffectInstance, Micros, Project, SECOND, TrackType};

use super::package::{EffectKind, EffectPackage, Lookup, Packages};

/// The time-varying inputs of an effect shader (the dialect's parameters).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectInputs {
    /// Seconds since the effect began, scaled by its Speed slider.
    pub time: f32,
    /// Intensity 0–1.
    pub k: f32,
    /// How far through its clip, 0–1.
    pub progress: f32,
    /// The clip's length in seconds.
    pub duration: f32,
    /// The effect's own sliders, 0–1.
    pub p: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pass {
    /// A 4×5 colour matrix (rows R, G, B, A; column 5 the offset) on
    /// unpremultiplied 0–1 colour.
    Matrix([f32; 20]),
    /// A packaged filter: `fl_filter`'s body, mixed in by `k`.
    Filter { body: Arc<str>, k: f32 },
    Effect {
        package: Arc<EffectPackage>,
        inputs: EffectInputs,
    },
}

/// The canvas the global effects run over.
#[derive(Clone, Debug, PartialEq)]
pub struct CanvasPasses {
    pub width: u32,
    pub height: u32,
    /// Straight RGBA 0–1.
    pub background: [f32; 4],
    pub passes: Vec<Pass>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stack {
    /// On the clip's frame, at its own size.
    pub clip: Vec<Pass>,
    /// Over the canvas; None when no effect-lane clip is on.
    pub canvas: Option<CanvasPasses>,
}

impl Stack {
    pub fn is_empty(&self) -> bool {
        self.clip.is_empty() && self.canvas.is_none()
    }
}

/// A slider held down: the preview follows it before the edit lands.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveParam {
    pub clip_id: String,
    pub effect_id: String,
    pub param: String,
    pub value: f64,
}

/// What building a stack found missing: catalog rows (for an effect's
/// package key) and packages to download.
#[derive(Default)]
pub struct Needs {
    pub rows: Vec<String>,
    pub effect_packages: Vec<String>,
    pub filter_packages: Vec<String>,
}

/// Where the builder looks things up.
pub struct Sources<'a> {
    pub packages: &'a Packages,
    /// An effect's current package key (its catalog row).
    pub effect_key: &'a dyn Fn(&str) -> Option<String>,
    /// A Classic filter's matrix at an intensity (None: identity / unknown).
    pub filter_matrix: &'a dyn Fn(&str, f64) -> Option<[f32; 20]>,
    pub live: Option<&'a LiveParam>,
}

/// Speed slider (0–100, 50 = normal) → clock multiplier: 0.25× … 1× … 2×.
pub fn speed_multiplier(percent: f64) -> f64 {
    let p = percent.clamp(0., 100.);
    if p <= 50. {
        0.25 + p / 50. * 0.75
    } else {
        1. + (p - 50.) / 50.
    }
}

fn param(e: &EffectInstance, id: &str, default: f64, clip: &Clip, live: Option<&LiveParam>) -> f64 {
    match live {
        Some(l) if l.clip_id == clip.id && l.effect_id == e.id && l.param == id => l.value,
        _ => e.param(id).unwrap_or(default),
    }
}

/// The first enabled effect of a kind on a clip (the one that's on).
fn active(clip: &Clip, prefix: &str) -> Option<EffectInstance> {
    clip.effects()
        .into_iter()
        .find(|e| e.enabled && e.kind.starts_with(prefix))
}

/// The clip's video effect as a pass at `t`, if it's on and here.
fn effect_pass(clip: &Clip, t: Micros, src: &Sources, needs: &mut Needs) -> Option<Pass> {
    let e = active(clip, EFFECT_PREFIX)?;
    let live = src.live;
    let k = param(&e, "intensity", 0., clip, live);
    if k <= 0. {
        return None;
    }
    let name = e.name();
    let Some(key) = (src.effect_key)(name) else {
        needs.rows.push(name.to_string());
        return None;
    };
    let package = match src.packages.effect(&key) {
        Lookup::Ready(p) => p,
        Lookup::Missing => {
            needs.effect_packages.push(key);
            return None;
        }
        Lookup::Failed => return None,
    };
    let into = (t - clip.timeline_start).max(0) as f64 / SECOND as f64;
    let duration = clip.timeline_duration as f64 / SECOND as f64;
    let speed = speed_multiplier(param(&e, "speed", 50., clip, live));
    let p = |id| (param(&e, id, 50., clip, live) / 100.).clamp(0., 1.) as f32;
    Some(Pass::Effect {
        package,
        inputs: EffectInputs {
            time: (into * speed) as f32,
            k: (k / 100.).clamp(0., 1.) as f32,
            progress: if duration > 0. {
                (into / duration).clamp(0., 1.) as f32
            } else {
                0.
            },
            duration: duration as f32,
            p: [p("p1"), p("p2"), p("p3"), p("p4")],
        },
    })
}

/// The clip's filter as a pass, if it's on and here.
fn filter_pass(clip: &Clip, src: &Sources, needs: &mut Needs) -> Option<Pass> {
    let e = active(clip, FILTER_PREFIX)?;
    let intensity = param(&e, "intensity", 0., clip, src.live);
    if intensity <= 0. {
        return None;
    }
    let name = e.name();
    if catalog::is_classic_filter(name) {
        return (src.filter_matrix)(name, intensity.min(100.)).map(Pass::Matrix);
    }
    let key = catalog::filter_package_key(name)?;
    match src.packages.filter(key) {
        Lookup::Ready(body) => Some(Pass::Filter {
            body,
            k: (intensity / 100.).clamp(0., 1.) as f32,
        }),
        Lookup::Missing => {
            needs.filter_packages.push(key.to_string());
            None
        }
        Lookup::Failed => None,
    }
}

fn is_distortion(p: &Pass) -> bool {
    matches!(p, Pass::Effect { package, .. } if package.kind == EffectKind::Distortion)
}

/// The passes for `shown` (the clip the player draws) at `t`, on a canvas
/// `canvas_px` pixels big.
pub fn build(
    project: &Project,
    shown: Option<&Clip>,
    t: Micros,
    canvas_px: (u32, u32),
    src: &Sources,
    needs: &mut Needs,
) -> Stack {
    let mut stack = Stack::default();
    if let Some(clip) = shown {
        let effect = effect_pass(clip, t, src, needs);
        let (before, after): (Vec<Pass>, Vec<Pass>) = effect.into_iter().partition(is_distortion);
        stack.clip.extend(before);
        stack.clip.extend(filter_pass(clip, src, needs));
        stack.clip.extend(after);
    }

    let mut lanes: Vec<_> = project
        .tracks
        .iter()
        .filter(|tr| tr.kind == TrackType::Effect && !tr.hidden)
        .collect();
    lanes.sort_by(|a, b| a.order.total_cmp(&b.order));
    let mut global = vec![];
    for lane in lanes {
        let mut on: Vec<&Clip> = lane
            .clips
            .iter()
            .filter(|c| !c.hidden && t >= c.timeline_start && t < c.end())
            .collect();
        on.sort_by_key(|c| c.timeline_start);
        global.extend(on.into_iter().filter_map(|c| effect_pass(c, t, src, needs)));
    }
    if !global.is_empty() {
        stack.canvas = Some(CanvasPasses {
            width: canvas_px.0,
            height: canvas_px.1,
            background: hex_rgba(&project.canvas.background_color),
            passes: global,
        });
    }
    stack
}

/// "#rrggbb" → straight RGBA 0–1 (black when it doesn't parse).
pub fn hex_rgba(s: &str) -> [f32; 4] {
    let hex = s.trim_start_matches('#');
    match u32::from_str_radix(hex, 16) {
        Ok(v) if hex.len() == 6 => [
            ((v >> 16) & 0xff) as f32 / 255.,
            ((v >> 8) & 0xff) as f32 / 255.,
            (v & 0xff) as f32 / 255.,
            1.,
        ],
        _ => [0., 0., 0., 1.],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::package::{Fetched, Packages};
    use crate::model::MediaItem;

    fn project_with_effect() -> (Project, String, String) {
        let mut p = Project::empty();
        let item = MediaItem {
            path: "/tmp/a.mp4".into(),
            name: "a.mp4".into(),
            duration: 10 * SECOND,
            width: 1920,
            height: 1080,
            has_video: true,
            still: false,
        };
        let video = p.insert_media(&item, None);
        let fx = EffectInstance::new(
            "Overlays:Shake".into(),
            &[("intensity".into(), 60.), ("speed".into(), 100.)],
        );
        let effect = p.add_effect_clip(2 * SECOND, fx);
        (p, video, effect)
    }

    #[test]
    fn effect_lane_clip_runs_over_the_canvas_while_under_the_playhead() {
        let (p, video, _) = project_with_effect();
        let mut packages = Packages::default();
        assert!(packages.begin("effects/shake.json"));
        packages.end(Fetched::Effect(
            "effects/shake.json".into(),
            Ok(EffectPackage {
                kind: EffectKind::Distortion,
                body: "return S(uv);".into(),
            }),
        ));
        let key = |_: &str| Some("effects/shake.json".to_string());
        let none = |_: &str, _: f64| None;
        let src = Sources {
            packages: &packages,
            effect_key: &key,
            filter_matrix: &none,
            live: None,
        };
        let clip = p.find_clip(&video).unwrap().1.clone();
        let mut needs = Needs::default();
        // Before the effect clip: nothing to do.
        assert!(build(&p, Some(&clip), SECOND, (1280, 720), &src, &mut needs).is_empty());
        // 1 s into it at Speed 100 (2×): time 2 s, a third through.
        let s = build(&p, Some(&clip), 3 * SECOND, (1280, 720), &src, &mut needs);
        let canvas = s.canvas.expect("canvas passes");
        let Pass::Effect { inputs, .. } = &canvas.passes[0] else {
            panic!("an effect pass");
        };
        assert_eq!(inputs.time, 2.);
        assert!((inputs.k - 0.6).abs() < 1e-6);
        assert!((inputs.progress - 1. / 3.).abs() < 1e-6);
        assert_eq!(inputs.p, [0.5; 4]);
        assert!(needs.effect_packages.is_empty());
    }

    #[test]
    fn missing_rows_and_packages_are_asked_for() {
        let (p, _, effect) = project_with_effect();
        let packages = Packages::default();
        let no_key = |_: &str| None;
        let none = |_: &str, _: f64| None;
        let mut src = Sources {
            packages: &packages,
            effect_key: &no_key,
            filter_matrix: &none,
            live: None,
        };
        let mut needs = Needs::default();
        assert!(build(&p, None, 3 * SECOND, (10, 10), &src, &mut needs).is_empty());
        assert_eq!(needs.rows, ["Shake"]);
        let key = |_: &str| Some("effects/shake.json".to_string());
        src.effect_key = &key;
        let mut needs = Needs::default();
        build(&p, None, 3 * SECOND, (10, 10), &src, &mut needs);
        assert_eq!(needs.effect_packages, ["effects/shake.json"]);
        // A held slider at 0 switches it off before the edit lands.
        let clip = p.find_clip(&effect).unwrap().1;
        let live = LiveParam {
            clip_id: effect.clone(),
            effect_id: clip.effects()[0].id.clone(),
            param: "intensity".into(),
            value: 0.,
        };
        src.live = Some(&live);
        let mut needs = Needs::default();
        build(&p, None, 3 * SECOND, (10, 10), &src, &mut needs);
        assert!(needs.effect_packages.is_empty());
    }

    #[test]
    fn speeds_and_colours() {
        assert_eq!(speed_multiplier(0.), 0.25);
        assert_eq!(speed_multiplier(50.), 1.);
        assert_eq!(speed_multiplier(100.), 2.);
        assert_eq!(hex_rgba("#ff0080"), [1., 0., 128. / 255., 1.]);
        assert_eq!(hex_rgba("nope"), [0., 0., 0., 1.]);
    }
}
