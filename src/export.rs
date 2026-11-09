//! Export: the timeline written to an MP4, off the UI thread.
//!
//! Frame by frame at the export's rate, the same way the player draws:
//! the engine says which clips show at that time (source times resolved),
//! each video / photo is decoded at that exact source time (its own
//! reader, frame-accurate rather than the preview's latest-wins), text is
//! drawn at the output's size, stickers loop their frames; then the GPU
//! composes the stack (looks, placement, Background, effect-lane clips) on
//! a canvas the output's size. The sound is the playback mix run offline
//! at 48 kHz. Written to `<file>.part` and renamed when it's whole; a
//! cancel or a failure leaves nothing behind.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};

use crate::audio::TimelineMix;
use crate::catalog;
use crate::decoder::{self, DecodedFrame, FrameReader};
use crate::effects::package::{self as fx_package, Packages};
use crate::effects::stack::{self as fx_stack, CanvasPasses, Needs, Placement, Sources};
use crate::effects::{self, Renderer};
use crate::encode::{AUDIO_RATE, Mp4Writer, PixelOrder, VideoOptions, bitrate_for};
use crate::engine::{self, Engine, LayerKind};
use crate::model::{Clip, Micros, Project, SECOND};
use crate::prefs::RESOLUTIONS;
use crate::stickers::{self, StickerFrames};
use crate::text_render::{self, RasterCache, TextStyle};

/// The frame rates offered; a project's own rate snaps to one of them.
pub const FRAME_RATES: [u32; 3] = [24, 30, 60];

/// Each resolution's bitrate base at 1080p30 (Mbps): low, high, maximum
/// quality, as the engine maps them.
const RESOLUTION_MBPS: [f64; 3] = [2.5, 8., 16.];

/// The longest side of the progress picture sent to the UI.
const PREVIEW_SIDE: u32 = 720;
/// Progress is sent at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);
/// The progress picture at most this often.
const PREVIEW_EVERY: Duration = Duration::from_millis(400);

/// What an export writes.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u64,
}

/// Whether a visible video clip shows a swapped face.
fn face_swapped(project: &Project) -> bool {
    project
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .any(|c| !c.hidden && c.kind == "video" && c.active_face_swap().is_some())
}

/// Visible clips whose face swap is on but not processed (the export
/// would show the original face): their ids.
pub fn unprocessed_face_swaps(project: &Project) -> Vec<String> {
    let results = crate::face_swap::cache::Results::default();
    project
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .filter(|c| !c.hidden && c.kind == "video" && c.active_face_swap().is_some())
        .filter(|c| results.for_clip(c).is_none())
        .map(|c| c.id.clone())
        .collect()
}

/// 24 and 60 stay, anything else is 30.
pub fn snap_fps(fps: f64) -> u32 {
    match fps.round() as u32 {
        f @ (24 | 60) => f,
        _ => 30,
    }
}

/// Rounds to an even number, up (4:2:0 needs even sizes).
fn to_even(n: f64) -> u32 {
    let r = n.round().max(2.) as u32;
    if r.is_multiple_of(2) { r } else { r + 1 }
}

impl Settings {
    /// The project's canvas at a header resolution (index into
    /// `RESOLUTIONS`) and frame rate (None: the project's own, snapped).
    pub fn new(project: &Project, resolution: usize, fps: Option<u32>, path: PathBuf) -> Self {
        let i = resolution.min(RESOLUTIONS.len() - 1);
        let scale = RESOLUTIONS[i].1 as f64;
        let width = to_even(project.canvas.width.max(2) as f64 * scale);
        let height = to_even(project.canvas.height.max(2) as f64 * scale);
        let fps = fps.unwrap_or_else(|| snap_fps(project.canvas.fps));
        Settings {
            path,
            width,
            height,
            fps,
            bitrate: bitrate_for(width, height, fps, RESOLUTION_MBPS[i]),
        }
    }
}

/// What the export reads besides the document: what the editor has
/// already loaded (the rest is fetched as it's needed).
pub struct Inputs {
    pub project: Project,
    /// Effect name → its package key (catalog rows the editor has).
    pub effect_keys: HashMap<String, String>,
    pub packages: Packages,
    /// Loaded stickers by URL.
    pub stickers: HashMap<String, Arc<StickerFrames>>,
}

/// What the export thread reports.
pub enum Update {
    Progress {
        done: u64,
        total: u64,
        /// The frame just written, small (width, height, BGRA).
        preview: Option<(u32, u32, Vec<u8>)>,
    },
    /// Written and in place at the settings' path.
    Finished,
    Failed(String),
    Cancelled,
}

/// A running export; dropping it doesn't stop it, `cancel` does.
pub struct Job {
    cancel: Arc<AtomicBool>,
}

impl Job {
    pub fn start(inputs: Inputs, settings: Settings, out: async_channel::Sender<Update>) -> Job {
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        std::thread::Builder::new()
            .name("export".into())
            .spawn(move || {
                let part = part_path(&settings.path);
                let result = write(inputs, &settings, &part, &stop, &out);
                let update = match result {
                    Ok(true) => match std::fs::rename(&part, &settings.path) {
                        Ok(()) => Update::Finished,
                        Err(e) => {
                            let _ = std::fs::remove_file(&part);
                            Update::Failed(format!("Couldn't save the video: {e}"))
                        }
                    },
                    Ok(false) => {
                        let _ = std::fs::remove_file(&part);
                        Update::Cancelled
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&part);
                        eprintln!("export failed: {e:#}");
                        Update::Failed(format!("{e:#}"))
                    }
                };
                let _ = out.send_blocking(update);
            })
            .expect("export thread starts");
        Job { cancel }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Where the file is written until it's whole: next to it.
fn part_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

/// The frames of a project at `fps` (the last one partly in).
pub fn frame_count(duration: Micros, fps: u32) -> u64 {
    let n = (duration.max(0) as u128 * fps as u128).div_ceil(SECOND as u128);
    n.max(1) as u64
}

/// Timeline time of frame `i` at `fps`.
fn frame_time(i: u64, fps: u32) -> Micros {
    (i as u128 * SECOND as u128 / fps as u128) as Micros
}

/// The effect rows and packages fetched as frames need them (blocking).
struct Looks {
    keys: HashMap<String, String>,
    packages: Packages,
    /// Rows asked for already (found or not).
    asked: HashSet<String>,
}

impl Looks {
    /// Fetches what a stack found missing; false when nothing new came of
    /// it (drawn without).
    fn fetch(&mut self, mut needs: Needs) -> bool {
        let mut any = false;
        let rows: Vec<String> = std::mem::take(&mut needs.rows)
            .into_iter()
            .filter(|r| self.asked.insert(r.clone()))
            .collect();
        if !rows.is_empty() {
            any = true;
            match catalog::fetch_named(&rows) {
                Ok(found) => {
                    for e in found.into_iter().filter(|e| !e.package_key.is_empty()) {
                        self.keys.insert(e.name, e.package_key);
                    }
                }
                Err(e) => eprintln!("export: effect rows: {e:#}"),
            }
        }
        for (key, kind) in needs.packages() {
            if self.packages.begin(&key) {
                any = true;
                let fetched = fx_package::fetch(key, kind);
                self.packages.end(fetched);
            }
        }
        any
    }
}

/// A frame scaled down to at most `PREVIEW_SIDE` (nearest pixel).
fn small(width: u32, height: u32, bgra: &[u8]) -> (u32, u32, Vec<u8>) {
    let k = (PREVIEW_SIDE as f32 / width.max(height) as f32).min(1.);
    let (w, h) = (
        ((width as f32 * k) as u32).max(1),
        ((height as f32 * k) as u32).max(1),
    );
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let sy = (y as u64 * height as u64 / h as u64) as usize;
        for x in 0..w {
            let sx = (x as u64 * width as u64 / w as u64) as usize;
            let i = (sy * width as usize + sx) * 4;
            out.extend_from_slice(&bgra[i..i + 4]);
        }
    }
    (w, h, out)
}

/// Writes the whole file to `part`: Ok(false) when cancelled.
fn write(
    inputs: Inputs,
    settings: &Settings,
    part: &Path,
    cancel: &AtomicBool,
    out: &async_channel::Sender<Update>,
) -> Result<bool> {
    let project = Arc::new(inputs.project);
    if project.duration <= 0 {
        bail!("There's nothing on the timeline to export");
    }
    if !unprocessed_face_swaps(&project).is_empty() {
        bail!("Process the face swap before exporting");
    }
    let mut engine = Engine::new();
    engine.load(&project).map_err(|e| anyhow!(e))?;
    let mut gpu = Renderer::new().context("Export needs a GPU, and none was found")?;
    let mut writer = Mp4Writer::create(
        part,
        &VideoOptions {
            width: settings.width,
            height: settings.height,
            fps: settings.fps,
            bitrate: settings.bitrate,
            input: PixelOrder::Bgra,
            audio: true,
            comment: face_swapped(&project).then(|| crate::face_swap::MANIPULATED_NOTE.to_string()),
        },
    )?;
    eprintln!(
        "export: {}x{} at {} fps, {:.1} Mbps ({}) → {}",
        settings.width,
        settings.height,
        settings.fps,
        settings.bitrate as f64 / 1e6,
        writer.codec_name,
        settings.path.display()
    );

    let fps = settings.fps;
    let total = frame_count(project.duration, fps);
    let out_px = (settings.width, settings.height);
    let canvas = (project.canvas.width.max(1), project.canvas.height.max(1));
    let out_scale = settings.width as f32 / canvas.0 as f32;
    let unit_px = text_render::unit_px(canvas.0, canvas.1);
    // Decoded big enough for the output's long side.
    let decode_width = settings.width.max(settings.height);
    let background = fx_stack::hex_rgba(&project.canvas.background_color);

    let mut looks = Looks {
        keys: inputs.effect_keys,
        packages: inputs.packages,
        asked: HashSet::new(),
    };
    let mut stickers: HashMap<String, Option<Arc<StickerFrames>>> = inputs
        .stickers
        .into_iter()
        .map(|(k, v)| (k, Some(v)))
        .collect();
    let mut readers: HashMap<String, FrameReader> = HashMap::new();
    // Face swaps: their processed results, pasted over the frames.
    let face_swaps = crate::face_swap::cache::Results::default();
    let mut texts = RasterCache::default();
    let mut backdrops: HashMap<String, Option<Arc<DecodedFrame>>> = HashMap::new();
    let mut mix = TimelineMix::new(project.clone(), 0, AUDIO_RATE);
    let mut sound = vec![];
    let mut sound_done: u64 = 0;
    let mut last_progress = Instant::now();
    let mut last_preview: Option<Instant> = None;

    for i in 0..total {
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let t = frame_time(i, fps);

        // Every picture showing at t, back to front, with its place.
        let layers = engine::picture_layers(&project, engine.active_layers(t));
        let mut frames: Vec<Arc<DecodedFrame>> = Vec::with_capacity(layers.len());
        let mut placed: Vec<(&Clip, Placement)> = Vec::with_capacity(layers.len());
        let mut keys_used: Vec<String> = vec![];
        for layer in layers {
            let Some((_, clip)) = project.find_clip(&layer.clip_id) else {
                continue;
            };
            let local = (t - clip.timeline_start).clamp(0, clip.timeline_duration.max(0));
            let prop = |id: &str, def: f64| {
                engine::clip_property_at(&engine, clip, id, local).unwrap_or(def) as f32
            };
            let mut place = fx_stack::place(clip, prop);
            let (frame, raster_px) = match layer.kind {
                LayerKind::Text => {
                    let raster_px = text_render::raster_px(prop("transform.scale", 1.), out_scale);
                    let style = TextStyle::from_clip(clip);
                    let Some((f, _)) = texts.get_or_draw(&style, canvas, unit_px * raster_px)
                    else {
                        continue; // nothing to write
                    };
                    (f, raster_px)
                }
                LayerKind::Sticker => {
                    let loaded = stickers.entry(layer.source.clone()).or_insert_with(|| {
                        stickers::load(&layer.source)
                            .map(Arc::new)
                            .map_err(|e| eprintln!("export: sticker {}: {e:#}", layer.source))
                            .ok()
                    });
                    let Some(frames) = loaded else { continue };
                    (frames.at(layer.offset), 1.)
                }
                _ => {
                    // Each layer of a file its own reader (two layers of
                    // one file don't make each other seek).
                    let turn = keys_used
                        .iter()
                        .filter(|k| k.rsplit_once('#').is_some_and(|(s, _)| s == layer.source))
                        .count();
                    let key = format!("{}#{turn}", layer.source);
                    keys_used.push(key.clone());
                    let reader = match readers.entry(key.clone()) {
                        std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                        std::collections::hash_map::Entry::Vacant(v) => v.insert(
                            FrameReader::open(&layer.source, &key, decode_width)
                                .with_context(|| format!("Can't read {}", layer.source))?,
                        ),
                    };
                    let Some(f) = reader
                        .frame_at(layer.source_time)
                        .with_context(|| format!("Can't decode {}", layer.source))?
                    else {
                        continue;
                    };
                    let f = match face_swaps.for_clip(clip) {
                        Some((r, opacity)) if layer.kind == LayerKind::Video => {
                            crate::face_swap::job::composite(&f, &r, opacity)
                                .map(Arc::new)
                                .unwrap_or(f)
                        }
                        _ => f,
                    };
                    (f, 1.)
                }
            };
            place.natural =
                fx_stack::natural_size(clip, (frame.width, frame.height), raster_px, canvas);
            frames.push(frame);
            placed.push((clip, place));
        }
        // Readers of clips gone from view are closed.
        readers.retain(|k, _| keys_used.contains(k));

        // Their looks (fetching rows and packages the first time).
        let mut stack = loop {
            let mut needs = Needs::default();
            let keys = &looks.keys;
            let effect_key = |name: &str| keys.get(name).cloned();
            let adjust = |c: &Clip| {
                let local = t - c.timeline_start;
                crate::adjust::matrix(c, |ctl| {
                    engine::clip_property_at(&engine, c, ctl.property, local).unwrap_or(0.)
                })
            };
            let src = Sources {
                packages: &looks.packages,
                effect_key: &effect_key,
                filter_matrix: &engine::filter_color_matrix,
                adjust_matrix: &adjust,
                live: None,
            };
            let stack = fx_stack::build(&project, &placed, t, out_px, &src, &mut needs);
            let missing = needs.any();
            if !missing || !looks.fetch(needs) {
                break stack;
            }
        };
        // Always the whole canvas at the output's size (the player
        // contain-fits a lone picture itself).
        let canvas_passes = stack.canvas.get_or_insert_with(|| CanvasPasses {
            width: out_px.0,
            height: out_px.1,
            background,
            backdrop: None,
            passes: vec![],
            passes_after: 0,
        });
        let backdrop = match &canvas_passes.backdrop {
            Some(fx_stack::Backdrop::Image(path)) => backdrops
                .entry(path.clone())
                .or_insert_with(|| {
                    decoder::thumbnail(path, 0, decode_width)
                        .map(Arc::new)
                        .map_err(|e| eprintln!("export: background {path}: {e:#}"))
                        .ok()
                })
                .clone(),
            _ => None,
        };

        let images: Vec<_> = frames.iter().map(|f| effects::image(f)).collect();
        let (w, h, bgra) = gpu.render(&images, backdrop.as_deref().map(effects::image), &stack);
        if (w, h) != out_px {
            bail!(
                "the GPU drew a {w}x{h} frame, not {}x{}",
                out_px.0,
                out_px.1
            );
        }
        writer.push(&bgra)?;

        // The sound up to the end of this frame.
        let upto = (i + 1) as u128 * AUDIO_RATE as u128 / fps as u128;
        let n = (upto as u64 - sound_done) as usize;
        sound.clear();
        sound.resize(n, [0f32; 2]);
        mix.mix(&mut sound);
        writer.push_audio(&sound)?;
        sound_done += n as u64;

        let last = i + 1 == total;
        if last || last_progress.elapsed() >= PROGRESS_EVERY {
            last_progress = Instant::now();
            let preview =
                (last || last_preview.is_none_or(|p| p.elapsed() >= PREVIEW_EVERY)).then(|| {
                    last_preview = Some(Instant::now());
                    small(w, h, &bgra)
                });
            let _ = out.send_blocking(Update::Progress {
                done: i + 1,
                total,
                preview,
            });
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(false);
    }
    writer.finish()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MediaItem;

    #[test]
    fn settings_follow_the_canvas_resolution_and_rate() {
        let mut p = Project::empty();
        p.canvas.width = 1080;
        p.canvas.height = 1920;
        p.canvas.fps = 29.97;
        let s = Settings::new(&p, 1, None, "/x.mp4".into());
        assert_eq!(
            (s.width, s.height, s.fps, s.bitrate),
            (1080, 1920, 30, 8_000_000)
        );
        // 720p of an odd shape rounds up to even sizes.
        p.canvas.width = 1001;
        p.canvas.height = 1001;
        let s = Settings::new(&p, 0, Some(60), "/x.mp4".into());
        assert_eq!((s.width, s.height, s.fps), (668, 668, 60));
        p.canvas.fps = 24.;
        assert_eq!(Settings::new(&p, 2, None, "/x.mp4".into()).fps, 24);
    }

    #[test]
    fn frames_cover_the_timeline() {
        assert_eq!(frame_count(SECOND, 30), 30);
        assert_eq!(frame_count(SECOND + 1, 30), 31);
        assert_eq!(frame_count(0, 30), 1);
        assert_eq!(frame_time(15, 30), SECOND / 2);
        assert_eq!(
            part_path(Path::new("/a/b c.mp4")),
            PathBuf::from("/a/b c.mp4.part")
        );
    }

    #[test]
    fn small_preview_keeps_the_shape() {
        let (w, h, px) = small(960, 540, &[7u8; 960 * 540 * 4]);
        assert_eq!((w, h), (720, 405));
        assert_eq!(px.len(), 720 * 405 * 4);
    }

    /// A whole export of a generated clip: a file of the right length with
    /// a picture and sound. Needs a GPU (skipped without one).
    #[test]
    fn exports_a_clip() {
        crate::decoder::init();
        if Renderer::new().is_none() {
            eprintln!("no GPU: skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("reel-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A 1 s source with sound, from the encoder itself.
        let src = dir.join("src.mp4");
        let mut w = Mp4Writer::create(
            &src,
            &VideoOptions {
                width: 64,
                height: 64,
                fps: 10,
                bitrate: 500_000,
                input: PixelOrder::Bgra,
                audio: true,
                comment: Some("test".into()),
            },
        )
        .unwrap();
        for _ in 0..10 {
            w.push(&[0, 0, 255, 255].repeat(64 * 64)).unwrap();
            w.push_audio(&vec![[0.25, 0.25]; 4800]).unwrap();
        }
        w.finish().unwrap();

        let item = MediaItem {
            path: src.to_string_lossy().into_owned(),
            name: "src.mp4".into(),
            duration: SECOND,
            width: 64,
            height: 64,
            has_video: true,
            still: false,
        };
        let mut p = Project::empty();
        p.canvas.width = 64;
        p.canvas.height = 64;
        p.canvas.fps = 10.;
        p.insert_media(&item, None);
        let out = dir.join("out.mp4");
        let settings = Settings {
            path: out.clone(),
            width: 64,
            height: 64,
            fps: 10,
            bitrate: 500_000,
        };
        let (tx, rx) = async_channel::unbounded();
        Job::start(
            Inputs {
                project: p,
                effect_keys: HashMap::new(),
                packages: Packages::default(),
                stickers: HashMap::new(),
            },
            settings,
            tx,
        );
        let mut done = 0;
        loop {
            match rx.recv_blocking().unwrap() {
                Update::Progress { done: d, total, .. } => {
                    assert_eq!(total, 10);
                    done = d;
                }
                Update::Finished => {
                    assert!(out.exists());
                    break;
                }
                Update::Failed(e) => panic!("{e}"),
                Update::Cancelled => panic!("cancelled"),
            }
        }
        assert_eq!(done, 10);
        assert!(!part_path(&out).exists());
        let input = ffmpeg_next::format::input(&out).unwrap();
        let secs = input.duration() as f64 / ffmpeg_next::ffi::AV_TIME_BASE as f64;
        assert!((secs - 1.).abs() < 0.15, "{secs}");
        assert!(
            input
                .streams()
                .best(ffmpeg_next::media::Type::Audio)
                .is_some()
        );
        // The picture is the source's red.
        let f = decoder::thumbnail(out.to_str().unwrap(), SECOND / 2, 64).unwrap();
        let mid = ((f.height / 2 * f.width + f.width / 2) * 4) as usize;
        let px = &f.bgra[mid..mid + 4];
        assert!(px[2] > 200 && px[0] < 60, "{px:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
