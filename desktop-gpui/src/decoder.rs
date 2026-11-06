//! Video decode for the preview, in-process through FFmpeg (ffmpeg-next).
//!
//! One `FrameServer` per source file owns a decoder on its own thread, so the
//! UI thread never blocks on I/O or decode. Requests are "latest wins": a
//! scrub that outruns the decoder only decodes the frame it ends on. Moving
//! forward a little (playback) decodes on from where it is; anything else
//! seeks to the keyframe before the target and decodes up to it.

use std::sync::mpsc;
use std::thread;

use ff::format::Pixel;
use ff::software::scaling;
use ffmpeg_next as ff;

use crate::model::{MediaItem, Micros, SECOND};

pub struct DecodedFrame {
    pub source: String,
    pub pts: Micros,
    pub width: u32,
    pub height: u32,
    /// Tightly packed BGRA, what gpui's RenderImage wants.
    pub bgra: Vec<u8>,
}

pub fn init() {
    ff::init().expect("FFmpeg initializes");
    ff::util::log::set_level(ff::util::log::Level::Error);
}

/// Reads a file's duration and picture size for the media library.
pub fn probe(path: &str) -> anyhow::Result<MediaItem> {
    let input = ff::format::input(path)?;
    let duration =
        (input.duration() as f64 * SECOND as f64 / ff::ffi::AV_TIME_BASE as f64) as Micros;
    let video = input.streams().best(ff::media::Type::Video);
    let (mut width, mut height) = (0, 0);
    if let Some(stream) = &video {
        let ctx = ff::codec::Context::from_parameters(stream.parameters())?;
        let dec = ctx.decoder().video()?;
        (width, height) = (dec.width(), dec.height());
        if !rotation_of(stream).is_multiple_of(180) {
            std::mem::swap(&mut width, &mut height);
        }
    }
    // A still image has a video stream but no real duration.
    let is_still = video.is_some() && duration <= 0;
    Ok(MediaItem {
        path: path.to_string(),
        name: std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        duration: if is_still {
            3 * SECOND
        } else {
            duration.max(0)
        },
        width,
        height,
        has_video: video.is_some(),
    })
}

/// Clockwise degrees the picture must turn to stand upright (0/90/180/270).
fn rotation_of(stream: &ff::format::stream::Stream) -> u32 {
    for sd in stream.side_data() {
        if sd.kind() != ff::codec::packet::side_data::Type::DisplayMatrix || sd.data().len() < 36 {
            continue;
        }
        let m: Vec<f64> = sd
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .take(2)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64)
            .collect();
        let deg = m[1].atan2(m[0]).to_degrees().round() as i32;
        return (((deg % 360) + 360) % 360) as u32 / 90 * 90;
    }
    0
}

pub struct FrameServer {
    requests: mpsc::Sender<Micros>,
}

impl FrameServer {
    pub fn spawn(source: String, max_width: u32, out: async_channel::Sender<DecodedFrame>) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name(format!(
                "decode {}",
                source.rsplit('/').next().unwrap_or("")
            ))
            .spawn(move || {
                if let Err(e) = serve(&source, max_width, rx, out) {
                    eprintln!("decoder for {source} stopped: {e}");
                }
            })
            .expect("decoder thread starts");
        FrameServer { requests: tx }
    }

    pub fn request(&self, source_time: Micros) {
        let _ = self.requests.send(source_time);
    }
}

struct Decoder {
    input: ff::format::context::Input,
    video: ff::decoder::Video,
    stream_index: usize,
    time_base: f64,
    frame_dur: Micros,
    scaler: Option<(scaling::Context, Pixel, u32, u32)>,
    rotation: u32,
    max_width: u32,
    eof: bool,
}

impl Decoder {
    fn open(path: &str, max_width: u32) -> anyhow::Result<Self> {
        let input = ff::format::input(path)?;
        let stream = input
            .streams()
            .best(ff::media::Type::Video)
            .ok_or_else(|| anyhow::anyhow!("no video stream"))?;
        let mut ctx = ff::codec::Context::from_parameters(stream.parameters())?;
        // Let FFmpeg use every core for software decode.
        if let Ok(threads) = thread::available_parallelism() {
            ctx.set_threading(ff::threading::Config {
                kind: ff::threading::Type::Frame,
                count: threads.get(),
            });
        }
        let video = ctx.decoder().video()?;
        let tb = stream.time_base();
        let rate = stream.avg_frame_rate();
        let fps = if rate.denominator() != 0 && rate.numerator() > 0 {
            rate.numerator() as f64 / rate.denominator() as f64
        } else {
            30.0
        };
        Ok(Decoder {
            stream_index: stream.index(),
            time_base: tb.numerator() as f64 / tb.denominator() as f64,
            frame_dur: (SECOND as f64 / fps) as Micros,
            rotation: rotation_of(&stream),
            input,
            video,
            scaler: None,
            max_width,
            eof: false,
        })
    }

    fn seek(&mut self, t: Micros) -> anyhow::Result<()> {
        // AV_TIME_BASE is microseconds, our unit.
        self.input.seek(t, ..t)?;
        self.video.flush();
        self.eof = false;
        Ok(())
    }

    /// Next decoded frame in presentation order, or None at the end.
    fn next(&mut self) -> Option<(Micros, ff::frame::Video)> {
        let mut frame = ff::frame::Video::empty();
        loop {
            if self.video.receive_frame(&mut frame).is_ok() {
                let pts = frame.timestamp().or(frame.pts()).unwrap_or(0);
                return Some((
                    (pts as f64 * self.time_base * SECOND as f64) as Micros,
                    frame,
                ));
            }
            if self.eof {
                return None;
            }
            let mut got = false;
            for (stream, packet) in self.input.packets() {
                if stream.index() == self.stream_index {
                    let _ = self.video.send_packet(&packet);
                    got = true;
                    break;
                }
            }
            if !got {
                self.eof = true;
                let _ = self.video.send_eof();
            }
        }
    }

    fn frame_bgra(&mut self, frame: &ff::frame::Video) -> anyhow::Result<(u32, u32, Vec<u8>)> {
        let (w, h, fmt) = (frame.width(), frame.height(), frame.format());
        let dw = w.min(self.max_width).max(2) & !1;
        let dh = ((h as u64 * dw as u64 / w as u64) as u32).max(2) & !1;
        let stale =
            !matches!(&self.scaler, Some((_, f, sw, sh)) if *f == fmt && *sw == w && *sh == h);
        if stale {
            let ctx =
                scaling::Context::get(fmt, w, h, Pixel::BGRA, dw, dh, scaling::Flags::BILINEAR)?;
            self.scaler = Some((ctx, fmt, w, h));
        }
        let mut out = ff::frame::Video::empty();
        self.scaler.as_mut().unwrap().0.run(frame, &mut out)?;
        let stride = out.stride(0);
        let row = dw as usize * 4;
        let mut packed = Vec::with_capacity(row * dh as usize);
        for y in 0..dh as usize {
            packed.extend_from_slice(&out.data(0)[y * stride..y * stride + row]);
        }
        Ok(rotate(dw, dh, packed, self.rotation))
    }
}

fn rotate(w: u32, h: u32, src: Vec<u8>, degrees: u32) -> (u32, u32, Vec<u8>) {
    if degrees == 0 {
        return (w, h, src);
    }
    let (w, h) = (w as usize, h as usize);
    let (ow, oh) = if degrees == 180 { (w, h) } else { (h, w) };
    let mut dst = vec![0u8; src.len()];
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match degrees {
                90 => (h - 1 - y, x),
                180 => (w - 1 - x, h - 1 - y),
                _ => (y, w - 1 - x),
            };
            let s = (y * w + x) * 4;
            let d = (ny * ow + nx) * 4;
            dst[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    (ow as u32, oh as u32, dst)
}

fn serve(
    source: &str,
    max_width: u32,
    rx: mpsc::Receiver<Micros>,
    out: async_channel::Sender<DecodedFrame>,
) -> anyhow::Result<()> {
    let mut dec = Decoder::open(source, max_width)?;
    // The frame on screen and the one decoded just after it (we read one
    // past the target to know the target's frame has ended).
    let mut shown: Option<Micros> = None;
    let mut lookahead: Option<(Micros, ff::frame::Video)> = None;

    while let Ok(mut target) = rx.recv() {
        while let Ok(newer) = rx.try_recv() {
            target = newer;
        }
        let close_ahead = shown.is_some_and(|s| target >= s && target - s < 2 * SECOND);
        if shown.is_some_and(|s| target >= s && target < s + dec.frame_dur)
            && lookahead.as_ref().is_none_or(|(p, _)| target < *p)
        {
            continue; // still the frame on screen
        }
        if !close_ahead {
            dec.seek(target)?;
            lookahead = None;
        }
        // Walk forward to the last frame starting at or before the target.
        let mut best = lookahead.take();
        while let Some((pts, frame)) = dec.next() {
            if pts > target + dec.frame_dur / 2 {
                if best.is_none() {
                    best = Some((pts, frame)); // target before the first frame
                } else {
                    lookahead = Some((pts, frame));
                }
                break;
            }
            best = Some((pts, frame));
        }
        let Some((pts, frame)) = best else { continue };
        let (width, height, bgra) = dec.frame_bgra(&frame)?;
        shown = Some(pts);
        if out
            .send_blocking(DecodedFrame {
                source: source.to_string(),
                pts,
                width,
                height,
                bgra,
            })
            .is_err()
        {
            break; // editor closed
        }
    }
    Ok(())
}
