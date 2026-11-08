//! Pictures in, a video file out: the agent's generated scenes are written
//! as H.264 in an MP4 (the hardware encoder when the system has it, else
//! x264), so they sit on the timeline like any imported clip and export
//! with it.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use ff::format::Pixel;
use ff::software::scaling;
use ff::{Dictionary, Rational};
use ffmpeg_next as ff;

pub struct Writer {
    octx: ff::format::context::Output,
    encoder: ff::encoder::Video,
    scaler: scaling::Context,
    rgba: ff::frame::Video,
    yuv: ff::frame::Video,
    stream: usize,
    time_base: Rational,
    next_pts: i64,
    width: u32,
    height: u32,
}

impl Writer {
    /// A new file at `path` (`.mp4`) of `width`×`height` (even numbers) at
    /// `fps` frames a second.
    pub fn create(path: &Path, width: u32, height: u32, fps: u32) -> Result<Self> {
        if !width.is_multiple_of(2) || !height.is_multiple_of(2) || width == 0 || height == 0 {
            bail!("a video needs an even size, not {width}x{height}");
        }
        let mut octx = ff::format::output(path).context("can't write the scene file")?;
        let global = octx
            .format()
            .flags()
            .contains(ff::format::Flags::GLOBAL_HEADER);
        let (codec, software) = match ff::encoder::find_by_name("h264_videotoolbox") {
            Some(c) => (c, false),
            None => (
                ff::encoder::find_by_name("libx264")
                    .or_else(|| ff::encoder::find(ff::codec::Id::H264))
                    .context("this FFmpeg has no H.264 encoder")?,
                true,
            ),
        };
        let mut ost = octx.add_stream(codec)?;
        let stream = ost.index();
        let mut enc = ff::codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()?;
        let time_base = Rational(1, fps as i32);
        enc.set_width(width);
        enc.set_height(height);
        enc.set_format(Pixel::YUV420P);
        enc.set_time_base(time_base);
        enc.set_frame_rate(Some(Rational(fps as i32, 1)));
        enc.set_gop(fps * 2);
        enc.set_max_b_frames(0);
        // Flat graphics compress well; this keeps edges clean at 1080p.
        enc.set_bit_rate(10_000_000);
        if global {
            enc.set_flags(ff::codec::Flags::GLOBAL_HEADER);
        }
        let mut opts = Dictionary::new();
        if software {
            opts.set("preset", "veryfast");
            opts.set("crf", "18");
        } else {
            opts.set("allow_sw", "1");
        }
        let encoder = enc
            .open_as_with(codec, opts)
            .context("can't open the H.264 encoder")?;
        ost.set_parameters(&encoder);
        ost.set_time_base(time_base);
        octx.write_header()?;
        let scaler = scaling::Context::get(
            Pixel::RGBA,
            width,
            height,
            Pixel::YUV420P,
            width,
            height,
            scaling::Flags::BILINEAR,
        )?;
        Ok(Writer {
            octx,
            encoder,
            scaler,
            rgba: ff::frame::Video::new(Pixel::RGBA, width, height),
            yuv: ff::frame::Video::new(Pixel::YUV420P, width, height),
            stream,
            time_base,
            next_pts: 0,
            width,
            height,
        })
    }

    /// Adds one opaque RGBA picture (`width × height × 4` bytes).
    pub fn push(&mut self, rgba: &[u8]) -> Result<()> {
        let row = self.width as usize * 4;
        if rgba.len() != row * self.height as usize {
            bail!("a frame of the wrong size");
        }
        let stride = self.rgba.stride(0);
        let data = self.rgba.data_mut(0);
        for y in 0..self.height as usize {
            data[y * stride..y * stride + row].copy_from_slice(&rgba[y * row..(y + 1) * row]);
        }
        self.scaler.run(&self.rgba, &mut self.yuv)?;
        self.yuv.set_pts(Some(self.next_pts));
        self.next_pts += 1;
        self.encoder.send_frame(&self.yuv)?;
        self.drain()
    }

    fn drain(&mut self) -> Result<()> {
        let out_tb = self.octx.stream(self.stream).map(|s| s.time_base());
        let out_tb = out_tb.unwrap_or(self.time_base);
        let mut packet = ff::Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(self.stream);
            packet.rescale_ts(self.time_base, out_tb);
            packet.write_interleaved(&mut self.octx)?;
        }
        Ok(())
    }

    /// Flushes the encoder and closes the file.
    pub fn finish(mut self) -> Result<()> {
        self.encoder.send_eof()?;
        self.drain()?;
        self.octx.write_trailer()?;
        Ok(())
    }
}
