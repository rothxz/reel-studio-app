//! Voiceover for the agent: words → speech, a file on disk.
//!
//! Four voices, the best one the user has:
//! - **Kokoro**, a small open speech model run on this Mac (`local_voice`):
//!   natural, free, offline once set up; the default on Apple silicon;
//! - **OpenAI's speech model** with the user's OpenAI key (natural voices
//!   that take a tone: "friendly teacher, upbeat");
//! - **Gemini's speech model** with the user's Gemini key (same idea);
//! - **the system's own speech** (`say` on macOS): offline, always there,
//!   robotic: the last resort.
//!
//! A line is spoken once and kept in `<data>/agent/voice/` under a hash of
//! what made it (engine, voice, tone, words), so asking again is instant
//! and a saved project plays offline.
//!
//! `measure` reads where the speech is audible and where it pauses, which
//! is what captions are timed against (`captions::chunks`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{keys, local_voice};
use crate::model::Micros;

/// Who speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Kokoro,
    OpenAi,
    Gemini,
    System,
}

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Engine::Kokoro => "kokoro",
            Engine::OpenAi => "openai",
            Engine::Gemini => "gemini",
            Engine::System => "system",
        }
    }

    /// The best engine there is (or the one asked for): the local voice
    /// when this Mac can run it, else a cloud one there's a key for.
    pub fn pick(wanted: Option<&str>) -> Result<Engine> {
        match wanted.map(|w| w.trim().to_lowercase()).as_deref() {
            None | Some("") | Some("auto") | Some("best") => Ok(if local_voice::available() {
                Engine::Kokoro
            } else if keys::has("openai") {
                Engine::OpenAi
            } else if keys::has("gemini") {
                Engine::Gemini
            } else {
                Engine::System
            }),
            Some("openai") | Some("chatgpt") => {
                if keys::has("openai") {
                    Ok(Engine::OpenAi)
                } else {
                    bail!(
                        "No OpenAI key saved: add one in the agent's setup (ChatGPT), or use the system voice"
                    )
                }
            }
            Some("gemini") | Some("google") => {
                if keys::has("gemini") {
                    Ok(Engine::Gemini)
                } else {
                    bail!(
                        "No Gemini key saved: add one in the agent's setup, or use the system voice"
                    )
                }
            }
            Some("kokoro") | Some("local") | Some("free") => {
                if local_voice::supported() {
                    Ok(Engine::Kokoro)
                } else {
                    bail!("The local voice needs a Mac with Apple silicon")
                }
            }
            Some("system") | Some("mac") | Some("say") => Ok(Engine::System),
            Some(other) => {
                bail!("voice engine \"{other}\" is auto, kokoro, openai, gemini or system")
            }
        }
    }
}

/// OpenAI's voices, and whether each reads as a woman's (for "female" /
/// "male" asks).
const OPENAI_VOICES: &[(&str, bool)] = &[
    ("coral", true),
    ("nova", true),
    ("shimmer", true),
    ("sage", true),
    ("alloy", true),
    ("ballad", false),
    ("ash", false),
    ("echo", false),
    ("onyx", false),
    ("fable", false),
    ("verse", false),
];
const OPENAI_MODEL: &str = "gpt-4o-mini-tts";

const GEMINI_VOICES: &[(&str, bool)] = &[
    ("Kore", true),
    ("Aoede", true),
    ("Leda", true),
    ("Zephyr", true),
    ("Puck", false),
    ("Charon", false),
    ("Fenrir", false),
    ("Orus", false),
];
const GEMINI_MODEL: &str = "gemini-2.5-flash-preview-tts";

/// Kokoro's English voices: (name, a woman's, British), the best first in
/// each kind. The default is the first.
pub const KOKORO_VOICES: &[(&str, bool, bool)] = &[
    ("af_heart", true, false),
    ("af_bella", true, false),
    ("af_nicole", true, false),
    ("af_sarah", true, false),
    ("af_nova", true, false),
    ("af_aoede", true, false),
    ("af_kore", true, false),
    ("af_sky", true, false),
    ("am_michael", false, false),
    ("am_fenrir", false, false),
    ("am_puck", false, false),
    ("am_echo", false, false),
    ("am_eric", false, false),
    ("am_liam", false, false),
    ("am_onyx", false, false),
    ("am_adam", false, false),
    ("bf_emma", true, true),
    ("bf_isabella", true, true),
    ("bf_alice", true, true),
    ("bf_lily", true, true),
    ("bm_george", false, true),
    ("bm_fable", false, true),
    ("bm_lewis", false, true),
    ("bm_daniel", false, true),
];

/// System voices worth a narration, best first; the novelty ones (Bells,
/// Bubbles, Zarvox…) never get picked.
const SYSTEM_FEMALE: &[&str] = &[
    "Ava", "Zoe", "Allison", "Susan", "Samantha", "Flo", "Karen", "Moira", "Shelley", "Sandy",
];
const SYSTEM_MALE: &[&str] = &[
    "Evan", "Nathan", "Tom", "Daniel", "Reed", "Eddy", "Rocko", "Alex", "Aaron",
];

/// Speaking rate for the system voice (words a minute): a little brisk,
/// as short-form narration is.
const SYSTEM_RATE: u32 = 182;

/// What to say and how.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub text: String,
    /// A voice name, or "female" / "male"; None: the engine's default.
    pub voice: Option<String>,
    /// How to say it ("friendly teacher, upbeat"); cloud voices only.
    pub tone: Option<String>,
    /// "auto" (default), "openai", "gemini" or "system".
    pub engine: Option<String>,
}

/// Spoken words on disk.
#[derive(Clone, Debug)]
pub struct Speech {
    pub path: PathBuf,
    pub engine: Engine,
    pub voice: String,
    pub shape: Measured,
}

/// Where a recording of speech is audible, and its pauses.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measured {
    pub length: Micros,
    /// First and last audible moments.
    pub onset: Micros,
    pub tail: Micros,
    /// Silences inside the speech (start, end), in order.
    pub pauses: Vec<(Micros, Micros)>,
}

fn voice_dir() -> PathBuf {
    crate::prefs::data_dir().join("agent").join("voice")
}

fn hash(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0]);
    }
    h.finalize()
        .iter()
        .take(10)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A voice from `list` for `wanted` (a name, "female", "male"), else the
/// first.
fn pick_listed(list: &[(&str, bool)], wanted: Option<&str>) -> String {
    let w = wanted.map(|w| w.trim().to_lowercase()).unwrap_or_default();
    let female = matches!(w.as_str(), "female" | "woman" | "girl" | "f");
    let male = matches!(w.as_str(), "male" | "man" | "boy" | "m");
    if let Some((v, _)) = list.iter().find(|(v, _)| v.to_lowercase() == w) {
        return v.to_string();
    }
    if male && let Some((v, _)) = list.iter().find(|(_, f)| !f) {
        return v.to_string();
    }
    let _ = female;
    list[0].0.to_string()
}

/// A Kokoro voice for `wanted`: a name ("af_heart", "heart", "Emma"),
/// "female" / "male", optionally "british" ("british male").
pub fn pick_kokoro(wanted: Option<&str>) -> String {
    let w = wanted.map(|w| w.trim().to_lowercase()).unwrap_or_default();
    if let Some((v, _, _)) = KOKORO_VOICES
        .iter()
        .find(|(v, _, _)| *v == w || v.split_once('_').is_some_and(|(_, n)| n == w))
    {
        return v.to_string();
    }
    let has = |words: &[&str]| {
        w.split(|c: char| !c.is_alphanumeric())
            .any(|x| words.contains(&x))
    };
    let male = has(&["male", "man", "boy", "m", "guy"]);
    let british = has(&["british", "uk", "london"]);
    KOKORO_VOICES
        .iter()
        .find(|(_, f, b)| *f != male && *b == british)
        .map_or("af_heart", |(v, _, _)| v)
        .to_string()
}

/// Kokoro takes no tone, only a pace: an energetic tone reads a little
/// quicker, a calm one a little slower.
pub fn kokoro_speed(tone: &str) -> f32 {
    let t = tone.to_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if any(&[
        "fast",
        "energetic",
        "excited",
        "punchy",
        "hype",
        "upbeat",
        "urgent",
    ]) {
        1.08
    } else if any(&[
        "slow", "calm", "soothing", "gentle", "relax", "meditat", "soft",
    ]) {
        0.92
    } else {
        1.0
    }
}

/// Says `req.text`; blocking (a cloud call or a `say` run): off the UI
/// thread.
pub fn speak(req: &Request) -> Result<Speech> {
    let text = req.text.trim();
    if text.is_empty() {
        bail!("Nothing to say");
    }
    let engine = Engine::pick(req.engine.as_deref())?;
    let tone = req
        .tone
        .clone()
        .unwrap_or_else(|| "Friendly, clear and warm, like a good teacher; natural pace".into());
    std::fs::create_dir_all(voice_dir())?;
    let (voice, path) = match engine {
        Engine::Kokoro => {
            let voice = pick_kokoro(req.voice.as_deref());
            let speed = kokoro_speed(&tone);
            let model = local_voice::model();
            let path = voice_dir().join(format!(
                "{}.wav",
                hash(&["kokoro", &model, &voice, &format!("{speed:.2}"), text])
            ));
            if !path.exists() {
                local_voice::speak(
                    &local_voice::Line {
                        text,
                        voice: &voice,
                        speed,
                        instruct: Some(&tone),
                    },
                    &path,
                )
                .context("Local voice")?;
            }
            (voice, path)
        }
        Engine::OpenAi => {
            let voice = pick_listed(OPENAI_VOICES, req.voice.as_deref());
            let path = voice_dir().join(format!(
                "{}.mp3",
                hash(&["openai", OPENAI_MODEL, &voice, &tone, text])
            ));
            if !path.exists() {
                openai_speech(text, &voice, &tone, &path)?;
            }
            (voice, path)
        }
        Engine::Gemini => {
            let voice = pick_listed(GEMINI_VOICES, req.voice.as_deref());
            let path = voice_dir().join(format!(
                "{}.wav",
                hash(&["gemini", GEMINI_MODEL, &voice, &tone, text])
            ));
            if !path.exists() {
                gemini_speech(text, &voice, &tone, &path)?;
            }
            (voice, path)
        }
        Engine::System => {
            let voice = system_voice(req.voice.as_deref())?;
            let path = voice_dir().join(format!(
                "{}.aiff",
                hash(&["system", &voice, &SYSTEM_RATE.to_string(), text])
            ));
            if !path.exists() {
                system_speech(text, &voice, &path)?;
            }
            (voice, path)
        }
    };
    let shape = measure(&path)?;
    Ok(Speech {
        path,
        engine,
        voice,
        shape,
    })
}

// ---- the engines -------------------------------------------------------------------

fn http() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(10)))
                .timeout_global(Some(Duration::from_secs(120)))
                .http_status_as_error(false)
                .build(),
        )
    })
}

/// Posts JSON; the body's bytes, or the service's own error.
fn post(url: &str, headers: &[(&str, String)], body: &Value) -> Result<Vec<u8>> {
    let mut req = http().post(url).header("Content-Type", "application/json");
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let mut response = req
        .send(body.to_string().as_bytes())
        .map_err(|e| anyhow!("{e}"))?;
    let status = response.status().as_u16();
    let bytes = response
        .body_mut()
        .with_config()
        .limit(64 << 20)
        .read_to_vec()?;
    if !(200..300).contains(&status) {
        let text = String::from_utf8_lossy(&bytes);
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .and_then(Value::as_str)
                    .map(String::from)
            })
            .unwrap_or_else(|| text.chars().take(200).collect());
        bail!("HTTP {status}: {message}");
    }
    Ok(bytes)
}

/// Writes `bytes` to `path` through a temporary name (never a half file).
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn openai_speech(text: &str, voice: &str, tone: &str, path: &Path) -> Result<()> {
    let key = keys::get("openai").context("No OpenAI key saved")?;
    let body = json!({
        "model": OPENAI_MODEL,
        "voice": voice,
        "input": text,
        "instructions": tone,
        "response_format": "mp3",
    });
    let bytes = post(
        "https://api.openai.com/v1/audio/speech",
        &[("Authorization", format!("Bearer {key}"))],
        &body,
    )
    .context("OpenAI speech")?;
    if bytes.len() < 256 {
        bail!("OpenAI sent back no sound");
    }
    write_atomic(path, &bytes)
}

fn gemini_speech(text: &str, voice: &str, tone: &str, path: &Path) -> Result<()> {
    let key = keys::get("gemini").context("No Gemini key saved")?;
    let body = json!({
        "contents": [{ "parts": [{ "text": format!("Say this ({tone}):\n{text}") }] }],
        "generationConfig": {
            "responseModalities": ["AUDIO"],
            "speechConfig": { "voiceConfig": { "prebuiltVoiceConfig": { "voiceName": voice } } }
        }
    });
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{GEMINI_MODEL}:generateContent"
    );
    let bytes = post(&url, &[("x-goog-api-key", key)], &body).context("Gemini speech")?;
    let json: Value = serde_json::from_slice(&bytes)?;
    let data = json
        .pointer("/candidates/0/content/parts/0/inlineData/data")
        .and_then(Value::as_str)
        .context("Gemini sent back no sound")?;
    let pcm = base64_decode(data).context("Gemini's sound didn't decode")?;
    write_atomic(path, &wav(&pcm, 24_000, 1))
}

/// 16-bit little-endian PCM in a WAV file.
fn wav(pcm: &[u8], rate: u32, channels: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm.len() + 44);
    let byte_rate = rate * channels as u32 * 2;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The system's installed voices: (name as `say` takes it, locale).
pub fn system_voices() -> Vec<(String, String)> {
    let Ok(out) = Command::new("say").args(["-v", "?"]).output() else {
        return vec![];
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(parse_voice_line)
        .collect()
}

/// "Samantha (English (US))   en_US    # Hello…" → (name, "en_US").
fn parse_voice_line(line: &str) -> Option<(String, String)> {
    let head = line.split('#').next()?.trim_end();
    let (name, locale) = head.rsplit_once(char::is_whitespace)?;
    let locale = locale.trim();
    if !locale.contains('_') {
        return None;
    }
    Some((name.trim().to_string(), locale.to_string()))
}

/// The plain name of a listed voice ("Samantha (English (US))" →
/// "Samantha"), with its quality tag kept ("Ava (Premium)").
fn base_name(listed: &str) -> &str {
    listed.split(" (").next().unwrap_or(listed).trim()
}

/// The system voice to use for `wanted` (a name, "female", "male").
fn system_voice(wanted: Option<&str>) -> Result<String> {
    if !cfg!(target_os = "macos") {
        bail!(
            "No voice: this system has no speech built in. Save an OpenAI or Gemini key in the agent's setup."
        );
    }
    let voices = system_voices();
    pick_system_voice(&voices, wanted)
        .ok_or_else(|| anyhow!("No English system voice is installed (System Settings › Accessibility › Spoken Content)"))
}

fn pick_system_voice(voices: &[(String, String)], wanted: Option<&str>) -> Option<String> {
    let w = wanted.map(|w| w.trim().to_lowercase()).unwrap_or_default();
    // A voice asked for by name, in any language.
    if !w.is_empty()
        && let Some((name, _)) = voices
            .iter()
            .find(|(n, _)| n.to_lowercase() == w || base_name(n).to_lowercase() == w)
    {
        return Some(name.clone());
    }
    let english: Vec<&(String, String)> =
        voices.iter().filter(|(_, l)| l.starts_with("en")).collect();
    let male = matches!(w.as_str(), "male" | "man" | "boy" | "m");
    let order = if male {
        [SYSTEM_MALE, SYSTEM_FEMALE]
    } else {
        [SYSTEM_FEMALE, SYSTEM_MALE]
    };
    // Premium / Enhanced downloads first, then the plain ones, US first.
    let quality = |n: &str| {
        let n = n.to_lowercase();
        if n.contains("premium") {
            0
        } else if n.contains("enhanced") {
            1
        } else {
            2
        }
    };
    let mut best: Option<(usize, usize, i32, &String)> = None;
    for (rank_list, list) in order.iter().enumerate() {
        for (rank, want) in list.iter().enumerate() {
            for (name, locale) in &english {
                if base_name(name) != *want {
                    continue;
                }
                let us = if locale == "en_US" { 0 } else { 1 };
                let key = (rank_list, quality(name), rank as i32 * 2 + us);
                if best.is_none_or(|(a, b, c, _)| key < (a, b, c)) {
                    best = Some((key.0, key.1, key.2, name));
                }
            }
        }
    }
    best.map(|(_, _, _, n)| n.clone())
}

fn system_speech(text: &str, voice: &str, path: &Path) -> Result<()> {
    let tmp = path.with_extension("part.aiff");
    let status = Command::new("say")
        .arg("-v")
        .arg(voice)
        .arg("-r")
        .arg(SYSTEM_RATE.to_string())
        .arg("-o")
        .arg(&tmp)
        .arg("--")
        .arg(text)
        .status()
        .context("couldn't run the system speech")?;
    if !status.success() || !tmp.exists() {
        let _ = std::fs::remove_file(&tmp);
        bail!("The system voice \"{voice}\" couldn't speak this");
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// ---- measuring -----------------------------------------------------------------------

/// A pause this long inside speech is a break between words / phrases.
const MIN_PAUSE: Micros = 120_000;

/// Reads a speech file's audible span and its pauses.
pub fn measure(path: &Path) -> Result<Measured> {
    let wave = crate::decoder::waveform(&path.to_string_lossy())?;
    measured_from(&wave.peaks, wave.per_second).context("The voice came out silent")
}

pub fn measured_from(peaks: &[f32], per_second: f64) -> Option<Measured> {
    let max = peaks.iter().copied().fold(0f32, f32::max);
    if max <= 1e-4 {
        return None;
    }
    let at = |i: usize| (i as f64 / per_second * 1e6).round() as Micros;
    let loud = |p: f32| p >= max * 0.06;
    let onset = peaks.iter().position(|&p| loud(p))?;
    let tail = peaks.iter().rposition(|&p| loud(p))? + 1;
    let mut pauses = vec![];
    let mut quiet_from = None;
    for (i, &p) in peaks.iter().enumerate().take(tail).skip(onset) {
        match (loud(p), quiet_from) {
            (false, None) => quiet_from = Some(i),
            (true, Some(from)) => {
                if at(i) - at(from) >= MIN_PAUSE {
                    pauses.push((at(from), at(i)));
                }
                quiet_from = None;
            }
            _ => {}
        }
    }
    Some(Measured {
        length: at(peaks.len()),
        onset: at(onset),
        tail: at(tail),
        pauses,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_system_voice_lines() {
        assert_eq!(
            parse_voice_line("Samantha            en_US    # Hello! My name is Samantha."),
            Some(("Samantha".into(), "en_US".into()))
        );
        assert_eq!(
            parse_voice_line("Eddy (English (UK)) en_GB    # Hello! My name is Eddy."),
            Some(("Eddy (English (UK))".into(), "en_GB".into()))
        );
        assert_eq!(parse_voice_line("garbage"), None);
    }

    #[test]
    fn picks_a_narration_voice() {
        let v = |n: &str, l: &str| (n.to_string(), l.to_string());
        let voices = vec![
            v("Bubbles", "en_US"),
            v("Daniel", "en_GB"),
            v("Flo (English (US))", "en_US"),
            v("Samantha (English (US))", "en_US"),
            v("Thomas", "fr_FR"),
        ];
        assert_eq!(
            pick_system_voice(&voices, None).as_deref(),
            Some("Samantha (English (US))")
        );
        assert_eq!(
            pick_system_voice(&voices, Some("male")).as_deref(),
            Some("Daniel")
        );
        assert_eq!(
            pick_system_voice(&voices, Some("thomas")).as_deref(),
            Some("Thomas")
        );
        let mut premium = voices.clone();
        premium.push(v("Ava (Premium)", "en_US"));
        assert_eq!(
            pick_system_voice(&premium, None).as_deref(),
            Some("Ava (Premium)")
        );
    }

    /// Speaks through the local voice for real (sets it up if needed: uv,
    /// ~1 GB). `cargo t kokoro_speaks -- --ignored`
    #[test]
    #[ignore]
    fn kokoro_speaks_for_real() {
        let s = speak(&Request {
            text: "Retrieval means the model reads your documents first, then answers.".into(),
            voice: Some("male".into()),
            tone: Some("upbeat".into()),
            engine: Some("kokoro".into()),
        })
        .unwrap();
        assert_eq!(s.engine, Engine::Kokoro);
        assert_eq!(s.voice, "am_michael");
        assert!(s.shape.tail - s.shape.onset > 2_000_000, "{:?}", s.shape);
        eprintln!("{} {:?}", s.path.display(), s.shape);
    }

    #[test]
    fn picks_kokoro_voices() {
        assert_eq!(pick_kokoro(None), "af_heart");
        assert_eq!(pick_kokoro(Some("male")), "am_michael");
        assert_eq!(pick_kokoro(Some("British male")), "bm_george");
        assert_eq!(pick_kokoro(Some("british female")), "bf_emma");
        assert_eq!(pick_kokoro(Some("Puck")), "am_puck");
        assert_eq!(pick_kokoro(Some("af_nova")), "af_nova");
        assert!(kokoro_speed("excited and punchy") > 1.);
        assert!(kokoro_speed("calm and warm") < 1.);
    }

    #[test]
    fn picks_cloud_voices() {
        assert_eq!(pick_listed(OPENAI_VOICES, None), "coral");
        assert_eq!(pick_listed(OPENAI_VOICES, Some("Onyx")), "onyx");
        assert_eq!(pick_listed(OPENAI_VOICES, Some("male")), "ballad");
    }

    #[test]
    fn finds_pauses_in_speech() {
        // 100 slices a second: quiet, 0.5 s talk, 0.3 s pause, 0.4 s talk, quiet.
        let mut p = vec![0.0; 10];
        p.extend(vec![0.8; 50]);
        p.extend(vec![0.01; 30]);
        p.extend(vec![0.6; 40]);
        p.extend(vec![0.0; 20]);
        let m = measured_from(&p, 100.).unwrap();
        assert_eq!(m.onset, 100_000);
        assert_eq!(m.tail, 1_300_000);
        assert_eq!(m.pauses, vec![(600_000, 900_000)]);
        assert_eq!(m.length, 1_500_000);
    }

    #[test]
    fn decodes_base64_and_wraps_wav() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        let w = wav(&[0, 0, 1, 0], 24_000, 1);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 48);
    }
}
