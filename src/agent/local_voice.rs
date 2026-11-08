//! The free natural voice: Kokoro (82M parameters), run on this Mac's GPU
//! through MLX by a small Python worker (`speech_worker.py`).
//!
//! Set up on first use in `<data>/agent/speech/`: a Python 3.12 environment
//! made with `uv` (pinned packages, no PyTorch) and the model (~340 MB)
//! downloaded from Hugging Face on the worker's first start. Deleting that
//! folder removes it all; the next voiceover sets it up again.
//!
//! The worker is started once and kept: it loads the model in a few seconds
//! and then speaks a line in about a second. Lines are spoken one at a time
//! (the GPU does one well, not four at once).

use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Value, json};

/// The model; `REEL_LOCAL_VOICE_MODEL` names another (an MLX build of
/// Kokoro, or of a Qwen3-TTS CustomVoice model).
pub const MODEL: &str = "mlx-community/Kokoro-82M-bf16";

/// Bumped when the packages change: an older setup is redone.
const SETUP_VERSION: &str = "1";

/// What the worker needs, pinned to versions known to work together.
/// misaki's `en` extra is spelled out without `spacy-curated-transformers`,
/// which would pull in PyTorch (~600 MB) for nothing.
const PACKAGES: &[&str] = &[
    "mlx-audio==0.6.4",
    "misaki==0.9.4",
    "spacy==3.8.18",
    "num2words==0.5.14",
    "phonemizer-fork==3.3.2",
    "espeakng-loader==0.2.4",
    "soundfile==0.14.0",
    "en_core_web_sm @ https://github.com/explosion/spacy-models/releases/download/en_core_web_sm-3.8.0/en_core_web_sm-3.8.0-py3-none-any.whl",
];

const WORKER_SOURCE: &str = include_str!("speech_worker.py");

pub fn model() -> String {
    std::env::var("REEL_LOCAL_VOICE_MODEL")
        .ok()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| MODEL.to_string())
}

fn dir() -> PathBuf {
    crate::prefs::data_dir().join("agent").join("speech")
}

fn python() -> PathBuf {
    dir().join("venv").join("bin").join("python")
}

fn marker() -> PathBuf {
    dir().join(format!("ready-{SETUP_VERSION}"))
}

/// MLX runs on Apple silicon only.
pub fn supported() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

/// Set up already, or can be (uv is installed).
pub fn available() -> bool {
    supported() && (is_set_up() || uv().is_some())
}

pub fn is_set_up() -> bool {
    marker().exists() && python().exists()
}

/// `uv`, looked for where installers put it: an app opened from the Finder
/// doesn't get the shell's PATH.
fn uv() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut places: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join("uv")).collect())
        .unwrap_or_default();
    places.extend([
        PathBuf::from("/opt/homebrew/bin/uv"),
        PathBuf::from("/usr/local/bin/uv"),
        home.join(".local/bin/uv"),
        home.join(".cargo/bin/uv"),
    ]);
    places.into_iter().find(|p| p.is_file())
}

/// One setup at a time (a video asks for several lines at once).
static SETUP: Mutex<()> = Mutex::new(());

/// Makes the Python environment if it isn't there. Blocking: about a
/// minute the first time, nothing after.
pub fn ensure_set_up() -> Result<()> {
    if !supported() {
        bail!("The free local voice needs a Mac with Apple silicon");
    }
    let _one = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    if is_set_up() {
        return Ok(());
    }
    let uv = uv().context(
        "The free natural voice needs uv to set itself up: install it (brew install uv), then try again",
    )?;
    std::fs::create_dir_all(dir())?;
    let log = dir().join("setup.log");
    let run = |args: &[&str]| -> Result<()> {
        let out = Command::new(&uv)
            .args(args)
            .current_dir(dir())
            .output()
            .context("couldn't run uv")?;
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        let _ = std::fs::write(&log, &text);
        if !out.status.success() {
            let last: Vec<&str> = text.lines().rev().take(3).collect();
            bail!(
                "setting up the local voice failed: {}",
                last.into_iter().rev().collect::<Vec<_>>().join(" ")
            );
        }
        Ok(())
    };
    if !python().exists() {
        run(&["venv", "--python", "3.12", "venv"])?;
    }
    let py = python().to_string_lossy().into_owned();
    let mut args = vec!["pip", "install", "--python", py.as_str()];
    args.extend(PACKAGES);
    run(&args)?;
    std::fs::write(marker(), model())?;
    Ok(())
}

struct Worker {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    model: String,
}

impl Worker {
    fn start(model: &str) -> Result<Worker> {
        let script = dir().join("speech_worker.py");
        if std::fs::read_to_string(&script).ok().as_deref() != Some(WORKER_SOURCE) {
            std::fs::write(&script, WORKER_SOURCE)?;
        }
        let log = std::fs::File::create(dir().join("worker.log"))?;
        let mut child = Command::new(python())
            .arg("-I")
            .arg(&script)
            .arg(model)
            .current_dir(dir())
            .env("HF_HOME", dir().join("hf"))
            .env("HF_HUB_DISABLE_PROGRESS_BARS", "1")
            .env("TOKENIZERS_PARALLELISM", "false")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .spawn()
            .context("couldn't start the local voice")?;
        let input = child.stdin.take().context("no worker input")?;
        let output = BufReader::new(child.stdout.take().context("no worker output")?);
        let mut w = Worker {
            child,
            input,
            output,
            model: model.to_string(),
        };
        // The first answer says the model loaded (it downloads the first time).
        let ready = w
            .read()
            .map_err(|e| anyhow!("the local voice didn't start: {e:#}"))?;
        if ready.get("ready").and_then(Value::as_bool) != Some(true) {
            let _ = w.child.kill();
            bail!(
                "the local voice didn't load: {}",
                ready
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
            );
        }
        Ok(w)
    }

    fn read(&mut self) -> Result<Value> {
        let mut line = String::new();
        loop {
            line.clear();
            if self.output.read_line(&mut line)? == 0 {
                let tail = std::fs::read_to_string(dir().join("worker.log")).unwrap_or_default();
                let tail: Vec<&str> = tail.lines().rev().take(2).collect();
                bail!("it stopped ({})", tail.join(" / "));
            }
            // Libraries sometimes print to stdout: only JSON lines are answers.
            if let Ok(v) = serde_json::from_str::<Value>(line.trim())
                && v.is_object()
            {
                return Ok(v);
            }
        }
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

static WORKER: Mutex<Option<Worker>> = Mutex::new(None);

/// What to say, and how.
pub struct Line<'a> {
    pub text: &'a str,
    pub voice: &'a str,
    pub speed: f32,
    /// A tone (Qwen models only; Kokoro has none).
    pub instruct: Option<&'a str>,
}

/// Speaks `line` into `out` (a .wav). Blocking: sets up and starts the
/// worker when needed.
pub fn speak(line: &Line, out: &Path) -> Result<()> {
    ensure_set_up()?;
    let model = model();
    let mut slot = WORKER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(w) = slot.as_mut()
        && (!w.alive() || w.model != model)
    {
        let _ = w.child.kill();
        *slot = None;
    }
    if slot.is_none() {
        *slot = Some(Worker::start(&model)?);
    }
    let w = slot.as_mut().expect("worker");
    let req = json!({
        "text": line.text,
        "voice": line.voice,
        "speed": line.speed,
        "instruct": line.instruct,
        "out": out.to_string_lossy(),
    });
    let answer = writeln!(w.input, "{req}")
        .map_err(anyhow::Error::from)
        .and_then(|_| w.input.flush().map_err(anyhow::Error::from))
        .and_then(|_| w.read());
    match answer {
        Ok(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => Ok(()),
        Ok(v) => bail!(
            "{}",
            v.get("error")
                .and_then(Value::as_str)
                .unwrap_or("the local voice failed")
        ),
        Err(e) => {
            // A dead worker is started again next time.
            let _ = w.child.kill();
            *slot = None;
            Err(e)
        }
    }
}
