//! Five-point face alignment: the similarity transform (rotation, uniform
//! scale, shift) that best maps a face's landmarks onto a model's template
//! (least squares, closed form), and the warps through it.

use image::RgbImage;

use super::types::{FaceLandmarks, FrameView, Point};

/// The embedding model's 112 px template (eyes, nose, mouth corners), the
/// geometry the identity models in this family are trained on.
pub const ARCFACE_112: [Point; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

/// A whole-head template (fractions of the crop) for the 512 px face
/// restoration model, trained on faces aligned this way.
pub const FFHQ_UNIT: [Point; 5] = [
    [0.376_916_8, 0.468_646_6],
    [0.622_857, 0.469_128_1],
    [0.501_238_6, 0.613_319],
    [0.393_088_2, 0.725_411],
    [0.611_502, 0.724_904_6],
];

/// The 112 px template placed in a `size` px crop: scaled when the size is
/// a multiple of 112, else centred in a 128-wide frame (a 128 crop keeps
/// the 112 face and adds 8 px each side, as the swapper expects).
pub fn arcface_template(size: u32) -> [Point; 5] {
    let (ratio, shift) = if size.is_multiple_of(112) {
        (size as f32 / 112., 0.)
    } else {
        let r = size as f32 / 128.;
        (r, 8. * r)
    };
    ARCFACE_112.map(|[x, y]| [x * ratio + shift, y * ratio])
}

pub fn ffhq_template(size: u32) -> [Point; 5] {
    FFHQ_UNIT.map(|[x, y]| [x * size as f32, y * size as f32])
}

/// `[[a, -b, tx], [b, a, ty]]`: a point `p` goes to
/// `(a·px − b·py + tx, b·px + a·py + ty)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Similarity {
    pub a: f32,
    pub b: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Similarity {
    pub const IDENTITY: Similarity = Similarity {
        a: 1.,
        b: 0.,
        tx: 0.,
        ty: 0.,
    };

    /// The least-squares similarity taking `src` onto `dst`.
    pub fn estimate(src: &[Point; 5], dst: &[Point; 5]) -> Similarity {
        let n = src.len() as f32;
        let mean = |p: &[Point; 5]| {
            let s = p.iter().fold([0., 0.], |a, q| [a[0] + q[0], a[1] + q[1]]);
            [s[0] / n, s[1] / n]
        };
        let (ms, md) = (mean(src), mean(dst));
        let (mut dot, mut cross, mut norm) = (0f32, 0f32, 0f32);
        for (s, d) in src.iter().zip(dst) {
            let (sx, sy) = (s[0] - ms[0], s[1] - ms[1]);
            let (dx, dy) = (d[0] - md[0], d[1] - md[1]);
            dot += sx * dx + sy * dy;
            cross += sx * dy - sy * dx;
            norm += sx * sx + sy * sy;
        }
        if norm <= f32::EPSILON {
            return Similarity {
                tx: md[0] - ms[0],
                ty: md[1] - ms[1],
                ..Self::IDENTITY
            };
        }
        let (a, b) = (dot / norm, cross / norm);
        Similarity {
            a,
            b,
            tx: md[0] - (a * ms[0] - b * ms[1]),
            ty: md[1] - (b * ms[0] + a * ms[1]),
        }
    }

    /// The transform aligning `face` to `template`.
    pub fn align(face: &FaceLandmarks, template: &[Point; 5]) -> Similarity {
        Self::estimate(&face.0, template)
    }

    pub fn apply(&self, [x, y]: Point) -> Point {
        [
            self.a * x - self.b * y + self.tx,
            self.b * x + self.a * y + self.ty,
        ]
    }

    pub fn scale(&self) -> f32 {
        (self.a * self.a + self.b * self.b).sqrt()
    }

    pub fn inverse(&self) -> Similarity {
        let d = self.a * self.a + self.b * self.b;
        if d <= f32::EPSILON {
            return Self::IDENTITY;
        }
        let (a, b) = (self.a / d, -self.b / d);
        Similarity {
            a,
            b,
            tx: -(a * self.tx - b * self.ty),
            ty: -(b * self.tx + a * self.ty),
        }
    }

    /// `self` after the input is scaled by `s` first: the same mapping for
    /// a picture `s` times the size.
    pub fn with_input_scale(&self, s: f32) -> Similarity {
        Similarity {
            a: self.a / s,
            b: self.b / s,
            ..*self
        }
    }
}

/// A BGRA pixel at a fractional position, bilinear, edges clamped; RGB out.
#[inline]
pub fn sample_bgra(f: &FrameView, x: f32, y: f32) -> [f32; 3] {
    let (w, h) = (f.width as i32, f.height as i32);
    let x = x.clamp(0., (w - 1) as f32);
    let y = y.clamp(0., (h - 1) as f32);
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let px = |xx: i32, yy: i32| {
        let i = (yy as usize * w as usize + xx as usize) * 4;
        let p = &f.bgra[i..i + 3];
        [p[2] as f32, p[1] as f32, p[0] as f32]
    };
    let (p00, p10, p01, p11) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    let mut out = [0f32; 3];
    for c in 0..3 {
        let top = p00[c] + (p10[c] - p00[c]) * fx;
        let bottom = p01[c] + (p11[c] - p01[c]) * fx;
        out[c] = top + (bottom - top) * fy;
    }
    out
}

/// An RGB pixel at a fractional position, bilinear, edges clamped.
#[inline]
pub fn sample_rgb(img: &RgbImage, x: f32, y: f32) -> [f32; 3] {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let x = x.clamp(0., (w - 1) as f32);
    let y = y.clamp(0., (h - 1) as f32);
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let raw = img.as_raw();
    let px = |xx: i32, yy: i32| {
        let i = (yy as usize * w as usize + xx as usize) * 3;
        [raw[i] as f32, raw[i + 1] as f32, raw[i + 2] as f32]
    };
    let (p00, p10, p01, p11) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    let mut out = [0f32; 3];
    for c in 0..3 {
        let top = p00[c] + (p10[c] - p00[c]) * fx;
        let bottom = p01[c] + (p11[c] - p01[c]) * fx;
        out[c] = top + (bottom - top) * fy;
    }
    out
}

/// The `size` px RGB crop that `m` (frame → crop) makes of `frame`. When
/// the crop is much smaller than the face in the frame, each crop pixel
/// averages a small grid of frame samples so fine detail doesn't alias.
pub fn warp_crop(frame: &FrameView, m: &Similarity, size: u32) -> RgbImage {
    let inv = m.inverse();
    // Frame pixels per crop pixel.
    let step = inv.scale();
    let taps = (step.ceil() as i32).clamp(1, 4);
    let mut out = RgbImage::new(size, size);
    let raw: &mut [u8] = &mut out;
    for y in 0..size {
        for x in 0..size {
            let mut acc = [0f32; 3];
            for sy in 0..taps {
                for sx in 0..taps {
                    let q = [
                        x as f32 + (sx as f32 + 0.5) / taps as f32 - 0.5,
                        y as f32 + (sy as f32 + 0.5) / taps as f32 - 0.5,
                    ];
                    let p = inv.apply(q);
                    let s = sample_bgra(frame, p[0], p[1]);
                    for c in 0..3 {
                        acc[c] += s[c];
                    }
                }
            }
            let n = (taps * taps) as f32;
            let i = ((y * size + x) * 3) as usize;
            for c in 0..3 {
                raw[i + c] = (acc[c] / n).round().clamp(0., 255.) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    /// A template rotated, scaled and shifted is mapped back exactly.
    #[test]
    fn recovers_a_known_similarity() {
        let truth = Similarity {
            a: 2.0 * 0.3f32.cos(),
            b: 2.0 * 0.3f32.sin(),
            tx: 40.,
            ty: -12.,
        };
        let src = ARCFACE_112;
        let dst = src.map(|p| truth.apply(p));
        let m = Similarity::estimate(&src, &dst);
        assert!((m.a - truth.a).abs() < 1e-4 && (m.b - truth.b).abs() < 1e-4);
        assert!((m.tx - truth.tx).abs() < 1e-2 && (m.ty - truth.ty).abs() < 1e-2);
        for (s, d) in src.iter().zip(&dst) {
            assert!(close(m.apply(*s), *d));
        }
        let back = m.inverse();
        for (s, d) in src.iter().zip(&dst) {
            assert!(close(back.apply(*d), *s));
        }
    }

    #[test]
    fn templates_sit_in_their_crops() {
        // 128: the 112 face shifted 8 px right; 112 multiples scale.
        assert!(close(arcface_template(128)[0], [46.2946, 51.6963]));
        assert!(close(arcface_template(224)[2], [112.0504, 143.4732]));
        assert!(close(ffhq_template(512)[2], [256.634, 314.019]));
    }

    #[test]
    fn input_scale_keeps_the_mapping() {
        let m = Similarity {
            a: 0.5,
            b: 0.1,
            tx: 3.,
            ty: 4.,
        };
        let half = m.with_input_scale(0.5);
        assert!(close(half.apply([50., 20.]), m.apply([100., 40.])));
    }

    /// A crop of a picture made of horizontal stripes follows the stripes
    /// through a 90° turn.
    #[test]
    fn warps_through_a_rotation() {
        let (w, h) = (64u32, 64u32);
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let v = if x < 32 { 255 } else { 0 };
                bgra[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        let f = FrameView {
            width: w,
            height: h,
            bgra: &bgra,
        };
        // Frame → crop: (x, y) → (y, 64 − x), the white left half lands
        // at the bottom.
        let rot = Similarity {
            a: 0.,
            b: -1.,
            tx: 0.,
            ty: 64.,
        };
        let crop = warp_crop(&f, &rot, 64);
        assert_eq!(crop.get_pixel(32, 8).0, [0, 0, 0]);
        assert_eq!(crop.get_pixel(32, 56).0, [255, 255, 255]);
    }
}
