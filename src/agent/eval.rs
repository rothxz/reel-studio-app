//! Does a real model do the job? Requests run through the agent's own
//! prompt, tools and loop rules against a test project, then the project is
//! checked. Ignored: they need a model server (and Openverse for sounds).
//!
//! ```sh
//! REEL_EVAL_MODEL=qwen3-vl:4b-instruct cargo t eval -- --ignored --nocapture
//! ```
//! `REEL_EVAL_URL` (default Ollama's) and `REEL_EVAL_DIALECT` (ollama /
//! openai) pick another server.

use serde_json::{Value, json};

use super::llm::{self, Message, Server};
use super::providers::Dialect;
use super::skills::{self, Loaded};
use super::snapshot::{SnapshotContext, snapshot};
use super::tools::test_env::Fixture;

fn server() -> Server {
    let dialect = match std::env::var("REEL_EVAL_DIALECT").as_deref() {
        Ok("openai") => Dialect::OpenAi,
        _ => Dialect::Ollama,
    };
    Server {
        dialect,
        base_url: std::env::var("REEL_EVAL_URL")
            .unwrap_or_else(|_| "http://localhost:11434".into()),
        model: std::env::var("REEL_EVAL_MODEL").unwrap_or_else(|_| "qwen3-vl:4b-instruct".into()),
        auth: None,
    }
}

/// One turn, headless: the calls made (name, ok) and the final answer.
fn turn(f: &mut Fixture, request: &str) -> (Vec<(String, bool)>, String) {
    turn_with(f, request, true)
}

/// `route` off: the model must find the skill itself (`use_skill`). The
/// built-in skills only, so the user's own files don't move the score.
fn turn_with(f: &mut Fixture, request: &str, route: bool) -> (Vec<(String, bool)>, String) {
    let server = server();
    let selected = f.selected.clone();
    let state = snapshot(
        &f.project,
        &mut f.aliases,
        &SnapshotContext {
            playhead: f.playhead,
            selected: selected.as_deref(),
            media: &f.media,
        },
    );
    let all = skills::builtins();
    let routed = if route {
        skills::route(&all, request)
    } else {
        vec![]
    };
    if !routed.is_empty() {
        let names: Vec<&str> = routed.iter().map(|s| s.name.as_str()).collect();
        eprintln!("  (skills: {})", names.join(", "));
    }
    let mut msgs = vec![
        Message::System(super::system_prompt(false, &all)),
        Message::User(skills::request(&state, &routed, request, false)),
    ];
    let mut loaded = Loaded::routed(&routed);
    let mut calls = vec![];
    let mut seen = vec![];
    let (mut edited, mut nudged) = (false, false);
    for _ in 0..super::MAX_LOCAL_ROUNDS {
        let defs = loaded.tools(&all, false);
        let reply = llm::chat(&server, &msgs, &defs).expect("model answers");
        msgs.push(Message::Assistant {
            content: reply.content.clone(),
            calls: reply.calls.clone(),
        });
        if reply.calls.is_empty() {
            if !edited && !nudged && super::claims_edit(&reply.content) {
                eprintln!("  (claimed an edit without making it: nudged)");
                nudged = true;
                msgs.push(Message::User(super::NUDGE.into()));
                continue;
            }
            return (calls, reply.content);
        }
        for call in reply.calls {
            seen.push((
                call.name.clone(),
                Value::Object(call.arguments.clone()).to_string(),
            ));
            assert!(!super::looping(&seen), "looping on {}", call.name);
            let args = Value::Object(call.arguments.clone());
            eprintln!("  → {} {}", call.name, args);
            let result = if Loaded::handles(&all, &call.name) {
                loaded.load_call(&all, &call.name, &call.arguments, false)
            } else {
                f.call(&call.name, args)
            };
            if let Err(e) = &result {
                eprintln!("    ✗ {}", e.0);
            }
            calls.push((call.name.clone(), result.is_ok()));
            edited |= result.is_ok()
                && !super::READ_ONLY.contains(&call.name.as_str())
                && !Loaded::handles(&all, &call.name);
            msgs.push(Message::Tool {
                call_id: call.id,
                name: call.name,
                content: super::tool_content(&result, false),
                error: result.is_err(),
            });
        }
    }
    panic!("no answer within the round limit");
}

fn did(calls: &[(String, bool)], name: &str) -> bool {
    calls.iter().any(|(n, ok)| n == name && *ok)
}

fn texts(f: &Fixture) -> Vec<String> {
    f.project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.kind == "text")
        .filter_map(|c| c.text.clone())
        .collect()
}

#[test]
#[ignore]
fn eval_local_model() {
    let mut score = 0;
    let mut total = 0;
    let mut check = |name: &str, ok: bool| {
        total += 1;
        if ok {
            score += 1;
        }
        eprintln!("{} {name}", if ok { "PASS" } else { "FAIL" });
    };

    let mut f = Fixture::with_main_clips(3, 3.);
    let (c, a) = turn(&mut f, "Split the first clip at 1.5 seconds");
    eprintln!("  ← {a}");
    check(
        "split",
        did(&c, "split_clip") && f.project.ordered_main_clips().len() == 4,
    );

    let mut f = Fixture::with_main_clips(3, 3.);
    let (c, a) = turn(&mut f, "Put an animated title “Summer trip” at the start");
    eprintln!("  ← {a}");
    check(
        "title",
        did(&c, "add_title")
            && texts(&f)
                .iter()
                .any(|t| t.to_lowercase().contains("summer")),
    );

    let mut f = Fixture::with_main_clips(2, 4.);
    f.selected = Some(f.id("c1"));
    let (c, a) = turn(&mut f, "slow zoom in on this clip");
    eprintln!("  ← {a}");
    let id = f.id("c1");
    check(
        "zoom",
        did(&c, "animate")
            && !f
                .project
                .find_clip(&id)
                .unwrap()
                .1
                .keyframe_times(1)
                .is_empty(),
    );

    let mut f = Fixture::with_main_clips(3, 3.);
    let (c, a) = turn(&mut f, "make the second clip twice as fast");
    eprintln!("  ← {a}");
    let id = f.id("c2");
    check(
        "speed",
        did(&c, "set_speed") && (f.project.find_clip(&id).unwrap().1.speed - 2.).abs() < 1e-6,
    );

    let mut f = Fixture::with_main_clips(3, 3.);
    f.call(
        "add_title",
        json!({ "text": "Day one", "style": "headline", "start": 0.5 }),
    )
    .unwrap();
    let (c, a) = turn(&mut f, "add sound effects to my video");
    eprintln!("  ← {a}");
    let sfx = f
        .project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.extra.contains_key("agentSfx"))
        .count();
    check("sound design", did(&c, "sound_design") && sfx >= 2);

    let mut f = Fixture::with_main_clips(2, 4.);
    let (c, a) = turn(&mut f, "add a whoosh sound at 4 seconds");
    eprintln!("  ← {a}");
    check("one sound", did(&c, "add_sound") || did(&c, "sound_design"));

    let mut f = Fixture::with_main_clips(2, 4.);
    let (c, a) = turn(
        &mut f,
        "Add a lower third that says Ana Lopez, Product designer",
    );
    eprintln!("  ← {a}");
    check(
        "lower third",
        did(&c, "add_title") && texts(&f).iter().any(|t| t.contains("Ana")),
    );

    let mut f = Fixture::with_main_clips(2, 4.);
    let (c, a) = turn(
        &mut f,
        "My video is about cooking pasta. Add a fitting animated title at the start.",
    );
    eprintln!("  ← {a}");
    check(
        "fitting title",
        did(&c, "add_title") && !texts(&f).is_empty(),
    );

    let mut f = Fixture::with_main_clips(2, 4.);
    let (c, a) = turn(&mut f, "make the video vertical for TikTok");
    eprintln!("  ← {a}");
    check(
        "ratio",
        did(&c, "set_aspect_ratio") || f.project.canvas.height > f.project.canvas.width,
    );

    eprintln!("\n{} — {score}/{total}", server().model);
    // A threshold, not perfection: this is a small model.
    assert!(score * 4 >= total * 3, "{score}/{total}");
}

/// Jobs only a skill's tools do: routed by the request's words, and (the
/// last ones) found by the model itself with `use_skill`.
#[test]
#[ignore]
fn eval_skills() {
    let mut score = 0;
    let mut total = 0;
    let mut check = |name: &str, ok: bool| {
        total += 1;
        if ok {
            score += 1;
        }
        eprintln!("{} {name}", if ok { "PASS" } else { "FAIL" });
    };
    let keyed = |f: &Fixture, alias: &str| {
        let id = f.id(alias);
        let clip = f.project.find_clip(&id).unwrap().1;
        clip.keyframe_times(1).len() >= 2
    };

    let mut f = Fixture::with_main_clips(3, 3.);
    let (c, a) = turn(&mut f, "Freeze the frame at 2 seconds for 2 seconds");
    eprintln!("  ← {a}");
    check(
        "freeze",
        did(&c, "freeze_frame") && f.project.ordered_main_clips().len() >= 4,
    );

    let mut f = Fixture::with_main_clips(2, 4.);
    let (c, a) = turn(&mut f, "Duplicate the second clip");
    eprintln!("  ← {a}");
    check(
        "duplicate",
        did(&c, "duplicate_clip") && f.project.ordered_main_clips().len() == 3,
    );

    let mut f = Fixture::with_main_clips(2, 4.);
    let (c, a) = turn(&mut f, "Detach the audio from the first clip");
    eprintln!("  ← {a}");
    check("extract audio", did(&c, "extract_audio"));

    let mut f = Fixture::with_main_clips(2, 4.);
    let (c, a) = turn(&mut f, "Lock the main lane");
    eprintln!("  ← {a}");
    let main = f
        .project
        .tracks
        .iter()
        .find(|t| f.project.is_main_track(&t.id));
    check(
        "lock lane",
        did(&c, "set_lane") && main.is_some_and(|t| t.locked),
    );

    let mut f = Fixture::with_main_clips(2, 4.);
    let (c, a) = turn(
        &mut f,
        "Use keyframes to move c1 from the left of the screen to the right over its length",
    );
    eprintln!("  ← {a}");
    check("keyframes", did(&c, "add_keyframe") && keyed(&f, "c1"));

    let mut f = Fixture::with_main_clips(2, 4.);
    let (c, a) = turn(
        &mut f,
        "Fade c2 in from fully transparent over its first second",
    );
    eprintln!("  ← {a}");
    check("fade by hand", keyed(&f, "c2") || did(&c, "animate"));

    // Not routed: the list in the system prompt is all it has.
    let mut f = Fixture::with_main_clips(3, 3.);
    let (c, a) = turn_with(
        &mut f,
        "Hold the picture of the first clip still for 2 seconds at 1s",
        false,
    );
    eprintln!("  ← {a}");
    check("use_skill: freeze", did(&c, "freeze_frame"));

    let mut f = Fixture::with_main_clips(2, 4.);
    let (c, a) = turn_with(&mut f, "Make a copy of c1", false);
    eprintln!("  ← {a}");
    check(
        "use_skill: copy",
        did(&c, "duplicate_clip") && f.project.ordered_main_clips().len() == 3,
    );

    eprintln!("\n{} — skills {score}/{total}", server().model);
    assert!(score * 4 >= total * 3, "{score}/{total}");
}

/// After a warm-up, the first real request doesn't load the model or read
/// the instructions + tools again (`REEL_AGENT_LOG=1` shows the times).
#[test]
#[ignore]
fn eval_warm_up() {
    let server = server();
    let all = skills::builtins();
    let defs = Loaded::default().tools(&all, false);
    let system = super::system_prompt(false, &all);
    // Unload it first: a cold start, as at launch.
    let _ = ureq::post(format!("{}/api/generate", server.base_url))
        .send(json!({ "model": server.model, "keep_alive": 0 }).to_string());
    let t = std::time::Instant::now();
    llm::warm_up(&server, &system, &defs).expect("warm-up");
    eprintln!("warm-up {} ms", t.elapsed().as_millis());
    let mut f = Fixture::with_main_clips(3, 3.);
    let t = std::time::Instant::now();
    let (calls, _) = turn(&mut f, "Split the first clip at 1.5 seconds");
    eprintln!("first request {} ms", t.elapsed().as_millis());
    assert!(did(&calls, "split_clip"));
}

/// A whole video from a long brief (the case that used to fail outright).
/// `REEL_EVAL_BRIEF` names a text file with the brief; it defaults to a
/// short one.
#[test]
#[ignore]
fn eval_make_video() {
    crate::decoder::init();
    let brief = std::env::var("REEL_EVAL_BRIEF")
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_else(|| "Make a 60-second vertical explainer about daily spending habits: where small expenses go, needs vs wants, a simple budget, and how saving $5 a day adds up. Friendly narration, subtitles, charts.".into());
    // REEL_EVAL_PROJECT: a project file to start from (default: empty).
    let project = std::env::var("REEL_EVAL_PROJECT")
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(crate::model::Project::empty);
    let mut f = Fixture::new(project);
    let t = std::time::Instant::now();
    let (c, a) = turn(&mut f, brief.trim());
    eprintln!("  ← {a}");
    eprintln!("  {:.0} s", t.elapsed().as_secs_f32());
    assert!(did(&c, "make_video"), "{c:?}");
    let scenes = f.project.ordered_main_clips().len();
    let voices = f
        .project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.extra.contains_key("voiceoverText"))
        .count();
    eprintln!(
        "  {scenes} scenes, {voices} voices, {:.1} s",
        f.project.duration as f64 / 1e6
    );
    assert!(scenes >= 3 && voices == scenes);
}
