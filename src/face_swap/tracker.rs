//! Following the chosen person through a clip.
//!
//! The track starts from the person picked in the panel (their identity
//! from the embedding model). Each frame, the detection that overlaps the
//! last box most is them, as long as it overlaps enough; their identity is
//! checked again every few frames, and whenever another face overlaps the
//! same spot. When the overlap or the identity fails the track is lost: no
//! swap on those frames (never another person's face). While lost, every
//! face is compared with the person and the track resumes on one that
//! clears a stricter bar.
//!
//! The five landmarks are smoothed through a One-Euro filter (steady when
//! the face holds still, little lag when it moves), measured in eye
//! distances so it behaves the same for near and far faces.

use super::config::TrackerConfig;
use super::error::Result;
use super::types::{DetectedFace, Embedding, FaceBox, FaceLandmarks};

/// One coordinate through a One-Euro filter.
#[derive(Clone, Copy, Debug)]
struct OneEuro {
    x: f32,
    dx: f32,
}

/// Smoothing per second: the cut-off when still (Hz), how fast it opens
/// with speed (per eye distance per second), the derivative's cut-off.
const MIN_CUTOFF: f32 = 1.5;
const BETA: f32 = 0.8;
const D_CUTOFF: f32 = 1.0;

fn alpha(cutoff: f32, dt: f32) -> f32 {
    let tau = 1. / (2. * std::f32::consts::PI * cutoff);
    1. / (1. + tau / dt)
}

impl OneEuro {
    /// `x` and the scale it's measured against, `dt` seconds after the last.
    fn step(&mut self, x: f32, scale: f32, dt: f32) -> f32 {
        let dx = (x - self.x) / dt;
        let a_d = alpha(D_CUTOFF, dt);
        self.dx += a_d * (dx - self.dx);
        let speed = self.dx.abs() / scale.max(1.);
        let a = alpha(MIN_CUTOFF + BETA * speed, dt);
        self.x += a * (x - self.x);
        self.x
    }
}

/// The landmarks' smoother.
#[derive(Clone, Debug, Default)]
pub struct LandmarkSmoother {
    state: Option<[OneEuro; 10]>,
}

impl LandmarkSmoother {
    pub fn reset(&mut self) {
        self.state = None;
    }

    pub fn smooth(&mut self, raw: &FaceLandmarks, dt: f32) -> FaceLandmarks {
        let scale = raw.eye_distance();
        let flat: [f32; 10] = std::array::from_fn(|i| raw.0[i / 2][i % 2]);
        let state = self
            .state
            .get_or_insert_with(|| flat.map(|x| OneEuro { x, dx: 0. }));
        let out: [f32; 10] = std::array::from_fn(|i| state[i].step(flat[i], scale, dt.max(1e-3)));
        FaceLandmarks(std::array::from_fn(|j| [out[2 * j], out[2 * j + 1]]))
    }
}

/// What the tracker did with a frame.
#[derive(Clone, Debug, PartialEq)]
pub enum TrackStep {
    /// The person, with smoothed landmarks.
    Found {
        face: DetectedFace,
        landmarks: FaceLandmarks,
        /// The identity was checked on this frame.
        verified: bool,
    },
    /// Not on this frame (gone, hidden, or not sure it's them).
    Lost,
}

/// The chosen person through the clip (`id` is the track's: one per job).
pub struct FaceTrack {
    pub id: u32,
    cfg: TrackerConfig,
    /// Who they are: the picked face, refined with confident sightings.
    identity: Embedding,
    picked: Embedding,
    last: Option<FaceBox>,
    since_check: u32,
    smoother: LandmarkSmoother,
    pub lost_frames: u32,
}

impl FaceTrack {
    /// Starts on `face` (the person as picked) with identity `identity`.
    #[cfg(test)]
    pub fn new(id: u32, cfg: TrackerConfig, face: &DetectedFace, identity: Embedding) -> FaceTrack {
        FaceTrack {
            id,
            cfg,
            picked: identity.clone(),
            identity,
            last: Some(face.bbox),
            since_check: 0,
            smoother: LandmarkSmoother::default(),
            lost_frames: 0,
        }
    }

    /// Starts lost, looking for the person with identity `identity`: the
    /// first frame showing them clearly enough picks them up.
    pub fn looking_for(id: u32, cfg: TrackerConfig, identity: Embedding) -> FaceTrack {
        FaceTrack {
            id,
            cfg,
            picked: identity.clone(),
            identity,
            last: None,
            since_check: 0,
            smoother: LandmarkSmoother::default(),
            lost_frames: 0,
        }
    }

    /// How much `e` looks like the person (the better of the picked face
    /// and the refined identity, so slow drift can't walk away from them).
    fn similarity(&self, e: &Embedding) -> f32 {
        e.similarity(&self.picked).max(e.similarity(&self.identity))
    }

    /// Follows the person into a frame with detections `faces`, `dt`
    /// seconds after the last. `embed` gives a face's identity (costly:
    /// only called when needed).
    pub fn step(
        &mut self,
        faces: &[DetectedFace],
        dt: f32,
        embed: &mut dyn FnMut(&DetectedFace) -> Result<Embedding>,
    ) -> Result<TrackStep> {
        let found = match self.last {
            Some(last) => self.follow(last, faces, embed)?,
            None => self.recover(faces, embed)?,
        };
        match found {
            Some((face, verified)) => {
                if self.last.is_none() {
                    self.smoother.reset();
                }
                self.last = Some(face.bbox);
                self.lost_frames = 0;
                let landmarks = self.smoother.smooth(&face.landmarks, dt);
                Ok(TrackStep::Found {
                    face,
                    landmarks,
                    verified,
                })
            }
            None => {
                self.last = None;
                self.lost_frames += 1;
                self.smoother.reset();
                Ok(TrackStep::Lost)
            }
        }
    }

    fn follow(
        &mut self,
        last: FaceBox,
        faces: &[DetectedFace],
        embed: &mut dyn FnMut(&DetectedFace) -> Result<Embedding>,
    ) -> Result<Option<(DetectedFace, bool)>> {
        let mut near: Vec<(f32, &DetectedFace)> = faces
            .iter()
            .map(|f| (f.bbox.iou(&last), f))
            .filter(|(iou, _)| *iou >= self.cfg.iou_min)
            .collect();
        near.sort_by(|a, b| b.0.total_cmp(&a.0));
        let Some(&(_, best)) = near.first() else {
            return Ok(None);
        };
        self.since_check += 1;
        // Two faces where the person was (crossing, hugging): check who.
        let crowded = near.len() > 1;
        if !crowded && self.since_check < self.cfg.verify_every {
            return Ok(Some((*best, false)));
        }
        self.since_check = 0;
        let mut pick: Option<(f32, &DetectedFace, Embedding)> = None;
        for (_, f) in near.iter().take(3) {
            let e = embed(f)?;
            let s = self.similarity(&e);
            if pick.as_ref().is_none_or(|(b, _, _)| s > *b) {
                pick = Some((s, f, e));
            }
        }
        match pick {
            Some((s, f, e)) if s >= self.cfg.keep_similarity => {
                self.learn(&e, s);
                Ok(Some((*f, true)))
            }
            _ => Ok(None),
        }
    }

    fn recover(
        &mut self,
        faces: &[DetectedFace],
        embed: &mut dyn FnMut(&DetectedFace) -> Result<Embedding>,
    ) -> Result<Option<(DetectedFace, bool)>> {
        // The biggest few faces are compared (a crowd isn't embedded whole).
        let mut by_size: Vec<&DetectedFace> = faces.iter().collect();
        by_size.sort_by(|a, b| b.bbox.area().total_cmp(&a.bbox.area()));
        let mut best: Option<(f32, &DetectedFace, Embedding)> = None;
        for f in by_size.into_iter().take(4) {
            let e = embed(f)?;
            let s = self.similarity(&e);
            if best.as_ref().is_none_or(|(b, _, _)| s > *b) {
                best = Some((s, f, e));
            }
        }
        match best {
            Some((s, f, e)) if s >= self.cfg.recover_similarity => {
                self.since_check = 0;
                self.learn(&e, s);
                Ok(Some((*f, true)))
            }
            _ => Ok(None),
        }
    }

    /// A confident sighting nudges the identity (new angles, light).
    fn learn(&mut self, e: &Embedding, similarity: f32) {
        if similarity >= self.cfg.recover_similarity {
            self.identity = self.identity.blend(e, 0.1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face_swap::error::FaceSwapError;

    fn face_at(x: f32, who: f32) -> DetectedFace {
        // `who` rides in the score so the fake embedder can tell people apart.
        DetectedFace {
            bbox: FaceBox {
                x,
                y: 100.,
                w: 100.,
                h: 120.,
            },
            landmarks: FaceLandmarks([
                [x + 30., 140.],
                [x + 70., 140.],
                [x + 50., 165.],
                [x + 35., 190.],
                [x + 65., 190.],
            ]),
            score: who,
        }
    }

    /// Person A is (1, 0); person B is (0, 1).
    fn identity(f: &DetectedFace) -> Result<Embedding> {
        Ok(if f.score > 0.5 {
            Embedding(vec![1., 0.])
        } else {
            Embedding(vec![0., 1.])
        })
    }

    fn track() -> FaceTrack {
        FaceTrack::new(
            1,
            TrackerConfig::default(),
            &face_at(100., 0.9),
            Embedding(vec![1., 0.]),
        )
    }

    #[test]
    fn follows_a_moving_face_without_checking_every_frame() {
        let mut t = track();
        let mut calls = 0;
        let mut embed = |f: &DetectedFace| {
            calls += 1;
            identity(f)
        };
        for i in 0..16 {
            let step = t
                .step(&[face_at(100. + i as f32 * 5., 0.9)], 1. / 30., &mut embed)
                .unwrap();
            assert!(matches!(step, TrackStep::Found { .. }), "frame {i}");
        }
        // Checked every `verify_every` frames, not every frame.
        assert_eq!(calls, 2);
    }

    /// The person leaves; a stranger walks into the same spot: no swap
    /// on them. The person comes back elsewhere: picked up again.
    #[test]
    fn never_swaps_a_stranger_and_recovers_the_person() {
        let mut t = track();
        let mut embed = identity;
        assert!(matches!(
            t.step(&[face_at(100., 0.9)], 0.033, &mut embed).unwrap(),
            TrackStep::Found { .. }
        ));
        // Gone for a few frames.
        for _ in 0..3 {
            assert_eq!(t.step(&[], 0.033, &mut embed).unwrap(), TrackStep::Lost);
        }
        // A stranger where they were.
        for _ in 0..5 {
            assert_eq!(
                t.step(&[face_at(100., 0.1)], 0.033, &mut embed).unwrap(),
                TrackStep::Lost
            );
        }
        // Back, across the frame, next to the stranger.
        match t
            .step(&[face_at(100., 0.1), face_at(600., 0.9)], 0.033, &mut embed)
            .unwrap()
        {
            TrackStep::Found { face, verified, .. } => {
                assert_eq!(face.bbox.x, 600.);
                assert!(verified);
            }
            TrackStep::Lost => panic!("not recovered"),
        }
    }

    /// Someone steps in front of the person: overlap alone would follow
    /// them; the identity check doesn't.
    #[test]
    fn checks_identity_when_faces_overlap() {
        let mut t = track();
        let mut embed = identity;
        t.step(&[face_at(100., 0.9)], 0.033, &mut embed).unwrap();
        let step = t
            .step(&[face_at(105., 0.1), face_at(140., 0.9)], 0.033, &mut embed)
            .unwrap();
        match step {
            TrackStep::Found { face, verified, .. } => {
                assert_eq!(face.bbox.x, 140.);
                assert!(verified);
            }
            TrackStep::Lost => panic!("lost"),
        }
    }

    #[test]
    fn embedder_errors_reach_the_caller() {
        let mut t = track();
        let mut fail =
            |_: &DetectedFace| -> Result<Embedding> { Err(FaceSwapError::Inference("x".into())) };
        assert!(
            t.step(&[face_at(100., 0.9), face_at(110., 0.9)], 0.033, &mut fail)
                .is_err()
        );
    }

    /// Jitter on a still face is damped; a real move is followed.
    #[test]
    fn smoothing_damps_jitter_and_follows_moves() {
        let mut s = LandmarkSmoother::default();
        let base = face_at(100., 1.).landmarks;
        let mut worst = 0f32;
        for i in 0..60 {
            let j = if i % 2 == 0 { 2. } else { -2. };
            let noisy = FaceLandmarks(base.0.map(|[x, y]| [x + j, y]));
            let out = s.smooth(&noisy, 1. / 30.);
            if i > 10 {
                worst = worst.max((out.0[0][0] - base.0[0][0]).abs());
            }
        }
        assert!(worst < 1.2, "jitter left: {worst}");
        // A 100 px jump is mostly followed within a few frames.
        let moved = FaceLandmarks(base.0.map(|[x, y]| [x + 100., y]));
        let mut out = base;
        for _ in 0..5 {
            out = s.smooth(&moved, 1. / 30.);
        }
        assert!(
            out.0[0][0] - base.0[0][0] > 80.,
            "lagging: {}",
            out.0[0][0] - base.0[0][0]
        );
    }
}
