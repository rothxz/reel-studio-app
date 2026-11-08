//! The everyday timeline edits: read the project, move the playhead, cut,
//! trim, move, retime, sound, text, position, colour, effects, keyframes.

use serde_json::{Value, json};

use super::{Args, Env, Outcome, Tier, Tool, ToolError, ToolResult, pick_name, schema};
use crate::adjust;
use crate::agent::snapshot::{Kind, Lane, clip_entry, lane_of, secs};
use crate::catalog::{
    DEFAULT_FILTER_INTENSITY, EFFECT_PREFIX, FILTER_PREFIX, default_effect_params,
    default_parameters, filters_in,
};
use crate::model::{
    EffectInstance, MIN_EFFECT_CLIP, Micros, SECOND, Side, TrackType, nearest_seam_index,
};
use crate::text_render::FONTS;
use crate::visual::BLEND_MODES;

const CLIP: &str = "Clip id from get_project, e.g. \"c3\" (default: the selected clip)";
const MIN_CLIP: Micros = 100_000;

fn done(v: Value) -> ToolResult<Outcome> {
    Ok(Outcome::Done(v))
}

// ---- reading ------------------------------------------------------------------------

fn get_project(_: &Args, env: &mut Env) -> ToolResult<Outcome> {
    done(env.snapshot())
}

fn get_clip(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let (track, clip) = env.clip(&id)?;
    let lane = lane_of(&env.project, track).name();
    let clip = clip.clone();
    let mut out = clip_entry(&clip, env.aliases, env.media);
    out["lane"] = lane.into();
    let effects: Vec<Value> = clip
        .effects()
        .iter()
        .map(|e| json!({ "type": e.kind, "parameters": e.parameters }))
        .collect();
    if !effects.is_empty() {
        out["effectSettings"] = effects.into();
    }
    if let Some(k) = clip.extra.get("keyframes") {
        out["keyframeTracks"] = k.clone();
    }
    for key in [
        "fontFamily",
        "fontSize",
        "color",
        "bold",
        "italic",
        "alignment",
        "outlineColor",
        "outlineWidth",
        "backgroundColor",
        "shadowColor",
        "glowColor",
        "mask",
        "blendMode",
        "audioFade",
    ] {
        if let Some(v) = clip.extra.get(key) {
            out[key] = v.clone();
        }
    }
    done(out)
}

fn list_options(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let what = args.str("what").unwrap_or_default().to_lowercase();
    let search = args.str("search").map(|s| s.to_lowercase());
    let mut names: Vec<String> = match what.as_str() {
        "filters" => filters_in("all").iter().map(|f| f.name.clone()).collect(),
        "effects" => {
            if env.effect_names.len() < 40 {
                // Only a few seen yet: ask the catalog for all of them.
                let search = search.clone();
                return Ok(Outcome::Later(Box::new(move |_: &super::Progress| {
                    let all = crate::catalog::fetch_all_effects()
                        .map_err(|e| ToolError(format!("Couldn't load the effects: {e}")))?;
                    let names: Vec<String> = all
                        .into_iter()
                        .filter(|e| e.enabled)
                        .map(|e| e.name)
                        .collect();
                    let finish: super::Finish = Box::new(move |env: &mut Env| {
                        env.effect_names = names.clone();
                        Ok(filtered(names, search.as_deref()))
                    });
                    Ok(finish)
                })));
            }
            env.effect_names.clone()
        }
        "fonts" => FONTS.iter().map(|(_, l)| l.to_string()).collect(),
        "blend_modes" => BLEND_MODES.iter().map(|(id, _)| id.to_string()).collect(),
        "aspect_ratios" => crate::ui::player::ASPECT_PRESETS
            .iter()
            .map(|(l, _)| l.to_string())
            .collect(),
        "adjust" => adjust::CONTROLS.iter().map(|c| c.key.to_string()).collect(),
        "title_styles" => super::motion::TITLE_STYLES
            .iter()
            .map(|(k, d)| format!("{k}: {d}"))
            .collect(),
        "motions" => super::motion::MOTIONS
            .iter()
            .map(|(k, d)| format!("{k}: {d}"))
            .collect(),
        "sound_styles" => super::sound::STYLES
            .iter()
            .map(|(k, d)| format!("{k}: {d}"))
            .collect(),
        _ => {
            return Err(
                "\"what\" is one of: filters, effects, fonts, blend_modes, aspect_ratios, adjust, title_styles, motions, sound_styles"
                    .into(),
            );
        }
    };
    names.dedup();
    done(filtered(names, search.as_deref()))
}

fn filtered(names: Vec<String>, search: Option<&str>) -> Value {
    let list: Vec<String> = match search {
        Some(s) => names
            .into_iter()
            .filter(|n| n.to_lowercase().contains(s))
            .collect(),
        None => names,
    };
    let total = list.len();
    let shown: Vec<String> = list.into_iter().take(120).collect();
    if total > shown.len() {
        json!({ "options": shown, "more": total - 120, "hint": "pass search to narrow it" })
    } else {
        json!({ "options": shown })
    }
}

// ---- playhead + selection ------------------------------------------------------------

fn seek(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let t = args
        .time("time")?
        .ok_or_else(|| ToolError("\"time\" is required".into()))?;
    let t = env.snap(t.min(env.project.duration.max(0)));
    env.seek_to = Some(t);
    done(json!({ "playhead": secs(t) }))
}

fn select_clip(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    env.select = Some(Some(id.clone()));
    let alias = env.alias(&id);
    done(json!({ "selected": alias }))
}

// ---- cut, trim, move -------------------------------------------------------------------

fn split_clip(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let at = env.snap(args.time("time")?.unwrap_or(env.playhead));
    let id = match args.str("clip") {
        Some(_) => env.target_clip(args, "clip")?,
        None => match &env.selected {
            Some(s)
                if env
                    .project
                    .find_clip(s)
                    .is_some_and(|(_, c)| at > c.timeline_start && at < c.end()) =>
            {
                s.clone()
            }
            _ => env
                .main_clip_at(at)
                .ok_or_else(|| ToolError("There's no clip at that time".into()))?,
        },
    };
    let (start, end) = {
        let (_, clip) = env.clip(&id)?;
        (clip.timeline_start, clip.end())
    };
    if at <= start + MIN_CLIP / 2 || at >= end - MIN_CLIP / 2 {
        return Err(format!(
            "{} runs {}–{}s; split inside it",
            env.alias(&id),
            secs(start),
            secs(end)
        )
        .into());
    }
    let before: Vec<String> = env
        .project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(|c| c.id.clone()))
        .collect();
    if !env.project.split_at(&id, at) {
        return Err("Couldn't split there".into());
    }
    let right = env
        .project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .find(|c| !before.contains(&c.id))
        .map(|c| c.id.clone())
        .unwrap_or_default();
    env.edited("Split");
    let (l, r) = (env.alias(&id), env.alias(&right));
    done(json!({ "left": l, "right": r, "at": secs(at) }))
}

fn delete_clip(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let main = env.is_main(&id);
    env.project.delete_clip(&id).ok_or("That clip is gone")?;
    env.magnet = main;
    env.edited("Delete");
    if env.selected.as_deref() == Some(&id) {
        env.select = Some(None);
    }
    done(json!({ "deleted": env.aliases.alias(Kind::Clip, &id), "gapClosed": main }))
}

fn trim_clip(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let (_, clip) = env.clip(&id)?;
    let (start, end) = (clip.timeline_start, clip.end());
    let media = matches!(clip.kind.as_str(), "video" | "audio") && !clip.freeze();
    let new_start = args.time("start")?.map(|t| env.snap(t)).unwrap_or(start);
    let new_end = args.time("end")?.map(|t| env.snap(t)).unwrap_or(end);
    if new_end - new_start < MIN_CLIP {
        return Err("That leaves less than 0.1 s of the clip".into());
    }
    if media && (new_start < start || new_end > end) {
        return Err(format!(
            "A {} clip can only be shortened here (it runs {}–{}s). To show more of its file, use add_media again.",
            clip.kind,
            secs(start),
            secs(end)
        )
        .into());
    }
    let mut changed = false;
    if new_start != start {
        changed |= env.project.trim_edge(&id, Side::Start, end - new_start);
    }
    if new_end != end {
        let s = env.clip(&id)?.1.timeline_start;
        changed |= env.project.trim_edge(&id, Side::End, new_end - s);
    }
    if !changed {
        return Err("Nothing to change: that's its start and end already".into());
    }
    env.magnet = env.is_main(&id);
    env.edited("Trim");
    let (_, c) = env.clip(&id)?;
    let (s, e) = (secs(c.timeline_start), secs(c.end()));
    done(json!({ "clip": env.alias(&id), "start": s, "end": e }))
}

fn move_clip(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let (track_id, kind, length, clip_start, from_lane) = {
        let (track, clip) = env.clip(&id)?;
        (
            track.id.clone(),
            clip.kind.clone(),
            clip.timeline_duration,
            clip.timeline_start,
            lane_of(&env.project, track),
        )
    };
    let from_main = env.project.is_main_track(&track_id);
    let picture = matches!(kind.as_str(), "video" | "image");
    let start = env.snap(args.time("start")?.unwrap_or(clip_start));
    let lane = args.str("lane").map(|l| l.to_lowercase());
    let to_overlay = lane.as_deref() == Some("overlay");
    let to_main = lane.as_deref() == Some("main");

    if to_overlay && !picture {
        return Err("Only videos and photos go on overlay lanes".into());
    }
    if to_main && !picture {
        return Err("Only videos and photos go on the main lane".into());
    }
    let main_index = |env: &Env| {
        let others: Vec<_> = env
            .project
            .ordered_main_clips()
            .into_iter()
            .filter(|c| c.id != id)
            .collect();
        nearest_seam_index(&others, start)
    };
    let changed = if from_main && !to_overlay && lane.as_deref().is_none_or(|l| l == "main") {
        let index = main_index(env);
        env.project.reorder_main(&id, index)
    } else if from_main && to_overlay {
        let overlay = env.project.overlay_lane_for(start, start + length);
        env.project.move_out_of_main(&id, overlay, start)
    } else if to_main {
        let index = main_index(env);
        env.project.move_into_main(&id, index)
    } else {
        let target = match &lane {
            Some(l) if l == "overlay" => env.project.overlay_lane_for(start, start + length).id,
            Some(l) => env.lane(l)?,
            None => track_id.clone(),
        };
        if lane.is_some() && target != track_id {
            let to = env
                .project
                .tracks
                .iter()
                .find(|t| t.id == target)
                .map(|t| lane_of(&env.project, t))
                .unwrap_or(from_lane);
            let compatible =
                to == from_lane || (picture && matches!(to, Lane::Overlay | Lane::Main));
            if !compatible {
                return Err(format!("A {kind} clip can't go on the {} lane", to.name()).into());
            }
        }
        if target != track_id && !env.project.tracks.iter().any(|t| t.id == target) {
            let lane = env.project.overlay_lane_for(start, start + length);
            env.project.tracks.push(crate::model::Track {
                clips: vec![],
                ..lane
            });
        }
        env.project.move_clip(&id, &target, start)
    };
    if !changed {
        return Err("Nothing moved: it's there already".into());
    }
    env.magnet = true;
    env.edited("Move");
    let (track, c) = env.clip(&id)?;
    let lane = lane_of(&env.project, track).name();
    let (s, e) = (secs(c.timeline_start), secs(c.end()));
    done(json!({ "clip": env.alias(&id), "lane": lane, "start": s, "end": e }))
}

fn duplicate_clip(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let copy = env.project.duplicate_clip(&id).ok_or("That clip is gone")?;
    if let Some(at) = args.time("start")?
        && !env.is_main(&copy)
    {
        let track = env
            .project
            .track_of(&copy)
            .map(|t| t.id.clone())
            .unwrap_or_default();
        env.project.move_clip(&copy, &track, env.snap(at));
    }
    env.magnet = env.is_main(&id);
    env.edited("Duplicate");
    let (_, c) = env.clip(&copy)?;
    let (s, e) = (secs(c.timeline_start), secs(c.end()));
    done(json!({ "copy": env.alias(&copy), "start": s, "end": e }))
}

fn set_speed(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let speed = args
        .num("speed")?
        .ok_or_else(|| ToolError("\"speed\" is required".into()))?;
    if !(0.1..=100.).contains(&speed) {
        return Err("speed is 0.1–100 (1 = normal)".into());
    }
    let (_, c) = env.clip(&id)?;
    if !matches!(c.kind.as_str(), "video" | "audio") {
        return Err(format!("Speed works on video and audio clips, not {}", c.kind).into());
    }
    if !env.project.set_speed(&id, speed) {
        return Err("That's its speed already".into());
    }
    env.magnet = env.is_main(&id);
    env.edited("Speed");
    let (_, c) = env.clip(&id)?;
    let (s, e) = (secs(c.timeline_start), secs(c.end()));
    done(json!({ "clip": env.alias(&id), "speed": speed, "start": s, "end": e }))
}

fn set_audio(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let (_, c) = env.clip(&id)?;
    if !matches!(c.kind.as_str(), "video" | "audio") {
        return Err(format!("{} clips have no sound", c.kind).into());
    }
    let volume = args.num("volume")?;
    let muted = args.bool("muted")?;
    let fade_in = args.time("fade_in")?;
    let fade_out = args.time("fade_out")?;
    if volume.is_none() && muted.is_none() && fade_in.is_none() && fade_out.is_none() {
        return Err("Give volume, muted, fade_in or fade_out".into());
    }
    let length = c.timeline_duration;
    env.project.update_clip(&id, |c| {
        if let Some(v) = volume {
            c.extra
                .insert("volume".into(), json!((v / 100.).clamp(0., 10.)));
        }
        if let Some(m) = muted {
            c.set_flag("muted", m);
        }
        if fade_in.is_some() || fade_out.is_some() {
            let (i, o) = c.audio_fade();
            let i = fade_in.unwrap_or(i).min(length / 2);
            let o = fade_out.unwrap_or(o).min(length / 2);
            c.extra.insert(
                "audioFade".into(),
                json!({ "inDurationMicros": i, "outDurationMicros": o }),
            );
        }
        true
    });
    env.edited("Volume");
    let (_, c) = env.clip(&id)?;
    let (vol, m, (fi, fo)) = ((c.volume() * 100.).round(), c.muted(), c.audio_fade());
    done(
        json!({ "clip": env.alias(&id), "volume": vol, "muted": m, "fadeIn": secs(fi), "fadeOut": secs(fo) }),
    )
}

// ---- text ------------------------------------------------------------------------------

/// "#ff0", "#FFAA00", "red" → "#rrggbb".
pub fn color(s: &str) -> ToolResult<String> {
    let t = s.trim().to_lowercase();
    let named = match t.as_str() {
        "white" => Some("#ffffff"),
        "black" => Some("#000000"),
        "red" => Some("#ff3b30"),
        "orange" => Some("#ff9500"),
        "yellow" => Some("#ffd60a"),
        "green" => Some("#34c759"),
        "mint" => Some("#00c7be"),
        "cyan" => Some("#32ade6"),
        "blue" => Some("#0a84ff"),
        "purple" => Some("#bf5af2"),
        "pink" => Some("#ff2d55"),
        "magenta" => Some("#ff00ff"),
        "gray" | "grey" => Some("#8e8e93"),
        "gold" => Some("#ffcc33"),
        _ => None,
    };
    if let Some(n) = named {
        return Ok(n.into());
    }
    let hex = t.trim_start_matches('#');
    let hex = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 => hex.to_string(),
        8 => hex[..6].to_string(),
        _ => String::new(),
    };
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(format!("#{hex}"))
    } else {
        Err(format!("\"{s}\" isn't a colour: use #RRGGBB or a name like white, yellow").into())
    }
}

fn font(s: &str) -> ToolResult<&'static str> {
    let options: Vec<String> = FONTS.iter().map(|(_, l)| l.to_string()).collect();
    let label = pick_name(s, &options, "font")?;
    Ok(FONTS
        .iter()
        .find(|(_, l)| l == label)
        .map(|(f, _)| *f)
        .unwrap())
}

/// Sets the text look a call gives (add_text / edit_text / add_title).
pub fn apply_text_style(args: &Args, clip: &mut crate::model::Clip) -> ToolResult<()> {
    let e = &mut clip.extra;
    if let Some(style) = args.str("style") {
        match style.to_lowercase().as_str() {
            "plain" => {}
            "outline" | "bold_outline" => {
                e.insert("bold".into(), true.into());
                e.insert("outlineColor".into(), "#000000".into());
                e.insert("outlineWidth".into(), json!(0.35));
            }
            "box" | "label" => {
                e.insert("backgroundColor".into(), "#000000".into());
                e.insert("backgroundOpacity".into(), json!(0.75));
                e.insert("backgroundRadius".into(), json!(0.4));
            }
            "shadow" => {
                e.insert("shadowColor".into(), "#000000".into());
            }
            "neon" | "glow" => {
                e.insert("glowColor".into(), "#32ade6".into());
                e.insert("glowIntensity".into(), json!(0.9));
            }
            "caption" => {
                e.insert("bold".into(), true.into());
                e.insert("outlineColor".into(), "#000000".into());
                e.insert("outlineWidth".into(), json!(0.25));
                e.insert("shadowColor".into(), "#000000".into());
            }
            other => {
                return Err(format!(
                    "style \"{other}\" isn't one of: plain, outline, box, shadow, neon, caption"
                )
                .into());
            }
        }
    }
    if let Some(c) = args.str("color") {
        e.insert("color".into(), color(&c)?.into());
    }
    if let Some(s) = args.num("size")? {
        e.insert("fontSize".into(), json!(s.clamp(6., 200.)));
    }
    if let Some(f) = args.str("font") {
        e.insert("fontFamily".into(), font(&f)?.into());
    }
    if let Some(b) = args.bool("bold")? {
        e.insert("bold".into(), b.into());
    }
    if let Some(i) = args.bool("italic")? {
        e.insert("italic".into(), i.into());
    }
    if let Some(a) = args.str("align") {
        let a = a.to_lowercase();
        if !matches!(a.as_str(), "left" | "center" | "right") {
            return Err("align is left, center or right".into());
        }
        e.insert("alignment".into(), a.into());
    }
    for (arg, key) in [
        ("outline_color", "outlineColor"),
        ("background_color", "backgroundColor"),
        ("shadow_color", "shadowColor"),
        ("glow_color", "glowColor"),
    ] {
        if let Some(v) = args.str(arg) {
            if v.eq_ignore_ascii_case("none") {
                e.remove(key);
            } else {
                e.insert(key.into(), color(&v)?.into());
            }
        }
    }
    Ok(())
}

/// The transform fields a call gives, in the document's terms.
fn transform_patch(args: &Args) -> ToolResult<Vec<(&'static str, f64)>> {
    let mut patch = vec![];
    if let Some(x) = args.num("x")? {
        patch.push(("transform.x", x.clamp(-1., 2.) - 0.5));
    }
    if let Some(y) = args.num("y")? {
        patch.push(("transform.y", y.clamp(-1., 2.) - 0.5));
    }
    if let Some(s) = args.num("scale")? {
        patch.push(("transform.scale", s.clamp(0.01, 10.)));
    }
    if let Some(r) = args.num("rotation")? {
        patch.push(("transform.rotation", r));
    }
    if let Some(o) = args.num("opacity")? {
        // 0..1, or a percentage.
        patch.push((
            "transform.opacity",
            if o > 1. { o / 100. } else { o }.clamp(0., 1.),
        ));
    }
    Ok(patch)
}

fn add_text(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let text = args.required_str("text")?;
    let at = env.snap(args.time("start")?.unwrap_or(env.playhead));
    let duration = args.time("duration")?.unwrap_or(3 * SECOND).max(MIN_CLIP);
    let id = env.project.add_text_clip(at, &text);
    let mut style_error = None;
    let patch = transform_patch(args)?;
    env.project.update_clip(&id, |c| {
        c.timeline_duration = duration;
        if let Err(e) = apply_text_style(args, c) {
            style_error = Some(e);
        }
        for (k, v) in &patch {
            c.set_properties_at(&[(*k, *v)], 0, false, 1);
        }
        true
    });
    if let Some(e) = style_error {
        return Err(e);
    }
    env.project.recompute_duration();
    env.edited("Add Text");
    env.select = Some(Some(id.clone()));
    let (s, e) = (secs(at), secs(at + duration));
    done(json!({ "clip": env.alias(&id), "start": s, "end": e }))
}

fn edit_text(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let kind = env.clip(&id)?.1.kind.clone();
    if kind != "text" {
        return Err(format!("{} is a {kind} clip, not text", env.alias(&id)).into());
    }
    let mut result = Ok(());
    env.project.update_clip(&id, |c| {
        if let Some(t) = args.str("text") {
            c.text = Some(t);
        }
        result = apply_text_style(args, c);
        true
    });
    result?;
    env.edited("Edit Text");
    done(json!({ "clip": env.alias(&id), "ok": true }))
}

// ---- picture ---------------------------------------------------------------------------

fn set_transform(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let patch = transform_patch(args)?;
    if patch.is_empty() {
        return Err("Give x, y, scale, rotation or opacity".into());
    }
    let (_, c) = env.clip(&id)?;
    if !(c.is_visual() || matches!(c.kind.as_str(), "text" | "sticker")) {
        return Err(format!("{} clips aren't on the picture", c.kind).into());
    }
    let local = (env.playhead - c.timeline_start).clamp(0, c.timeline_duration);
    let eps = env.frame();
    env.project.update_clip(&id, |c| {
        c.set_properties_at(&patch, local, false, eps);
        true
    });
    env.edited("Transform");
    done(json!({ "clip": env.alias(&id), "ok": true }))
}

pub const EASINGS: [&str; 5] = ["linear", "easeIn", "easeOut", "easeInOut", "hold"];

fn easing(s: &str) -> ToolResult<&'static str> {
    let n: String = s
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    EASINGS
        .iter()
        .find(|e| e.to_lowercase() == n)
        .copied()
        .ok_or_else(|| ToolError(format!("easing is one of: {}", EASINGS.join(", "))))
}

/// Sets the easing of each keyframe at clip-local `t` (within `eps`).
pub fn set_easing_at(clip: &mut crate::model::Clip, t: Micros, eps: Micros, ease: &str) {
    if let Some(tracks) = clip
        .extra
        .get_mut("keyframes")
        .and_then(Value::as_array_mut)
    {
        for track in tracks {
            for k in track
                .get_mut("keyframes")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                if k.get("time")
                    .and_then(Value::as_f64)
                    .is_some_and(|kt| (kt as Micros - t).abs() <= eps)
                {
                    k["easing"] = ease.into();
                }
            }
        }
    }
}

fn add_keyframe(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let (_, c) = env.clip(&id)?;
    let at = env.snap(args.time("time")?.unwrap_or(env.playhead));
    if at < c.timeline_start || at > c.end() {
        return Err(format!(
            "time is on the timeline: this clip runs {}–{}s",
            secs(c.timeline_start),
            secs(c.end())
        )
        .into());
    }
    let local = at - c.timeline_start;
    let mut values: Vec<(String, f64)> = transform_patch(args)?
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    if let Some(v) = args.num("volume")? {
        values.push(("volume".into(), (v / 100.).clamp(0., 10.)));
    }
    if values.is_empty() {
        return Err("Give at least one of x, y, scale, rotation, opacity, volume".into());
    }
    let ease = args.str("easing").map(|e| easing(&e)).transpose()?;
    let eps = env.frame() / 2;
    env.project.update_clip(&id, |c| {
        c.add_keyframe_at(local, eps, &values);
        if let Some(e) = ease {
            set_easing_at(c, local, eps, e);
        }
        true
    });
    env.edited("Keyframe");
    done(
        json!({ "clip": env.alias(&id), "at": secs(at), "keyed": values.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>() }),
    )
}

fn remove_keyframes(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let (_, c) = env.clip(&id)?;
    let start = c.timeline_start;
    let eps = env.frame();
    let changed = match args.time("time")? {
        Some(t) => env
            .project
            .update_clip(&id, |c| c.delete_keyframes_at(t - start, eps)),
        None => env
            .project
            .update_clip(&id, |c| c.extra.remove("keyframes").is_some()),
    };
    if !changed {
        return Err("No keyframes there".into());
    }
    env.edited("Remove Keyframes");
    done(json!({ "clip": env.alias(&id), "ok": true }))
}

fn set_filter(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let (_, c) = env.clip(&id)?;
    if !c.is_visual() {
        return Err("Filters go on video and photo clips".into());
    }
    let name = args.required_str("name")?;
    if name.eq_ignore_ascii_case("none") {
        if !env.project.set_clip_effect(&id, FILTER_PREFIX, None) {
            return Err("It has no filter".into());
        }
        env.edited("Remove Filter");
        return done(json!({ "clip": env.alias(&id), "filter": null }));
    }
    let options: Vec<String> = filters_in("all").iter().map(|f| f.name.clone()).collect();
    if options.is_empty() {
        return Err("This build has no filters".into());
    }
    let picked = pick_name(&name, &options, "filter")?.clone();
    let intensity = args
        .num("intensity")?
        .unwrap_or(DEFAULT_FILTER_INTENSITY)
        .clamp(0., 100.);
    let effect = EffectInstance::new(
        format!("{FILTER_PREFIX}{picked}"),
        &[("intensity".into(), intensity)],
    );
    env.project
        .set_clip_effect(&id, FILTER_PREFIX, Some(effect));
    env.edited("Filter");
    done(json!({ "clip": env.alias(&id), "filter": picked, "intensity": intensity }))
}

fn adjust_color(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let (_, c) = env.clip(&id)?;
    if !c.is_visual() {
        return Err("Colour adjustments go on video and photo clips".into());
    }
    let mut set = vec![];
    for ctl in adjust::CONTROLS {
        if let Some(v) = args.num(ctl.key)? {
            set.push((ctl.key, v.clamp(ctl.min, ctl.max)));
        }
    }
    if set.is_empty() {
        return Err(format!(
            "Give one of: {}",
            adjust::CONTROLS
                .iter()
                .map(|c| c.key)
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into());
    }
    env.project.update_clip(&id, |c| {
        for (k, v) in &set {
            c.set_adjust_value(k, *v);
        }
        true
    });
    env.edited("Adjust");
    done(
        json!({ "clip": env.alias(&id), "set": set.iter().map(|(k, v)| json!({ *k: v })).collect::<Vec<_>>() }),
    )
}

fn add_effect(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let name = args.required_str("name")?;
    if name.eq_ignore_ascii_case("none") {
        let id = env.target_clip(args, "clip")?;
        if !env.project.set_clip_effect(&id, EFFECT_PREFIX, None) {
            return Err("It has no effect".into());
        }
        env.edited("Remove Effect");
        return done(json!({ "clip": env.alias(&id), "effect": null }));
    }
    let wants = name.clone();
    let known = env.effect_names.clone();
    let args_copy = Args(args.0.clone());
    match pick_name(&wants, &known, "effect") {
        Ok(picked) => place_effect(&args_copy, env, picked.clone(), None),
        // Not seen yet: ask the catalog for it.
        Err(_) => Ok(Outcome::Later(Box::new(move |_: &super::Progress| {
            let all = crate::catalog::fetch_all_effects()
                .map_err(|e| ToolError(format!("Couldn't load the effects: {e}")))?;
            let names: Vec<String> = all
                .iter()
                .filter(|e| e.enabled)
                .map(|e| e.name.clone())
                .collect();
            let picked = pick_name(&wants, &names, "effect")?.clone();
            let params = all
                .iter()
                .find(|e| e.name == picked)
                .map(|e| default_parameters(&e.params()));
            let finish: super::Finish = Box::new(move |env: &mut Env| {
                env.effect_names = names;
                match place_effect(&args_copy, env, picked, params)? {
                    Outcome::Done(v) => Ok(v),
                    Outcome::Later(_) => Err("Couldn't place the effect".into()),
                }
            });
            Ok(finish)
        }))),
    }
}

fn place_effect(
    args: &Args,
    env: &mut Env,
    name: String,
    params: Option<Vec<(String, f64)>>,
) -> ToolResult<Outcome> {
    let mut params = params.unwrap_or_else(|| default_parameters(&default_effect_params()));
    if let Some(i) = args.num("intensity")? {
        for (k, v) in params.iter_mut() {
            if k == "intensity" {
                *v = i.clamp(0., 100.);
            }
        }
    }
    let effect = EffectInstance::new(format!("{EFFECT_PREFIX}{name}"), &params);
    if args.has("clip") {
        let id = env.target_clip(args, "clip")?;
        let (_, c) = env.clip(&id)?;
        if !c.is_visual() {
            return Err(
                "Clip effects go on video and photo clips (leave clip out for the effect lane)"
                    .into(),
            );
        }
        env.project
            .set_clip_effect(&id, EFFECT_PREFIX, Some(effect));
        env.edited("Effect");
        return done(json!({ "clip": env.alias(&id), "effect": name }));
    }
    if env.project.duration <= 0 {
        return Err("Add a video first: an effect lane clip needs a picture under it".into());
    }
    let at = env.snap(args.time("start")?.unwrap_or(env.playhead));
    let id = env.project.add_effect_clip(at, effect);
    if let Some(d) = args.time("duration")? {
        let d = d.max(MIN_EFFECT_CLIP);
        env.project.update_clip(&id, |c| {
            c.timeline_duration = d;
            true
        });
        env.project.recompute_duration();
    }
    env.edited("Add Effect");
    let (_, c) = env.clip(&id)?;
    let (s, e) = (secs(c.timeline_start), secs(c.end()));
    done(json!({ "clip": env.alias(&id), "effect": name, "start": s, "end": e }))
}

fn set_blend(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let (_, c) = env.clip(&id)?;
    if !c.is_visual() {
        return Err("Blending is for video and photo clips".into());
    }
    let options: Vec<String> = BLEND_MODES.iter().map(|(m, _)| m.to_string()).collect();
    let mode = pick_name(&args.required_str("mode")?, &options, "blend mode")?.clone();
    env.project.update_clip(&id, |c| {
        c.set_blend_mode(&mode);
        true
    });
    env.edited("Blend");
    done(json!({ "clip": env.alias(&id), "blend": mode }))
}

fn set_aspect_ratio(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let r = args.required_str("ratio")?;
    let ratio = match r.split_once(':') {
        Some((w, h)) => {
            let (w, h) = (super::parse_number(w), super::parse_number(h));
            match (w, h) {
                (Some(w), Some(h)) if w > 0. && h > 0. => w / h,
                _ => return Err(format!("\"{r}\" isn't a ratio like 9:16").into()),
            }
        }
        None => super::parse_number(&r)
            .filter(|v| *v > 0.)
            .ok_or_else(|| ToolError(format!("\"{r}\" isn't a ratio like 9:16")))?,
    };
    let (w, h) = crate::ui::player::canvas_size_for_ratio(ratio);
    if (env.project.canvas.width, env.project.canvas.height) == (w, h) {
        return Err("It's that shape already".into());
    }
    env.project.canvas.width = w;
    env.project.canvas.height = h;
    env.edited("Change Ratio");
    done(json!({ "canvas": format!("{w}x{h}") }))
}

// ---- media, lanes, clip tools ------------------------------------------------------------

fn add_media(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let r = args.required_str("media")?;
    let path = env.aliases.resolve(Kind::Media, &r).ok_or_else(|| {
        ToolError(format!(
            "No media \"{r}\". Library ids (m1, m2…) are in get_project."
        ))
    })?;
    let mut item = env
        .media
        .iter()
        .find(|m| m.path == path)
        .cloned()
        .ok_or("That file left the library")?;
    let source_start = args.time("source_start")?.unwrap_or(0);
    if let Some(d) = args.time("duration")? {
        if item.still {
            item.duration = d.max(MIN_CLIP);
        } else {
            item.duration = d.min(item.duration - source_start).max(MIN_CLIP);
        }
    } else if !item.still {
        item.duration = (item.duration - source_start).max(MIN_CLIP);
    }
    if item.still && item.duration <= 0 {
        item.duration = 3 * SECOND;
    }
    let lane = args
        .str("lane")
        .unwrap_or_else(|| if item.has_video { "main" } else { "audio" }.into());
    let at = env.snap(args.time("start")?.unwrap_or(env.playhead));
    let id = if lane == "main" && item.has_video {
        let main = env.project.ordered_main_clips();
        let index = if args.has("start") || !main.is_empty() {
            nearest_seam_index(&main, at)
        } else {
            main.len()
        };
        env.project.insert_media(&item, Some(index))
    } else {
        env.project.insert_media_at(&item, at)
    };
    if source_start > 0 {
        env.project.update_clip(&id, |c| {
            c.source_start = Some(source_start);
            true
        });
    }
    env.magnet = lane == "main";
    env.edited("Add Media");
    let (track, c) = env.clip(&id)?;
    let lane = lane_of(&env.project, track).name();
    let (s, e) = (secs(c.timeline_start), secs(c.end()));
    done(json!({ "clip": env.alias(&id), "lane": lane, "start": s, "end": e }))
}

fn freeze_frame(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let at = env.snap(args.time("time")?.unwrap_or(env.playhead));
    let hold = args
        .time("duration")?
        .unwrap_or(crate::clip_edits::FREEZE_HOLD)
        .max(MIN_CLIP);
    let (_, c) = env.clip(&id)?;
    if at < c.timeline_start || at >= c.end() {
        return Err(format!(
            "time must be inside the clip ({}–{}s)",
            secs(c.timeline_start),
            secs(c.end())
        )
        .into());
    }
    let frozen = (env.layers_at)(at)
        .into_iter()
        .find(|l| l.clip_id == id)
        .map(|l| l.source_time)
        .unwrap_or_else(|| {
            c.source_start.unwrap_or(0) + ((at - c.timeline_start) as f64 * c.speed) as Micros
        });
    let still = env.project.freeze_frame(&id, at, frozen, hold)?;
    env.edited("Freeze Frame");
    done(json!({ "still": env.alias(&still), "from": secs(at), "to": secs(at + hold) }))
}

fn extract_audio(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let id = env.target_clip(args, "clip")?;
    let audio = env.project.extract_audio(&id)?;
    env.edited("Extract Audio");
    done(json!({ "audio": env.alias(&audio), "videoMuted": true }))
}

fn set_lane(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let lane = env.lane(&args.required_str("lane")?)?;
    let (locked, hidden, muted) = (
        args.bool("locked")?,
        args.bool("hidden")?,
        args.bool("muted")?,
    );
    if locked.is_none() && hidden.is_none() && muted.is_none() {
        return Err("Give locked, hidden or muted".into());
    }
    let track = env
        .project
        .tracks
        .iter_mut()
        .find(|t| t.id == lane)
        .ok_or("That lane is gone")?;
    if muted.is_some()
        && !matches!(
            track.kind,
            TrackType::Video | TrackType::Audio | TrackType::Overlay
        )
    {
        return Err("Only picture and audio lanes have sound".into());
    }
    track.locked = locked.unwrap_or(track.locked);
    track.hidden = hidden.unwrap_or(track.hidden);
    track.muted = muted.unwrap_or(track.muted);
    let (l, h, m) = (track.locked, track.hidden, track.muted);
    env.edited("Lane");
    done(
        json!({ "lane": env.aliases.alias(Kind::Lane, &lane), "locked": l, "hidden": h, "muted": m }),
    )
}

// ---- the table -------------------------------------------------------------------------

pub static TOOLS: &[Tool] = &[
    Tool {
        name: "get_project",
        tier: Tier::Core,
        label: "Read the timeline",
        description: "The current timeline: lanes with their clips (id, kind, start/end seconds, settings), playhead, selection, the media library.",
        params: || schema(&[]),
        run: get_project,
    },
    Tool {
        name: "get_clip",
        tier: Tier::Full,
        label: "Read clip",
        description: "Everything about one clip: timing, transform, effects with settings, keyframe tracks (clip-local micros), text style, fades.",
        params: || schema(&[("clip", "string", CLIP)]),
        run: get_clip,
    },
    Tool {
        name: "list_options",
        tier: Tier::Core,
        label: "Look up options",
        description: "Names other tools take: filters, effects, fonts, blend_modes, aspect_ratios, adjust, title_styles, motions, sound_styles. search narrows a long list.",
        params: || {
            schema(&[
                (
                    "what!",
                    "enum:filters|effects|fonts|blend_modes|aspect_ratios|adjust|title_styles|motions|sound_styles",
                    "Which list",
                ),
                ("search", "string", "Only names containing this"),
            ])
        },
        run: list_options,
    },
    Tool {
        name: "seek",
        tier: Tier::Core,
        label: "Move playhead",
        description: "Move the playhead to a time (seconds).",
        params: || schema(&[("time!", "number", "Seconds")]),
        run: seek,
    },
    Tool {
        name: "select_clip",
        tier: Tier::Core,
        label: "Select clip",
        description: "Select a clip so the user sees its settings.",
        params: || schema(&[("clip!", "string", CLIP)]),
        run: select_clip,
    },
    Tool {
        name: "split_clip",
        tier: Tier::Core,
        label: "Split",
        description: "Cut a clip in two at a timeline time (default playhead). Returns the left and right ids (the right half has a new id).",
        params: || {
            schema(&[
                ("clip", "string", CLIP),
                ("time", "number", "Seconds (default: playhead)"),
            ])
        },
        run: split_clip,
    },
    Tool {
        name: "delete_clip",
        tier: Tier::Core,
        label: "Delete",
        description: "Remove a clip. On the main lane the clips after it close the gap.",
        params: || schema(&[("clip!", "string", CLIP)]),
        run: delete_clip,
    },
    Tool {
        name: "trim_clip",
        tier: Tier::Core,
        label: "Trim",
        description: "Set a clip's new start and/or end on the timeline (seconds). Video/audio can only be shortened; text, stickers, photos and effects can also be lengthened.",
        params: || {
            schema(&[
                ("clip!", "string", CLIP),
                ("start", "number", "New start, seconds"),
                ("end", "number", "New end, seconds"),
            ])
        },
        run: trim_clip,
    },
    Tool {
        name: "move_clip",
        tier: Tier::Core,
        label: "Move",
        description: "Move a clip to start at a time. A main-lane clip is reordered to the nearest cut. lane moves it: \"overlay\" (picture-in-picture, a free overlay lane), \"main\", or a lane id.",
        params: || {
            schema(&[
                ("clip!", "string", CLIP),
                ("start", "number", "Seconds"),
                (
                    "lane",
                    "string",
                    "\"main\", \"overlay\" or a lane id like t3",
                ),
            ])
        },
        run: move_clip,
    },
    Tool {
        name: "duplicate_clip",
        tier: Tier::Full,
        label: "Duplicate",
        description: "Copy a clip with all its settings, right after it (on the main lane later clips move right; elsewhere start sets where the copy goes).",
        params: || {
            schema(&[
                ("clip!", "string", CLIP),
                ("start", "number", "Seconds (not on the main lane)"),
            ])
        },
        run: duplicate_clip,
    },
    Tool {
        name: "set_speed",
        tier: Tier::Core,
        label: "Speed",
        description: "Playback speed of a video or audio clip (0.1–100, 1 = normal). Changes its length.",
        params: || {
            schema(&[
                ("clip", "string", CLIP),
                ("speed!", "number", "2 = twice as fast, 0.5 = slow motion"),
            ])
        },
        run: set_speed,
    },
    Tool {
        name: "set_audio",
        tier: Tier::Core,
        label: "Volume",
        description: "Sound of a video or audio clip: volume percent (100 = original, 0–1000), muted, fade_in / fade_out seconds.",
        params: || {
            schema(&[
                ("clip", "string", CLIP),
                ("volume", "number", "Percent"),
                ("muted", "boolean", "Silence it"),
                ("fade_in", "number", "Seconds"),
                ("fade_out", "number", "Seconds"),
            ])
        },
        run: set_audio,
    },
    Tool {
        name: "add_text",
        tier: Tier::Core,
        label: "Add text",
        description: "Add a plain text clip (for an animated title use add_title). x/y = its centre, 0..1 across the frame (y 0 top, 1 bottom). size ~24 normal, 48 big. style: plain, outline, box, shadow, neon, caption.",
        params: || {
            schema(&[
                ("text!", "string", "The words"),
                ("start", "number", "Seconds (default: playhead)"),
                ("duration", "number", "Seconds (default 3)"),
                ("x", "number", "0..1, default 0.5"),
                ("y", "number", "0..1, default 0.5"),
                ("size", "number", "Font size"),
                ("color", "string", "#RRGGBB or a name"),
                ("font", "string", "list_options fonts"),
                ("bold", "boolean", ""),
                (
                    "style",
                    "enum:plain|outline|box|shadow|neon|caption",
                    "A ready look",
                ),
            ])
        },
        run: add_text,
    },
    Tool {
        name: "edit_text",
        tier: Tier::Core,
        label: "Edit text",
        description: "Change a text clip's words or look. *_color \"none\" removes that part.",
        params: || {
            schema(&[
                ("clip!", "string", CLIP),
                ("text", "string", "New words"),
                ("size", "number", "Font size"),
                ("color", "string", "#RRGGBB or a name"),
                ("font", "string", "list_options fonts"),
                ("bold", "boolean", ""),
                ("italic", "boolean", ""),
                ("align", "enum:left|center|right", ""),
                (
                    "style",
                    "enum:plain|outline|box|shadow|neon|caption",
                    "A ready look",
                ),
                ("outline_color", "string", ""),
                ("background_color", "string", ""),
                ("shadow_color", "string", ""),
                ("glow_color", "string", ""),
            ])
        },
        run: edit_text,
    },
    Tool {
        name: "set_transform",
        tier: Tier::Core,
        label: "Position & size",
        description: "Place a clip on the canvas: x/y = its centre, 0..1 across the frame (0.5 = middle), scale (1 = fit), rotation degrees, opacity 0..1. To animate, use animate or add_keyframe.",
        params: || {
            schema(&[
                ("clip", "string", CLIP),
                ("x", "number", ""),
                ("y", "number", ""),
                ("scale", "number", ""),
                ("rotation", "number", ""),
                ("opacity", "number", ""),
            ])
        },
        run: set_transform,
    },
    Tool {
        name: "add_keyframe",
        tier: Tier::Full,
        label: "Keyframe",
        description: "Key a clip's values AT a timeline time; between keyframes it moves smoothly. Call at two or more times to animate. x/y 0..1 across the frame, scale (1 = fit), rotation degrees, opacity 0..1, volume percent. easing shapes the motion leaving this keyframe.",
        params: || {
            schema(&[
                ("clip!", "string", CLIP),
                ("time!", "number", "Timeline seconds"),
                ("x", "number", ""),
                ("y", "number", ""),
                ("scale", "number", ""),
                ("rotation", "number", ""),
                ("opacity", "number", ""),
                ("volume", "number", ""),
                ("easing", "enum:linear|easeIn|easeOut|easeInOut|hold", ""),
            ])
        },
        run: add_keyframe,
    },
    Tool {
        name: "remove_keyframes",
        tier: Tier::Full,
        label: "Remove keyframes",
        description: "Remove a clip's keyframe at a timeline time, or with no time ALL its keyframes.",
        params: || {
            schema(&[
                ("clip!", "string", CLIP),
                ("time", "number", "Timeline seconds"),
            ])
        },
        run: remove_keyframes,
    },
    Tool {
        name: "set_filter",
        tier: Tier::Core,
        label: "Filter",
        description: "Colour filter on a video/photo clip by name (list_options filters), or \"none\". intensity 0–100.",
        params: || {
            schema(&[
                ("clip", "string", CLIP),
                ("name!", "string", "Filter name"),
                ("intensity", "number", "0–100"),
            ])
        },
        run: set_filter,
    },
    Tool {
        name: "adjust_color",
        tier: Tier::Core,
        label: "Adjust",
        description: "Colour sliders of a video/photo clip, each -100..100 (hue -180..180, 0 = untouched). Only the ones given change.",
        params: || {
            schema(&[
                ("clip", "string", CLIP),
                ("brightness", "number", ""),
                ("contrast", "number", ""),
                ("saturation", "number", ""),
                ("exposure", "number", ""),
                ("temperature", "number", "Warm > 0, cool < 0"),
                ("tint", "number", ""),
                ("hue", "number", ""),
            ])
        },
        run: adjust_color,
    },
    Tool {
        name: "add_effect",
        tier: Tier::Core,
        label: "Effect",
        description: "A video effect by name (list_options effects, e.g. \"Glitch\", \"Old TV\"). Without clip it goes on the effect lane over everything from start (default playhead) for duration (default 3s); with clip it applies to that video/photo. name \"none\" with clip removes it.",
        params: || {
            schema(&[
                ("name!", "string", "Effect name"),
                (
                    "clip",
                    "string",
                    "Only this clip (leave out for the effect lane)",
                ),
                ("start", "number", "Seconds"),
                ("duration", "number", "Seconds"),
                ("intensity", "number", "0–100"),
            ])
        },
        run: add_effect,
    },
    Tool {
        name: "set_blend",
        tier: Tier::Full,
        label: "Blend",
        description: "How a video/photo mixes with the layers under it (list_options blend_modes; \"normal\" covers them). For overlays like a light leak on \"lighten\".",
        params: || schema(&[("clip!", "string", CLIP), ("mode!", "string", "Blend mode")]),
        run: set_blend,
    },
    Tool {
        name: "set_aspect_ratio",
        tier: Tier::Core,
        label: "Ratio",
        description: "Canvas shape: \"9:16\" (vertical), \"16:9\", \"1:1\", \"4:3\", \"3:4\".",
        params: || schema(&[("ratio!", "string", "Like 9:16")]),
        run: set_aspect_ratio,
    },
    Tool {
        name: "add_media",
        tier: Tier::Full,
        label: "Add media",
        description: "Put a media-library file (m1, m2… in get_project) on the timeline. Videos/photos go on \"main\" (at the cut nearest start, later clips move right) or \"overlay\" (picture-in-picture at start); audio on the audio lane. source_start/duration take part of a file; duration sets a photo's length. You can't import new files: ask the user to.",
        params: || {
            schema(&[
                ("media!", "string", "Library id like m2"),
                ("lane", "enum:main|overlay|audio", ""),
                ("start", "number", "Seconds (default: playhead)"),
                ("source_start", "number", "Seconds into the file"),
                ("duration", "number", "Seconds"),
            ])
        },
        run: add_media,
    },
    Tool {
        name: "freeze_frame",
        tier: Tier::Full,
        label: "Freeze frame",
        description: "Hold a video's picture at a time (default playhead) for duration seconds (default 3), splitting the clip around it; later clips on the lane move right.",
        params: || {
            schema(&[
                ("clip", "string", CLIP),
                ("time", "number", "Seconds"),
                ("duration", "number", "Seconds"),
            ])
        },
        run: freeze_frame,
    },
    Tool {
        name: "extract_audio",
        tier: Tier::Full,
        label: "Extract audio",
        description: "Copy a video clip's sound onto the audio lane as its own clip and mute the video.",
        params: || schema(&[("clip!", "string", CLIP)]),
        run: extract_audio,
    },
    Tool {
        name: "set_lane",
        tier: Tier::Full,
        label: "Lane switches",
        description: "A lane's switches: locked (no hand edits), hidden (not shown or exported), muted (main, overlay and audio lanes).",
        params: || {
            schema(&[
                ("lane!", "string", "Lane id (t2) or kind (main, audio…)"),
                ("locked", "boolean", ""),
                ("hidden", "boolean", ""),
                ("muted", "boolean", ""),
            ])
        },
        run: set_lane,
    },
];

#[cfg(test)]
mod tests {
    use super::super::test_env::Fixture;
    use serde_json::json;

    #[test]
    fn split_returns_both_halves() {
        let mut f = Fixture::with_main_clips(2, 4.);
        let r = f
            .call("split_clip", json!({ "clip": "c1", "time": "2s" }))
            .unwrap();
        assert_eq!(r["left"], "c1");
        assert_eq!(r["right"], "c3");
        assert_eq!(f.project.ordered_main_clips().len(), 3);
        // Out of range is an error the model can read.
        let e = f
            .call("split_clip", json!({ "clip": "c2", "time": 1 }))
            .unwrap_err();
        assert!(e.0.contains("runs 4–8s"), "{e:?}");
    }

    #[test]
    fn delete_closes_the_main_gap() {
        let mut f = Fixture::with_main_clips(3, 2.);
        f.call("delete_clip", json!({ "clip": "c2" })).unwrap();
        let main = f.project.ordered_main_clips();
        assert_eq!(main.len(), 2);
        assert_eq!(main[1].timeline_start, 2_000_000);
    }

    #[test]
    fn video_trims_only_shorten() {
        let mut f = Fixture::with_main_clips(1, 5.);
        f.call("trim_clip", json!({ "clip": "c1", "end": 3 }))
            .unwrap();
        assert_eq!(f.project.duration, 3_000_000);
        assert!(
            f.call("trim_clip", json!({ "clip": "c1", "end": 9 }))
                .is_err()
        );
    }

    #[test]
    fn text_with_a_look() {
        let mut f = Fixture::with_main_clips(1, 5.);
        let r = f
            .call("add_text", json!({ "text": "Hi", "start": 1, "y": 0.8, "color": "yellow", "style": "outline" }))
            .unwrap();
        let id = f.id(r["clip"].as_str().unwrap());
        let (_, c) = f.project.find_clip(&id).unwrap();
        assert_eq!(c.extra["color"], "#ffd60a");
        assert_eq!(c.extra["outlineColor"], "#000000");
        assert!((c.transform().y - 0.3).abs() < 1e-9);
        assert!(
            f.call("edit_text", json!({ "clip": "c1", "text": "x" }))
                .is_err()
        );
    }

    #[test]
    fn keyframes_and_speed() {
        let mut f = Fixture::with_main_clips(1, 4.);
        f.call(
            "add_keyframe",
            json!({ "clip": "c1", "time": 0, "scale": 1 }),
        )
        .unwrap();
        f.call(
            "add_keyframe",
            json!({ "clip": "c1", "time": 4, "scale": 1.3, "easing": "ease-in-out" }),
        )
        .unwrap();
        let id = f.id("c1");
        assert_eq!(
            f.project.find_clip(&id).unwrap().1.keyframe_times(1).len(),
            2
        );
        f.call("set_speed", json!({ "clip": "c1", "speed": 2 }))
            .unwrap();
        assert_eq!(f.project.duration, 2_000_000);
    }

    #[test]
    fn unknown_tools_and_bad_args_are_errors() {
        let mut f = Fixture::with_main_clips(1, 4.);
        assert!(
            f.call("make_it_pop", json!({}))
                .unwrap_err()
                .0
                .contains("no tool")
        );
        assert!(
            f.call("set_speed", json!({ "clip": "c1", "speed": "fast" }))
                .is_err()
        );
        assert!(
            f.call("delete_clip", json!({ "clip": "c9" }))
                .unwrap_err()
                .0
                .contains("get_project")
        );
    }
}
