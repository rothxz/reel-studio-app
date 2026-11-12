//! Making a video, not just editing one: the narrator's voice
//! (`add_voiceover`, captions timed to it with `add_captions`), a music
//! bed (`add_music`), illustrations from the sticker library
//! (`add_sticker`), motion-graphic scenes drawn from nothing
//! (`add_scene`: animated backdrops, charts, counters, comparisons, steps),
//! and `make_video`, which does all of it from one script: every scene's
//! narration spoken and measured, the scene drawn exactly that long, its
//! key point as an animated title, captions on the voice, stickers popping
//! in, music under it all and sound effects on the cuts and titles.
//!
//! Everything `make_video` adds is tagged (`agentVideo`), so asking again
//! replaces the last attempt instead of stacking a second video after it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Map, Value, json};

use super::sound::{self, Placed};
use super::{Args, Env, Finish, Outcome, Progress, Tier, Tool, ToolError, ToolResult, schema};
use crate::agent::captions;
use crate::agent::dice::Dice;
use crate::agent::graphics::{
    self, Backdrop, CodeStep, Flow, Icon, Item, Layout, Node, PALETTES, Palette, Scene, Visual,
    canvas,
};
use crate::agent::snapshot::{Aliases, secs};
use crate::agent::voice::{self, Speech};
use crate::library::{self, MUSIC_MOODS, MusicQuery, StickerItem, StickerKind};
use crate::model::{Clip, MediaItem, Micros, Project, SECOND, Track, TrackType, new_id};

/// Narration speed, for estimating a script's length.
const WORDS_PER_SECOND: f64 = 2.6;
/// What `make_video` made (a second run replaces it).
const TAG: &str = "agentVideo";
/// Room kept before a voice's first word and after its last.
const VOICE_PAD_IN: Micros = 60_000;
const VOICE_PAD_OUT: Micros = 180_000;
/// A scene starts this long before its narration, and holds this long after.
const LEAD: Micros = 350_000;
const HOLD: Micros = 450_000;
const MIN_SCENE: Micros = 2_500_000;
/// Music under a voice sits this low (linear), alone louder.
const MUSIC_UNDER_VOICE: f64 = 0.13;
const MUSIC_ALONE: f64 = 0.6;

pub const CAPTION_STYLES: &[(&str, &str)] = &[
    ("bold", "white bold words with a dark outline (default)"),
    ("box", "white words on a dark box"),
    ("yellow", "yellow bold words with a dark outline"),
    ("clean", "white words with a soft shadow"),
];

// ---- lanes and clips ---------------------------------------------------------------------

/// An audio lane of its own (`<project>-<key>-1`), made on first use.
fn audio_lane(p: &mut Project, key: &str, name: &str) -> String {
    let id = format!("{}-{key}-1", p.id);
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
            name: name.into(),
            clips: vec![],
            order,
            muted: false,
            hidden: false,
            locked: false,
        });
    }
    id
}

#[allow(clippy::too_many_arguments)]
fn audio_clip(
    path: &Path,
    title: &str,
    start: Micros,
    src_in: Micros,
    len: Micros,
    src_len: Micros,
    volume: f64,
    fades: (Micros, Micros),
) -> Clip {
    let mut extra = Map::new();
    extra.insert("title".into(), title.into());
    extra.insert("volume".into(), json!(volume));
    extra.insert("muted".into(), false.into());
    extra.insert(
        "transform".into(),
        json!({ "x": 0, "y": 0, "scale": 1, "rotation": 0, "opacity": 1 }),
    );
    extra.insert("effects".into(), json!([]));
    if fades.0 > 0 || fades.1 > 0 {
        extra.insert(
            "audioFade".into(),
            json!({ "inDurationMicros": fades.0, "outDurationMicros": fades.1 }),
        );
    }
    Clip {
        id: new_id("clip"),
        kind: "audio".into(),
        timeline_start: start.max(0),
        timeline_duration: len.max(SECOND / 10),
        speed: 1.0,
        source: Some(path.to_string_lossy().into_owned()),
        source_start: Some(src_in.max(0)),
        source_duration: Some(src_len.max(len)),
        text: None,
        hidden: false,
        extra,
    }
}

fn push(p: &mut Project, lane: &str, clip: Clip) -> String {
    let id = clip.id.clone();
    if let Some(t) = p.tracks.iter_mut().find(|t| t.id == lane) {
        t.clips.push(clip);
    }
    p.recompute_duration();
    id
}

/// Where the picture ends (the main lane's last clip; 0 when empty).
fn picture_end(p: &Project) -> Micros {
    p.ordered_main_clips()
        .iter()
        .map(|c| c.end())
        .max()
        .unwrap_or(0)
}

fn portrait(p: &Project) -> bool {
    p.canvas.height >= p.canvas.width
}

/// The audible part of a speech file and where its first word sits in it.
fn voice_span(s: &Speech) -> (Micros, Micros) {
    let src_in = (s.shape.onset - VOICE_PAD_IN).max(0);
    let src_out = (s.shape.tail + VOICE_PAD_OUT)
        .min(s.shape.length)
        .max(src_in + SECOND / 10);
    (src_in, src_out - src_in)
}

/// The voice on the Voiceover lane, its first word at `at`. Returns the
/// clip id.
fn place_voice(p: &mut Project, s: &Speech, text: &str, at: Micros) -> String {
    let lane = audio_lane(p, "voice", "Voiceover");
    let n = p
        .tracks
        .iter()
        .find(|t| t.id == lane)
        .map_or(0, |t| t.clips.len())
        + 1;
    let (src_in, len) = voice_span(s);
    let start = (at - (s.shape.onset - src_in)).max(0);
    let mut clip = audio_clip(
        &s.path,
        &format!("Voiceover {n}"),
        start,
        src_in,
        len,
        s.shape.length,
        1.0,
        (0, 0),
    );
    clip.extra.insert("voiceoverText".into(), text.into());
    clip.extra.insert(
        "agentVoice".into(),
        format!("{} · {}", s.engine.name(), s.voice).into(),
    );
    push(p, &lane, clip)
}

fn caption_look(extra: &mut Map<String, Value>, style: &str) -> ToolResult<()> {
    extra.insert("fontFamily".into(), "Nunito_700Bold".into());
    extra.insert("color".into(), "#ffffff".into());
    extra.insert("alignment".into(), "center".into());
    match style {
        "bold" => {
            extra.insert("fontSize".into(), json!(22));
            extra.insert("outlineColor".into(), "#000000".into());
            extra.insert("outlineWidth".into(), json!(0.32));
        }
        "yellow" => {
            extra.insert("fontSize".into(), json!(22));
            extra.insert("color".into(), "#ffd60a".into());
            extra.insert("outlineColor".into(), "#000000".into());
            extra.insert("outlineWidth".into(), json!(0.32));
        }
        "box" => {
            extra.insert("fontSize".into(), json!(19));
            extra.insert("backgroundColor".into(), "#000000".into());
            extra.insert("backgroundOpacity".into(), json!(0.7));
            extra.insert("backgroundRadius".into(), json!(0.4));
        }
        "clean" => {
            extra.insert("fontSize".into(), json!(20));
            extra.insert("shadowColor".into(), "#000000".into());
        }
        other => {
            return Err(format!(
                "caption style \"{other}\" is one of: {}",
                CAPTION_STYLES
                    .iter()
                    .map(|(s, _)| *s)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .into());
        }
    }
    Ok(())
}

/// Captions for the voice clip `voice_id` (made from `s`), timed to the
/// speech; the old ones for that clip go. Returns how many.
fn place_captions(
    p: &mut Project,
    s: &Speech,
    text: &str,
    voice_id: &str,
    style: &str,
) -> ToolResult<usize> {
    let Some((_, voice)) = p.find_clip(voice_id) else {
        return Err("That voice clip is gone".into());
    };
    let (start, src_in, end, tagged) = (
        voice.timeline_start,
        voice.source_start.unwrap_or(0),
        voice.end(),
        voice.extra.contains_key(TAG),
    );
    let old: Vec<String> = p
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.extra.get("agentCaption").and_then(Value::as_str) == Some(voice_id))
        .map(|c| c.id.clone())
        .collect();
    for id in &old {
        p.delete_clip(id);
    }
    let y = if portrait(p) { 0.76 } else { 0.85 };
    let cues = captions::time_cues(&captions::chunks(text, captions::MAX_WORDS), &s.shape);
    let mut n = 0;
    for cue in cues {
        let at = start + (cue.start - src_in);
        let until = (start + (cue.end - src_in)).min(end + SECOND / 4);
        if until - at < 80_000 {
            continue;
        }
        let id = p.add_text_clip(at, &cue.text);
        let mut err = None;
        p.update_clip(&id, |c| {
            c.timeline_duration = until - at;
            c.extra
                .insert("caption".into(), json!({ "language": "en", "lines": 1 }));
            c.extra.insert("agentCaption".into(), voice_id.into());
            if tagged {
                c.extra.insert(TAG.into(), true.into());
            }
            if let Err(e) = caption_look(&mut c.extra, style) {
                err = Some(e);
            }
            c.set_properties_at(&[("transform.y", y - 0.5)], 0, false, 1);
            true
        });
        if let Some(e) = err {
            return Err(e);
        }
        n += 1;
    }
    p.recompute_duration();
    Ok(n)
}

/// A music bed from `from` to `to` (the track repeated if it's short),
/// fading in and out; the agent's last bed goes.
fn place_music(
    p: &mut Project,
    path: &Path,
    title: &str,
    track_len: Micros,
    (from, to): (Micros, Micros),
    volume: f64,
    tagged: bool,
) -> Vec<String> {
    let old: Vec<String> = p
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.extra.contains_key("agentMusic"))
        .map(|c| c.id.clone())
        .collect();
    for id in &old {
        p.delete_clip(id);
    }
    let lane = audio_lane(p, "music", "Music");
    let mut ids = vec![];
    let mut at = from;
    let track_len = track_len.max(SECOND);
    while at < to - SECOND / 10 {
        let len = track_len.min(to - at);
        let first = at == from;
        // A sliver left over isn't worth a clip: this one is the last.
        let last = at + len >= to - SECOND / 10;
        let fades = (
            if first { SECOND.min(len / 3) } else { 0 },
            if last { (2 * SECOND).min(len / 3) } else { 0 },
        );
        let mut clip = audio_clip(path, title, at, 0, len, track_len, volume, fades);
        clip.extra.insert("agentMusic".into(), true.into());
        if tagged {
            clip.extra.insert(TAG.into(), true.into());
        }
        ids.push(push(p, &lane, clip));
        at += len;
    }
    ids
}

/// "top_right", "center", "0.3,0.7" → the transform's offset from the
/// centre (canvas fractions).
fn position(s: Option<&str>) -> ToolResult<(f64, f64)> {
    let s = s
        .unwrap_or("center")
        .trim()
        .to_lowercase()
        .replace([' ', '-'], "_");
    Ok(match s.as_str() {
        "center" | "middle" => (0., 0.),
        "top" => (0., -0.28),
        "bottom" => (0., 0.2),
        "left" => (-0.27, 0.),
        "right" => (0.27, 0.),
        "top_left" => (-0.27, -0.26),
        "top_right" => (0.27, -0.26),
        "bottom_left" => (-0.27, 0.18),
        "bottom_right" => (0.27, 0.18),
        other => {
            let (x, y) = other
                .split_once(',')
                .and_then(|(x, y)| Some((super::parse_number(x)?, super::parse_number(y)?)))
                .ok_or_else(|| {
                    ToolError(
                        "position is center, top, bottom, left, right, top_left… or \"x,y\" (0..1)"
                            .into(),
                    )
                })?;
            (x.clamp(0., 1.) - 0.5, y.clamp(0., 1.) - 0.5)
        }
    })
}

/// A library sticker at `at`, `size` canvas widths wide, at `pos`, with an
/// entrance motion. Returns its id.
fn place_sticker(
    env: &mut Env,
    item: &StickerItem,
    (at, len): (Micros, Micros),
    pos: (f64, f64),
    size: f64,
    motion: Option<&str>,
) -> ToolResult<String> {
    let id = env
        .project
        .add_sticker_clip(at, &item.url, item.width, item.height);
    // A sticker fits 40 % of the short side at scale 1.
    let (cw, ch) = (
        env.project.canvas.width.max(1) as f64,
        env.project.canvas.height.max(1) as f64,
    );
    let short = cw.min(ch);
    let fit = (short * 0.4 / item.width.max(1) as f64).min(short * 0.4 / item.height.max(1) as f64);
    let natural_w = item.width as f64 * fit / cw;
    let scale = (size / natural_w.max(0.01)).clamp(0.05, 5.);
    let title = item.title.clone();
    env.project.update_clip(&id, |c| {
        c.timeline_duration = len.max(SECOND / 2);
        if !title.is_empty() {
            c.extra.insert("title".into(), title.into());
        }
        c.set_properties_at(
            &[
                ("transform.x", pos.0),
                ("transform.y", pos.1),
                ("transform.scale", scale),
            ],
            0,
            false,
            1,
        );
        true
    });
    env.project.recompute_duration();
    if let Some(m) = motion.filter(|m| *m != "none") {
        let alias = env.alias(&id);
        let mut a = Map::new();
        a.insert("clip".into(), alias.into());
        a.insert("motion".into(), m.into());
        if m.ends_with("_in") {
            a.insert("duration".into(), json!(0.45));
        }
        super::run("animate", a, env)?;
    }
    Ok(id)
}

/// Runs another tool and tags every clip it made.
fn run_tagged(env: &mut Env, name: &str, args: Value) -> ToolResult<Value> {
    let before: std::collections::HashSet<String> = env
        .project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(|c| c.id.clone()))
        .collect();
    let args = match args {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let v = match super::run(name, args, env)? {
        Outcome::Done(v) => v,
        Outcome::Later(_) => return Err("internal: a background tool inside make_video".into()),
    };
    for t in &mut env.project.tracks {
        for c in &mut t.clips {
            if !before.contains(&c.id) {
                c.extra.insert(TAG.into(), true.into());
            }
        }
    }
    Ok(v)
}

fn tag(p: &mut Project, id: &str) {
    p.update_clip(id, |c| {
        c.extra.insert(TAG.into(), true.into());
        true
    });
}

/// Where a scene's own animation lands, on its clip (`sound_design` puts
/// sounds there).
fn set_beats(p: &mut Project, id: &str, scene: &Scene) {
    let beats: Vec<Value> = scene
        .beats()
        .into_iter()
        .map(|(t, cue, span)| {
            let r = |x: f32| ((x * 1000.).round() / 1000.) as f64;
            json!([r(t), cue, r(span)])
        })
        .collect();
    if beats.is_empty() {
        return;
    }
    p.update_clip(id, |c| {
        c.extra.insert("agentBeats".into(), beats.into());
        true
    });
}

// ---- reading scene arguments --------------------------------------------------------------

/// "4.50", "$1,825", "12%" → 4.5, 1825, 12.
fn loose_number(s: &str) -> Option<f64> {
    let t: String = s
        .trim()
        .chars()
        .filter(|c| !matches!(c, '$' | '€' | '£' | '¥' | ',' | '%' | ' ' | '+'))
        .collect();
    let t = t.trim_end_matches(|c: char| c.is_alphabetic() || c == '/');
    t.parse().ok()
}

/// One chart row: {"label", "value"}, or "Coffee: $4.50" / "Coffee 4.5".
fn parse_item(v: &Value) -> Option<Item> {
    match v {
        Value::Object(o) => {
            let label = o
                .get("label")
                .or_else(|| o.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let value = match o.get("value")? {
                Value::Number(n) => n.as_f64()?,
                Value::String(s) => loose_number(s)?,
                _ => return None,
            };
            Some(Item { label, value })
        }
        Value::String(s) => {
            let s = s.trim();
            let (label, num) = match s.rsplit_once([':', '=']) {
                Some((l, n)) => (l.trim(), n.trim()),
                None => s.rsplit_once(' ').map(|(l, n)| (l.trim(), n.trim()))?,
            };
            Some(Item {
                label: label.to_string(),
                value: loose_number(num)?,
            })
        }
        Value::Number(n) => Some(Item {
            label: String::new(),
            value: n.as_f64()?,
        }),
        _ => None,
    }
}

fn strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| match x {
                Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect(),
        Some(Value::String(s)) => s
            .split(['\n', ';', '|'])
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        _ => vec![],
    }
}

/// One diagram node: {"label", "detail", "icon", "note"}, or a string
/// "Label: detail" / "Label | detail | note"; with the icon it named.
/// The others are guessed with the whole diagram in view (`read_nodes`).
fn read_node(v: &Value) -> Option<(Node, Option<Icon>)> {
    let clean = |s: &str| {
        let s = s.trim();
        (!s.is_empty()).then(|| s.to_string())
    };
    let (label, detail, icon, note) = match v {
        Value::Object(o) => {
            let get = |keys: &[&str]| {
                keys.iter()
                    .find_map(|k| o.get(*k).and_then(Value::as_str))
                    .and_then(clean)
            };
            (
                get(&["label", "name", "title", "text"])?,
                get(&["detail", "description", "sub", "subtitle"]),
                get(&["icon"]),
                get(&["note", "caption", "step", "says"]),
            )
        }
        Value::String(s) => {
            let parts: Vec<&str> = s.split('|').collect();
            let (label, detail) = match parts[0].split_once([':', '—']) {
                Some((l, d)) => (clean(l)?, clean(d)),
                None => (clean(parts[0])?, parts.get(1).and_then(|d| clean(d))),
            };
            let note = parts
                .get(if parts[0].contains([':', '—']) {
                    1
                } else {
                    2
                })
                .and_then(|n| clean(n));
            (label, detail, None, note)
        }
        _ => return None,
    };
    let given = icon.as_deref().and_then(Icon::parse);
    Some((
        Node {
            label: label.chars().take(28).collect(),
            detail: detail.map(|d| d.chars().take(48).collect()),
            icon: given.unwrap_or(Icon::Sparkle),
            note: note.map(|n| n.chars().take(70).collect()),
        },
        given,
    ))
}

/// A diagram's nodes, the icons nobody named guessed from their words: in
/// a scene about tech, tech pictures; about anything else, everyday ones,
/// each a different picture where the words allow.
fn read_nodes(raw: &[Value], title: Option<&str>) -> Vec<Node> {
    let read: Vec<(Node, Option<Icon>)> = raw.iter().filter_map(read_node).take(8).collect();
    let mut nodes: Vec<Node> = read.iter().map(|(n, _)| n.clone()).collect();
    let tech = Visual::Flow {
        title: title.map(String::from),
        nodes: nodes.clone(),
        layout: Layout::Auto,
    }
    .technical();
    let items: Vec<(&str, &str, Option<Icon>)> = read
        .iter()
        .map(|(n, given)| (n.label.as_str(), n.detail.as_deref().unwrap_or(""), *given))
        .collect();
    for (node, icon) in nodes.iter_mut().zip(Icon::guess_all(&items, tech)) {
        node.icon = icon;
    }
    nodes
}

/// "1-2", "4", "lines 4–5" → (from, to), 1-based.
fn line_range(s: &str) -> Option<(usize, usize)> {
    let digits: Vec<usize> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|d| d.parse().ok())
        .collect();
    match digits.as_slice() {
        [a] => Some((*a, *a)),
        [a, b, ..] => Some(((*a).min(*b), (*a).max(*b))),
        _ => None,
    }
}

/// The code's blocks (runs of lines between blank ones), 1-based.
fn code_blocks(lines: &[String]) -> Vec<(usize, usize)> {
    let mut out = vec![];
    let mut start = None;
    for (i, l) in lines.iter().enumerate() {
        match (l.trim().is_empty(), start) {
            (false, None) => start = Some(i + 1),
            (true, Some(s)) => {
                out.push((s, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, lines.len()));
    }
    out
}

/// A block's own heading: "# 1. RETRIEVE: search…" → ("Retrieve", "search…").
fn block_heading(line: &str) -> Option<(String, Option<String>)> {
    let t = line.trim();
    let body = t.strip_prefix('#').or_else(|| t.strip_prefix("//"))?.trim();
    let body = body
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ')')
        .trim();
    let (head, rest) = match body.split_once(':') {
        Some((h, r)) => (
            h.trim(),
            Some(r.trim().to_string()).filter(|r| !r.is_empty()),
        ),
        None => (body, None),
    };
    if head.is_empty() || head.split_whitespace().count() > 3 {
        return None;
    }
    let mut c = head.to_lowercase().chars().collect::<Vec<_>>();
    if let Some(f) = c.first_mut() {
        *f = f.to_ascii_uppercase();
    }
    Some((c.into_iter().collect(), rest))
}

/// The steps of a code scene: given ("Retrieve: 1-2 | note", or objects
/// with label / lines / note), lines found by block when not given; none
/// given: one per commented block.
fn code_steps(raw: &[Value], lines: &[String]) -> Vec<CodeStep> {
    let blocks = code_blocks(lines);
    let n = lines.len();
    let mut steps: Vec<CodeStep> = vec![];
    for (k, v) in raw.iter().take(5).enumerate() {
        let (label, range, note) = match v {
            Value::Object(o) => {
                let get = |key: &str| {
                    o.get(key)
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                };
                let range = match o.get("lines") {
                    Some(Value::String(r)) => line_range(r),
                    Some(Value::Array(a)) => {
                        let xs: Vec<usize> = a
                            .iter()
                            .filter_map(Value::as_u64)
                            .map(|x| x as usize)
                            .collect();
                        xs.first().map(|&f| (f, *xs.last().unwrap_or(&f)))
                    }
                    Some(Value::Number(x)) => x.as_u64().map(|x| (x as usize, x as usize)),
                    _ => match (
                        o.get("from").and_then(Value::as_u64),
                        o.get("to").and_then(Value::as_u64),
                    ) {
                        (Some(f), Some(t)) => Some((f as usize, t as usize)),
                        _ => None,
                    },
                };
                let Some(label) = get("label").or_else(|| get("name")) else {
                    continue;
                };
                (label.to_string(), range, get("note").map(String::from))
            }
            Value::String(s) => {
                let (head, note) = match s.split_once('|') {
                    Some((h, n)) => (
                        h.trim(),
                        Some(n.trim().to_string()).filter(|n| !n.is_empty()),
                    ),
                    None => (s.trim(), None),
                };
                let (label, range) = match head.split_once([':', '(']) {
                    Some((l, r)) if line_range(r).is_some() => {
                        (l.trim().to_string(), line_range(r))
                    }
                    _ => (head.to_string(), None),
                };
                (label, range, note)
            }
            _ => continue,
        };
        let range = range.or_else(|| blocks.get(k).copied()).unwrap_or_else(|| {
            let per = n.div_ceil(raw.len().max(1));
            (k * per + 1, ((k + 1) * per).min(n))
        });
        steps.push(CodeStep {
            label: label.chars().take(16).collect(),
            from: range.0.clamp(1, n),
            to: range.1.clamp(1, n),
            note,
        });
    }
    if steps.is_empty() {
        for (k, (from, to)) in blocks.iter().enumerate().take(5) {
            let (label, note) = block_heading(&lines[from - 1])
                .unwrap_or_else(|| (format!("Step {}", k + 1), None));
            steps.push(CodeStep {
                label,
                from: *from,
                to: *to,
                note,
            });
        }
        if steps.len() < 2 {
            steps.clear();
        }
    }
    steps
}

/// A scene's visual and its number marks from a call's arguments.
#[cfg(test)]
fn read_visual(a: &Args) -> ToolResult<(Visual, String, String)> {
    read_visual_noted(a).map(|(v, prefix, suffix, _)| (v, prefix, suffix))
}

/// `read_visual`, with what it read forgivingly or left out (each line
/// says how to fix it).
pub(super) fn read_visual_noted(a: &Args) -> ToolResult<(Visual, String, String, Vec<String>)> {
    let mut notes = vec![];
    let kind = a
        .str("visual")
        .or_else(|| a.str("kind"))
        .unwrap_or_else(|| "background".into())
        .to_lowercase()
        .replace([' ', '-'], "_");
    let raw_items: Vec<Value> = match a.0.get("items") {
        Some(Value::Array(v)) => v.clone(),
        Some(Value::String(s)) => strings(Some(&Value::String(s.clone())))
            .into_iter()
            .map(Value::String)
            .collect(),
        _ => vec![],
    };
    let raw_text = raw_items
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let chart_title = a.str("chart_title");
    // A chart given only from / to (small models do this) is a number
    // counting up: that's what it says.
    let has_rows = raw_items.iter().filter_map(parse_item).count() >= 2;
    let kind = if !has_rows
        && a.has("to")
        && matches!(
            kind.as_str(),
            "bar_chart" | "bars" | "chart" | "bar" | "line_chart" | "line" | "growth"
        ) {
        "counter".to_string()
    } else {
        kind
    };
    // Elements given with no visual named: that's a canvas.
    let composed =
        a.0.get("elements")
            .and_then(Value::as_array)
            .is_some_and(|e| !e.is_empty());
    let kind = if composed && matches!(kind.as_str(), "background" | "backdrop" | "none" | "plain")
    {
        "canvas".to_string()
    } else {
        kind
    };
    let visual = match kind.as_str() {
        "background" | "backdrop" | "none" | "plain" => Visual::Background,
        "canvas" | "compose" | "composed" | "custom" | "free" | "layers" | "elements" => {
            let raw: Vec<Value> = match a.0.get("elements") {
                Some(Value::Array(v)) => v.clone(),
                Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
                    Ok(Value::Array(v)) => v,
                    _ => strings(Some(&Value::String(s.clone())))
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                },
                _ => raw_items.clone(),
            };
            let tech = graphics::technical(&Value::Array(raw.clone()).to_string());
            let (elements, more) = canvas::read(&raw, tech)?;
            notes.extend(more);
            let flow = a
                .str("arrange")
                .or_else(|| a.str("layout"))
                .and_then(|l| Flow::parse(&l))
                .unwrap_or(Flow::Stack);
            Visual::Canvas { elements, flow }
        }
        "bar_chart" | "bars" | "chart" | "bar" => {
            let items: Vec<Item> = raw_items.iter().filter_map(parse_item).take(7).collect();
            if items.is_empty() {
                return Err("bar_chart needs items like [\"Coffee: 4.50\", \"Snacks: 3\"]".into());
            }
            Visual::Bars {
                title: chart_title,
                items,
            }
        }
        "line_chart" | "line" | "growth" => {
            let items: Vec<Item> = raw_items.iter().filter_map(parse_item).take(12).collect();
            if items.len() < 2 {
                return Err(
                    "line_chart needs 2+ items in order, like [\"Week 1: 35\", \"Month 1: 150\"]"
                        .into(),
                );
            }
            Visual::Line {
                title: chart_title,
                items,
            }
        }
        "counter" | "number" | "stat" => {
            let to = a
                .num("to")?
                .or_else(|| raw_items.first().and_then(parse_item).map(|i| i.value))
                .ok_or("counter needs \"to\" (the number it counts up to)")?;
            Visual::Counter {
                from: a.num("from")?.unwrap_or(0.),
                to,
                label: a.str("label").or(chart_title),
            }
        }
        "comparison" | "split" | "versus" | "vs" => {
            let left = strings(a.0.get("left_items"));
            let right = strings(a.0.get("right_items"));
            if left.is_empty() || right.is_empty() {
                return Err("comparison needs left_items and right_items".into());
            }
            Visual::Split {
                left: (
                    a.str("left_title").unwrap_or_else(|| "Needs".into()),
                    left.into_iter().take(5).collect(),
                ),
                right: (
                    a.str("right_title").unwrap_or_else(|| "Wants".into()),
                    right.into_iter().take(5).collect(),
                ),
            }
        }
        "flow" | "diagram" | "process" | "pipeline" | "flowchart" | "architecture" => {
            let raw = match a.0.get("nodes") {
                Some(Value::Array(v)) if !v.is_empty() => v.clone(),
                _ => raw_items.clone(),
            };
            let nodes = read_nodes(&raw, chart_title.as_deref());
            if nodes.len() < 2 {
                return Err(
                    "flow needs 2–8 nodes: [{\"label\": \"Query\", \"detail\": \"what the user asks\", \"icon\": \"chat\", \"note\": \"the user asks a question\"}, …] or [\"Query: what the user asks\", …]"
                        .into(),
                );
            }
            let layout = match a.str("layout") {
                None => Layout::Auto,
                Some(l) => Layout::parse(&l).ok_or_else(|| {
                    ToolError(format!("layout is one of: {}", Layout::NAMES.join(", ")))
                })?,
            };
            Visual::Flow {
                title: chart_title,
                nodes,
                layout,
            }
        }
        "code" | "editor" | "snippet" => {
            let lines: Vec<String> = match a.0.get("code") {
                Some(Value::Array(v)) => v
                    .iter()
                    .filter_map(|x| x.as_str().map(|l| l.trim_end().replace('\t', "    ")))
                    .collect(),
                Some(Value::String(c)) => c
                    .lines()
                    .map(|l| l.trim_end().replace('\t', "    "))
                    .collect(),
                _ => vec![],
            };
            // No blank lines at the ends; at most 14 lines.
            let first = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
            let last = lines
                .iter()
                .rposition(|l| !l.trim().is_empty())
                .map_or(0, |i| i + 1);
            let lines: Vec<String> = lines[first..last.max(first)]
                .iter()
                .take(14)
                .cloned()
                .collect();
            if lines.is_empty() {
                return Err("code needs code: the lines to show (a string with newlines)".into());
            }
            let raw_steps: Vec<Value> = match a.0.get("steps") {
                Some(Value::Array(v)) => v.clone(),
                Some(Value::String(t)) => strings(Some(&Value::String(t.clone())))
                    .into_iter()
                    .map(Value::String)
                    .collect(),
                _ => raw_items.clone(),
            };
            Visual::Code {
                title: chart_title,
                file: a.str("file").or_else(|| a.str("filename")),
                steps: code_steps(&raw_steps, &lines),
                lines,
            }
        }
        "steps" | "list" | "checklist" | "tips" => {
            let items: Vec<String> = raw_items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .take(6)
                .collect();
            if items.is_empty() {
                return Err("steps needs items: short lines".into());
            }
            Visual::List {
                title: chart_title,
                items,
            }
        }
        other => {
            return Err(format!(
                "visual \"{other}\" is one of: {}",
                graphics::KINDS
                    .iter()
                    .map(|(k, _)| *k)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .into());
        }
    };
    let prefix = a.str("prefix").unwrap_or_else(|| {
        ["$", "€", "£"]
            .into_iter()
            .find(|c| raw_text.contains(c))
            .unwrap_or("")
            .to_string()
    });
    let suffix = a.str("suffix").unwrap_or_else(|| {
        if raw_text.contains('%') {
            "%".into()
        } else {
            String::new()
        }
    });
    // "$" given as both marks would read "$500 $".
    let suffix = if !prefix.is_empty() && suffix.trim() == prefix.trim() {
        String::new()
    } else {
        suffix
    };
    Ok((visual, prefix, suffix, notes))
}

/// The palette asked for; None: the caller picks (`any_palette`).
fn read_palette(a: &Args) -> ToolResult<Option<&'static Palette>> {
    match a.str("palette") {
        None => Ok(None),
        Some(p) => graphics::palette(&p).map(Some).ok_or_else(|| {
            ToolError(format!(
                "palette is one of: {}",
                PALETTES
                    .iter()
                    .map(|p| p.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }),
    }
}

/// Palettes made for technical topics (glowing circuitry, blueprints).
const TECH_PALETTES: &[&str] = &["neon", "blueprint"];

/// A palette for a video nobody chose one for: varied, never the bare
/// black-and-white one, and the tech ones only for a technical topic.
fn any_palette(dice: &mut Dice, tech: bool) -> &'static Palette {
    let options: Vec<&'static Palette> = PALETTES
        .iter()
        .filter(|p| p.name != "mono" && (tech || !TECH_PALETTES.contains(&p.name)))
        .collect();
    *dice.pick(&options)
}

/// The backdrop asked for, else one the seed picks (the tech grid only
/// behind a technical topic).
fn read_backdrop(a: &Args, dice: &mut Dice, tech: bool) -> ToolResult<Backdrop> {
    match a.str("backdrop") {
        None => {
            let options: Vec<Backdrop> = graphics::BACKDROPS
                .iter()
                .map(|(b, _, _)| *b)
                .filter(|b| tech || *b != Backdrop::Grid)
                .collect();
            Ok(*dice.pick(&options))
        }
        Some(b) => Backdrop::parse(&b).ok_or_else(|| {
            ToolError(format!(
                "backdrop is one of: {}",
                graphics::BACKDROPS
                    .iter()
                    .map(|(_, n, _)| *n)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }),
    }
}

/// A `MusicQuery` for a mood word or a search.
fn music_query(s: &str) -> MusicQuery {
    let w = s.trim().to_lowercase();
    match MUSIC_MOODS
        .iter()
        .find(|(label, tag)| label.to_lowercase() == w || *tag == w)
    {
        Some((_, tag)) => MusicQuery::Mood(tag),
        None => MusicQuery::Search(s.trim().to_string()),
    }
}

/// A music track at least `need` long (else the longest), downloaded and
/// measured. Blocking.
fn fetch_music(query: &str, need: Micros, seed: u64) -> anyhow::Result<(PathBuf, String, Micros)> {
    let tracks = library::music(&music_query(query))?;
    if tracks.is_empty() {
        anyhow::bail!("no music found for \"{query}\"");
    }
    let need_s = need as f64 / SECOND as f64;
    let mut order: Vec<&library::MusicTrack> = tracks.iter().collect();
    order.sort_by(|a, b| {
        let fits = |t: &library::MusicTrack| t.duration_secs >= need_s;
        fits(b).cmp(&fits(a)).then(if fits(a) {
            a.duration_secs.total_cmp(&b.duration_secs)
        } else {
            b.duration_secs.total_cmp(&a.duration_secs)
        })
    });
    // Not always the same track for a mood: one of the best few that fit.
    let fitting = order.iter().filter(|t| t.duration_secs >= need_s).count();
    let pool = fitting.clamp(1, 5).min(order.len());
    Dice::new(seed).shuffle(&mut order[..pool]);
    let mut last = None;
    for t in order.into_iter().take(3) {
        match library::download_track(t) {
            Ok(path) => {
                let len = crate::decoder::probe(&path.to_string_lossy())
                    .map(|m| m.duration)
                    .unwrap_or((t.duration_secs * SECOND as f64) as Micros);
                return Ok((path, t.clip_title(), len));
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("no track downloaded")))
}

/// The first usable sticker for a search, its file fetched. Blocking.
fn fetch_sticker(query: &str, gif: bool) -> anyhow::Result<StickerItem> {
    let kind = if gif {
        StickerKind::Gifs
    } else {
        StickerKind::Stickers
    };
    let items = library::stickers(kind, Some(query), 1)?;
    for item in items.into_iter().take(4) {
        if crate::assets::fetch(crate::assets::REMOTE, &item.url).is_ok() {
            return Ok(item);
        }
    }
    anyhow::bail!("no sticker found for \"{query}\"")
}

fn voice_request(a: &Args, text: String) -> voice::Request {
    voice::Request {
        text,
        voice: a.str("voice"),
        tone: a.str("tone"),
        engine: a.str("engine").or_else(|| a.str("voice_engine")),
    }
}

/// Speaks, falling back to the system voice when the chosen one fails.
fn speak(req: &voice::Request, notes: &mut Vec<String>) -> ToolResult<Speech> {
    match voice::speak(req) {
        Ok(s) => Ok(s),
        Err(e) if !matches!(req.engine.as_deref(), Some("system")) => {
            let mut sys = req.clone();
            sys.engine = Some("system".into());
            let s =
                voice::speak(&sys).map_err(|e2| ToolError(format!("No voice: {e:#}; {e2:#}")))?;
            let engine = voice::Engine::pick(req.engine.as_deref()).map_or("chosen", |e| e.name());
            let note =
                format!("The {engine} voice failed ({e:#}), so the system voice spoke instead");
            if !notes.contains(&note) {
                notes.push(note);
            }
            Ok(s)
        }
        Err(e) => Err(ToolError(format!("No voice: {e:#}"))),
    }
}

fn voice_note(s: &Speech) -> Option<String> {
    (s.engine == voice::Engine::System).then(|| {
        if crate::agent::local_voice::supported() {
            "This is the computer's built-in voice. The free natural voice (Kokoro, on this Mac) sets itself up on first use and needs uv: install it with `brew install uv`, then ask again.".to_string()
        } else {
            "This is the computer's built-in voice. For a natural voice, save an OpenAI or Gemini key in the agent's setup (it's used for the voice even when another model chats).".to_string()
        }
    })
}

// ---- add_voiceover / add_captions ------------------------------------------------------

fn add_voiceover(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let text = args.required_str("text")?;
    let at = env.snap(args.time("start")?.unwrap_or(env.playhead));
    let with_captions = args.bool("captions")?.unwrap_or(true);
    let style = args
        .str("caption_style")
        .unwrap_or_else(|| "bold".into())
        .to_lowercase();
    caption_look(&mut Map::new(), &style)?;
    let req = voice_request(args, text.clone());
    Ok(Outcome::Later(Box::new(move |progress: &Progress| {
        progress.set("Speaking…");
        let mut notes = vec![];
        let speech = speak(&req, &mut notes)?;
        progress.check()?;
        let finish: Finish = Box::new(move |env: &mut Env| {
            let id = place_voice(&mut env.project, &speech, &text, at);
            let n = if with_captions {
                place_captions(&mut env.project, &speech, &text, &id, &style)?
            } else {
                0
            };
            env.edited("Add Voiceover");
            let (_, c) = env.clip(&id)?;
            let (s, e) = (c.timeline_start, c.end());
            let mut v = json!({
                "clip": env.alias(&id),
                "start": secs(s),
                "end": secs(e),
                "duration": secs(e - s),
                "voice": format!("{} ({})", speech.voice, speech.engine.name()),
                "captions": n,
            });
            notes.extend(voice_note(&speech));
            if !notes.is_empty() {
                v["notes"] = notes.into();
            }
            Ok(v)
        });
        Ok(finish)
    })))
}

fn add_captions(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let (_, c) = env.clip(&id)?;
    let text = c
        .extra
        .get("voiceoverText")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or("Captions need a voiceover the agent made (add_voiceover): recognising speech in other sound isn't available yet")?;
    let path = PathBuf::from(c.source.clone().unwrap_or_default());
    let style = args
        .str("style")
        .unwrap_or_else(|| "bold".into())
        .to_lowercase();
    caption_look(&mut Map::new(), &style)?;
    Ok(Outcome::Later(Box::new(move |progress: &Progress| {
        progress.set("Timing the words");
        let shape = voice::measure(&path).map_err(|e| ToolError(format!("{e:#}")))?;
        let speech = Speech {
            path,
            engine: voice::Engine::System,
            voice: String::new(),
            shape,
        };
        let finish: Finish = Box::new(move |env: &mut Env| {
            let n = place_captions(&mut env.project, &speech, &text, &id, &style)?;
            env.edited("Add Captions");
            Ok(json!({ "captions": n, "for": env.alias(&id), "style": style }))
        });
        Ok(finish)
    })))
}

// ---- add_music -------------------------------------------------------------------------

fn add_music(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let query = args
        .str("mood")
        .or_else(|| args.str("search"))
        .unwrap_or_else(|| "upbeat".into());
    let from = args.time("start")?.unwrap_or(0);
    let end = picture_end(&env.project).max(env.project.duration);
    let to = args.time("end")?.unwrap_or(end);
    if to <= from + SECOND / 2 {
        return Err("Nothing to put music under yet: add the picture or a voiceover first, or give start and end".into());
    }
    let has_voice = env
        .project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .any(|c| c.extra.contains_key("voiceoverText"));
    let volume = match args.num("volume")? {
        Some(v) => (v / 100.).clamp(0., 2.),
        None if has_voice => MUSIC_UNDER_VOICE,
        None => MUSIC_ALONE,
    };
    Ok(Outcome::Later(Box::new(move |progress: &Progress| {
        progress.set(format!("Finding {query} music"));
        let (path, title, len) = fetch_music(&query, to - from, Dice::fresh_seed())
            .map_err(|e| ToolError(format!("Music: {e:#}")))?;
        progress.check()?;
        let finish: Finish = Box::new(move |env: &mut Env| {
            let ids = place_music(
                &mut env.project,
                &path,
                &title,
                len,
                (from, to),
                volume,
                false,
            );
            env.edited("Add Music");
            let clips: Vec<String> = ids.iter().map(|id| env.alias(id)).collect();
            Ok(json!({
                "clips": clips,
                "music": title,
                "start": secs(from),
                "end": secs(to),
                "volume": (volume * 100.).round(),
            }))
        });
        Ok(finish)
    })))
}

// ---- add_sticker -----------------------------------------------------------------------

fn add_sticker(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let query = args.required_str("search")?;
    let at = env.snap(args.time("start")?.unwrap_or(env.playhead));
    let len = args.time("duration")?.unwrap_or(3 * SECOND);
    let pos = position(args.str("position").as_deref())?;
    let size = args.num("size")?.unwrap_or(0.35);
    let size = if size > 1.5 { size / 100. } else { size }.clamp(0.05, 1.2);
    let motion = args.str("motion").unwrap_or_else(|| "pop_in".into());
    let gif = args.bool("gif")?.unwrap_or(false);
    Ok(Outcome::Later(Box::new(move |progress: &Progress| {
        progress.set(format!("Finding \"{query}\""));
        let item = fetch_sticker(&query, gif).map_err(|e| ToolError(format!("{e:#}")))?;
        let finish: Finish = Box::new(move |env: &mut Env| {
            let id = place_sticker(env, &item, (at, len), pos, size, Some(&motion))?;
            env.edited("Add Sticker");
            env.select = Some(Some(id.clone()));
            Ok(json!({
                "clip": env.alias(&id),
                "sticker": item.title,
                "start": secs(at),
                "end": secs(at + len),
            }))
        });
        Ok(finish)
    })))
}

// ---- add_scene ---------------------------------------------------------------------------

fn add_scene(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let (visual, prefix, suffix, notes) = read_visual_noted(args)?;
    let seed = Dice::fresh_seed();
    let mut dice = Dice::new(seed);
    let tech = visual.technical();
    let palette = read_palette(args)?.unwrap_or_else(|| any_palette(&mut dice, tech));
    let backdrop = read_backdrop(args, &mut dice, tech)?;
    let len = args
        .time("duration")?
        .unwrap_or(5 * SECOND)
        .clamp(SECOND, 60 * SECOND);
    let (w, h) = graphics::frame_size(env.project.canvas.width, env.project.canvas.height);
    let fps = env.fps().round().clamp(24., 60.) as u32;
    let main = env.project.ordered_main_clips();
    let index = args
        .time("start")?
        .map(|t| crate::model::nearest_seam_index(&main, t));
    let scene = Scene {
        visual,
        palette,
        backdrop,
        seed,
        prefix,
        suffix,
        length: len,
        width: w,
        height: h,
        fps,
    };
    Ok(Outcome::Later(Box::new(move |progress: &Progress| {
        let kind = scene.visual.kind();
        let workers = std::thread::available_parallelism().map_or(4, |c| c.get());
        let path = graphics::render(&scene, workers, &|f| {
            progress.set(format!("Drawing {kind} {}%", (f * 100.) as u32));
            !progress.stopped()
        })
        .map_err(|e| ToolError(format!("Couldn't draw the scene: {e:#}")))?;
        let finish: Finish = Box::new(move |env: &mut Env| {
            let item = scene_media(&path, &scene, kind);
            let id = env.project.insert_media(&item, index);
            set_beats(&mut env.project, &id, &scene);
            env.library_add.push(item);
            env.magnet = true;
            env.edited("Add Scene");
            let (_, c) = env.clip(&id)?;
            let (s, e) = (secs(c.timeline_start), secs(c.end()));
            let mut v = json!({
                "clip": env.alias(&id),
                "scene": kind,
                "palette": scene.palette.name,
                "backdrop": scene.backdrop.name(),
                "start": s,
                "end": e,
                "tip": "add_title / add_sticker put words and pictures on it",
            });
            if !notes.is_empty() {
                v["fix"] = notes.into();
            }
            Ok(v)
        });
        Ok(finish)
    })))
}

fn scene_media(path: &Path, scene: &Scene, name: &str) -> MediaItem {
    MediaItem {
        path: path.to_string_lossy().into_owned(),
        name: format!("{name}.mp4"),
        duration: scene.length,
        width: scene.width,
        height: scene.height,
        has_video: true,
        still: false,
    }
}

// ---- make_video ----------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct ScenePlan {
    narration: String,
    title: Option<String>,
    visual: Visual,
    prefix: String,
    suffix: String,
    stickers: Vec<String>,
    min: Micros,
}

#[derive(Clone, Debug)]
struct Plan {
    title: Option<String>,
    /// What was read forgivingly (shown with the result).
    notes: Vec<String>,
    /// Seconds asked for.
    target: Option<f64>,
    canvas: (u32, u32),
    set_canvas: bool,
    fps: u32,
    palette: &'static Palette,
    backdrop: Backdrop,
    /// This video's picks (layouts, light, sounds, music, title styles).
    seed: u64,
    voice: voice::Request,
    music: Option<String>,
    sfx: Option<String>,
    captions: bool,
    caption_style: String,
    replace: bool,
    scenes: Vec<ScenePlan>,
}

/// What the slow part fetched and drew.
#[derive(Clone)]
struct Built {
    /// The drawn scenes (their beats go on their clips).
    scenes: Vec<Scene>,
    voices: Vec<Speech>,
    lengths: Vec<Micros>,
    files: Vec<PathBuf>,
    frame: (u32, u32),
    stickers: Vec<Vec<StickerItem>>,
    music: Option<(PathBuf, String, Micros)>,
    notes: Vec<String>,
}

/// Scenes one `make_video` call takes; a storyboard, built up over many
/// calls, takes more.
const MAX_SCENES: usize = 14;

fn read_plan(args: &Args, env: &Env) -> ToolResult<Plan> {
    read_plan_of(args, env, MAX_SCENES)
}

fn read_plan_of(args: &Args, env: &Env, max_scenes: usize) -> ToolResult<Plan> {
    let raw = match args.0.get("scenes") {
        Some(Value::Array(a)) if !a.is_empty() => a.clone(),
        Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Array(a)) if !a.is_empty() => a,
            _ => return Err("scenes must be a list of scene objects".into()),
        },
        _ => return Err("scenes is required: a list of {narration, title, visual…}".into()),
    };
    if raw.len() > max_scenes {
        return Err(format!("At most {max_scenes} scenes in one video").into());
    }
    let mut scenes = vec![];
    let mut notes = vec![];
    for (i, s) in raw.iter().enumerate() {
        let o = match s {
            Value::Object(o) => o.clone(),
            Value::String(t) => {
                let mut o = Map::new();
                o.insert("narration".into(), t.clone().into());
                o
            }
            _ => return Err(format!("scene {} must be an object", i + 1).into()),
        };
        let a = Args(o);
        let narration = a
            .str("narration")
            .or_else(|| a.str("voiceover"))
            .or_else(|| a.str("text"))
            .ok_or_else(|| ToolError(format!("scene {} needs narration", i + 1)))?;
        // A visual with its data missing doesn't stop the video: the scene
        // gets the plain backdrop (its title and narration carry it).
        let (visual, prefix, suffix) = match read_visual_noted(&a) {
            Ok((v, prefix, suffix, more)) => {
                notes.extend(more.into_iter().map(|n| format!("Scene {}: {n}", i + 1)));
                (v, prefix, suffix)
            }
            Err(e) => {
                notes.push(format!(
                    "Scene {}: {} (drawn as a plain backdrop)",
                    i + 1,
                    e.0
                ));
                (Visual::Background, String::new(), String::new())
            }
        };
        scenes.push(ScenePlan {
            narration,
            title: a.str("title").or_else(|| a.str("key_point")),
            visual,
            prefix,
            suffix,
            stickers: strings(a.0.get("stickers")).into_iter().take(2).collect(),
            min: a.time("min_duration")?.unwrap_or(0),
        });
    }
    // A length asked for: never refused (a small model asked again and
    // again loops rather than makes nothing); a short result says so.
    let target = args.num("duration")?.filter(|d| *d >= 10.);
    let has_main = !env.project.ordered_main_clips().is_empty();
    let format = args.str("format").or_else(|| args.str("ratio"));
    let replace = args.bool("replace")?.unwrap_or(true);
    let only_ours = env
        .project
        .ordered_main_clips()
        .iter()
        .all(|c| c.extra.contains_key(TAG));
    let set_canvas = format.is_some() || !has_main || (replace && only_ours);
    let canvas = if set_canvas {
        let f = format.unwrap_or_else(|| "9:16".into());
        let ratio = match f.split_once(':') {
            Some((w, h)) => match (super::parse_number(w), super::parse_number(h)) {
                (Some(w), Some(h)) if w > 0. && h > 0. => w / h,
                _ => return Err(format!("format \"{f}\" isn't a ratio like 9:16").into()),
            },
            None => match f.to_lowercase().as_str() {
                "vertical" | "portrait" | "reel" | "short" | "tiktok" => 9. / 16.,
                "horizontal" | "landscape" | "youtube" => 16. / 9.,
                "square" => 1.,
                _ => return Err(format!("format \"{f}\" isn't a ratio like 9:16").into()),
            },
        };
        crate::ui::player::canvas_size_for_ratio(ratio)
    } else {
        (env.project.canvas.width, env.project.canvas.height)
    };
    let off = |v: Option<String>| {
        v.filter(|m| !matches!(m.to_lowercase().as_str(), "none" | "off" | "no" | "false"))
    };
    let sfx = off(Some(
        args.str("sound_effects")
            .unwrap_or_else(|| "subtle".into())
            .to_lowercase(),
    ));
    if let Some(s) = &sfx
        && !sound::STYLES.iter().any(|(n, _)| n == s)
    {
        return Err(format!(
            "sound_effects is none or one of: {}",
            sound::STYLES
                .iter()
                .map(|(s, _)| *s)
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into());
    }
    let caption_style = args
        .str("caption_style")
        .unwrap_or_else(|| "bold".into())
        .to_lowercase();
    caption_look(&mut Map::new(), &caption_style)?;
    let seed = args
        .num("seed")?
        .map_or_else(Dice::fresh_seed, |s| s.abs() as u64);
    let mut dice = Dice::new(seed);
    let tech = scenes.iter().any(|s| s.visual.technical());
    let palette = read_palette(args)?.unwrap_or_else(|| any_palette(&mut dice, tech));
    let backdrop = read_backdrop(args, &mut dice, tech)?;
    Ok(Plan {
        title: args.str("title"),
        notes,
        target,
        canvas,
        set_canvas,
        fps: env.fps().round().clamp(24., 60.) as u32,
        palette,
        backdrop,
        seed,
        voice: voice_request(args, String::new()),
        music: off(Some(args.str("music").unwrap_or_else(|| "upbeat".into()))),
        sfx,
        captions: args.bool("captions")?.unwrap_or(true),
        caption_style,
        replace,
        scenes,
    })
}

/// A whole number of frames.
fn frames(len: Micros, fps: u32) -> Micros {
    let n = (len as f64 * fps as f64 / SECOND as f64).ceil();
    (n * SECOND as f64 / fps as f64).round() as Micros
}

/// How long each scene runs: its narration with room around it.
fn scene_lengths(plan: &Plan, voices: &[Speech]) -> Vec<Micros> {
    let n = plan.scenes.len();
    plan.scenes
        .iter()
        .zip(voices)
        .enumerate()
        .map(|(i, (s, v))| {
            let spoken = voice_span(v).1;
            let tail = if i + 1 == n { HOLD + SECOND } else { HOLD };
            frames((LEAD + spoken + tail).max(MIN_SCENE).max(s.min), plan.fps)
        })
        .collect()
}

/// The slow part, overlapped: the voices (a few at once), the stickers and
/// the music are fetched together; then the scenes are drawn (a few at
/// once, each on several threads) while the sound effects for the laid-out
/// video are found. Returns what was made and the sound effects. Blocking.
fn build(plan: &Plan, draft: Project, progress: &Progress) -> ToolResult<(Built, Vec<Placed>)> {
    let n = plan.scenes.len();
    let clock = std::time::Instant::now();
    // Where the time went, on stderr with REEL_AGENT_LOG.
    let log = |what: &str| {
        if std::env::var_os("REEL_AGENT_LOG").is_some() {
            eprintln!(
                "[make_video] {what} at {:.1}s",
                clock.elapsed().as_secs_f32()
            );
        }
    };
    let notes = Mutex::new(Vec::<String>::new());
    let note = |s: String| {
        if let Ok(mut v) = notes.lock()
            && !v.contains(&s)
        {
            v.push(s);
        }
    };
    // About 2.6 spoken words a second: enough to pick a long enough track
    // before the voices are measured (a short one repeats anyway).
    let words: usize = plan
        .scenes
        .iter()
        .map(|s| s.narration.split_whitespace().count())
        .sum();
    let guess = ((words as f64 / WORDS_PER_SECOND + n as f64 * 0.8 + 1.) * SECOND as f64) as Micros;
    let spoken = AtomicUsize::new(0);
    progress.set(format!("Voices 0/{n}, stickers and music"));
    // The sound effects a video wants don't depend on its script: fetched
    // from the first second, placed once the video is laid out.
    let beats = plan.scenes.iter().any(|s| {
        matches!(
            s.visual,
            Visual::Flow { .. } | Visual::Code { .. } | Visual::List { .. } | Visual::Canvas { .. }
        )
    });
    let sounds = plan.sfx.clone().map(|style| {
        let quiet = progress.quiet();
        let seed = plan.seed;
        std::thread::spawn(move || sound::prefetch_for_video(&style, beats, seed, &quiet))
    });
    let (voices, stickers, music) = std::thread::scope(|s| {
        let stickers = s.spawn(|| {
            let wanted: Vec<(usize, &String)> = plan
                .scenes
                .iter()
                .enumerate()
                .flat_map(|(i, sc)| sc.stickers.iter().map(move |q| (i, q)))
                .collect();
            let found: Vec<(usize, anyhow::Result<StickerItem>, &String)> =
                std::thread::scope(|s2| {
                    let hs: Vec<_> = wanted
                        .iter()
                        .map(|&(i, q)| s2.spawn(move || (i, fetch_sticker(q, false), q)))
                        .collect();
                    hs.into_iter().filter_map(|h| h.join().ok()).collect()
                });
            log("stickers");
            let mut rows = vec![vec![]; n];
            for (i, r, q) in found {
                match r {
                    Ok(item) => rows[i].push(item),
                    Err(e) => note(format!("No sticker for \"{q}\": {e:#}")),
                }
            }
            rows
        });
        let music = s.spawn(|| {
            let m = plan
                .music
                .as_ref()
                .map(|q| fetch_music(q, guess, plan.seed));
            log("music");
            m
        });
        // Cloud voices four at once (rate limits); the system's all at once.
        let at_once = match voice::Engine::pick(plan.voice.engine.as_deref()) {
            Ok(voice::Engine::System) => n.max(1),
            _ => 4,
        };
        let mut voices: Vec<ToolResult<Speech>> = vec![];
        for chunk in (0..n).collect::<Vec<_>>().chunks(at_once) {
            if progress.stopped() {
                break;
            }
            let rs: Vec<ToolResult<Speech>> = std::thread::scope(|s2| {
                let hs: Vec<_> = chunk
                    .iter()
                    .map(|&i| {
                        let (spoken, note) = (&spoken, &note);
                        s2.spawn(move || {
                            let mut req = plan.voice.clone();
                            req.text = plan.scenes[i].narration.clone();
                            let mut mine = vec![];
                            let r = speak(&req, &mut mine);
                            mine.into_iter().for_each(note);
                            let d = spoken.fetch_add(1, Ordering::Relaxed) + 1;
                            log(&format!("voice {}", i + 1));
                            progress.set(format!("Voices {d}/{n}, stickers and music"));
                            r
                        })
                    })
                    .collect();
                hs.into_iter()
                    .map(|h| h.join().unwrap_or_else(|_| Err("The voice crashed".into())))
                    .collect()
            });
            voices.extend(rs);
        }
        let stickers = stickers.join().unwrap_or_else(|_| vec![vec![]; n]);
        let music = music.join().ok().flatten();
        (voices, stickers, music)
    });
    progress.check()?;
    let voices: Vec<Speech> = voices.into_iter().collect::<ToolResult<_>>()?;
    let music = match music {
        Some(Ok(m)) => Some(m),
        Some(Err(e)) => {
            note(format!("No music: {e:#}"));
            None
        }
        None => None,
    };
    let lengths = scene_lengths(plan, &voices);
    let frame = graphics::frame_size(plan.canvas.0, plan.canvas.1);
    let scenes: Vec<Scene> = plan
        .scenes
        .iter()
        .zip(&lengths)
        .enumerate()
        .map(|(i, (s, &length))| Scene {
            visual: s.visual.clone(),
            palette: plan.palette,
            backdrop: plan.backdrop,
            seed: Dice::mix(plan.seed, i as u64),
            prefix: s.prefix.clone(),
            suffix: s.suffix.clone(),
            length,
            width: frame.0,
            height: frame.1,
            fps: plan.fps,
        })
        .collect();
    let built = Built {
        scenes: scenes.clone(),
        voices,
        lengths,
        // Known before they're drawn: the sound effects can be found now.
        files: scenes.iter().map(Scene::path).collect(),
        frame,
        stickers,
        music,
        notes: vec![],
    };
    let total: usize = scenes
        .iter()
        .map(|s| (s.length as f64 * s.fps as f64 / SECOND as f64).ceil() as usize)
        .sum();
    let done = AtomicUsize::new(0);
    let at_once = n.clamp(1, 3);
    let cores = std::thread::available_parallelism().map_or(4, |c| c.get());
    let workers = (cores / at_once).max(2);
    let drawn = {
        let drawn = (|| -> ToolResult<()> {
            for batch in scenes.chunks(at_once) {
                progress.check()?;
                let results: Vec<anyhow::Result<PathBuf>> = std::thread::scope(|s2| {
                    let hs: Vec<_> = batch
                        .iter()
                        .map(|scene| {
                            let done = &done;
                            s2.spawn(move || {
                                graphics::render(scene, workers, &|_| {
                                    let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                                    if d.is_multiple_of(20) {
                                        progress.set(format!(
                                            "Drawing scenes {}%",
                                            (d * 100 / total.max(1)).min(100)
                                        ));
                                    }
                                    !progress.stopped()
                                })
                            })
                        })
                        .collect();
                    hs.into_iter()
                        .map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("crashed"))))
                        .collect()
                });
                for r in results {
                    progress.check()?;
                    r.map_err(|e| ToolError(format!("Couldn't draw a scene: {e:#}")))?;
                }
            }
            Ok(())
        })();
        log("scenes drawn");
        drawn
    };
    drawn?;
    progress.check()?;
    let mut placed = vec![];
    if let (Some(style), Some(sounds)) = (&plan.sfx, sounds) {
        progress.set("Sound effects");
        match sounds.join() {
            Ok(Ok(pool)) => {
                log("sound effects");
                if !pool.missing.is_empty() {
                    note(format!("Some sounds missing: {}", pool.missing.join("; ")));
                }
                // Lay the video out on a copy to read its moments.
                let mut aliases = Aliases::default();
                let mut shelf = super::SoundShelf::default();
                let mut board = None;
                let layers = |_: Micros| vec![];
                let mut copy = Env {
                    project: draft,
                    aliases: &mut aliases,
                    sounds: &mut shelf,
                    board: &mut board,
                    media: &[],
                    playhead: 0,
                    selected: None,
                    effect_names: vec![],
                    layers_at: &layers,
                    vision: None,
                    changed: None,
                    magnet: false,
                    seek_to: None,
                    select: None,
                    library_add: vec![],
                };
                assemble(&mut copy, plan, &built)?;
                placed = sound::design_from(&copy.project, style, &pool);
            }
            Ok(Err(e)) if e.0 == "Stopped" => return Err(e),
            Ok(Err(e)) => note(format!("No sound effects: {}", e.0)),
            Err(_) => note("No sound effects: the search crashed".into()),
        }
    }
    let mut built = built;
    built.notes = notes.into_inner().unwrap_or_default();
    Ok((built, placed))
}

/// Puts the made video on the timeline (deterministic: the slow part runs
/// it once on a copy to find where the sound effects go).
fn assemble(env: &mut Env, plan: &Plan, built: &Built) -> ToolResult<Value> {
    env.edited("Make Video");
    let mut replaced = 0;
    if plan.replace {
        let old: Vec<String> = env
            .project
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.extra.contains_key(TAG))
            .map(|c| c.id.clone())
            .collect();
        replaced = old.len();
        for id in &old {
            env.project.delete_clip(id);
        }
    }
    if plan.set_canvas {
        env.project.canvas.width = plan.canvas.0;
        env.project.canvas.height = plan.canvas.1;
    }
    if let Some(t) = &plan.title
        && (env.project.name.is_empty() || env.project.name == "New Project")
    {
        env.project.name = t.clone();
    }
    let p = plan.palette;
    // The video's title styles: (first and last, the rest), its seed's pick.
    let title_pairs: &[(&str, &str)] = if p.light {
        &[
            ("pop", "slide_up"),
            ("headline", "slide_up"),
            ("pop", "word_by_word"),
            ("typewriter", "slide_up"),
        ]
    } else {
        &[
            ("pop", "slide_up"),
            ("headline", "slide_up"),
            ("pop", "word_by_word"),
            ("neon", "slide_up"),
            ("typewriter", "word_by_word"),
            ("headline", "typewriter"),
        ]
    };
    let titles = *Dice::new(Dice::mix(plan.seed, 0x7171)).pick(title_pairs);
    let tall = portrait(&env.project);
    let base = picture_end(&env.project);
    let n = plan.scenes.len();
    let mut at = base;
    let mut rows = vec![];
    let mut caption_count = 0;
    for (i, scene) in plan.scenes.iter().enumerate() {
        let len = built.lengths[i];
        let kind = scene.visual.kind();
        let item = MediaItem {
            path: built.files[i].to_string_lossy().into_owned(),
            name: format!("Scene {} – {kind}.mp4", i + 1),
            duration: len,
            width: built.frame.0,
            height: built.frame.1,
            has_video: true,
            still: false,
        };
        let clip = env.project.insert_media(&item, None);
        tag(&mut env.project, &clip);
        if let Some(sc) = built.scenes.get(i) {
            set_beats(&mut env.project, &clip, sc);
        }
        env.library_add.push(item);
        // The key point, as an animated title at the top.
        if let Some(title) = &scene.title {
            let (style, size) = match i {
                0 => (titles.0, 34),
                _ if i + 1 == n => (titles.0, 32),
                _ => (titles.1, 28),
            };
            let mut a = json!({
                "text": title,
                "style": style,
                "start": secs(at + SECOND / 6),
                "duration": secs(len - SECOND / 4),
                "position": if tall { "0.17" } else { "0.14" },
                "size": size,
                "color": p.text_hex(),
            });
            if p.light {
                a["outline_color"] = "#ffffff".into();
            }
            run_tagged(env, "add_title", a)?;
        }
        // The narration, its first word just after the cut.
        let speech = &built.voices[i];
        let voice = place_voice(&mut env.project, speech, &scene.narration, at + LEAD);
        tag(&mut env.project, &voice);
        if plan.captions {
            caption_count += place_captions(
                &mut env.project,
                speech,
                &scene.narration,
                &voice,
                &plan.caption_style,
            )?;
        }
        // Illustrations popping in.
        // A diagram, code or a composed scene fills its frame: no
        // stickers over it.
        let busy = matches!(
            scene.visual,
            Visual::Flow { .. } | Visual::Code { .. } | Visual::Canvas { .. }
        );
        let stickers = if busy {
            vec![]
        } else {
            built.stickers.get(i).cloned().unwrap_or_default()
        };
        let plain = matches!(scene.visual, Visual::Background);
        let spots: [((f64, f64), f64); 2] = if plain {
            if stickers.len() == 1 {
                [((0., 0.), 0.5), ((0., 0.), 0.)]
            } else {
                [((-0.2, 0.0), 0.36), ((0.2, 0.0), 0.36)]
            }
        } else if tall {
            [((0.33, -0.22), 0.2), ((-0.33, -0.22), 0.2)]
        } else {
            [((0.36, -0.26), 0.14), ((-0.36, -0.26), 0.14)]
        };
        for (k, item) in stickers.iter().enumerate().take(2) {
            let (pos, size) = spots[k];
            let start = env.snap(at + SECOND * 3 / 10 + k as Micros * SECOND / 4);
            let id = place_sticker(
                env,
                item,
                (start, at + len - SECOND / 10 - start),
                pos,
                size,
                Some("pop_in"),
            )?;
            tag(&mut env.project, &id);
        }
        rows.push(json!({
            "scene": i + 1,
            "clip": env.alias(&clip),
            "visual": kind,
            "start": secs(at),
            "end": secs(at + len),
        }));
        at += len;
    }
    let mut music = Value::Null;
    if let Some((path, title, track_len)) = &built.music {
        place_music(
            &mut env.project,
            path,
            title,
            *track_len,
            (base, at),
            MUSIC_UNDER_VOICE,
            true,
        );
        music = title.clone().into();
    }
    env.project.recompute_duration();
    env.seek_to = Some(base);
    env.select = Some(None);
    let voice = built
        .voices
        .first()
        .map(|s| format!("{} ({})", s.voice, s.engine.name()))
        .unwrap_or_default();
    Ok(json!({
        "start": secs(base),
        "end": secs(at),
        "duration": secs(at - base),
        "scenes": rows,
        "voice": voice,
        "captions": caption_count,
        "music": music,
        "palette": p.name,
        "backdrop": plan.backdrop.name(),
        "titles": format!("{} / {}", titles.0, titles.1),
        "replaced": replaced,
    }))
}

fn make_video(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    make_video_of(
        args,
        env,
        MAX_SCENES,
        "Tell the user what you made. Small fixes: edit_text, add_title, add_sticker, add_music, sound_design; a whole redo: make_video again (it replaces this one).",
    )
}

/// Whether a video's settings (format, palette, sounds…) read, before any
/// scene is written.
pub(super) fn check_settings(args: &Args, env: &Env) -> ToolResult<()> {
    let mut a = args.0.clone();
    a.insert("scenes".into(), json!([{ "narration": "x" }]));
    read_plan(&Args(a), env).map(|_| ())
}

/// About how long a scene with this narration runs, seconds.
pub(super) fn spoken_secs(narration: &str) -> f64 {
    let words = narration.split_whitespace().count() as f64;
    (words / WORDS_PER_SECOND + secs(LEAD + HOLD)).max(secs(MIN_SCENE))
}

/// `make_video` for a script of up to `max_scenes` scenes; `next` is what
/// the result tells the model to do afterwards.
pub(super) fn make_video_of(
    args: &Args,
    env: &mut Env,
    max_scenes: usize,
    next: &'static str,
) -> ToolResult<Outcome> {
    let plan = read_plan_of(args, env, max_scenes)?;
    let draft = env.project.clone();
    Ok(Outcome::Later(Box::new(move |progress: &Progress| {
        let (built, placed) = build(&plan, draft, progress)?;
        let finish: Finish = Box::new(move |env: &mut Env| {
            let mut v = assemble(env, &plan, &built)?;
            for s in &placed {
                let id = sound::add_sfx_clip(&mut env.project, s);
                tag(&mut env.project, &id);
            }
            v["soundEffects"] = placed.len().into();
            let mut notes = plan.notes.clone();
            notes.extend(built.notes.iter().cloned());
            let made = v["duration"].as_f64().unwrap_or(0.);
            if let Some(t) = plan.target
                && made < t * 0.85
            {
                notes.push(format!(
                    "It runs {made:.0} s, shorter than the {t:.0} s asked: tell the user, and offer a longer script."
                ));
            }
            if let Some(s) = built.voices.first() {
                notes.extend(voice_note(s));
            }
            if !notes.is_empty() {
                v["notes"] = notes.into();
            }
            v["next"] = next.into();
            Ok(v)
        });
        Ok(finish)
    })))
}

// ---- the table ------------------------------------------------------------------------------

/// A scene's arguments. `composed`: with the free canvas and its elements
/// (left out of `make_video`: one call for a whole video is already as much
/// as a small model writes well).
pub(super) fn scene_props(with_narration: bool, composed: bool) -> Map<String, Value> {
    let kinds: Vec<&str> = graphics::KINDS
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| composed || *k != "canvas")
        .collect();
    let strings =
        |d: &str| json!({ "type": "array", "items": { "type": "string" }, "description": d });
    let mut p = Map::new();
    if with_narration {
        p.insert(
            "narration".into(),
            json!({ "type": "string", "description": "What the narrator says in this scene: 2–4 spoken sentences, about 25–40 words" }),
        );
        p.insert(
            "title".into(),
            json!({ "type": "string", "description": "On-screen key point, 2–6 words" }),
        );
        p.insert(
            "stickers".into(),
            strings("0–2 illustration searches, e.g. \"coffee cup\", \"piggy bank\""),
        );
    }
    p.insert(
        "visual".into(),
        json!({ "type": "string", "enum": kinds, "description": "The graphic (default background)" }),
    );
    p.insert(
        "items".into(),
        strings(
            "bar_chart / line_chart: \"Label: value\" (e.g. \"Coffee: $4.50\"); steps: short lines",
        ),
    );
    p.insert(
        "nodes".into(),
        json!({
            "type": "array",
            "description": "flow: 2–8 parts or steps in order, each {label (1–2 words), detail (a few words), icon, note (the line shown while it's lit, ~6–10 words)}",
            "items": {
                "type": "object",
                "properties": {
                    "label": { "type": "string" },
                    "detail": { "type": "string" },
                    "icon": { "type": "string", "enum": Icon::names() },
                    "note": { "type": "string" }
                },
                "required": ["label"]
            }
        }),
    );
    p.insert(
        "layout".into(),
        json!({ "type": "string", "enum": Layout::NAMES, "description": "flow: snake (two columns), row, column, cycle (a loop); auto varies it" }),
    );
    p.insert(
        "code".into(),
        json!({ "type": "string", "description": "code: the lines to show (≤14, short), blocks apart by blank lines, each block headed by a comment like \"# 1. RETRIEVE: …\"" }),
    );
    p.insert(
        "file".into(),
        json!({ "type": "string", "description": "code: the file name on the window" }),
    );
    p.insert(
        "steps".into(),
        strings("code: \"Label: lines | note\", e.g. \"Retrieve: 1-2 | pulls the top 3 chunks\" (default: one per commented block)"),
    );
    p.insert(
        "chart_title".into(),
        json!({ "type": "string", "description": "A heading inside the graphic (flow / code: a short tagline in a pill)" }),
    );
    p.insert(
        "from".into(),
        json!({ "type": "number", "description": "counter: start number (0)" }),
    );
    p.insert(
        "to".into(),
        json!({ "type": "number", "description": "counter: end number" }),
    );
    p.insert(
        "label".into(),
        json!({ "type": "string", "description": "counter: words under the number" }),
    );
    p.insert(
        "left_title".into(),
        json!({ "type": "string", "description": "comparison: left heading" }),
    );
    p.insert("left_items".into(), strings("comparison: left column"));
    p.insert(
        "right_title".into(),
        json!({ "type": "string", "description": "comparison: right heading" }),
    );
    p.insert("right_items".into(), strings("comparison: right column"));
    p.insert(
        "prefix".into(),
        json!({ "type": "string", "description": "Before every number, e.g. $" }),
    );
    p.insert(
        "suffix".into(),
        json!({ "type": "string", "description": "After every number, e.g. %" }),
    );
    if composed {
        let text = |d: &str| json!({ "type": "string", "description": d });
        let pick =
            |names: &[&str], d: &str| json!({ "type": "string", "enum": names, "description": d });
        let number = |d: &str| json!({ "type": "number", "description": d });
        p.insert(
            "elements".into(),
            json!({
                "type": "array",
                "description": "canvas: what's on screen, in the order it appears. Each element is ONE of text, number, icon, shape, bar, card, line, plus how it looks, where it sits and how it moves. Leave out at / start and it is laid out and timed for you.",
                "items": {
                    "type": "object",
                    "properties": {
                        "text": text("Words on screen"),
                        "number": number("A number counting up to this (with prefix / suffix, from)"),
                        "icon": pick(&Icon::names(), "A line icon in a disc"),
                        "shape": pick(canvas::SHAPES, "A plain shape"),
                        "bar": number("A progress bar filled to this percent"),
                        "card": text("A panel with these words (and detail, icon)"),
                        "line": { "type": "array", "items": { "type": "string" }, "description": "An arrow joining two elements by id: [\"a\", \"b\"]" },
                        "label": text("A small line under a number, icon or shape; over a bar"),
                        "detail": text("card: a second, smaller line"),
                        "prefix": text("number: before it, e.g. $"),
                        "suffix": text("number: after it, e.g. %"),
                        "id": text("A name lines can join"),
                        "size": text("hero, large, medium, small or tiny"),
                        "color": text("accent, accent2, accent3, text, muted or #rrggbb"),
                        "at": text("Its centre on the stage: \"x,y\" each 0–1 (\"0.5,0.2\"), or center, top, bottom, left, right, top_left, top_right, bottom_left, bottom_right"),
                        "start": number("Seconds into the scene it comes in"),
                        "end": number("Seconds into the scene it leaves (default: stays)"),
                        "in": pick(canvas::ENTRANCES, "How it comes in (default: words rise, icons pop, lines draw, numbers count)"),
                        "out": pick(canvas::EXITS, "How it leaves"),
                        "loop": pick(canvas::LOOPS, "A motion it keeps once landed"),
                        "moves": {
                            "type": "array",
                            "description": "Later changes: from `at` seconds, over `duration`, it goes to x / y (0–1), scale, rotate (degrees), opacity",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "at": number("Seconds into the scene"),
                                    "duration": number("Seconds (0.6)"),
                                    "x": number("0–1"), "y": number("0–1"),
                                    "scale": number("1 = its size"),
                                    "rotate": number("Degrees"),
                                    "opacity": number("0–1")
                                }
                            }
                        }
                    }
                }
            }),
        );
        p.insert(
            "arrange".into(),
            pick(canvas::FLOWS, "canvas: how elements with no \"at\" are laid out (default stack: one under another)"),
        );
    }
    p
}

fn palettes_enum() -> Value {
    json!({
        "type": "string",
        "enum": PALETTES.iter().map(|p| p.name).collect::<Vec<_>>(),
        "description": format!(
            "Pick one that fits the topic (default: a different one each time). {}",
            PALETTES.iter().map(|p| format!("{}: {}", p.name, p.about)).collect::<Vec<_>>().join("; ")
        ),
    })
}

fn backdrops_enum() -> Value {
    json!({
        "type": "string",
        "enum": graphics::BACKDROPS.iter().map(|(_, n, _)| *n).collect::<Vec<_>>(),
        "description": format!(
            "Default: a different one each time. {}",
            graphics::BACKDROPS.iter().map(|(_, n, d)| format!("{n}: {d}")).collect::<Vec<_>>().join("; ")
        ),
    })
}

/// What a whole video is set to (everything of `make_video` but its
/// scenes).
pub(super) fn video_props() -> Map<String, Value> {
    let caption_styles: Vec<&str> = CAPTION_STYLES.iter().map(|(s, _)| *s).collect();
    let mut sfx: Vec<&str> = sound::STYLES.iter().map(|(s, _)| *s).collect();
    sfx.push("none");
    let v = json!({
            "title": { "type": "string", "description": "The video's name" },
            "duration": { "type": "number", "description": "Seconds the user asked for (e.g. 75); narration runs about 2.5 words a second" },
            "format": { "type": "string", "enum": ["9:16", "16:9", "1:1", "4:5"], "description": "Default 9:16 (Reels, Shorts, TikTok)" },
            "palette": palettes_enum(),
            "backdrop": backdrops_enum(),
            "voice": { "type": "string", "description": "female (default), male, british female / british male, or a voice name (Kokoro: af_heart, af_bella, af_nicole, am_michael, am_fenrir, am_puck, bf_emma, bm_george…)" },
            "tone": { "type": "string", "description": "How the narrator sounds, e.g. \"friendly teacher, upbeat\"" },
            "engine": { "type": "string", "enum": ["auto", "kokoro", "openai", "gemini", "system"], "description": "Voice engine; auto (default) is the free local Kokoro voice on Apple silicon" },
            "music": { "type": "string", "description": "Mood: chill, upbeat, cinematic, corporate, electronic, acoustic; a search; or none" },
            "sound_effects": { "type": "string", "enum": sfx, "description": "Default subtle" },
            "captions": { "type": "boolean", "description": "Subtitles on the narration (default true)" },
            "caption_style": { "type": "string", "enum": caption_styles },
    });
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn make_video_params() -> Value {
    let mut props = video_props();
    props.insert(
        "scenes".into(),
        json!({
            "type": "array",
            "description": "In order. A 60–90 s video is 6–8 scenes of 25–40 words of narration each (about 2.5 words a second).",
            "items": { "type": "object", "properties": scene_props(true, false), "required": ["narration"] }
        }),
    );
    json!({ "type": "object", "properties": props, "required": ["scenes"] })
}

fn add_scene_params() -> Value {
    let mut p = scene_props(false, true);
    p.insert("palette".into(), palettes_enum());
    p.insert("backdrop".into(), backdrops_enum());
    p.insert(
        "duration".into(),
        json!({ "type": "number", "description": "Seconds (default 5)" }),
    );
    p.insert(
        "start".into(),
        json!({ "type": "number", "description": "Insert at the main-lane cut nearest this time (default: the end)" }),
    );
    json!({ "type": "object", "properties": p })
}

pub static TOOLS: &[Tool] = &[
    Tool {
        name: "make_video",
        tier: Tier::Core,
        label: "Make video",
        description: "Makes a whole video from nothing in one call: per scene the narrator speaks its narration (a real voice) over an animated motion-graphic scene drawn that long (background, bar_chart, counter, line_chart, comparison, steps; flow: a diagram whose nodes light up in turn; code: an editor highlighting each step), its title animates in, captions follow the voice, stickers pop in; music and sound effects on top. For \"make a video about…\", explainers, educational and promo videos. Write the script; vary the visuals (how it works → flow, programming → code, numbers → charts). Any topic, not only tech: each scene's look follows its words; give flow nodes an icon that shows the idea. Palette, backdrop, layouts, title styles, music and sounds vary by themselves; set palette to fit the topic. Pass duration when a length is asked. A second call replaces the first.",
        params: make_video_params,
        run: make_video,
    },
    Tool {
        name: "add_voiceover",
        tier: Tier::Core,
        label: "Voiceover",
        description: "The narrator speaks your words (text to speech) on the Voiceover lane at start, with captions timed to the voice. voice: female, male or a name; tone: how it sounds.",
        params: || {
            schema(&[
                ("text!", "string", "The words to speak"),
                ("start", "number", "Seconds (default: playhead)"),
                (
                    "voice",
                    "string",
                    "female (default), male, british female / british male, or a voice name (Kokoro: af_heart, af_bella, af_nicole, am_michael, am_fenrir, am_puck, bf_emma, bm_george…)",
                ),
                (
                    "tone",
                    "string",
                    "e.g. calm and warm, excited (sets the pace of the local voice)",
                ),
                (
                    "engine",
                    "enum:auto|kokoro|openai|gemini|system",
                    "auto (default): the free local Kokoro voice on Apple silicon, else a cloud voice with a saved key",
                ),
                ("captions", "boolean", "Subtitles (default true)"),
                (
                    "caption_style",
                    "enum:bold|box|yellow|clean",
                    "Default bold",
                ),
            ])
        },
        run: add_voiceover,
    },
    Tool {
        name: "add_music",
        tier: Tier::Core,
        label: "Music",
        description: "A music bed from the library under the video (or start–end), faded in and out, low under a voiceover. mood: chill, upbeat, cinematic, corporate, electronic, acoustic, or search words. Replaces the agent's last bed.",
        params: || {
            schema(&[
                (
                    "mood",
                    "string",
                    "chill, upbeat, cinematic, corporate, electronic, acoustic",
                ),
                ("search", "string", "Words to search instead of a mood"),
                ("start", "number", "Seconds (default 0)"),
                ("end", "number", "Seconds (default: the end)"),
                (
                    "volume",
                    "number",
                    "Percent (default 13 under a voice, else 60)",
                ),
            ])
        },
        run: add_music,
    },
    Tool {
        name: "add_sticker",
        tier: Tier::Full,
        label: "Sticker",
        description: "An illustration from the sticker library (search words: \"coffee cup\", \"money bag\", \"thumbs up\") at a time and spot, popping in. size: share of the canvas width (0.35).",
        params: || {
            schema(&[
                ("search!", "string", "What to show"),
                ("start", "number", "Seconds (default: playhead)"),
                ("duration", "number", "Seconds (default 3)"),
                (
                    "position",
                    "string",
                    "center, top, bottom, left, right, top_left… or \"x,y\" 0..1",
                ),
                ("size", "number", "0.05–1.2 canvas widths (default 0.35)"),
                ("motion", "string", "Entrance (default pop_in), or none"),
                ("gif", "boolean", "An animated GIF instead of a sticker"),
            ])
        },
        run: add_sticker,
    },
    Tool {
        name: "add_scene",
        tier: Tier::Full,
        label: "Scene",
        description: "Draws a motion-graphic scene on the main lane: an animated backdrop (glow, grid, dots, beams), alone or with a growing bar_chart, a counter, a line_chart, a comparison (two columns), steps, a flow diagram (nodes lighting up in turn along animated connectors), code (an editor highlighting each step), or a canvas you compose from elements (words, numbers, icons, shapes, bars, cards, joining lines), each placed, timed and animated. Numbers count up; prefix/suffix mark them ($, %).",
        params: add_scene_params,
        run: add_scene,
    },
    Tool {
        name: "add_captions",
        tier: Tier::Full,
        label: "Captions",
        description: "(Re)captions a voiceover clip the agent made, timed to its words, in a caption style (bold, box, yellow, clean).",
        params: || {
            schema(&[
                ("clip", "string", "The voiceover clip (default: selected)"),
                ("style", "enum:bold|box|yellow|clean", "Default bold"),
            ])
        },
        run: add_captions,
    },
];

#[cfg(test)]
mod tests {
    use super::super::test_env::Fixture;
    use super::*;
    use crate::agent::voice::{Engine, Measured};

    fn speech(len_s: f64) -> Speech {
        let len = (len_s * SECOND as f64) as Micros;
        Speech {
            path: PathBuf::from("/tmp/voice.aiff"),
            engine: Engine::System,
            voice: "Samantha".into(),
            shape: Measured {
                length: len,
                onset: 100_000,
                tail: len - 100_000,
                pauses: vec![],
            },
        }
    }

    #[test]
    fn reads_chart_rows_forgivingly() {
        let i = |v: Value| parse_item(&v).unwrap();
        assert_eq!(i(json!("Coffee: $4.50")).value, 4.5);
        assert_eq!(i(json!("Coffee: $4.50")).label, "Coffee");
        assert_eq!(i(json!("Month 1 = 1,500")).value, 1500.);
        assert_eq!(i(json!("Snacks 3")).label, "Snacks");
        assert_eq!(i(json!({ "label": "Rides", "value": "6" })).value, 6.);
        assert!(parse_item(&json!("nothing here")).is_none());
        let (v, prefix, _) = read_visual(&Args(
            json!({ "visual": "bar_chart", "items": ["Coffee: $4.50", "Snacks: $3"] })
                .as_object()
                .unwrap()
                .clone(),
        ))
        .unwrap();
        assert_eq!(prefix, "$");
        assert!(matches!(v, Visual::Bars { ref items, .. } if items.len() == 2));
    }

    #[test]
    fn a_bad_visual_says_what_it_needs() {
        let a = |v: Value| Args(v.as_object().unwrap().clone());
        let e = read_visual(&a(json!({ "visual": "bar_chart" }))).unwrap_err();
        assert!(e.0.contains("items"), "{e:?}");
        let e = read_visual(&a(json!({ "visual": "pie" }))).unwrap_err();
        assert!(e.0.contains("bar_chart"), "{e:?}");
    }

    fn plan_for(f: &mut Fixture, args: Value) -> Plan {
        let layers = |_: Micros| vec![];
        let env = Env {
            project: f.project.clone(),
            aliases: &mut f.aliases,
            sounds: &mut f.sounds,
            board: &mut f.board,
            media: &[],
            playhead: 0,
            selected: None,
            effect_names: vec![],
            layers_at: &layers,
            vision: None,
            changed: None,
            magnet: false,
            seek_to: None,
            select: None,
            library_add: vec![],
        };
        read_plan(&Args(args.as_object().unwrap().clone()), &env).unwrap()
    }

    #[test]
    fn makes_a_whole_timeline() {
        let mut f = Fixture::new(Project::empty());
        f.project.canvas.width = 1920;
        f.project.canvas.height = 1080;
        let plan = plan_for(
            &mut f,
            json!({
                "title": "Money habits",
                "music": "none",
                "scenes": [
                    { "narration": "Do you know where your money goes every day?", "title": "Where does it go?", "stickers": ["wallet"] },
                    { "narration": "Small buys add up.", "title": "It adds up", "visual": "bar_chart", "items": ["Coffee: $4.50", "Snacks: $3"] },
                    { "narration": "Save a little every day.", "visual": "counter", "to": 1825, "prefix": "$" }
                ]
            }),
        );
        assert_eq!(plan.canvas, (1080, 1920), "an empty project goes vertical");
        let voices = vec![speech(3.), speech(1.5), speech(2.)];
        let lengths = scene_lengths(&plan, &voices);
        assert!(lengths[1] >= MIN_SCENE);
        assert!(lengths[0] > 3 * SECOND);
        let built = Built {
            scenes: vec![],
            voices,
            lengths: lengths.clone(),
            files: vec![PathBuf::from("/tmp/a.mp4"); 3],
            frame: (1080, 1920),
            stickers: vec![
                vec![StickerItem {
                    id: "1".into(),
                    title: "Wallet".into(),
                    url: "https://example.com/w.gif".into(),
                    preview_url: String::new(),
                    width: 200,
                    height: 200,
                }],
                vec![],
                vec![],
            ],
            music: Some((
                PathBuf::from("/tmp/m.mp3"),
                "Song – Artist".into(),
                2 * SECOND,
            )),
            notes: vec![],
        };
        let layers = |_: Micros| vec![];
        let mut env = Env {
            project: f.project.clone(),
            aliases: &mut f.aliases,
            sounds: &mut f.sounds,
            board: &mut f.board,
            media: &[],
            playhead: 0,
            selected: None,
            effect_names: vec![],
            layers_at: &layers,
            vision: None,
            changed: None,
            magnet: false,
            seek_to: None,
            select: None,
            library_add: vec![],
        };
        let v = assemble(&mut env, &plan, &built).unwrap();
        let p = env.project.clone();
        assert_eq!((p.canvas.width, p.canvas.height), (1080, 1920));
        assert_eq!(p.name, "Money habits");
        let main = p.ordered_main_clips();
        assert_eq!(main.len(), 3);
        // Back to back.
        assert_eq!(main[1].timeline_start, main[0].end());
        let total: Micros = lengths.iter().sum();
        assert_eq!(main[2].end(), total);
        let all: Vec<&Clip> = p.tracks.iter().flat_map(|t| t.clips.iter()).collect();
        let voices: Vec<&&Clip> = all
            .iter()
            .filter(|c| c.extra.contains_key("voiceoverText"))
            .collect();
        assert_eq!(voices.len(), 3);
        assert!(voices.iter().all(|c| c.timeline_start >= 0));
        assert!(all.iter().any(|c| c.is_caption()));
        assert!(all.iter().any(|c| c.kind == "sticker"));
        // The 2 s track repeats under the whole video.
        let music: Vec<&&Clip> = all
            .iter()
            .filter(|c| c.extra.contains_key("agentMusic"))
            .collect();
        assert!(music.len() >= 3);
        let music_end = music.iter().map(|c| c.end()).max().unwrap();
        assert!(total - music_end < SECOND / 10, "{music_end} vs {total}");
        let last = music.iter().max_by_key(|c| c.end()).unwrap();
        assert!(last.audio_fade().1 > 0, "the bed fades out");
        assert!(
            all.iter().all(|c| c.extra.contains_key(TAG)),
            "everything is tagged"
        );
        assert_eq!(v["scenes"].as_array().unwrap().len(), 3);
        // What the agent makes passes its own check.
        let found = super::super::check::issues(&p, &mut Aliases::default()).0;
        assert!(found.is_empty(), "{found:?}");

        // Again: the first attempt is replaced, not stacked.
        let v = assemble(&mut env, &plan, &built).unwrap();
        assert!(v["replaced"].as_u64().unwrap() > 0);
        assert_eq!(env.project.ordered_main_clips().len(), 3);
        assert_eq!(v["start"], json!(0.0));
    }

    #[test]
    fn music_loops_and_fades() {
        let mut p = Project::empty();
        let ids = place_music(
            &mut p,
            Path::new("/tmp/m.mp3"),
            "Song",
            4 * SECOND,
            (0, 10 * SECOND),
            0.2,
            false,
        );
        assert_eq!(ids.len(), 3);
        let (_, last) = p.find_clip(&ids[2]).unwrap();
        assert_eq!(last.end(), 10 * SECOND);
        assert!(last.audio_fade().1 > 0);
        let (_, first) = p.find_clip(&ids[0]).unwrap();
        assert!(first.audio_fade().0 > 0);
    }

    #[test]
    fn positions_read_names_and_numbers() {
        assert_eq!(position(Some("top_right")).unwrap(), (0.27, -0.26));
        assert_eq!(position(Some("0.5,0.25")).unwrap(), (0., -0.25));
        assert!(position(Some("nowhere")).is_err());
    }

    #[test]
    fn music_moods_map_to_tags() {
        assert_eq!(music_query("Chill"), MusicQuery::Mood("chillout"));
        assert_eq!(
            music_query("lofi piano"),
            MusicQuery::Search("lofi piano".into())
        );
    }

    #[test]
    fn captions_follow_the_voice() {
        let mut p = Project::empty();
        let s = speech(3.);
        let text = "Do you know where your money goes every day? It adds up.";
        let id = place_voice(&mut p, &s, text, SECOND);
        let n = place_captions(&mut p, &s, text, &id, "bold").unwrap();
        assert_eq!(n, 3);
        let (_, v) = p.find_clip(&id).unwrap();
        assert_eq!(
            v.timeline_start + (s.shape.onset - v.source_start.unwrap()),
            SECOND
        );
        let caps: Vec<&Clip> = p
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.is_caption())
            .collect();
        assert_eq!(caps[0].timeline_start, SECOND);
        // Captioning again replaces.
        place_captions(&mut p, &s, text, &id, "box").unwrap();
        let again = p
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.is_caption())
            .count();
        assert_eq!(again, 3);
    }

    /// The whole thing for real (voices, drawing, stickers, music, sound
    /// effects: needs the network and the catalog config), saved to the
    /// projects folder as "Money habits (agent test)" to open in the app.
    #[test]
    #[ignore]
    fn make_video_for_real() {
        crate::decoder::init();
        let mut f = Fixture::new(Project::empty());
        let script = json!({
            "title": "Money habits (agent test)",
            "palette": "fresh",
            "music": "chill",
            "tone": "Friendly, clear and motivating, like a favourite teacher",
            "scenes": [
                { "narration": "Do you know where your money goes every day?", "title": "Where does your money go?", "stickers": ["wallet"] },
                { "narration": "A coffee here, a snack there, a ride across town. Each one feels tiny.", "title": "Small daily buys", "stickers": ["coffee cup", "shopping bag"] },
                { "narration": "But these small costs add up fast. Twelve dollars a day is over three hundred and sixty dollars a month.", "title": "It adds up", "visual": "bar_chart", "chart_title": "Per day", "items": ["Coffee: $4.50", "Snacks: $3", "Rides: $4.50"] },
                { "narration": "Needs keep your life running. Wants are nice to have. Knowing the difference is the first step.", "title": "Needs vs wants", "visual": "comparison", "left_items": ["Rent", "Groceries", "Transport"], "right_items": ["Daily latte", "New sneakers", "Extra streaming"] },
                { "narration": "Try this: write down every spend, set a daily limit, and save first, then spend.", "title": "A simple budget", "visual": "steps", "items": ["Write down every spend", "Set a daily limit", "Save first, spend after"] },
                { "narration": "Save just five dollars a day, and in one year you will have over eighteen hundred dollars.", "title": "$5 a day", "visual": "line_chart", "chart_title": "Saving $5 a day", "items": ["Month 1: 150", "Month 3: 450", "Month 6: 900", "Year 1: 1825"] },
                { "narration": "Small spending decisions today can create a better financial future tomorrow.", "title": "Small choices, big future", "visual": "counter", "to": 1825, "prefix": "$", "label": "saved in one year" }
            ]
        });
        let v = f.call("make_video", script).expect("make_video");
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
        let mut p = f.project.clone();
        p.name = "Money habits (agent test)".into();
        crate::projects::save(&mut p).unwrap();
        println!("saved {}", p.id);
        assert!(p.duration > 30 * SECOND);
    }

    /// A tech explainer like the reference clip, made for real (the local
    /// voice, network sounds and music): diagrams and code, nothing chosen
    /// but the words. Saved as "RAG explainer (agent test)".
    /// `cargo t make_explainer_for_real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn make_explainer_for_real() {
        crate::decoder::init();
        let mut f = Fixture::new(Project::empty());
        let script = json!({
            "title": "RAG explainer (agent test)",
            "music": "electronic",
            "sound_effects": "ui",
            "scenes": [
                { "narration": "On its own, a language model only knows its training data. Ask about your private docs and it guesses.", "title": "LLM vs RAG",
                  "visual": "flow", "chart_title": "guessing vs grounded in real data", "layout": "row",
                  "nodes": [
                    { "label": "Prompt", "detail": "your question", "note": "you ask about your own data" },
                    { "label": "LLM", "detail": "training data only", "note": "the model has never seen it" },
                    { "label": "Answer", "detail": "a confident guess", "note": "so it guesses, confidently" } ] },
                { "narration": "Retrieval changes that. The question is embedded, the closest chunks come back from your docs, they're merged into the prompt, and the model answers from those facts.", "title": "How RAG works",
                  "visual": "flow", "chart_title": "now the LLM answers from the real source",
                  "nodes": [
                    { "label": "Query", "detail": "\"What's our API rate limit?\"", "note": "the user asks a question" },
                    { "label": "Retriever", "detail": "semantic vector search", "note": "the query is embedded and searched" },
                    { "label": "Knowledge base", "detail": "vector store · your docs", "note": "top-k chunks come back from your docs" },
                    { "label": "Augment", "detail": "query + retrieved context", "note": "query and context merged into one prompt" },
                    { "label": "LLM", "detail": "generates the answer", "note": "the LLM answers from the retrieved facts" },
                    { "label": "Answer", "detail": "500 req/min (from docs)", "note": "a grounded answer, with its source" } ] },
                { "narration": "In code it's three steps. Retrieve the top chunks, put them in the prompt with the question, and let the model generate.", "title": "In Python",
                  "visual": "code", "file": "rag_pipeline.py", "chart_title": "the same pipeline, written as code",
                  "code": "# 1. RETRIEVE: search your vector database\ndocs = vector_store.similarity_search(query, k=3)\n\n# 2. AUGMENT: query + retrieved context\nprompt = f\"Use this context: {docs}\\nQuestion: {query}\"\n\n# 3. GENERATE: answer from your data\nanswer = llm.invoke(prompt)" },
                { "narration": "So the model brings the reasoning, and retrieval brings the facts. Smart model plus your data means far fewer made-up answers.", "title": "The takeaway",
                  "visual": "comparison", "left_title": "LLM alone", "left_items": ["Reasoning", "Language", "Frozen memory"], "right_title": "With RAG", "right_items": ["Live facts", "Your docs", "Cites sources"] }
            ]
        });
        let v = f.call("make_video", script).expect("make_video");
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
        assert_eq!(
            v["voice"].as_str().map(|s| s.ends_with("(kokoro)")),
            Some(true),
            "{v}"
        );
        let p = f.project.clone();
        let beats = p
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.extra.contains_key("agentBeats"))
            .count();
        assert!(beats >= 3, "the explainer scenes carry their beats");
        let sfx: Vec<String> = p
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter_map(|c| {
                c.extra
                    .get("agentSfx")
                    .map(|k| format!("{} @ {:.2}", k, c.timeline_start as f64 / 1e6))
            })
            .collect();
        println!("sound effects: {sfx:?}");
        let mut p = p;
        p.name = "RAG explainer (agent test)".into();
        crate::projects::save(&mut p).unwrap();
        println!("saved {}", p.id);
    }

    #[test]
    fn reads_flow_and_code_scenes() {
        let a = Args(
            json!({
                "visual": "flow",
                "layout": "snake",
                "nodes": [
                    { "label": "Query", "detail": "what the user asks", "note": "the user asks a question" },
                    "Retriever: semantic search | finds the closest chunks",
                    { "label": "LLM", "icon": "chip" }
                ]
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let (v, _, _) = read_visual(&a).unwrap();
        let Visual::Flow { nodes, layout, .. } = v else {
            panic!("{v:?}")
        };
        assert_eq!(layout, Layout::Snake);
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].icon, Icon::Chat);
        assert_eq!(nodes[1].icon, Icon::Search);
        assert_eq!(nodes[1].detail.as_deref(), Some("semantic search"));
        assert_eq!(nodes[1].note.as_deref(), Some("finds the closest chunks"));
        assert_eq!(nodes[2].icon, Icon::Chip);

        let code = "# 1. RETRIEVE: search the store\ndocs = store.search(q)\n\n# 2. GENERATE: answer\nanswer = llm(docs)";
        let a = Args(
            json!({ "visual": "code", "code": code })
                .as_object()
                .unwrap()
                .clone(),
        );
        let (v, _, _) = read_visual(&a).unwrap();
        let Visual::Code { lines, steps, .. } = v else {
            panic!("{v:?}")
        };
        assert_eq!(lines.len(), 5);
        assert_eq!(steps.len(), 2);
        assert_eq!(
            (steps[0].label.as_str(), steps[0].from, steps[0].to),
            ("Retrieve", 1, 2)
        );
        assert_eq!(steps[1].note.as_deref(), Some("answer"));
        assert_eq!((steps[1].from, steps[1].to), (4, 5));
        let a = Args(
            json!({ "visual": "code", "code": code, "steps": ["Find: 2 | the search", "Answer"] })
                .as_object()
                .unwrap()
                .clone(),
        );
        let (v, _, _) = read_visual(&a).unwrap();
        let Visual::Code { steps, .. } = v else {
            panic!()
        };
        assert_eq!((steps[0].from, steps[0].to), (2, 2));
        assert_eq!((steps[1].from, steps[1].to), (4, 5));
    }

    #[test]
    fn a_scene_can_be_composed() {
        let a = |v: Value| Args(v.as_object().unwrap().clone());
        let (v, _, _, notes) = read_visual_noted(&a(json!({
            "visual": "canvas",
            "arrange": "row",
            "elements": [
                { "icon": "bulb", "label": "Idea", "id": "a" },
                { "icon": "rocket", "label": "Launch", "id": "b" },
                { "line": ["a", "b"] },
                { "text": "Ship it", "in": "sideways" },
            ],
        })))
        .unwrap();
        let Visual::Canvas { elements, flow } = &v else {
            panic!("{v:?}")
        };
        assert_eq!((elements.len(), *flow), (4, Flow::Row));
        assert!(notes[0].contains("no entrance \"sideways\""), "{notes:?}");
        // Elements alone say canvas; make_video carries the notes by scene.
        let (v, _, _) = read_visual(&a(json!({ "elements": ["One", "Two"] }))).unwrap();
        assert_eq!(v.kind(), "canvas");
        let mut f = Fixture::new(Project::empty());
        let plan = plan_for(
            &mut f,
            json!({ "scenes": [
                { "narration": "First." },
                { "narration": "Second.", "elements": [{ "text": "Hi", "color": "plaid" }] },
            ] }),
        );
        assert_eq!(plan.scenes[1].visual.kind(), "canvas");
        assert!(plan.notes[0].starts_with("Scene 2: "), "{:?}", plan.notes);
        // make_video's own schema stays the short one.
        let params = make_video_params().to_string();
        assert!(!params.contains("\"elements\""));
        assert!(add_scene_params().to_string().contains("\"elements\""));
    }

    #[test]
    fn a_chart_with_only_from_and_to_counts_up() {
        let a =
            json!({ "visual": "line_chart", "from": 0, "to": 1825, "chart_title": "In a year" });
        let (v, _, _) = read_visual(&Args(a.as_object().unwrap().clone())).unwrap();
        assert_eq!(
            v,
            Visual::Counter {
                from: 0.,
                to: 1825.,
                label: Some("In a year".into())
            }
        );
    }
}
