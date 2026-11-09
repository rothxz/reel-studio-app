//! Face detection: YuNet (MIT, OpenCV Zoo), a 230 KB model that finds
//! faces with their five landmarks in one pass.
//!
//! The model sees a 640 × 640 picture (the frame scaled to fit,
//! top-left, the rest black; BGR 0–255). It answers at three strides
//! (8, 16, 32 px), one anchor per cell: a face score (class × objectness),
//! a box (centre offset in cells, log size), and five points (offsets in
//! cells). Overlapping answers are thinned by non-maximum suppression.

use std::path::Path;

use super::error::{FaceSwapError, Result};
use super::inference::Model;
use super::types::{DetectedFace, FaceBox, FaceLandmarks, FrameView};

const STRIDES: [usize; 3] = [8, 16, 32];
/// Overlap above which two answers are the same face.
const NMS_IOU: f32 = 0.3;
/// Faces kept per frame at most.
const TOP_K: usize = 32;

pub struct Detector {
    model: Model,
    side: usize,
}

impl Detector {
    pub fn load(path: &Path, gpu: bool, cache: Option<&Path>) -> Result<Detector> {
        let model = Model::load(path, gpu, cache)?;
        let side = match model.input_hw(0) {
            Some((h, w)) if h == w && h % 32 == 0 => h as usize,
            None => 640,
            other => {
                return Err(FaceSwapError::ModelShape(format!(
                    "detector input {other:?}"
                )));
            }
        };
        if model.outputs.len() != 12 {
            return Err(FaceSwapError::ModelShape(format!(
                "detector has {} outputs, expected 12",
                model.outputs.len()
            )));
        }
        Ok(Detector { model, side })
    }

    /// Every face in `frame` scoring at least `threshold` (0–1), best
    /// first; boxes and points in frame pixels.
    pub fn detect(&mut self, frame: &FrameView, threshold: f32) -> Result<Vec<DetectedFace>> {
        let side = self.side;
        let scale = side as f32 / frame.width.max(frame.height) as f32;
        let input = letterbox_bgr(frame, side, scale);
        let outs = self
            .model
            .run(vec![(vec![1, 3, side as i64, side as i64], input)])?;
        let by_name = |prefix: &str, stride: usize| {
            let name = format!("{prefix}_{stride}");
            self.model
                .outputs
                .iter()
                .position(|p| p.name == name)
                .map(|i| &outs[i].data)
                .ok_or_else(|| FaceSwapError::ModelShape(format!("detector has no {name}")))
        };
        let mut faces = vec![];
        for stride in STRIDES {
            let cells = side / stride;
            let (cls, obj) = (by_name("cls", stride)?, by_name("obj", stride)?);
            let (bbox, kps) = (by_name("bbox", stride)?, by_name("kps", stride)?);
            if cls.len() < cells * cells
                || bbox.len() < cells * cells * 4
                || kps.len() < cells * cells * 10
            {
                return Err(FaceSwapError::ModelShape(format!(
                    "detector stride {stride} outputs are short"
                )));
            }
            decode_level(
                stride,
                cells,
                Level {
                    cls,
                    obj,
                    bbox,
                    kps,
                },
                threshold,
                1. / scale,
                &mut faces,
            );
        }
        let faces = nms(faces, NMS_IOU, TOP_K);
        Ok(faces
            .into_iter()
            .map(|f| clamp_to(f, frame.width as f32, frame.height as f32))
            .collect())
    }
}

/// The frame scaled by `scale` into the top-left of a `side`² picture,
/// planar BGR 0–255 (area-averaged when shrinking a lot).
fn letterbox_bgr(frame: &FrameView, side: usize, scale: f32) -> Vec<f32> {
    let plane = side * side;
    let mut out = vec![0f32; plane * 3];
    let dw = ((frame.width as f32 * scale).round() as usize).min(side);
    let dh = ((frame.height as f32 * scale).round() as usize).min(side);
    let step = 1. / scale;
    let taps = (step.ceil() as usize).clamp(1, 3);
    let (fw, fh) = (frame.width as usize, frame.height as usize);
    for y in 0..dh {
        for x in 0..dw {
            let mut acc = [0f32; 3];
            for sy in 0..taps {
                for sx in 0..taps {
                    let fx = ((x as f32 + (sx as f32 + 0.5) / taps as f32) * step) as usize;
                    let fy = ((y as f32 + (sy as f32 + 0.5) / taps as f32) * step) as usize;
                    let i = (fy.min(fh - 1) * fw + fx.min(fw - 1)) * 4;
                    for (a, v) in acc.iter_mut().zip(&frame.bgra[i..i + 3]) {
                        *a += *v as f32;
                    }
                }
            }
            let n = (taps * taps) as f32;
            let o = y * side + x;
            for c in 0..3 {
                out[c * plane + o] = acc[c] / n;
            }
        }
    }
    out
}

struct Level<'a> {
    cls: &'a [f32],
    obj: &'a [f32],
    bbox: &'a [f32],
    kps: &'a [f32],
}

/// One stride's anchors → faces (in frame pixels: model px × `unscale`).
fn decode_level(
    stride: usize,
    cells: usize,
    l: Level,
    threshold: f32,
    unscale: f32,
    out: &mut Vec<DetectedFace>,
) {
    let s = stride as f32;
    for i in 0..cells * cells {
        let score = (l.cls[i].clamp(0., 1.) * l.obj[i].clamp(0., 1.)).sqrt();
        if score < threshold {
            continue;
        }
        let (r, c) = ((i / cells) as f32, (i % cells) as f32);
        let b = &l.bbox[i * 4..i * 4 + 4];
        let (cx, cy) = ((c + b[0]) * s, (r + b[1]) * s);
        let (w, h) = (b[2].exp() * s, b[3].exp() * s);
        let k = &l.kps[i * 10..i * 10 + 10];
        let mut pts = [[0f32; 2]; 5];
        for (j, p) in pts.iter_mut().enumerate() {
            *p = [
                (c + k[2 * j]) * s * unscale,
                (r + k[2 * j + 1]) * s * unscale,
            ];
        }
        out.push(DetectedFace {
            bbox: FaceBox {
                x: (cx - w / 2.) * unscale,
                y: (cy - h / 2.) * unscale,
                w: w * unscale,
                h: h * unscale,
            },
            landmarks: FaceLandmarks(pts),
            score,
        });
    }
}

/// Best first; drops any answer overlapping a better one by more than `iou`.
pub fn nms(mut faces: Vec<DetectedFace>, iou: f32, keep: usize) -> Vec<DetectedFace> {
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<DetectedFace> = vec![];
    for f in faces {
        if kept.len() >= keep {
            break;
        }
        if kept.iter().all(|k| k.bbox.iou(&f.bbox) <= iou) {
            kept.push(f);
        }
    }
    kept
}

fn clamp_to(mut f: DetectedFace, w: f32, h: f32) -> DetectedFace {
    let x0 = f.bbox.x.clamp(0., w);
    let y0 = f.bbox.y.clamp(0., h);
    let x1 = (f.bbox.x + f.bbox.w).clamp(0., w);
    let y1 = (f.bbox.y + f.bbox.h).clamp(0., h);
    f.bbox = FaceBox {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    };
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(x: f32, score: f32) -> DetectedFace {
        DetectedFace {
            bbox: FaceBox {
                x,
                y: 0.,
                w: 10.,
                h: 10.,
            },
            landmarks: FaceLandmarks::default(),
            score,
        }
    }

    #[test]
    fn nms_keeps_the_best_of_each_face() {
        let kept = nms(
            vec![face(0., 0.6), face(1., 0.9), face(50., 0.7), face(2., 0.8)],
            0.3,
            10,
        );
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].score, 0.9);
        assert_eq!(kept[1].bbox.x, 50.);
        assert_eq!(nms(vec![face(0., 0.5), face(50., 0.4)], 0.3, 1).len(), 1);
    }

    /// One anchor decoded by hand: cell (row 2, col 3) at stride 8.
    #[test]
    fn decodes_an_anchor() {
        let cells = 4;
        let mut cls = vec![0f32; 16];
        let mut obj = vec![0f32; 16];
        let mut bbox = vec![0f32; 64];
        let mut kps = vec![0f32; 160];
        let i = 2 * cells + 3;
        cls[i] = 0.81;
        obj[i] = 1.;
        bbox[i * 4..i * 4 + 4].copy_from_slice(&[0.5, 0.5, 2f32.ln(), 0.]);
        kps[i * 10] = 1.;
        let mut out = vec![];
        let l = Level {
            cls: &cls,
            obj: &obj,
            bbox: &bbox,
            kps: &kps,
        };
        decode_level(8, cells, l, 0.5, 2., &mut out);
        assert_eq!(out.len(), 1);
        let f = out[0];
        assert!((f.score - 0.9).abs() < 1e-6);
        // Centre (3.5, 2.5) cells = (28, 20) px; 16 × 8; then × 2.
        assert_eq!(
            f.bbox,
            FaceBox {
                x: 40.,
                y: 32.,
                w: 32.,
                h: 16.
            }
        );
        assert_eq!(f.landmarks.0[0], [64., 32.]);
    }

    #[test]
    fn letterbox_keeps_aspect_top_left() {
        let (w, h) = (64u32, 32u32);
        let bgra: Vec<u8> = (0..w * h).flat_map(|_| [10u8, 20, 30, 255]).collect();
        let f = FrameView {
            width: w,
            height: h,
            bgra: &bgra,
        };
        let out = letterbox_bgr(&f, 32, 0.5);
        // B plane: filled rows 0..16, black below.
        assert_eq!(out[0], 10.);
        assert_eq!(out[15 * 32 + 31], 10.);
        assert_eq!(out[16 * 32], 0.);
        // R plane.
        assert_eq!(out[2 * 1024], 30.);
    }
}
