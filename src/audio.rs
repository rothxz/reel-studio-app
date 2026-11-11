//! Sound out: the timeline's audio played in step with the picture, and a
//! library preview (a track or sound effect listened to before it's
//! added).
//!
//! Three parts:
//! - the **output** (cpal) pulls stereo f32 from a ring of mixed samples
//!   and counts what it played (the audio clock);
//! - the **mixer** thread keeps the ring a little ahead: each audible clip
//!   (video or audio, not muted, on a lane that isn't muted) decoded by
//!   FFmpeg from its source time, resampled linearly to the device rate
//!   (a clip's speed changes its pitch, as the engine's export does),
//!   times its volume (0–1000 %) and fades, summed;
//! - commands from the editor: play from a time, stop, preview a file.
//!
//! The engine's clock drives the picture; the editor restarts the sound
//! at the engine's time on play, and again if the two drift apart.
//! Not yet: volume keyframes, reversed clips (the app plays a baked
//! reversed copy), freeze frames are silent.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ffmpeg_next as ff;

use crate::model::{Clip, Micros, Project, SECOND, TrackType};

/// The mixer keeps this much sound ready ahead of the speaker.
const AHEAD: Duration = Duration::from_millis(120);
/// Mixed this many frames at a time.
const BLOCK: usize = 512;
/// Clips starting this soon get their decoder opened and placed early.
const PREPARE: Micros = SECOND;
/// A stream more than this off where it should be is moved (seek).
const SEEK_SLACK: Micros = 60_000;
/// Longest fade a side can have (the app's cap).
pub const MAX_FADE: Micros = 10 * SECOND;
/// A clip's volume is a linear gain up to this (1000 %).
pub const MAX_VOLUME: f64 = 10.;

enum Cmd {
    Play { project: Arc<Project>, at: Micros },
    Stop,
    Preview { id: String, source: String },
    PreviewReady { id: String, stream: Result<Stream> },
    StopPreview,
}

struct Shared {
    /// Interleaved stereo, ready to play.
    ring: Mutex<VecDeque<f32>>,
    /// Frames the device has played since the last restart.
    played: AtomicU64,
    /// The timeline playing (else only a preview, or nothing).
    timeline: AtomicBool,
    /// The preview playing, by id (None: none or it ended).
    preview: Mutex<Option<String>>,
}

pub struct Audio {
    shared: Arc<Shared>,
    tx: Sender<Cmd>,
    rate: u32,
    /// Where the playing timeline started (timeline time).
    started: Option<Micros>,
}

impl Audio {
    /// Opens the default output device; None when there is none.
    pub fn new() -> Option<Audio> {
        let shared = Arc::new(Shared {
            ring: Mutex::new(VecDeque::new()),
            played: AtomicU64::new(0),
            timeline: AtomicBool::new(false),
            preview: Mutex::new(None),
        });
        // The stream lives on its own thread (it isn't Send everywhere).
        let (rate_tx, rate_rx) = channel::<Option<u32>>();
        let out = shared.clone();
        std::thread::Builder::new()
            .name("audio-out".into())
            .spawn(move || match open_output(out) {
                Ok((stream, rate)) => {
                    let _ = rate_tx.send(Some(rate));
                    let _keep = stream;
                    loop {
                        std::thread::park();
                    }
                }
                Err(e) => {
                    eprintln!("no sound output: {e:#}");
                    let _ = rate_tx.send(None);
                }
            })
            .ok()?;
        let rate = rate_rx.recv().ok()??;
        let (tx, rx) = channel();
        let mixer_shared = shared.clone();
        let back = tx.clone();
        std::thread::Builder::new()
            .name("audio-mix".into())
            .spawn(move || Mixer::new(mixer_shared, rate, back).run(rx))
            .ok()?;
        Some(Audio {
            shared,
            tx,
            rate,
            started: None,
        })
    }

    /// Plays the timeline of `project` from `at`.
    pub fn play(&mut self, project: Arc<Project>, at: Micros) {
        self.started = Some(at);
        let _ = self.tx.send(Cmd::Play { project, at });
    }

    pub fn stop(&mut self) {
        if self.started.take().is_some() {
            let _ = self.tx.send(Cmd::Stop);
        }
    }

    pub fn is_playing(&self) -> bool {
        self.started.is_some()
    }

    /// Where the sound is on the timeline (what the speaker has played).
    pub fn position(&self) -> Option<Micros> {
        let at = self.started?;
        let played = self.shared.played.load(Ordering::Relaxed);
        Some(at + (played as f64 * SECOND as f64 / self.rate as f64) as Micros)
    }

    /// Listens to a file or URL (`id` names it for `previewing`); a second
    /// call with the same id stops it.
    pub fn toggle_preview(&mut self, id: &str, source: &str) {
        if self.previewing().as_deref() == Some(id) {
            self.stop_preview();
            return;
        }
        *self.shared.preview.lock().unwrap() = Some(id.to_string());
        let _ = self.tx.send(Cmd::Preview {
            id: id.to_string(),
            source: source.to_string(),
        });
    }

    pub fn stop_preview(&mut self) {
        *self.shared.preview.lock().unwrap() = None;
        let _ = self.tx.send(Cmd::StopPreview);
    }

    /// The preview playing (None once it ends).
    pub fn previewing(&self) -> Option<String> {
        self.shared.preview.lock().unwrap().clone()
    }
}

fn open_output(shared: Arc<Shared>) -> Result<(cpal::Stream, u32)> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("no output device"))?;
    let supported = device.default_output_config()?;
    let format = supported.sample_format();
    let config = supported.config();
    let channels = config.channels as usize;
    let rate = config.sample_rate;
    let err = |e| eprintln!("sound output: {e}");
    let stream = match format {
        cpal::SampleFormat::F32 => device.build_output_stream(
            config,
            move |out: &mut [f32], _| fill(&shared, out, channels, |v| v),
            err,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_output_stream(
            config,
            move |out: &mut [i16], _| {
                fill(&shared, out, channels, |v| {
                    (v.clamp(-1., 1.) * 32767.) as i16
                })
            },
            err,
            None,
        )?,
        cpal::SampleFormat::U16 => device.build_output_stream(
            config,
            move |out: &mut [u16], _| {
                fill(&shared, out, channels, |v| {
                    ((v.clamp(-1., 1.) + 1.) * 32767.5) as u16
                })
            },
            err,
            None,
        )?,
        other => return Err(anyhow!("unsupported sample format {other:?}")),
    };
    stream.play()?;
    Ok((stream, rate))
}

/// The device's callback: stereo from the ring to its channels (silence
/// when the ring runs dry).
fn fill<T: Copy>(shared: &Shared, out: &mut [T], channels: usize, conv: impl Fn(f32) -> T) {
    let frames = out.len() / channels.max(1);
    let mut ring = shared.ring.lock().unwrap();
    let have = (ring.len() / 2).min(frames);
    for f in 0..frames {
        let (l, r) = if f < have {
            (
                ring.pop_front().unwrap_or(0.),
                ring.pop_front().unwrap_or(0.),
            )
        } else {
            (0., 0.)
        };
        let frame = &mut out[f * channels..(f + 1) * channels];
        for (c, s) in frame.iter_mut().enumerate() {
            *s = conv(match c {
                0 => l,
                1 => r,
                _ => 0.,
            });
        }
    }
    drop(ring);
    shared.played.fetch_add(have as u64, Ordering::Relaxed);
}

// ---- decoding ----------------------------------------------------------------

/// One file's sound, decoded on demand at its own rate as stereo f32.
pub struct Stream {
    input: ff::format::context::Input,
    index: usize,
    decoder: ff::decoder::Audio,
    rate: u32,
    time_base: f64,
    /// Decoded frames not yet used, from `front_time` on.
    buf: VecDeque<[f32; 2]>,
    /// Fractional read position into `buf`.
    frac: f64,
    /// Source time of `buf`'s first frame.
    front_time: Micros,
    /// Frames before this source time are dropped as they're decoded
    /// (after a seek lands early).
    skip_to: Option<Micros>,
    eof: bool,
}

impl Stream {
    pub fn open(path: &str) -> Result<Stream> {
        let input = ff::format::input(path)?;
        let stream = input
            .streams()
            .best(ff::media::Type::Audio)
            .ok_or_else(|| anyhow!("no sound"))?;
        let index = stream.index();
        let tb = stream.time_base();
        let ctx = ff::codec::Context::from_parameters(stream.parameters())?;
        let decoder = ctx.decoder().audio()?;
        let rate = decoder.rate().max(1);
        Ok(Stream {
            input,
            index,
            decoder,
            rate,
            time_base: tb.numerator() as f64 / tb.denominator().max(1) as f64,
            buf: VecDeque::new(),
            frac: 0.,
            front_time: 0,
            skip_to: None,
            eof: false,
        })
    }

    /// The source time the next read starts at.
    fn position(&self) -> Micros {
        self.front_time + (self.frac * SECOND as f64 / self.rate as f64) as Micros
    }

    fn seek(&mut self, t: Micros) {
        let t = t.max(0);
        let _ = self.input.seek(t, ..t);
        self.decoder.flush();
        self.buf.clear();
        self.frac = 0.;
        self.front_time = t;
        self.skip_to = Some(t);
        self.eof = false;
    }

    /// Decodes until there's more in `buf` (false at the end).
    fn decode_more(&mut self) -> bool {
        if self.eof {
            return false;
        }
        let mut frame = ff::frame::Audio::empty();
        loop {
            let Some((s, packet)) = self.input.packets().next() else {
                let _ = self.decoder.send_eof();
                let mut got = false;
                while self.decoder.receive_frame(&mut frame).is_ok() {
                    got |= self.push_frame(&frame);
                }
                self.eof = true;
                return got;
            };
            if s.index() != self.index || self.decoder.send_packet(&packet).is_err() {
                continue;
            }
            let mut got = false;
            while self.decoder.receive_frame(&mut frame).is_ok() {
                got |= self.push_frame(&frame);
            }
            if got {
                return true;
            }
        }
    }

    /// Appends a decoded frame (as stereo f32), dropping what lies before
    /// a seek's target.
    fn push_frame(&mut self, frame: &ff::frame::Audio) -> bool {
        let samples = to_stereo(frame);
        if samples.is_empty() {
            return false;
        }
        let mut skip = 0;
        if let Some(target) = self.skip_to {
            let start = frame
                .pts()
                .map(|p| (p as f64 * self.time_base * SECOND as f64) as Micros)
                .unwrap_or(target);
            let end = start + (samples.len() as f64 * SECOND as f64 / self.rate as f64) as Micros;
            if end <= target {
                return false;
            }
            skip = (((target - start).max(0)) as f64 * self.rate as f64 / SECOND as f64) as usize;
            self.skip_to = None;
            self.front_time = start.max(target);
            self.buf.clear();
            self.frac = 0.;
        }
        self.buf.extend(samples.into_iter().skip(skip));
        true
    }

    /// Fills `out` reading `step` source frames per output frame (linear
    /// interpolation); silence past the end.
    fn read(&mut self, out: &mut [[f32; 2]], step: f64) {
        for o in out.iter_mut() {
            let i = self.frac as usize;
            while self.buf.len() < i + 2 {
                if !self.decode_more() {
                    break;
                }
            }
            let (Some(a), b) = (self.buf.get(i), self.buf.get(i + 1)) else {
                *o = [0., 0.];
                self.frac += step;
                continue;
            };
            let b = b.unwrap_or(a);
            let f = (self.frac - i as f64) as f32;
            *o = [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
            self.frac += step;
        }
        let used = (self.frac.floor() as usize).min(self.buf.len());
        self.buf.drain(..used);
        self.frac -= used as f64;
        self.front_time += (used as f64 * SECOND as f64 / self.rate as f64) as Micros;
    }

    fn ended(&self) -> bool {
        self.eof && self.buf.len() <= self.frac as usize + 1
    }
}

/// The sound of `path` from `from` to `to` (source time) as mono f32 at
/// `rate` (speech recognition's input).
pub fn read_mono(path: &str, from: Micros, to: Micros, rate: u32) -> Result<Vec<f32>> {
    let mut s = Stream::open(path)?;
    s.seek(from);
    let n = ((to - from).max(0) as f64 * rate as f64 / SECOND as f64) as usize;
    let step = s.rate as f64 / rate as f64;
    let mut out = Vec::with_capacity(n);
    let mut chunk = vec![[0f32; 2]; 4096];
    while out.len() < n {
        let take = (n - out.len()).min(chunk.len());
        s.read(&mut chunk[..take], step);
        out.extend(chunk[..take].iter().map(|[l, r]| (l + r) * 0.5));
        if s.ended() {
            break;
        }
    }
    Ok(out)
}

/// All of `path`'s sound from its start (up to `max`) as stereo blocks at
/// `rate`, handed to `f` as they decode (memory stays flat for a long
/// song). Returns how much was read.
pub fn stream_stereo(
    path: &str,
    rate: u32,
    max: Micros,
    mut f: impl FnMut(&[[f32; 2]]),
) -> Result<Micros> {
    const BLOCK: usize = 1024;
    let mut s = Stream::open(path)?;
    let rate = rate.max(1);
    let step = s.rate as f64 / rate as f64;
    let limit = (max.max(0) as f64 * rate as f64 / SECOND as f64) as u64;
    let mut chunk = vec![[0f32; 2]; BLOCK];
    let mut frames: u64 = 0;
    while frames < limit && !s.ended() {
        // Past the end `read` pads with silence: a block at most.
        let take = (limit - frames).min(BLOCK as u64) as usize;
        s.read(&mut chunk[..take], step);
        f(&chunk[..take]);
        frames += take as u64;
    }
    Ok((frames as u128 * SECOND as u128 / rate as u128) as Micros)
}

/// A decoded frame as stereo f32 (mono doubled, more channels: the first
/// two).
fn to_stereo(frame: &ff::frame::Audio) -> Vec<[f32; 2]> {
    use ff::format::Sample;
    let samples = frame.samples();
    let channels = (frame.channels() as usize).max(1);
    let planar = frame.is_planar();
    let (size, read): (usize, fn(&[u8]) -> f32) = match frame.format() {
        Sample::U8(_) => (1, |b| (b[0] as f32 - 128.) / 128.),
        Sample::I16(_) => (2, |b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.),
        Sample::I32(_) => (4, |b| {
            i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.
        }),
        Sample::I64(_) => (8, |b| {
            i64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32
                / 9_223_372_036_854_775_808.
        }),
        Sample::F32(_) => (4, |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        Sample::F64(_) => (8, |b| {
            f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32
        }),
        _ => return vec![],
    };
    let planes: Vec<&[u8]> = (0..if planar { channels } else { 1 })
        .map(|p| frame.data(p))
        .collect();
    let get = |s: usize, c: usize| -> f32 {
        let (plane, i) = if planar {
            (c, s)
        } else {
            (0, s * channels + c)
        };
        planes[plane].get(i * size..(i + 1) * size).map_or(0., read)
    };
    (0..samples)
        .map(|s| {
            let l = get(s, 0);
            let r = if channels > 1 { get(s, 1) } else { l };
            [l, r]
        })
        .collect()
}

// ---- mixing ------------------------------------------------------------------

/// A clip's level `local` into it: volume × fades (linear ramps), 0 muted.
pub fn clip_gain(clip: &Clip, local: Micros) -> f32 {
    if clip.muted() {
        return 0.;
    }
    let mut g = clip.volume().clamp(0., MAX_VOLUME) as f32;
    let (fade_in, fade_out) = clip.audio_fade();
    if fade_in > 0 && local < fade_in {
        g *= local.max(0) as f32 / fade_in as f32;
    }
    let left = clip.timeline_duration - local;
    if fade_out > 0 && left < fade_out {
        g *= left.max(0) as f32 / fade_out as f32;
    }
    g
}

/// The clips that sound in the timeline (lane and clip not muted).
fn audible(project: &Project) -> Vec<&Clip> {
    project
        .tracks
        .iter()
        .filter(|t| {
            !t.muted
                && matches!(
                    t.kind,
                    TrackType::Video | TrackType::Overlay | TrackType::Audio
                )
        })
        .flat_map(|t| t.clips.iter())
        .filter(|c| {
            (c.kind == "video" || c.kind == "audio")
                && !c.hidden
                && !c.muted()
                && !c.freeze()
                && !c.reversed()
                && c.source.is_some()
        })
        .collect()
}

/// The timeline's sound from a time on, mixed block by block at a rate:
/// each audible clip decoded from its source time, resampled, times its
/// gain, summed. The playback mixer feeds the speaker from it; export
/// feeds the encoder.
pub struct TimelineMix {
    project: Arc<Project>,
    /// Where it started (timeline time) and the frames mixed since: the
    /// clock, counted in samples so it doesn't drift.
    start: Micros,
    mixed: u64,
    rate: u32,
    /// Open decoders by clip (None: the file has no sound).
    streams: HashMap<String, Option<Stream>>,
}

impl TimelineMix {
    pub fn new(project: Arc<Project>, at: Micros, rate: u32) -> Self {
        TimelineMix {
            project,
            start: at,
            mixed: 0,
            rate: rate.max(1),
            streams: HashMap::new(),
        }
    }

    /// Timeline time of the next frame mixed.
    fn clock(&self) -> Micros {
        self.start + (self.mixed as u128 * SECOND as u128 / self.rate as u128) as Micros
    }

    /// Adds the next `mix.len()` frames of the timeline into `mix`
    /// (unclamped).
    pub fn mix(&mut self, mix: &mut [[f32; 2]]) {
        let n = mix.len();
        let mut scratch = vec![[0f32; 2]; n];
        let rate = self.rate as f64;
        let t0 = self.clock();
        self.mixed += n as u64;
        let t1 = self.clock();
        let project = self.project.clone();
        if t0 >= project.duration {
            return; // past the end: quiet
        }
        let clips = audible(&project);
        for clip in &clips {
            let (start, end) = (clip.timeline_start, clip.end());
            // Opened (and placed) a little before it starts.
            if start >= t1 && start < t1 + PREPARE {
                if let Some(s) = self
                    .streams
                    .entry(clip.id.clone())
                    .or_insert_with(|| clip.source.as_deref().and_then(|p| Stream::open(p).ok()))
                    .as_mut()
                {
                    let want = clip.source_start.unwrap_or(0);
                    if (s.position() - want).abs() > SEEK_SLACK {
                        s.seek(want);
                    }
                }
                continue;
            }
            if end <= t0 || start >= t1 {
                continue;
            }
            let Some(stream) = self
                .streams
                .entry(clip.id.clone())
                .or_insert_with(|| clip.source.as_deref().and_then(|p| Stream::open(p).ok()))
                .as_mut()
            else {
                continue;
            };
            // The part of the block inside the clip.
            let from = ((start - t0).max(0) as f64 * rate / SECOND as f64).round() as usize;
            let to = (((end - t0) as f64 * rate / SECOND as f64).round() as usize).min(n);
            if from >= to {
                continue;
            }
            let speed = if clip.speed > 0. { clip.speed } else { 1. };
            let at = t0 + (from as f64 * SECOND as f64 / rate) as Micros;
            let local = at - start;
            let src = clip.source_start.unwrap_or(0) + (local as f64 * speed) as Micros;
            if (stream.position() - src).abs() > SEEK_SLACK {
                stream.seek(src);
            }
            let step = stream.rate as f64 * speed / rate;
            let part = &mut scratch[from..to];
            stream.read(part, step);
            for (i, s) in part.iter().enumerate() {
                let t_local = local + ((i as f64) * SECOND as f64 / rate) as Micros;
                let g = clip_gain(clip, t_local);
                mix[from + i][0] += s[0] * g;
                mix[from + i][1] += s[1] * g;
            }
        }
        // Decoders of clips well behind or ahead are closed.
        self.streams.retain(|id, _| {
            clips.iter().any(|c| {
                c.id == *id && c.end() > t0 - SECOND && c.timeline_start < t1 + 2 * PREPARE
            })
        });
    }
}

struct Mixer {
    shared: Arc<Shared>,
    rate: u32,
    back: Sender<Cmd>,
    timeline: Option<TimelineMix>,
    preview: Option<(String, Stream)>,
    /// The preview asked for last (an older one opening is dropped).
    preview_wanted: Option<String>,
}

impl Mixer {
    fn new(shared: Arc<Shared>, rate: u32, back: Sender<Cmd>) -> Self {
        Mixer {
            shared,
            rate,
            back,
            timeline: None,
            preview: None,
            preview_wanted: None,
        }
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        let ahead = (AHEAD.as_secs_f64() * self.rate as f64) as usize * 2;
        loop {
            // Commands first (waiting for one while there's nothing to play).
            let idle = self.timeline.is_none() && self.preview.is_none();
            let cmd = if idle {
                match rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                }
            } else {
                rx.try_recv().ok()
            };
            if let Some(cmd) = cmd {
                self.command(cmd);
                continue;
            }
            let queued = self.shared.ring.lock().unwrap().len();
            if queued >= ahead {
                std::thread::sleep(Duration::from_millis(4));
                continue;
            }
            let block = self.mix_block();
            self.shared.ring.lock().unwrap().extend(block);
        }
    }

    fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Play { project, at } => {
                // Keep the open decoders of clips still in the document.
                let mut streams = self.timeline.take().map(|t| t.streams).unwrap_or_default();
                streams.retain(|id, _| project.find_clip(id).is_some());
                let mut timeline = TimelineMix::new(project, at, self.rate);
                timeline.streams = streams;
                self.timeline = Some(timeline);
                self.shared.timeline.store(true, Ordering::Relaxed);
                self.restart_output();
            }
            Cmd::Stop => {
                self.timeline = None;
                self.shared.timeline.store(false, Ordering::Relaxed);
                self.restart_output();
            }
            Cmd::Preview { id, source } => {
                self.preview = None;
                self.preview_wanted = Some(id.clone());
                // Opening (maybe over the network) mustn't stall the mix.
                let back = self.back.clone();
                std::thread::Builder::new()
                    .name("audio-preview".into())
                    .spawn(move || {
                        let stream = Stream::open(&source);
                        let _ = back.send(Cmd::PreviewReady { id, stream });
                    })
                    .ok();
            }
            Cmd::PreviewReady { id, stream } => {
                if self.preview_wanted.as_deref() != Some(&id) {
                    return;
                }
                match stream {
                    Ok(s) => {
                        self.preview = Some((id, s));
                        if self.timeline.is_none() {
                            self.restart_output();
                        }
                    }
                    Err(e) => {
                        eprintln!("preview {id}: {e:#}");
                        self.end_preview();
                    }
                }
            }
            Cmd::StopPreview => {
                self.preview = None;
                self.preview_wanted = None;
                if self.timeline.is_none() {
                    self.restart_output();
                }
            }
        }
    }

    fn end_preview(&mut self) {
        self.preview = None;
        let mut p = self.shared.preview.lock().unwrap();
        if *p == self.preview_wanted {
            *p = None;
        }
        self.preview_wanted = None;
    }

    /// Drops what's queued and restarts the audio clock.
    fn restart_output(&self) {
        let mut ring = self.shared.ring.lock().unwrap();
        ring.clear();
        self.shared.played.store(0, Ordering::Relaxed);
    }

    fn mix_block(&mut self) -> Vec<f32> {
        let mut mix = vec![[0f32; 2]; BLOCK];
        let mut scratch = vec![[0f32; 2]; BLOCK];
        let rate = self.rate as f64;
        if let Some(tl) = &mut self.timeline {
            tl.mix(&mut mix);
        }
        let mut ended = false;
        if let Some((_, stream)) = &mut self.preview {
            let step = stream.rate as f64 / rate;
            stream.read(&mut scratch, step);
            for (m, s) in mix.iter_mut().zip(&scratch) {
                m[0] += s[0];
                m[1] += s[1];
            }
            ended = stream.ended();
        }
        if ended {
            self.end_preview();
        }
        mix.into_iter()
            .flat_map(|[l, r]| [l.clamp(-1., 1.), r.clamp(-1., 1.)])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_follows_volume_and_fades() {
        let mut p = Project::empty();
        let id = p.add_audio_clip_fitted(0, "/x.mp3", "x", 10 * SECOND);
        p.update_clip(&id, |c| {
            c.extra.insert("volume".into(), 2.into());
            c.extra.insert(
                "audioFade".into(),
                serde_json::json!({ "inDurationMicros": 2_000_000, "outDurationMicros": 1_000_000 }),
            );
            true
        });
        let c = p.find_clip(&id).unwrap().1;
        assert_eq!(clip_gain(c, 0), 0.);
        assert!((clip_gain(c, SECOND) - 1.).abs() < 1e-6);
        assert!((clip_gain(c, 5 * SECOND) - 2.).abs() < 1e-6);
        assert!((clip_gain(c, 9 * SECOND + SECOND / 2) - 1.).abs() < 1e-6);
    }

    /// Decodes a generated tone and reads it back resampled.
    #[test]
    fn decodes_and_resamples_a_file() {
        let dir = std::env::temp_dir().join(format!("reel-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        // 1 s of 440 Hz at 8 kHz, 16-bit mono.
        let rate = 8000u32;
        let samples: Vec<i16> = (0..rate)
            .map(|i| {
                ((i as f32 / rate as f32 * 440. * std::f32::consts::TAU).sin() * 16000.) as i16
            })
            .collect();
        let mut wav = Vec::new();
        let data_len = samples.len() as u32 * 2;
        wav.extend(b"RIFF");
        wav.extend((36 + data_len).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(rate.to_le_bytes());
        wav.extend((rate * 2).to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(data_len.to_le_bytes());
        for s in &samples {
            wav.extend(s.to_le_bytes());
        }
        std::fs::write(&path, wav).unwrap();
        crate::decoder::init();
        let mut s = Stream::open(path.to_str().unwrap()).unwrap();
        assert_eq!(s.rate, 8000);
        // To 48 kHz: 6 output frames per source frame.
        let mut out = vec![[0f32; 2]; 4800];
        s.read(&mut out, 8000. / 48000.);
        let loud = out.iter().map(|f| f[0].abs()).fold(0., f32::max);
        assert!(loud > 0.4, "{loud}");
        assert_eq!(out[100][0], out[100][1], "mono is doubled");
        // 0.1 s read: the stream is about 0.1 s in.
        assert!(
            (s.position() - SECOND / 10).abs() < 2_000,
            "{}",
            s.position()
        );
        s.seek(SECOND / 2);
        s.read(&mut out[..10], 1.);
        assert!(
            (s.position() - SECOND / 2).abs() < 5_000,
            "{}",
            s.position()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
