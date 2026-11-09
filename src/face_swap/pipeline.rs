//! The models, loaded once per run of the app, and what's done with them:
//! a source photo's identity, the people in a clip, and one frame's swap.
//!
//! A frame: detect faces → follow the chosen person (identity checks,
//! smoothed landmarks) → align their face to the swapper's crop (at
//! `boost`× its size: High swaps in interleaved tiles for detail) → swap →
//! match the frame's colour (steadied over frames) → (restore) → the crop
//! and where it goes. Nothing outside the face is touched.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use image::RgbImage;

use super::align::{Similarity, ffhq_template, sample_bgra, sample_rgb, warp_crop};
use super::blend::{MaskShape, color_offset, shift_colors};
use super::config::{FaceSwapConfig, TargetAnchor};
use super::detector::Detector;
use super::embedder::Embedder;
use super::error::{FaceSwapError, Result};
use super::inference::Provider;
use super::models::{self, ModelSpec};
use super::restorer::{self, Restorer};
use super::swapper::{FaceSwapper, Inswapper, SourceLatent, swap_boosted};
use super::tracker::{FaceTrack, TrackStep};
use super::types::{DetectedFace, Embedding, FaceBox, FrameView};
use crate::model::Micros;

pub struct Models {
    pub detector: Detector,
    pub embedder: Embedder,
    pub swapper: Box<dyn FaceSwapper>,
    pub restorer: Option<Restorer>,
    /// Where the swapper runs (the heaviest model).
    pub provider: Provider,
    pub gpu: bool,
}

fn need(spec: &ModelSpec) -> Result<std::path::PathBuf> {
    if spec.present() {
        Ok(spec.path())
    } else {
        Err(FaceSwapError::ModelMissing(spec.id))
    }
}

impl Models {
    pub fn load(gpu: bool) -> Result<Models> {
        let cache = models::compile_cache();
        let cache = Some(cache.as_path());
        // The detector is tiny and runs every frame: the CPU is quicker
        // than a round trip to the GPU for it.
        let detector = Detector::load(&need(&models::DETECTOR)?, false, cache)?;
        let embedder = Embedder::load(&need(&models::EMBEDDER)?, gpu, cache)?;
        let swapper = Inswapper::load(&need(&models::SWAPPER)?, gpu, cache)?;
        Ok(Models {
            detector,
            embedder,
            provider: swapper_provider(&swapper),
            swapper: Box::new(swapper),
            restorer: None,
            gpu,
        })
    }

    fn ensure_restorer(&mut self) -> Result<&mut Restorer> {
        if self.restorer.is_none() {
            let cache = models::compile_cache();
            self.restorer = Some(Restorer::load(
                &need(&models::RESTORER)?,
                self.gpu,
                Some(&cache),
            )?);
        }
        Ok(self.restorer.as_mut().unwrap())
    }
}

fn swapper_provider(s: &Inswapper) -> Provider {
    s.provider()
}

static SHARED: Mutex<Option<Models>> = Mutex::new(None);

/// Runs `f` with the shared models, loading them the first time. Held
/// only for the call: a long job takes them frame by frame, so the panel's
/// face search can go between.
pub fn with_models<T>(gpu: bool, f: impl FnOnce(&mut Models) -> Result<T>) -> Result<T> {
    let mut guard = SHARED
        .lock()
        .map_err(|_| FaceSwapError::Inference("the face models crashed earlier".into()))?;
    if guard.as_ref().is_none_or(|m| m.gpu != gpu) {
        *guard = Some(Models::load(gpu)?);
    }
    f(guard.as_mut().unwrap())
}

/// Drops the loaded models (their files are being deleted).
pub fn unload() {
    if let Ok(mut g) = SHARED.lock() {
        *g = None;
    }
}

/// A photo as a BGRA frame.
pub fn load_photo(path: &str) -> Result<(u32, u32, Vec<u8>)> {
    let img = image::open(path)
        .map_err(|e| FaceSwapError::Io(format!("Can't open the photo: {e}")))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    let mut bgra = img.into_raw();
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    Ok((w, h, bgra))
}

/// Source identities by photo hash (a photo is embedded once per run).
fn source_cache() -> &'static Mutex<HashMap<String, Embedding>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Embedding>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The face in a photo: the biggest one the detector is sure of.
pub fn photo_face(m: &mut Models, frame: &FrameView) -> Result<DetectedFace> {
    m.detector
        .detect(frame, 0.5)?
        .into_iter()
        .max_by(|a, b| a.bbox.area().total_cmp(&b.bbox.area()))
        .ok_or(FaceSwapError::NoSourceFace)
}

/// The identity of the face in the photo at `path` (hash `hash`).
pub fn source_identity(m: &mut Models, path: &str, hash: &str) -> Result<Embedding> {
    if let Some(e) = source_cache()
        .lock()
        .ok()
        .and_then(|c| c.get(hash).cloned())
    {
        return Ok(e);
    }
    let (width, height, bgra) = load_photo(path)?;
    let frame = FrameView {
        width,
        height,
        bgra: &bgra,
    };
    let face = photo_face(m, &frame)?;
    let e = m.embedder.embed(&frame, &face.landmarks)?;
    if let Ok(mut c) = source_cache().lock() {
        c.insert(hash.to_string(), e.clone());
    }
    Ok(e)
}

/// The face at `anchor` in `frame` (the frame at the anchor's time): the
/// detection overlapping the stored box most.
pub fn anchor_face(
    m: &mut Models,
    frame: &FrameView,
    anchor: &TargetAnchor,
    threshold: f32,
) -> Result<DetectedFace> {
    let want = FaceBox::from_normalized(anchor.bbox, frame.width, frame.height);
    m.detector
        .detect(frame, threshold)?
        .into_iter()
        .map(|f| (f.bbox.iou(&want), f))
        .filter(|(iou, _)| *iou >= 0.2)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, f)| f)
        .ok_or(FaceSwapError::TargetNotFound)
}

/// A person seen in the clip, for the panel to offer.
#[derive(Clone)]
pub struct Person {
    /// A square picture of their face (RGB).
    pub thumb: RgbImage,
    pub anchor: TargetAnchor,
    /// Sample frames they were on.
    pub sightings: usize,
}

/// Two sightings this alike are the same person.
const SAME_PERSON: f32 = 0.4;

/// The people on `frames` (source time, frame), most seen first: every
/// face found is grouped with the person it looks most like.
pub fn find_people(m: &mut Models, frames: &[(Micros, FrameView)]) -> Result<Vec<Person>> {
    struct Group {
        identity: Embedding,
        best: (f32, Micros, DetectedFace, usize),
        frames: Vec<Micros>,
    }
    let mut groups: Vec<Group> = vec![];
    for (fi, (t, frame)) in frames.iter().enumerate() {
        for face in m.detector.detect(frame, 0.75)? {
            let e = m.embedder.embed(frame, &face.landmarks)?;
            let quality = face.score * face.bbox.area().sqrt();
            let near = groups
                .iter_mut()
                .map(|g| (g.identity.similarity(&e), g))
                .filter(|(s, _)| *s >= SAME_PERSON)
                .max_by(|a, b| a.0.total_cmp(&b.0));
            match near {
                Some((_, g)) => {
                    g.identity = g.identity.blend(&e, 1. / (g.frames.len() as f32 + 1.));
                    if !g.frames.contains(t) {
                        g.frames.push(*t);
                    }
                    if quality > g.best.0 {
                        g.best = (quality, *t, face, fi);
                    }
                }
                None => groups.push(Group {
                    identity: e,
                    best: (quality, *t, face, fi),
                    frames: vec![*t],
                }),
            }
        }
    }
    let mut people: Vec<Person> = groups
        .into_iter()
        .map(|g| {
            let (_, t, face, fi) = g.best;
            let frame = &frames[fi].1;
            Person {
                thumb: face_thumb(frame, &face.bbox, 96),
                anchor: TargetAnchor {
                    time: t,
                    bbox: face.bbox.normalized(frame.width, frame.height),
                },
                sightings: g.frames.len(),
            }
        })
        .collect();
    people.sort_by(|a, b| {
        b.sightings
            .cmp(&a.sightings)
            .then(b.anchor.bbox[2].total_cmp(&a.anchor.bbox[2]))
    });
    Ok(people)
}

/// A `size` px square around a face box (a little loose), RGB.
pub fn face_thumb(frame: &FrameView, b: &FaceBox, size: u32) -> RgbImage {
    let [cx, cy] = b.center();
    let side = b.w.max(b.h) * 1.5;
    let s = size as f32 / side;
    let m = Similarity {
        a: s,
        b: 0.,
        tx: size as f32 / 2. - cx * s,
        ty: size as f32 / 2. - cy * s,
    };
    warp_crop(frame, &m, size)
}

/// One frame's swapped face: where it goes (frame → crop) and the crop.
pub struct SwappedFace {
    pub m: Similarity,
    pub crop: RgbImage,
}

/// Swaps the person's face frame after frame.
pub struct FrameProcessor {
    cfg: FaceSwapConfig,
    latent: SourceLatent,
    pub track: FaceTrack,
    /// The swapper's own crop side, px (the swap runs at `boost`× it).
    swap_size: u32,
    /// The colour match, steadied over frames (None: start again).
    color: Option<[f32; 3]>,
    dt: f32,
}

/// How far a frame's colour match moves toward its own measurement.
const COLOR_FOLLOW: f32 = 0.25;
/// The colour match's limit (0–255 per channel).
const COLOR_LIMIT: f32 = 24.;
/// How much of the restored face shows over the plain swap.
const RESTORE_MIX: f32 = 0.8;

impl FrameProcessor {
    pub fn new(
        m: &mut Models,
        cfg: FaceSwapConfig,
        source: &Embedding,
        target: Embedding,
        frame_duration: Micros,
    ) -> FrameProcessor {
        FrameProcessor {
            cfg,
            latent: m.swapper.source_latent(source),
            track: FaceTrack::looking_for(1, cfg.tracker, target),
            swap_size: m.swapper.crop_size(),
            color: None,
            dt: (frame_duration as f32 / 1e6).max(1e-3),
        }
    }

    /// The person's swapped face on `frame`, or None when they aren't on it.
    pub fn process(&mut self, m: &mut Models, frame: &FrameView) -> Result<Option<SwappedFace>> {
        let faces = m.detector.detect(frame, self.cfg.detect_threshold)?;
        let embedder = &mut m.embedder;
        let step = self.track.step(&faces, self.dt, &mut |f| {
            embedder.embed(frame, &f.landmarks)
        })?;
        let TrackStep::Found { landmarks, .. } = step else {
            self.color = None;
            return Ok(None);
        };
        let k = self.cfg.boost;
        let size = m.swapper.crop_size() * k;
        let template = m
            .swapper
            .template()
            .map(|[x, y]| [x * k as f32, y * k as f32]);
        let align = Similarity::align(&landmarks, &template);
        let target = warp_crop(frame, &align, size);
        let mut swapped = swap_boosted(m.swapper.as_mut(), &target, &self.latent, k)?;
        let measured = color_offset(&swapped, &target, &MaskShape::SWAP, COLOR_LIMIT);
        let offset = match self.color {
            Some(c) => std::array::from_fn(|i| c[i] + (measured[i] - c[i]) * COLOR_FOLLOW),
            None => measured,
        };
        self.color = Some(offset);
        shift_colors(&mut swapped, offset);
        if !self.cfg.restore {
            return Ok(Some(SwappedFace {
                m: align,
                crop: swapped,
            }));
        }
        // Restoration sees the whole head with the swapped face in it.
        let head = Similarity::align(&landmarks, &ffhq_template(restorer::SIZE));
        let before = paste_into_crop(frame, &head, restorer::SIZE, &align, &swapped);
        let restored = m.ensure_restorer()?.restore(&before)?;
        let mut mixed = before;
        for (o, r) in mixed.pixels_mut().zip(restored.pixels()) {
            for c in 0..3 {
                o.0[c] = (o.0[c] as f32 * (1. - RESTORE_MIX) + r.0[c] as f32 * RESTORE_MIX).round()
                    as u8;
            }
        }
        Ok(Some(SwappedFace {
            m: head,
            crop: mixed,
        }))
    }

    /// The mask the crops of this processor are pasted through: the face
    /// mask for a swap; a restored crop is the whole head, its edge kept
    /// further in.
    pub fn mask(&self) -> MaskShape {
        if self.cfg.restore {
            MaskShape::SWAP
        } else {
            MaskShape::FACE
        }
    }

    /// The side of the crops this processor makes, px.
    pub fn crop_size(&self) -> u32 {
        if self.cfg.restore {
            restorer::SIZE
        } else {
            self.swap_size * self.cfg.boost
        }
    }
}

/// The `size` px crop `head` (frame → crop) makes of the frame with
/// `swapped` (frame → crop by `face`) pasted in through its mask.
fn paste_into_crop(
    frame: &FrameView,
    head: &Similarity,
    size: u32,
    face: &Similarity,
    swapped: &RgbImage,
) -> RgbImage {
    let inv = head.inverse();
    let fs = swapped.width() as f32;
    let mut out = RgbImage::new(size, size);
    for (x, y, px) in out.enumerate_pixels_mut() {
        let p = inv.apply([x as f32, y as f32]);
        let mut c = sample_bgra(frame, p[0], p[1]);
        let q = face.apply(p);
        if q[0] >= 0. && q[1] >= 0. && q[0] <= fs - 1. && q[1] <= fs - 1. {
            let a = MaskShape::FACE.at(q, fs);
            if a > 0. {
                let s = sample_rgb(swapped, q[0], q[1]);
                for i in 0..3 {
                    c[i] += (s[i] - c[i]) * a;
                }
            }
        }
        px.0 = c.map(|v| v.round().clamp(0., 255.) as u8);
    }
    out
}
