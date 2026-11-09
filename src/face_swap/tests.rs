//! The pipeline end to end on the real models (ignored: they need the
//! downloaded models and test media, see below). Scores are printed so a
//! change in quality shows, and checked against loose floors.
//!
//! ```sh
//! REEL_FACE_MODELS=<folder with the four .onnx files> \
//! REEL_FACE_MEDIA=<folder: src_a.jpg src_b.jpg one.mp4 two.mp4 gap.mp4> \
//! REEL_FACE_OUT=<folder for the pictures to look at (optional)> \
//! cargo test --release face_swap::tests -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Media: two portraits of different people (the source faces); `one.mp4`
//! one person talking (a few seconds); `two.mp4` two people side by side,
//! the one on the right still; `gap.mp4` like one.mp4 with the face
//! covered from 2 s to 3 s. Use footage you may use (public domain); keep
//! it out of the repository.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use super::cache::{self, ResultFile};
use super::config::{FaceSwapSettings, Quality, TargetAnchor, photo_hash};
use super::job::{self, FaceSwapJob, FaceSwapProgress};
use super::pipeline::{self, FrameProcessor};
use super::types::{Embedding, FaceBox, FrameView};
use crate::decoder::{self, DecodedFrame, FrameReader};
use crate::model::{Micros, SECOND};

fn media(name: &str) -> String {
    let dir = std::env::var("REEL_FACE_MEDIA").expect("REEL_FACE_MEDIA");
    PathBuf::from(dir).join(name).to_string_lossy().into_owned()
}

fn out(name: &str) -> Option<PathBuf> {
    std::env::var_os("REEL_FACE_OUT").map(|d| {
        let d = PathBuf::from(d);
        let _ = std::fs::create_dir_all(&d);
        d.join(name)
    })
}

fn setup() {
    decoder::init();
    if std::env::var_os("REEL_FACE_CACHE").is_none() {
        let d = std::env::temp_dir().join(format!("face-swap-test-{}", std::process::id()));
        // SAFETY: tests run with --test-threads=1 (documented above).
        unsafe { std::env::set_var("REEL_FACE_CACHE", d) };
    }
}

fn save(f: &DecodedFrame, name: &str) {
    if let Some(path) = out(name) {
        let mut rgba = f.bgra.clone();
        for p in rgba.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
        }
        image::RgbaImage::from_raw(f.width, f.height, rgba)
            .unwrap()
            .save(path)
            .unwrap();
    }
}

fn frame_at(path: &str, t: Micros, width: u32) -> Arc<DecodedFrame> {
    FrameReader::open(path, "t", width)
        .unwrap()
        .frame_at(t)
        .unwrap()
        .unwrap()
}

fn settings(source: &str, anchor: TargetAnchor) -> FaceSwapSettings {
    FaceSwapSettings {
        source: source.to_string(),
        source_hash: photo_hash(source).unwrap(),
        target: Some(anchor),
        consent: true,
        ..Default::default()
    }
}

/// The identity of the biggest face in `f`, and its box.
fn identity_of(f: &DecodedFrame) -> Option<(Embedding, FaceBox, super::types::FaceLandmarks)> {
    pipeline::with_models(true, |m| {
        let v = FrameView::of(f);
        let face = m
            .detector
            .detect(&v, 0.5)?
            .into_iter()
            .max_by(|a, b| a.bbox.area().total_cmp(&b.bbox.area()));
        Ok(match face {
            Some(face) => Some((
                m.embedder.embed(&v, &face.landmarks)?,
                face.bbox,
                face.landmarks,
            )),
            None => None,
        })
    })
    .unwrap()
}

fn source_identity(path: &str) -> Embedding {
    let hash = photo_hash(path).unwrap();
    pipeline::with_models(true, |m| pipeline::source_identity(m, path, &hash)).unwrap()
}

/// Processes `media` from 0 to `end` with `s`; the result file.
fn process(media: &str, end: Micros, s: FaceSwapSettings) -> (super::FaceSwapResult, ResultFile) {
    let job = FaceSwapJob {
        media: media.to_string(),
        key: s.cache_key(media),
        settings: s,
        start: 0,
        end,
    };
    let r = job::process(&job, &AtomicBool::new(false), &|_| {}).unwrap();
    println!(
        "  {} frames, {} swapped, {:.1} s on {} ({:.0} ms / frame)",
        r.frames,
        r.swapped,
        r.seconds,
        r.provider,
        r.seconds * 1000. / r.frames.max(1) as f32
    );
    let file = ResultFile::open(&cache::path_for(&r.key)).unwrap();
    (r, file)
}

#[test]
#[ignore]
fn detector_finds_five_landmarks_in_order() {
    setup();
    let f = frame_at(&media("one.mp4"), 2 * SECOND, 1280);
    // At the people search's threshold (lower, a face on the poster
    // behind her shows up too; tracking checks identity, so it's never
    // swapped).
    let faces =
        pipeline::with_models(true, |m| m.detector.detect(&FrameView::of(&f), 0.75)).unwrap();
    assert_eq!(faces.len(), 1, "one person: {faces:?}");
    let [le, re, nose, ml, mr] = faces[0].landmarks.0;
    println!("  {faces:?}");
    assert!(
        le[0] < nose[0] && nose[0] < re[0],
        "eyes either side of the nose"
    );
    assert!(ml[0] < mr[0], "mouth corners left to right");
    assert!(
        le[1] < nose[1] && nose[1] < ml[1],
        "eyes, nose, mouth top to bottom"
    );
    assert!(faces[0].bbox.w > 100. && faces[0].score > 0.8);
}

/// A still: the person in the frame gets the photo's identity, and the
/// rest of the frame isn't touched.
#[test]
#[ignore]
fn image_swap_takes_the_source_identity() {
    setup();
    let src = media("src_a.jpg");
    let source = source_identity(&src);
    let target = frame_at(&media("one.mp4"), 2 * SECOND, 1280);
    let (before, bbox, _) = identity_of(&target).unwrap();
    let swapped = pipeline::with_models(true, |m| {
        let mut p = FrameProcessor::new(
            m,
            super::config::FaceSwapConfig::new(Quality::Fast, false),
            &source,
            before.clone(),
            33_333,
        );
        p.process(m, &FrameView::of(&target))
    })
    .unwrap()
    .expect("the person is found");
    let mut bgra = target.bgra.clone();
    super::blend::composite(
        &mut bgra,
        target.width,
        target.height,
        &super::blend::Paste {
            m: swapped.m,
            crop: &swapped.crop,
            mask: super::blend::MaskShape::SWAP,
            opacity: 1.,
        },
    );
    let result = DecodedFrame {
        source: String::new(),
        key: String::new(),
        pts: target.pts,
        width: target.width,
        height: target.height,
        bgra,
    };
    save(&target, "image_before.png");
    save(&result, "image_after.png");
    let (after, _, _) = identity_of(&result).unwrap();
    let to_source = after.similarity(&source);
    let to_target = after.similarity(&before);
    println!(
        "  identity: result↔source {to_source:.3}, result↔original {to_target:.3}, original↔source {:.3}",
        before.similarity(&source)
    );
    assert!(to_source > 0.3, "takes the source's identity");
    assert!(to_source > before.similarity(&source) + 0.2);
    // Far from the face nothing changed.
    let far = |b: &[u8]| {
        let (x, y) = (40usize, 600usize);
        let i = (y * target.width as usize + x) * 4;
        b[i..i + 4].to_vec()
    };
    assert_eq!(far(&result.bgra), far(&target.bgra));
    assert!(bbox.w > 0.);
}

/// The next frame from `t` (before `end`), stepping `t` past it; None at
/// the end (the reader hands its last frame back again past the end).
fn next_frame(reader: &mut FrameReader, t: &mut Micros, end: Micros) -> Option<Arc<DecodedFrame>> {
    if *t >= end {
        return None;
    }
    let f = reader.frame_at(*t).unwrap()?;
    let next = f.pts + reader.frame_duration();
    if next <= *t {
        return None;
    }
    *t = next;
    Some(f)
}

/// Mean absolute difference of two frames inside `b` (0–255).
fn diff_in(a: &DecodedFrame, b: &DecodedFrame, bx: &FaceBox) -> f32 {
    let (mut sum, mut n) = (0f64, 0f64);
    let w = a.width as usize;
    for y in bx.y.max(0.) as usize..((bx.y + bx.h) as usize).min(a.height as usize) {
        for x in bx.x.max(0.) as usize..((bx.x + bx.w) as usize).min(w) {
            let i = (y * w + x) * 4;
            for c in 0..3 {
                sum += (a.bgra[i + c] as f64 - b.bgra[i + c] as f64).abs();
            }
            n += 3.;
        }
    }
    (sum / n.max(1.)) as f32
}

/// One person talking: swapped on (nearly) every frame, the source's
/// identity, their expression kept, and no more flicker than the footage
/// has itself.
#[test]
#[ignore]
fn video_single_person() {
    setup();
    let video = media("one.mp4");
    let src = media("src_a.jpg");
    let people = job::find_people(&video, 0, 6 * SECOND, true, &AtomicBool::new(false)).unwrap();
    assert_eq!(people.len(), 1, "one person");
    let (r, file) = process(&video, 6 * SECOND, settings(&src, people[0].anchor.clone()));
    assert!(
        r.swapped as f32 >= r.frames as f32 * 0.95,
        "swapped {}/{}",
        r.swapped,
        r.frames
    );

    let source = source_identity(&src);
    let mut reader = FrameReader::open(&video, "t", 1280).unwrap();
    let (mut sims, mut moves, mut flicker_in, mut flicker_out) = (vec![], vec![], vec![], vec![]);
    let mut prev: Option<(Arc<DecodedFrame>, DecodedFrame)> = None;
    let mut t = 0;
    while let Some(f) = next_frame(&mut reader, &mut t, 6 * SECOND) {
        let Some(out_f) = job::composite(&f, &file, 1.) else {
            continue;
        };
        if let Some((pf, po)) = &prev
            && let Some((_, bbox, _)) = identity_of(&f).filter(|_| f.pts % (5 * 33_333) < 33_333)
        {
            flicker_in.push(diff_in(pf, &f, &bbox));
            flicker_out.push(diff_in(po, &out_f, &bbox));
        }
        if f.pts % (10 * 33_333) < 33_333 {
            let (orig_id, _, orig_lm) = identity_of(&f).unwrap();
            let (new_id, _, new_lm) = identity_of(&out_f).unwrap();
            sims.push((new_id.similarity(&source), orig_id.similarity(&source)));
            // Expression: the mouth corners stay where they were.
            let eye = orig_lm.eye_distance();
            let d =
                |a: [f32; 2], b: [f32; 2]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
            moves.push((d(orig_lm.0[3], new_lm.0[3]) + d(orig_lm.0[4], new_lm.0[4])) / 2. / eye);
            if sims.len() == 3 {
                save(&f, "video_before.png");
                save(&out_f, "video_after.png");
            }
        }
        prev = Some((f, out_f));
    }
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
    let id_new = mean(&sims.iter().map(|s| s.0).collect::<Vec<_>>());
    let id_old = mean(&sims.iter().map(|s| s.1).collect::<Vec<_>>());
    let (fi, fo) = (mean(&flicker_in), mean(&flicker_out));
    println!(
        "  identity to source {id_new:.3} (was {id_old:.3}); mouth moved {:.3} eye distances; \
         frame-to-frame change in the face {fo:.2} (footage {fi:.2})",
        mean(&moves)
    );
    assert!(id_new > 0.3 && id_new > id_old + 0.2);
    assert!(mean(&moves) < 0.12, "expression kept");
    assert!(fo < fi * 1.6 + 1., "no added flicker: {fo:.2} vs {fi:.2}");
}

/// Two people: the one picked (on the right) is swapped; the other's
/// pixels never change.
#[test]
#[ignore]
fn video_two_people_only_the_chosen_one() {
    setup();
    let video = media("two.mp4");
    let people = job::find_people(&video, 0, 4 * SECOND, true, &AtomicBool::new(false)).unwrap();
    assert_eq!(people.len(), 2, "two people");
    let right = people
        .iter()
        .find(|p| p.anchor.bbox[0] > 0.5)
        .expect("someone on the right");
    if let Some(path) = out("people_right.png") {
        right.thumb.save(path).unwrap();
    }
    let (r, file) = process(
        &video,
        4 * SECOND,
        settings(&media("src_a.jpg"), right.anchor.clone()),
    );
    assert!(r.swapped as f32 >= r.frames as f32 * 0.95);
    for t in [SECOND / 2, 2 * SECOND, 3 * SECOND] {
        let f = frame_at(&video, t, 1280);
        let o = job::composite(&f, &file, 1.).expect("swapped");
        let left = FaceBox {
            x: 0.,
            y: 0.,
            w: 636.,
            h: 720.,
        };
        let face = FaceBox {
            x: 860.,
            y: 100.,
            w: 240.,
            h: 260.,
        };
        assert_eq!(diff_in(&f, &o, &left), 0., "left person untouched at {t}");
        assert!(diff_in(&f, &o, &face) > 3., "right person swapped at {t}");
        if t == 2 * SECOND {
            save(&o, "two_after.png");
        }
    }
}

/// The face covered for a second: no swap while it's hidden (not on
/// the box covering it), back when it shows again.
#[test]
#[ignore]
fn video_face_disappears_and_comes_back() {
    setup();
    let video = media("gap.mp4");
    let people = job::find_people(&video, 0, 2 * SECOND, true, &AtomicBool::new(false)).unwrap();
    let (r, file) = process(
        &video,
        5 * SECOND,
        settings(&media("src_a.jpg"), people[0].anchor.clone()),
    );
    let mut reader = FrameReader::open(&video, "t", 1280).unwrap();
    let (mut hidden_swapped, mut after_swapped, mut after_total) = (0, 0, 0);
    let mut t = 0;
    while let Some(f) = next_frame(&mut reader, &mut t, 5 * SECOND) {
        let has = file.face_at(f.pts).is_some();
        if f.pts > 2 * SECOND + 50_000 && f.pts < 3 * SECOND - 50_000 && has {
            hidden_swapped += 1;
        }
        if f.pts > 3 * SECOND + 300_000 {
            after_total += 1;
            after_swapped += has as usize;
        }
    }
    println!("  hidden frames swapped {hidden_swapped}; after: {after_swapped}/{after_total}");
    assert_eq!(hidden_swapped, 0);
    assert!(
        after_swapped as f32 >= after_total as f32 * 0.9,
        "recovered"
    );
    assert!(r.swapped < r.frames);
}

/// Cancelled midway: an error, and no result left behind.
#[test]
#[ignore]
fn cancelling_leaves_nothing() {
    setup();
    let video = media("one.mp4");
    let people = job::find_people(&video, 0, 6 * SECOND, true, &AtomicBool::new(false)).unwrap();
    let mut s = settings(&media("src_b.jpg"), people[0].anchor.clone());
    s.quality = Quality::High; // a key no other test made
    let job = FaceSwapJob {
        media: video.clone(),
        key: s.cache_key(&video),
        settings: s,
        start: 0,
        end: 6 * SECOND,
    };
    let cancel = AtomicBool::new(false);
    let seen = std::cell::Cell::new(0);
    let r = job::process(&job, &cancel, &|p| {
        if matches!(p, FaceSwapProgress::Processing { .. }) {
            seen.set(seen.get() + 1);
            if seen.get() == 5 {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    });
    assert_eq!(r.err(), Some(super::FaceSwapError::Cancelled));
    let path = cache::path_for(&job.key);
    assert!(!path.exists() && !path.with_extension("part").exists());
}

/// The CPU and the GPU / Neural Engine give the same swap (within a
/// little rounding).
#[test]
#[ignore]
fn cpu_and_gpu_agree() {
    setup();
    let src = media("src_a.jpg");
    let target = frame_at(&media("one.mp4"), 2 * SECOND, 1280);
    let source = source_identity(&src);
    let (who, _, _) = identity_of(&target).unwrap();
    let run = |gpu: bool| {
        let t = std::time::Instant::now();
        let cfg = super::config::FaceSwapConfig {
            gpu,
            ..super::config::FaceSwapConfig::new(Quality::Fast, false)
        };
        let out = pipeline::with_models(gpu, |m| {
            let mut p = FrameProcessor::new(m, cfg, &source, who.clone(), 33_333);
            let first = p.process(m, &FrameView::of(&target))?;
            // Warm: time a second run.
            let t2 = std::time::Instant::now();
            p.process(m, &FrameView::of(&target))?;
            Ok((first, t2.elapsed(), m.provider))
        })
        .unwrap();
        println!(
            "  gpu={gpu}: {:?} per frame on {:?} (load + first {:?})",
            out.1,
            out.2,
            t.elapsed()
        );
        out.0.unwrap()
    };
    let (cpu, gpu) = (run(false), run(true));
    let d: f32 = cpu
        .crop
        .as_raw()
        .iter()
        .zip(gpu.crop.as_raw())
        .map(|(a, b)| (*a as f32 - *b as f32).abs())
        .sum::<f32>()
        / cpu.crop.as_raw().len() as f32;
    println!("  mean difference {d:.2} / 255");
    assert!(d < 6., "CPU and GPU differ by {d}");
}

/// Times the swapper builds on the CPU and the GPU (pass their files in
/// `REEL_FACE_SWAPPERS`, comma-separated).
#[test]
#[ignore]
fn bench_swapper_builds() {
    setup();
    let Ok(list) = std::env::var("REEL_FACE_SWAPPERS") else {
        return;
    };
    let crop = image::RgbImage::from_pixel(128, 128, image::Rgb([120, 100, 90]));
    for path in list.split(',') {
        for gpu in [false, true] {
            use super::swapper::FaceSwapper;
            let mut s =
                super::swapper::Inswapper::load(std::path::Path::new(path), gpu, None).unwrap();
            let latent = s.source_latent(&Embedding::normalized(vec![1.; 512]));
            s.swap(&crop, &latent).unwrap();
            let t = std::time::Instant::now();
            for _ in 0..5 {
                s.swap(&crop, &latent).unwrap();
            }
            println!(
                "  {path} gpu={gpu} {:?}: {:?} per swap",
                s.provider(),
                t.elapsed() / 5
            );
        }
    }
}

/// High quality with restoration: a 512 px head crop that still carries
/// the source identity (the restorer sharpens, it doesn't undo the swap).
#[test]
#[ignore]
fn restoration_keeps_the_identity() {
    setup();
    let src = media("src_a.jpg");
    let source = source_identity(&src);
    let target = frame_at(&media("one.mp4"), 2 * SECOND, 1280);
    let (before, _, _) = identity_of(&target).unwrap();
    let cfg = super::config::FaceSwapConfig::new(Quality::High, true);
    let swapped = pipeline::with_models(true, |m| {
        let mut p = FrameProcessor::new(m, cfg, &source, before.clone(), 33_333);
        p.process(m, &FrameView::of(&target))
    })
    .unwrap()
    .expect("found");
    assert_eq!(swapped.crop.width(), super::restorer::SIZE);
    let mut bgra = target.bgra.clone();
    super::blend::composite(
        &mut bgra,
        target.width,
        target.height,
        &super::blend::Paste {
            m: swapped.m,
            crop: &swapped.crop,
            mask: super::blend::MaskShape::SWAP,
            opacity: 1.,
        },
    );
    let result = DecodedFrame {
        source: String::new(),
        key: String::new(),
        pts: target.pts,
        width: target.width,
        height: target.height,
        bgra,
    };
    save(&result, "restored_after.png");
    let (after, _, _) = identity_of(&result).unwrap();
    println!(
        "  restored identity to source {:.3}",
        after.similarity(&source)
    );
    assert!(after.similarity(&source) > 0.3);
}
