//! Motion graphics in one call: `animate` puts a ready motion (zoom, pan,
//! pop, slide, shake…) on any clip as keyframes, and `add_title` builds an
//! animated title (pop, slide up, typewriter, word by word, lower third,
//! headline, neon, countdown) out of text clips and keyframes. Both are
//! plain document edits the preview and export already play, and both
//! carry a small model a long way: one call instead of a dozen keyframes.

use serde_json::{Value, json};

use super::edit::{apply_text_style, set_easing_at};
use super::{Args, Env, Outcome, Tier, Tool, ToolError, ToolResult, schema};
use crate::agent::snapshot::secs;
use crate::model::{Clip, Micros, SECOND};

pub const MOTIONS: &[(&str, &str)] = &[
    ("zoom_in", "slow push in over the clip (Ken Burns)"),
    ("zoom_out", "slow pull back"),
    ("pan_left", "drift towards the left"),
    ("pan_right", "drift towards the right"),
    ("pan_up", "drift up"),
    ("pan_down", "drift down"),
    ("fade_in", "fades in at its start"),
    ("fade_out", "fades out at its end"),
    ("pop_in", "springs in from small at its start"),
    ("pop_out", "shrinks away at its end"),
    ("slide_in_left", "slides in from the left"),
    ("slide_in_right", "slides in from the right"),
    ("slide_in_up", "rises in from below"),
    ("slide_in_down", "drops in from above"),
    ("slide_out_left", "leaves to the left at its end"),
    ("slide_out_right", "leaves to the right at its end"),
    ("shake", "a camera shake (impact) at a time"),
    ("spin", "one full turn"),
    ("pulse", "a quick beat-like bump at a time"),
];

pub const TITLE_STYLES: &[(&str, &str)] = &[
    ("pop", "bold title that springs in and fades out"),
    ("slide_up", "rises into place, clean and calm"),
    ("typewriter", "letters appear one after another"),
    (
        "word_by_word",
        "big words one at a time, kinetic-type reel style",
    ),
    (
        "lower_third",
        "name / label bar near the bottom, slides in from the left",
    ),
    ("headline", "huge outlined title that settles in"),
    ("neon", "glowing text that flickers on"),
    ("countdown", "3, 2, 1 (text = the number to start from)"),
];

/// Keys transform values at clip-local times: (local, [(property, value)], easing).
type Keys = Vec<(Micros, Vec<(&'static str, f64)>, &'static str)>;

fn apply_keys(clip: &mut Clip, keys: &Keys, eps: Micros) {
    for (t, values, ease) in keys {
        let t = (*t).clamp(0, clip.timeline_duration);
        let values: Vec<(String, f64)> = values.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        clip.add_keyframe_at(t, eps, &values);
        set_easing_at(clip, t, eps, ease);
    }
}

/// The keyframes of a motion, given the clip's resting values.
fn motion_keys(
    motion: &str,
    clip: &Clip,
    at: Micros,
    len: Micros,
    strength: f64,
) -> ToolResult<Keys> {
    let base = |p: &str| {
        clip.static_property(p)
            .unwrap_or(if p.ends_with("scale") || p.ends_with("opacity") {
                1.
            } else {
                0.
            })
    };
    let (x, y, s, r, o) = (
        base("transform.x"),
        base("transform.y"),
        base("transform.scale"),
        base("transform.rotation"),
        base("transform.opacity"),
    );
    let end = (at + len).min(clip.timeline_duration);
    let d = clip.timeline_duration;
    let k = strength.clamp(0.1, 3.);
    let pan = 0.06 * k;
    let pan_scale = s * (1. + 0.12 * k).max(1.);
    let keys: Keys = match motion {
        "zoom_in" => vec![
            (at, vec![("transform.scale", s)], "easeInOut"),
            (end, vec![("transform.scale", s * (1. + 0.2 * k))], "linear"),
        ],
        "zoom_out" => vec![
            (
                at,
                vec![("transform.scale", s * (1. + 0.2 * k))],
                "easeInOut",
            ),
            (end, vec![("transform.scale", s)], "linear"),
        ],
        "pan_left" | "pan_right" => {
            let dir = if motion == "pan_left" { -1. } else { 1. };
            vec![
                (
                    at,
                    vec![
                        ("transform.x", x + pan * dir),
                        ("transform.scale", pan_scale),
                    ],
                    "easeInOut",
                ),
                (
                    end,
                    vec![
                        ("transform.x", x - pan * dir),
                        ("transform.scale", pan_scale),
                    ],
                    "linear",
                ),
            ]
        }
        "pan_up" | "pan_down" => {
            let dir = if motion == "pan_up" { -1. } else { 1. };
            vec![
                (
                    at,
                    vec![
                        ("transform.y", y + pan * dir),
                        ("transform.scale", pan_scale),
                    ],
                    "easeInOut",
                ),
                (
                    end,
                    vec![
                        ("transform.y", y - pan * dir),
                        ("transform.scale", pan_scale),
                    ],
                    "linear",
                ),
            ]
        }
        "fade_in" => {
            let f = len.min(d / 2);
            vec![
                (0, vec![("transform.opacity", 0.)], "easeOut"),
                (f, vec![("transform.opacity", o)], "linear"),
            ]
        }
        "fade_out" => {
            let f = len.min(d / 2);
            vec![
                (d - f, vec![("transform.opacity", o)], "easeIn"),
                (d, vec![("transform.opacity", 0.)], "linear"),
            ]
        }
        "pop_in" => {
            let f = len.min(d / 2);
            vec![
                (
                    0,
                    vec![("transform.scale", s * 0.4), ("transform.opacity", 0.)],
                    "easeOut",
                ),
                (
                    f * 6 / 10,
                    vec![
                        ("transform.scale", s * (1. + 0.12 * k)),
                        ("transform.opacity", o),
                    ],
                    "easeInOut",
                ),
                (f, vec![("transform.scale", s)], "linear"),
            ]
        }
        "pop_out" => {
            let f = len.min(d / 2);
            vec![
                (
                    d - f,
                    vec![("transform.scale", s), ("transform.opacity", o)],
                    "easeInOut",
                ),
                (
                    d - f * 4 / 10,
                    vec![("transform.scale", s * (1. + 0.1 * k))],
                    "easeIn",
                ),
                (
                    d,
                    vec![("transform.scale", s * 0.3), ("transform.opacity", 0.)],
                    "linear",
                ),
            ]
        }
        "slide_in_left" | "slide_in_right" | "slide_in_up" | "slide_in_down" => {
            let f = len.min(d / 2);
            let (prop, rest, from) = match motion {
                "slide_in_left" => ("transform.x", x, x - 0.7),
                "slide_in_right" => ("transform.x", x, x + 0.7),
                "slide_in_up" => ("transform.y", y, y + 0.25),
                _ => ("transform.y", y, y - 0.25),
            };
            vec![
                (0, vec![(prop, from), ("transform.opacity", 0.)], "easeOut"),
                (f * 3 / 10, vec![("transform.opacity", o)], "easeOut"),
                (f, vec![(prop, rest)], "linear"),
            ]
        }
        "slide_out_left" | "slide_out_right" => {
            let f = len.min(d / 2);
            let to = if motion == "slide_out_left" {
                x - 0.7
            } else {
                x + 0.7
            };
            vec![
                (d - f, vec![("transform.x", x)], "easeIn"),
                (d, vec![("transform.x", to)], "linear"),
            ]
        }
        "shake" => {
            let mut keys: Keys = vec![(
                at,
                vec![
                    ("transform.x", x),
                    ("transform.y", y),
                    ("transform.scale", s * 1.06),
                ],
                "linear",
            )];
            let step = SECOND / 20;
            let amp = 0.018 * k;
            let mut t = at + step;
            let mut i = 0;
            while t < end {
                let fade = 1. - (t - at) as f64 / (end - at).max(1) as f64;
                let dx = if i % 2 == 0 { amp } else { -amp } * fade;
                let dy = if i % 3 == 0 { -amp * 0.7 } else { amp * 0.5 } * fade;
                keys.push((
                    t,
                    vec![("transform.x", x + dx), ("transform.y", y + dy)],
                    "linear",
                ));
                t += step;
                i += 1;
            }
            keys.push((
                end,
                vec![
                    ("transform.x", x),
                    ("transform.y", y),
                    ("transform.scale", s * 1.06),
                ],
                "linear",
            ));
            keys
        }
        "spin" => vec![
            (at, vec![("transform.rotation", r)], "easeInOut"),
            (end, vec![("transform.rotation", r + 360.)], "linear"),
        ],
        "pulse" => {
            let half = len.min(SECOND) / 2;
            vec![
                (at, vec![("transform.scale", s)], "easeOut"),
                (
                    at + half / 2,
                    vec![("transform.scale", s * (1. + 0.1 * k))],
                    "easeIn",
                ),
                ((at + half).min(d), vec![("transform.scale", s)], "linear"),
            ]
        }
        other => {
            return Err(ToolError(format!(
                "No motion \"{other}\". Pick one of: {}",
                MOTIONS
                    .iter()
                    .map(|(m, _)| *m)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    };
    Ok(keys)
}

fn animate(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let motion = args
        .required_str("motion")?
        .to_lowercase()
        .replace([' ', '-'], "_");
    let (_, clip) = env.clip(&id)?;
    if !(clip.is_visual() || matches!(clip.kind.as_str(), "text" | "sticker")) {
        return Err(format!("{} clips aren't on the picture", clip.kind).into());
    }
    let d = clip.timeline_duration;
    // Whole-clip motions default to the whole clip; the rest are short.
    let whole = motion.starts_with("zoom") || motion.starts_with("pan") || motion == "spin";
    let at = match args.time("time")? {
        Some(t) => (env.snap(t) - clip.timeline_start).clamp(0, d),
        None if motion == "shake" || motion == "pulse" => {
            (env.playhead - clip.timeline_start).clamp(0, d)
        }
        None => 0,
    };
    let len = args.time("duration")?.unwrap_or(if whole {
        d - at
    } else if motion == "shake" {
        SECOND / 2
    } else {
        SECOND * 4 / 10
    });
    let strength = args.num("strength")?.unwrap_or(1.);
    let clip = clip.clone();
    let keys = motion_keys(&motion, &clip, at, len.max(SECOND / 20), strength)?;
    let eps = env.frame() / 2;
    env.project.update_clip(&id, |c| {
        apply_keys(c, &keys, eps);
        true
    });
    env.edited("Animate");
    let start = clip.timeline_start;
    Ok(Outcome::Done(json!({
        "clip": env.alias(&id),
        "motion": motion,
        "from": secs(start + at),
        "to": secs(start + (at + len).min(d)),
    })))
}

// ---- titles -------------------------------------------------------------------------

/// A styled text clip at `at` for `len` (its id).
#[allow(clippy::too_many_arguments)]
fn text_clip(
    env: &mut Env,
    at: Micros,
    len: Micros,
    text: &str,
    y: f64,
    size: f64,
    args: &Args,
    extra: &[(&str, Value)],
) -> ToolResult<String> {
    let id = env.project.add_text_clip(at, text);
    let mut err = None;
    env.project.update_clip(&id, |c| {
        c.timeline_duration = len.max(SECOND / 20);
        c.extra.insert("fontSize".into(), json!(size));
        for (k, v) in extra {
            c.extra.insert((*k).into(), v.clone());
        }
        // The call's own look wins over the style's.
        if let Err(e) = apply_text_style(args, c) {
            err = Some(e);
        }
        c.set_properties_at(&[("transform.y", y - 0.5)], 0, false, 1);
        true
    });
    match err {
        Some(e) => Err(e),
        None => Ok(id),
    }
}

fn animate_clip(env: &mut Env, id: &str, motion: &str, len: Micros) -> ToolResult<()> {
    let clip = env.clip(id)?.1.clone();
    let keys = motion_keys(motion, &clip, 0, len, 1.)?;
    let eps = env.frame() / 2;
    env.project.update_clip(id, |c| {
        apply_keys(c, &keys, eps);
        true
    });
    Ok(())
}

fn add_title(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let text = args.required_str("text")?;
    // A subtitle only means something on a lower third: one given without a
    // style asks for it.
    let style = args
        .str("style")
        .unwrap_or_else(|| {
            if args.has("subtitle") {
                "lower_third"
            } else {
                "pop"
            }
            .into()
        })
        .to_lowercase()
        .replace([' ', '-'], "_");
    let at = env.snap(args.time("start")?.unwrap_or(env.playhead));
    let len = args.time("duration")?.unwrap_or(3 * SECOND).max(SECOND / 2);
    let y = match args
        .str("position")
        .as_deref()
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("top") => 0.18,
        Some("bottom") => 0.82,
        Some("center") | Some("middle") | None => {
            if style == "lower_third" {
                0.8
            } else {
                0.5
            }
        }
        Some(p) => super::parse_number(p)
            .ok_or_else(|| ToolError("position is top, center, bottom or a y 0..1".into()))?,
    };
    let size = args.num("size")?;
    // The text look comes from the title style; the call's colour / font win.
    let mut look = args.0.clone();
    look.remove("style");
    let args = &Args(look);
    let bold = [("bold", Value::Bool(true))];
    let outline = [
        ("bold", Value::Bool(true)),
        ("outlineColor", "#000000".into()),
        ("outlineWidth", json!(0.3)),
    ];
    let mut made = vec![];
    match style.as_str() {
        "pop" => {
            let id = text_clip(env, at, len, &text, y, size.unwrap_or(40.), args, &outline)?;
            animate_clip(env, &id, "pop_in", SECOND * 4 / 10)?;
            animate_clip(env, &id, "fade_out", SECOND * 3 / 10)?;
            made.push(id);
        }
        "slide_up" => {
            let id = text_clip(env, at, len, &text, y, size.unwrap_or(34.), args, &bold)?;
            animate_clip(env, &id, "slide_in_up", SECOND / 2)?;
            animate_clip(env, &id, "fade_out", SECOND * 3 / 10)?;
            made.push(id);
        }
        "headline" => {
            let extra = [
                ("bold", Value::Bool(true)),
                ("outlineColor", "#000000".into()),
                ("outlineWidth", json!(0.4)),
                ("shadowColor", "#000000".into()),
            ];
            let id = text_clip(
                env,
                at,
                len,
                &text.to_uppercase(),
                y,
                size.unwrap_or(56.),
                args,
                &extra,
            )?;
            let eps = env.frame() / 2;
            let keys: Keys = vec![
                (
                    0,
                    vec![("transform.scale", 1.35), ("transform.opacity", 0.)],
                    "easeOut",
                ),
                (
                    SECOND / 2,
                    vec![("transform.scale", 1.), ("transform.opacity", 1.)],
                    "linear",
                ),
            ];
            env.project.update_clip(&id, |c| {
                apply_keys(c, &keys, eps);
                true
            });
            animate_clip(env, &id, "fade_out", SECOND * 4 / 10)?;
            made.push(id);
        }
        "lower_third" => {
            let extra = [
                ("bold", Value::Bool(true)),
                ("backgroundColor", "#000000".into()),
                ("backgroundOpacity", json!(0.8)),
                ("backgroundRadius", json!(0.25)),
            ];
            let id = text_clip(env, at, len, &text, y, size.unwrap_or(22.), args, &extra)?;
            animate_clip(env, &id, "slide_in_left", SECOND / 2)?;
            animate_clip(env, &id, "slide_out_left", SECOND * 4 / 10)?;
            made.push(id);
            if let Some(sub) = args.str("subtitle") {
                let sub_args = Args(serde_json::Map::new());
                let sid = text_clip(
                    env,
                    at + SECOND / 5,
                    len - SECOND / 5,
                    &sub,
                    y + 0.055,
                    size.unwrap_or(22.) * 0.7,
                    &sub_args,
                    &[("color", "#dddddd".into())],
                )?;
                animate_clip(env, &sid, "slide_in_left", SECOND / 2)?;
                animate_clip(env, &sid, "slide_out_left", SECOND * 4 / 10)?;
                made.push(sid);
            }
        }
        "neon" => {
            let extra = [
                ("bold", Value::Bool(true)),
                ("color", "#ffffff".into()),
                ("glowColor", "#ff2d95".into()),
                ("glowIntensity", json!(1.0)),
                ("glowRange", json!(0.6)),
            ];
            let id = text_clip(env, at, len, &text, y, size.unwrap_or(40.), args, &extra)?;
            let eps = env.frame() / 2;
            let ms = |m: i64| m * 1_000;
            let keys: Keys = [
                (0, 0.),
                (60, 1.),
                (120, 0.2),
                (180, 1.),
                (280, 0.5),
                (360, 1.),
            ]
            .iter()
            .map(|&(t, o)| (ms(t), vec![("transform.opacity", o)], "hold"))
            .collect();
            env.project.update_clip(&id, |c| {
                apply_keys(c, &keys, eps);
                true
            });
            animate_clip(env, &id, "fade_out", SECOND * 3 / 10)?;
            made.push(id);
        }
        "typewriter" => {
            let chars: Vec<char> = text.chars().collect();
            // At most ~24 steps: by letters, or by words when it's long.
            let pieces: Vec<String> = if chars.len() <= 24 {
                (1..=chars.len())
                    .map(|n| chars[..n].iter().collect())
                    .collect()
            } else {
                let words: Vec<&str> = text.split_whitespace().collect();
                (1..=words.len()).map(|n| words[..n].join(" ")).collect()
            };
            let reveal = (len * 6 / 10)
                .min(SECOND / 12 * pieces.len() as i64)
                .max(SECOND / 3);
            let step = (reveal / pieces.len().max(1) as i64).max(env.frame());
            let n = pieces.len();
            for (i, piece) in pieces.iter().enumerate() {
                let start = at + step * i as i64;
                let piece_len = if i + 1 == n {
                    (at + len - start).max(step)
                } else {
                    step
                };
                let id = text_clip(
                    env,
                    start,
                    piece_len,
                    piece,
                    y,
                    size.unwrap_or(30.),
                    args,
                    &[
                        ("fontFamily", "IBM Plex Mono".into()),
                        ("alignment", "left".into()),
                    ],
                )?;
                made.push(id);
            }
            if let Some(last) = made.last().cloned() {
                animate_clip(env, &last, "fade_out", SECOND * 3 / 10)?;
            }
        }
        "word_by_word" | "kinetic" => {
            let words: Vec<&str> = text.split_whitespace().collect();
            if words.is_empty() {
                return Err("text is empty".into());
            }
            let each = (len / words.len() as i64).max(SECOND / 4);
            for (i, w) in words.iter().enumerate() {
                let id = text_clip(
                    env,
                    at + each * i as i64,
                    each,
                    &w.to_uppercase(),
                    y,
                    size.unwrap_or(52.),
                    args,
                    &outline,
                )?;
                let eps = env.frame() / 2;
                let keys: Keys = vec![
                    (0, vec![("transform.scale", 1.4)], "easeOut"),
                    (SECOND / 8, vec![("transform.scale", 1.)], "linear"),
                ];
                env.project.update_clip(&id, |c| {
                    apply_keys(c, &keys, eps);
                    true
                });
                made.push(id);
            }
        }
        "countdown" => {
            let from = super::parse_number(&text)
                .map(|n| n as i64)
                .unwrap_or(3)
                .clamp(1, 20);
            let each = SECOND.min(len / from).max(SECOND / 4);
            for (i, n) in (1..=from).rev().enumerate() {
                let id = text_clip(
                    env,
                    at + each * i as i64,
                    each,
                    &n.to_string(),
                    y,
                    size.unwrap_or(96.),
                    args,
                    &outline,
                )?;
                animate_clip(env, &id, "pop_in", SECOND * 3 / 10)?;
                made.push(id);
            }
        }
        other => {
            return Err(ToolError(format!(
                "No title style \"{other}\". Pick one of: {}",
                TITLE_STYLES
                    .iter()
                    .map(|(s, _)| *s)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }
    env.project.recompute_duration();
    env.edited("Add Title");
    let first = made.first().cloned();
    env.select = Some(first);
    let end = made
        .iter()
        .filter_map(|id| env.project.find_clip(id).map(|(_, c)| c.end()))
        .max()
        .unwrap_or(at + len);
    let ids: Vec<String> = made.iter().map(|id| env.alias(id)).collect();
    Ok(Outcome::Done(json!({
        "clips": ids,
        "style": style,
        "start": secs(at),
        "end": secs(end),
        "tip": "sound_design adds whooshes and pops to titles like this one",
    })))
}

pub static TOOLS: &[Tool] = &[
    Tool {
        name: "animate",
        tier: Tier::Core,
        label: "Animate",
        description: "Put a ready motion on a clip (video, photo, text, sticker) as keyframes: zoom_in, zoom_out, pan_left/right/up/down (whole clip), fade_in, fade_out, pop_in, pop_out, slide_in_left/right/up/down, slide_out_left/right, shake and pulse (at time), spin. strength 0.5 subtle … 2 strong.",
        params: || {
            schema(&[
                ("clip", "string", "Clip id (default: the selected clip)"),
                ("motion!", "string", "A motion name (list_options motions)"),
                (
                    "time",
                    "number",
                    "Timeline seconds where it starts (shake/pulse default: playhead)",
                ),
                ("duration", "number", "Seconds"),
                ("strength", "number", "1 = normal"),
            ])
        },
        run: animate,
    },
    Tool {
        name: "add_title",
        tier: Tier::Core,
        label: "Animated title",
        description: "An animated title (motion graphics) in one call. style: pop, slide_up, typewriter, word_by_word, lower_third (with subtitle), headline, neon, countdown. position top/center/bottom.",
        params: || {
            schema(&[
                ("text!", "string", "The words"),
                (
                    "style",
                    "enum:pop|slide_up|typewriter|word_by_word|lower_third|headline|neon|countdown",
                    "Default pop",
                ),
                ("start", "number", "Seconds (default: playhead)"),
                ("duration", "number", "Seconds (default 3)"),
                ("position", "string", "top, center, bottom (or y 0..1)"),
                ("subtitle", "string", "lower_third's second line"),
                ("color", "string", "#RRGGBB or a name"),
                ("size", "number", "Font size"),
                ("font", "string", "list_options fonts"),
            ])
        },
        run: add_title,
    },
];

#[cfg(test)]
mod tests {
    use super::super::test_env::Fixture;
    use serde_json::json;

    #[test]
    fn animate_keys_a_zoom_over_the_clip() {
        let mut f = Fixture::with_main_clips(1, 4.);
        let r = f
            .call("animate", json!({ "clip": "c1", "motion": "zoom in" }))
            .unwrap();
        assert_eq!(r["to"], 4.0);
        let id = f.id("c1");
        let c = f.project.find_clip(&id).unwrap().1;
        assert_eq!(c.keyframe_times(1), vec![0, 4_000_000]);
        assert_eq!(c.animated_properties(), vec!["transform.scale"]);
        assert!(
            f.call("animate", json!({ "clip": "c1", "motion": "wobble" }))
                .is_err()
        );
    }

    #[test]
    fn titles_build_text_clips() {
        let mut f = Fixture::with_main_clips(1, 6.);
        let r = f.call("add_title", json!({ "text": "Big news today", "style": "word_by_word", "start": 1, "duration": 3 })).unwrap();
        assert_eq!(r["clips"].as_array().unwrap().len(), 3);
        let r = f
            .call(
                "add_title",
                json!({ "text": "Hello", "style": "typewriter" }),
            )
            .unwrap();
        assert_eq!(r["clips"].as_array().unwrap().len(), 5);
        let r = f
            .call(
                "add_title",
                json!({ "text": "Ana", "subtitle": "Designer" }),
            )
            .unwrap();
        assert_eq!(r["style"], "lower_third");
        assert_eq!(r["clips"].as_array().unwrap().len(), 2);
        assert!(
            f.call("add_title", json!({ "text": "x", "style": "sparkle" }))
                .is_err()
        );
    }
}
