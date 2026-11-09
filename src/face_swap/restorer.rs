//! Optional face restoration: GFPGAN 1.4 (Apache-2.0). The swapper works
//! at 128 px, soft on a big face; this redraws the face at 512 px with
//! sharper detail. It sees the whole head aligned to its own template
//! (RGB, −1…1) and answers in the same space. Slow: export quality only.

use std::path::Path;

use image::RgbImage;

use super::embedder::{from_planar, planar};
use super::error::{FaceSwapError, Result};
use super::inference::Model;

pub const SIZE: u32 = 512;

pub struct Restorer {
    model: Model,
}

impl Restorer {
    pub fn load(path: &Path, gpu: bool, cache: Option<&Path>) -> Result<Restorer> {
        let model = Model::load(path, gpu, cache)?;
        if model.input_hw(0) != Some((SIZE as i64, SIZE as i64)) {
            return Err(FaceSwapError::ModelShape(format!(
                "restorer input {:?}",
                model.inputs.first().map(|p| &p.shape)
            )));
        }
        Ok(Restorer { model })
    }

    /// A 512 px head crop, restored.
    pub fn restore(&mut self, crop: &RgbImage) -> Result<RgbImage> {
        let s = SIZE as i64;
        let out = self.model.run(vec![(
            vec![1, 3, s, s],
            planar(crop, |v| (v / 255. - 0.5) / 0.5),
        )])?;
        let data = out
            .into_iter()
            .next()
            .map(|o| o.data)
            .filter(|d| d.len() == (3 * s * s) as usize)
            .ok_or_else(|| FaceSwapError::ModelShape("restorer output size".into()))?;
        Ok(from_planar(&data, SIZE, |v| {
            (v.clamp(-1., 1.) * 0.5 + 0.5) * 255.
        }))
    }
}
