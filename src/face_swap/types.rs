//! The values passed along the pipeline.

/// A point in pixels.
pub type Point = [f32; 2];

/// An axis-aligned box in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FaceBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl FaceBox {
    pub fn area(&self) -> f32 {
        self.w.max(0.) * self.h.max(0.)
    }

    pub fn center(&self) -> Point {
        [self.x + self.w / 2., self.y + self.h / 2.]
    }

    /// Intersection over union with `o`.
    pub fn iou(&self, o: &FaceBox) -> f32 {
        let ix = ((self.x + self.w).min(o.x + o.w) - self.x.max(o.x)).max(0.);
        let iy = ((self.y + self.h).min(o.y + o.h) - self.y.max(o.y)).max(0.);
        let inter = ix * iy;
        let union = self.area() + o.area() - inter;
        if union <= 0. { 0. } else { inter / union }
    }

    /// In 0–1 of a `w` × `h` picture (what the document stores).
    pub fn normalized(&self, w: u32, h: u32) -> [f32; 4] {
        let (w, h) = (w.max(1) as f32, h.max(1) as f32);
        [self.x / w, self.y / h, self.w / w, self.h / h]
    }

    pub fn from_normalized(n: [f32; 4], w: u32, h: u32) -> FaceBox {
        let (w, h) = (w as f32, h as f32);
        FaceBox {
            x: n[0] * w,
            y: n[1] * h,
            w: n[2] * w,
            h: n[3] * h,
        }
    }
}

/// Five points, in picture order: the eye on the picture's left, the eye
/// on its right, the nose tip, the mouth corner on the left, on the right.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FaceLandmarks(pub [Point; 5]);

impl FaceLandmarks {
    /// Distance between the eyes (the face's scale).
    pub fn eye_distance(&self) -> f32 {
        let [l, r, ..] = self.0;
        ((r[0] - l[0]).powi(2) + (r[1] - l[1]).powi(2)).sqrt()
    }
}

/// One face the detector found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectedFace {
    pub bbox: FaceBox,
    pub landmarks: FaceLandmarks,
    /// 0–1.
    pub score: f32,
}

/// A frame to read from: tightly packed BGRA (what the decoder hands out).
#[derive(Clone, Copy)]
pub struct FrameView<'a> {
    pub width: u32,
    pub height: u32,
    pub bgra: &'a [u8],
}

impl<'a> FrameView<'a> {
    pub fn of(f: &'a crate::decoder::DecodedFrame) -> Self {
        FrameView {
            width: f.width,
            height: f.height,
            bgra: &f.bgra,
        }
    }
}

/// A unit-length identity vector (512 floats from the embedding model).
#[derive(Clone, Debug, PartialEq)]
pub struct Embedding(pub Vec<f32>);

impl Embedding {
    /// Scaled to length 1 (a zero vector stays zero).
    pub fn normalized(mut v: Vec<f32>) -> Embedding {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if n > 0. {
            v.iter_mut().for_each(|x| *x /= n);
        }
        Embedding(v)
    }

    /// Cosine similarity (both are unit length): 1 the same face, around 0
    /// strangers.
    pub fn similarity(&self, o: &Embedding) -> f32 {
        self.0.iter().zip(&o.0).map(|(a, b)| a * b).sum()
    }

    /// The mean of `self` weighted `1 - w` and `o` weighted `w`, unit length.
    pub fn blend(&self, o: &Embedding, w: f32) -> Embedding {
        Embedding::normalized(
            self.0
                .iter()
                .zip(&o.0)
                .map(|(a, b)| a * (1. - w) + b * w)
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iou_of_boxes() {
        let a = FaceBox {
            x: 0.,
            y: 0.,
            w: 10.,
            h: 10.,
        };
        assert_eq!(a.iou(&a), 1.);
        let b = FaceBox { x: 5., ..a };
        assert!((a.iou(&b) - 50. / 150.).abs() < 1e-6);
        let far = FaceBox { x: 100., ..a };
        assert_eq!(a.iou(&far), 0.);
    }

    #[test]
    fn embeddings_compare_by_angle() {
        let a = Embedding::normalized(vec![3., 4.]);
        assert!((a.0[0] - 0.6).abs() < 1e-6);
        assert!((a.similarity(&a) - 1.).abs() < 1e-6);
        let b = Embedding::normalized(vec![-4., 3.]);
        assert!(a.similarity(&b).abs() < 1e-6);
        let mid = a.blend(&b, 0.5);
        assert!((mid.similarity(&a) - mid.similarity(&b)).abs() < 1e-6);
    }

    #[test]
    fn boxes_round_trip_normalized() {
        let b = FaceBox {
            x: 192.,
            y: 108.,
            w: 384.,
            h: 216.,
        };
        let n = b.normalized(1920, 1080);
        assert_eq!(n, [0.1, 0.1, 0.2, 0.2]);
        let back = FaceBox::from_normalized(n, 1920, 1080);
        assert!((back.x - b.x).abs() < 1e-3 && (back.h - b.h).abs() < 1e-3);
    }
}
