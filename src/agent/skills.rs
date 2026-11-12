//! Skills: a job's know-how (steps, worked examples) and the tools it needs,
//! loaded only for the requests that need them.
//!
//! A small local model picks worse the more tools it sees, so it gets the
//! `Core` set. A skill hands it the rest one job at a time: keyframes,
//! picture-in-picture, freeze frames, lanes… Two ways in:
//!
//! - **Routed**: the request's words match a skill's triggers ("keyframe",
//!   "picture in picture", "freeze"), so its instructions come with the
//!   request and its tools with the turn. No extra model call.
//! - **Asked for**: the system prompt lists every skill in a line, and the
//!   model calls `use_skill` when a request needs one the words missed.
//!   Its result is the instructions; its tools join the next round.
//!
//! The local server caches the fixed start of every request (the system
//! prompt and tools: `llm::warm_up`), so skills never change it: their
//! instructions go in the user message, their tools after the fixed ones.
//!
//! The user's own skills are Markdown files in `<data>/skills/` (one per
//! skill; a built-in's name replaces it):
//!
//! ```text
//! ---
//! name: channel-intro
//! description: My channel's intro: the title card, upbeat music, a whoosh
//! triggers: intro, channel opener
//! tools: add_title, add_music, add_sound
//! ---
//! 1. add_title {"text": "Roth Makes", "style": "neon", "start": 0}
//! 2. add_music {"mood": "upbeat"}
//! …
//! ```

use std::path::PathBuf;

use serde_json::{Map, Value, json};

use super::llm::ToolDef;
use super::tools::{self, ToolError, ToolResult};

/// The tool a model loads a skill with.
pub const USE_SKILL: &str = "use_skill";
/// Skills a request is routed to at most (each one's text costs context).
const MAX_ROUTED: usize = 2;
/// A user skill's limits: its text goes into a small context.
const MAX_BODY_CHARS: usize = 4_000;
const MAX_DESCRIPTION_CHARS: usize = 200;
const MAX_TOOLS: usize = 8;

#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    /// What `use_skill` takes ("keyframes").
    pub name: String,
    /// One line in the system prompt's list.
    pub description: String,
    /// Words or phrases in a request that route it here.
    pub triggers: Vec<String>,
    /// Tools it adds to the turn (ones the model has already are skipped).
    pub tools: Vec<String>,
    /// The instructions and worked examples.
    pub body: String,
    /// From the skills folder.
    pub user: bool,
}

// ---- the built-in skills --------------------------------------------------------------

struct Builtin {
    name: &'static str,
    description: &'static str,
    triggers: &'static [&'static str],
    tools: &'static [&'static str],
    body: &'static str,
}

/// Each one opens up what the `Full` tools do, or walks a model through a
/// job of several steps.
const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "video",
        description: "Write a whole video scene by scene, any length: plan it, write each scene, build it. For videos of 30 seconds or more.",
        triggers: &[
            "make a video",
            "make me a video",
            "create a video",
            "generate a video",
            "video about",
            "video on",
            "video explaining",
            "video that explains",
            "explainer",
            "storyboard",
            "second video",
            "seconds video",
            "sec video",
            "minute video",
            "min video",
            "long video",
            "promo video",
            "video script",
            "from a prompt",
            "scene by scene",
        ],
        tools: &["start_video", "write_scene", "build_video", "check_video"],
        body: "\
A video of 30 seconds or more, or one with several points, is written scene by scene. (A quick short one is one make_video call.)
1. Plan first: the one idea a viewer should remember, then a line per scene. Scene 1 is a hook (a question, a surprising number, the result first). The middle is one point per scene. The last scene is the takeaway. About one scene per 10 seconds asked.
2. start_video with duration (the seconds asked), the outline (those lines), and what the whole video shares: palette, voice, music.
3. write_scene once per scene, in order:
   - narration: what is SAID. 2–4 spoken sentences, 20–40 words (2.5 words a second).
   - title: the key point ON SCREEN, 2–6 words. Never the narration again.
   - visual: what shows it. Numbers → bar_chart, line_chart, counter. A contrast → comparison. How-to → steps. How something works → flow. Programming → code. Anything else → canvas (you compose it: use_skill motion_graphics), or background with 1–2 stickers. Change the visual from scene to scene.
4. Every answer lists what to fix (\"fix\") and names the next step (\"next\"): make the fixes, then do what next says.
5. build_video once every scene is written. It takes a minute. Then say what you made.
A change later: write_scene with \"scene\": its number, then build_video again.

Example (\"a 45 second video about sleep\"):
start_video {\"title\": \"Why sleep matters\", \"duration\": 45, \"palette\": \"night\", \"outline\": [\"Hook: a third of your life\", \"What sleep repairs\", \"What too little does\", \"Three habits\", \"Takeaway: start tonight\"]}
write_scene {\"narration\": \"You will spend a third of your life asleep. That is twenty five years. So what is your body doing with all that time?\", \"title\": \"25 years asleep\", \"visual\": \"counter\", \"to\": 25, \"suffix\": \" years\"}
write_scene {\"narration\": \"While you sleep, your body works the night shift. Your brain clears out the day's waste, your muscles repair, and your memory files away what you learned.\", \"title\": \"The night shift\", \"visual\": \"flow\", \"nodes\": [{\"label\": \"Brain\", \"note\": \"clears the day's waste\"}, {\"label\": \"Muscles\", \"note\": \"repair and grow\"}, {\"label\": \"Memory\", \"note\": \"files what you learned\"}]}
(one write_scene for each line of the outline)
build_video {}",
    },
    Builtin {
        name: "motion_graphics",
        description: "Compose a motion-graphic scene yourself on a canvas: words, numbers, icons, shapes, bars, cards and lines, each placed, timed and animated.",
        triggers: &[
            "motion graphic",
            "motion graphics",
            "motion video",
            "motion design",
            "motion clip",
            "animated video",
            "animated scene",
            "animated explainer",
            "animated infographic",
            "animation video",
            "infographic",
            "kinetic",
            "canvas",
            "compose a scene",
        ],
        tools: &["add_scene", "write_scene", "start_video", "build_video"],
        body: "\
visual \"canvas\" draws a scene from the elements you list: in write_scene (a scene of a whole video) or add_scene (one scene in an edit). The stage is the middle of the frame; the title sits above it and captions below.
Each element is ONE of:
- {\"text\": \"Small steps win\"}: words
- {\"number\": 87, \"suffix\": \"%\", \"label\": \"finish the course\"}: counts up
- {\"icon\": \"rocket\", \"label\": \"Launch\"}: a line icon in a disc
- {\"shape\": \"ring\"}: circle, ring, rect, pill, triangle, star, line
- {\"bar\": 72, \"label\": \"Battery\"}: a progress bar, percent
- {\"card\": \"Sleep\", \"detail\": \"7–9 hours\", \"icon\": \"moon\"}: a panel
- {\"line\": [\"a\", \"b\"]}: an arrow from the element with \"id\": \"a\" to the one with \"id\": \"b\"
On any element: size (hero, large, medium, small), color (accent, accent2, accent3, text, muted, #hex), in (fade, rise, drop, slide_left, slide_right, pop, grow, spin, draw, type, words), end + out (when and how it leaves), loop (float, pulse, spin).
Where: leave \"at\" out and the elements are laid out for you by arrange: stack (one under another, the default), row or grid. Or place one with \"at\": \"x,y\" (0–1 across the stage: \"0.5,0.2\" is top middle) or a name (center, top_left, bottom_right…).
When: leave \"start\" out and they come in one after another through the scene, in the order listed: list them in the order the narration mentions them. Or give start (seconds into the scene).
Moves: \"moves\": [{\"at\": 2, \"x\": 0.8, \"y\": 0.3, \"scale\": 1.4, \"duration\": 0.8}] sends it there from second 2.

What makes it look designed:
- One idea a scene, 3–6 elements: one hero (a big number, or 1–3 big words), the rest small.
- Different ways in for different things, never everything rising the same way.
- Something changes every 2–3 seconds: staggered starts, or one element with a move.
- A title sits still once it has landed. Loops are for things that turn or float in life.
- accent for the one thing that matters, muted for labels.

Examples:
- a stat: {\"visual\": \"canvas\", \"elements\": [{\"text\": \"Course finish rate\", \"size\": \"small\", \"color\": \"muted\"}, {\"number\": 87, \"suffix\": \"%\"}, {\"bar\": 87, \"label\": \"With a study buddy\"}, {\"bar\": 31, \"label\": \"Alone\", \"color\": \"muted\"}]}
- a process: {\"visual\": \"canvas\", \"arrange\": \"row\", \"elements\": [{\"icon\": \"bulb\", \"label\": \"Idea\", \"id\": \"a\"}, {\"icon\": \"gear\", \"label\": \"Build\", \"id\": \"b\"}, {\"icon\": \"rocket\", \"label\": \"Launch\", \"id\": \"c\"}, {\"line\": [\"a\", \"b\"]}, {\"line\": [\"b\", \"c\"]}]}
- a big line: {\"visual\": \"canvas\", \"elements\": [{\"shape\": \"ring\", \"at\": \"center\", \"size\": \"hero\", \"color\": \"accent2\"}, {\"text\": \"3×\", \"size\": \"hero\", \"at\": \"0.5,0.45\", \"color\": \"accent\"}, {\"text\": \"faster than last year\", \"size\": \"small\", \"at\": \"0.5,0.62\"}]}
- four facts: {\"visual\": \"canvas\", \"arrange\": \"grid\", \"elements\": [{\"card\": \"Sleep\", \"detail\": \"7–9 hours\", \"icon\": \"moon\"}, {\"card\": \"Water\", \"detail\": \"2 litres\", \"icon\": \"drop\"}, {\"card\": \"Move\", \"detail\": \"30 minutes\", \"icon\": \"heart\"}, {\"card\": \"Sun\", \"detail\": \"10 minutes\", \"icon\": \"sun\"}]}",
    },
    Builtin {
        name: "review",
        description: "Look a finished edit over for what a viewer would notice (black gaps, sound past the end, unreadable words) and fix it.",
        triggers: &[
            "check the video",
            "check my video",
            "check the edit",
            "check the timeline",
            "review",
            "anything wrong",
            "what's wrong",
            "find problems",
            "find mistakes",
            "polish",
            "ready to export",
            "before i export",
            "quality check",
        ],
        tools: &["check_video"],
        body: "\
1. check_video {} lists what a viewer would notice, each problem with the call that fixes it.
2. Make every fix it lists (the calls are written out: copy them).
3. check_video {} again until it says ok. Its tips are suggestions: take the ones that fit what the user asked for.
4. Say in a sentence or two what you fixed. If nothing was wrong, say the video is ready.
look {} shows you frames when the question is how it looks, not how it is put together.",
    },
    Builtin {
        name: "keyframes",
        description: "Animate by hand: a clip moving, growing, turning or fading between times you choose.",
        triggers: &[
            "keyframe",
            "keyframes",
            "key frame",
            "animate from",
            "move from",
            "moves from",
            "slide from",
            "grow from",
            "fade from",
            "from the left",
            "from the right",
            "from the top",
            "from the bottom",
            "across the screen",
            "remove the animation",
            "transparent",
            "opacity",
            "see through",
        ],
        tools: &["add_keyframe", "remove_keyframes", "get_clip"],
        body: "\
- add_keyframe sets a clip's values AT one timeline time; between keyframes the clip changes smoothly. An animation is two or more calls: the start values, then the end values.
- x/y are the clip's centre, 0..1 across the frame (0.5 = middle, 0 = left/top edge, 1 = right/bottom edge). scale 1 = fit. opacity 0..1. rotation in degrees. volume in percent.
- Times are timeline seconds inside the clip's own span (the project state gives each clip's start and end).
- A ready motion (zoom_in, pop_in, shake, slide_in_up…) is one animate call: prefer it when one fits.
- remove_keyframes without a time clears a clip's animation; get_clip shows the keyframes it has.

Examples (c1 runs 0s–4s, c2 runs 4s–8s):
- \"move c1 from left to right\" → add_keyframe {\"clip\": \"c1\", \"time\": 0, \"x\": 0.25} then add_keyframe {\"clip\": \"c1\", \"time\": 4, \"x\": 0.75}
- \"fade c2 in over its first second\" → add_keyframe {\"clip\": \"c2\", \"time\": 4, \"opacity\": 0} then add_keyframe {\"clip\": \"c2\", \"time\": 5, \"opacity\": 1}
- \"grow c1 from half size to full by 2s\" → add_keyframe {\"clip\": \"c1\", \"time\": 0, \"scale\": 0.5, \"easing\": \"easeOut\"} then add_keyframe {\"clip\": \"c1\", \"time\": 2, \"scale\": 1}
- \"remove the animation on c1\" → remove_keyframes {\"clip\": \"c1\"}",
    },
    Builtin {
        name: "overlay",
        description: "Videos or photos on top of the main video: picture-in-picture, the user's library files, blend modes.",
        triggers: &[
            "overlay",
            "picture in picture",
            "pip",
            "on top of",
            "in the corner",
            "corner",
            "blend",
            "blend mode",
            "light leak",
            "media library",
            "from my library",
            "my files",
            "add media",
        ],
        tools: &["add_media", "move_clip", "set_transform", "set_blend"],
        body: "\
- The media library (m1, m2… in the project state) holds the user's imported files. add_media with lane \"overlay\" puts one on top of the main video at start; lane \"main\" inserts it into the sequence. It returns the new clip's id.
- A clip already on the timeline goes on top with move_clip {\"lane\": \"overlay\"}.
- set_transform then places it. A corner picture is scale 0.35 at x 0.78, y 0.2 (top right), 0.22 / 0.2 (top left), 0.78 / 0.8 (bottom right), 0.22 / 0.8 (bottom left).
- set_blend mixes it with what's under it: \"lighten\" or \"filter\" for light leaks and flares, \"darken\" or \"burn\" for dark textures (list_options blend_modes has them all).
- You can't import new files. If the library has nothing that fits, ask the user to import it.

Examples:
- \"put m2 in the top right corner at 3s\" → add_media {\"media\": \"m2\", \"lane\": \"overlay\", \"start\": 3}, then set_transform on the clip it returned {\"x\": 0.78, \"y\": 0.2, \"scale\": 0.35}
- \"make c3 a picture in picture\" → move_clip {\"clip\": \"c3\", \"lane\": \"overlay\"} then set_transform {\"clip\": \"c3\", \"x\": 0.78, \"y\": 0.8, \"scale\": 0.35}
- \"blend c4 in like a light leak\" → set_blend {\"clip\": \"c4\", \"mode\": \"lighten\"}",
    },
    Builtin {
        name: "clip_tools",
        description: "Duplicate a clip, freeze a frame, or pull a video's sound out onto its own audio clip.",
        triggers: &[
            "duplicate",
            "copy",
            "clone",
            "repeat the clip",
            "freeze",
            "hold the frame",
            "still frame",
            "pause the picture",
            "extract audio",
            "extract the audio",
            "detach",
            "separate the audio",
            "separate audio",
            "own audio clip",
        ],
        tools: &["duplicate_clip", "freeze_frame", "extract_audio"],
        body: "\
- duplicate_clip copies a clip with all its settings right after it (later main-lane clips move right).
- freeze_frame holds the picture at a time (default the playhead) for duration seconds (default 3): the clip splits around the still and later clips move right.
- extract_audio copies a video's sound onto the audio lane as its own clip and mutes the video, so the sound can be moved or trimmed apart.

Examples:
- \"duplicate the second clip\" → duplicate_clip {\"clip\": \"c2\"}
- \"freeze the frame at 2 seconds for 2 seconds\" → freeze_frame {\"time\": 2, \"duration\": 2}
- \"detach the audio from c1\" → extract_audio {\"clip\": \"c1\"}",
    },
    Builtin {
        name: "lanes",
        description: "Lock, hide or mute a whole lane (main, overlay, audio, text…).",
        triggers: &[
            "lane",
            "lanes",
            "lock",
            "unlock",
            "hide the",
            "unhide",
            "mute the main",
            "mute the overlay",
            "mute the audio lane",
            "mute all",
            "mute everything",
        ],
        tools: &["set_lane"],
        body: "\
- set_lane switches a whole lane: locked (no hand edits), hidden (not shown or exported), muted (main, overlay and audio lanes). Name it by kind (\"main\", \"overlay\", \"audio\", \"text\", \"stickers\", \"effect\") or by id (t2).
- One clip's sound is set_audio, not set_lane.

Examples:
- \"lock the main lane\" → set_lane {\"lane\": \"main\", \"locked\": true}
- \"hide the text\" → set_lane {\"lane\": \"text\", \"hidden\": true}
- \"mute the original sound of the video\" → set_lane {\"lane\": \"main\", \"muted\": true}",
    },
    Builtin {
        name: "stickers",
        description: "Sticker illustrations or animated GIFs at a moment and spot on the video.",
        triggers: &[
            "sticker",
            "stickers",
            "gif",
            "gifs",
            "emoji",
            "illustration",
            "clipart",
        ],
        tools: &["add_sticker"],
        body: "\
- add_sticker finds an illustration in the sticker library by search words (concrete things: \"coffee cup\", \"thumbs up\", \"money bag\") and pops it in at start for duration seconds. gif true takes an animated GIF instead.
- position is center, top, bottom, left, right, top_left, top_right, bottom_left, bottom_right, or \"x,y\" 0..1. size is a share of the canvas width (0.35).

Examples:
- \"add a thumbs up sticker at 2s\" → add_sticker {\"search\": \"thumbs up\", \"start\": 2}
- \"a small heart gif in the top right corner\" → add_sticker {\"search\": \"heart\", \"gif\": true, \"position\": \"top_right\", \"size\": 0.2}",
    },
    Builtin {
        name: "captions",
        description: "Captions on a voiceover, restyled (bold, box, yellow, clean).",
        triggers: &["caption", "captions", "subtitle", "subtitles", "subs"],
        tools: &["add_captions"],
        body: "\
- add_voiceover already captions the words it speaks. add_captions (re)captions a voiceover clip the agent made, in a style: bold, box, yellow or clean.
- Captions of the user's own speech can't be made yet: say so, and offer a voiceover with captions instead.

Examples:
- \"make the captions yellow\" → add_captions {\"style\": \"yellow\"}
- \"add subtitles to the voiceover\" → add_captions {}",
    },
    Builtin {
        name: "scene",
        description: "One drawn motion-graphic scene on the main lane: a chart, a counter, a comparison, steps, a flow diagram or code.",
        triggers: &[
            "chart",
            "graph",
            "counter",
            "comparison",
            "diagram",
            "scene",
        ],
        tools: &["add_scene"],
        body: "\
- add_scene draws one scene and puts it on the main lane (at the cut nearest start, default the end). make_video is for a whole video; add_scene is for one more scene in an edit.
- visual picks the graphic: bar_chart / line_chart (items \"Label: value\"), counter (to, with prefix \"$\" or suffix \"%\"), comparison, steps (items as short lines), flow (nodes in order), code, or background alone. What none of these shows: canvas, composed from elements (use_skill motion_graphics).

Examples:
- \"add a bar chart of our sales: 2023 $40k, 2024 $65k\" → add_scene {\"visual\": \"bar_chart\", \"title\": \"Sales\", \"items\": [\"2023: $40k\", \"2024: $65k\"]}
- \"a counter to 1 million users at the end\" → add_scene {\"visual\": \"counter\", \"title\": \"Users\", \"to\": 1000000}",
    },
    Builtin {
        name: "color_grade",
        description: "A colour look: grade the footage warm, cool, moody, vintage, bright or black and white.",
        triggers: &[
            "color grade",
            "colour grade",
            "grade",
            "grading",
            "warmer",
            "cooler",
            "moody",
            "vintage",
            "brighter",
            "darker",
            "contrast",
            "saturation",
            "saturated",
            "cinematic look",
            "black and white",
        ],
        tools: &["set_filter", "adjust_color"],
        body: "\
- adjust_color moves the colour sliders of one clip (-100..100, 0 = untouched; only the ones given change). set_filter puts a named look on it (list_options filters, search narrows it).
- A grade goes on every video clip on the main lane: one call per clip.
- Recipes: warm → temperature 25, saturation 10. cool → temperature -25. moody → contrast 20, saturation -25, exposure -10. bright → exposure 15, brightness 10. punchy → contrast 25, saturation 20. black and white → saturation -100 (or a black-and-white filter).

Examples (main clips c1, c2):
- \"make it warmer\" → adjust_color {\"clip\": \"c1\", \"temperature\": 25, \"saturation\": 10} then adjust_color {\"clip\": \"c2\", \"temperature\": 25, \"saturation\": 10}
- \"a vintage look\" → list_options {\"what\": \"filters\", \"search\": \"vint\"} then set_filter on each clip",
    },
    Builtin {
        name: "highlight_reel",
        description: "Cut long footage down to its best moments, then finish it with a title, music and sound.",
        triggers: &[
            "highlight",
            "highlights",
            "best moments",
            "best parts",
            "recap",
            "montage",
            "cut it down",
            "cut down",
            "trailer",
        ],
        tools: &["look", "split_clip", "delete_clip", "check_video"],
        body: "\
Work through these steps in order, without asking at each one:
1. look {} to see what the video shows and where the good moments are.
2. Cut the dull parts out: split_clip at the start and end of each dull part, then delete_clip the piece in between (use the ids split_clip returns). The magnet closes the gaps.
3. add_title with a short title for the reel at 0s (style pop or headline).
4. add_music {\"mood\": \"upbeat\"} (or a mood that fits what look saw).
5. sound_design {}.
6. check_video {} and make the fixes it lists.
Then say in two sentences what you kept and what you added.",
    },
];

pub fn builtins() -> Vec<Skill> {
    BUILTINS
        .iter()
        .map(|b| Skill {
            name: b.name.into(),
            description: b.description.into(),
            triggers: b.triggers.iter().map(|t| t.to_string()).collect(),
            tools: b.tools.iter().map(|t| t.to_string()).collect(),
            body: b.body.into(),
            user: false,
        })
        .collect()
}

// ---- the user's skills ----------------------------------------------------------------

pub fn folder() -> PathBuf {
    crate::prefs::data_dir().join("skills")
}

/// The built-in skills, then the user's (one replacing a built-in of the
/// same name takes its place). Read on every turn: cheap, and a file just
/// saved counts at once.
pub fn load() -> Vec<Skill> {
    let mut skills = builtins();
    let mut files: Vec<PathBuf> = std::fs::read_dir(folder())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("md")))
        .collect();
    // A stable order: the list is part of the prompt the server caches.
    files.sort();
    for path in files {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(skill) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| parse(&text, &stem))
        else {
            continue;
        };
        match skills.iter_mut().find(|s| s.name == skill.name) {
            Some(slot) => *slot = skill,
            None => skills.push(skill),
        }
    }
    skills
}

/// A skill file: an optional `---` header of `key: value` lines (name,
/// description, triggers, tools; lists comma-separated), then the body.
/// Unknown tools are dropped; a file with no body is no skill.
pub fn parse(text: &str, file_stem: &str) -> Option<Skill> {
    let text = text.trim_start_matches('\u{feff}');
    let (header, body) = match text.trim_start().strip_prefix("---") {
        Some(rest) => {
            let end = rest.find("\n---")?;
            let after = &rest[end + 4..];
            (&rest[..end], after.split_once('\n').map_or("", |(_, b)| b))
        }
        None => ("", text),
    };
    let mut name = String::new();
    let mut description = String::new();
    let mut triggers = vec![];
    let mut tools_list = vec![];
    for line in header.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'']).trim();
        let list = || -> Vec<String> {
            value
                .trim_matches(['[', ']'])
                .split(',')
                .map(|s| s.trim().trim_matches(['"', '\'']).trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
        match key.trim().to_lowercase().as_str() {
            "name" => name = value.to_string(),
            "description" => description = value.to_string(),
            "triggers" | "keywords" => triggers = list(),
            "tools" => tools_list = list(),
            _ => {}
        }
    }
    let name = slug(if name.is_empty() { file_stem } else { &name });
    let body = cut(body.trim(), MAX_BODY_CHARS);
    if name.is_empty() || body.is_empty() {
        return None;
    }
    if description.is_empty() {
        description = body.lines().next().unwrap_or_default().to_string();
    }
    let mut tool_names: Vec<String> = tools_list
        .into_iter()
        .filter(|t| tools::find(t).is_some())
        .collect();
    tool_names.dedup();
    tool_names.truncate(MAX_TOOLS);
    Some(Skill {
        name,
        description: cut(description.trim(), MAX_DESCRIPTION_CHARS),
        triggers: triggers.iter().map(|t| t.to_lowercase()).collect(),
        tools: tool_names,
        body,
        user: true,
    })
}

/// "Channel Intro" → "channel_intro".
fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

fn cut(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

// ---- routing --------------------------------------------------------------------------

/// Lower-case words split on anything else, padded: " fade c1 in ".
fn words(s: &str) -> String {
    let mut out = String::from(" ");
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

/// The skills a request's words call for, best first: a trigger counts
/// as many times as it has words, so "picture in picture" beats "corner".
pub fn route<'a>(skills: &'a [Skill], request: &str) -> Vec<&'a Skill> {
    let text = words(request);
    let mut scored: Vec<(usize, usize, &Skill)> = skills
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let score: usize = s
                .triggers
                .iter()
                .map(|t| words(t))
                .filter(|t| t.trim().len() > 1 && text.contains(t.as_str()))
                .map(|t| t.split_whitespace().count())
                .sum();
            (score > 0).then_some((score, i, s))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(MAX_ROUTED)
        .map(|(_, _, s)| s)
        .collect()
}

// ---- what the model reads -------------------------------------------------------------

/// The system prompt's list (fixed while the files don't change).
pub fn catalog(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut s = String::from(
        "\n\nSkills: more tools and know-how for one kind of job. A request's skill usually comes loaded with it (\"Skill loaded\"). When a request needs one that isn't, call use_skill with its name first, then follow what it returns:",
    );
    for skill in skills {
        s.push_str(&format!("\n- {}: {}", skill.name, skill.description));
    }
    s
}

/// A skill as the model gets it, in the request or a `use_skill` result.
pub fn instructions(skill: &Skill, cloud: bool) -> String {
    let tools = skill
        .tools
        .iter()
        .filter(|t| cloud || tools::find(t).is_some())
        .cloned()
        .collect::<Vec<_>>();
    let using = if tools.is_empty() {
        String::new()
    } else {
        format!(" Tools: {}.", tools.join(", "))
    };
    format!("Skill loaded: {}.{using}\n{}", skill.name, skill.body)
}

/// The request's message with its routed skills: the project state, the
/// skills, then the request last (where a small model looks hardest).
pub fn request(state: &Value, routed: &[&Skill], text: &str, cloud: bool) -> String {
    if routed.is_empty() {
        return super::request_with_state(state, text);
    }
    let skills = routed
        .iter()
        .map(|s| instructions(s, cloud))
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("Current project state:\n{state}\n\n{skills}\n\nRequest: {text}")
}

fn use_skill_def(skills: &[Skill]) -> ToolDef {
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    ToolDef {
        name: USE_SKILL,
        description: "Load a skill (listed in your instructions): its steps, examples and tools for one kind of job. Call it before the job when that skill isn't loaded yet.".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "enum": names, "description": "The skill" }
            },
            "required": ["name"],
        }),
    }
}

// ---- the turn's skills ----------------------------------------------------------------

/// The skills a turn has loaded, routed or asked for.
#[derive(Clone, Debug, Default)]
pub struct Loaded(pub Vec<String>);

impl Loaded {
    pub fn routed(routed: &[&Skill]) -> Self {
        Loaded(routed.iter().map(|s| s.name.clone()).collect())
    }

    /// The turn's tools: the fixed set (the cached prefix), `use_skill`,
    /// then the loaded skills' extra tools.
    pub fn tools(&self, skills: &[Skill], cloud: bool) -> Vec<ToolDef> {
        let mut defs = tools::defs(cloud);
        if !skills.is_empty() {
            defs.push(use_skill_def(skills));
        }
        for name in &self.0 {
            let Some(skill) = skills.iter().find(|s| &s.name == name) else {
                continue;
            };
            for tool in &skill.tools {
                if defs.iter().any(|d| d.name == tool.as_str()) {
                    continue;
                }
                if let Some(t) = tools::find(tool) {
                    defs.push(tools::def(t));
                }
            }
        }
        defs
    }

    /// Is this call the harness's to run? `use_skill`, or a skill's own
    /// name called as if it were a tool (what a small model does).
    pub fn handles(skills: &[Skill], call: &str) -> bool {
        call == USE_SKILL || skills.iter().any(|s| s.name == call)
    }

    /// Runs a `use_skill` call: the skill's instructions as its result,
    /// its tools offered from the next round on. A skill called by its own
    /// name is loaded too, but answers as an error: the model believes a
    /// call that went through did the edit it meant.
    pub fn load_call(
        &mut self,
        skills: &[Skill],
        call: &str,
        args: &Map<String, Value>,
        cloud: bool,
    ) -> ToolResult {
        if call == USE_SKILL {
            return self.load(skills, args, cloud);
        }
        let result = self.load(
            skills,
            &Map::from_iter([("name".to_string(), Value::from(call))]),
            cloud,
        )?;
        let instructions = result["instructions"].as_str().unwrap_or_default();
        Err(ToolError(format!(
            "Nothing changed: {call} is a skill, not a tool. It is loaded now, so call its tools to make the edit.\n{instructions}"
        )))
    }

    pub fn load(&mut self, skills: &[Skill], args: &Map<String, Value>, cloud: bool) -> ToolResult {
        let wanted = args
            .get("name")
            .or_else(|| args.get("skill"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
        if wanted.trim().is_empty() {
            return Err(ToolError(format!(
                "\"name\" is required: one of {}",
                names.join(", ")
            )));
        }
        let name = tools::pick_name(wanted, &names, "skill")?.clone();
        let skill = skills.iter().find(|s| s.name == name).expect("listed");
        if !self.0.contains(&name) {
            self.0.push(name.clone());
        }
        Ok(json!({
            "skill": name,
            "instructions": instructions(skill, cloud),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&Skill]) -> Vec<String> {
        v.iter().map(|s| s.name.clone()).collect()
    }

    #[test]
    fn builtins_name_real_tools() {
        for s in builtins() {
            for t in &s.tools {
                assert!(tools::find(t).is_some(), "{}: no tool {t}", s.name);
            }
            assert!(s.description.len() <= MAX_DESCRIPTION_CHARS, "{}", s.name);
            assert!(s.body.len() <= MAX_BODY_CHARS, "{}", s.name);
            assert_eq!(slug(&s.name), s.name);
        }
    }

    #[test]
    fn routes_by_words() {
        let skills = builtins();
        assert_eq!(
            names(&route(&skills, "Freeze the frame at 2s")),
            ["clip_tools"]
        );
        assert_eq!(
            names(&route(
                &skills,
                "Make m2 a picture-in-picture in the corner"
            )),
            ["overlay"]
        );
        assert_eq!(
            names(&route(&skills, "move c1 from the left to the right")),
            ["keyframes"]
        );
        // Whole words only: "unlocked" is not "lock", "pipe" not "pip".
        assert!(route(&skills, "unlocked pipe dreams").is_empty());
        // Everyday core edits get nothing extra.
        assert!(route(&skills, "split the first clip at 1.5 seconds").is_empty());
        assert!(route(&skills, "add some chill music").is_empty());
        assert!(route(&skills, "add sound effects").is_empty());
        assert!(route(&skills, "what happens in my video?").is_empty());
        assert!(route(&skills, "put an animated title at the start").is_empty());
        assert!(route(&skills, "x").is_empty());
        // Making a video is written scene by scene; a composed one loads
        // the canvas too.
        assert_eq!(
            names(&route(
                &skills,
                "Make a 60-second explainer about saving money"
            )),
            ["video"]
        );
        assert_eq!(
            names(&route(
                &skills,
                "create a video about black holes with motion graphics"
            )),
            ["video", "motion_graphics"]
        );
        assert_eq!(
            names(&route(
                &skills,
                "is anything wrong? check my video before I export"
            )),
            ["review"]
        );
    }

    #[test]
    fn routes_two_at_most() {
        let skills = builtins();
        let r = route(
            &skills,
            "duplicate c1, freeze it, lock the lane, add a sticker and a chart and subtitles",
        );
        assert_eq!(r.len(), MAX_ROUTED);
        assert_eq!(r[0].name, "clip_tools");
    }

    #[test]
    fn reads_a_skill_file() {
        let s = parse(
            "---\nname: Channel Intro\ndescription: \"My intro\"\ntriggers: intro, Channel Opener\ntools: [add_title, add_music, make_coffee]\n---\n1. add_title {\"text\": \"Hi\"}\n",
            "file",
        )
        .unwrap();
        assert_eq!(s.name, "channel_intro");
        assert_eq!(s.description, "My intro");
        assert_eq!(s.triggers, ["intro", "channel opener"]);
        assert_eq!(s.tools, ["add_title", "add_music"]);
        assert_eq!(s.body, "1. add_title {\"text\": \"Hi\"}");
        assert!(s.user);
    }

    #[test]
    fn a_file_without_a_header_is_named_after_itself() {
        let s = parse("Always add a neon title first.", "My Rules").unwrap();
        assert_eq!(s.name, "my_rules");
        assert_eq!(s.description, "Always add a neon title first.");
        assert!(s.tools.is_empty());
        assert!(parse("---\nname: x\n---\n  \n", "x").is_none());
        assert!(parse("---\nname: x\nno end", "x").is_none());
    }

    #[test]
    fn a_long_file_is_cut() {
        let s = parse(&"word ".repeat(2000), "long").unwrap();
        assert!(s.body.chars().count() <= MAX_BODY_CHARS + 1);
    }

    #[test]
    fn tools_keep_the_fixed_start() {
        let skills = builtins();
        let fixed = tools::defs(false);
        let none = Loaded::default().tools(&skills, false);
        assert_eq!(none.len(), fixed.len() + 1);
        assert_eq!(none.last().unwrap().name, USE_SKILL);
        let loaded = Loaded::routed(&route(&skills, "keyframe c1 and freeze c2"));
        let more = loaded.tools(&skills, false);
        for (a, b) in fixed.iter().zip(&more) {
            assert_eq!(a.name, b.name);
        }
        let added: Vec<&str> = more[fixed.len() + 1..].iter().map(|d| d.name).collect();
        assert!(added.contains(&"add_keyframe"), "{added:?}");
        assert!(added.contains(&"freeze_frame"), "{added:?}");
        // Tools the model already has aren't sent twice.
        let mut seen = std::collections::HashSet::new();
        assert!(more.iter().all(|d| seen.insert(d.name)));
        // A cloud model has every tool already.
        let cloud = loaded.tools(&skills, true);
        assert_eq!(cloud.len(), tools::defs(true).len() + 1);
    }

    #[test]
    fn use_skill_loads_by_name() {
        let skills = builtins();
        let mut loaded = Loaded::default();
        let args = |v: Value| v.as_object().unwrap().clone();
        let r = loaded
            .load(&skills, &args(json!({ "name": "Keyframes" })), false)
            .unwrap();
        assert_eq!(r["skill"], "keyframes");
        assert!(r["instructions"].as_str().unwrap().contains("add_keyframe"));
        assert_eq!(loaded.0, ["keyframes"]);
        loaded
            .load(&skills, &args(json!({ "name": "keyframes" })), false)
            .unwrap();
        assert_eq!(loaded.0.len(), 1);
        let err = loaded
            .load(&skills, &args(json!({ "name": "juggling" })), false)
            .unwrap_err();
        assert!(err.0.contains("Closest"), "{err:?}");
        assert!(loaded.load(&skills, &Map::new(), false).is_err());
    }

    #[test]
    fn a_skill_called_as_a_tool_loads_it() {
        let skills = builtins();
        assert!(Loaded::handles(&skills, USE_SKILL));
        assert!(Loaded::handles(&skills, "clip_tools"));
        assert!(!Loaded::handles(&skills, "duplicate_clip"));
        let mut loaded = Loaded::default();
        let args = json!({ "action": "duplicate", "clip": "c1" });
        let err = loaded
            .load_call(&skills, "clip_tools", args.as_object().unwrap(), false)
            .unwrap_err();
        assert!(err.0.contains("not a tool"), "{err:?}");
        assert!(err.0.contains("duplicate_clip"), "{err:?}");
        let tools = loaded.tools(&skills, false);
        assert!(tools.iter().any(|d| d.name == "duplicate_clip"));
    }

    #[test]
    fn the_request_puts_skills_before_the_words() {
        let skills = builtins();
        let routed = route(&skills, "freeze at 2s");
        let r = request(&json!({}), &routed, "freeze at 2s", false);
        let skill = r.find("Skill loaded: clip_tools").unwrap();
        let words = r.find("Request: freeze at 2s").unwrap();
        assert!(skill < words);
        assert_eq!(
            request(&json!({}), &[], "hi", false),
            super::super::request_with_state(&json!({}), "hi")
        );
    }
}
