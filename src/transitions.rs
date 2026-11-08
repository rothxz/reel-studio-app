//! Transitions between two clips of the main lane.
//!
//! A transition is stored on the outgoing clip as `outTransition` (`id`,
//! `type`, `durationMicros`), and overlaps the two clips by its length: the
//! clips after it move left by as much, so the video gets shorter, not
//! longer. Over the overlap the outgoing clip moves out and the incoming
//! one (drawn on top) moves in, both by a few posed waypoints; or the
//! incoming one is revealed over the outgoing picture by a downloaded
//! shader (the reveals that also open clips, compiled in from the app
//! checkout at build time, their packages fetched on first use).
//!
//! A stored type this build doesn't know plays as Mix.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::effects::stack::Placement;
use crate::model::{Clip, Micros, Project, SECOND, TrackType, new_id};

pub const MIN_MICROS: Micros = 100_000;
/// The slider's ceiling; a seam takes at most half of either clip.
pub const MAX_MICROS: Micros = 3 * SECOND;
const DEFAULT_MICROS: Micros = 500_000;
/// Reveals start no longer than this (they were made at 1–2 s).
const REVEAL_DEFAULT_CAP: Micros = SECOND;
pub const TRENDING: &str = "trending";
/// A pick replays the seam from this long before its window.
pub const REPLAY_LEAD: Micros = 500_000;

/// The category tabs (key, label), in order.
pub const CATEGORIES: &[(&str, &str)] = &[
    (TRENDING, "Trending"),
    ("basic", "Basic"),
    ("slide", "Slide"),
    ("zoom", "Zoom"),
    ("spin", "Spin"),
    ("light", "Light"),
    ("glitch", "Glitch"),
    ("mask", "Mask"),
    ("3d", "3D"),
];

/// The reveal categories that read as transitions.
const REVEAL_CATEGORIES: &[&str] = &["light", "glitch", "mask", "3d"];

/// A clip's pose: its rest is all defaults. Translation is in its own
/// drawn width / height, rotation degrees clockwise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub opacity: f32,
    pub tx: f32,
    pub ty: f32,
    pub scale: f32,
    pub rotation: f32,
    pub sx: f32,
    pub sy: f32,
}

pub const REST: Pose = Pose {
    opacity: 1.,
    tx: 0.,
    ty: 0.,
    scale: 1.,
    rotation: 0.,
    sx: 1.,
    sy: 1.,
};

impl Pose {
    fn lerp(a: Pose, b: Pose, k: f32) -> Pose {
        let m = |x: f32, y: f32| x + (y - x) * k;
        Pose {
            opacity: m(a.opacity, b.opacity),
            tx: m(a.tx, b.tx),
            ty: m(a.ty, b.ty),
            scale: m(a.scale, b.scale),
            rotation: m(a.rotation, b.rotation),
            sx: m(a.sx, b.sx),
            sy: m(a.sy, b.sy),
        }
    }

    /// Both poses at once (a clip short enough to be in two windows).
    fn then(self, o: Pose) -> Pose {
        Pose {
            opacity: self.opacity * o.opacity,
            tx: self.tx + o.tx,
            ty: self.ty + o.ty,
            scale: self.scale * o.scale,
            rotation: self.rotation + o.rotation,
            sx: self.sx * o.sx,
            sy: self.sy * o.sy,
        }
    }

    /// Puts the pose on a picture's placement.
    pub fn apply(&self, p: &mut Placement) {
        p.opacity = (p.opacity * self.opacity).clamp(0., 1.);
        p.scale *= self.scale.max(0.);
        p.rotation += self.rotation;
        p.motion = [self.tx, self.ty, self.sx.max(0.), self.sy.max(0.)];
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ease {
    Linear,
    /// Fast, then settling.
    Out,
    /// Slow, then fast.
    In,
    InOut,
    /// Out, past the mark and back.
    Back,
}

impl Ease {
    fn at(self, t: f32) -> f32 {
        match self {
            Ease::Linear => t,
            Ease::Out => 1. - (1. - t).powi(3),
            Ease::In => t * t * t,
            Ease::InOut => {
                if t < 0.5 {
                    4. * t * t * t
                } else {
                    1. - (-2. * t + 2.).powi(3) / 2.
                }
            }
            Ease::Back => {
                let s = 1.6;
                let u = t - 1.;
                1. + (s + 1.) * u * u * u + s * u * u
            }
        }
    }
}

/// A waypoint: the pose at `t` (0–1 through the window), reached from the
/// one before by `ease`.
#[derive(Clone, Copy, Debug)]
pub struct Stage {
    pub t: f32,
    pub pose: Pose,
    pub ease: Ease,
}

const fn at(t: f32, pose: Pose, ease: Ease) -> Stage {
    Stage { t, pose, ease }
}

/// The pose of a waypoint list at `p` (0–1).
pub fn pose_at(stages: &[Stage], p: f32) -> Pose {
    let Some(first) = stages.first() else {
        return REST;
    };
    if p <= first.t {
        return first.pose;
    }
    for w in stages.windows(2) {
        let (a, b) = (w[0], w[1]);
        if p <= b.t {
            let span = (b.t - a.t).max(1e-6);
            let k = b.ease.at(((p - a.t) / span).clamp(0., 1.));
            return Pose::lerp(a.pose, b.pose, k);
        }
    }
    stages[stages.len() - 1].pose
}

#[derive(Clone, Debug)]
pub enum Motion {
    /// Each clip moves by its waypoints (none: it holds still).
    Posed { out: Vec<Stage>, inn: Vec<Stage> },
    /// The incoming clip is revealed by this shader (a reveal's id).
    Reveal(String),
}

#[derive(Clone, Debug)]
pub struct Preset {
    pub id: String,
    pub label: String,
    pub categories: Vec<&'static str>,
    pub default_micros: Micros,
    pub motion: Motion,
    /// The tile's animation (an asset-host key), if there is one.
    pub preview: Option<String>,
}

fn pose(f: impl FnOnce(&mut Pose)) -> Pose {
    let mut p = REST;
    f(&mut p);
    p
}

fn posed(
    id: &str,
    label: &str,
    categories: &[&'static str],
    micros: Micros,
    out: Vec<Stage>,
    inn: Vec<Stage>,
) -> Preset {
    Preset {
        id: id.into(),
        label: label.into(),
        categories: categories.to_vec(),
        default_micros: micros,
        motion: Motion::Posed { out, inn },
        preview: None,
    }
}

/// Both pictures travel one frame along (dx, dy): the old one out, the
/// new one in behind it.
fn push(id: &str, label: &str, dx: f32, dy: f32) -> Preset {
    posed(
        id,
        label,
        &["slide"],
        DEFAULT_MICROS,
        vec![
            at(0., REST, Ease::Linear),
            at(1., pose(|p| (p.tx, p.ty) = (-dx, -dy)), Ease::InOut),
        ],
        vec![
            at(0., pose(|p| (p.tx, p.ty) = (dx, dy)), Ease::Linear),
            at(1., REST, Ease::InOut),
        ],
    )
}

/// The new picture slides in over the old one, which stays put.
fn cover(id: &str, label: &str, dx: f32, dy: f32) -> Preset {
    posed(
        id,
        label,
        &["slide"],
        DEFAULT_MICROS,
        vec![],
        vec![
            at(0., pose(|p| (p.tx, p.ty) = (dx, dy)), Ease::Linear),
            at(1., REST, Ease::Out),
        ],
    )
}

fn posed_presets() -> Vec<Preset> {
    use Ease::*;
    let gone = |f: fn(&mut Pose)| {
        pose(|p| {
            p.opacity = 0.;
            f(p)
        })
    };
    vec![
        // The old picture fades only in the second half, under the new one
        // fading in: a full frame never dips toward the background.
        posed(
            "mix",
            "Mix",
            &["basic"],
            DEFAULT_MICROS,
            vec![
                at(0., REST, Linear),
                at(0.5, REST, Linear),
                at(1., gone(|_| {}), Linear),
            ],
            vec![at(0., gone(|_| {}), Linear), at(1., REST, Linear)],
        ),
        posed(
            "black-fade",
            "Black Fade",
            &["basic"],
            800_000,
            vec![at(0., REST, Linear), at(0.5, gone(|_| {}), InOut)],
            vec![
                at(0., gone(|_| {}), Linear),
                at(0.5, gone(|_| {}), Linear),
                at(1., REST, InOut),
            ],
        ),
        posed(
            "pull-in",
            "Pull In",
            &["basic", "zoom"],
            DEFAULT_MICROS,
            vec![at(0., REST, Linear), at(1., gone(|p| p.scale = 1.5), In)],
            vec![at(0., gone(|p| p.scale = 0.65), Linear), at(1., REST, Out)],
        ),
        posed(
            "pull-out",
            "Pull Out",
            &["basic", "zoom"],
            DEFAULT_MICROS,
            vec![at(0., REST, Linear), at(1., gone(|p| p.scale = 0.65), In)],
            vec![at(0., gone(|p| p.scale = 1.5), Linear), at(1., REST, Out)],
        ),
        posed(
            "zoom-in",
            "Zoom In",
            &["zoom"],
            DEFAULT_MICROS,
            vec![at(0., REST, Linear), at(0.6, gone(|p| p.scale = 2.4), In)],
            vec![
                at(0., gone(|p| p.scale = 2.), Linear),
                at(0.4, gone(|p| p.scale = 2.), Linear),
                at(1., REST, Out),
            ],
        ),
        posed(
            "zoom-out",
            "Zoom Out",
            &["zoom"],
            DEFAULT_MICROS,
            vec![at(0., REST, Linear), at(0.6, gone(|p| p.scale = 0.3), In)],
            vec![
                at(0., gone(|p| p.scale = 0.3), Linear),
                at(0.4, gone(|p| p.scale = 0.3), Linear),
                at(1., REST, Out),
            ],
        ),
        // Turned edge-on, then the new picture turns back face-on.
        posed(
            "flip",
            "Flip",
            &["basic"],
            DEFAULT_MICROS,
            vec![at(0., REST, Linear), at(0.5, pose(|p| p.sx = 0.), In)],
            vec![
                at(0., pose(|p| p.sx = 0.), Linear),
                at(0.5, pose(|p| p.sx = 0.), Linear),
                at(1., REST, Out),
            ],
        ),
        push("slide-left", "Slide Left", 1., 0.),
        push("slide-right", "Slide Right", -1., 0.),
        push("slide-up", "Slide Up", 0., 1.),
        push("slide-down", "Slide Down", 0., -1.),
        cover("cover-left", "Cover Left", 1., 0.),
        cover("cover-right", "Cover Right", -1., 0.),
        cover("cover-up", "Cover Up", 0., 1.),
        cover("cover-down", "Cover Down", 0., -1.),
        posed(
            "spin",
            "Spin",
            &["spin"],
            700_000,
            vec![
                at(0., REST, Linear),
                at(
                    0.5,
                    gone(|p| {
                        p.rotation = 180.;
                        p.scale = 0.;
                    }),
                    In,
                ),
            ],
            vec![
                at(
                    0.,
                    gone(|p| {
                        p.rotation = -180.;
                        p.scale = 0.;
                    }),
                    Linear,
                ),
                at(
                    0.5,
                    gone(|p| {
                        p.rotation = -180.;
                        p.scale = 0.;
                    }),
                    Linear,
                ),
                at(1., REST, Out),
            ],
        ),
        posed(
            "rotate",
            "Rotate",
            &["spin"],
            DEFAULT_MICROS,
            vec![at(0., REST, Linear), at(1., gone(|p| p.rotation = 90.), In)],
            vec![
                at(0., gone(|p| p.rotation = -90.), Linear),
                at(1., REST, Out),
            ],
        ),
        posed(
            "swing",
            "Swing",
            &["spin", "slide"],
            DEFAULT_MICROS,
            vec![
                at(0., REST, Linear),
                at(
                    1.,
                    gone(|p| {
                        p.rotation = 25.;
                        p.tx = -0.6;
                    }),
                    In,
                ),
            ],
            vec![
                at(
                    0.,
                    gone(|p| {
                        p.rotation = -25.;
                        p.tx = 0.6;
                    }),
                    Linear,
                ),
                at(1., REST, Back),
            ],
        ),
    ]
}

/// The reveal shaders compiled in from the app checkout (`[]` without one).
#[derive(serde::Deserialize)]
struct RevealRow {
    id: String,
    label: String,
    cats: Vec<String>,
    #[serde(rename = "durationMicros")]
    duration_micros: Micros,
}

static MANIFEST: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!(concat!(
        env!("OUT_DIR"),
        "/package_manifest.json"
    )))
    .unwrap_or(Value::Null)
});

static PREVIEWS: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!(concat!(
        env!("OUT_DIR"),
        "/reveal_previews.json"
    )))
    .unwrap_or(Value::Null)
});

fn reveal_presets() -> Vec<Preset> {
    let rows: Vec<RevealRow> =
        serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/reveals.json")))
            .unwrap_or_default();
    rows.into_iter()
        .filter_map(|r| {
            let categories: Vec<&'static str> = REVEAL_CATEGORIES
                .iter()
                .copied()
                .filter(|c| r.cats.iter().any(|x| x == c))
                .collect();
            if categories.is_empty() {
                return None;
            }
            let preview = PREVIEWS
                .get("animations")
                .and_then(|a| a.get(&r.id))
                .and_then(|e| e.get("gif"))
                .and_then(Value::as_str)
                .map(str::to_string);
            Some(Preset {
                id: format!("fx-{}", r.id.strip_prefix("in-").unwrap_or(&r.id)),
                label: r.label,
                categories,
                default_micros: r.duration_micros.clamp(MIN_MICROS, REVEAL_DEFAULT_CAP),
                motion: Motion::Reveal(r.id),
                preview,
            })
        })
        .collect()
}

pub static PRESETS: LazyLock<Vec<Preset>> = LazyLock::new(|| {
    let mut all = posed_presets();
    all.extend(reveal_presets());
    all
});

static BY_ID: LazyLock<HashMap<&'static str, usize>> = LazyLock::new(|| {
    PRESETS
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.as_str(), i))
        .collect()
});

/// The first tab: a mix of the most used, posed and revealed.
const TRENDING_IDS: &[&str] = &[
    "mix",
    "black-fade",
    "pull-in",
    "slide-left",
    "fx-circular-wipe",
    "zoom-in",
    "fx-flash-in",
    "fx-glitch-displacement",
    "spin",
    "fx-diagonal-brush",
    "flip",
    "fx-ink-spill",
    "cover-up",
    "fx-page-unfurled",
    "pull-out",
];

/// The preset a stored type names, if this build has it.
pub fn find(id: &str) -> Option<&'static Preset> {
    BY_ID.get(id).map(|&i| &PRESETS[i])
}

/// What a stored type plays: its preset, else Mix.
pub fn get(id: &str) -> &'static Preset {
    find(id).unwrap_or(&PRESETS[0])
}

/// A tab's presets, in order.
pub fn in_category(key: &str) -> Vec<&'static Preset> {
    if key == TRENDING {
        return TRENDING_IDS.iter().filter_map(|id| find(id)).collect();
    }
    PRESETS
        .iter()
        .filter(|p| p.categories.contains(&key))
        .collect()
}

/// A reveal's package key (the package manifest's `animations` section).
pub fn reveal_package_key(shader: &str) -> Option<&'static str> {
    MANIFEST.get("animations")?.get(shader)?.as_str()
}

/// What a clip's `outTransition` holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    pub id: String,
    pub kind: String,
    pub micros: Micros,
}

pub fn of(clip: &Clip) -> Option<Transition> {
    let t = clip.extra.get("outTransition")?;
    let micros = t.get("durationMicros").and_then(Value::as_i64)?;
    (micros > 0).then(|| Transition {
        id: t
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        kind: t
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("mix")
            .to_string(),
        micros,
    })
}

/// The longest transition a seam takes: half of either clip.
pub fn max_micros(outgoing: &Clip, incoming: &Clip) -> Micros {
    MIN_MICROS.max(
        MAX_MICROS
            .min(outgoing.timeline_duration / 2)
            .min(incoming.timeline_duration / 2),
    )
}

/// The seams of the main lane in time order: (outgoing, incoming).
pub fn seams(project: &Project) -> Vec<(&Clip, &Clip)> {
    let clips = project.ordered_main_clips();
    clips.windows(2).map(|w| (w[0], w[1])).collect()
}

/// The seam whose cut is nearest `t` (its outgoing clip's id).
pub fn nearest_seam(project: &Project, t: Micros) -> Option<String> {
    seams(project)
        .into_iter()
        .min_by_key(|(o, _)| (o.end() - t).abs())
        .map(|(o, _)| o.id.clone())
}

/// Sets (or with None clears) the transition at the end of main clip
/// `clip_id`, at most what its seam takes, and moves every later main clip
/// by the change in overlap. False when it isn't a seam's outgoing clip.
pub fn set(project: &mut Project, clip_id: &str, to: Option<(&str, Micros)>) -> bool {
    let (max, old, keep_id) = {
        let Some((o, i)) = seams(project).into_iter().find(|(o, _)| o.id == clip_id) else {
            return false;
        };
        let old = of(o);
        (
            max_micros(o, i),
            old.as_ref().map_or(0, |t| t.micros),
            old.map(|t| t.id),
        )
    };
    let new = to.map_or(0, |(_, m)| m.clamp(MIN_MICROS, max));
    let Some(main) = project
        .tracks
        .iter_mut()
        .find(|t| t.kind == TrackType::Video)
    else {
        return false;
    };
    main.clips.sort_by_key(|c| c.timeline_start);
    let Some(ix) = main.clips.iter().position(|c| c.id == clip_id) else {
        return false;
    };
    match to {
        Some((kind, _)) => {
            let id = keep_id
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| new_id("transition"));
            main.clips[ix].extra.insert(
                "outTransition".into(),
                json!({ "id": id, "type": kind, "durationMicros": new }),
            );
        }
        None => {
            main.clips[ix].extra.remove("outTransition");
        }
    }
    let delta = old - new;
    for c in main.clips.iter_mut().skip(ix + 1) {
        c.timeline_start = (c.timeline_start + delta).max(0);
    }
    project.recompute_duration();
    true
}

/// `kind` on every seam at `micros` (each clamped to what it takes).
pub fn set_all(project: &mut Project, kind: &str, micros: Micros) -> bool {
    let ids: Vec<String> = seams(project).iter().map(|(o, _)| o.id.clone()).collect();
    let mut any = false;
    for id in ids {
        any |= set(project, &id, Some((kind, micros)));
    }
    any
}

/// How a clip shows at a time inside a transition window.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipMotion {
    pub pose: Pose,
    /// A reveal: its shader id, progress 0–1, window seconds.
    pub reveal: Option<(String, f32, f32)>,
}

/// `clip`'s motion at timeline time `t` from the transitions at its ends
/// (main-lane clips only); None outside every window.
pub fn motion_at(project: &Project, clip: &Clip, t: Micros) -> Option<ClipMotion> {
    let main = project.main_track()?;
    if !main.clips.iter().any(|c| c.id == clip.id) {
        return None;
    }
    let clips = project.ordered_main_clips();
    let ix = clips.iter().position(|c| c.id == clip.id)?;
    let mut out = ClipMotion {
        pose: REST,
        reveal: None,
    };
    let mut any = false;
    // Into the next clip: its out motion over the window at its end.
    if let (Some(tr), Some(next)) = (of(clip), clips.get(ix + 1)) {
        let overlap = clip.end() - next.timeline_start;
        if overlap > 0 && t >= next.timeline_start && t < clip.end() {
            let p = (t - next.timeline_start) as f32 / overlap as f32;
            if let Motion::Posed { out: stages, .. } = &get(&tr.kind).motion {
                out.pose = out.pose.then(pose_at(stages, p));
            }
            any = true;
        }
    }
    // From the clip before: its in motion over the window at its start.
    if let Some(prev) = ix.checked_sub(1).map(|i| clips[i])
        && let Some(tr) = of(prev)
    {
        let overlap = prev.end() - clip.timeline_start;
        if overlap > 0 && t >= clip.timeline_start && t < prev.end() {
            let p = (t - clip.timeline_start) as f32 / overlap as f32;
            match &get(&tr.kind).motion {
                Motion::Posed { inn, .. } => out.pose = out.pose.then(pose_at(inn, p)),
                Motion::Reveal(shader) => {
                    out.reveal = Some((shader.clone(), p, overlap as f32 / SECOND as f32))
                }
            }
            any = true;
        }
    }
    any.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MediaItem;

    fn three_clips() -> (Project, Vec<String>) {
        let mut p = Project::empty();
        let item = MediaItem {
            path: "/tmp/a.mp4".into(),
            name: "a.mp4".into(),
            duration: 4 * SECOND,
            width: 1920,
            height: 1080,
            has_video: true,
            still: false,
        };
        let ids = (0..3).map(|_| p.insert_media(&item, None)).collect();
        (p, ids)
    }

    #[test]
    fn a_transition_overlaps_its_seam_and_moves_what_follows() {
        let (mut p, ids) = three_clips();
        assert!(set(&mut p, &ids[0], Some(("slide-left", SECOND))));
        let start = |p: &Project, i: usize| p.find_clip(&ids[i]).unwrap().1.timeline_start;
        assert_eq!(start(&p, 1), 3 * SECOND);
        assert_eq!(start(&p, 2), 7 * SECOND);
        // Longer than half a clip: clamped to it.
        assert!(set(&mut p, &ids[0], Some(("mix", 9 * SECOND))));
        assert_eq!(start(&p, 1), 2 * SECOND);
        let tr = of(p.find_clip(&ids[0]).unwrap().1).unwrap();
        assert_eq!((tr.kind.as_str(), tr.micros), ("mix", 2 * SECOND));
        // Cleared: back where they were.
        assert!(set(&mut p, &ids[0], None));
        assert_eq!((start(&p, 1), start(&p, 2)), (4 * SECOND, 8 * SECOND));
        // The last clip has no seam.
        assert!(!set(&mut p, &ids[2], Some(("mix", SECOND))));
        assert!(set_all(&mut p, "flip", SECOND));
        assert_eq!(start(&p, 2), 6 * SECOND);
    }

    #[test]
    fn both_clips_move_through_the_window() {
        let (mut p, ids) = three_clips();
        set(&mut p, &ids[0], Some(("slide-left", SECOND)));
        let a = p.find_clip(&ids[0]).unwrap().1.clone();
        let b = p.find_clip(&ids[1]).unwrap().1.clone();
        // Before the window: at rest.
        assert_eq!(motion_at(&p, &a, 2 * SECOND), None);
        // Half way: the old one half out to the left, the new one half in.
        let half = 3 * SECOND + SECOND / 2;
        let out = motion_at(&p, &a, half).unwrap().pose;
        let inn = motion_at(&p, &b, half).unwrap().pose;
        assert!((out.tx + 0.5).abs() < 1e-4, "{out:?}");
        assert!((inn.tx - 0.5).abs() < 1e-4, "{inn:?}");
        // After it: the new one at rest.
        assert_eq!(motion_at(&p, &b, 4 * SECOND + 1), None);
    }

    #[test]
    fn unknown_types_play_as_mix_and_tabs_list_presets() {
        assert_eq!(get("crossfade").id, "mix");
        assert!(in_category(TRENDING).iter().any(|p| p.id == "mix"));
        assert_eq!(in_category("slide").len(), 9);
        let mix = get("mix");
        let Motion::Posed { out, inn } = &mix.motion else {
            panic!("mix is posed");
        };
        assert_eq!(pose_at(out, 0.25).opacity, 1.);
        assert!((pose_at(inn, 0.5).opacity - 0.5).abs() < 1e-6);
    }
}

#[cfg(test)]
mod catalog_check {
    use super::*;

    /// Every compiled-in reveal can fetch its shader (none without a
    /// checkout), and every Trending pick is in this build.
    #[test]
    fn reveals_have_packages() {
        for p in PRESETS.iter() {
            if let Motion::Reveal(s) = &p.motion {
                assert!(reveal_package_key(s).is_some(), "{} has no package", p.id);
            }
        }
        let reveals = PRESETS
            .iter()
            .any(|p| matches!(p.motion, Motion::Reveal(_)));
        if reveals {
            assert_eq!(in_category(TRENDING).len(), TRENDING_IDS.len());
        }
    }
}
