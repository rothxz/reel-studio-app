//! `look`: real frames of the edit, described by a vision model. The frames
//! are laid out as one contact sheet (reading order, times listed in the
//! question) and the chat model reads only the answer, so a text-only
//! chat model can still "see", and pictures never fill its context.

use serde_json::{Value, json};

use super::{Args, Env, Finish, Outcome, Progress, Tier, Tool, ToolError, ToolResult, schema};
use crate::agent::llm;
use crate::agent::snapshot::{Kind, secs};
use crate::model::{Micros, SECOND};

const MAX_FRAMES: usize = 12;
const TILE_WIDTH: u32 = 320;

fn times_arg(args: &Args) -> ToolResult<Vec<Micros>> {
    let Some(v) = args.0.get("times") else {
        return Ok(vec![]);
    };
    let list: Vec<f64> = match v {
        Value::Array(items) => items
            .iter()
            .filter_map(|x| {
                x.as_f64()
                    .or_else(|| x.as_str().and_then(super::parse_number))
            })
            .collect(),
        Value::String(s) => s
            .split([',', ' '])
            .filter_map(super::parse_number)
            .collect(),
        Value::Number(n) => n.as_f64().into_iter().collect(),
        _ => vec![],
    };
    Ok(list
        .into_iter()
        .take(MAX_FRAMES)
        .map(|s| (s.max(0.) * 1e6) as Micros)
        .collect())
}

/// Evenly spread sample times over [from, to).
fn spread(from: Micros, to: Micros) -> Vec<Micros> {
    let span = (to - from).max(1);
    let n = ((span / (2 * SECOND)) as usize + 1).clamp(1, MAX_FRAMES);
    (0..n)
        .map(|i| from + span * (2 * i as i64 + 1) / (2 * n as i64))
        .collect()
}

fn look(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let Some(vision) = env.vision.clone() else {
        return Err("No model is set up to look at pictures (the user can pick one in the agent's settings). Carry on with what the request and the project tell you: don't stop here.".into());
    };
    let clip = match args.str("clip") {
        Some(r) => Some(
            env.aliases
                .resolve(Kind::Clip, &r)
                .ok_or_else(|| ToolError(format!("No clip \"{r}\"")))?,
        ),
        None => None,
    };
    let mut times = times_arg(args)?;
    if times.is_empty() {
        times = match &clip {
            Some(id) => {
                let (_, c) = env.clip(id)?;
                spread(c.timeline_start, c.end())
            }
            None => spread(0, env.project.duration),
        };
    }
    if env.project.duration <= 0 {
        return Err("The timeline is empty".into());
    }
    // What's on screen at each time: the clip asked about, else the
    // bottom picture (the main video), else the top one.
    let mut frames: Vec<(Micros, String, Micros)> = vec![];
    for t in times {
        let t = t.min(env.project.duration - 1);
        let layers = (env.layers_at)(t);
        let pick = match &clip {
            Some(id) => layers.iter().find(|l| &l.clip_id == id),
            None => layers
                .iter()
                .find(|l| l.is_visual && env.is_main(&l.clip_id))
                .or_else(|| layers.iter().find(|l| l.is_visual)),
        };
        if let Some(l) = pick
            && l.is_visual
        {
            frames.push((t, l.source.clone(), l.source_time));
        }
    }
    if frames.is_empty() {
        return Err("There's no video or photo on screen at those times".into());
    }
    let question = args.str("question").unwrap_or_default();
    Ok(Outcome::Later(Box::new(move |progress: &Progress| {
        let mut tiles = vec![];
        for (i, (t, path, src)) in frames.iter().enumerate() {
            progress.check()?;
            progress.set(format!("Reading frames {}/{}", i + 1, frames.len()));
            match crate::decoder::thumbnail(path, *src, TILE_WIDTH) {
                Ok(f) => tiles.push((*t, f)),
                Err(e) => eprintln!("look: frame at {t}: {e:#}"),
            }
        }
        if tiles.is_empty() {
            return Err("Couldn't read any frames".into());
        }
        let jpeg = contact_sheet(&tiles).map_err(|e| ToolError(format!("{e:#}")))?;
        let listed: Vec<String> = tiles
            .iter()
            .enumerate()
            .map(|(i, (t, _))| format!("{}={}s", i + 1, secs(*t)))
            .collect();
        let prompt = format!(
            "These are {} frames from a video, laid out in reading order (left to right, then top to bottom). Their times: {}.\n\
             Describe each frame in one short line, starting with its time: who or what is in it, what's happening, the setting, and any readable on-screen text. \
             Then say in one line what happens over the whole sequence.{}",
            tiles.len(),
            listed.join(", "),
            if question.is_empty() {
                String::new()
            } else {
                format!("\nAlso answer: {question}")
            }
        );
        progress.set("Looking");
        let seen = llm::ask_about_image(&vision, &prompt, &jpeg)
            .map_err(|e| ToolError(format!("The vision model couldn't answer: {e:#}")))?;
        let times: Vec<f64> = tiles.iter().map(|(t, _)| secs(*t)).collect();
        let finish: Finish =
            Box::new(move |_env: &mut Env| Ok(json!({ "frames": times, "seen": seen })));
        Ok(finish)
    })))
}

/// The frames on one sheet (up to 4 across), as JPEG.
fn contact_sheet(tiles: &[(Micros, crate::decoder::DecodedFrame)]) -> anyhow::Result<Vec<u8>> {
    let cols = match tiles.len() {
        1 => 1,
        2 | 4 => 2,
        3 | 5 | 6 | 9 => 3,
        _ => 4,
    };
    let rows = tiles.len().div_ceil(cols);
    let tw = tiles
        .iter()
        .map(|(_, f)| f.width)
        .max()
        .unwrap_or(TILE_WIDTH);
    let th = tiles
        .iter()
        .map(|(_, f)| f.height)
        .max()
        .unwrap_or(TILE_WIDTH);
    let gap = 6;
    let w = cols as u32 * tw + (cols as u32 - 1) * gap;
    let h = rows as u32 * th + (rows as u32 - 1) * gap;
    let mut sheet = image::RgbImage::from_pixel(w, h, image::Rgb([24, 24, 24]));
    for (i, (_, f)) in tiles.iter().enumerate() {
        let (cx, cy) = ((i % cols) as u32, (i / cols) as u32);
        let (ox, oy) = (cx * (tw + gap), cy * (th + gap));
        for y in 0..f.height {
            for x in 0..f.width {
                let p = ((y * f.width + x) * 4) as usize;
                if let Some(px) = f.bgra.get(p..p + 3) {
                    sheet.put_pixel(ox + x, oy + y, image::Rgb([px[2], px[1], px[0]]));
                }
            }
        }
    }
    let mut out = vec![];
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80).encode_image(&sheet)?;
    Ok(out)
}

pub static TOOLS: &[Tool] = &[Tool {
    name: "look",
    tier: Tier::Core,
    label: "Look at the video",
    description: "See the actual video: real frames described (people, actions, setting, on-screen text), each with its timeline time. Use it whenever the request depends on what's IN the picture: what it shows, finding a moment, where to cut, a fitting title. With no times it scans the whole clip (or the whole video) every ~2 s; pass times to look closer.",
    params: || {
        schema(&[
            ("times", "array", "Timeline seconds to look at (up to 12)"),
            ("clip", "string", "Only this clip"),
            ("question", "string", "Something to answer about the frames"),
        ])
    },
    run: look,
}];

#[cfg(test)]
mod tests {
    #[test]
    fn spreads_samples() {
        assert_eq!(
            super::spread(0, 4_000_000),
            vec![666_666, 2_000_000, 3_333_333]
        );
        assert_eq!(super::spread(0, 100 * 1_000_000).len(), super::MAX_FRAMES);
    }
}
