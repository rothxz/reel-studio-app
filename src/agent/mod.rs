//! The editor's AI agent: a model that edits the timeline through tool
//! calls. It runs on a model server on this computer (Ollama, LM Studio,
//! llama.cpp, any OpenAI-compatible server: nothing leaves the machine) or
//! a cloud provider with the user's own API key (Claude, ChatGPT, Gemini,
//! OpenRouter, Grok, DeepSeek, Mistral).
//!
//! One request is one turn: ask the model, run the tools it calls, hand
//! back their results, repeat until it answers in words. Every tool call is
//! its own undo step, and "Undo AI edits" takes back a whole turn.
//!
//! Small local models are the hard case. They get fewer tools, worked
//! examples in the prompt, and macro tools that do a whole job in one call
//! (`sound_design`, `add_title`, `animate`, `make_video`), so "add sound
//! effects", "make a title" or "make a video about…" is one correct call
//! rather than forty.
//!
//! - `providers`: the servers the setup offers
//! - `keys`: API keys in the keychain
//! - `llm`: the chat client (three dialects), vision
//! - `snapshot`: the project as the model reads it, short ids
//! - `tools`: what the model can do
//! - `skills`: a job's know-how and extra tools, loaded per request
//!   (routed by its words, or `use_skill`); the user's own in `<data>/skills/`
//! - `sfx`: real sound effects (the app's library + Openverse)
//! - `voice`: text to speech (the free local Kokoro voice, `local_voice`;
//!   OpenAI / Gemini with the user's key; else the system voice);
//!   `captions`: captions timed to it
//! - `graphics`: motion-graphic scenes (backdrops, charts, counters…)
//!   drawn frame by frame; `movie`: frames → an H.264 file
//!
//! The panel and the turn loop are `ui/agent.rs`.

pub mod captions;
pub mod dice;
#[cfg(test)]
mod eval;
pub mod graphics;
pub mod keys;
pub mod llm;
pub mod local_voice;
pub mod movie;
// On macOS only its tests build it (the voice picking is plain code).
#[cfg(any(not(target_os = "macos"), test))]
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub mod os_voice;
pub mod providers;
pub mod sfx;
pub mod skills;
pub mod snapshot;
pub mod tools;
pub mod voice;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use llm::{Message, Server};
use providers::preset;
use snapshot::Aliases;
use tools::{Progress, SoundShelf};

/// Model calls per turn before it's stopped (a small model can loop).
pub const MAX_LOCAL_ROUNDS: usize = 16;
/// A cloud model builds whole edits: many more steps, and it doesn't loop.
pub const MAX_CLOUD_ROUNDS: usize = 40;
/// Conversation kept between turns (messages, not counting the system's).
const HISTORY_LIMIT: usize = 30;
/// A local model's context is small: long tool results are cut to this.
const LOCAL_RESULT_CHARS: usize = 6_000;

// ---- settings -----------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelChoice {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub vision_model: String,
}

/// What the setup remembers (`<data>/agent.json`). Keys never go here.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub preset: String,
    pub base_url: String,
    pub model: String,
    /// The model `look` asks about frames; "" = the chat model.
    pub vision_model: String,
    /// Each server's last picks, so switching back restores them.
    pub saved: HashMap<String, ModelChoice>,
    pub open: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            preset: "ollama".into(),
            base_url: preset("ollama").base_url.into(),
            model: String::new(),
            vision_model: String::new(),
            saved: HashMap::new(),
            open: false,
        }
    }
}

fn settings_path() -> PathBuf {
    crate::prefs::data_dir().join("agent.json")
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read_to_string(settings_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(settings_path(), json);
        }
    }

    pub fn cloud(&self) -> bool {
        preset(&self.preset).cloud
    }

    fn server_for(&self, model: &str) -> Option<Server> {
        if model.is_empty() {
            return None;
        }
        let p = preset(&self.preset);
        Some(Server {
            dialect: p.dialect,
            base_url: if p.cloud {
                p.base_url.to_string()
            } else {
                self.base_url.clone()
            },
            model: model.to_string(),
            auth: p.cloud.then(|| p.id.to_string()),
        })
    }

    /// The chat model, once one is picked.
    pub fn server(&self) -> Option<Server> {
        self.server_for(&self.model)
    }

    /// The model `look` uses.
    pub fn vision_server(&self) -> Option<Server> {
        if self.vision_model.is_empty() {
            self.server()
        } else {
            self.server_for(&self.vision_model)
        }
    }

    /// Switches server, keeping each one's picks.
    pub fn pick_preset(&mut self, id: &str) {
        let current = ModelChoice {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            vision_model: self.vision_model.clone(),
        };
        self.saved.insert(self.preset.clone(), current);
        let p = preset(id);
        self.preset = p.id.into();
        let back = self.saved.get(p.id).cloned().unwrap_or_default();
        self.base_url = if back.base_url.is_empty() || p.cloud {
            p.base_url.into()
        } else {
            back.base_url
        };
        self.model = back.model;
        self.vision_model = back.vision_model;
    }
}

// ---- the chat ------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    Ok,
    Error,
}

/// What the chat shows: the user's words, each tool run, the answers.
#[derive(Clone)]
pub enum Item {
    User(String),
    Assistant(String),
    Tool {
        name: String,
        /// A short reading of the arguments ("c3 at 2.5s").
        detail: String,
        status: Status,
        /// The result's gist, or the error.
        result: String,
        progress: Option<Progress>,
    },
    Error(String),
}

/// The agent as the editor holds it (settings persist; the chat lasts as
/// long as the project stays open).
pub struct Agent {
    pub settings: Settings,
    pub items: Vec<Item>,
    pub history: Vec<Message>,
    pub busy: bool,
    pub stop: Arc<AtomicBool>,
    /// Bumped by every turn and by New chat: a late answer from an old
    /// turn is dropped.
    pub turn: u64,
    pub aliases: Aliases,
    pub sounds: SoundShelf,
    /// Video effect names fetched for the agent (the catalog's are added).
    pub effect_names: Vec<String>,
    /// The connected server's models.
    pub models: Vec<String>,
    pub connecting: bool,
    pub setup_error: Option<String>,
    /// The setup shows over the chat.
    pub setup_open: bool,
    /// The undo stack's size before / after the last turn's edits.
    pub last_turn: Option<(usize, usize)>,
    /// The local model last warmed up (server, model) and when: the server
    /// keeps it loaded a while, so typing again soon doesn't re-send it.
    pub warmed: Option<(String, String, std::time::Instant)>,
}

impl Agent {
    pub fn new() -> Self {
        let settings = Settings::load();
        let setup_open = settings.model.is_empty();
        Agent {
            settings,
            items: vec![],
            history: vec![],
            busy: false,
            stop: Arc::new(AtomicBool::new(false)),
            turn: 0,
            aliases: Aliases::default(),
            sounds: SoundShelf::default(),
            effect_names: vec![],
            models: vec![],
            connecting: false,
            setup_error: None,
            setup_open,
            last_turn: None,
            warmed: None,
        }
    }

    /// New chat (also a different project): ids start over.
    pub fn reset(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.items.clear();
        self.history.clear();
        self.busy = false;
        self.turn += 1;
        self.aliases = Aliases::default();
        self.sounds = SoundShelf::default();
        self.last_turn = None;
    }
}

// ---- prompts --------------------------------------------------------------------------

pub const SYSTEM_PROMPT: &str = "\
You are the editing assistant inside Reel Studio, a video editor. You change the user's project ONLY by calling the tools.
The project data doesn't show the picture, but the look tool does: it shows you real frames. Call it whenever the request depends on what's in the video (what it shows, a moment, where to cut, a fitting title). Never say you can't see the video.

Timeline basics:
- Times are seconds on the timeline. Clip ids look like c1, c2; lanes t1, t2; library files m1, m2. Use exactly the ids in the project state.
- Lanes: main (the video, clips back to back from 0s), overlay (picture-in-picture), stickers, audio, text, effect.
- After split_clip the right half has a NEW id (returned by the tool).
- When the user says \"this clip\" or names none, use the selected clip, else the main clip under the playhead.
- When the user gives no time, leave start/time out: it defaults to the playhead.
- Each request comes with the project state. Your edits since then show in the tool results; call get_project for the whole timeline again.

You can make things, not just edit them:
- make_video makes a whole video from nothing in ONE call: a narrator's voice, animated motion-graphic scenes (charts, counters, comparisons, steps, flow diagrams, code), titles, captions, sticker illustrations, music and sound effects. When asked to make, generate or create a video about something, write the script yourself and call make_video. Never say you can't make videos.
- add_voiceover speaks words in a real voice, with captions timed to it. add_music lays a music bed under the video.
- sound_design adds real sound effects to the whole edit in one call (cuts, titles, text, stickers, effects). find_sounds + add_sound place a single sound.
- add_title makes an animated title (motion graphics): pop, slide_up, typewriter, word_by_word, lower_third, headline, neon, countdown.
- animate puts a motion on any clip: zoom_in, pan_left, pop_in, slide_in_up, shake, fade_out…
- add_effect adds a video effect, set_filter a colour look.

How to answer:
- Make the edit with tools first. Several tool calls in one reply are fine.
- If a tool returns an error, fix the arguments and try again, or explain the problem.
- Then reply in one or two short sentences saying what you changed. Never claim an edit you didn't make with a tool.
- If no tool can do what's asked, say so plainly and suggest the closest thing you can do.
- Reply in the language the user writes in.";

/// Worked examples: a small model copies a pattern far better than it
/// follows a rule.
pub const LOCAL_EXAMPLES: &str = "

Examples of requests and the call that does them:
- \"make a short video about saving water\" → make_video {\"title\": \"Save water\", \"duration\": 10, \"scenes\": [{\"narration\": \"Every drop counts.\", \"title\": \"Every drop counts\", \"stickers\": [\"water drop\"]}, {\"narration\": \"A running tap wastes six liters a minute.\", \"title\": \"6 L a minute\", \"visual\": \"counter\", \"to\": 6, \"suffix\": \" L\"}, {\"narration\": \"Turn it off while you brush.\", \"title\": \"Turn it off\"}]}
- \"add a voiceover saying welcome to my channel\" → add_voiceover {\"text\": \"Welcome to my channel!\"}
- \"add some chill music\" → add_music {\"mood\": \"chill\"}
- \"add sound effects\" → sound_design {}
- \"make it sound cinematic\" → sound_design {\"style\": \"cinematic\"}
- \"add a whoosh at 3 seconds\" → add_sound {\"sound\": \"whoosh\", \"time\": 3, \"anchor\": \"peak\"}
- \"put a title 'Summer trip' at the start\" → add_title {\"text\": \"Summer trip\", \"style\": \"pop\", \"start\": 0}
- \"add a lower third with my name Ana, designer\" → add_title {\"text\": \"Ana\", \"subtitle\": \"Designer\", \"style\": \"lower_third\"}
- \"slow zoom on this clip\" → animate {\"motion\": \"zoom_in\"}
- \"shake the screen at 4s\" → animate {\"motion\": \"shake\", \"time\": 4}
- \"cut at 5 seconds\" → split_clip {\"time\": 5}
- \"remove the first 2 seconds of c1\" → trim_clip {\"clip\": \"c1\", \"start\": 2}
- \"make it black and white\" → list_options {\"what\": \"filters\", \"search\": \"b\"} then set_filter
- \"what is in my video?\" → look {}";

/// What a cloud model is told on top: it has every tool.
pub const CLOUD_EXTRA: &str = "

The whole timeline is yours to work with:
- Overlay lanes hold picture-in-picture videos/photos (move_clip with lane \"overlay\"). The effect lane holds effects over everything (add_effect without a clip). set_lane locks, hides or mutes a lane.
- The media library (m1, m2…) lists the user's imported files: add_media puts them on the timeline. You can't import new files: ask the user to.
- get_clip shows everything about one clip. add_keyframe animates by hand (two or more times); animate does ready motions.
- Plan bigger requests (\"make a highlight reel\", \"a title on every scene\", \"make this feel like a trailer\") as a sequence of calls and carry it through without asking at each step; look at the video first when the content matters. A finished edit usually wants titles, motion, a colour look and sound design.
- Several independent calls can go in one reply; calls that need an earlier result (new ids after split_clip) wait for it.

Making a video from a prompt (make_video):
- Write the script first, in your head: a hook, the scenes the user asked for, a memorable ending. Spoken English runs about 2.5 words a second, so 60–90 s is 150–220 words of narration: 6–8 scenes of 2–4 sentences (25–40 words) each. Scene length follows its narration.
- Every scene: narration (spoken, conversational), title (the on-screen key point, 2–6 words), and the visual that fits: numbers → bar_chart, counter or line_chart (real, plausible figures, with prefix \"$\" for money); a contrast → comparison; how-to → steps; how something works (a system, a pipeline, a process, cause and effect) → flow, its nodes in order with a note each that follows the narration (the narration walks through the nodes as they light up); programming → code with a step per block; otherwise background with 1–2 stickers (concrete things: \"coffee cup\", \"piggy bank\", \"calendar\"). Mix kinds across scenes; don't repeat one visual scene after scene. For tech topics palette neon or blueprint fits.
- Pick a palette that suits the topic, a music mood, and a tone for the voice. Captions are on by default.
- Pass duration (seconds) when the user asked for a length. Call make_video once with everything. It takes a minute (voices, drawing, downloads). Then say what you made, scene by scene in a line or two, and mention any notes it returned (like the voice being the computer's own).
- The user's own footage: if the media library has clips that fit, add them with add_media after make_video and move them to an overlay, or skip make_video and edit their footage with add_voiceover, add_title, add_music and sound_design.";

/// The fixed instructions, the skills' list last.
pub fn system_prompt(cloud: bool, skills: &[skills::Skill]) -> String {
    let extra = if cloud { CLOUD_EXTRA } else { LOCAL_EXAMPLES };
    format!("{SYSTEM_PROMPT}{extra}{}", skills::catalog(skills))
}

/// The turn's user message: the timeline as it is now, then the request.
pub fn request_with_state(state: &serde_json::Value, text: &str) -> String {
    format!("Current project state:\n{state}\n\nRequest: {text}")
}

/// A tool result as the model reads it (cut short for a small context).
pub fn tool_content(result: &Result<serde_json::Value, tools::ToolError>, cloud: bool) -> String {
    let s = match result {
        Ok(v) => v.to_string(),
        Err(e) => serde_json::json!({ "error": e.0 }).to_string(),
    };
    if !cloud && s.len() > LOCAL_RESULT_CHARS {
        let mut cut = LOCAL_RESULT_CHARS;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}… (cut short)", &s[..cut])
    } else {
        s
    }
}

/// Keeps the tail, never starting on a tool result (its call would be gone).
pub fn trim_history(mut messages: Vec<Message>) -> Vec<Message> {
    if messages.len() <= HISTORY_LIMIT {
        return messages;
    }
    let mut start = messages.len() - HISTORY_LIMIT;
    while start < messages.len() && !matches!(messages[start], Message::User(_)) {
        start += 1;
    }
    messages.drain(..start);
    messages
}

/// Tools that only read (or move the playhead): no edit.
pub const READ_ONLY: [&str; 8] = [
    "get_project",
    skills::USE_SKILL,
    "get_clip",
    "look",
    "list_options",
    "find_sounds",
    "seek",
    "select_clip",
];

/// Sent back once when a reply says it changed the project but no editing
/// tool ran: small models "answer" an edit instead of making it.
pub const NUDGE: &str = "Nothing in the project has changed yet: you haven't called a tool that edits it in this request. Call the tool now to make the edit you described (for example add_title, sound_design, animate, split_clip), or say plainly that you can't.";

/// Does this answer claim an edit was made?
pub fn claims_edit(text: &str) -> bool {
    let t = text.to_lowercase();
    [
        "i've ",
        "i have ",
        "i added",
        "i applied",
        "i split",
        "i trimmed",
        "i moved",
        "i removed",
        "i deleted",
        "i changed",
        "i set",
        "i put",
        "i placed",
        "i made",
        "i created",
        "has been added",
        "have been added",
        "was added",
        "now has",
        "is now",
        "added a",
        "added an",
        "added the",
    ]
    .iter()
    .any(|w| t.contains(w))
}

/// The same call (name and arguments) three times running: a loop.
pub fn looping(calls: &[(String, String)]) -> bool {
    calls.len() >= 3 && {
        let n = calls.len();
        calls[n - 1] == calls[n - 2] && calls[n - 2] == calls[n - 3]
    }
}

/// A short reading of a call's arguments for the chat row.
pub fn describe_args(args: &serde_json::Map<String, serde_json::Value>) -> String {
    args.iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(k, v)| {
            let v = match v {
                serde_json::Value::String(s) => s.clone(),
                v => v.to_string(),
            };
            let v: String = v.chars().take(40).collect();
            format!("{k} {v}")
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// A result's gist for the chat row.
pub fn describe_result(v: &serde_json::Value) -> String {
    let Some(o) = v.as_object() else {
        return String::new();
    };
    let pick = |k: &str| {
        o.get(k).map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            v => v.to_string(),
        })
    };
    if o.contains_key("instructions") {
        return "Loaded".into();
    }
    if let Some(scenes) = o.get("scenes").and_then(|s| s.as_array()) {
        let d = o.get("duration").and_then(|d| d.as_f64()).unwrap_or(0.);
        return format!(
            "{} scene{} · {}:{:02}",
            scenes.len(),
            if scenes.len() == 1 { "" } else { "s" },
            (d / 60.) as u64,
            (d % 60.).round() as u64
        );
    }
    if let (Some(v), Some(d)) = (o.get("voice"), o.get("duration").and_then(|d| d.as_f64())) {
        return format!("{} · {d:.1}s", v.as_str().unwrap_or(""));
    }
    if let Some(m) = o.get("music").and_then(|m| m.as_str()) {
        return m.to_string();
    }
    if let Some(n) = o.get("added").and_then(|v| v.as_u64()) {
        return format!("{n} sound{} added", if n == 1 { "" } else { "s" });
    }
    if let Some(s) = pick("seen") {
        return s.chars().take(160).collect();
    }
    if let Some(list) = o.get("sounds").and_then(|s| s.as_array()) {
        return format!("{} found", list.len());
    }
    if let Some(list) = o.get("options").and_then(|s| s.as_array()) {
        return format!("{} options", list.len());
    }
    if o.contains_key("lanes") {
        return "Read".into();
    }
    ["clip", "clips", "copy", "still", "audio", "right"]
        .iter()
        .find_map(|k| pick(k).map(|v| format!("{k} {v}")))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_servers_keeps_their_picks() {
        let mut s = Settings::default();
        s.model = "qwen3:8b".into();
        s.pick_preset("anthropic");
        assert_eq!(s.base_url, "https://api.anthropic.com/v1");
        assert!(s.model.is_empty());
        s.model = "claude-sonnet-5".into();
        s.pick_preset("ollama");
        assert_eq!(s.model, "qwen3:8b");
        s.pick_preset("anthropic");
        assert_eq!(s.model, "claude-sonnet-5");
        let server = s.server().unwrap();
        assert_eq!(server.auth.as_deref(), Some("anthropic"));
    }

    #[test]
    fn history_trims_at_a_user_message() {
        let mut m = vec![];
        for i in 0..20 {
            m.push(Message::User(format!("u{i}")));
            m.push(Message::Tool {
                call_id: "x".into(),
                name: "seek".into(),
                content: "{}".into(),
                error: false,
            });
        }
        let t = trim_history(m);
        assert!(t.len() <= 30);
        assert!(matches!(t[0], Message::User(_)));
    }

    #[test]
    fn spots_claimed_edits() {
        assert!(claims_edit(
            "I've added a fitting animated title at the start."
        ));
        assert!(claims_edit("The clip is now twice as fast."));
        assert!(!claims_edit("The video shows a beach at sunset."));
    }

    #[test]
    fn spots_a_loop() {
        let c = |n: &str| (n.to_string(), "{}".to_string());
        assert!(!looping(&[c("a"), c("a")]));
        assert!(looping(&[c("b"), c("a"), c("a"), c("a")]));
        assert!(!looping(&[c("a"), c("b"), c("a")]));
    }
}
