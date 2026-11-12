//! The agent's tools: what a model may call, and how each call edits the
//! project.
//!
//! A tool runs on the UI thread against an `Env`: a working copy of the
//! document plus what the editor knows (playhead, selection, media
//! library). The editor turns a changed copy into ONE undo step, so every
//! tool call can be undone on its own and a whole turn with "Undo AI
//! edits". A tool that needs the network or the decoder (sounds, frames)
//! returns `Outcome::Later`: a job for a background thread whose result is
//! applied back on the UI thread.
//!
//! Every failure is a `ToolError`: a plain sentence the model can act on.
//!
//! Tools come in two tiers. Local models get the `Core` set: the everyday
//! edits and the macro tools that do a whole job in one call (sound design,
//! animated titles, motion presets), because a small model picks worse the
//! more tools it sees and can't plan forty steps. Cloud models get all.
//!
//! - `edit`: the everyday timeline edits
//! - `motion`: ready motions and animated titles
//! - `sound`: sound effects
//! - `produce`: voiceover, captions, music, stickers, drawn scenes, and
//!   `make_video` (a whole video from one script)
//! - `board`: a video written scene by scene (the storyboard), any length
//! - `check`: the timeline looked over, each problem with its fix
//! - `look`: frames for a vision model

pub mod board;
pub mod check;
pub mod edit;
pub mod look;
pub mod motion;
pub mod produce;
pub mod sound;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

use super::llm::{Server, ToolDef};
use super::snapshot::{Aliases, Kind, Lane, SnapshotContext, lane_of, micros, snapshot};
use crate::engine::ActiveLayer;
use crate::model::{Clip, MediaItem, Micros, Project, SECOND, Track};

pub use sound::SoundShelf;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolError(pub String);

impl<T: Into<String>> From<T> for ToolError {
    fn from(s: T) -> Self {
        ToolError(s.into())
    }
}

pub type ToolResult<T = Value> = Result<T, ToolError>;

/// The second half of a background tool, run back on the UI thread.
pub type Finish = Box<dyn FnOnce(&mut Env) -> ToolResult + Send>;
/// A background tool's slow part (downloads, decoding, a vision call).
pub type Job = Box<dyn FnOnce(&Progress) -> ToolResult<Finish> + Send>;

pub enum Outcome {
    Done(Value),
    Later(Job),
}

/// A long tool's progress line for the chat ("3/8"), and the turn's stop.
#[derive(Clone, Default)]
pub struct Progress {
    text: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
}

impl Progress {
    pub fn new(stop: Arc<AtomicBool>) -> Self {
        Progress {
            text: Arc::default(),
            stop,
        }
    }
    pub fn set(&self, s: impl Into<String>) {
        if let Ok(mut t) = self.text.lock() {
            *t = s.into();
        }
    }
    pub fn get(&self) -> String {
        self.text.lock().map(|t| t.clone()).unwrap_or_default()
    }
    /// A progress of its own for work running beside this one (its text
    /// isn't shown), stopped by the same Stop.
    pub fn quiet(&self) -> Progress {
        Progress {
            text: Arc::default(),
            stop: self.stop.clone(),
        }
    }
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
    pub fn check(&self) -> ToolResult<()> {
        if self.stopped() {
            Err("Stopped".into())
        } else {
            Ok(())
        }
    }
}

/// What a tool sees and changes.
pub struct Env<'a> {
    /// The working copy; the editor commits it when `changed` is set.
    pub project: Project,
    pub aliases: &'a mut Aliases,
    pub sounds: &'a mut SoundShelf,
    /// The video being written scene by scene, if one is.
    pub board: &'a mut Option<board::Board>,
    pub media: &'a [MediaItem],
    pub playhead: Micros,
    pub selected: Option<String>,
    /// Video effect names the catalog has shown so far.
    pub effect_names: Vec<String>,
    /// What the engine shows at a time (source times resolved).
    pub layers_at: &'a dyn Fn(Micros) -> Vec<ActiveLayer>,
    /// The model the `look` tool asks about frames.
    pub vision: Option<Server>,

    // What the tool asks of the editor.
    /// The undo step's name when the document changed.
    pub changed: Option<&'static str>,
    /// Re-pack the main lane afterwards (when the magnet is on).
    pub magnet: bool,
    pub seek_to: Option<Micros>,
    pub select: Option<Option<String>>,
    /// Files the tool made (scenes) for the media library.
    pub library_add: Vec<MediaItem>,
}

impl Env<'_> {
    pub fn fps(&self) -> f64 {
        if self.project.canvas.fps > 0. {
            self.project.canvas.fps
        } else {
            30.
        }
    }

    pub fn frame(&self) -> Micros {
        (SECOND as f64 / self.fps()).round() as Micros
    }

    /// `t` on the nearest frame.
    pub fn snap(&self, t: Micros) -> Micros {
        let frames = (t as f64 * self.fps() / SECOND as f64).round();
        (frames * SECOND as f64 / self.fps()).round() as Micros
    }

    pub fn alias(&mut self, clip_id: &str) -> String {
        self.aliases.alias(Kind::Clip, clip_id)
    }

    pub fn clip(&self, id: &str) -> ToolResult<(&Track, &Clip)> {
        self.project.find_clip(id).ok_or_else(|| {
            ToolError("That clip is gone. Call get_project for the current ids.".into())
        })
    }

    /// The clip a call names (`key`), else the selected one, else the main
    /// clip under the playhead.
    pub fn target_clip(&self, args: &Args, key: &str) -> ToolResult<String> {
        if let Some(r) = args.str(key) {
            return self.aliases.resolve(Kind::Clip, &r).ok_or_else(|| {
                ToolError(format!(
                    "No clip \"{r}\". Clip ids (c1, c2…) are in get_project."
                ))
            });
        }
        if let Some(id) = &self.selected
            && self.project.find_clip(id).is_some()
        {
            return Ok(id.clone());
        }
        self.main_clip_at(self.playhead)
            .ok_or_else(|| ToolError(format!("\"{key}\" is required (no clip is selected)")))
    }

    pub fn main_clip_at(&self, t: Micros) -> Option<String> {
        let main = self.project.ordered_main_clips();
        main.iter()
            .find(|c| t >= c.timeline_start && t < c.end())
            .or_else(|| main.last().filter(|c| t >= c.end()))
            .map(|c| c.id.clone())
    }

    /// A lane by alias ("t3") or kind ("main", "audio"…: the first of it).
    pub fn lane(&self, reference: &str) -> ToolResult<String> {
        let key = reference.trim().to_lowercase();
        if let Some(lane) = Lane::ALL.iter().find(|l| l.name() == key) {
            let mut tracks: Vec<&Track> = self
                .project
                .tracks
                .iter()
                .filter(|t| lane_of(&self.project, t) == *lane)
                .collect();
            tracks.sort_by(|a, b| a.order.total_cmp(&b.order));
            return tracks
                .first()
                .map(|t| t.id.clone())
                .ok_or_else(|| ToolError(format!("This project has no {} lane yet", lane.name())));
        }
        self.aliases
            .resolve(Kind::Lane, reference)
            .filter(|id| self.project.tracks.iter().any(|t| &t.id == id))
            .ok_or_else(|| {
                ToolError(format!(
                    "No lane \"{reference}\". Lane ids (t1, t2…) are in get_project."
                ))
            })
    }

    pub fn is_main(&self, clip_id: &str) -> bool {
        self.project
            .track_of(clip_id)
            .is_some_and(|t| self.project.is_main_track(&t.id))
    }

    pub fn snapshot(&mut self) -> Value {
        let selected = self.selected.clone();
        snapshot(
            &self.project,
            self.aliases,
            &SnapshotContext {
                playhead: self.playhead,
                selected: selected.as_deref(),
                media: self.media,
            },
        )
    }

    /// Marks the document edited (one undo step named `label`).
    pub fn edited(&mut self, label: &'static str) {
        self.changed.get_or_insert(label);
    }
}

// ---- arguments ---------------------------------------------------------------------

/// A call's arguments, read forgivingly: numbers may come as strings,
/// times as "5s", "1:05" or "00:01:05".
pub struct Args(pub Map<String, Value>);

impl Args {
    pub fn has(&self, key: &str) -> bool {
        self.0.get(key).is_some_and(|v| !v.is_null() && v != "")
    }

    pub fn str(&self, key: &str) -> Option<String> {
        match self.0.get(key)? {
            Value::Null => None,
            Value::String(s) if s.trim().is_empty() => None,
            Value::String(s) => Some(s.trim().to_string()),
            v => Some(v.to_string()),
        }
    }

    pub fn num(&self, key: &str) -> ToolResult<Option<f64>> {
        let Some(v) = self.0.get(key) else {
            return Ok(None);
        };
        match v {
            Value::Null => Ok(None),
            Value::Number(n) => Ok(n.as_f64()),
            Value::String(s) if s.trim().is_empty() => Ok(None),
            Value::String(s) => parse_number(s)
                .map(Some)
                .ok_or_else(|| ToolError(format!("\"{key}\" must be a number, not \"{s}\""))),
            Value::Bool(_) | Value::Array(_) | Value::Object(_) => {
                Err(format!("\"{key}\" must be a number").into())
            }
        }
    }

    pub fn bool(&self, key: &str) -> ToolResult<Option<bool>> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(Value::String(s)) => match s.trim().to_lowercase().as_str() {
                "true" | "yes" | "on" | "1" => Ok(Some(true)),
                "false" | "no" | "off" | "0" => Ok(Some(false)),
                _ => Err(format!("\"{key}\" must be true or false").into()),
            },
            Some(Value::Number(n)) => Ok(Some(n.as_f64() != Some(0.))),
            _ => Err(format!("\"{key}\" must be true or false").into()),
        }
    }

    /// A time in seconds → micros.
    pub fn time(&self, key: &str) -> ToolResult<Option<Micros>> {
        Ok(self.num(key)?.map(|s| micros(s.max(0.))))
    }

    pub fn required_str(&self, key: &str) -> ToolResult<String> {
        self.str(key)
            .ok_or_else(|| ToolError(format!("\"{key}\" is required")))
    }
}

/// "5", "5.5s", "1:05", "00:01:05.5", "50%" (→ 50).
pub fn parse_number(s: &str) -> Option<f64> {
    let t = s
        .trim()
        .trim_end_matches(['s', 'S', '%', '°', 'x', 'X'])
        .trim();
    if let Ok(v) = t.parse::<f64>() {
        return Some(v);
    }
    if t.contains(':') {
        let mut total = 0.;
        for part in t.split(':') {
            total = total * 60. + part.trim().parse::<f64>().ok()?;
        }
        return Some(total);
    }
    None
}

// ---- matching names ------------------------------------------------------------------

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The option `wanted` means: exact (ignoring case and punctuation), then
/// a unique prefix / containment match. Otherwise an error listing the
/// closest names, so the model can retry.
pub fn pick_name<'a>(wanted: &str, options: &'a [String], what: &str) -> ToolResult<&'a String> {
    let w = normalize(wanted);
    if let Some(o) = options.iter().find(|o| normalize(o) == w) {
        return Ok(o);
    }
    let partial: Vec<&String> = options
        .iter()
        .filter(|o| {
            let n = normalize(o);
            !w.is_empty() && (n.starts_with(&w) || n.contains(&w) || w.contains(&n))
        })
        .collect();
    if let [one] = partial.as_slice() {
        return Ok(one);
    }
    if partial.len() <= 3
        && let Some(best) = partial.iter().min_by_key(|o| o.len())
    {
        return Ok(best);
    }
    let close: Vec<&str> = if partial.is_empty() {
        let mut scored: Vec<(usize, &String)> = options
            .iter()
            .map(|o| (edit_distance(&normalize(o), &w), o))
            .collect();
        scored.sort_by_key(|(d, _)| *d);
        scored.iter().take(8).map(|(_, o)| o.as_str()).collect()
    } else {
        partial.iter().take(8).map(|o| o.as_str()).collect()
    };
    Err(format!(
        "No {what} called \"{wanted}\". Closest: {}. (list_options shows them all.)",
        close.join(", ")
    )
    .into())
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + (ca != *cb) as usize)
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

// ---- the table ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// Offered to every model.
    Core,
    /// Cloud models only.
    Full,
}

pub struct Tool {
    pub name: &'static str,
    pub tier: Tier,
    /// The chat's label for a run of it.
    pub label: &'static str,
    pub description: &'static str,
    pub params: fn() -> Value,
    pub run: fn(&Args, &mut Env) -> ToolResult<Outcome>,
}

/// A JSON-schema object from `(name, type, description)` rows; names ending
/// in `!` are required.
pub fn schema(props: &[(&str, &str, &str)]) -> Value {
    let mut properties = Map::new();
    let mut required = vec![];
    for (name, ty, description) in props {
        let (name, req) = match name.strip_suffix('!') {
            Some(n) => (n, true),
            None => (*name, false),
        };
        if req {
            required.push(Value::from(name));
        }
        let mut p = json!({ "description": description });
        if let Some(options) = ty.strip_prefix("enum:") {
            p["type"] = "string".into();
            p["enum"] = options.split('|').collect::<Vec<_>>().into();
        } else if *ty == "array" {
            // Some providers refuse an array without its item type.
            p["type"] = "array".into();
            p["items"] = json!({ "type": "number" });
        } else {
            p["type"] = (*ty).into();
        }
        properties.insert(name.to_string(), p);
    }
    let mut s = json!({ "type": "object", "properties": properties });
    if !required.is_empty() {
        s["required"] = required.into();
    }
    s
}

pub fn all() -> Vec<&'static Tool> {
    edit::TOOLS
        .iter()
        .chain(motion::TOOLS)
        .chain(sound::TOOLS)
        .chain(produce::TOOLS)
        .chain(board::TOOLS)
        .chain(check::TOOLS)
        .chain(look::TOOLS)
        .collect()
}

pub fn find(name: &str) -> Option<&'static Tool> {
    all().into_iter().find(|t| t.name == name)
}

pub fn label(name: &str) -> &'static str {
    if name == super::skills::USE_SKILL {
        return "Skill";
    }
    find(name).map_or("Tool", |t| t.label)
}

/// A tool as the model is told about it.
pub fn def(t: &Tool) -> ToolDef {
    ToolDef {
        name: t.name,
        description: t.description.to_string(),
        parameters: (t.params)(),
    }
}

/// The fixed tool definitions a model gets (skills add more: `skills.rs`).
pub fn defs(cloud: bool) -> Vec<ToolDef> {
    all()
        .into_iter()
        .filter(|t| cloud || t.tier == Tier::Core)
        .map(def)
        .collect()
}

/// Runs a call; an unknown tool or bad arguments come back as errors the
/// model reads.
pub fn run(name: &str, args: Map<String, Value>, env: &mut Env) -> ToolResult<Outcome> {
    let tool = find(name).ok_or_else(|| {
        ToolError(format!(
            "There is no tool \"{name}\". Use one of: {}",
            all().iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
        ))
    })?;
    (tool.run)(&Args(args), env)
}

#[cfg(test)]
pub mod test_env {
    use super::*;

    pub struct Fixture {
        pub project: Project,
        pub aliases: Aliases,
        pub sounds: SoundShelf,
        pub board: Option<board::Board>,
        pub media: Vec<MediaItem>,
        pub playhead: Micros,
        pub selected: Option<String>,
    }

    pub fn no_layers(_: Micros) -> Vec<ActiveLayer> {
        vec![]
    }

    impl Fixture {
        pub fn new(project: Project) -> Self {
            Fixture {
                project,
                aliases: Aliases::default(),
                sounds: SoundShelf::default(),
                board: None,
                media: vec![],
                playhead: 0,
                selected: None,
            }
        }

        /// Runs a tool (a background one synchronously) and keeps its edit.
        pub fn call(&mut self, name: &str, args: Value) -> ToolResult {
            let layers = no_layers;
            let mut env = Env {
                project: self.project.clone(),
                aliases: &mut self.aliases,
                sounds: &mut self.sounds,
                board: &mut self.board,
                media: &self.media,
                playhead: self.playhead,
                selected: self.selected.clone(),
                effect_names: vec!["Glitch".into(), "Old TV".into()],
                layers_at: &layers,
                vision: None,
                changed: None,
                magnet: false,
                seek_to: None,
                select: None,
                library_add: vec![],
            };
            let args = match args {
                Value::Object(m) => m,
                _ => Map::new(),
            };
            let value = match run(name, args, &mut env)? {
                Outcome::Done(v) => v,
                Outcome::Later(job) => {
                    let finish = job(&Progress::default())?;
                    finish(&mut env)?
                }
            };
            let mut project = env.project;
            if env.magnet {
                project = crate::model::with_magnet(&self.project, project, true);
            }
            if let Some(t) = env.seek_to {
                self.playhead = t;
            }
            if let Some(s) = env.select {
                self.selected = s;
            }
            self.project = project;
            Ok(value)
        }

        /// A project with `n` main clips of `secs` seconds each.
        pub fn with_main_clips(n: usize, secs: f64) -> Self {
            let mut p = Project::empty();
            for i in 0..n {
                let media = MediaItem {
                    path: format!("/tmp/clip{i}.mp4"),
                    name: format!("clip{i}.mp4"),
                    duration: micros(secs),
                    width: 1080,
                    height: 1920,
                    has_video: true,
                    still: false,
                };
                p.insert_media(&media, None);
            }
            let mut f = Fixture::new(p);
            // Aliases in timeline order, as get_project would hand them out.
            let ids: Vec<String> = f
                .project
                .ordered_main_clips()
                .iter()
                .map(|c| c.id.clone())
                .collect();
            for id in ids {
                f.aliases.alias(Kind::Clip, &id);
            }
            f
        }

        pub fn id(&self, alias: &str) -> String {
            self.aliases.resolve(Kind::Clip, alias).expect("alias")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_numbers_forgivingly() {
        assert_eq!(parse_number("5"), Some(5.));
        assert_eq!(parse_number("2.5s"), Some(2.5));
        assert_eq!(parse_number("1:05"), Some(65.));
        assert_eq!(parse_number("00:01:05.5"), Some(65.5));
        assert_eq!(parse_number("80%"), Some(80.));
        assert_eq!(parse_number("soon"), None);
    }

    #[test]
    fn picks_names_forgivingly() {
        let options: Vec<String> = ["Old TV", "Glitch", "Glitch Pro", "Film Grain"]
            .map(String::from)
            .to_vec();
        assert_eq!(pick_name("old tv", &options, "effect").unwrap(), "Old TV");
        assert_eq!(pick_name("oldtv", &options, "effect").unwrap(), "Old TV");
        assert_eq!(pick_name("glitch", &options, "effect").unwrap(), "Glitch");
        assert_eq!(
            pick_name("grain", &options, "effect").unwrap(),
            "Film Grain"
        );
        let err = pick_name("sparkles", &options, "effect").unwrap_err();
        assert!(err.0.contains("Closest"), "{err:?}");
    }

    #[test]
    fn every_tool_has_a_valid_schema() {
        for t in all() {
            let s = (t.params)();
            assert_eq!(s["type"], "object", "{}", t.name);
            assert!(
                t.description.len() < 900,
                "{} is too long for a small model",
                t.name
            );
        }
        let core = defs(false).len();
        assert!(core <= 26, "local models get {core} tools");
    }
}
