//! Captions on the document: text clips with `caption` data, on the text
//! lane (the timeline gives them their own Captions lane). They come from
//! a subtitle file (.srt / .vtt / .ass, kept at the file's times) or from
//! the speech in the video (`speech.rs`), in the picked caption style
//! (`caption_styles.rs`), and replace the captions they overlap.

use serde_json::{Value, json};

use crate::caption_styles::CaptionStyle;
use crate::model::{Clip, Micros, Project, SECOND, TrackType};

/// One timed caption.
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub start: Micros,
    pub end: Micros,
    pub text: String,
}

/// A cue shorter than this on the timeline is dropped.
pub const MIN_CUE: Micros = 200_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubtitleFormat {
    Srt,
    Vtt,
    Ass,
}

/// A file's format from its name, else from its first lines.
pub fn subtitle_format(name: &str, text: &str) -> Option<SubtitleFormat> {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("srt") => return Some(SubtitleFormat::Srt),
        Some("vtt") => return Some(SubtitleFormat::Vtt),
        Some("ass") | Some("ssa") => return Some(SubtitleFormat::Ass),
        _ => {}
    }
    let head = text.trim_start_matches('\u{feff}').trim_start();
    if head.starts_with("WEBVTT") {
        Some(SubtitleFormat::Vtt)
    } else if head.starts_with("[Script Info]") {
        Some(SubtitleFormat::Ass)
    } else if text.contains("-->") {
        Some(SubtitleFormat::Srt)
    } else {
        None
    }
}

/// "01:02:03,456", "02:03.456" or ASS's "1:02:03.45" → micros.
fn clock(s: &str) -> Option<Micros> {
    let s = s.trim().replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?, *s),
        [m, s] => (0, m.parse::<i64>().ok()?, *s),
        _ => return None,
    };
    let secs: f64 = sec.parse().ok()?;
    Some(((h * 3600 + m * 60) * SECOND) + (secs * SECOND as f64).round() as Micros)
}

/// Markup out: `<i>…</i>`, `{\an8}`, ASS's `\N` line breaks.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = (0, 0); // inside <…>, {…}
    for c in text.replace("\\N", "\n").replace("\\n", "\n").chars() {
        match c {
            '<' => depth.0 += 1,
            '>' if depth.0 > 0 => depth.0 -= 1,
            '{' => depth.1 += 1,
            '}' if depth.1 > 0 => depth.1 -= 1,
            _ if depth == (0, 0) => out.push(c),
            _ => {}
        }
    }
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The cues of a subtitle file, in time order, markup removed.
pub fn parse_subtitles(text: &str, format: SubtitleFormat) -> Vec<Cue> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut cues = match format {
        SubtitleFormat::Srt | SubtitleFormat::Vtt => blocks(&text),
        SubtitleFormat::Ass => ass(&text),
    };
    cues.retain(|c| c.end > c.start && !c.text.is_empty());
    cues.sort_by_key(|c| c.start);
    cues
}

/// SRT / WebVTT: blank-line separated blocks, each with a `-->` line
/// (VTT's settings after the end time, NOTE / STYLE blocks skipped).
fn blocks(text: &str) -> Vec<Cue> {
    let mut out = vec![];
    for block in text.split("\n\n") {
        let mut lines = block.lines().skip_while(|l| !l.contains("-->"));
        let Some(times) = lines.next() else { continue };
        let Some((a, b)) = times.split_once("-->") else {
            continue;
        };
        let b = b.split_whitespace().next().unwrap_or("");
        let (Some(start), Some(end)) = (clock(a), clock(b)) else {
            continue;
        };
        let body: Vec<&str> = lines.collect();
        out.push(Cue {
            start,
            end,
            text: plain(&body.join("\n")),
        });
    }
    out
}

/// ASS / SSA: the `Dialogue:` lines, by their `[Events]` Format.
fn ass(text: &str) -> Vec<Cue> {
    let mut fields: Vec<String> = [
        "layer", "start", "end", "style", "name", "marginl", "marginr", "marginv", "effect", "text",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mut out = vec![];
    for line in text.lines() {
        if let Some(f) = line.strip_prefix("Format:") {
            fields = f
                .split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .collect();
            continue;
        }
        let Some(rest) = line.strip_prefix("Dialogue:") else {
            continue;
        };
        // The text is the last field and may hold commas.
        let parts: Vec<&str> = rest.splitn(fields.len(), ',').collect();
        let get = |name: &str| {
            fields
                .iter()
                .position(|f| f == name)
                .and_then(|i| parts.get(i))
        };
        let (Some(a), Some(b), Some(t)) = (get("start"), get("end"), get("text")) else {
            continue;
        };
        if let (Some(start), Some(end)) = (clock(a), clock(b)) {
            out.push(Cue {
                start,
                end,
                text: plain(t),
            });
        }
    }
    out
}

/// Languages written without spaces between words.
pub fn unspaced(language: &str) -> bool {
    matches!(language, "zh" | "ja" | "th" | "km" | "lo" | "my")
}

/// How a language joins the parts of one caption.
pub fn joiner(language: &str) -> &'static str {
    if unspaced(language) { "" } else { " " }
}

const KHMER_COENG: char = '\u{17d2}';

/// Khmer dependent vowels and signs (and ZWNJ / ZWJ): they belong to the
/// character before them.
fn khmer_mark(c: char) -> bool {
    matches!(
        c,
        '\u{17b6}'..='\u{17d1}' | '\u{17d3}' | '\u{17dd}' | '\u{200c}' | '\u{200d}'
    )
}

fn has_khmer(text: &str) -> bool {
    text.chars().any(|c| ('\u{1780}'..='\u{17ff}').contains(&c))
}

/// Khmer orthographic syllables: a base character with its subscripts
/// (COENG + consonant) and vowel signs. Text is only ever cut between
/// them: a cut inside one strands a vowel sign on a dotted circle.
pub fn khmer_clusters(text: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut start = 0;
    let mut after_coeng = false;
    for (i, c) in text.char_indices() {
        let joins = i > start && (after_coeng || c == KHMER_COENG || khmer_mark(c));
        if !joins && i > start {
            out.push(&text[start..i]);
            start = i;
        }
        after_coeng = c == KHMER_COENG;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// whisper.cpp cuts segments between tokens, and a Khmer syllable can
/// span two: one segment then ends in a COENG or the next starts with a
/// vowel sign. Such neighbours are joined back into one.
pub fn join_split_syllables(cues: Vec<Cue>) -> Vec<Cue> {
    let mut out: Vec<Cue> = vec![];
    for c in cues {
        if let Some(prev) = out.last_mut()
            && (prev.text.ends_with(KHMER_COENG) || c.text.chars().next().is_some_and(khmer_mark))
        {
            prev.end = prev.end.max(c.end);
            prev.text.push_str(&c.text);
            continue;
        }
        out.push(c);
    }
    out
}

/// The units a caption line breaks between: words for spaced languages;
/// for Khmer the space-separated phrases (each keeps its space), chunked
/// between syllables when longer than a line; characters otherwise.
fn tokens(text: &str, language: &str, max: usize) -> Vec<String> {
    if !unspaced(language) {
        return text.split_whitespace().map(str::to_string).collect();
    }
    if !has_khmer(text) {
        return text
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(String::from)
            .collect();
    }
    let mut out = vec![];
    for word in text.split_inclusive(char::is_whitespace) {
        if word.trim().is_empty() {
            continue;
        }
        let mut chunk = String::new();
        for cluster in khmer_clusters(word) {
            if !chunk.is_empty() && chunk.chars().count() + cluster.chars().count() > max {
                out.push(std::mem::take(&mut chunk));
            }
            chunk.push_str(cluster);
        }
        if !chunk.is_empty() {
            out.push(chunk);
        }
    }
    out
}

fn ends_sentence(token: &str) -> bool {
    token.trim_end().ends_with([
        '.', ',', '!', '?', ';', ':', '…', '，', '。', '！', '？', '、', '។', '៕',
    ])
}

/// One transcribed segment as line-sized cues: tokens grouped up to
/// `max` characters (early after punctuation once a line is fairly
/// full), each line timed by its share of the characters.
pub fn lines(text: &str, start: Micros, end: Micros, language: &str, max: usize) -> Vec<Cue> {
    let join = joiner(language);
    let max = max.max(1);
    let mut groups: Vec<String> = vec![];
    let mut current = String::new();
    for token in tokens(text, language, max) {
        let sep = if current.is_empty() { "" } else { join };
        let len = current.chars().count();
        if len > 0 && len + sep.len() + token.chars().count() > max {
            groups.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push_str(join);
        }
        current.push_str(&token);
        if ends_sentence(&token) && current.chars().count() * 10 >= max * 4 {
            groups.push(std::mem::take(&mut current));
        }
    }
    groups.push(current);
    let texts: Vec<String> = groups
        .iter()
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty())
        .collect();
    let total: usize = texts.iter().map(|t| t.chars().count()).sum();
    let span = (end - start) as f64;
    let mut done = 0;
    texts
        .into_iter()
        .map(|text| {
            let a = start + (span * done as f64 / total as f64).round() as Micros;
            done += text.chars().count();
            let b = start + (span * done as f64 / total as f64).round() as Micros;
            Cue {
                start: a,
                end: b,
                text,
            }
        })
        .collect()
}

/// `text` with any run of one short unit repeated over and over (a speech
/// model stuck in a loop: "ng ng ng ng…") cut to one copy.
pub fn collapse_repeats(text: &str) -> String {
    let c: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < c.len() {
        // The period covering the most characters from here.
        let mut best = (0, 0); // (period, repeats)
        for p in 1..=12.min(c.len() - i) {
            let unit = &c[i..i + p];
            if !unit.iter().any(|ch| ch.is_alphabetic()) {
                continue;
            }
            let mut r = 1;
            while i + (r + 1) * p <= c.len() && c[i + r * p..i + (r + 1) * p] == *unit {
                r += 1;
            }
            let needed = if p == 1 { 6 } else { 4 };
            if r >= needed && r * p > best.0 * best.1 {
                best = (p, r);
            }
        }
        if best.0 > 0 {
            out.extend(&c[i..i + best.0]);
            i += best.0 * best.1;
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

/// Cleaned speech: repeats collapsed; None when that leaves under half of
/// it (the model looped rather than heard anything).
pub fn clean_speech(text: &str) -> Option<String> {
    let text = text.trim();
    let kept = collapse_repeats(text);
    let (a, b) = (text.chars().count(), kept.trim().chars().count());
    (b > 0 && (a < 12 || b * 2 >= a)).then(|| kept.trim().to_string())
}

/// `index` (chars) moved to the nearest Khmer syllable edge; unchanged
/// in other scripts.
pub fn syllable_edge(text: &str, index: usize) -> usize {
    if !has_khmer(text) {
        return index;
    }
    let (mut edge, mut best) = (0usize, 0usize);
    for cluster in khmer_clusters(text) {
        if edge.abs_diff(index) < best.abs_diff(index) {
            best = edge;
        }
        edge += cluster.chars().count();
    }
    if edge.abs_diff(index) < best.abs_diff(index) {
        best = edge;
    }
    best
}

/// Neither half of a split caption may be shorter than this.
pub const MIN_SPLIT: Micros = 300_000;

/// `text` cut at char `index`: the trimmed text before and after, None
/// when either side is left empty.
pub fn cut_text(text: &str, index: usize) -> Option<(String, String)> {
    let byte = text
        .char_indices()
        .nth(index)
        .map_or(text.len(), |(b, _)| b);
    let (a, b) = (text[..byte].trim(), text[byte..].trim());
    (!a.is_empty() && !b.is_empty()).then(|| (a.to_string(), b.to_string()))
}

/// A caption's text for a split `share` (0–1) of the way through its
/// time: cut at the word (or Khmer syllable) nearest the same share of
/// its characters.
pub fn split_text_at_share(text: &str, share: f64) -> Option<(String, String)> {
    let n = text.chars().count();
    cut_text(
        text,
        nearest_break(text, (n as f64 * share).round() as usize),
    )
}

/// The char index nearest `at` where `text` may be cut: a space, else a
/// Khmer syllable edge, else anywhere (Chinese). 0 / len when none.
pub fn nearest_break(text: &str, at: usize) -> usize {
    let c: Vec<char> = text.chars().collect();
    let at = at.min(c.len());
    for d in 0..=c.len() {
        if at > d && c[at - d - 1].is_whitespace() {
            return at - d;
        }
        if at + d < c.len() && c[at + d].is_whitespace() {
            return at + d + 1;
        }
    }
    if has_khmer(text) {
        let (mut edge, mut best) = (0usize, 0usize);
        for cluster in khmer_clusters(text) {
            if edge.abs_diff(at) < best.abs_diff(at) {
                best = edge;
            }
            edge += cluster.chars().count();
        }
        if edge.abs_diff(at) < best.abs_diff(at) {
            best = edge;
        }
        return best;
    }
    at
}

/// Where captions sit: low in the frame, a little higher in portrait.
fn caption_y(project: &Project) -> f64 {
    let portrait = project.canvas.height > project.canvas.width;
    if portrait { 0.26 } else { 0.35 }
}

/// The end of the video (every lane but text): captions stop there.
fn video_end(project: &Project) -> Micros {
    project
        .tracks
        .iter()
        .filter(|t| t.kind != TrackType::Text)
        .flat_map(|t| t.clips.iter())
        .map(Clip::end)
        .max()
        .unwrap_or(0)
}

/// Puts `cues` on the timeline as captions in `style`, replacing the
/// captions they overlap; cues past the end of the video are cut at it or
/// dropped. Returns how many landed.
pub fn place(project: &mut Project, cues: &[Cue], style: &CaptionStyle, language: &str) -> usize {
    let end = video_end(project);
    let cues: Vec<Cue> = cues
        .iter()
        .filter_map(|c| {
            let until = if end > 0 { c.end.min(end) } else { c.end };
            (until - c.start >= MIN_CUE).then(|| Cue {
                end: until,
                ..c.clone()
            })
        })
        .collect();
    if cues.is_empty() {
        return 0;
    }
    // Captions under the new ones go.
    let old: Vec<String> = project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.is_caption())
        .filter(|c| {
            cues.iter()
                .any(|q| c.timeline_start < q.end && q.start < c.end())
        })
        .map(|c| c.id.clone())
        .collect();
    for id in &old {
        project.delete_clip(id);
    }
    let y = caption_y(project);
    for cue in &cues {
        let text = cue
            .text
            .split('\n')
            .collect::<Vec<_>>()
            .join(joiner(language));
        let id = project.add_text_clip(cue.start, text.trim());
        project.update_clip(&id, |c| {
            c.timeline_duration = cue.end - cue.start;
            c.extra.insert("fontSize".into(), json!(22));
            c.extra
                .insert("caption".into(), json!({ "language": language }));
            if let Some(Value::Object(t)) = c.extra.get_mut("transform") {
                t.insert("y".into(), json!(y));
            }
            style.apply(&mut c.extra);
            true
        });
    }
    project.recompute_duration();
    cues.len()
}

/// Every caption, in time order.
pub fn all(project: &Project) -> Vec<&Clip> {
    let mut out: Vec<&Clip> = project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.is_caption())
        .collect();
    out.sort_by_key(|c| c.timeline_start);
    out
}

fn language_of(clip: &Clip) -> &str {
    clip.extra
        .get("caption")
        .and_then(|c| c.get("language"))
        .and_then(Value::as_str)
        .unwrap_or("en")
}

/// `first` and `second` (in time order) as one caption: the first's look,
/// spanning both, the words joined the way the language joins them.
/// Returns the kept caption's id.
pub fn merge(project: &mut Project, first: &str, second: &str) -> Option<String> {
    let (_, b) = project.find_clip(second).map(|(t, c)| (t, c.clone()))?;
    let (_, a) = project.find_clip(first).map(|(t, c)| (t, c.clone()))?;
    let join = joiner(language_of(&a));
    let text = [a.text.as_deref(), b.text.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(join);
    let end = a.end().max(b.end());
    project.delete_clip(second)?;
    project.update_clip(first, |c| {
        c.text = Some(text);
        c.timeline_duration = end - c.timeline_start;
        true
    });
    Some(a.id)
}

/// Splits caption `id` at char `index` of its words, its time cut by the
/// words' share. Returns the new (right) caption's id; None when a side
/// would have no words or be too short to read.
pub fn split_at_char(project: &mut Project, id: &str, index: usize) -> Option<String> {
    let clip = project.find_clip(id).map(|(_, c)| c.clone())?;
    let text = clip.text.as_deref().unwrap_or("");
    // A caret can sit inside a Khmer syllable (before its vowel sign):
    // the cut moves to the syllable's nearer edge.
    let (a, b) = cut_text(text, syllable_edge(text, index))?;
    let (na, nb) = (a.chars().count(), b.chars().count());
    let offset = (clip.timeline_duration as f64 * na as f64 / (na + nb) as f64).round() as Micros;
    if offset < MIN_SPLIT || clip.timeline_duration - offset < MIN_SPLIT {
        return None;
    }
    // The time is cut by the split itself (which cuts the text by share),
    // then each half gets exactly its side of the cursor.
    if !project.split_at(id, clip.timeline_start + offset) {
        return None;
    }
    let right = project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .find(|c| c.id != id && c.is_caption() && c.timeline_start == clip.timeline_start + offset)
        .map(|c| c.id.clone())?;
    project.update_clip(id, |c| {
        c.text = Some(a.clone());
        true
    });
    project.update_clip(&right, |c| {
        c.text = Some(b.clone());
        true
    });
    Some(right)
}

/// Case-insensitive, literal matches of `find` in `text`, as byte ranges.
fn matches(text: &str, find: &str) -> Vec<std::ops::Range<usize>> {
    if find.is_empty() {
        return vec![];
    }
    let (hay, needle) = (text.to_lowercase(), find.to_lowercase());
    // Lowercasing can change byte lengths; only match where it didn't.
    if hay.len() != text.len() {
        return text
            .match_indices(find)
            .map(|(i, m)| i..i + m.len())
            .collect();
    }
    hay.match_indices(&needle)
        .map(|(i, m)| i..i + m.len())
        .collect()
}

/// How many times `find` occurs across the captions.
pub fn count_matches(project: &Project, find: &str) -> usize {
    all(project)
        .iter()
        .map(|c| matches(c.text.as_deref().unwrap_or(""), find).len())
        .sum()
}

/// Replaces every `find` (any case) in the captions with `replace`;
/// returns how many captions changed.
pub fn replace_all(project: &mut Project, find: &str, replace: &str) -> usize {
    let ids: Vec<String> = all(project).iter().map(|c| c.id.clone()).collect();
    let mut changed = 0;
    for id in ids {
        project.update_clip(&id, |c| {
            let text = c.text.clone().unwrap_or_default();
            let found = matches(&text, find);
            if found.is_empty() {
                return false;
            }
            let mut out = String::with_capacity(text.len());
            let mut at = 0;
            for r in found {
                out.push_str(&text[at..r.start]);
                out.push_str(replace);
                at = r.end;
            }
            out.push_str(&text[at..]);
            c.text = Some(out);
            changed += 1;
            true
        });
    }
    changed
}

/// Puts `style` on every caption; false when there are none.
pub fn restyle(project: &mut Project, style: &CaptionStyle) -> bool {
    let ids: Vec<String> = project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.is_caption())
        .map(|c| c.id.clone())
        .collect();
    for id in &ids {
        project.update_clip(id, |c| {
            style.apply(&mut c.extra);
            true
        });
    }
    !ids.is_empty()
}

/// A clip whose sound can be captioned: a video or audio clip of a file,
/// not muted, frozen or reversed, on a lane that isn't muted.
fn captionable(project: &Project, clip: &Clip) -> bool {
    matches!(clip.kind.as_str(), "video" | "audio")
        && clip.source.is_some()
        && !clip.muted()
        && !clip.freeze()
        && !clip.reversed()
        && clip.volume() > 0.
        && project.track_of(&clip.id).is_some_and(|t| !t.muted)
}

/// What Generate covers: the selected clips with sound, else every video
/// clip (true when it's the selection).
pub fn targets(project: &Project, selected: &[String]) -> (Vec<Clip>, bool) {
    let picked: Vec<Clip> = selected
        .iter()
        .filter_map(|id| project.find_clip(id).map(|(_, c)| c))
        .filter(|c| captionable(project, c))
        .cloned()
        .collect();
    if !picked.is_empty() {
        return (picked, true);
    }
    let mut all: Vec<Clip> = project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.kind == "video" && captionable(project, c))
        .cloned()
        .collect();
    all.sort_by_key(|c| c.timeline_start);
    (all, false)
}

/// Any audio clip that could be captioned once selected.
pub fn has_audio_to_pick(project: &Project) -> bool {
    project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .any(|c| c.kind == "audio" && captionable(project, c))
}

/// A clip's source span for its timeline span: (source start, source
/// end, speed).
pub fn source_span(clip: &Clip) -> (Micros, Micros, f64) {
    let speed = if clip.speed > 0. { clip.speed } else { 1. };
    let from = clip.source_start.unwrap_or(0);
    (
        from,
        from + (clip.timeline_duration as f64 * speed) as Micros,
        speed,
    )
}

/// Speech found in a clip's sound (times from its source start) onto the
/// timeline through its trim and speed; cues it hides are dropped.
pub fn to_timeline(clip: &Clip, speech: &[Cue]) -> Vec<Cue> {
    let (_, _, speed) = source_span(clip);
    let map = |t: Micros| clip.timeline_start + (t as f64 / speed) as Micros;
    speech
        .iter()
        .filter_map(|c| {
            let start = map(c.start).max(clip.timeline_start);
            let end = map(c.end).min(clip.end());
            (end - start >= MIN_CUE).then(|| Cue {
                start,
                end,
                text: c.text.trim().to_string(),
            })
        })
        .filter(|c| !c.text.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MediaItem;

    const SRT: &str = "1\n00:00:01,000 --> 00:00:02,500\n<i>Hello</i> there\n\n2\n00:00:03,000 --> 00:00:04,000\nSecond\nline\n";
    const VTT: &str = "WEBVTT\n\nNOTE skip me\n\n00:01.000 --> 00:02.000 align:start\n<v Ann>Hi</v>\n\nid-2\n00:00:03.000 --> 00:00:04.250\nBye\n";
    const ASS: &str = "[Script Info]\nTitle: x\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:02.50,Default,,0,0,0,,{\\an8}Hello, world\\Nagain\n";

    #[test]
    fn reads_subtitle_files() {
        assert_eq!(subtitle_format("a.SRT", ""), Some(SubtitleFormat::Srt));
        assert_eq!(subtitle_format("noext", VTT), Some(SubtitleFormat::Vtt));
        assert_eq!(subtitle_format("noext", ASS), Some(SubtitleFormat::Ass));
        assert_eq!(subtitle_format("noext", "hello"), None);
        let srt = parse_subtitles(SRT, SubtitleFormat::Srt);
        assert_eq!(
            srt,
            [
                Cue {
                    start: SECOND,
                    end: 2_500_000,
                    text: "Hello there".into()
                },
                Cue {
                    start: 3 * SECOND,
                    end: 4 * SECOND,
                    text: "Second\nline".into()
                },
            ]
        );
        let vtt = parse_subtitles(VTT, SubtitleFormat::Vtt);
        assert_eq!(vtt.len(), 2);
        assert_eq!((vtt[0].text.as_str(), vtt[1].end), ("Hi", 4_250_000));
        let a = parse_subtitles(ASS, SubtitleFormat::Ass);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].text, "Hello, world\nagain");
        assert_eq!(a[0].end, 2_500_000);
    }

    #[test]
    fn captions_replace_what_they_overlap_and_stop_at_the_end() {
        let mut p = Project::empty();
        p.insert_media(
            &MediaItem {
                path: "/tmp/a.mp4".into(),
                name: "a.mp4".into(),
                duration: 5 * SECOND,
                width: 1920,
                height: 1080,
                has_video: true,
                still: false,
            },
            None,
        );
        let style = crate::caption_styles::get("classic");
        let cues = parse_subtitles(SRT, SubtitleFormat::Srt);
        assert_eq!(place(&mut p, &cues, style, "en"), 2);
        let captions = |p: &Project| -> Vec<Clip> {
            p.tracks
                .iter()
                .flat_map(|t| t.clips.iter())
                .filter(|c| c.is_caption())
                .cloned()
                .collect()
        };
        let got = captions(&p);
        assert_eq!(got.len(), 2);
        assert!(got.iter().any(|c| c.text.as_deref() == Some("Second line")));
        // Again over the first one, and one past the end (cut, then gone).
        let more = [
            Cue {
                start: 1_500_000,
                end: 2 * SECOND,
                text: "Again".into(),
            },
            Cue {
                start: 4_500_000,
                end: 9 * SECOND,
                text: "Late".into(),
            },
            Cue {
                start: 6 * SECOND,
                end: 7 * SECOND,
                text: "Gone".into(),
            },
        ];
        assert_eq!(place(&mut p, &more, style, "en"), 2);
        let got = captions(&p);
        let texts: Vec<_> = got.iter().filter_map(|c| c.text.clone()).collect();
        assert!(!texts.contains(&"Hello there".to_string()));
        assert!(texts.contains(&"Again".to_string()));
        let late = got
            .iter()
            .find(|c| c.text.as_deref() == Some("Late"))
            .unwrap();
        assert_eq!(late.end(), 5 * SECOND);
        // A new style on all of them.
        assert!(restyle(&mut p, crate::caption_styles::get("box-black")));
        assert!(
            captions(&p)
                .iter()
                .all(|c| c.extra.contains_key("backgroundColor"))
        );
    }

    #[test]
    fn khmer_is_cut_between_syllables_only() {
        // ក + ្ + ម (a subscript) is one syllable; ា and ិ are vowel signs.
        assert_eq!(khmer_clusters("ក្មានិង"), ["ក្មា", "និ", "ង"]);
        let cues = lines("សួស្តីបងប្អូនទាំងអស់គ្នា", 0, 2 * SECOND, "km", 8);
        assert!(cues.len() > 1);
        for c in &cues {
            assert!(
                !c.text.starts_with(|ch| khmer_mark(ch) || ch == KHMER_COENG),
                "{}",
                c.text
            );
        }
        assert_eq!(
            cues.iter().map(|c| c.text.as_str()).collect::<String>(),
            "សួស្តីបងប្អូនទាំងអស់គ្នា"
        );
        assert_eq!((cues[0].start, cues.last().unwrap().end), (0, 2 * SECOND));
        // A syllable cut between two segments is put back together.
        let parts = vec![
            Cue {
                start: 0,
                end: SECOND,
                text: "ស្".into(),
            },
            Cue {
                start: SECOND,
                end: 2 * SECOND,
                text: "តី".into(),
            },
        ];
        assert_eq!(join_split_syllables(parts)[0].text, "ស្តី");
        // A caret before a vowel sign cuts before its whole syllable.
        assert_eq!(syllable_edge("ហ្នឹងអានឹង", 6), 5);
        assert_eq!(syllable_edge("hello", 2), 2);
        // English: whole words.
        let en = lines("one two three four", 0, SECOND, "en", 9);
        assert_eq!(
            en.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
            ["one two", "three", "four"]
        );
    }

    #[test]
    fn a_looping_model_is_caught() {
        assert_eq!(clean_speech("ng ng ng ng ng ng ng ng ng ng"), None);
        assert_eq!(clean_speech(" Thu Thu Thu Thu Thu Thu Thu"), None);
        assert_eq!(clean_speech("ទិញស្តស្តស្តស្តស្ត្រហ៊ុន").as_deref(), Some("ទិញស្ត្រហ៊ុន"));
        assert_eq!(
            clean_speech("It cost 1000000 dollars").as_deref(),
            Some("It cost 1000000 dollars")
        );
        assert_eq!(clean_speech("Hello there").as_deref(), Some("Hello there"));
    }

    #[test]
    fn captions_split_merge_and_replace() {
        let mut p = Project::empty();
        p.insert_media(
            &MediaItem {
                path: "/tmp/a.mp4".into(),
                name: "a.mp4".into(),
                duration: 10 * SECOND,
                width: 1920,
                height: 1080,
                has_video: true,
                still: false,
            },
            None,
        );
        let style = crate::caption_styles::get("classic");
        let cue = |s, e, t: &str| Cue {
            start: s,
            end: e,
            text: t.into(),
        };
        place(
            &mut p,
            &[
                cue(0, 4 * SECOND, "one two three four"),
                cue(4 * SECOND, 6 * SECOND, "five"),
            ],
            style,
            "en",
        );
        let ids: Vec<String> = all(&p).iter().map(|c| c.id.clone()).collect();
        // Split at the playhead: each half keeps its own words.
        assert!(p.split_at(&ids[0], 2 * SECOND));
        let texts = |p: &Project| {
            all(p)
                .iter()
                .map(|c| c.text.clone().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(texts(&p), ["one two", "three four", "five"]);
        // Too close to an end: no split.
        assert!(!p.split_at(&ids[1], 4 * SECOND + 100_000));
        // Merge the halves back.
        let now: Vec<String> = all(&p).iter().map(|c| c.id.clone()).collect();
        assert_eq!(
            merge(&mut p, &now[0], &now[1]).as_deref(),
            Some(now[0].as_str())
        );
        assert_eq!(texts(&p), ["one two three four", "five"]);
        assert_eq!(all(&p)[0].end(), 4 * SECOND);
        // Split at a cursor: time by the words' share.
        let right = split_at_char(&mut p, &now[0], 8).unwrap();
        assert_eq!(texts(&p), ["one two", "three four", "five"]);
        assert_eq!(p.find_clip(&right).unwrap().1.timeline_start, 1_647_059);
        assert_eq!(split_at_char(&mut p, &right, 0), None);
        // Find & replace, any case.
        assert_eq!(count_matches(&p, "O"), 3);
        assert_eq!(replace_all(&mut p, "o", "0"), 2);
        assert_eq!(texts(&p), ["0ne tw0", "three f0ur", "five"]);
    }

    #[test]
    fn speech_follows_trim_and_speed() {
        let mut clip = Clip {
            id: "c".into(),
            kind: "video".into(),
            timeline_start: 10 * SECOND,
            timeline_duration: 4 * SECOND,
            speed: 2.,
            source: Some("/tmp/a.mp4".into()),
            source_start: Some(SECOND),
            source_duration: None,
            text: None,
            hidden: false,
            extra: Default::default(),
        };
        assert_eq!(source_span(&clip), (SECOND, 9 * SECOND, 2.));
        let speech = [
            Cue {
                start: 2 * SECOND,
                end: 4 * SECOND,
                text: " Hi ".into(),
            },
            Cue {
                start: 20 * SECOND,
                end: 21 * SECOND,
                text: "after".into(),
            },
        ];
        let got = to_timeline(&clip, &speech);
        assert_eq!(
            got,
            [Cue {
                start: 11 * SECOND,
                end: 12 * SECOND,
                text: "Hi".into()
            }]
        );
        clip.speed = 1.;
        assert_eq!(to_timeline(&clip, &speech)[0].start, 12 * SECOND);
    }
}
