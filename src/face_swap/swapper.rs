//! The face swap model, behind a trait so another one-shot swapper can
//! take its place (the pipeline only needs: the crop it works on, the
//! template the crop is aligned to, and how a source identity becomes its
//! input).
//!
//! The one shipped: inswapper 128 (InsightFace). It takes the target face
//! aligned to a 128 px crop (RGB 0–1) and the source identity as its own
//! 512-d latent, the ArcFace embedding times the model's `emap` matrix
//! (stored in the model file), normalized; it gives the crop back with
//! the source's face. It has no strength input: how much of it shows is
//! the blend at paste time.

use std::path::Path;

use image::RgbImage;

use super::align::arcface_template;
use super::embedder::{from_planar, planar};
use super::error::{FaceSwapError, Result};
use super::inference::Model;
use super::onnx;
use super::types::{Embedding, Point};

/// The source identity in the swapper's own terms.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceLatent(pub Vec<f32>);

pub trait FaceSwapper: Send {
    /// The aligned crop's side, px.
    fn crop_size(&self) -> u32;
    /// Where the five landmarks sit in that crop.
    fn template(&self) -> [Point; 5];
    fn source_latent(&self, identity: &Embedding) -> SourceLatent;
    /// The crop with the source's face.
    fn swap(&mut self, target: &RgbImage, source: &SourceLatent) -> Result<RgbImage>;
}

pub struct Inswapper {
    model: Model,
    /// 512 × 512, row-major.
    emap: Vec<f32>,
}

pub const INSWAPPER_SIZE: u32 = 128;

impl Inswapper {
    pub fn load(path: &Path, gpu: bool, cache: Option<&Path>) -> Result<Inswapper> {
        let model = Model::load(path, gpu, cache)?;
        let s = INSWAPPER_SIZE as i64;
        let fits = model.inputs.len() == 2
            && model.inputs[0].shape == [1, 3, s, s]
            && model.inputs[1].shape == [1, 512];
        if !fits {
            return Err(FaceSwapError::ModelShape(format!(
                "swapper inputs {:?}",
                model.inputs.iter().map(|p| &p.shape).collect::<Vec<_>>()
            )));
        }
        let emap = onnx::find_initializer(path, &[512, 512])?.data;
        Ok(Inswapper { model, emap })
    }

    pub fn provider(&self) -> super::inference::Provider {
        self.model.provider
    }
}

/// `v` (1 × 512) times `m` (512 × 512), unit length.
pub fn project(v: &[f32], m: &[f32]) -> Vec<f32> {
    let n = v.len();
    let mut out = vec![0f32; n];
    for (i, vi) in v.iter().enumerate() {
        let row = &m[i * n..(i + 1) * n];
        for (o, r) in out.iter_mut().zip(row) {
            *o += vi * r;
        }
    }
    Embedding::normalized(out).0
}

impl FaceSwapper for Inswapper {
    fn crop_size(&self) -> u32 {
        INSWAPPER_SIZE
    }

    fn template(&self) -> [Point; 5] {
        arcface_template(INSWAPPER_SIZE)
    }

    fn source_latent(&self, identity: &Embedding) -> SourceLatent {
        SourceLatent(project(&identity.0, &self.emap))
    }

    fn swap(&mut self, target: &RgbImage, source: &SourceLatent) -> Result<RgbImage> {
        let s = INSWAPPER_SIZE as i64;
        let out = self.model.run(vec![
            (vec![1, 3, s, s], planar(target, |v| v / 255.)),
            (vec![1, 512], source.0.clone()),
        ])?;
        let data = out
            .into_iter()
            .next()
            .map(|o| o.data)
            .filter(|d| d.len() == (3 * s * s) as usize)
            .ok_or_else(|| FaceSwapError::ModelShape("swap output size".into()))?;
        Ok(from_planar(&data, INSWAPPER_SIZE, |v| v * 255.))
    }
}

/// The swap of a crop `k`× the swapper's size: the crop is split into k²
/// interleaved tiles (tile (i, j) holds the crop's pixels (k·x + i,
/// k·y + j)), each a whole face at the swapper's size; each is swapped and
/// woven back. The model only works at its own size, so this is how it
/// gives detail past it (sharper eyes, brows, moustache): k² swaps a frame.
pub fn swap_boosted(
    s: &mut dyn FaceSwapper,
    target: &RgbImage,
    source: &SourceLatent,
    k: u32,
) -> Result<RgbImage> {
    if k <= 1 {
        return s.swap(target, source);
    }
    let n = s.crop_size();
    if target.width() != n * k || target.height() != n * k {
        return Err(FaceSwapError::ModelShape(format!(
            "boosted crop {}×{} isn't {k} × {n}",
            target.width(),
            target.height()
        )));
    }
    let mut out = RgbImage::new(n * k, n * k);
    let mut tile = RgbImage::new(n, n);
    for i in 0..k {
        for j in 0..k {
            for (x, y, p) in tile.enumerate_pixels_mut() {
                *p = *target.get_pixel(x * k + i, y * k + j);
            }
            let swapped = s.swap(&tile, source)?;
            for (x, y, p) in swapped.enumerate_pixels() {
                out.put_pixel(x * k + i, y * k + j, *p);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inverts its crop; counts its calls.
    struct Invert(usize);

    impl FaceSwapper for Invert {
        fn crop_size(&self) -> u32 {
            4
        }
        fn template(&self) -> [Point; 5] {
            [[0.; 2]; 5]
        }
        fn source_latent(&self, _: &Embedding) -> SourceLatent {
            SourceLatent(vec![])
        }
        fn swap(&mut self, target: &RgbImage, _: &SourceLatent) -> Result<RgbImage> {
            assert_eq!(target.width(), 4);
            self.0 += 1;
            let mut out = target.clone();
            for p in out.pixels_mut() {
                p.0 = p.0.map(|v| 255 - v);
            }
            Ok(out)
        }
    }

    /// Every pixel goes through exactly one tile and comes back in place.
    #[test]
    fn boosted_swap_weaves_the_tiles_back() {
        let mut target = RgbImage::new(8, 8);
        for (x, y, p) in target.enumerate_pixels_mut() {
            p.0 = [(x * 8 + y) as u8, x as u8, y as u8];
        }
        let mut s = Invert(0);
        let out = swap_boosted(&mut s, &target, &SourceLatent(vec![]), 2).unwrap();
        assert_eq!(s.0, 4);
        for (x, y, p) in out.enumerate_pixels() {
            assert_eq!(p.0, target.get_pixel(x, y).0.map(|v| 255 - v));
        }
        assert!(swap_boosted(&mut s, &RgbImage::new(6, 6), &SourceLatent(vec![]), 2).is_err());
    }

    #[test]
    fn projects_and_normalizes() {
        // 2-d stand-in: swap the axes and double.
        let m = [0., 2., 2., 0.];
        let p = project(&[1., 0.], &m);
        assert_eq!(p, vec![0., 1.]);
    }
}
