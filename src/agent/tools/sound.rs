//! Sound effects: find real recordings (`find_sounds`), place one
//! (`add_sound`), or sound-design the whole edit in one call
//! (`sound_design`).
//!
//! Sound design reads the moments off the timeline: cuts and transitions
//! on the main lane, titles and text landing, stickers popping in, glitch
//! and shake effects, typewriter runs, and (in a long clip with no cuts
//! of its own) scene changes found in the picture. Each moment kind has a
//! recipe per style: what to search for, how long the sound may run, which
//! part of it lands on the moment (a whoosh's peak on the cut, a hit's
//! attack on the title, a riser's end on the reveal) and how loud it sits.
//! Same-kind moments rotate through a few recordings so a reel of cuts
//! isn't one sound on repeat, and each pass picks afresh: every kind has
//! several ways to search for it, the candidates are shuffled (a seed per
//! video) and sounds used in recent videos are passed over, so two videos
//! don't sound alike. Motion-graphic scenes say where their own animation
//! lands (`agentBeats` on the clip: a node lighting up, code typing), and
//! those beats get sounds too. Everything goes on its own "SFX" audio lane,
//! each clip tagged so a second pass replaces the first.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

use super::{Args, Env, Finish, Outcome, Progress, Tier, Tool, ToolError, ToolResult, schema};
use crate::agent::dice::Dice;
use crate::agent::sfx::{self, Shape, SoundHit};
use crate::agent::snapshot::{Kind, micros, secs};
use crate::model::{Clip, Micros, Project, SECOND, Track, TrackType};

/// Sounds found this conversation, by key ("ov:…", "lib:…").
#[derive(Default)]
pub struct SoundShelf {
    hits: HashMap<String, SoundHit>,
}

impl SoundShelf {
    pub fn put(&mut self, hit: SoundHit) {
        self.hits.insert(hit.key.clone(), hit);
    }
    pub fn get(&self, key: &str) -> Option<&SoundHit> {
        self.hits.get(key)
    }
}

pub const STYLES: &[(&str, &str)] = &[
    (
        "subtle",
        "soft whooshes and light accents, sits under voice or music",
    ),
    (
        "punchy",
        "fast whooshes and hits, social-video energy (default)",
    ),
    ("cinematic", "deep whooshes, booms and risers, trailer feel"),
    ("playful", "pops, boings and bubbly cartoon accents"),
    (
        "ui",
        "clicks, swipes and dings, for product and screen videos",
    ),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Cue {
    Cut,
    Transition,
    Title,
    Text,
    Typing,
    Sticker,
    Glitch,
    Impact,
    Flash,
    Riser,
}

impl Cue {
    /// A scene's beat ("text", "cut", "typing", "impact", "riser").
    fn from_beat(name: &str) -> Option<Cue> {
        Some(match name {
            "text" | "pop" | "click" => Cue::Text,
            "cut" | "whoosh" => Cue::Cut,
            "typing" => Cue::Typing,
            "impact" | "hit" => Cue::Impact,
            "riser" => Cue::Riser,
            "title" => Cue::Title,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Cue::Cut => "cut",
            Cue::Transition => "transition",
            Cue::Title => "title",
            Cue::Text => "text",
            Cue::Typing => "typing",
            Cue::Sticker => "sticker",
            Cue::Glitch => "glitch",
            Cue::Impact => "impact",
            Cue::Flash => "flash",
            Cue::Riser => "riser",
        }
    }
}

/// Which part of the sound lands on the moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// Its attack (a hit, a pop, a click).
    Start,
    /// Its loudest point (a whoosh past a cut).
    Peak,
    /// Where it dies away (a riser into a reveal).
    End,
}

impl Anchor {
    fn parse(s: &str) -> ToolResult<Anchor> {
        match s.to_lowercase().as_str() {
            "start" | "onset" | "attack" => Ok(Anchor::Start),
            "peak" | "hit" | "middle" => Ok(Anchor::Peak),
            "end" => Ok(Anchor::End),
            _ => Err("anchor is start, peak or end".into()),
        }
    }
}

struct Recipe {
    /// Ways to search for it (one or two are used per pass).
    queries: &'static [&'static str],
    /// Longest it may run, seconds.
    max: f64,
    anchor: Anchor,
    /// Linear clip volume.
    volume: f64,
    /// Which wins when two moments crowd each other.
    priority: u8,
}

fn recipe(cue: Cue, style: &str) -> Recipe {
    let r = |queries, max, anchor, volume, priority| Recipe {
        queries,
        max,
        anchor,
        volume,
        priority,
    };
    use Anchor::*;
    match (cue, style) {
        (Cue::Cut, "subtle") => r(
            &["soft whoosh", "air whoosh", "gentle swoosh", "light swish"],
            0.7,
            Peak,
            0.35,
            4,
        ),
        (Cue::Cut, "cinematic") => r(
            &[
                "deep whoosh",
                "cinematic swoosh",
                "low whoosh",
                "heavy whoosh",
            ],
            1.0,
            Peak,
            0.55,
            4,
        ),
        (Cue::Cut, "playful") => r(
            &["swoosh", "cartoon swish", "quick swish", "zip"],
            0.6,
            Peak,
            0.45,
            4,
        ),
        (Cue::Cut, "ui") => r(
            &["swipe", "ui swipe", "slide whoosh", "interface swoosh"],
            0.5,
            Peak,
            0.4,
            4,
        ),
        (Cue::Cut, _) => r(
            &[
                "fast whoosh",
                "quick whoosh",
                "swish",
                "whip whoosh",
                "swoosh",
            ],
            0.7,
            Peak,
            0.5,
            4,
        ),
        (Cue::Transition, "cinematic") => r(
            &[
                "cinematic whoosh",
                "cinematic transition",
                "deep swoosh",
                "trailer whoosh",
            ],
            1.6,
            Peak,
            0.6,
            5,
        ),
        (Cue::Transition, "subtle") => r(
            &["soft whoosh", "smooth transition", "airy whoosh"],
            1.2,
            Peak,
            0.4,
            5,
        ),
        (Cue::Transition, _) => r(
            &["whoosh transition", "swoosh", "transition whoosh", "swish"],
            1.4,
            Peak,
            0.5,
            5,
        ),
        (Cue::Title, "subtle") => r(
            &[
                "soft chime",
                "gentle bell",
                "light chime",
                "soft notification",
            ],
            1.5,
            Start,
            0.35,
            6,
        ),
        (Cue::Title, "cinematic") => r(
            &[
                "cinematic boom",
                "cinematic hit",
                "deep impact",
                "trailer boom",
            ],
            2.5,
            Start,
            0.6,
            6,
        ),
        (Cue::Title, "playful") => r(
            &["bubble pop", "cartoon pop", "pop", "boing"],
            0.6,
            Start,
            0.5,
            6,
        ),
        (Cue::Title, "ui") => r(
            &[
                "notification ding",
                "ui confirm",
                "success chime",
                "digital ding",
            ],
            1.0,
            Start,
            0.4,
            6,
        ),
        (Cue::Title, _) => r(
            &["impact hit", "punch hit", "bass hit", "hit"],
            1.2,
            Start,
            0.55,
            6,
        ),
        (Cue::Text, "ui") => r(
            &["click", "ui click", "mouse click", "button click", "tap"],
            0.25,
            Start,
            0.35,
            2,
        ),
        (Cue::Text, "playful") => r(
            &["pop", "bubble pop", "plop", "cork pop"],
            0.4,
            Start,
            0.4,
            2,
        ),
        (Cue::Text, _) => r(
            &["soft pop", "pop", "soft click", "blip", "tick"],
            0.4,
            Start,
            0.35,
            2,
        ),
        (Cue::Typing, _) => r(
            &[
                "keyboard typing",
                "typing",
                "mechanical keyboard",
                "laptop typing",
            ],
            4.0,
            Start,
            0.3,
            3,
        ),
        (Cue::Sticker, "playful") => r(
            &["cartoon boing", "boing", "spring", "cartoon pop"],
            0.8,
            Start,
            0.45,
            3,
        ),
        (Cue::Sticker, _) => r(
            &["pop", "bubble pop", "plop", "soft pop"],
            0.5,
            Start,
            0.4,
            3,
        ),
        (Cue::Glitch, _) => r(
            &[
                "glitch",
                "digital glitch",
                "static glitch",
                "electronic glitch",
            ],
            1.0,
            Start,
            0.45,
            5,
        ),
        (Cue::Impact, "subtle") => r(&["thud", "soft thud", "low thump"], 0.8, Start, 0.4, 5),
        (Cue::Impact, _) => r(
            &["impact hit", "punch", "bass hit", "thump"],
            1.2,
            Start,
            0.55,
            5,
        ),
        (Cue::Flash, _) => r(
            &["camera shutter", "camera click", "flash", "shutter"],
            0.6,
            Start,
            0.45,
            5,
        ),
        (Cue::Riser, _) => r(
            &["riser", "tension riser", "uplifter", "swell"],
            3.0,
            End,
            0.45,
            3,
        ),
    }
}

/// One moment that wants a sound.
#[derive(Clone, Debug)]
struct Moment {
    at: Micros,
    cue: Cue,
    /// How long it lasts (typing), else 0.
    span: Micros,
}

fn is_title(c: &Clip) -> bool {
    let size = c
        .extra
        .get("fontSize")
        .and_then(Value::as_f64)
        .unwrap_or(24.);
    let bold = c
        .extra
        .get("bold")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    size >= 36.
        || (bold && c.extra.contains_key("outlineColor"))
        || c.extra.contains_key("glowColor")
}

/// The moments on the timeline between `from` and `to`.
fn moments(p: &Project, from: Micros, to: Micros, style: &str) -> Vec<Moment> {
    let mut out = vec![];
    let mut push = |at: Micros, cue: Cue, span: Micros| {
        if at >= from && at <= to {
            out.push(Moment { at, cue, span });
        }
    };
    // Cuts and transitions on the main lane.
    let main = p.ordered_main_clips();
    for pair in main.windows(2) {
        let overlap = pair[0].out_transition();
        if overlap > 0 {
            push(pair[1].timeline_start + overlap / 2, Cue::Transition, 0);
        } else {
            push(pair[1].timeline_start, Cue::Cut, 0);
        }
    }
    // A motion-graphic scene's own beats (where its animation lands), in
    // its source time.
    for c in &main {
        let Some(Value::Array(beats)) = c.extra.get("agentBeats") else {
            continue;
        };
        let s0 = c.source_start.unwrap_or(0);
        for b in beats {
            let (Some(t), Some(cue)) = (
                b.get(0).and_then(Value::as_f64),
                b.get(1).and_then(Value::as_str).and_then(Cue::from_beat),
            ) else {
                continue;
            };
            let span = b.get(2).and_then(Value::as_f64).map_or(0, micros);
            let at = c.timeline_start + ((micros(t) - s0) as f64 / c.speed.max(0.01)) as Micros;
            if at > c.timeline_start && at < c.end() - SECOND / 5 {
                push(at, cue, span);
            }
        }
    }
    for t in &p.tracks {
        // Overlay pictures arriving: a light whoosh.
        if t.is_overlay_video(&p.id) {
            for c in t.clips.iter().filter(|c| !c.hidden) {
                push(c.timeline_start, Cue::Cut, 0);
            }
        }
        match t.kind {
            TrackType::Overlay if !t.is_overlay_video(&p.id) => {
                for c in t.clips.iter().filter(|c| !c.hidden && c.kind == "sticker") {
                    push(c.timeline_start, Cue::Sticker, 0);
                }
            }
            TrackType::Effect => {
                for c in t.clips.iter().filter(|c| !c.hidden) {
                    let name = c
                        .effects()
                        .first()
                        .map(|e| e.name().to_lowercase())
                        .unwrap_or_default();
                    let cue = if ["glitch", "vhs", "tv", "static", "noise", "rgb"]
                        .iter()
                        .any(|w| name.contains(w))
                    {
                        Some(Cue::Glitch)
                    } else if ["shake", "quake", "impact", "punch", "zoom"]
                        .iter()
                        .any(|w| name.contains(w))
                    {
                        Some(Cue::Impact)
                    } else if ["flash", "flare", "light", "strobe"]
                        .iter()
                        .any(|w| name.contains(w))
                    {
                        Some(Cue::Flash)
                    } else {
                        None
                    };
                    if let Some(cue) = cue {
                        push(c.timeline_start, cue, 0);
                    }
                }
            }
            _ => {}
        }
    }
    // Text: typewriter runs (each clip the last one's words and more, back to
    // back) get typing; other text a pop, a title a hit.
    let mut texts: Vec<&Clip> = p
        .tracks
        .iter()
        .filter(|t| t.kind == TrackType::Text)
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.kind == "text" && !c.hidden && !c.is_caption())
        .collect();
    texts.sort_by_key(|c| c.timeline_start);
    let mut i = 0;
    let mut first_title: Option<Micros> = None;
    while i < texts.len() {
        let mut j = i;
        while j + 1 < texts.len()
            && (texts[j + 1].timeline_start - texts[j].end()).abs() < 50_000
            && texts[j + 1]
                .text
                .as_deref()
                .unwrap_or("")
                .starts_with(texts[j].text.as_deref().unwrap_or("\u{0}"))
        {
            j += 1;
        }
        if j - i >= 2 {
            let start = texts[i].timeline_start;
            push(start, Cue::Typing, texts[j].timeline_start - start);
            i = j + 1;
            continue;
        }
        let c = texts[i];
        if is_title(c) {
            first_title.get_or_insert(c.timeline_start);
            push(c.timeline_start, Cue::Title, 0);
        } else {
            push(c.timeline_start, Cue::Text, 0);
        }
        i += 1;
    }
    // A cinematic opening builds into its first title.
    if style == "cinematic"
        && let Some(t) = first_title
        && t >= 2 * SECOND
    {
        push(t, Cue::Riser, 0);
    }
    out.sort_by_key(|m| m.at);
    out
}

/// Keeps the moments that matter when they crowd: none closer than 0.3 s
/// (the higher priority wins), at most 4 in any 2 s.
fn thin(moments: Vec<Moment>, style: &str) -> Vec<Moment> {
    let mut kept: Vec<Moment> = vec![];
    for m in moments {
        let pr = recipe(m.cue, style).priority;
        if let Some(last) = kept.last()
            && m.at - last.at < 300_000
        {
            if pr > recipe(last.cue, style).priority {
                kept.pop();
            } else {
                continue;
            }
        }
        let window = kept.iter().filter(|k| m.at - k.at < 2 * SECOND).count();
        if window >= 4 {
            continue;
        }
        kept.push(m);
    }
    kept
}

/// The audible part of a sound to put on the timeline so its anchor lands
/// on `at`: (timeline start, source in, length, fade out). None: too short.
pub fn place(
    shape: &Shape,
    at: Micros,
    anchor: Anchor,
    max_len: Micros,
) -> Option<(Micros, Micros, Micros, Micros)> {
    let pre = 15_000;
    let anchor_src = match anchor {
        Anchor::Start => shape.onset,
        Anchor::Peak => shape.peak,
        Anchor::End => shape.tail,
    };
    let mut src_in = match anchor {
        Anchor::End => (shape.tail - max_len).max(shape.onset - pre).max(0),
        _ => (shape.onset - pre).max(0),
    };
    let src_out = (shape.tail + 60_000)
        .min(shape.length)
        .min(src_in + max_len);
    let mut start = at - (anchor_src - src_in);
    if start < 0 {
        src_in -= start;
        start = 0;
    }
    let len = src_out - src_in;
    if len < 50_000 {
        return None;
    }
    let cut_short = src_out < shape.tail;
    let fade = if cut_short { (len / 3).min(150_000) } else { 0 };
    Some((start, src_in, len, fade))
}

/// The project's SFX lane (made on first use).
fn sfx_lane(p: &mut Project) -> String {
    let id = format!("{}-sfx-1", p.id);
    if !p.tracks.iter().any(|t| t.id == id) {
        let order = p
            .tracks
            .iter()
            .filter(|t| t.kind == TrackType::Audio)
            .map(|t| t.order)
            .fold(1., f64::max)
            + 0.5;
        p.tracks.push(Track {
            id: id.clone(),
            kind: TrackType::Audio,
            name: "SFX".into(),
            clips: vec![],
            order,
            muted: false,
            hidden: false,
            locked: false,
        });
    }
    id
}

/// A placed sound effect.
#[derive(Clone, Debug)]
pub struct Placed {
    pub path: PathBuf,
    pub hit: SoundHit,
    pub shape: Shape,
    pub start: Micros,
    pub src_in: Micros,
    pub len: Micros,
    pub fade: Micros,
    pub volume: f64,
    pub tag: &'static str,
}

/// Puts a placed sound on the SFX lane; returns the clip id.
pub fn add_sfx_clip(p: &mut Project, s: &Placed) -> String {
    let lane = sfx_lane(p);
    let mut extra = Map::new();
    extra.insert("title".into(), s.hit.title.clone().into());
    extra.insert("volume".into(), json!(s.volume));
    extra.insert("muted".into(), false.into());
    extra.insert(
        "transform".into(),
        json!({ "x": 0, "y": 0, "scale": 1, "rotation": 0, "opacity": 1 }),
    );
    extra.insert("effects".into(), json!([]));
    if s.fade > 0 {
        extra.insert(
            "audioFade".into(),
            json!({ "inDurationMicros": 0, "outDurationMicros": s.fade }),
        );
    }
    extra.insert("agentSfx".into(), s.tag.into());
    if !s.hit.attribution.is_empty() {
        extra.insert("attribution".into(), s.hit.attribution.clone().into());
    }
    if !s.hit.license.is_empty() {
        extra.insert("license".into(), s.hit.license.clone().into());
    }
    let id = crate::model::new_id("clip");
    let clip = Clip {
        id: id.clone(),
        kind: "audio".into(),
        timeline_start: s.start,
        timeline_duration: s.len,
        speed: 1.0,
        source: Some(s.path.to_string_lossy().into_owned()),
        source_start: Some(s.src_in),
        source_duration: Some(s.shape.length),
        text: None,
        hidden: false,
        extra,
    };
    if let Some(t) = p.tracks.iter_mut().find(|t| t.id == lane) {
        t.clips.push(clip);
    }
    p.recompute_duration();
    id
}

/// Does anything else in the edit make sound (then effects sit lower)?
fn has_other_sound(p: &Project) -> bool {
    p.tracks.iter().filter(|t| !t.muted).any(|t| {
        t.clips.iter().any(|c| {
            !c.hidden
                && !c.muted()
                && !c.extra.contains_key("agentSfx")
                && (c.kind == "audio" || (c.kind == "video" && !c.freeze()))
        })
    })
}

fn credits(placed: &[Placed]) -> Vec<String> {
    let mut list: Vec<String> = placed
        .iter()
        .filter(|p| !p.hit.attribution.is_empty())
        .map(|p| p.hit.attribution.clone())
        .collect();
    list.sort();
    list.dedup();
    list
}

// ---- find_sounds / add_sound ------------------------------------------------------------

fn find_sounds(args: &Args, _env: &mut Env) -> ToolResult<Outcome> {
    let query = args.required_str("query")?;
    let max = args.num("max_duration")?.unwrap_or(4.).clamp(0.2, 60.);
    let count = args.num("count")?.unwrap_or(6.).clamp(1., 12.) as usize;
    Ok(Outcome::Later(Box::new(move |progress: &Progress| {
        progress.set(format!("Searching \"{query}\""));
        let hits = sfx::search(&query, max)
            .map_err(|e| ToolError(format!("Sound search failed: {e:#}")))?;
        let hits: Vec<SoundHit> = hits
            .into_iter()
            .filter(|h| h.duration.is_none_or(|d| d <= max * 1.5))
            .take(count)
            .collect();
        let finish: Finish = Box::new(move |env: &mut Env| {
            if hits.is_empty() {
                return Err(format!("No sounds found for \"{query}\": try simpler words (whoosh, pop, hit, click, riser)").into());
            }
            let list: Vec<Value> = hits
                .into_iter()
                .map(|h| {
                    let alias = env.aliases.alias(Kind::Sound, &h.key);
                    let v = h.summary(&alias);
                    env.sounds.put(h);
                    v
                })
                .collect();
            Ok(json!({ "sounds": list, "next": "add_sound with one of these ids" }))
        });
        Ok(finish)
    })))
}

fn add_sound(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let at = env.snap(args.time("time")?.unwrap_or(env.playhead));
    let anchor = args
        .str("anchor")
        .map(|a| Anchor::parse(&a))
        .transpose()?
        .unwrap_or(Anchor::Start);
    let volume = (args.num("volume")?.unwrap_or(60.) / 100.).clamp(0., 4.);
    let max_len = args.time("duration")?.unwrap_or(4 * SECOND).max(100_000);
    let reference = args.required_str("sound")?;
    // "s3" from find_sounds, else words to search for.
    let known = env
        .aliases
        .resolve(Kind::Sound, &reference)
        .and_then(|key| env.sounds.get(&key).cloned());
    let query = reference.clone();
    let end = env.project.duration;
    Ok(Outcome::Later(Box::new(move |progress: &Progress| {
        let candidates = match known {
            Some(hit) => vec![hit],
            None => {
                progress.set(format!("Searching \"{query}\""));
                sfx::search(&query, max_len as f64 / 1e6)
                    .map_err(|e| ToolError(format!("Sound search failed: {e:#}")))?
                    .into_iter()
                    .take(4)
                    .collect()
            }
        };
        let mut last_err = String::from("No sound found");
        for hit in candidates {
            progress.check()?;
            progress.set(format!("Downloading {}", hit.title));
            let path = match sfx::download(&hit) {
                Ok(p) => p,
                Err(e) => {
                    last_err = format!("{e:#}");
                    continue;
                }
            };
            let shape = match sfx::analyze(&path) {
                Ok(s) => s,
                Err(e) => {
                    last_err = format!("{e:#}");
                    continue;
                }
            };
            let Some((start, src_in, mut len, fade)) = place(&shape, at, anchor, max_len) else {
                continue;
            };
            if end > 0 && start < end {
                len = len.min(end + SECOND / 2 - start);
            }
            let placed = Placed {
                path,
                hit,
                shape,
                start,
                src_in,
                len,
                fade,
                volume,
                tag: "sound",
            };
            let finish: Finish = Box::new(move |env: &mut Env| {
                let id = add_sfx_clip(&mut env.project, &placed);
                env.edited("Add Sound");
                let alias = env.alias(&id);
                let mut v = json!({
                    "clip": alias,
                    "sound": placed.hit.title,
                    "start": secs(placed.start),
                    "end": secs(placed.start + placed.len),
                });
                if !placed.hit.attribution.is_empty() {
                    v["credit"] = placed.hit.attribution.clone().into();
                }
                Ok(v)
            });
            return Ok(finish);
        }
        Err(ToolError(format!("Couldn't use that sound: {last_err}")))
    })))
}

/// A downloaded recording and where it's audible.
type Found = (PathBuf, SoundHit, Shape);

/// Recordings found for each kind of moment, ready to place.
#[derive(Default)]
pub struct SoundPool {
    sounds: HashMap<Cue, Vec<Found>>,
    /// The kinds nothing was found for, and why.
    pub missing: Vec<String>,
}

/// Searches one or two of a kind's `queries` (a different pair each
/// seed), their hits merged, best first.
fn search_some(queries: &[&str], max: f64, dice: &mut Dice) -> anyhow::Result<Vec<SoundHit>> {
    let first = dice.below(queries.len());
    let mut picked = vec![queries[first]];
    if queries.len() > 1 {
        let second = (first + 1 + dice.below(queries.len() - 1)) % queries.len();
        picked.push(queries[second]);
    }
    let mut hits: Vec<SoundHit> = vec![];
    let mut last_err = None;
    for q in picked {
        match sfx::search(q, max) {
            Ok(h) => {
                for hit in h {
                    if !hits.iter().any(|x| x.key == hit.key) {
                        hits.push(hit);
                    }
                }
            }
            Err(e) => last_err = Some(e),
        }
    }
    match (hits.is_empty(), last_err) {
        (true, Some(e)) => Err(e),
        _ => Ok(hits),
    }
}

/// `take` candidates from ranked `hits`: from the best dozen, in a shuffled
/// order, sounds not used lately first.
fn vary(hits: Vec<SoundHit>, recent: &[String], take: usize, dice: &mut Dice) -> Vec<SoundHit> {
    let mut top: Vec<SoundHit> = hits.into_iter().take(12).collect();
    dice.shuffle(&mut top);
    top.sort_by_key(|h| recent.iter().any(|k| *k == h.key));
    top.truncate(take);
    top
}

/// A few recordings for each kind (`want` of each, so repeats rotate). The
/// kinds are searched at once and each kind's candidates downloaded at
/// once: servers are slow, and waiting on them one by one took most of a
/// minute.
fn fetch_pool(
    kinds: &[(Cue, usize)],
    style: &str,
    seed: u64,
    progress: &Progress,
) -> ToolResult<SoundPool> {
    let recent = sfx::recently_used();
    progress.set(format!(
        "Finding {} sounds",
        kinds
            .iter()
            .map(|(c, _)| c.name())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let rows: Vec<(Cue, Result<Vec<Found>, String>)> = std::thread::scope(|s| {
        let hs: Vec<_> = kinds
            .iter()
            .map(|&(cue, want)| {
                let recent = &recent;
                s.spawn(move || {
                    let r = recipe(cue, style);
                    let mut dice = Dice::new(Dice::mix(seed, cue as u64));
                    let hits = match search_some(r.queries, r.max, &mut dice) {
                        Ok(h) => h,
                        Err(e) => return (cue, Err(format!("{e:#}"))),
                    };
                    let usable: Vec<SoundHit> = hits
                        .into_iter()
                        // Typing is cut to its span: a long take is fine.
                        .filter(|h| {
                            let longest = if cue == Cue::Typing { 120. } else { r.max * 3. };
                            h.duration.is_none_or(|d| d <= longest && d >= 0.08)
                        })
                        .collect();
                    let candidates = vary(usable, recent, (want * 2).clamp(3, 6), &mut dice);
                    let got: Vec<Option<Found>> = std::thread::scope(|s2| {
                        let hs: Vec<_> = candidates
                            .into_iter()
                            .map(|hit| {
                                s2.spawn(move || {
                                    if progress.stopped() {
                                        return None;
                                    }
                                    let path = sfx::download(&hit).ok()?;
                                    let shape = sfx::analyze(&path).ok()?;
                                    Some((path, hit, shape))
                                })
                            })
                            .collect();
                        hs.into_iter().map(|h| h.join().ok().flatten()).collect()
                    });
                    // `vary`'s order: keep the first that worked.
                    let got: Vec<_> = got.into_iter().flatten().take(want.max(1)).collect();
                    (cue, Ok(got))
                })
            })
            .collect();
        hs.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    progress.check()?;
    let mut pool = SoundPool::default();
    for (cue, r) in rows {
        match r {
            Ok(got) if got.is_empty() => {
                pool.missing.push(format!("{}: nothing usable", cue.name()))
            }
            Ok(got) => {
                pool.sounds.insert(cue, got);
            }
            Err(e) => pool.missing.push(format!("{}: {e}", cue.name())),
        }
    }
    Ok(pool)
}

/// Each moment gets a sound of its kind from the pool, on its anchor.
fn place_found(
    found: &[Moment],
    pool: &SoundPool,
    end: Micros,
    style: &str,
    level: f64,
    louder: bool,
) -> Vec<Placed> {
    let mut placed = vec![];
    let mut turn: HashMap<Cue, usize> = HashMap::new();
    for m in found {
        let Some(sounds) = pool.sounds.get(&m.cue).filter(|s| !s.is_empty()) else {
            continue;
        };
        let n = turn.entry(m.cue).or_insert(0);
        let (path, hit, shape) = &sounds[*n % sounds.len()];
        *n += 1;
        let r = recipe(m.cue, style);
        let max = if m.cue == Cue::Typing {
            m.span.max(SECOND / 2)
        } else {
            micros(r.max)
        };
        let Some((start, src_in, mut len, fade)) = place(shape, m.at, r.anchor, max) else {
            continue;
        };
        len = len.min(end + SECOND / 3 - start);
        if len < 50_000 {
            continue;
        }
        let volume = r.volume * level * if louder { 1.25 } else { 1. };
        placed.push(Placed {
            path: path.clone(),
            hit: hit.clone(),
            shape: *shape,
            start,
            src_in,
            len,
            fade,
            volume: volume.min(2.),
            tag: m.cue.name(),
        });
    }
    placed
}

/// Sounds for the moments `found`, with the kinds nothing was found for.
fn gather(
    mut found: Vec<Moment>,
    end: Micros,
    style: &str,
    level: f64,
    louder: bool,
    progress: &Progress,
) -> ToolResult<(Vec<Placed>, Vec<String>)> {
    found.sort_by_key(|m| m.at);
    let found = thin(found, style);
    if found.is_empty() {
        return Err(ToolError(
            "No moments to sound here: no cuts, titles, stickers or effects in that range. Add some, or use add_sound at a time.".into(),
        ));
    }
    let mut kinds: Vec<(Cue, usize)> = vec![];
    for m in &found {
        match kinds.iter_mut().find(|(c, _)| *c == m.cue) {
            Some((_, n)) => *n = (*n + 1).min(3),
            None => kinds.push((m.cue, 1)),
        }
    }
    let pool = fetch_pool(&kinds, style, Dice::fresh_seed(), progress)?;
    let placed = place_found(&found, &pool, end, style, level, louder);
    sfx::mark_used(placed.iter().map(|p| p.hit.key.as_str()));
    if placed.is_empty() {
        return Err(ToolError(format!(
            "Couldn't get any sounds ({}). Check the internet connection.",
            pool.missing.join("; ")
        )));
    }
    Ok((placed, pool.missing))
}

/// The sounds a made video wants (`make_video`: cuts, titles, stickers;
/// with `beats`, its scenes' own: pops and typing), fetched before it's
/// laid out. Blocking.
pub fn prefetch_for_video(
    style: &str,
    beats: bool,
    seed: u64,
    progress: &Progress,
) -> ToolResult<SoundPool> {
    let mut kinds = vec![(Cue::Cut, 3), (Cue::Title, 3), (Cue::Sticker, 2)];
    if beats {
        kinds.extend([(Cue::Text, 3), (Cue::Typing, 1)]);
    }
    fetch_pool(&kinds, style, seed, progress)
}

/// Sound design for a whole project the agent laid out, from a prefetched
/// pool (no searching here).
pub fn design_from(project: &Project, style: &str, pool: &SoundPool) -> Vec<Placed> {
    let mut found = moments(project, 0, project.duration, style);
    found.sort_by_key(|m| m.at);
    let found = thin(found, style);
    let placed = place_found(
        &found,
        pool,
        project.duration,
        style,
        1.,
        !has_other_sound(project),
    );
    sfx::mark_used(placed.iter().map(|p| p.hit.key.as_str()));
    placed
}

// ---- sound_design ---------------------------------------------------------------------

fn sound_design(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let style = args
        .str("style")
        .unwrap_or_else(|| "punchy".into())
        .to_lowercase();
    if !STYLES.iter().any(|(s, _)| *s == style) {
        return Err(format!(
            "style is one of: {}",
            STYLES
                .iter()
                .map(|(s, _)| *s)
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into());
    }
    let from = args.time("start")?.unwrap_or(0);
    let to = args.time("end")?.unwrap_or(env.project.duration);
    let replace = args.bool("replace")?.unwrap_or(true);
    let scan = args.bool("scan_picture")?;
    let level = (args.num("volume")?.unwrap_or(100.) / 100.).clamp(0.1, 3.);
    if env.project.duration <= 0 {
        return Err("The timeline is empty: add a video first".into());
    }
    let project = env.project.clone();
    let mut found = moments(&project, from, to, &style);

    // Long main clips with no cuts of their own: look for scene changes.
    let main = project.ordered_main_clips();
    let scan = scan.unwrap_or(main.len() <= 2);
    let scans: Vec<(String, Micros, Micros, Micros, f64)> = if scan {
        main.iter()
            .filter(|c| {
                c.kind == "video"
                    && !c.freeze()
                    && !c.reversed()
                    && !c.extra.contains_key("speedCurve")
            })
            .filter_map(|c| {
                let src = c.source.clone()?;
                let s0 = c.source_start.unwrap_or(0);
                let s1 = s0 + (c.timeline_duration as f64 * c.speed) as Micros;
                Some((src, s0, s1, c.timeline_start, c.speed))
            })
            .collect()
    } else {
        vec![]
    };
    let louder = !has_other_sound(&project);
    let old: Vec<String> = if replace {
        project
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter(|c| {
                c.extra.contains_key("agentSfx")
                    && c.timeline_start >= from
                    && c.timeline_start <= to
            })
            .map(|c| c.id.clone())
            .collect()
    } else {
        vec![]
    };

    Ok(Outcome::Later(Box::new(move |progress: &Progress| {
        for (i, (src, s0, s1, t0, speed)) in scans.iter().enumerate() {
            progress.check()?;
            progress.set(format!(
                "Watching for scene changes {}/{}",
                i + 1,
                scans.len()
            ));
            if let Ok(cuts) = crate::decoder::scene_cuts(src, *s0, (*s1).min(*s0 + 600 * SECOND)) {
                for c in cuts {
                    let t = t0 + ((c - s0) as f64 / speed.max(0.01)) as Micros;
                    if t >= from && t <= to {
                        found.push(Moment {
                            at: t,
                            cue: Cue::Cut,
                            span: 0,
                        });
                    }
                }
            }
        }
        let (placed, failures) = gather(found, project.duration, &style, level, louder, progress)?;
        let finish: Finish = Box::new(move |env: &mut Env| {
            for id in &old {
                env.project.delete_clip(id);
            }
            let mut by_kind: HashMap<&str, usize> = HashMap::new();
            for p in &placed {
                add_sfx_clip(&mut env.project, p);
                *by_kind.entry(p.tag).or_default() += 1;
            }
            env.edited("Sound Design");
            let lane = format!("{}-sfx-1", env.project.id);
            let lane = env.aliases.alias(Kind::Lane, &lane);
            let mut v = json!({
                "added": placed.len(),
                "replaced": old.len(),
                "byMoment": by_kind,
                "lane": lane,
                "style": style,
            });
            let credits = credits(&placed);
            if !credits.is_empty() {
                v["creditsNeeded"] = credits.into();
            }
            if !failures.is_empty() {
                v["missing"] = failures.into();
            }
            Ok(v)
        });
        Ok(finish)
    })))
}

pub static TOOLS: &[Tool] = &[
    Tool {
        name: "sound_design",
        tier: Tier::Core,
        label: "Sound design",
        description: "Add sound effects to the whole edit in one call: real recordings on every cut, transition, title, text, sticker, glitch/shake effect and typewriter, timed to the frame, on an SFX lane. Running it again replaces its earlier sounds. style: subtle, punchy (default), cinematic, playful, ui.",
        params: || {
            schema(&[
                (
                    "style",
                    "enum:subtle|punchy|cinematic|playful|ui",
                    "The feel",
                ),
                ("start", "number", "Only from here (seconds)"),
                ("end", "number", "Only up to here (seconds)"),
                ("volume", "number", "Percent of the normal level (100)"),
                (
                    "replace",
                    "boolean",
                    "Remove its earlier sounds first (default true)",
                ),
                (
                    "scan_picture",
                    "boolean",
                    "Also find scene changes inside long clips (default: when there are few cuts)",
                ),
            ])
        },
        run: sound_design,
    },
    Tool {
        name: "find_sounds",
        tier: Tier::Core,
        label: "Find sounds",
        description: "Search real sound-effect recordings (the app's library and Openverse, free to use). Returns ids s1, s2… with name, length and licence for add_sound. Use plain words: whoosh, pop, hit, click, riser, glitch, typing, camera shutter, applause.",
        params: || {
            schema(&[
                ("query!", "string", "What it sounds like"),
                ("max_duration", "number", "Longest, seconds (default 4)"),
                ("count", "number", "How many (default 6)"),
            ])
        },
        run: find_sounds,
    },
    Tool {
        name: "add_sound",
        tier: Tier::Core,
        label: "Add sound",
        description: "Put one sound effect on the timeline: sound = an id from find_sounds (s2) or words to search. anchor: start (its attack lands on time, default), peak (its loudest point, for whooshes over a cut) or end (a riser finishing on time).",
        params: || {
            schema(&[
                (
                    "sound!",
                    "string",
                    "s-id from find_sounds, or words like \"whoosh\"",
                ),
                ("time", "number", "Seconds (default: playhead)"),
                ("anchor", "enum:start|peak|end", ""),
                ("volume", "number", "Percent (default 60)"),
                ("duration", "number", "Longest it may run, seconds"),
            ])
        },
        run: add_sound,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tools::test_env::Fixture;

    fn shape() -> Shape {
        Shape {
            length: 1_000_000,
            onset: 200_000,
            peak: 500_000,
            tail: 800_000,
        }
    }

    #[test]
    fn places_by_anchor() {
        // Peak on 2 s: the audible part starts 0.3 s before it.
        let (start, src_in, len, _) = place(&shape(), 2_000_000, Anchor::Peak, 5_000_000).unwrap();
        assert_eq!(src_in, 185_000);
        assert_eq!(start, 2_000_000 - (500_000 - 185_000));
        assert_eq!(len, 860_000 - 185_000);
        // Attack on 0.1 s: shifted into the sound instead of before 0.
        let (start, src_in, _, _) = place(&shape(), 100_000, Anchor::Peak, 5_000_000).unwrap();
        assert_eq!(start, 0);
        assert!(src_in > 185_000);
        // A short window fades out.
        let (_, _, len, fade) = place(&shape(), 2_000_000, Anchor::Start, 300_000).unwrap();
        assert_eq!(len, 300_000);
        assert!(fade > 0);
    }

    #[test]
    fn finds_moments_on_the_timeline() {
        let mut f = Fixture::with_main_clips(3, 2.);
        f.call(
            "add_title",
            json!({ "text": "Hello", "style": "headline", "start": 0.5 }),
        )
        .unwrap();
        f.call(
            "add_title",
            json!({ "text": "typing this", "style": "typewriter", "start": 3 }),
        )
        .unwrap();
        let m = moments(&f.project, 0, f.project.duration, "punchy");
        let kinds: Vec<&str> = m.iter().map(|m| m.cue.name()).collect();
        assert_eq!(kinds, ["title", "cut", "typing", "cut"], "{m:?}");
        let typing = m.iter().find(|m| m.cue == Cue::Typing).unwrap();
        assert!(typing.span > 0);
    }

    #[test]
    fn crowded_moments_thin_out() {
        let ms = vec![
            Moment {
                at: 1_000_000,
                cue: Cue::Text,
                span: 0,
            },
            Moment {
                at: 1_100_000,
                cue: Cue::Title,
                span: 0,
            },
            Moment {
                at: 1_200_000,
                cue: Cue::Cut,
                span: 0,
            },
            Moment {
                at: 3_000_000,
                cue: Cue::Cut,
                span: 0,
            },
        ];
        let kept: Vec<Cue> = thin(ms, "punchy").iter().map(|m| m.cue).collect();
        assert_eq!(kept, [Cue::Title, Cue::Cut]);
    }

    #[test]
    fn sfx_go_on_their_own_lane() {
        let mut p = Project::empty();
        let hit = SoundHit {
            key: "ov:1".into(),
            title: "Whoosh".into(),
            source: "Openverse".into(),
            duration: Some(1.),
            license: "by".into(),
            creator: "x".into(),
            attribution: "\"Whoosh\" by x, CC BY".into(),
            url: String::new(),
            tags: vec![],
            library: None,
        };
        let placed = Placed {
            path: "/tmp/w.mp3".into(),
            hit,
            shape: shape(),
            start: 0,
            src_in: 0,
            len: 500_000,
            fade: 0,
            volume: 0.5,
            tag: "cut",
        };
        let id = add_sfx_clip(&mut p, &placed);
        let (t, c) = p.find_clip(&id).unwrap();
        assert_eq!(t.name, "SFX");
        assert_eq!(c.extra["agentSfx"], "cut");
        assert!(c.extra["attribution"].as_str().unwrap().contains("CC BY"));
        assert!(!has_other_sound(&p));
    }
}
