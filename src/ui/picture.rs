//! A big picture that changes every frame (the player's, the export
//! preview's), kept out of the way of gpui's image atlas.
//!
//! The atlas puts an image bigger than 1024 px into a texture of its own
//! size but at least 1024 × 1024, and packs later small images (filmstrip
//! tiles, thumbnails) into the room left over. It never reuses a removed
//! image's room, and frees a texture only once every image in it is gone.
//! So a 720 × 1280 frame dropped after a tile landed beside it pins a
//! 5 MB texture for as long as the tile lives: one more per frame shown.
//!
//! A `FramePicture` is padded to fill its texture exactly (at least
//! 1024 px each way), so nothing else lands in it and dropping it
//! (`Window::drop_image`) frees the texture. It's drawn cropped back to
//! its own pixels.

use std::sync::Arc;

use gpui::{
    AnyElement, Bounds, ContentMask, Corners, IntoElement, ParentElement as _, RenderImage,
    Styled as _, canvas, div, point, px, size,
};

/// The atlas's smallest texture side (gpui's `DEFAULT_ATLAS_SIZE`).
const ATLAS_SIDE: u32 = 1024;

/// A frame's picture: `width` × `height` BGRA pixels at the top left of a
/// padded image.
#[derive(Clone)]
pub struct FramePicture {
    pub image: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
}

impl FramePicture {
    /// From `width` × `height` BGRA rows; None when the sizes don't match.
    pub fn new(width: u32, height: u32, bgra: Vec<u8>) -> Option<Self> {
        let (w, h) = (width as usize, height as usize);
        if w == 0 || h == 0 || bgra.len() != w * h * 4 {
            return None;
        }
        let (pw, ph) = (width.max(ATLAS_SIDE), height.max(ATLAS_SIDE));
        let bytes = if (pw, ph) == (width, height) {
            bgra
        } else {
            let mut padded = vec![0u8; pw as usize * ph as usize * 4];
            for (row, src) in bgra.chunks_exact(w * 4).enumerate() {
                let at = row * pw as usize * 4;
                padded[at..at + w * 4].copy_from_slice(src);
            }
            padded
        };
        let buffer = image::RgbaImage::from_raw(pw, ph, bytes)?;
        Some(FramePicture {
            image: Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])),
            width,
            height,
        })
    }

    /// The padded image's row length in pixels.
    pub fn stride(&self) -> usize {
        self.image.size(0).width.0 as usize
    }

    /// The BGRA pixel at (`x`, `y`) of the picture.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = (y as usize * self.stride() + x as usize) * 4;
        let px = self.image.as_bytes(0)?.get(at..at + 4)?;
        Some([px[0], px[1], px[2], px[3]])
    }

    /// Fills its parent (which must be `relative`), the picture
    /// contain-fitted and centred.
    pub fn element(&self) -> AnyElement {
        let picture = self.clone();
        div()
            .absolute()
            .inset_0()
            .size_full()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let (w, h) = (picture.width as f32, picture.height as f32);
                        let (bw, bh) =
                            (f32::from(bounds.size.width), f32::from(bounds.size.height));
                        let k = (bw / w).min(bh / h);
                        if !k.is_finite() || k <= 0. {
                            return;
                        }
                        let shown = Bounds {
                            origin: point(
                                bounds.origin.x + px((bw - w * k) / 2.),
                                bounds.origin.y + px((bh - h * k) / 2.),
                            ),
                            size: size(px(w * k), px(h * k)),
                        };
                        let full = picture.image.size(0);
                        let whole = Bounds {
                            origin: shown.origin,
                            size: size(px(full.width.0 as f32 * k), px(full.height.0 as f32 * k)),
                        };
                        window.with_content_mask(Some(ContentMask { bounds: shown }), |window| {
                            let _ = window.paint_image(
                                whole,
                                Corners::default(),
                                picture.image.clone(),
                                0,
                                false,
                            );
                        });
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_pictures_are_padded_to_fill_a_texture() {
        let bgra: Vec<u8> = (0..720 * 1280).flat_map(|i| [i as u8, 1, 2, 255]).collect();
        let p = FramePicture::new(720, 1280, bgra).unwrap();
        let s = p.image.size(0);
        assert_eq!((s.width.0, s.height.0), (1024, 1280));
        assert_eq!((p.width, p.height), (720, 1280));
        assert_eq!(p.pixel(5, 0), Some([5, 1, 2, 255]));
        assert_eq!(p.pixel(0, 1), Some([(720u32 % 256) as u8, 1, 2, 255]));
        assert_eq!(p.pixel(720, 0), None);
    }

    #[test]
    fn big_pictures_are_kept_as_they_are() {
        let p = FramePicture::new(1920, 1080, vec![7; 1920 * 1080 * 4]).unwrap();
        let s = p.image.size(0);
        assert_eq!((s.width.0, s.height.0), (1920, 1080));
        assert!(FramePicture::new(4, 4, vec![0; 3]).is_none());
    }
}
