//! What the player draws at a time: every visible picture (back to front)
//! with its own looks and its place on the canvas, then the effect-lane
//! clips over the whole canvas.
//!
//! Per picture (the order export shades a clip in): its video effect if it
//! distorts, its filter (a Classic look's colour matrix, or a packaged
//! filter's shader mixed by its intensity) with its Adjust sliders' matrix
//! (inside a Classic look, after a packaged one), then its video effect if
//! it's laid over the picture. Then it's placed on the canvas: crop, mirror /
//! flip, transform, shape mask, opacity and Splice blend over what's
//! below. Under everything, the canvas colour, or the main clip's
//! Background (a colour, a photo, or its own picture blurred). Then every
//! effect-lane clip under the playhead, lane order. Effects and filters at
//! intensity 0, switched off, on a deactivated clip or a hidden lane are
//! skipped.

use std::sync::Arc;

use crate::catalog::{self, EFFECT_PREFIX, FILTER_PREFIX};
use crate::model::{Clip, EffectInstance, Micros, Project, SECOND, TrackType};
use crate::visual::Background;

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

/// A clip's shape mask, as the compose pass reads it (see
/// `visual::ClipMask`; `shape` in `MaskShape::ALL` order).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaskPlan {
    pub shape: u32,
    pub center: [f32; 2],
    pub size: [f32; 2],
    /// Degrees, clockwise.
    pub rotation: f32,
    pub feather: f32,
    pub invert: bool,
}

/// Where and how a picture lands on the canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    /// Centre offset in canvas widths / heights (+y down).
    pub x: f32,
    pub y: f32,
    /// Over the contain-fit.
    pub scale: f32,
    /// Degrees, clockwise.
    pub rotation: f32,
    pub opacity: f32,
    pub flip_x: bool,
    pub flip_y: bool,
    /// The visible part of the source, 0–1: x, y, width, height.
    pub crop: [f32; 4],
    /// The crop's straighten, degrees clockwise: the source turns about
    /// the crop's centre (grown to cover it) under the crop box.
    pub crop_turn: f32,
    pub mask: Option<MaskPlan>,
    /// Splice: index in `visual::BLEND_MODES` (0 normal).
    pub blend: u32,
    /// The picture's own size on the canvas at scale 1 (canvas widths,
    /// heights) instead of contain-fitting it: text and stickers.
    pub natural: Option<[f32; 2]>,
}

impl Default for Placement {
    fn default() -> Self {
        Placement {
            x: 0.,
            y: 0.,
            scale: 1.,
            rotation: 0.,
            opacity: 1.,
            flip_x: false,
            flip_y: false,
            crop: [0., 0., 1., 1.],
            crop_turn: 0.,
            mask: None,
            blend: 0,
            natural: None,
        }
    }
}

impl Placement {
    /// Contain-fit as it is: nothing to compose.
    pub fn is_plain(&self) -> bool {
        *self == Placement::default()
    }
}

/// One picture: its looks (at its own size), then its place.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerPlan {
    pub passes: Vec<Pass>,
    pub place: Placement,
}

/// What's under the pictures besides the canvas colour.
#[derive(Clone, Debug, PartialEq)]
pub enum Backdrop {
    /// Layer `layer`'s picture covering the canvas, blurred by `sigma` (a
    /// fraction of the canvas's short side).
    Blur { layer: usize, sigma: f32 },
    /// A photo (its path) covering the canvas.
    Image(String),
}

/// The canvas the pictures are composed on.
#[derive(Clone, Debug, PartialEq)]
pub struct CanvasPasses {
    pub width: u32,
    pub height: u32,
    /// Straight RGBA 0–1.
    pub background: [f32; 4],
    pub backdrop: Option<Backdrop>,
    /// The effect-lane clips, over the footage: they run once this many
    /// layers are composed (after the last video / photo), so text and
    /// stickers above stay unfiltered.
    pub passes: Vec<Pass>,
    pub passes_after: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stack {
    /// Back to front.
    pub layers: Vec<LayerPlan>,
    /// None: one picture as it is (through its own looks, at its own
    /// size), contain-fitted by the player.
    pub canvas: Option<CanvasPasses>,
}

impl Stack {
    /// Nothing for the GPU: the one picture shows as it was decoded.
    pub fn is_empty(&self) -> bool {
        self.canvas.is_none() && self.layers.iter().all(|l| l.passes.is_empty())
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
    /// The clip's Adjust sliders as a matrix at the preview time (None:
    /// untouched).
    pub adjust_matrix: &'a dyn Fn(&Clip) -> Option<[f32; 20]>,
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

/// Gaussian sigma of each Background blur strength, as a fraction of the
/// canvas's short side (the old editor's four tiles).
pub fn blur_sigma(level: u8) -> f32 {
    match level {
        1 => 0.012,
        2 => 0.028,
        3 => 0.05,
        _ => 0.08,
    }
}

/// The plan for the pictures under the preview time `t` (back to front,
/// each with its placement there) on a canvas `canvas_px` pixels big.
pub fn build(
    project: &Project,
    layers: &[(&Clip, Placement)],
    t: Micros,
    canvas_px: (u32, u32),
    src: &Sources,
    needs: &mut Needs,
) -> Stack {
    let mut stack = Stack::default();
    for (clip, place) in layers {
        let mut passes = vec![];
        let effect = effect_pass(clip, t, src, needs);
        let (before, after): (Vec<Pass>, Vec<Pass>) = effect.into_iter().partition(is_distortion);
        passes.extend(before);
        // Like export: a packaged filter's shader, then the colour matrix
        // (Adjust inside, a Classic filter's look outside).
        let filter = filter_pass(clip, src, needs);
        let adjust = (src.adjust_matrix)(clip).map(Pass::Matrix);
        if matches!(filter, Some(Pass::Matrix(_))) {
            passes.extend(adjust);
            passes.extend(filter);
        } else {
            passes.extend(filter);
            passes.extend(adjust);
        }
        passes.extend(after);
        stack.layers.push(LayerPlan {
            passes,
            place: place.clone(),
        });
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

    // The main clip's Background, while it's on screen.
    let main = project.main_track().map(|tr| tr.id.as_str());
    let mut background = hex_rgba(&project.canvas.background_color);
    let mut backdrop = None;
    let main_layer = layers.iter().position(|(c, _)| {
        main.is_some_and(|m| project.track_of(&c.id).is_some_and(|tr| tr.id == m))
    });
    if let Some(i) = main_layer {
        match layers[i].0.background() {
            Some(Background::Color { color }) => background = hex_rgba(&color),
            Some(Background::Blur { level }) => {
                backdrop = Some(Backdrop::Blur {
                    layer: i,
                    sigma: blur_sigma(level),
                })
            }
            Some(Background::Image { uri, .. }) => backdrop = Some(Backdrop::Image(uri)),
            // Patterns aren't drawn yet: the canvas colour.
            Some(Background::Pattern { .. }) | None => {}
        }
    }
    let canvas_colour = hex_rgba(&project.canvas.background_color);
    let plain = stack.layers.len() == 1
        && stack.layers[0].place.is_plain()
        && global.is_empty()
        && backdrop.is_none()
        && background == canvas_colour;
    if (!plain && !stack.layers.is_empty()) || !global.is_empty() {
        let passes_after = layers
            .iter()
            .rposition(|(c, _)| c.kind == "video" || c.kind == "image")
            .map_or(0, |i| i + 1);
        stack.canvas = Some(CanvasPasses {
            width: canvas_px.0,
            height: canvas_px.1,
            background,
            backdrop,
            passes: global,
            passes_after,
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
            adjust_matrix: &|_| None,
            live: None,
        };
        let clip = p.find_clip(&video).unwrap().1.clone();
        let layers = [(&clip, Placement::default())];
        let mut needs = Needs::default();
        // Before the effect clip: nothing to do.
        assert!(build(&p, &layers, SECOND, (1280, 720), &src, &mut needs).is_empty());
        // 1 s into it at Speed 100 (2×): time 2 s, a third through.
        let s = build(&p, &layers, 3 * SECOND, (1280, 720), &src, &mut needs);
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
            adjust_matrix: &|_| None,
            live: None,
        };
        let mut needs = Needs::default();
        assert!(build(&p, &[], 3 * SECOND, (10, 10), &src, &mut needs).is_empty());
        assert_eq!(needs.rows, ["Shake"]);
        let key = |_: &str| Some("effects/shake.json".to_string());
        src.effect_key = &key;
        let mut needs = Needs::default();
        build(&p, &[], 3 * SECOND, (10, 10), &src, &mut needs);
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
        build(&p, &[], 3 * SECOND, (10, 10), &src, &mut needs);
        assert!(needs.effect_packages.is_empty());
    }

    #[test]
    fn a_placed_picture_or_a_background_needs_the_canvas() {
        let (p, video, _) = project_with_effect();
        let packages = Packages::default();
        let key = |_: &str| None;
        let none = |_: &str, _: f64| None;
        let src = Sources {
            packages: &packages,
            effect_key: &key,
            filter_matrix: &none,
            adjust_matrix: &|_| None,
            live: None,
        };
        let mut clip = p.find_clip(&video).unwrap().1.clone();
        let mut needs = Needs::default();
        let plain = [(&clip, Placement::default())];
        assert!(build(&p, &plain, 0, (64, 36), &src, &mut needs).is_empty());
        let zoomed = Placement {
            scale: 0.5,
            ..Placement::default()
        };
        let s = build(&p, &[(&clip, zoomed)], 0, (64, 36), &src, &mut needs);
        assert_eq!(
            s.canvas.as_ref().map(|c| (c.width, c.height)),
            Some((64, 36))
        );
        // The main clip's Background: a colour fills, a blur is its picture.
        clip.set_background(Some(&Background::Color {
            color: "#ff0000".into(),
        }));
        let mut p = p.clone();
        p.update_clip(&video, |c| {
            *c = clip.clone();
            true
        });
        let s = build(
            &p,
            &[(&clip, Placement::default())],
            0,
            (64, 36),
            &src,
            &mut needs,
        );
        assert_eq!(s.canvas.unwrap().background, [1., 0., 0., 1.]);
        clip.set_background(Some(&Background::Blur { level: 2 }));
        p.update_clip(&video, |c| {
            *c = clip.clone();
            true
        });
        let s = build(
            &p,
            &[(&clip, Placement::default())],
            0,
            (64, 36),
            &src,
            &mut needs,
        );
        assert_eq!(
            s.canvas.unwrap().backdrop,
            Some(Backdrop::Blur {
                layer: 0,
                sigma: 0.028
            })
        );
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
