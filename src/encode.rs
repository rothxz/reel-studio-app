//! Pictures and sound in, an MP4 out: H.264 video (the system's hardware
//! encoder when there is one, else a software one) and AAC stereo sound,
//! through FFmpeg. Export writes the timeline with it; the agent writes
//! its drawn scenes.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use ff::format::{Pixel, Sample, sample};
use ff::software::scaling;
use ff::{ChannelLayout, Dictionary, Rational};
use ffmpeg_next as ff;

/// The H.264 encoders tried in turn: the platform's own (hardware, and
/// part of the OS: fine in an LGPL build), then software ones. MPEG-4
/// part 2 last, which every FFmpeg has and every player opens.
const VIDEO_ENCODERS: &[&str] = &[
    "h264_videotoolbox",
    "h264_mf",
    "libx264",
    "libopenh264",
    "mpeg4",
];

/// AAC encoders: FFmpeg's own, then the system's (macOS).
const AUDIO_ENCODERS: &[&str] = &["aac", "aac_at"];

/// Sound is written at this rate (Hz), stereo.
pub const AUDIO_RATE: u32 = 48_000;
const AUDIO_BITRATE: usize = 128_000;

/// What a picture handed to `push` is laid out as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelOrder {
    Rgba,
    Bgra,
}

/// The video to write.
#[derive(Clone, Debug)]
pub struct VideoOptions {
    /// Even numbers (4:2:0).
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u64,
    pub input: PixelOrder,
    /// With an AAC stereo track at `AUDIO_RATE`.
    pub audio: bool,
}

struct AudioTrack {
    encoder: ff::encoder::Audio,
    stream: usize,
    format: Sample,
    /// Samples per frame the encoder takes.
    frame_size: usize,
    /// Interleaved stereo not yet encoded.
    pending: Vec<f32>,
    next_pts: i64,
}

pub struct Mp4Writer {
    octx: ff::format::context::Output,
    encoder: ff::encoder::Video,
    stream: usize,
    scaler: scaling::Context,
    input: ff::frame::Video,
    output: ff::frame::Video,
    time_base: Rational,
    next_pts: i64,
    width: u32,
    height: u32,
    audio: Option<AudioTrack>,
    /// The video encoder in use (for the log).
    pub codec_name: String,
}

/// The H.264 bitrate for a size and rate at a quality: its base at
/// 1080p30, scaled by pixels per second, kept within 0.5–50 Mbps (the
/// engine's rule, so files match the other apps' exports).
pub fn bitrate_for(width: u32, height: u32, fps: u32, base_mbps_1080p30: f64) -> u64 {
    let scale = (width as f64 * height as f64 * fps as f64) / (1920. * 1080. * 30.);
    (base_mbps_1080p30 * 1_000_000. * scale).clamp(500_000., 50_000_000.) as u64
}

fn open_video(
    name: &str,
    opts: &VideoOptions,
    global_header: bool,
) -> Result<(ff::encoder::Video, ff::Codec, Pixel)> {
    let codec = ff::encoder::find_by_name(name).context("not in this FFmpeg")?;
    // 4:2:0 in the layout the encoder takes.
    let formats: Vec<Pixel> = codec
        .video()
        .ok()
        .and_then(|v| v.formats().map(|f| f.collect()))
        .unwrap_or_default();
    let format = [Pixel::YUV420P, Pixel::NV12]
        .into_iter()
        .find(|f| formats.is_empty() || formats.contains(f))
        .context("no 4:2:0 input")?;
    let mut enc = ff::codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()?;
    let time_base = Rational(1, opts.fps as i32);
    enc.set_width(opts.width);
    enc.set_height(opts.height);
    enc.set_format(format);
    enc.set_time_base(time_base);
    enc.set_frame_rate(Some(Rational(opts.fps as i32, 1)));
    // A keyframe every 2 s: the file seeks well.
    enc.set_gop(opts.fps * 2);
    enc.set_max_b_frames(0);
    enc.set_bit_rate(opts.bitrate as usize);
    enc.set_max_bit_rate(opts.bitrate as usize * 2);
    if global_header {
        enc.set_flags(ff::codec::Flags::GLOBAL_HEADER);
    }
    let mut dict = Dictionary::new();
    match name {
        // Software fallback in a VM / on a Mac without the block.
        "h264_videotoolbox" => {
            dict.set("allow_sw", "1");
            dict.set("profile", "high");
        }
        "libx264" => {
            dict.set("preset", "fast");
            dict.set("profile", "high");
        }
        _ => {}
    }
    let encoder = enc.open_as_with(codec, dict)?;
    Ok((encoder, codec, format))
}

fn open_audio(name: &str, global_header: bool) -> Result<(ff::encoder::Audio, ff::Codec, Sample)> {
    let codec = ff::encoder::find_by_name(name).context("not in this FFmpeg")?;
    let formats: Vec<Sample> = codec
        .audio()
        .ok()
        .and_then(|a| a.formats().map(|f| f.collect()))
        .unwrap_or_default();
    let format = [
        Sample::F32(sample::Type::Planar),
        Sample::F32(sample::Type::Packed),
        Sample::I16(sample::Type::Planar),
        Sample::I16(sample::Type::Packed),
    ]
    .into_iter()
    .find(|f| formats.contains(f))
    .context("no sample format we write")?;
    let mut enc = ff::codec::context::Context::new_with_codec(codec)
        .encoder()
        .audio()?;
    enc.set_rate(AUDIO_RATE as i32);
    enc.set_format(format);
    enc.set_channel_layout(ChannelLayout::STEREO);
    enc.set_bit_rate(AUDIO_BITRATE);
    enc.set_time_base(Rational(1, AUDIO_RATE as i32));
    if global_header {
        enc.set_flags(ff::codec::Flags::GLOBAL_HEADER);
    }
    let encoder = enc.open_as_with(codec, Dictionary::new())?;
    Ok((encoder, codec, format))
}

impl Mp4Writer {
    /// A new MP4 at `path` (whatever its extension).
    pub fn create(path: &Path, opts: &VideoOptions) -> Result<Self> {
        let (w, h) = (opts.width, opts.height);
        if !w.is_multiple_of(2) || !h.is_multiple_of(2) || w == 0 || h == 0 {
            bail!("a video needs an even size, not {w}x{h}");
        }
        if opts.fps == 0 {
            bail!("a video needs a frame rate");
        }
        let mut octx = ff::format::output_as(path, "mp4")
            .with_context(|| format!("can't write {}", path.display()))?;
        let global = octx
            .format()
            .flags()
            .contains(ff::format::Flags::GLOBAL_HEADER);

        let mut tried = vec![];
        let mut opened = None;
        for name in VIDEO_ENCODERS {
            match open_video(name, opts, global) {
                Ok(o) => {
                    opened = Some(o);
                    break;
                }
                Err(e) => tried.push(format!("{name}: {e:#}")),
            }
        }
        let Some((encoder, codec, format)) = opened else {
            bail!(
                "this build has no video encoder that works here ({})",
                tried.join("; ")
            );
        };
        let codec_name = codec.name().to_string();
        let mut ost = octx.add_stream(codec)?;
        let stream = ost.index();
        ost.set_parameters(&encoder);
        ost.set_time_base(Rational(1, opts.fps as i32));

        let audio = if opts.audio {
            let mut opened = None;
            let mut tried = vec![];
            for name in AUDIO_ENCODERS {
                match open_audio(name, global) {
                    Ok(o) => {
                        opened = Some(o);
                        break;
                    }
                    Err(e) => tried.push(format!("{name}: {e:#}")),
                }
            }
            let Some((encoder, codec, format)) = opened else {
                bail!(
                    "this build has no AAC encoder that works here ({})",
                    tried.join("; ")
                );
            };
            let mut ost = octx.add_stream(codec)?;
            let stream = ost.index();
            ost.set_parameters(&encoder);
            ost.set_time_base(Rational(1, AUDIO_RATE as i32));
            let frame_size = (encoder.frame_size() as usize).max(1);
            Some(AudioTrack {
                encoder,
                stream,
                format,
                frame_size: if frame_size <= 1 { 1024 } else { frame_size },
                pending: vec![],
                next_pts: 0,
            })
        } else {
            None
        };

        // The index at the front, so players start before it's all read.
        let mut header = Dictionary::new();
        header.set("movflags", "+faststart");
        octx.write_header_with(header)?;

        let in_format = match opts.input {
            PixelOrder::Rgba => Pixel::RGBA,
            PixelOrder::Bgra => Pixel::BGRA,
        };
        let scaler =
            scaling::Context::get(in_format, w, h, format, w, h, scaling::Flags::BILINEAR)?;
        Ok(Mp4Writer {
            octx,
            encoder,
            stream,
            scaler,
            input: ff::frame::Video::new(in_format, w, h),
            output: ff::frame::Video::new(format, w, h),
            time_base: Rational(1, opts.fps as i32),
            next_pts: 0,
            width: w,
            height: h,
            audio,
            codec_name,
        })
    }

    /// Adds the next opaque picture (`width × height × 4` bytes in the
    /// order given at creation).
    pub fn push(&mut self, pixels: &[u8]) -> Result<()> {
        let row = self.width as usize * 4;
        if pixels.len() != row * self.height as usize {
            bail!("a frame of the wrong size");
        }
        let stride = self.input.stride(0);
        let data = self.input.data_mut(0);
        for y in 0..self.height as usize {
            data[y * stride..y * stride + row].copy_from_slice(&pixels[y * row..(y + 1) * row]);
        }
        self.scaler.run(&self.input, &mut self.output)?;
        self.output.set_pts(Some(self.next_pts));
        self.next_pts += 1;
        self.encoder.send_frame(&self.output)?;
        self.drain_video()
    }

    /// Adds sound after what's written: interleaved stereo at
    /// `AUDIO_RATE`, -1…1.
    pub fn push_audio(&mut self, stereo: &[[f32; 2]]) -> Result<()> {
        let Some(track) = &mut self.audio else {
            return Ok(());
        };
        track.pending.extend(
            stereo
                .iter()
                .flat_map(|[l, r]| [l.clamp(-1., 1.), r.clamp(-1., 1.)]),
        );
        let frames = track.pending.len() / (track.frame_size * 2);
        for _ in 0..frames {
            self.encode_audio_frame(false)?;
        }
        Ok(())
    }

    /// Encodes one frame of pending sound (the last one padded with
    /// silence).
    fn encode_audio_frame(&mut self, last: bool) -> Result<()> {
        let Some(track) = &mut self.audio else {
            return Ok(());
        };
        let n = track.frame_size;
        let mut take: Vec<f32> = track
            .pending
            .drain(..(n * 2).min(track.pending.len()))
            .collect();
        if take.is_empty() && last {
            return Ok(());
        }
        take.resize(n * 2, 0.);
        let mut frame = ff::frame::Audio::new(track.format, n, ChannelLayout::STEREO);
        frame.set_rate(AUDIO_RATE);
        match track.format {
            Sample::F32(sample::Type::Planar) => {
                for c in 0..2 {
                    let plane = frame.plane_mut::<f32>(c);
                    for (i, s) in plane.iter_mut().take(n).enumerate() {
                        *s = take[i * 2 + c];
                    }
                }
            }
            Sample::F32(sample::Type::Packed) => {
                let data = frame.data_mut(0);
                for (i, s) in take.iter().enumerate() {
                    data[i * 4..i * 4 + 4].copy_from_slice(&s.to_ne_bytes());
                }
            }
            Sample::I16(sample::Type::Planar) => {
                for c in 0..2 {
                    let plane = frame.plane_mut::<i16>(c);
                    for (i, s) in plane.iter_mut().take(n).enumerate() {
                        *s = (take[i * 2 + c] * 32767.) as i16;
                    }
                }
            }
            _ => {
                let data = frame.data_mut(0);
                for (i, s) in take.iter().enumerate() {
                    let v = (s * 32767.) as i16;
                    data[i * 2..i * 2 + 2].copy_from_slice(&v.to_ne_bytes());
                }
            }
        }
        frame.set_pts(Some(track.next_pts));
        track.next_pts += n as i64;
        track.encoder.send_frame(&frame)?;
        self.drain_audio()
    }

    fn drain_video(&mut self) -> Result<()> {
        let out_tb = self
            .octx
            .stream(self.stream)
            .map_or(self.time_base, |s| s.time_base());
        let mut packet = ff::Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(self.stream);
            packet.rescale_ts(self.time_base, out_tb);
            packet.write_interleaved(&mut self.octx)?;
        }
        Ok(())
    }

    fn drain_audio(&mut self) -> Result<()> {
        let Some(track) = &mut self.audio else {
            return Ok(());
        };
        let tb = Rational(1, AUDIO_RATE as i32);
        let out_tb = self.octx.stream(track.stream).map_or(tb, |s| s.time_base());
        let mut packet = ff::Packet::empty();
        while track.encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(track.stream);
            packet.rescale_ts(tb, out_tb);
            packet.write_interleaved(&mut self.octx)?;
        }
        Ok(())
    }

    /// Flushes the encoders and closes the file.
    pub fn finish(mut self) -> Result<()> {
        if self.audio.is_some() {
            self.encode_audio_frame(true)?;
            if let Some(track) = &mut self.audio {
                track.encoder.send_eof()?;
            }
            self.drain_audio()?;
        }
        self.encoder.send_eof()?;
        self.drain_video()?;
        self.octx.write_trailer()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrate_follows_size_and_rate() {
        // The base at 1080p30 (portrait or not), scaled, clamped.
        assert_eq!(bitrate_for(1080, 1920, 30, 8.), 8_000_000);
        assert_eq!(bitrate_for(2160, 3840, 30, 16.), 50_000_000);
        assert_eq!(bitrate_for(720, 1280, 30, 2.5), 1_111_111);
        assert_eq!(bitrate_for(64, 64, 24, 2.5), 500_000);
    }

    /// Writes a short clip with sound and reads it back.
    #[test]
    fn writes_video_and_sound() {
        crate::decoder::init();
        let dir = std::env::temp_dir().join(format!("reel-encode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clip.mp4.part");
        let (w, h, fps) = (64, 36, 10);
        let mut writer = Mp4Writer::create(
            &path,
            &VideoOptions {
                width: w,
                height: h,
                fps,
                bitrate: 500_000,
                input: PixelOrder::Bgra,
                audio: true,
            },
        )
        .unwrap();
        let per_frame = (AUDIO_RATE / fps) as usize;
        for i in 0..20u32 {
            let shade = (i * 12) as u8;
            writer
                .push(&[shade, 0, 255 - shade, 255].repeat((w * h) as usize))
                .unwrap();
            let tone: Vec<[f32; 2]> = (0..per_frame)
                .map(|s| {
                    let v = ((s as f32 / AUDIO_RATE as f32) * 440. * std::f32::consts::TAU).sin();
                    [v * 0.5, v * 0.5]
                })
                .collect();
            writer.push_audio(&tone).unwrap();
        }
        writer.finish().unwrap();
        let input = ff::format::input(&path).unwrap();
        let secs = input.duration() as f64 / ff::ffi::AV_TIME_BASE as f64;
        assert!((secs - 2.).abs() < 0.15, "{secs}");
        assert!(input.streams().best(ff::media::Type::Video).is_some());
        assert!(input.streams().best(ff::media::Type::Audio).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
