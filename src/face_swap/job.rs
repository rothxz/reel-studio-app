//! The face swap's background work: finding the people in a clip, and
//! processing a clip's frames into a result. Both block: run them on
//! their own thread. They check `cancel` between frames.
//!
//! Processing reads the clip's frames on one thread and swaps on another,
//! with a few frames between them (a bounded queue: never the whole clip
//! in memory). Each frame's face is written to the result as it's done.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use super::cache::{self, ResultFile, ResultHeader, ResultWriter};
use super::config::FaceSwapSettings;
use super::error::{FaceSwapError, Result};
use super::models;
use super::pipeline::{self, FrameProcessor, Person};
use super::types::FrameView;
use crate::decoder::{DecodedFrame, FrameReader};
use crate::model::Micros;

/// Frames waiting between the reader and the swapper.
const QUEUE: usize = 4;
/// Frames sampled across a clip to find its people.
const PEOPLE_SAMPLES: usize = 8;
/// Frames are read this wide to find people.
const PEOPLE_WIDTH: u32 = 1280;

/// Process `media`'s source times `start..=end` with `settings` into the
/// result named `key`.
#[derive(Clone, Debug)]
pub struct FaceSwapJob {
    pub media: String,
    pub settings: FaceSwapSettings,
    pub start: Micros,
    pub end: Micros,
    pub key: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FaceSwapProgress {
    LoadingModels,
    FindingPerson,
    /// Frames done, and the share of the clip (0–1).
    Processing {
        frames: usize,
        fraction: f32,
    },
    Saving,
}

impl FaceSwapProgress {
    pub fn label(&self) -> &'static str {
        match self {
            FaceSwapProgress::LoadingModels => "Loading the face models…",
            FaceSwapProgress::FindingPerson => "Finding the person…",
            FaceSwapProgress::Processing { .. } => "Swapping faces…",
            FaceSwapProgress::Saving => "Saving…",
        }
    }

    pub fn fraction(&self) -> Option<f32> {
        match self {
            FaceSwapProgress::Processing { fraction, .. } => Some(*fraction),
            _ => None,
        }
    }
}

/// A finished job.
#[derive(Clone, Debug)]
pub struct FaceSwapResult {
    pub key: String,
    pub frames: usize,
    /// Frames the person was found (and swapped) on.
    pub swapped: usize,
    pub provider: &'static str,
    pub seconds: f32,
}

fn decode_err(e: anyhow::Error) -> FaceSwapError {
    FaceSwapError::Decode(format!("{e:#}"))
}

/// Runs `job`. `progress` hears how far it is.
pub fn process(
    job: &FaceSwapJob,
    cancel: &AtomicBool,
    progress: &dyn Fn(FaceSwapProgress),
) -> Result<FaceSwapResult> {
    let began = Instant::now();
    job.settings.ready()?;
    let cfg = job.settings.config();
    cfg.validate()?;
    let anchor = job
        .settings
        .target
        .clone()
        .ok_or(FaceSwapError::TargetNotFound)?;
    let check = || {
        if cancel.load(Ordering::Relaxed) {
            Err(FaceSwapError::Cancelled)
        } else {
            Ok(())
        }
    };

    progress(FaceSwapProgress::LoadingModels);
    let source = pipeline::with_models(cfg.gpu, |m| {
        pipeline::source_identity(m, &job.settings.source, &job.settings.source_hash)
    })?;
    check()?;

    // The person as picked, at the frame they were picked on.
    progress(FaceSwapProgress::FindingPerson);
    let mut reader =
        FrameReader::open(&job.media, "face-swap", cfg.decode_width).map_err(decode_err)?;
    let frame_duration = reader.frame_duration().max(1);
    let picked = reader
        .frame_at(anchor.time)
        .map_err(decode_err)?
        .ok_or(FaceSwapError::TargetNotFound)?;
    drop(reader);
    let (ref_w, ref_h) = (picked.width, picked.height);
    let (target, provider) = pipeline::with_models(cfg.gpu, |m| {
        let frame = FrameView::of(&picked);
        let face = pipeline::anchor_face(m, &frame, &anchor, 0.5)?;
        Ok((
            m.embedder.embed(&frame, &face.landmarks)?,
            m.provider.label(),
        ))
    })?;
    drop(picked);
    check()?;

    let mut proc = pipeline::with_models(cfg.gpu, |m| {
        Ok(FrameProcessor::new(m, cfg, &source, target, frame_duration))
    })?;
    let header = ResultHeader {
        key: job.key.clone(),
        media: job.media.clone(),
        ref_width: ref_w,
        ref_height: ref_h,
        crop_size: if cfg.restore {
            super::restorer::SIZE
        } else {
            super::swapper::INSWAPPER_SIZE
        },
        mask_pad: proc.mask().pad,
        mask_blur: proc.mask().blur,
        frame_duration,
        start: job.start,
        end: job.end,
        track_id: proc.track.id,
        models: models::versions(),
        provider: provider.to_string(),
    };
    let mut writer = ResultWriter::create(&cache::path_for(&job.key), &header)?;

    // Frames in order, read ahead on their own thread.
    let (tx, rx) = mpsc::sync_channel::<Arc<DecodedFrame>>(QUEUE);
    let stop = Arc::new(AtomicBool::new(false));
    let reading = {
        let (media, stop) = (job.media.clone(), stop.clone());
        let (start, end, width) = (job.start, job.end, cfg.decode_width);
        std::thread::Builder::new()
            .name("face swap read".into())
            .spawn(move || read_frames(&media, start, end, width, &stop, tx))
            .map_err(|e| FaceSwapError::Io(e.to_string()))?
    };

    let span = (job.end - job.start).max(1) as f32;
    let mut swapped = 0;
    let outcome = (|| {
        for f in rx.iter() {
            check()?;
            if (f.width, f.height) != (ref_w, ref_h) {
                return Err(FaceSwapError::Decode(
                    "the frame size changed mid-clip".into(),
                ));
            }
            let frame = FrameView::of(&f);
            match pipeline::with_models(cfg.gpu, |m| proc.process(m, &frame))? {
                Some(face) => {
                    writer.push(f.pts, Some((&face.m, &face.crop)))?;
                    swapped += 1;
                }
                None => writer.push(f.pts, None)?,
            }
            progress(FaceSwapProgress::Processing {
                frames: writer.frames(),
                fraction: ((f.pts - job.start) as f32 / span).clamp(0., 1.),
            });
        }
        Ok(())
    })();
    stop.store(true, Ordering::Relaxed);
    let read = reading
        .join()
        .unwrap_or_else(|_| Err(FaceSwapError::Decode("the reader crashed".into())));
    if let Err(e) = outcome.and(read).and_then(|()| check()) {
        writer.abandon();
        return Err(e);
    }
    let frames = writer.frames();
    if frames == 0 {
        writer.abandon();
        return Err(FaceSwapError::Decode(
            "no frames in that part of the clip".into(),
        ));
    }
    progress(FaceSwapProgress::Saving);
    writer.finish()?;
    Ok(FaceSwapResult {
        key: job.key.clone(),
        frames,
        swapped,
        provider,
        seconds: began.elapsed().as_secs_f32(),
    })
}

/// Sends every frame of `media` from `start` to `end` (source times) in
/// order, until `stop` or the receiver goes.
fn read_frames(
    media: &str,
    start: Micros,
    end: Micros,
    width: u32,
    stop: &AtomicBool,
    tx: mpsc::SyncSender<Arc<DecodedFrame>>,
) -> Result<()> {
    let mut reader = FrameReader::open(media, "face-swap", width).map_err(decode_err)?;
    let dur = reader.frame_duration().max(1);
    let mut t = start;
    let mut last: Option<Micros> = None;
    let mut stalls = 0;
    while !stop.load(Ordering::Relaxed) {
        let Some(f) = reader.frame_at(t).map_err(decode_err)? else {
            break;
        };
        if f.pts > end + dur / 2 {
            break;
        }
        if last == Some(f.pts) {
            // The file ended (its last frame stays), or a gap: step on.
            stalls += 1;
            if stalls > 2 {
                break;
            }
            t += dur;
            continue;
        }
        stalls = 0;
        last = Some(f.pts);
        t = f.pts + dur;
        if tx.send(f).is_err() {
            break;
        }
    }
    Ok(())
}

/// The people in `media` between source times `start` and `end`, from a
/// few frames spread across it.
pub fn find_people(
    media: &str,
    start: Micros,
    end: Micros,
    gpu: bool,
    cancel: &AtomicBool,
) -> Result<Vec<Person>> {
    let mut reader =
        FrameReader::open(media, "face-swap-people", PEOPLE_WIDTH).map_err(decode_err)?;
    let span = (end - start).max(0);
    let mut frames: Vec<(Micros, Arc<DecodedFrame>)> = vec![];
    for i in 0..PEOPLE_SAMPLES {
        if cancel.load(Ordering::Relaxed) {
            return Err(FaceSwapError::Cancelled);
        }
        let t = start + span * (2 * i as i64 + 1) / (2 * PEOPLE_SAMPLES as i64);
        if let Some(f) = reader.frame_at(t).map_err(decode_err)?
            && frames.last().is_none_or(|(_, l)| !Arc::ptr_eq(l, &f))
        {
            frames.push((f.pts, f));
        }
    }
    let views: Vec<(Micros, FrameView)> =
        frames.iter().map(|(t, f)| (*t, FrameView::of(f))).collect();
    pipeline::with_models(gpu, |m| pipeline::find_people(m, &views))
}

/// `frame` with the result's swapped face pasted in at `opacity` (0–1),
/// or None when the result has no face for that frame.
pub fn composite(frame: &DecodedFrame, result: &ResultFile, opacity: f32) -> Option<DecodedFrame> {
    let rec = result.face_at(frame.pts)?;
    let scale = frame.width as f32 / result.header.ref_width.max(1) as f32;
    let mut bgra = frame.bgra.clone();
    super::blend::composite(
        &mut bgra,
        frame.width,
        frame.height,
        &super::blend::Paste {
            m: rec.m.with_input_scale(scale),
            crop: &rec.crop,
            mask: result.header.mask(),
            opacity,
        },
    );
    Some(DecodedFrame {
        source: frame.source.clone(),
        key: frame.key.clone(),
        pts: frame.pts,
        width: frame.width,
        height: frame.height,
        bgra,
    })
}
