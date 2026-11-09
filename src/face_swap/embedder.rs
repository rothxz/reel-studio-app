//! Face identity: ArcFace (ResNet-50, 512-d). A face aligned to the 112 px
//! template goes in (RGB, scaled to −1…1), a vector comes out; the
//! closer two vectors point, the likelier the same person. Used for the
//! swapper's source identity and to keep tracking the chosen person.

use std::path::Path;

use image::RgbImage;

use super::align::{Similarity, arcface_template, warp_crop};
use super::error::{FaceSwapError, Result};
use super::inference::Model;
use super::types::{Embedding, FaceLandmarks, FrameView};

pub const SIZE: u32 = 112;

pub struct Embedder {
    model: Model,
}

impl Embedder {
    pub fn load(path: &Path, gpu: bool, cache: Option<&Path>) -> Result<Embedder> {
        let model = Model::load(path, gpu, cache)?;
        if model.input_hw(0) != Some((SIZE as i64, SIZE as i64)) {
            return Err(FaceSwapError::ModelShape(format!(
                "embedder input {:?}",
                model.inputs.first().map(|p| &p.shape)
            )));
        }
        Ok(Embedder { model })
    }

    /// The identity of the face at `landmarks` in `frame`.
    pub fn embed(&mut self, frame: &FrameView, landmarks: &FaceLandmarks) -> Result<Embedding> {
        let m = Similarity::align(landmarks, &arcface_template(SIZE));
        self.embed_crop(&warp_crop(frame, &m, SIZE))
    }

    /// The identity of an aligned 112 px crop.
    pub fn embed_crop(&mut self, crop: &RgbImage) -> Result<Embedding> {
        let input = planar(crop, |v| (v - 127.5) / 127.5);
        let s = SIZE as i64;
        let out = self.model.run(vec![(vec![1, 3, s, s], input)])?;
        let v = out
            .into_iter()
            .next()
            .map(|o| o.data)
            .filter(|d| d.len() == 512)
            .ok_or_else(|| FaceSwapError::ModelShape("embedding isn't 512 long".into()))?;
        Ok(Embedding::normalized(v))
    }
}

/// An RGB crop as planar CHW floats through `f`.
pub fn planar(img: &RgbImage, f: impl Fn(f32) -> f32) -> Vec<f32> {
    let plane = (img.width() * img.height()) as usize;
    let mut out = vec![0f32; plane * 3];
    for (i, p) in img.pixels().enumerate() {
        for c in 0..3 {
            out[c * plane + i] = f(p.0[c] as f32);
        }
    }
    out
}

/// Planar CHW floats through `f` back into an RGB picture.
pub fn from_planar(data: &[f32], size: u32, f: impl Fn(f32) -> f32) -> RgbImage {
    let plane = (size * size) as usize;
    let mut img = RgbImage::new(size, size);
    for (i, p) in img.pixels_mut().enumerate() {
        for c in 0..3 {
            p.0[c] = f(data[c * plane + i]).round().clamp(0., 255.) as u8;
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planar_round_trips() {
        let mut img = RgbImage::new(2, 2);
        img.put_pixel(1, 0, image::Rgb([255, 0, 128]));
        let p = planar(&img, |v| v / 255.);
        assert_eq!(p.len(), 12);
        assert_eq!(p[1], 1.); // R plane, pixel 1
        assert_eq!(p[4 + 1], 0.); // G plane
        assert!((p[8 + 1] - 128. / 255.).abs() < 1e-6);
        let back = from_planar(&p, 2, |v| v * 255.);
        assert_eq!(back, img);
    }
}
