//! Files the app downloads at run time from the asset host: one bucket
//! behind a public domain, the first path segment naming what's in it
//! (`effect-previews/…` holds the effect and filter tiles). Keys are
//! content-hashed, so a changed file is a new key: a download is kept on
//! disk for good (`<data>/cache/<bucket>/<key>`) and never re-checked.
//!
//! Tile pictures are fetched and decoded on a few worker threads, newest
//! request first (the tiles just scrolled into view win), and delivered to
//! the UI over a channel. A tile shows its first frame; the full animation
//! is decoded only when asked for (the hovered tile).

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use gpui::RenderImage;
use image::codecs::{gif::GifDecoder, webp::WebPDecoder};
use image::{AnimationDecoder, DynamicImage, Frame, ImageFormat, Rgba};

/// Tile previews of effects and filters.
pub const EFFECT_PREVIEWS: &str = "effect-previews";
/// Effect and filter shaders (`effects/…`, `filters/…`), as data.
pub const EFFECT_PACKAGES: &str = "effect-packages";
/// The sound effects library's files (keys from its table).
pub const SOUND_EFFECTS: &str = "sound-effects";
/// Not a bucket: keys are whole URLs elsewhere (sticker and GIF files,
/// album covers), cached under a hash of the URL.
pub const REMOTE: &str = "remote";

const WORKERS: usize = 4;
const TIMEOUT: Duration = Duration::from_secs(20);

/// A build-time setting (see build.rs), overridable at run time.
pub fn setting(name: &str, built_in: &'static str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| (!built_in.is_empty()).then(|| built_in.to_string()))
}

/// The asset host's public base URL, if this build has one.
pub fn base_url() -> Option<String> {
    setting("REEL_ASSETS_URL", env!("REEL_ASSETS_URL")).map(|s| s.trim_end_matches('/').into())
}

/// Percent-encodes one path segment.
fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The public URL of `key` in `bucket` (None without an asset host).
pub fn url(bucket: &str, key: &str) -> Option<String> {
    let base = base_url()?;
    let path: Vec<String> = key.split('/').map(encode_segment).collect();
    Some(format!("{base}/{bucket}/{}", path.join("/")))
}

/// Where a download of `key` is kept.
fn cache_path(bucket: &str, key: &str) -> PathBuf {
    let mut p = crate::prefs::data_dir().join("cache").join(bucket);
    for part in key.split('/').filter(|s| !s.is_empty() && *s != "..") {
        p.push(part);
    }
    p
}

/// One HTTP agent for the app (timeouts, connection reuse).
pub fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(TIMEOUT))
                .build(),
        )
    })
}

/// Where a download of a whole URL is kept: a hash of it, with the URL's
/// file extension (decoders and FFmpeg go by it).
fn remote_path(url: &str) -> PathBuf {
    // FNV-1a, 64 bit: stable across runs and builds.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in url.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let name = url.split(['?', '#']).next().unwrap_or("");
    let ext = name
        .rsplit_once('/')
        .map_or(name, |(_, last)| last)
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()));
    let file = match ext {
        Some(e) => format!("{h:016x}.{e}"),
        None => format!("{h:016x}"),
    };
    crate::prefs::data_dir()
        .join("cache")
        .join(REMOTE)
        .join(file)
}

/// Downloads `url` to `path` (written aside, then renamed: a half-written
/// file never looks cached).
fn download_to(url: &str, path: &std::path::Path, limit: u64) -> Result<Vec<u8>> {
    let agent = if limit > 32 << 20 {
        download_agent()
    } else {
        agent()
    };
    let mut response = agent.get(url).call().with_context(|| url.to_string())?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("part{}", std::process::id()));
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(bytes)
}

/// An agent for big downloads (a song, a sound): the same, but given
/// minutes rather than seconds to finish.
pub fn download_agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_connect(Some(TIMEOUT))
                .timeout_global(Some(Duration::from_secs(600)))
                .build(),
        )
    })
}

/// The bytes of `key`: from the disk cache, else downloaded (and cached).
/// In `REMOTE` the key is the URL itself.
pub fn fetch(bucket: &str, key: &str) -> Result<Vec<u8>> {
    let path = if bucket == REMOTE {
        remote_path(key)
    } else {
        cache_path(bucket, key)
    };
    if let Ok(bytes) = std::fs::read(&path) {
        return Ok(bytes);
    }
    let url = if bucket == REMOTE {
        key.to_string()
    } else {
        match url(bucket, key) {
            Some(u) => u,
            None => bail!("no asset host configured"),
        }
    };
    let bytes = download_to(&url, &path, 32 << 20)?;
    count(bucket, bytes.len());
    Ok(bytes)
}

/// Counts a download for Data Usage (tile pictures and sticker files are
/// previews, not counted).
fn count(bucket: &str, bytes: usize) {
    use crate::data_usage::{Kind, record};
    match bucket {
        EFFECT_PACKAGES => record(Kind::Effects, bytes as u64),
        SOUND_EFFECTS => record(Kind::Sounds, bytes as u64),
        _ => {}
    }
}

/// A local copy of `key` (see `fetch`), for things read by path (sound
/// through FFmpeg). Blocking: run it off the UI thread.
pub fn fetch_file(bucket: &str, key: &str) -> Result<PathBuf> {
    let path = if bucket == REMOTE {
        remote_path(key)
    } else {
        cache_path(bucket, key)
    };
    if !path.exists() {
        let url = if bucket == REMOTE {
            key.to_string()
        } else {
            match url(bucket, key) {
                Some(u) => u,
                None => bail!("no asset host configured"),
            }
        };
        let bytes = download_to(&url, &path, 256 << 20)?;
        count(bucket, bytes.len());
    }
    Ok(path)
}

/// RGBA → BGRA, as GPUI's atlas wants.
fn to_bgra(frame: &mut Frame) {
    for px in frame.buffer_mut().as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
}

/// Decodes a picture: its first frame only, or every frame of an
/// animation (WebP / GIF). Stills come back as one frame either way.
pub fn decode(bytes: &[u8], all_frames: bool) -> Result<Vec<Frame>> {
    let take = if all_frames { usize::MAX } else { 1 };
    let mut frames: Vec<Frame> = match image::guess_format(bytes)? {
        ImageFormat::WebP => {
            let mut d = WebPDecoder::new(Cursor::new(bytes))?;
            if d.has_animation() {
                let _ = d.set_background_color(Rgba([0, 0, 0, 0]));
                d.into_frames().take(take).collect::<Result<_, _>>()?
            } else {
                vec![Frame::new(DynamicImage::from_decoder(d)?.into_rgba8())]
            }
        }
        ImageFormat::Gif => GifDecoder::new(Cursor::new(bytes))?
            .into_frames()
            .take(take)
            .collect::<Result<_, _>>()?,
        format => vec![Frame::new(
            image::load_from_memory_with_format(bytes, format)?.into_rgba8(),
        )],
    };
    if frames.is_empty() {
        bail!("no frames");
    }
    frames.iter_mut().for_each(to_bgra);
    Ok(frames)
}

/// What a worker is asked for: a tile's first frame, or its animation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PictureJob {
    pub key: String,
    pub animated: bool,
}

/// A decoded tile picture (None: no host, download or decode failed).
pub struct Picture {
    pub job: PictureJob,
    pub image: Option<Arc<RenderImage>>,
}

/// A tile picture's shorter side at most (device pixels): tiles show
/// at ~100–200 points, so a full-size download is shrunk to fit.
const TILE_SIDE: u32 = 400;

/// The jobs waiting (newest last), and whether the server was dropped.
#[derive(Default)]
struct Queue {
    jobs: Vec<PictureJob>,
    quit: bool,
}

/// Fetches + decodes tile pictures of one bucket on worker threads.
pub struct PictureServer {
    queue: Arc<(Mutex<Queue>, Condvar)>,
}

impl Drop for PictureServer {
    /// Ends the workers (each finishes the job in hand first).
    fn drop(&mut self) {
        let (lock, ready) = &*self.queue;
        lock.lock().unwrap().quit = true;
        ready.notify_all();
    }
}

/// Scales `frames` down so the shorter side is at most `side` (never up).
fn shrink(frames: Vec<Frame>, side: u32) -> Vec<Frame> {
    frames
        .into_iter()
        .map(|f| {
            let (w, h) = f.buffer().dimensions();
            let short = w.min(h);
            if short <= side {
                return f;
            }
            let k = side as f64 / short as f64;
            let (nw, nh) = (
                ((w as f64 * k).round() as u32).max(1),
                ((h as f64 * k).round() as u32).max(1),
            );
            let (left, top, delay) = (f.left(), f.top(), f.delay());
            let small =
                image::imageops::resize(f.buffer(), nw, nh, image::imageops::FilterType::Triangle);
            Frame::from_parts(small, left, top, delay)
        })
        .collect()
}

impl PictureServer {
    pub fn new(bucket: &'static str, out: async_channel::Sender<Picture>) -> Self {
        let queue: Arc<(Mutex<Queue>, Condvar)> = Arc::default();
        for i in 0..WORKERS {
            let queue = queue.clone();
            let out = out.clone();
            std::thread::Builder::new()
                .name(format!("pictures-{i}"))
                .spawn(move || {
                    loop {
                        let job = {
                            let (lock, ready) = &*queue;
                            let mut queue = lock.lock().unwrap();
                            loop {
                                if queue.quit {
                                    return;
                                }
                                // Newest first: what was just shown.
                                if let Some(job) = queue.jobs.pop() {
                                    break job;
                                }
                                queue = ready.wait(queue).unwrap();
                            }
                        };
                        let image = fetch(bucket, &job.key)
                            .and_then(|bytes| decode(&bytes, job.animated))
                            .map(|frames| shrink(frames, TILE_SIDE))
                            .map(|frames| Arc::new(RenderImage::new(frames)));
                        if let Err(e) = &image {
                            eprintln!("tile picture {}: {e:#}", job.key);
                        }
                        let picture = Picture {
                            job,
                            image: image.ok(),
                        };
                        if out.send_blocking(picture).is_err() {
                            return;
                        }
                    }
                })
                .expect("spawn picture worker");
        }
        PictureServer { queue }
    }

    pub fn request(&self, job: PictureJob) {
        let (lock, ready) = &*self.queue;
        let jobs = &mut lock.lock().unwrap().jobs;
        // Asked again: move it to the front.
        jobs.retain(|j| *j != job);
        jobs.push(job);
        ready.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_encoded_per_segment() {
        assert_eq!(encode_segment("milk tea-1.jpg"), "milk%20tea-1.jpg");
        assert_eq!(encode_segment("B&W"), "B%26W");
    }

    #[test]
    fn remote_paths_keep_the_extension() {
        let p = remote_path("https://x.test/a/b/qSaImyCY.gif?x=1");
        assert_eq!(p.extension().unwrap(), "gif");
        assert_ne!(p, remote_path("https://x.test/a/b/other.gif"));
        let q = remote_path("https://x.test/?trackid=2&format=mp32");
        assert!(q.extension().is_none());
    }

    #[test]
    fn cache_paths_stay_inside_the_bucket() {
        let p = cache_path("effect-previews", "../../etc/x.jpg");
        assert!(p.ends_with("cache/effect-previews/etc/x.jpg"));
    }

    #[test]
    fn decodes_a_still() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(2, 3, Rgba([10, 20, 30, 255]))
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let frames = decode(&png, true).unwrap();
        assert_eq!(frames.len(), 1);
        // BGRA now.
        assert_eq!(&frames[0].buffer().as_raw()[..4], &[30, 20, 10, 255]);
    }
}
