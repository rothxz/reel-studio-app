//! A video written scene by scene: the storyboard.
//!
//! `make_video` takes a whole script in one call, which caps a video at
//! what a model writes well in one reply (a small local model: three short
//! scenes, half a minute). Here the script is built up over many small
//! calls instead, so its length is the storyboard's, not the reply's:
//!
//! - `start_video` sets the video up (length, format, look, voice, music)
//!   and, when the model gives one, keeps its outline: a line per scene;
//! - `write_scene` writes one scene (or rewrites, or removes one), checks it
//!   and answers with what to fix and what comes next;
//! - `build_video` makes the video from the scenes written.
//!
//! Each answer says where the script stands (scenes, seconds of the
//! seconds asked) and names the next step, so a model that can't hold a
//! plan is led through it. The storyboard lasts as long as the chat:
//! "make scene 3 a chart" is one `write_scene` and a `build_video`.

use serde_json::{Map, Value, json};

use super::produce;
use super::{Args, Env, Outcome, Tier, Tool, ToolError, ToolResult};

/// Scenes a storyboard takes (about five minutes of video).
pub const MAX_SCENES: usize = 40;
/// Fixes one answer asks for at most, the most important first: a small
/// model handed a list of five stops instead of fixing any.
const MAX_FIXES: usize = 2;
/// A scene's narration, in words: under this it flashes by, over this it
/// sits on one picture too long.
const FEW_WORDS: usize = 8;
const MANY_WORDS: usize = 75;

#[derive(Clone, Debug, Default)]
pub struct Board {
    /// The whole video's settings, as `make_video` takes them.
    pub settings: Map<String, Value>,
    /// What each planned scene is about, a line each.
    pub outline: Vec<String>,
    pub scenes: Vec<Map<String, Value>>,
    /// `build_video` said once that the script runs short.
    warned: bool,
    pub built: bool,
}

fn narration(scene: &Map<String, Value>) -> String {
    ["narration", "voiceover", "text"]
        .iter()
        .find_map(|k| scene.get(*k).and_then(Value::as_str))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn words(s: &str) -> usize {
    s.split_whitespace().count()
}

fn round1(v: f64) -> f64 {
    (v * 10.).round() / 10.
}

fn visual_name(scene: &Map<String, Value>) -> String {
    produce::read_visual_noted(&Args(scene.clone()))
        .map_or("background", |(v, _, _, _)| v.kind())
        .to_string()
}

impl Board {
    /// Seconds asked for.
    pub fn target(&self) -> Option<f64> {
        Args(self.settings.clone())
            .num("duration")
            .ok()
            .flatten()
            .filter(|d| *d >= 5.)
    }

    /// Scenes the video is planned to have: the outline's, else what its
    /// length takes at about ten seconds a scene.
    pub fn planned(&self) -> usize {
        if !self.outline.is_empty() {
            return self.outline.len();
        }
        self.target()
            .map_or(0, |t| ((t / 10.).round() as usize).clamp(2, MAX_SCENES))
    }

    /// About how long the scenes written so far run.
    pub fn seconds(&self) -> f64 {
        self.scenes
            .iter()
            .map(|s| produce::spoken_secs(&narration(s)))
            .sum()
    }

    /// The storyboard in the project state, so a later request can change
    /// one scene.
    pub fn summary(&self) -> Value {
        let scenes: Vec<Value> = self
            .scenes
            .iter()
            .enumerate()
            .map(|(i, s)| {
                json!({
                    "scene": i + 1,
                    "title": s.get("title").and_then(Value::as_str).unwrap_or_default(),
                    "visual": visual_name(s),
                    "words": words(&narration(s)),
                })
            })
            .collect();
        let mut v = json!({
            "scenes": scenes,
            "seconds": round1(self.seconds()),
            "built": self.built,
        });
        if let Some(t) = self.target() {
            v["target"] = json!(t);
        }
        if self.outline.len() > self.scenes.len() {
            v["still_to_write"] = json!(self.outline[self.scenes.len()..]);
        }
        v
    }

    /// What to tell a model that stops while the video is still only a
    /// script: nothing is on the timeline until it is built.
    pub fn pending(&self) -> Option<String> {
        (!self.built && !self.scenes.is_empty()).then(|| {
            format!(
                "The video isn't made yet: the storyboard has {} scene{} written and nothing is on the timeline. Next: {}",
                self.scenes.len(),
                if self.scenes.len() == 1 { "" } else { "s" },
                self.next()
            )
        })
    }

    /// The step that comes next, as an instruction.
    fn next(&self) -> String {
        let n = self.scenes.len();
        let planned = self.planned();
        if let Some(about) = self.outline.get(n) {
            return format!("write_scene for scene {} of {planned}: \"{about}\"", n + 1);
        }
        if !self.outline.is_empty() {
            return format!("All {planned} scenes are written: call build_video now.");
        }
        match self.target() {
            Some(t) if self.seconds() < t * 0.9 && n < MAX_SCENES => format!(
                "About {:.0} s to go: write_scene for scene {} (then build_video once the script is complete).",
                t - self.seconds(),
                n + 1
            ),
            _ if n == 0 => "write_scene for scene 1: the hook.".into(),
            _ => {
                "write_scene for the next scene, or build_video when the script is complete.".into()
            }
        }
    }
}

/// An outline's lines: a list of strings, objects with a line in them, or
/// one string with a line per scene.
fn read_outline(v: Option<Value>) -> Vec<String> {
    let line = |s: &str| {
        s.trim()
            .trim_start_matches(|c: char| c.is_ascii_digit() || matches!(c, '.' | ')' | '-' | '*'))
            .trim()
            .to_string()
    };
    let lines: Vec<String> = match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(line(s)),
                Value::Object(o) => ["about", "title", "idea", "summary", "narration"]
                    .iter()
                    .find_map(|k| o.get(*k).and_then(Value::as_str))
                    .map(line),
                _ => None,
            })
            .collect(),
        Some(Value::String(s)) => s.lines().map(line).collect(),
        _ => vec![],
    };
    lines
        .into_iter()
        .filter(|l| !l.is_empty())
        .take(MAX_SCENES)
        .collect()
}

/// A canvas scene's layout, its words aside: which kinds sit where. Two
/// scenes with the same one look the same.
fn layout_of(scene: &Map<String, Value>) -> Option<String> {
    let elements = scene.get("elements")?.as_array()?;
    let kinds = ["text", "number", "icon", "shape", "bar", "card", "line"];
    let parts: Vec<String> = elements
        .iter()
        .map(|e| {
            let kind = kinds
                .iter()
                .find(|k| e.get(**k).is_some())
                .copied()
                .unwrap_or("text");
            format!(
                "{kind}@{}",
                e.get("at").map(Value::to_string).unwrap_or_default()
            )
        })
        .collect();
    (parts.len() >= 2).then(|| {
        format!(
            "{}|{}",
            scene
                .get("arrange")
                .and_then(Value::as_str)
                .unwrap_or("stack"),
            parts.join(",")
        )
    })
}

/// A suggestion, not a fault: the scene is fine as it is. (Asked as a fix,
/// a small model stopped altogether.)
fn tip(board: &Board, index: usize) -> Option<String> {
    let shape = layout_of(&board.scenes[index])?;
    (index >= 1 && layout_of(&board.scenes[index - 1]).as_ref() == Some(&shape)).then(|| {
        format!(
            "Scene {} is laid out like scene {index}. For the next scenes, vary the picture: arrange row or grid, a number, bars, cards or icons joined by lines.",
            index + 1
        )
    })
}

/// What a scene's script wants fixed (each line ends with its fix).
fn lint(board: &Board, index: usize) -> Vec<String> {
    let scene = &board.scenes[index];
    let n = index + 1;
    let mut fix = vec![];
    let count = words(&narration(scene));
    if count < FEW_WORDS {
        fix.push(format!(
            "Scene {n} says only {count} words (about {:.0} s on screen). Fix: write_scene again with \"scene\": {n} and 2–4 spoken sentences, 20–40 words.",
            produce::spoken_secs(&narration(scene))
        ));
    } else if count > MANY_WORDS {
        fix.push(format!(
            "Scene {n} says {count} words: about {:.0} s on one picture. Fix: split it into two scenes of 25–40 words.",
            produce::spoken_secs(&narration(scene))
        ));
    }
    let kind = visual_name(scene);
    let stickers = scene
        .get("stickers")
        .is_some_and(|s| s.as_array().is_some_and(|a| !a.is_empty()) || s.is_string());
    if kind == "background" && !stickers && scene.get("visual").is_none() {
        fix.push(format!(
            "Scene {n} has no picture, only the backdrop. Fix: give it a visual that shows what it says (canvas composes anything), or 1–2 stickers."
        ));
    }
    if index >= 2
        && kind != "canvas"
        && (index - 2..index).all(|k| visual_name(&board.scenes[k]) == kind)
    {
        fix.push(format!(
            "Three {kind} scenes in a row look alike. Fix: write_scene again with \"scene\": {n} and another visual."
        ));
    }
    if let Some(title) = scene.get("title").and_then(Value::as_str)
        && words(title) > 7
    {
        fix.push(format!(
            "Scene {n}'s title is {} words. Fix: the key point in 2–6 words; the narration says the rest.",
            words(title)
        ));
    }
    fix
}

fn start_video(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let mut settings = args.0.clone();
    let outline = read_outline(settings.remove("outline"));
    // Scenes handed over here already are scenes written.
    let scenes: Vec<Map<String, Value>> = match settings.remove("scenes") {
        Some(Value::Array(a)) => a
            .into_iter()
            .filter_map(|s| match s {
                Value::Object(o) if !narration(&o).is_empty() => Some(o),
                _ => None,
            })
            .take(MAX_SCENES)
            .collect(),
        _ => vec![],
    };
    produce::check_settings(&Args(settings.clone()), env)?;
    let board = Board {
        settings,
        outline,
        scenes,
        warned: false,
        built: false,
    };
    let mut v = json!({ "storyboard": "started" });
    if let Some(t) = board.target() {
        let planned = board.planned();
        v["target"] = json!(t);
        v["plan"] = format!(
            "{t:.0} s is about {:.0} spoken words: {planned} scenes of {:.0} words each.",
            t * 2.3,
            (t * 2.3 / planned.max(1) as f64).clamp(12., 45.)
        )
        .into();
    }
    if !board.outline.is_empty() {
        v["scenes_planned"] = board.outline.len().into();
    }
    if !board.scenes.is_empty() {
        v["scenes_written"] = board.scenes.len().into();
    }
    v["next"] = board.next().into();
    *env.board = Some(board);
    Ok(Outcome::Done(v))
}

fn write_scene(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let mut scene = args.0.clone();
    let at = ["scene", "number", "index"]
        .iter()
        .find_map(|k| scene.remove(*k))
        .and_then(|v| {
            Args(Map::from_iter([("n".to_string(), v)]))
                .num("n")
                .ok()
                .flatten()
        })
        .map(|n| n.max(1.) as usize);
    let remove = ["remove", "delete"]
        .iter()
        .any(|k| scene.remove(*k).is_some_and(|v| v == true || v == "true"));
    // A scene written before the video was set up starts one with the
    // defaults.
    let board = env.board.get_or_insert_with(Board::default);
    let count = board.scenes.len();
    if remove {
        let n = at.ok_or("\"scene\" is required to remove one: its number")?;
        if n > count {
            return Err(format!("There is no scene {n}: the storyboard has {count}.").into());
        }
        board.scenes.remove(n - 1);
        if n <= board.outline.len() {
            board.outline.remove(n - 1);
        }
        return Ok(Outcome::Done(json!({
            "removed": n,
            "scenes": board.scenes.len(),
            "total_seconds": round1(board.seconds()),
            "next": if board.built { "build_video to rebuild the video without it.".to_string() } else { board.next() },
        })));
    }
    if narration(&scene).is_empty() {
        return Err(
            "\"narration\" is required: what the narrator says in this scene, 2–4 spoken sentences."
                .into(),
        );
    }
    if at.is_none() && count >= MAX_SCENES {
        return Err(format!(
            "The storyboard is full ({MAX_SCENES} scenes). Fix: build_video, or rewrite a scene with \"scene\": its number."
        )
        .into());
    }
    let index = match at {
        Some(n) if n <= count => {
            board.scenes[n - 1] = scene.clone();
            n - 1
        }
        _ => {
            board.scenes.push(scene.clone());
            count
        }
    };
    let n = index + 1;
    // A visual that can't be read doesn't lose the scene: it says what to
    // send again.
    // The script's own faults first, then the picture's small ones. Only
    // what a second try would improve: the rest was put right while
    // reading, and a small model acts on every line it is given.
    let mut fix = vec![];
    let mut small = vec![];
    match produce::read_visual_noted(&Args(scene)) {
        Ok((_, _, _, notes)) => small.extend(notes.into_iter().filter(|n| n.contains("Fix:"))),
        Err(e) => fix.push(format!(
            "Scene {n}'s visual couldn't be read, so it would be a plain backdrop: {} Fix: write_scene again with \"scene\": {n}, the same narration and the visual's data.",
            e.0
        )),
    }
    fix.extend(lint(board, index));
    fix.extend(small);
    fix.truncate(MAX_FIXES);
    board.warned = false;
    let text = narration(&board.scenes[index]);
    let mut v = json!({
        "scene": n,
        "visual": visual_name(&board.scenes[index]),
        "words": words(&text),
        "seconds": round1(produce::spoken_secs(&text)),
        "scenes": board.scenes.len(),
        "total_seconds": round1(board.seconds()),
    });
    if let Some(t) = board.target() {
        v["target"] = json!(t);
    }
    if !fix.is_empty() {
        v["fix"] = fix.into();
    }
    if let Some(t) = tip(board, index) {
        v["tip"] = t.into();
    }
    v["next"] = if at.is_some_and(|a| a <= count) && board.built {
        "build_video to rebuild the video with this scene.".to_string()
    } else {
        board.next()
    }
    .into();
    Ok(Outcome::Done(v))
}

/// Whether the storyboard is ready to build.
fn ready(board: &mut Board, anyway: bool) -> ToolResult<()> {
    if board.scenes.is_empty() {
        return Err(format!("The storyboard has no scenes yet. Fix: {}", board.next()).into());
    }
    // A script well short of what was asked: said once, built on the
    // second ask (a model that can't write more must still get a video).
    let (made, planned) = (board.scenes.len(), board.planned());
    let short = match board.target() {
        Some(t) if board.seconds() < t * 0.7 => Some(format!(
            "The script runs about {:.0} s of the {t:.0} s asked.",
            board.seconds()
        )),
        _ if !board.outline.is_empty() && made < planned => Some(format!(
            "Only {made} of the {planned} planned scenes are written."
        )),
        _ => None,
    };
    if let Some(short) = short
        && !anyway
        && !board.warned
    {
        board.warned = true;
        return Err(ToolError(format!(
            "{short} Fix: {} To build it as it is, call build_video again.",
            board.next()
        )));
    }
    Ok(())
}

/// `build_video` without the building (the model evals: what a model
/// wrote is what's scored, not a minute of drawing).
#[cfg(test)]
pub fn dry_build(board: &mut Board) -> ToolResult {
    ready(board, false)?;
    board.built = true;
    Ok(json!({ "duration": round1(board.seconds()), "scenes": board.scenes.len() }))
}

fn build_video(args: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let anyway = ["anyway", "force"]
        .iter()
        .any(|k| args.bool(k).ok().flatten() == Some(true));
    let Some(board) = env.board.as_mut() else {
        return Err(
            "There is no storyboard yet. Fix: start_video, then write_scene for each scene, then build_video."
                .into(),
        );
    };
    ready(board, anyway)?;
    let mut a = board.settings.clone();
    a.insert(
        "scenes".into(),
        Value::Array(board.scenes.iter().cloned().map(Value::Object).collect()),
    );
    board.built = true;
    produce::make_video_of(
        &Args(a),
        env,
        MAX_SCENES,
        "Tell the user what you made, scene by scene in a line or two. To change one scene: write_scene with \"scene\": its number, then build_video again (it replaces this video).",
    )
}

fn start_params() -> Value {
    let mut props = produce::video_props();
    props.insert(
        "outline".into(),
        json!({
            "type": "array",
            "items": { "type": "string" },
            "description": "The plan: one short line per scene, in order (the hook first, the takeaway last). About one scene per 10 seconds."
        }),
    );
    json!({ "type": "object", "properties": props })
}

fn scene_params() -> Value {
    let mut props = produce::scene_props(true, true);
    props.insert(
        "scene".into(),
        json!({ "type": "number", "description": "Rewrite this scene (1 = the first). Left out: the next scene." }),
    );
    props.insert(
        "remove".into(),
        json!({ "type": "boolean", "description": "Take scene number `scene` out" }),
    );
    json!({ "type": "object", "properties": props, "required": ["narration"] })
}

pub static TOOLS: &[Tool] = &[
    Tool {
        name: "start_video",
        tier: Tier::Full,
        label: "Storyboard",
        description: "Starts a video you then write scene by scene: any length, up to 40 scenes. Set what the whole video shares (duration asked, format, palette, voice, music) and pass outline: one short line per scene. Then call write_scene once per scene and build_video at the end. Use this instead of make_video for a video over about 30 seconds, or one whose scenes you want to compose yourself.",
        params: start_params,
        run: start_video,
    },
    Tool {
        name: "write_scene",
        tier: Tier::Full,
        label: "Write scene",
        description: "Writes one scene of the storyboard: its narration (spoken), title (on screen) and visual with its data, or visual \"canvas\" with elements you compose (words, numbers, icons, shapes, bars, cards, lines; each placed, timed and animated). Scenes are kept in order; \"scene\": 3 rewrites scene 3. The answer lists anything to fix and names the next step: follow it. Nothing is on the timeline until build_video.",
        params: scene_params,
        run: write_scene,
    },
    Tool {
        name: "build_video",
        tier: Tier::Full,
        label: "Build video",
        description: "Makes the video from the storyboard's scenes: voices, drawn scenes, titles, captions, stickers, music and sound effects on the timeline. Call it once every scene is written. After changing a scene with write_scene, call it again: it replaces the video.",
        params: || json!({ "type": "object", "properties": {} }),
        run: build_video,
    },
];

#[cfg(test)]
mod tests {
    use super::super::test_env::Fixture;
    use super::*;
    use crate::model::Project;

    fn scene(words: usize, extra: Value) -> Value {
        let mut v = json!({ "narration": vec!["word"; words].join(" "), "title": "A key point" });
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    }

    #[test]
    fn a_storyboard_leads_through_its_outline() {
        let mut f = Fixture::new(Project::empty());
        let r = f
            .call(
                "start_video",
                json!({
                    "title": "Sleep",
                    "duration": 40,
                    "outline": ["1. Hook: you sleep a third of your life", "- What sleep does", "Three habits", "Start tonight"],
                }),
            )
            .unwrap();
        assert_eq!(r["scenes_planned"], 4);
        assert_eq!(r["target"], 40.);
        let next = r["next"].as_str().unwrap();
        assert!(next.contains("scene 1 of 4"), "{next}");
        // The outline's numbering and bullets are dropped.
        assert!(
            next.contains("\"Hook: you sleep a third of your life\""),
            "{next}"
        );
        assert!(
            f.board.as_ref().unwrap().pending().is_none(),
            "nothing written yet"
        );

        let r = f
            .call(
                "write_scene",
                scene(26, json!({ "visual": "counter", "to": 33, "suffix": "%" })),
            )
            .unwrap();
        assert_eq!(r["scene"], 1);
        assert_eq!(r["visual"], "counter");
        assert_eq!(r["words"], 26);
        assert!(r.get("fix").is_none(), "{r}");
        assert!(r["next"].as_str().unwrap().contains("\"What sleep does\""));

        for _ in 0..2 {
            f.call("write_scene", scene(24, json!({ "stickers": ["bed"] })))
                .unwrap();
        }
        let r = f
            .call("write_scene", scene(24, json!({ "stickers": ["moon"] })))
            .unwrap();
        assert_eq!(r["scenes"], 4);
        assert!(r["next"].as_str().unwrap().contains("build_video"), "{r}");
        let board = f.board.as_ref().unwrap();
        let pending = board.pending().unwrap();
        assert!(
            pending.contains("4 scenes written") && pending.contains("build_video"),
            "{pending}"
        );
        assert!((board.seconds() - 4. * produce::spoken_secs(&"w ".repeat(24))).abs() < 2.);
        assert_eq!(board.summary()["scenes"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn a_scene_says_what_to_fix() {
        let mut f = Fixture::new(Project::empty());
        // No video started: the first scene starts one.
        let r = f.call("write_scene", scene(3, json!({}))).unwrap();
        let fix = r["fix"].to_string();
        assert!(fix.contains("only 3 words"), "{fix}");
        assert!(fix.contains("no picture"), "{fix}");
        // A visual without its data keeps the scene and asks for the data.
        let r = f
            .call("write_scene", scene(25, json!({ "visual": "bar_chart" })))
            .unwrap();
        assert_eq!(r["scenes"], 2);
        let fix = r["fix"].to_string();
        assert!(fix.contains("bar_chart needs items"), "{fix}");
        assert!(fix.contains("\\\"scene\\\": 2"), "{fix}");
        // Sent again with the data: the same scene, nothing to fix.
        let r = f
            .call(
                "write_scene",
                scene(
                    25,
                    json!({ "scene": 2, "visual": "bar_chart", "items": ["A: 1", "B: 2"] }),
                ),
            )
            .unwrap();
        assert_eq!(
            (r["scene"].as_u64(), r["scenes"].as_u64()),
            (Some(2), Some(2))
        );
        assert!(r.get("fix").is_none(), "{r}");
        // Three alike in a row, a long title, too many words.
        let steps = json!({ "visual": "steps", "items": ["One", "Two"] });
        for _ in 0..2 {
            f.call("write_scene", scene(25, steps.clone())).unwrap();
        }
        let mut long = scene(90, steps);
        long["title"] = "This title goes on for far too many words".into();
        let r = f.call("write_scene", long.clone()).unwrap();
        let fix = r["fix"].to_string();
        // The two that matter most; the long title waits its turn.
        assert_eq!(r["fix"].as_array().unwrap().len(), MAX_FIXES);
        assert!(fix.contains("90 words"), "{fix}");
        assert!(fix.contains("Three steps scenes in a row"), "{fix}");
        long["scene"] = 5.into();
        long["narration"] = vec!["word"; 25].join(" ").into();
        long["visual"] = "background".into();
        let fix = f.call("write_scene", long).unwrap()["fix"].to_string();
        assert!(fix.contains("title is 9 words"), "{fix}");
        // A canvas's own notes come through.
        let r = f
            .call(
                "write_scene",
                scene(
                    25,
                    json!({ "elements": [{ "text": "Hi", "in": "wobble" }] }),
                ),
            )
            .unwrap();
        assert_eq!(r["visual"], "canvas");
        assert!(r["fix"].to_string().contains("no entrance"), "{r}");
        // The same layout twice is said; a note that needs no second try
        // (an icon drawn as the nearest) is not passed on.
        let hero = |word: &str| {
            scene(
                25,
                json!({ "elements": [
                { "text": word, "size": "hero", "at": "center" },
                { "icon": "bee", "at": "0.5,0.3" },
            ] }),
            )
        };
        let r = f.call("write_scene", hero("Honey")).unwrap();
        assert!(r.get("fix").is_none(), "{r}");
        let r = f.call("write_scene", hero("Pollen")).unwrap();
        assert!(r.get("fix").is_none(), "{r}");
        assert!(
            r["tip"].as_str().unwrap().contains("laid out like scene 7"),
            "{r}"
        );
        f.call("write_scene", json!({ "scene": 8, "remove": true }))
            .unwrap();
        f.call("write_scene", json!({ "scene": 7, "remove": true }))
            .unwrap();
        // Removing one.
        let r = f
            .call("write_scene", json!({ "scene": 1, "remove": true }))
            .unwrap();
        assert_eq!(
            (r["removed"].as_u64(), r["scenes"].as_u64()),
            (Some(1), Some(5))
        );
        assert!(
            f.call("write_scene", json!({ "title": "No words" }))
                .is_err()
        );
    }

    /// A storyboard built for real (the system voice; no music or sounds,
    /// so no network): canvas scenes drawn, spoken and laid on the timeline.
    /// `cargo test storyboard_for_real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn storyboard_for_real() {
        crate::decoder::init();
        let mut f = Fixture::new(Project::empty());
        f.call(
            "start_video",
            json!({
                "title": "Study buddies (agent test)", "duration": 30, "palette": "night",
                "engine": "system", "music": "none", "sound_effects": "none",
                "outline": ["Hook: most people quit", "Why a buddy works", "Takeaway"],
            }),
        )
        .unwrap();
        let scenes = [
            json!({
                "narration": "Most people who start an online course never finish it. Only about one in three makes it to the end. But there is a simple fix.",
                "title": "Most people quit",
                "visual": "canvas",
                "elements": [
                    { "text": "Course finish rate", "size": "small", "color": "muted" },
                    { "number": 31, "suffix": "%", "label": "finish when they study alone" },
                    { "bar": 31, "label": "Alone", "color": "muted" },
                    { "bar": 87, "label": "With a study buddy" },
                ],
            }),
            json!({
                "narration": "A study buddy changes three things. You show up because someone is waiting. You explain what you learned. And you keep going when it gets hard.",
                "title": "Why it works",
                "visual": "canvas",
                "arrange": "row",
                "elements": [
                    { "icon": "clock", "label": "Show up", "id": "a" },
                    { "icon": "chat", "label": "Explain", "id": "b" },
                    { "icon": "trophy", "label": "Keep going", "id": "c" },
                    { "line": ["a", "b"] }, { "line": ["b", "c"] },
                ],
            }),
            json!({
                "narration": "So before your next course, text one friend and ask them to join. Small steps, taken together, get finished.",
                "title": "Find your buddy",
                "visual": "canvas",
                "elements": [
                    { "shape": "ring", "at": "center", "size": "hero", "color": "accent2" },
                    { "text": "2×", "size": "hero", "at": "0.5,0.44", "color": "accent" },
                    { "text": "more likely to finish", "size": "small", "at": "0.5,0.62", "in": "words" },
                ],
            }),
        ];
        for s in scenes {
            let r = f.call("write_scene", s).unwrap();
            println!("{r}");
            assert!(r.get("fix").is_none(), "{r}");
        }
        let v = f.call("build_video", json!({})).expect("build_video");
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
        let main = f.project.ordered_main_clips();
        assert_eq!(main.len(), 3);
        for c in &main {
            println!("scene file: {}", c.source.as_deref().unwrap_or_default());
        }
        assert!(f.project.duration > 20 * crate::model::SECOND);
        let found = super::super::check::issues(&f.project, &mut f.aliases).0;
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_short_script_is_said_once() {
        let mut f = Fixture::new(Project::empty());
        assert!(
            f.call("build_video", json!({}))
                .unwrap_err()
                .0
                .contains("start_video")
        );
        f.call(
            "start_video",
            json!({ "duration": 90, "music": "none", "sound_effects": "none" }),
        )
        .unwrap();
        assert!(
            f.call("build_video", json!({}))
                .unwrap_err()
                .0
                .contains("no scenes")
        );
        f.call(
            "write_scene",
            scene(25, json!({ "visual": "counter", "to": 5 })),
        )
        .unwrap();
        let e = f.call("build_video", json!({})).unwrap_err();
        assert!(e.0.contains("of the 90 s asked"), "{e:?}");
        assert!(e.0.contains("write_scene for scene 2"), "{e:?}");
        // Said once: asked again, it builds what there is. Another scene
        // written, and a short script is said again.
        let board = f.board.as_mut().unwrap();
        assert!(ready(board, false).is_ok());
        board.warned = false;
        assert!(ready(board, true).is_ok());
        f.call(
            "write_scene",
            scene(25, json!({ "visual": "counter", "to": 6 })),
        )
        .unwrap();
        assert!(ready(f.board.as_mut().unwrap(), false).is_err());
        // Bad settings are refused before any scene is written.
        let e = f
            .call("start_video", json!({ "palette": "plaid" }))
            .unwrap_err();
        assert!(e.0.contains("palette is one of"), "{e:?}");
        // The storyboard that was there is kept.
        assert_eq!(f.board.as_ref().unwrap().scenes.len(), 2);
    }
}
