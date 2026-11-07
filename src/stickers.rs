//! Sticker and GIF clips' pictures for the player: the file downloaded
//! (cached by URL), every frame decoded with its delay, premultiplied
//! (the compositor's textures are) and kept at most `MAX_SIDE` px a side.
//! An animation loops over the clip's length.

use std::sync::Arc;

use anyhow::{Result, bail};
use image::imageops::FilterType;

use crate::assets;
use crate::decoder::DecodedFrame;
use crate::model::Micros;

/// Bigger stickers are scaled down (they're drawn at most ~40 % of a
/// preview-sized canvas).
const MAX_SIDE: u32 = 480;
/// Longer animations keep their first frames only (memory).
const MAX_FRAMES: usize = 150;
/// A frame with no delay of its own shows this long (browsers do the same).
const DEFAULT_DELAY: Micros = 100_000;

pub struct StickerFrames {
    /// Each frame and when it ends, from the start of the loop.
    frames: Vec<(Micros, Arc<DecodedFrame>)>,
    total: Micros,
    /// The first frame with straight alpha (what GPUI's images want), for
    /// the timeline.
    pub thumb: (u32, u32, Vec<u8>),
}

impl StickerFrames {
    /// The frame shown `offset` into the clip (looping).
    pub fn at(&self, offset: Micros) -> Arc<DecodedFrame> {
        if self.frames.len() == 1 || self.total <= 0 {
            return self.frames[0].1.clone();
        }
        let t = offset.rem_euclid(self.total);
        let i = self.frames.partition_point(|(end, _)| *end <= t);
        self.frames[i.min(self.frames.len() - 1)].1.clone()
    }
}

/// Downloads (or reads the cached copy of) `url` and decodes it.
/// Blocking: run it off the UI thread.
pub fn load(url: &str) -> Result<StickerFrames> {
    let bytes = assets::fetch(assets::REMOTE, url)?;
    let decoded = assets::decode(&bytes, true)?;
    let mut frames = Vec::with_capacity(decoded.len().min(MAX_FRAMES));
    let mut thumb = None;
    let mut end: Micros = 0;
    for (i, frame) in decoded.into_iter().take(MAX_FRAMES).enumerate() {
        let (num, den) = frame.delay().numer_denom_ms();
        let delay = if den == 0 || num == 0 {
            DEFAULT_DELAY
        } else {
            // GIFs saying 0–10 ms mean "as fast as you like": browsers use 100 ms.
            let micros = num as i64 * 1000 / den as i64;
            if micros <= 10_000 {
                DEFAULT_DELAY
            } else {
                micros
            }
        };
        end += delay;
        let mut buffer = frame.into_buffer();
        let (w, h) = buffer.dimensions();
        if w.max(h) > MAX_SIDE {
            let k = MAX_SIDE as f32 / w.max(h) as f32;
            let (nw, nh) = (
                ((w as f32 * k).round() as u32).max(1),
                ((h as f32 * k).round() as u32).max(1),
            );
            buffer = image::imageops::resize(&buffer, nw, nh, FilterType::Triangle);
        }
        let (width, height) = buffer.dimensions();
        let mut bgra = buffer.into_raw();
        if thumb.is_none() {
            thumb = Some((width, height, bgra.clone()));
        }
        premultiply(&mut bgra);
        frames.push((
            end,
            Arc::new(DecodedFrame {
                source: url.to_string(),
                key: format!("sticker:{url}#{i}"),
                pts: end - delay,
                width,
                height,
                bgra,
            }),
        ));
    }
    let Some(thumb) = thumb else {
        bail!("no frames");
    };
    Ok(StickerFrames {
        frames,
        total: end,
        thumb,
    })
}

/// Straight → premultiplied alpha (channel order doesn't matter).
fn premultiply(px: &mut [u8]) {
    for p in px.as_chunks_mut::<4>().0 {
        let a = p[3] as u16;
        if a == 255 {
            continue;
        }
        for c in &mut p[..3] {
            *c = ((*c as u16 * a + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(end: Micros) -> (Micros, Arc<DecodedFrame>) {
        (
            end,
            Arc::new(DecodedFrame {
                source: String::new(),
                key: end.to_string(),
                pts: 0,
                width: 1,
                height: 1,
                bgra: vec![0; 4],
            }),
        )
    }

    #[test]
    fn frames_loop_over_time() {
        let s = StickerFrames {
            frames: vec![frame(100_000), frame(200_000), frame(300_000)],
            total: 300_000,
            thumb: (1, 1, vec![0; 4]),
        };
        assert_eq!(s.at(0).key, "100000");
        assert_eq!(s.at(150_000).key, "200000");
        assert_eq!(s.at(299_999).key, "300000");
        assert_eq!(s.at(300_000).key, "100000");
        assert_eq!(s.at(1_050_000).key, "200000");
    }

    #[test]
    fn premultiplies() {
        let mut px = [200, 100, 50, 128, 10, 20, 30, 255];
        premultiply(&mut px);
        assert_eq!(px, [100, 50, 25, 128, 10, 20, 30, 255]);
    }
}
