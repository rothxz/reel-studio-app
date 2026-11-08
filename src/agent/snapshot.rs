//! What the model is told about the edit: a compact JSON view of the
//! timeline in SECONDS with short ids: "c1" for clips, "t1" for lanes,
//! "m1" for media-library files, "s1" for sounds a search found. Small
//! local models copy long generated ids badly, so each real id gets an
//! alias that stays the same for the whole conversation; a split's two
//! halves get fresh ones.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::catalog::{EFFECT_PREFIX, FILTER_PREFIX};
use crate::model::{Clip, MediaItem, Micros, Project, Track, TrackType};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Clip,
    Lane,
    Media,
    Sound,
}

impl Kind {
    fn prefix(self) -> &'static str {
        match self {
            Kind::Clip => "c",
            Kind::Lane => "t",
            Kind::Media => "m",
            Kind::Sound => "s",
        }
    }
}

#[derive(Default, Debug)]
pub struct Aliases {
    by_id: HashMap<(Kind, String), String>,
    by_alias: HashMap<String, String>,
    next: HashMap<Kind, usize>,
}

impl Aliases {
    pub fn alias(&mut self, kind: Kind, id: &str) -> String {
        if let Some(a) = self.by_id.get(&(kind, id.to_string())) {
            return a.clone();
        }
        let n = self.next.entry(kind).or_insert(1);
        let alias = format!("{}{}", kind.prefix(), n);
        *n += 1;
        self.by_id.insert((kind, id.to_string()), alias.clone());
        self.by_alias.insert(alias.clone(), id.to_string());
        alias
    }

    /// The real id behind "c3" ("C3", " c3 ", a bare 3 and a real id
    /// passed straight through work too).
    pub fn resolve(&self, kind: Kind, reference: &str) -> Option<String> {
        let key = reference.trim().trim_matches('"').to_lowercase();
        let key = if key.chars().all(|c| c.is_ascii_digit()) && !key.is_empty() {
            format!("{}{key}", kind.prefix())
        } else {
            key
        };
        if key.starts_with(kind.prefix())
            && let Some(id) = self.by_alias.get(&key)
        {
            return Some(id.clone());
        }
        let raw = reference.trim();
        self.by_id
            .contains_key(&(kind, raw.to_string()))
            .then(|| raw.to_string())
    }
}

/// Seconds, 2 decimals: what the model reads and writes.
pub fn secs(t: Micros) -> f64 {
    (t as f64 / 10_000.).round() / 100.
}

pub fn micros(seconds: f64) -> Micros {
    (seconds * 1_000_000.).round() as Micros
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Main,
    Overlay,
    Stickers,
    Audio,
    Text,
    Effect,
}

impl Lane {
    pub const ALL: [Lane; 6] = [
        Lane::Main,
        Lane::Overlay,
        Lane::Stickers,
        Lane::Audio,
        Lane::Text,
        Lane::Effect,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Lane::Main => "main",
            Lane::Overlay => "overlay",
            Lane::Stickers => "stickers",
            Lane::Audio => "audio",
            Lane::Text => "text",
            Lane::Effect => "effect",
        }
    }
}

pub fn lane_of(project: &Project, track: &Track) -> Lane {
    match track.kind {
        TrackType::Video => Lane::Main,
        TrackType::Overlay if track.is_overlay_video(&project.id) => Lane::Overlay,
        TrackType::Overlay => Lane::Stickers,
        TrackType::Audio => Lane::Audio,
        TrackType::Text => Lane::Text,
        TrackType::Effect => Lane::Effect,
    }
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .chars()
        .take(60)
        .collect()
}

fn num(clip: &Clip, key: &str) -> Option<f64> {
    clip.extra.get(key).and_then(Value::as_f64)
}

fn round2(v: f64) -> f64 {
    (v * 100.).round() / 100.
}

fn clip_details(clip: &Clip, aliases: &mut Aliases, media: &[MediaItem]) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(src) = &clip.source
        && matches!(clip.kind.as_str(), "video" | "image" | "audio")
    {
        out.insert(
            "media".into(),
            match media.iter().find(|m| &m.path == src) {
                Some(m) => format!("{} {}", aliases.alias(Kind::Media, &m.path), m.name).into(),
                None => file_name(src).into(),
            },
        );
    }
    let effects = clip.effects();
    if let Some(f) = effects
        .iter()
        .find_map(|e| e.kind.strip_prefix(FILTER_PREFIX))
    {
        out.insert("filter".into(), f.into());
    }
    if clip.kind != "effect" {
        let fx: Vec<&str> = effects
            .iter()
            .filter_map(|e| e.kind.strip_prefix(EFFECT_PREFIX))
            .collect();
        if !fx.is_empty() {
            out.insert("effects".into(), json!(fx));
        }
    }
    if clip.has_adjust() {
        out.insert("adjusted".into(), true.into());
    }
    if (clip.speed - 1.).abs() > 1e-6 {
        out.insert("speed".into(), json!(round2(clip.speed)));
    }
    if clip.hidden {
        out.insert("hidden".into(), true.into());
    }
    if clip.locked() {
        out.insert("locked".into(), true.into());
    }
    if clip.out_transition() > 0 {
        out.insert("transitionOut".into(), json!(secs(clip.out_transition())));
    }
    if clip.is_visual() || matches!(clip.kind.as_str(), "text" | "sticker") {
        let t = clip.transform();
        if t.x != 0. || t.y != 0. || t.scale != 1. || t.rotation != 0. || t.opacity != 1. {
            // Positions as the tools take them: 0..1 across the frame.
            out.insert(
                "transform".into(),
                json!({
                    "x": round2(t.x + 0.5), "y": round2(t.y + 0.5),
                    "scale": round2(t.scale), "rotation": round2(t.rotation), "opacity": round2(t.opacity),
                }),
            );
        }
    }
    let keys = clip.keyframe_times(1_000);
    if !keys.is_empty() {
        out.insert(
            "keyframes".into(),
            json!({
                "properties": clip.animated_properties(),
                // Clip-local seconds (0 = the clip's start).
                "at": keys.iter().map(|&t| secs(t)).collect::<Vec<_>>(),
            }),
        );
    }
    match clip.kind.as_str() {
        "video" => {
            out.insert("volume".into(), json!((clip.volume() * 100.).round()));
            if clip.muted() {
                out.insert("muted".into(), true.into());
            }
            if clip.reversed() {
                out.insert("reversed".into(), true.into());
            }
            if clip.freeze() {
                out.insert("freeze".into(), true.into());
            }
            out.insert(
                "sourceIn".into(),
                json!(secs(clip.source_start.unwrap_or(0))),
            );
            if clip.blend_mode() != "normal" {
                out.insert("blend".into(), clip.blend_mode().into());
            }
        }
        "audio" => {
            if let Some(t) = clip.extra.get("title").and_then(Value::as_str) {
                out.insert("title".into(), t.into());
            }
            out.insert("volume".into(), json!((clip.volume() * 100.).round()));
            if clip.muted() {
                out.insert("muted".into(), true.into());
            }
            let (fade_in, fade_out) = clip.audio_fade();
            if fade_in > 0 {
                out.insert("fadeIn".into(), json!(secs(fade_in)));
            }
            if fade_out > 0 {
                out.insert("fadeOut".into(), json!(secs(fade_out)));
            }
        }
        "text" => {
            out.insert("text".into(), clip.text.clone().unwrap_or_default().into());
            if let Some(c) = clip.extra.get("color").and_then(Value::as_str) {
                out.insert("color".into(), c.into());
            }
            if let Some(s) = num(clip, "fontSize") {
                out.insert("fontSize".into(), json!(s));
            }
            if clip.is_caption() {
                out.insert("caption".into(), true.into());
            }
        }
        "effect" => {
            if let Some(e) = effects.first() {
                out.insert("effect".into(), e.name().into());
            }
        }
        "sticker" => {
            if let Some(t) = clip.extra.get("title").and_then(Value::as_str) {
                out.insert("title".into(), t.into());
            }
        }
        _ => {}
    }
    out
}

pub struct SnapshotContext<'a> {
    pub playhead: Micros,
    pub selected: Option<&'a str>,
    pub media: &'a [MediaItem],
}

/// One clip as the snapshot lists it.
pub fn clip_entry(clip: &Clip, aliases: &mut Aliases, media: &[MediaItem]) -> Value {
    let mut v = Map::new();
    v.insert("id".into(), aliases.alias(Kind::Clip, &clip.id).into());
    v.insert("kind".into(), clip.kind.clone().into());
    v.insert("start".into(), json!(secs(clip.timeline_start)));
    v.insert("end".into(), json!(secs(clip.end())));
    v.extend(clip_details(clip, aliases, media));
    Value::Object(v)
}

/// The project as the model sees it (also `get_project`'s result).
pub fn snapshot(project: &Project, aliases: &mut Aliases, ctx: &SnapshotContext) -> Value {
    let mut tracks: Vec<&Track> = project
        .tracks
        .iter()
        .filter(|t| {
            !t.clips.is_empty()
                || matches!(
                    t.kind,
                    TrackType::Video | TrackType::Text | TrackType::Audio
                )
                || t.is_overlay_video(&project.id)
        })
        .collect();
    tracks.sort_by(|a, b| a.order.total_cmp(&b.order));
    let lanes: Vec<Value> = tracks
        .iter()
        .map(|track| {
            let mut clips: Vec<&Clip> = track.clips.iter().collect();
            clips.sort_by_key(|c| c.timeline_start);
            let mut lane = json!({
                "id": aliases.alias(Kind::Lane, &track.id),
                "lane": lane_of(project, track).name(),
            });
            for (flag, on) in [
                ("muted", track.muted),
                ("hidden", track.hidden),
                ("locked", track.locked),
            ] {
                if on {
                    lane[flag] = true.into();
                }
            }
            lane["clips"] = clips
                .iter()
                .map(|c| clip_entry(c, aliases, ctx.media))
                .collect();
            lane
        })
        .collect();
    let mut out = json!({
        "name": project.name,
        "canvas": format!("{}x{} @{}fps", project.canvas.width, project.canvas.height, project.canvas.fps),
        "duration": secs(project.duration),
        "playhead": secs(ctx.playhead),
        "selected": ctx.selected.map(|id| aliases.alias(Kind::Clip, id)),
        "lanes": lanes,
    });
    if !ctx.media.is_empty() {
        out["media"] = ctx
            .media
            .iter()
            .map(|m| {
                let kind = if !m.has_video {
                    "audio"
                } else if m.still {
                    "photo"
                } else {
                    "video"
                };
                let mut v = json!({
                    "id": aliases.alias(Kind::Media, &m.path),
                    "name": m.name,
                    "type": kind,
                });
                if !m.still && m.duration > 0 {
                    v["duration"] = json!(secs(m.duration));
                }
                v
            })
            .collect();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_are_stable_and_forgiving() {
        let mut a = Aliases::default();
        assert_eq!(a.alias(Kind::Clip, "clip-xyz"), "c1");
        assert_eq!(a.alias(Kind::Clip, "clip-abc"), "c2");
        assert_eq!(a.alias(Kind::Clip, "clip-xyz"), "c1");
        assert_eq!(a.alias(Kind::Lane, "track-1"), "t1");
        assert_eq!(a.resolve(Kind::Clip, " C2 ").as_deref(), Some("clip-abc"));
        assert_eq!(a.resolve(Kind::Clip, "1").as_deref(), Some("clip-xyz"));
        assert_eq!(
            a.resolve(Kind::Clip, "clip-xyz").as_deref(),
            Some("clip-xyz")
        );
        assert_eq!(a.resolve(Kind::Clip, "t1"), None);
        assert_eq!(a.resolve(Kind::Lane, "t1").as_deref(), Some("track-1"));
    }

    #[test]
    fn snapshot_lists_lanes_in_seconds() {
        let mut p = Project::empty();
        let id = p.add_text_clip(1_500_000, "Hello");
        p.recompute_duration();
        let mut a = Aliases::default();
        let s = snapshot(
            &p,
            &mut a,
            &SnapshotContext {
                playhead: 0,
                selected: Some(&id),
                media: &[],
            },
        );
        assert_eq!(s["selected"], "c1");
        let text = s["lanes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["lane"] == "text")
            .unwrap();
        assert_eq!(text["clips"][0]["start"], 1.5);
        assert_eq!(text["clips"][0]["end"], 4.5);
        assert_eq!(text["clips"][0]["text"], "Hello");
    }
}
