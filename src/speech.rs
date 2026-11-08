//! Speech to text for auto captions, on this computer: whisper.cpp's
//! command-line tool (`whisper-cli`, e.g. from Homebrew's `whisper-cpp`)
//! with a multilingual model kept in `<data>/models/` (downloaded the first
//! time). Nothing is uploaded.
//!
//! A job runs on its own thread: per clip, its sound (the part the timeline
//! shows) decoded to 16 kHz mono, written as a WAV, transcribed into
//! line-sized segments, mapped back onto the timeline through the clip's
//! trim and speed. It reports its phase and progress, and stops when
//! cancelled.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::Value;

use crate::captions::{self, Cue};
use crate::model::{Clip, Micros, SECOND};

const RATE: u32 = 16_000;

/// A whisper.cpp model: its file in `<data>/models/` and where it's from.
struct Model {
    file: &'static str,
    url: &'static str,
}

/// The stock multilingual model.
const STOCK: Model = Model {
    file: "ggml-base.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
};

/// Khmer: the stock models can't write it (they loop on one syllable), so
/// it gets sengtha/whisper-base-khmer, Whisper Base fine-tuned on Khmer
/// (CER 22.8 % on FLEURS km_kh), as its author's q5_1 file (60 MB), pinned
/// to one commit. CC BY-SA 4.0: credit "sengtha / iAny" where it shows.
const KHMER: Model = Model {
    file: "ggml-base-khmer-q5_1.bin",
    url: "https://huggingface.co/sengtha/whisper-base-khmer/resolve/d7da9b014bec98f34074c627ee4a75421460148c/ggml-base-khmer-q5_1.bin",
};

fn model_for(language: &str) -> &'static Model {
    if language == "km" { &KHMER } else { &STOCK }
}

/// The spoken languages offered (code, name in its own language).
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("km", "ខ្មែរ"),
    ("es", "Español"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("pt", "Português"),
    ("it", "Italiano"),
    ("nl", "Nederlands"),
    ("ru", "Русский"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("zh", "中文"),
    ("vi", "Tiếng Việt"),
    ("id", "Bahasa Indonesia"),
    ("th", "ไทย"),
    ("hi", "हिन्दी"),
    ("ar", "العربية"),
    ("tr", "Türkçe"),
];

pub fn language_name(code: &str) -> &'static str {
    LANGUAGES
        .iter()
        .find(|(c, _)| *c == code)
        .map_or("English", |(_, n)| n)
}

/// whisper.cpp's tool: `REEL_WHISPER`, else `whisper-cli` on the PATH or
/// where Homebrew puts it.
pub fn whisper() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("REEL_WHISPER").map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    let exe = if cfg!(windows) {
        "whisper-cli.exe"
    } else {
        "whisper-cli"
    };
    dirs.into_iter().map(|d| d.join(exe)).find(|p| p.is_file())
}

fn model_path(model: &Model) -> PathBuf {
    crate::prefs::data_dir().join("models").join(model.file)
}

/// Where a job is.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Checking,
    Downloading,
    Transcribing,
    Applying,
}

impl Phase {
    /// The line shown while it runs.
    pub fn label(&self) -> &'static str {
        match self {
            Phase::Checking => "Getting ready…",
            Phase::Downloading => "Downloading the speech model…",
            Phase::Transcribing => "Writing captions…",
            Phase::Applying => "Adding captions…",
        }
    }
}

pub enum Update {
    /// A phase and its progress (0–1, None when it can't be measured).
    Progress(Phase, Option<f32>),
    /// Every clip's cues, on the timeline.
    Done(Vec<Cue>),
    Failed(String),
}

/// What to caption: the clips (a snapshot) and the spoken language.
pub struct Job {
    pub clips: Vec<Clip>,
    pub language: String,
    /// Characters a segment holds at most (longer for two-line styles).
    pub max_chars: usize,
}

/// Runs `job` (blocking: on its own thread), reporting through `send`.
pub fn run(job: Job, cancel: Arc<AtomicBool>, send: impl Fn(Update)) {
    match transcribe_all(&job, &cancel, &send) {
        Ok(cues) if !cancel.load(Ordering::Relaxed) => send(Update::Done(cues)),
        Ok(_) => {}
        Err(e) if !cancel.load(Ordering::Relaxed) => {
            eprintln!("captions: {e:#}");
            send(Update::Failed(format!("{e:#}")))
        }
        Err(_) => {}
    }
}

fn transcribe_all(job: &Job, cancel: &AtomicBool, send: &impl Fn(Update)) -> Result<Vec<Cue>> {
    send(Update::Progress(Phase::Checking, None));
    let tool = whisper().ok_or_else(|| {
        anyhow!(
            "Auto captions need whisper.cpp on this computer (Homebrew: brew install whisper-cpp)"
        )
    })?;
    let spec = model_for(&job.language);
    let model = model_path(spec);
    if !model.is_file() {
        download_model(spec.url, &model, cancel, send)?;
    }
    let dir = crate::prefs::data_dir().join("cache").join("captions");
    std::fs::create_dir_all(&dir)?;
    let mut out = vec![];
    let n = job.clips.len().max(1) as f32;
    for (i, clip) in job.clips.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(vec![]);
        }
        let report = |p: f32| {
            send(Update::Progress(
                Phase::Transcribing,
                Some(((i as f32 + p.clamp(0., 1.)) / n).min(1.)),
            ))
        };
        report(0.);
        let Some(source) = clip.source.as_deref() else {
            continue;
        };
        let (from, to, _) = captions::source_span(clip);
        let samples = match crate::audio::read_mono(source, from, to, RATE) {
            Ok(s) => s,
            // A file without sound has nothing to say.
            Err(e) => {
                eprintln!("captions: {source}: {e:#}");
                continue;
            }
        };
        if samples.iter().all(|s| s.abs() < 1e-4) {
            continue;
        }
        let wav = dir.join(format!("{}.wav", clip.id));
        let run = Run {
            tool: &tool,
            model: &model,
            wav: &wav,
            job,
            cancel,
        };
        let speech = if job.language == "km" {
            transcribe_pieces(&run, &samples, &report)?
        } else {
            write_wav(&wav, &samples)?;
            let speech = run.whisper(false, &report);
            let _ = std::fs::remove_file(&wav);
            speech?
        };
        out.extend(captions::to_timeline(clip, &speech));
    }
    send(Update::Progress(Phase::Applying, None));
    out.sort_by_key(|c| c.start);
    Ok(out)
}

/// The model, downloaded once (to a `.part`, renamed when whole).
fn download_model(
    url: &str,
    path: &Path,
    cancel: &AtomicBool,
    send: &impl Fn(Update),
) -> Result<()> {
    send(Update::Progress(Phase::Downloading, Some(0.)));
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    // Its own agent: a big file outlasts the app's usual timeouts.
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_connect(Some(std::time::Duration::from_secs(15)))
            .build(),
    );
    let mut res = agent
        .get(url)
        .call()
        .context("Couldn't download the speech model")?;
    let total = res
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let part = path.with_extension("part");
    let mut file = std::fs::File::create(&part)?;
    let mut reader = res.body_mut().with_config().limit(1 << 31).reader();
    let mut buf = vec![0u8; 1 << 16];
    let mut got = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            let _ = std::fs::remove_file(&part);
            bail!("cancelled");
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        got += n as u64;
        if let Some(t) = total {
            send(Update::Progress(
                Phase::Downloading,
                Some(got as f32 / t.max(1) as f32),
            ));
        }
    }
    file.sync_all()?;
    if total.is_some_and(|t| t != got) {
        let _ = std::fs::remove_file(&part);
        bail!("The speech model download was cut short");
    }
    std::fs::rename(&part, path)?;
    Ok(())
}

/// 16-bit mono PCM at `RATE`.
fn write_wav(path: &Path, samples: &[f32]) -> Result<()> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1., 1.) * 32767.) as i16).to_le_bytes());
    }
    std::fs::write(path, out)?;
    Ok(())
}

/// Loudness hops: 20 ms.
const HOP: usize = RATE as usize / 50;
const HOP_MICROS: Micros = 20_000;

/// Khmer pieces: at most 6 s (Whisper spends 2–3 tokens per Khmer
/// character, and a longer piece overflows its window: empty or looping
/// text), cut at a quiet point past 3 s, never leaving under 1.5 s.
const PIECE_MAX: usize = 6 * 50;
const PIECE_MIN_CUT: usize = 3 * 50;
const PIECE_MIN_TAIL: usize = 75;
/// Speech is padded 0.25 s and joined across pauses of up to 1 s, so a
/// piece reads as a phrase rather than scraps with clipped words.
const SPEECH_PAD: usize = 12;
const JOIN_GAP: usize = 50;
/// A piece that came back empty is split and retried this many times…
const MAX_SPLIT_DEPTH: u32 = 2;
/// …while each half keeps 3 s.
const MIN_SPLIT_SAMPLES: usize = RATE as usize * 3;

/// RMS loudness per hop.
fn envelope(samples: &[f32]) -> Vec<f32> {
    samples
        .as_chunks::<HOP>()
        .0
        .iter()
        .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / HOP as f32).sqrt())
        .collect()
}

/// Where there's sound worth transcribing, in hops: louder than the
/// clip's quiet floor by a share of its range, padded and joined across
/// short pauses.
fn speech_ranges(env: &[f32]) -> Vec<(usize, usize)> {
    if env.is_empty() {
        return vec![];
    }
    let mut sorted = env.to_vec();
    sorted.sort_by(f32::total_cmp);
    let at = |q: f32| sorted[((sorted.len() - 1) as f32 * q) as usize];
    let (floor, peak) = (at(0.1), at(0.95));
    let threshold = (floor + (peak - floor) * 0.12).max(0.003);
    let mut out: Vec<(usize, usize)> = vec![];
    let mut i = 0;
    while i < env.len() {
        if env[i] < threshold {
            i += 1;
            continue;
        }
        let start = i;
        while i < env.len() && env[i] >= threshold {
            i += 1;
        }
        let (a, b) = (
            start.saturating_sub(SPEECH_PAD),
            (i + SPEECH_PAD).min(env.len()),
        );
        match out.last_mut() {
            Some(last) if a <= last.1 + JOIN_GAP => last.1 = b,
            _ => out.push((a, b)),
        }
    }
    out
}

/// The quietest hop in `from..to`.
fn quietest(env: &[f32], from: usize, to: usize) -> Option<usize> {
    (from..to.min(env.len())).min_by(|&a, &b| env[a].total_cmp(&env[b]))
}

/// Ranges longer than a piece cut at quiet points.
fn pieces(env: &[f32], ranges: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut out = vec![];
    for &(a, b) in ranges {
        let mut start = a;
        while b - start > PIECE_MAX {
            let (from, to) = (
                start + PIECE_MIN_CUT,
                (start + PIECE_MAX).min(b - PIECE_MIN_TAIL),
            );
            let cut = if from < to {
                quietest(env, from, to).unwrap_or(to)
            } else {
                start + (b - start) / 2
            };
            out.push((start, cut));
            start = cut;
        }
        out.push((start, b));
    }
    out
}

/// Khmer: the clip's speech in short pieces, each transcribed on its own
/// and cut into lines; segments come back in the clip's source time
/// (from the start of `samples`).
fn transcribe_pieces(run: &Run, samples: &[f32], report: &impl Fn(f32)) -> Result<Vec<Cue>> {
    let env = envelope(samples);
    let list = pieces(&env, &speech_ranges(&env));
    let n = list.len().max(1) as f32;
    let mut segments = vec![];
    for (k, &(a, b)) in list.iter().enumerate() {
        if run.cancel.load(Ordering::Relaxed) {
            break;
        }
        report(k as f32 / n);
        let piece = &samples[a * HOP..(b * HOP).min(samples.len())];
        let offset = a as Micros * HOP_MICROS;
        for seg in transcribe_span(run, piece, 0)? {
            segments.push(Cue {
                start: offset + seg.start,
                end: offset + seg.end,
                ..seg
            });
        }
    }
    report(1.);
    let _ = std::fs::remove_file(run.wav);
    // A syllable can be cut between pieces too.
    Ok(captions::join_split_syllables(segments)
        .iter()
        .flat_map(|s| {
            captions::lines(
                &s.text,
                s.start,
                s.end,
                &run.job.language,
                run.job.max_chars,
            )
        })
        .collect())
}

/// One piece through whisper, its segments clamped to it and Khmer
/// syllables split across segments joined. A piece that comes back empty
/// (too much speech for one window) is split at its quietest middle point
/// and each half retried.
fn transcribe_span(run: &Run, samples: &[f32], depth: u32) -> Result<Vec<Cue>> {
    let len = samples.len() as Micros * SECOND / RATE as Micros;
    write_wav(run.wav, samples)?;
    let got: Vec<Cue> = run
        .whisper(true, &|_| {})?
        .into_iter()
        .filter_map(|c| {
            let (start, end) = (c.start.clamp(0, len), c.end.clamp(0, len));
            (end > start).then_some(Cue { start, end, ..c })
        })
        .collect();
    let got = captions::join_split_syllables(got);
    if !got.is_empty()
        || depth >= MAX_SPLIT_DEPTH
        || samples.len() < 2 * MIN_SPLIT_SAMPLES
        || run.cancel.load(Ordering::Relaxed)
    {
        return Ok(got);
    }
    let env = envelope(samples);
    let cut = quietest(&env, env.len() * 3 / 10, env.len() * 7 / 10)
        .map_or(samples.len() / 2, |h| h * HOP);
    let at = cut as Micros * SECOND / RATE as Micros;
    let mut out = transcribe_span(run, &samples[..cut], depth + 1)?;
    for c in transcribe_span(run, &samples[cut..], depth + 1)? {
        out.push(Cue {
            start: c.start + at,
            end: c.end + at,
            ..c
        });
    }
    Ok(out)
}

/// One whisper-cli run's inputs.
struct Run<'a> {
    tool: &'a Path,
    model: &'a Path,
    wav: &'a Path,
    job: &'a Job,
    cancel: &'a AtomicBool,
}

impl Run<'_> {
    /// The WAV through whisper-cli: its segments (times from the WAV's
    /// start). A whole file is cut into line-sized segments by whisper; a
    /// `piece` (Khmer) is short already and is cut into lines afterwards.
    fn whisper(&self, piece: bool, report: &impl Fn(f32)) -> Result<Vec<Cue>> {
        let base = self.wav.with_extension("");
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
        let mut cmd = Command::new(self.tool);
        cmd.arg("-m")
            .arg(self.model)
            .arg("-f")
            .arg(self.wav)
            .args(["-l", &self.job.language])
            .args(["-t", &threads.to_string()])
            // Beam search: greedy decoding gives up on the Khmer model.
            .args(["-bs", "5"])
            .args(["-oj", "-np", "-pp"])
            .arg("-of")
            .arg(&base);
        if !piece {
            // whisper's own cut counts bytes, which would split a Khmer
            // syllable; pieces are cut into lines by `captions::lines`.
            cmd.args(["-ml", &self.job.max_chars.to_string(), "-sow"]);
        }
        let mut child = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Couldn't start {}", self.tool.display()))?;
        // Progress lines on stderr: "… progress =  42%".
        let mut last_err = String::new();
        if let Some(err) = child.stderr.take() {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                if self.cancel.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    break;
                }
                if let Some(p) = line
                    .split("progress =")
                    .nth(1)
                    .and_then(|s| s.trim().trim_end_matches('%').trim().parse::<f32>().ok())
                {
                    report(p / 100.);
                } else if !line.trim().is_empty() {
                    last_err = line;
                }
            }
        }
        let status = child.wait()?;
        if self.cancel.load(Ordering::Relaxed) {
            return Ok(vec![]);
        }
        let json_path = base.with_extension("json");
        if !status.success() {
            bail!("whisper-cli failed: {last_err}");
        }
        let bytes = std::fs::read(&json_path).context("whisper-cli wrote nothing")?;
        let _ = std::fs::remove_file(&json_path);
        Ok(parse_whisper_json(&stitch_utf8(&bytes)))
    }
}

/// whisper.cpp writes each segment's bytes as decoded, and a segment can
/// end inside a character (a Khmer letter is 3 bytes, split across two
/// tokens): the next segment then starts with the rest of it. The leading
/// part is moved to where its rest is; bytes that never complete are
/// dropped.
fn stitch_utf8(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut pending: Vec<u8> = vec![];
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let len = match b {
            0x00..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            _ => 0, // a continuation byte with no lead
        };
        if len == 0 {
            // The rest of a character whose start was cut off before.
            let mut run = 0;
            while i + run < bytes.len() && (0x80..=0xbf).contains(&bytes[i + run]) {
                run += 1;
            }
            let mut whole = std::mem::take(&mut pending);
            whole.extend_from_slice(&bytes[i..i + run]);
            if std::str::from_utf8(&whole).is_ok() {
                out.extend_from_slice(&whole);
            }
            i += run;
            continue;
        }
        let end = (i + len).min(bytes.len());
        let tail = bytes[i + 1..end]
            .iter()
            .take_while(|c| (0x80..=0xbf).contains(*c))
            .count();
        if 1 + tail == len {
            out.extend_from_slice(&bytes[i..end]);
            i = end;
        } else {
            // Cut short: keep it for the continuation further on.
            pending = bytes[i..i + 1 + tail].to_vec();
            i += 1 + tail;
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

/// whisper.cpp's JSON: `transcription[].offsets.{from,to}` (ms), `text`.
fn parse_whisper_json(text: &str) -> Vec<Cue> {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return vec![];
    };
    let ms = |s: &Value, k: &str| s["offsets"][k].as_i64().map(|m| m as Micros * 1000);
    v["transcription"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    // A model stuck in a loop wrote nothing worth showing.
                    let text = captions::clean_speech(s["text"].as_str()?)?;
                    // Non-speech marks ("[Music]", "(laughs)") aren't captions.
                    let bracketed = (text.starts_with('[') && text.ends_with(']'))
                        || (text.starts_with('(') && text.ends_with(')'));
                    (!text.is_empty() && !bracketed).then_some(Cue {
                        start: ms(s, "from")?,
                        end: ms(s, "to")?,
                        text,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_whisper_segments() {
        let json = r#"{"transcription":[
            {"offsets":{"from":0,"to":1500},"text":" Hello there"},
            {"offsets":{"from":1500,"to":2000},"text":" [Music]"},
            {"offsets":{"from":2000,"to":3200},"text":" again"}]}"#;
        let cues = parse_whisper_json(json);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "Hello there");
        assert_eq!((cues[1].start, cues[1].end), (2_000_000, 3_200_000));
    }

    /// `REEL_SPEECH_FILE=… [REEL_SPEECH_LANG=km] cargo test --release
    /// transcribes_a_file -- --ignored --nocapture`: the whole job on a real
    /// file (needs whisper-cli; downloads the model once).
    #[test]
    #[ignore]
    fn transcribes_a_file() {
        let path = std::env::var("REEL_SPEECH_FILE").expect("REEL_SPEECH_FILE");
        let language = std::env::var("REEL_SPEECH_LANG").unwrap_or_else(|_| "km".into());
        let secs: i64 = std::env::var("REEL_SPEECH_SECS").map_or(30, |s| s.parse().unwrap());
        let clip = Clip {
            id: "speech-test".into(),
            kind: "video".into(),
            timeline_start: 0,
            timeline_duration: secs * SECOND,
            speed: 1.,
            source: Some(path),
            source_start: Some(0),
            source_duration: None,
            text: None,
            hidden: false,
            extra: Default::default(),
        };
        let job = Job {
            clips: vec![clip],
            language,
            max_chars: 26,
        };
        let cues = transcribe_all(&job, &AtomicBool::new(false), &|_| {}).unwrap();
        for c in &cues {
            println!(
                "{:6.2}-{:6.2} {}",
                c.start as f64 / 1e6,
                c.end as f64 / 1e6,
                c.text
            );
        }
        assert!(!cues.is_empty());
    }

    #[test]
    fn stitches_characters_cut_between_segments() {
        // "ក" is E1 9E 80: its first byte ends one text, the rest starts the next.
        let mut b = br#"{"text":"a"#.to_vec();
        b.push(0xe1);
        b.extend_from_slice(br#""},{"text":""#);
        b.extend_from_slice(&[0x9e, 0x80]);
        b.extend_from_slice(br#"b"}"#);
        assert_eq!(stitch_utf8(&b), r#"{"text":"a"},{"text":"កb"}"#);
        assert_eq!(stitch_utf8("ok ក".as_bytes()), "ok ក");
        // A cut-off end is dropped.
        assert_eq!(stitch_utf8(&[b'x', 0xe1, 0x9e]), "x");
    }

    #[test]
    fn writes_a_wav_header() {
        let dir = std::env::temp_dir().join("reel-wav-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.wav");
        write_wav(&p, &[0., 0.5, -0.5]).unwrap();
        let b = std::fs::read(&p).unwrap();
        assert_eq!(&b[..4], b"RIFF");
        assert_eq!(b.len(), 44 + 6);
        assert_eq!(u32::from_le_bytes([b[24], b[25], b[26], b[27]]), RATE);
    }
}
