//! Pictures in, a video file out: the agent's generated scenes are written
//! as H.264 in an MP4 (see `encode`), so they sit on the timeline like any
//! imported clip and export with it.

use std::path::Path;

use anyhow::Result;

use crate::encode::{Mp4Writer, PixelOrder, VideoOptions};

pub struct Writer(Mp4Writer);

impl Writer {
    /// A new file at `path` (`.mp4`) of `width`×`height` (even numbers) at
    /// `fps` frames a second.
    pub fn create(path: &Path, width: u32, height: u32, fps: u32) -> Result<Self> {
        Mp4Writer::create(
            path,
            &VideoOptions {
                width,
                height,
                fps,
                // Flat graphics compress well; this keeps edges clean at 1080p.
                bitrate: 10_000_000,
                input: PixelOrder::Rgba,
                audio: false,
                comment: None,
            },
        )
        .map(Writer)
    }

    /// Adds one opaque RGBA picture (`width × height × 4` bytes).
    pub fn push(&mut self, rgba: &[u8]) -> Result<()> {
        self.0.push(rgba)
    }

    /// Flushes the encoder and closes the file.
    pub fn finish(self) -> Result<()> {
        self.0.finish()
    }
}
