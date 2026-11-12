//! `check_video`: the timeline read the way an editor looks it over before
//! exporting. It lists what a viewer would notice (black between clips,
//! sound running on after the picture, two voices at once, words gone
//! before they can be read, music drowning the narrator) and, for each,
//! the call that fixes it, so a model can finish its own work instead of
//! stopping at "done".
//!
//! It only reads: nothing is changed, whatever it finds.

use serde_json::{Value, json};

use super::{Args, Env, Outcome, Tier, Tool, ToolResult};
use crate::agent::snapshot::{Aliases, Kind, Lane, lane_of, secs};
use crate::model::{Clip, Micros, Project, SECOND};

/// Problems listed at most (the worst come first).
const MAX_ISSUES: usize = 12;
/// Music louder than this under a voice fights it.
const LOUD_UNDER_VOICE: f64 = 0.35;
/// A photo held longer than this without moving looks frozen.
const LONG_STILL: Micros = 6 * SECOND;

#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub problem: String,
    pub fix: String,
}

fn issue(problem: String, fix: String) -> Issue {
    Issue { problem, fix }
}

/// Seconds words need on screen to be read: half a second, then a third
/// of a second a word.
fn read_time(words: usize) -> Micros {
    ((0.5 + 0.3 * words as f64).min(8.) * SECOND as f64) as Micros
}

fn overlap(a: &Clip, b: &Clip) -> Micros {
    a.end().min(b.end()) - a.timeline_start.max(b.timeline_start)
}

/// What's wrong with the timeline, and softer suggestions.
pub fn issues(p: &Project, aliases: &mut Aliases) -> (Vec<Issue>, Vec<String>) {
    let mut out = vec![];
    let mut tips = vec![];
    let frame = if p.canvas.fps > 0. {
        (SECOND as f64 / p.canvas.fps) as Micros
    } else {
        SECOND / 30
    };
    let mut id = |c: &Clip| aliases.alias(Kind::Clip, &c.id);
    let main: Vec<&Clip> = p.ordered_main_clips();
    let Some(picture_end) = main.iter().map(|c| c.end()).max() else {
        return (
            vec![issue(
                "The main lane is empty: there is no video yet.".into(),
                "add_media puts a library file on it; make_video or start_video makes one from a script.".into(),
            )],
            tips,
        );
    };
    // The lanes that show or sound, by kind.
    let on = |lane: Lane| -> Vec<&Clip> {
        let mut v: Vec<&Clip> = p
            .tracks
            .iter()
            .filter(|t| !t.hidden && lane_of(p, t) == lane)
            .flat_map(|t| t.clips.iter())
            .filter(|c| !c.hidden)
            .collect();
        v.sort_by_key(|c| c.timeline_start);
        v
    };
    let audio = on(Lane::Audio);
    let texts = on(Lane::Text);

    // Black: before the first clip, between two.
    if let Some(first) = main.first()
        && first.timeline_start > frame
    {
        out.push(issue(
            format!(
                "The video opens on {:.1} s of black before {}.",
                secs(first.timeline_start),
                id(first)
            ),
            format!("move_clip {{\"clip\": \"{}\", \"start\": 0}}", id(first)),
        ));
    }
    for w in main.windows(2) {
        let gap = w[1].timeline_start - w[0].end();
        if gap > frame {
            out.push(issue(
                format!(
                    "{:.1} s of black between {} and {} (at {} s).",
                    secs(gap),
                    id(w[0]),
                    id(w[1]),
                    secs(w[0].end())
                ),
                format!(
                    "move_clip {{\"clip\": \"{}\", \"start\": {}}}",
                    id(w[1]),
                    secs(w[0].end())
                ),
            ));
        }
    }

    // Two voices at once.
    let voices: Vec<&Clip> = audio
        .iter()
        .copied()
        .filter(|c| c.extra.contains_key("agentVoice"))
        .collect();
    for w in voices.windows(2) {
        if overlap(w[0], w[1]) > SECOND * 15 / 100 {
            out.push(issue(
                format!(
                    "The voiceovers {} and {} talk over each other at {} s.",
                    id(w[0]),
                    id(w[1]),
                    secs(w[1].timeline_start)
                ),
                format!(
                    "move_clip {{\"clip\": \"{}\", \"start\": {}}}",
                    id(w[1]),
                    secs(w[0].end())
                ),
            ));
        }
    }

    // Music that drowns the narrator.
    for c in audio.iter().filter(|c| !c.extra.contains_key("agentVoice")) {
        let under: Micros = voices.iter().map(|v| overlap(c, v).max(0)).sum();
        if under > SECOND && !c.muted() && c.volume() > LOUD_UNDER_VOICE {
            out.push(issue(
                format!(
                    "{} plays at {:.0}% under the voiceover: the words will be hard to hear.",
                    id(c),
                    c.volume() * 100.
                ),
                format!("set_audio {{\"clip\": \"{}\", \"volume\": 13}}", id(c)),
            ));
        }
    }

    // Sound, words or pictures running on after the video is over.
    let stickers = on(Lane::Stickers);
    let overlays = on(Lane::Overlay);
    for (c, slack) in audio
        .iter()
        .map(|c| (*c, SECOND / 4))
        .chain(texts.iter().map(|c| (*c, frame * 2)))
        .chain(stickers.iter().map(|c| (*c, frame * 2)))
        .chain(overlays.iter().map(|c| (*c, frame * 2)))
    {
        if c.end() > picture_end + slack {
            let (what, fix) = if c.timeline_start >= picture_end {
                (
                    format!(
                        "{} ({}) starts after the video has ended ({} s): it plays over black.",
                        id(c),
                        c.kind,
                        secs(picture_end)
                    ),
                    format!(
                        "move_clip it earlier, or delete_clip {{\"clip\": \"{}\"}}",
                        id(c)
                    ),
                )
            } else {
                (
                    format!(
                        "{} ({}) runs {:.1} s past the end of the video ({} s).",
                        id(c),
                        c.kind,
                        secs(c.end() - picture_end),
                        secs(picture_end)
                    ),
                    format!(
                        "trim_clip {{\"clip\": \"{}\", \"end\": {}}}",
                        id(c),
                        secs(picture_end)
                    ),
                )
            };
            out.push(issue(what, fix));
        }
    }

    // Captions on top of each other.
    let captions: Vec<&Clip> = texts.iter().copied().filter(|c| c.is_caption()).collect();
    for w in captions.windows(2) {
        if overlap(w[0], w[1]) > SECOND / 10 {
            out.push(issue(
                format!(
                    "The captions {} and {} show at the same time at {} s.",
                    id(w[0]),
                    id(w[1]),
                    secs(w[1].timeline_start)
                ),
                format!(
                    "trim_clip {{\"clip\": \"{}\", \"end\": {}}}",
                    id(w[0]),
                    secs(w[1].timeline_start)
                ),
            ));
        }
    }

    // Words gone before they can be read. A title animated word by word
    // or typed out is a run of short clips end to end: those are left
    // alone.
    let words: Vec<&Clip> = texts.iter().copied().filter(|c| !c.is_caption()).collect();
    for (i, c) in words.iter().enumerate() {
        let count = c
            .text
            .as_deref()
            .map_or(0, |t| t.split_whitespace().count());
        let in_a_run = words.iter().enumerate().any(|(k, o)| {
            k != i
                && ((o.timeline_start - c.end()).abs() <= frame * 2
                    || (c.timeline_start - o.end()).abs() <= frame * 2)
        });
        let need = read_time(count);
        if count >= 3 && !in_a_run && c.timeline_duration * 10 < need * 8 {
            let text: String = c
                .text
                .as_deref()
                .unwrap_or_default()
                .chars()
                .take(40)
                .collect();
            out.push(issue(
                format!(
                    "\"{text}\" ({}) is up for {:.1} s; {count} words take about {:.1} s to read.",
                    id(c),
                    secs(c.timeline_duration),
                    secs(need)
                ),
                format!(
                    "trim_clip {{\"clip\": \"{}\", \"end\": {}}} (or fewer words: edit_text)",
                    id(c),
                    secs(c.timeline_start + need)
                ),
            ));
        }
    }
    // Words on top of other words.
    for (i, a) in words.iter().enumerate() {
        for b in &words[i + 1..] {
            let (ta, tb) = (a.transform(), b.transform());
            if overlap(a, b) > SECOND / 2 && (ta.y - tb.y).abs() < 0.05 && (ta.x - tb.x).abs() < 0.1
            {
                out.push(issue(
                    format!(
                        "The texts {} and {} sit on the same spot at the same time ({} s).",
                        id(a),
                        id(b),
                        secs(a.timeline_start.max(b.timeline_start))
                    ),
                    format!(
                        "set_transform {{\"clip\": \"{}\", \"y\": {:.2}}} (or trim_clip one of them)",
                        id(b),
                        (tb.y + 0.5 + if tb.y > 0.2 { -0.14 } else { 0.14 }).clamp(0.08, 0.92)
                    ),
                ));
            }
        }
    }

    // Softer: what would make it better, not what's broken.
    for c in &main {
        if c.kind == "image"
            && c.timeline_duration > LONG_STILL
            && c.keyframe_times(1_000).is_empty()
        {
            tips.push(format!(
                "{} is a photo held still for {:.0} s: animate {{\"clip\": \"{}\", \"motion\": \"zoom_in\"}} gives it life.",
                id(c),
                secs(c.timeline_duration),
                id(c)
            ));
        }
    }
    if audio.is_empty() && picture_end > 10 * SECOND {
        tips.push(
            "There is no music or sound on the audio lanes: add_music {\"mood\": \"…\"} and sound_design {} finish an edit."
                .into(),
        );
    }
    if texts.is_empty() && picture_end > 10 * SECOND {
        tips.push("There are no words on screen: add_title puts a title on the opening.".into());
    }
    (out, tips)
}

fn check_video(_: &Args, env: &mut Env) -> ToolResult<Outcome> {
    let (found, tips) = issues(&env.project, env.aliases);
    let clips: usize = env.project.tracks.iter().map(|t| t.clips.len()).sum();
    let main_end = env
        .project
        .ordered_main_clips()
        .iter()
        .map(|c| c.end())
        .max()
        .unwrap_or(0);
    let mut v = json!({
        "ok": found.is_empty(),
        "duration": secs(main_end),
        "clips": clips,
    });
    if found.is_empty() {
        v["summary"] = "No problems found.".into();
    } else {
        v["issues"] = found
            .iter()
            .take(MAX_ISSUES)
            .map(|i| json!({ "problem": i.problem, "fix": i.fix }))
            .collect::<Vec<Value>>()
            .into();
        if found.len() > MAX_ISSUES {
            v["more"] = (found.len() - MAX_ISSUES).into();
        }
        v["next"] = "Make each fix, then call check_video again.".into();
    }
    if !tips.is_empty() {
        v["tips"] = tips.into_iter().take(4).collect::<Vec<String>>().into();
    }
    Ok(Outcome::Done(v))
}

pub static TOOLS: &[Tool] = &[Tool {
    name: "check_video",
    tier: Tier::Full,
    label: "Check",
    description: "Looks the whole timeline over for what a viewer would notice: black between clips, sound or words running past the end, two voices at once, music drowning the voiceover, captions or texts on top of each other, words gone too fast to read. Each problem comes with the call that fixes it. Changes nothing. Call it after a bigger edit, and before saying a video is finished.",
    params: || json!({ "type": "object", "properties": {} }),
    run: check_video,
}];

#[cfg(test)]
mod tests {
    use super::super::test_env::Fixture;
    use super::*;
    use crate::agent::snapshot::micros;

    fn problems(f: &mut Fixture) -> Vec<String> {
        issues(&f.project, &mut f.aliases)
            .0
            .into_iter()
            .map(|i| format!("{} → {}", i.problem, i.fix))
            .collect()
    }

    #[test]
    fn a_clean_timeline_passes() {
        let mut f = Fixture::with_main_clips(3, 4.);
        assert!(problems(&mut f).is_empty());
        let r = f.call("check_video", json!({})).unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["duration"], 12.);
        // An empty project says where to start.
        let mut empty = Fixture::new(crate::model::Project::empty());
        let r = empty.call("check_video", json!({})).unwrap();
        assert_eq!(r["ok"], false);
        assert!(
            r["issues"][0]["fix"]
                .as_str()
                .unwrap()
                .contains("make_video")
        );
    }

    #[test]
    fn finds_black_between_clips() {
        let mut f = Fixture::with_main_clips(2, 4.);
        let second = f.id("c2");
        f.project.update_clip(&second, |c| {
            c.timeline_start += micros(1.5);
            true
        });
        let found = problems(&mut f);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("1.5 s of black between c1 and c2"),
            "{found:?}"
        );
        assert!(
            found[0].contains("move_clip {\"clip\": \"c2\", \"start\": 4}"),
            "{found:?}"
        );
    }

    #[test]
    fn finds_words_too_quick_and_past_the_end() {
        let mut f = Fixture::with_main_clips(1, 4.);
        f.call(
            "add_text",
            json!({ "text": "Seven whole words that nobody can read", "start": 0, "duration": 0.8 }),
        )
        .unwrap();
        f.call(
            "add_text",
            json!({ "text": "Late", "start": 3.5, "duration": 2, "y": 0.8 }),
        )
        .unwrap();
        let found = problems(&mut f).join("\n");
        assert!(
            found.contains("7 words take about 2.6 s to read"),
            "{found}"
        );
        assert!(
            found.contains("runs 1.5 s past the end of the video (4 s)"),
            "{found}"
        );
        assert!(found.contains("\"end\": 4}"), "{found}");
        // The fix it names works.
        let late = f
            .project
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .find(|c| c.text.as_deref() == Some("Late"))
            .map(|c| c.id.clone())
            .unwrap();
        let alias = f.aliases.alias(Kind::Clip, &late);
        f.call("trim_clip", json!({ "clip": alias, "end": 4 }))
            .unwrap();
        assert!(!problems(&mut f).join("\n").contains("past the end"));
    }

    #[test]
    fn an_animated_title_is_not_words_too_quick() {
        let mut f = Fixture::with_main_clips(1, 6.);
        for style in ["word_by_word", "typewriter", "pop", "lower_third"] {
            f.call(
                "add_title",
                json!({ "text": "Big news for everyone today", "subtitle": "Right now", "style": style, "start": 0, "duration": 3 }),
            )
            .unwrap();
            let found = problems(&mut f);
            assert!(found.is_empty(), "{style}: {found:?}");
            let ids: Vec<String> = f
                .project
                .tracks
                .iter()
                .flat_map(|t| t.clips.iter())
                .filter(|c| c.kind == "text")
                .map(|c| c.id.clone())
                .collect();
            for id in ids {
                f.project.delete_clip(&id);
            }
        }
    }

    #[test]
    fn finds_texts_on_the_same_spot() {
        let mut f = Fixture::with_main_clips(1, 6.);
        for text in ["One", "Two"] {
            f.call(
                "add_text",
                json!({ "text": text, "start": 1, "duration": 3 }),
            )
            .unwrap();
        }
        let found = problems(&mut f).join("\n");
        assert!(found.contains("sit on the same spot"), "{found}");
        assert!(found.contains("set_transform"), "{found}");
    }
}
