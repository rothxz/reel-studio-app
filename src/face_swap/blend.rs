//! Putting a swapped face back: the soft mask it's pasted through, the
//! colour match that keeps it in the frame's light, and the paste itself.
//!
//! The mask lives in the aligned crop, where every frame's face sits at
//! the same place, so it's the same shape on every frame (no per-frame
//! segmentation to flicker): a box with its edges feathered.

use image::RgbImage;

use super::align::{Similarity, sample_rgb};

/// A feathered box in a square crop: `pad` insets each side (top, right,
/// bottom, left, fractions of the crop), `blur` is the feather's width
/// (fraction of half the crop).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaskShape {
    pub pad: [f32; 4],
    pub blur: f32,
}

impl MaskShape {
    /// The swapper's crop, feathered over 30 %: solid only on the middle
    /// of the face. The colour match reads its edge; a restored head crop
    /// (which reaches past the face) is pasted through it.
    pub const SWAP: MaskShape = MaskShape {
        pad: [0.; 4],
        blur: 0.3,
    };

    /// The swapped face pasted back: feathered over 16 %, so the cheeks,
    /// jaw and chin are the swapped face's too (with the wider feather they
    /// stayed the target's, and the face read as theirs).
    pub const FACE: MaskShape = MaskShape {
        pad: [0.; 4],
        blur: 0.16,
    };

    /// 0–1 at crop point `q` (pixels) of a `size` px crop.
    #[inline]
    pub fn at(&self, [x, y]: [f32; 2], size: f32) -> f32 {
        let feather = (size * 0.5 * self.blur).max(1.);
        // The box's own edge sits half a feather in (the frame around a
        // crop is never pasted).
        let inset = feather * 0.5;
        let [top, right, bottom, left] = self.pad.map(|p| p * size + inset);
        let ramp = |d: f32| {
            let t = (d / feather).clamp(0., 1.);
            t * t * (3. - 2. * t)
        };
        ramp(x - left) * ramp(size - 1. - right - x) * ramp(y - top) * ramp(size - 1. - bottom - y)
    }
}

/// The colour shift (RGB, added) that brings `swapped` back to the light
/// of `target` (both the same aligned crop): measured where the mask
/// fades out, around the face, so the swapped person keeps their own skin
/// tone and only the model's drift in overall colour is undone. Kept
/// within ±`limit`.
pub fn color_offset(
    swapped: &RgbImage,
    target: &RgbImage,
    mask: &MaskShape,
    limit: f32,
) -> [f32; 3] {
    let size = swapped.width();
    debug_assert_eq!(size, target.width());
    let (mut sum, mut weight) = ([0f64; 3], 0f64);
    let (s, t) = (swapped.as_raw(), target.as_raw());
    for y in 0..size {
        for x in 0..size {
            let m = mask.at([x as f32, y as f32], size as f32);
            // Around the face: the part of the crop the mask lets through
            // only partly.
            let w = (m * (1. - m) * 4.) as f64;
            if w <= 0.01 {
                continue;
            }
            let i = ((y * size + x) * 3) as usize;
            for c in 0..3 {
                sum[c] += w * (t[i + c] as f64 - s[i + c] as f64);
            }
            weight += w;
        }
    }
    if weight <= 0. {
        return [0.; 3];
    }
    sum.map(|v| ((v / weight) as f32).clamp(-limit, limit))
}

/// `img` with `offset` added to every pixel.
pub fn shift_colors(img: &mut RgbImage, offset: [f32; 3]) {
    if offset == [0.; 3] {
        return;
    }
    for p in img.pixels_mut() {
        for (v, o) in p.0.iter_mut().zip(offset) {
            *v = (*v as f32 + o).round().clamp(0., 255.) as u8;
        }
    }
}

/// A face to paste: `m` takes frame pixels to crop pixels.
pub struct Paste<'a> {
    pub m: Similarity,
    pub crop: &'a RgbImage,
    pub mask: MaskShape,
    /// 0–1: how much of the swapped face shows.
    pub opacity: f32,
}

/// Pastes `p` into a BGRA frame `w` × `h`, through its mask. Only the
/// pixels under the crop are touched.
pub fn composite(bgra: &mut [u8], w: u32, h: u32, p: &Paste) {
    if p.opacity <= 0. {
        return;
    }
    let size = p.crop.width() as f32;
    let inv = p.m.inverse();
    let corners = [[0., 0.], [size, 0.], [0., size], [size, size]].map(|q| inv.apply(q));
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for [x, y] in corners {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let xs = (x0.floor().max(0.) as u32)..(x1.ceil().min(w as f32) as u32);
    let ys = (y0.floor().max(0.) as u32)..(y1.ceil().min(h as f32) as u32);
    for y in ys {
        for x in xs.clone() {
            let q = p.m.apply([x as f32, y as f32]);
            if q[0] < 0. || q[1] < 0. || q[0] > size - 1. || q[1] > size - 1. {
                continue;
            }
            let a = p.mask.at(q, size) * p.opacity;
            if a <= 0.002 {
                continue;
            }
            let rgb = sample_rgb(p.crop, q[0], q[1]);
            let i = ((y * w + x) * 4) as usize;
            // BGRA out.
            for (c, ch) in [(2usize, 0usize), (1, 1), (0, 2)] {
                let old = bgra[i + ch] as f32;
                bgra[i + ch] = (old + (rgb[c] - old) * a).round().clamp(0., 255.) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_is_solid_inside_and_clear_at_the_edge() {
        let m = MaskShape::SWAP;
        assert!((m.at([64., 64.], 128.) - 1.).abs() < 1e-6);
        assert_eq!(m.at([0., 64.], 128.), 0.);
        assert_eq!(m.at([127., 64.], 128.), 0.);
        // Rising monotonically from the edge in.
        let mut last = 0.;
        for x in 0..64 {
            let v = m.at([x as f32, 64.], 128.);
            assert!(v >= last - 1e-6);
            last = v;
        }
        // Same shape at any crop size.
        assert!((m.at([16., 64.], 128.) - m.at([64., 256.], 512.)).abs() < 1e-5);
        // Padding moves the edge in.
        let padded = MaskShape {
            pad: [0., 0., 0.25, 0.],
            blur: 0.3,
        };
        assert_eq!(padded.at([64., 110.], 128.), 0.);
    }

    #[test]
    fn color_offset_reads_the_ring_and_is_clamped() {
        let target = RgbImage::from_pixel(64, 64, image::Rgb([120, 100, 90]));
        let swapped = RgbImage::from_pixel(64, 64, image::Rgb([110, 104, 90]));
        let o = color_offset(&swapped, &target, &MaskShape::SWAP, 30.);
        assert!((o[0] - 10.).abs() < 1e-3 && (o[1] + 4.).abs() < 1e-3 && o[2].abs() < 1e-3);
        let dark = RgbImage::from_pixel(64, 64, image::Rgb([0, 0, 0]));
        let o = color_offset(&dark, &target, &MaskShape::SWAP, 30.);
        assert_eq!(o, [30., 30., 30.]);
        let mut fixed = swapped.clone();
        shift_colors(&mut fixed, [10., -4., 0.]);
        assert_eq!(fixed.get_pixel(5, 5).0, [120, 100, 90]);
    }

    /// A white crop pasted at 2× over a black frame: full in the middle,
    /// nothing outside the crop, a soft edge, and BGRA order kept.
    #[test]
    fn pastes_through_the_mask() {
        let (w, h) = (200u32, 200u32);
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        for px in bgra.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        let crop = RgbImage::from_pixel(64, 64, image::Rgb([255, 128, 0]));
        // Frame → crop: the crop covers frame 36..164 at half scale.
        let m = Similarity {
            a: 0.5,
            b: 0.,
            tx: -18.,
            ty: -18.,
        };
        composite(
            &mut bgra,
            w,
            h,
            &Paste {
                m,
                crop: &crop,
                mask: MaskShape::SWAP,
                opacity: 1.,
            },
        );
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [bgra[i], bgra[i + 1], bgra[i + 2], bgra[i + 3]]
        };
        assert_eq!(at(100, 100), [0, 128, 255, 255]);
        assert_eq!(at(10, 10), [0, 0, 0, 255]);
        let edge = at(50, 100)[2];
        assert!(edge > 0 && edge < 255, "soft edge, got {edge}");
    }
}
