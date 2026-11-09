//! A processed face swap on disk: per source frame, where the swapped face
//! goes (the alignment transform) and the face itself (a small JPEG crop).
//! Not whole frames: the player and export paste the crop onto the frame
//! they decode anyway, at their own size, so one result serves the
//! preview and a full-resolution export alike, and is a few MB a minute.
//!
//! `<data>/cache/face-swap/<key>.rsfs`, the key naming everything that
//! went into it (see `FaceSwapSettings::cache_key`). Written to `.part`
//! and renamed when whole, so a cancelled job leaves nothing.
//!
//! ```text
//! "RSFACE01" · u32 header length · header JSON
//! per frame: i64 pts · u8 has face · 4 × f32 transform · u32 n · n JPEG bytes
//! index: u32 count · count × (i64 pts, u64 offset) · u64 index offset · "RSFIDX01"
//! ```

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use image::RgbImage;
use serde::{Deserialize, Serialize};

use super::align::Similarity;
use super::blend::MaskShape;
use super::error::{FaceSwapError, Result};
use crate::model::Micros;

const MAGIC: &[u8; 8] = b"RSFACE01";
const INDEX_MAGIC: &[u8; 8] = b"RSFIDX01";
const JPEG_QUALITY: u8 = 95;
/// Decoded crops kept in memory per open result.
const KEEP_DECODED: usize = 48;

/// `<data>/cache/face-swap`, or `REEL_FACE_CACHE` (tests).
pub fn dir() -> PathBuf {
    match std::env::var_os("REEL_FACE_CACHE") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => crate::prefs::data_dir().join("cache").join("face-swap"),
    }
}

pub fn path_for(key: &str) -> PathBuf {
    dir().join(format!("{key}.rsfs"))
}

/// What a result is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultHeader {
    pub key: String,
    pub media: String,
    /// The frame size the transforms are in.
    pub ref_width: u32,
    pub ref_height: u32,
    pub crop_size: u32,
    pub mask_pad: [f32; 4],
    pub mask_blur: f32,
    /// The source's frame duration.
    pub frame_duration: Micros,
    /// The source times processed.
    pub start: Micros,
    pub end: Micros,
    pub track_id: u32,
    pub models: String,
    pub provider: String,
}

impl ResultHeader {
    pub fn mask(&self) -> MaskShape {
        MaskShape {
            pad: self.mask_pad,
            blur: self.mask_blur,
        }
    }
}

/// One frame's swapped face.
#[derive(Clone)]
pub struct FaceRecord {
    /// Frame (at the header's reference size) → crop.
    pub m: Similarity,
    pub crop: Arc<RgbImage>,
}

pub struct ResultWriter {
    out: BufWriter<File>,
    part: PathBuf,
    path: PathBuf,
    index: Vec<(Micros, u64)>,
    at: u64,
}

impl ResultWriter {
    pub fn create(path: &Path, header: &ResultHeader) -> Result<ResultWriter> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let part = path.with_extension("part");
        let mut out = BufWriter::new(File::create(&part)?);
        let json = serde_json::to_vec(header).map_err(|e| FaceSwapError::Io(e.to_string()))?;
        out.write_all(MAGIC)?;
        out.write_all(&(json.len() as u32).to_le_bytes())?;
        out.write_all(&json)?;
        Ok(ResultWriter {
            out,
            part,
            path: path.to_path_buf(),
            index: vec![],
            at: (MAGIC.len() + 4 + json.len()) as u64,
        })
    }

    /// The frame at `pts`: its face, or None when the person isn't on it.
    pub fn push(&mut self, pts: Micros, face: Option<(&Similarity, &RgbImage)>) -> Result<()> {
        let mut jpeg = vec![];
        let m = match face {
            Some((m, crop)) => {
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
                    .encode_image(crop)
                    .map_err(|e| FaceSwapError::Io(e.to_string()))?;
                *m
            }
            None => Similarity::IDENTITY,
        };
        self.index.push((pts, self.at));
        let mut rec = Vec::with_capacity(8 + 1 + 16 + 4 + jpeg.len());
        rec.extend_from_slice(&pts.to_le_bytes());
        rec.push(face.is_some() as u8);
        for v in [m.a, m.b, m.tx, m.ty] {
            rec.extend_from_slice(&v.to_le_bytes());
        }
        rec.extend_from_slice(&(jpeg.len() as u32).to_le_bytes());
        rec.extend_from_slice(&jpeg);
        self.out.write_all(&rec)?;
        self.at += rec.len() as u64;
        Ok(())
    }

    pub fn frames(&self) -> usize {
        self.index.len()
    }

    /// Writes the index and puts the file in place.
    pub fn finish(mut self) -> Result<PathBuf> {
        let index_at = self.at;
        self.out
            .write_all(&(self.index.len() as u32).to_le_bytes())?;
        for (pts, off) in &self.index {
            self.out.write_all(&pts.to_le_bytes())?;
            self.out.write_all(&off.to_le_bytes())?;
        }
        self.out.write_all(&index_at.to_le_bytes())?;
        self.out.write_all(INDEX_MAGIC)?;
        let file = self
            .out
            .into_inner()
            .map_err(|e| FaceSwapError::Io(e.to_string()))?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&self.part, &self.path)?;
        Ok(self.path)
    }

    /// Leaves nothing behind.
    pub fn abandon(self) {
        drop(self.out);
        let _ = std::fs::remove_file(&self.part);
    }
}

/// Decoded records by file offset (None: no face there), oldest first.
type Decoded = (HashMap<u64, Option<FaceRecord>>, VecDeque<u64>);

/// A finished result, open for lookups (shared between threads).
pub struct ResultFile {
    pub header: ResultHeader,
    /// (pts, offset), by time.
    index: Vec<(Micros, u64)>,
    file: Mutex<File>,
    decoded: Mutex<Decoded>,
}

impl ResultFile {
    pub fn open(path: &Path) -> Result<ResultFile> {
        let bad = || FaceSwapError::Io(format!("{} isn't a face swap result", path.display()));
        let mut f = File::open(path)?;
        let mut magic = [0u8; 8];
        f.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(bad());
        }
        let mut n = [0u8; 4];
        f.read_exact(&mut n)?;
        let mut json = vec![0u8; u32::from_le_bytes(n) as usize];
        f.read_exact(&mut json)?;
        let header: ResultHeader = serde_json::from_slice(&json).map_err(|_| bad())?;
        f.seek(SeekFrom::End(-16))?;
        let mut tail = [0u8; 16];
        f.read_exact(&mut tail)?;
        if &tail[8..] != INDEX_MAGIC {
            return Err(bad());
        }
        let index_at = u64::from_le_bytes(tail[..8].try_into().unwrap());
        f.seek(SeekFrom::Start(index_at))?;
        f.read_exact(&mut n)?;
        let count = u32::from_le_bytes(n) as usize;
        let mut raw = vec![0u8; count * 16];
        f.read_exact(&mut raw)?;
        let mut index: Vec<(Micros, u64)> = raw
            .as_chunks::<16>()
            .0
            .iter()
            .map(|c| {
                (
                    i64::from_le_bytes(c[..8].try_into().unwrap()),
                    u64::from_le_bytes(c[8..].try_into().unwrap()),
                )
            })
            .collect();
        index.sort_by_key(|(p, _)| *p);
        Ok(ResultFile {
            header,
            index,
            file: Mutex::new(f),
            decoded: Mutex::new((HashMap::new(), VecDeque::new())),
        })
    }

    pub fn frames(&self) -> usize {
        self.index.len()
    }

    /// Whether source times `start..end` were processed (to a frame).
    pub fn covers(&self, start: Micros, end: Micros) -> bool {
        let slack = self.header.frame_duration;
        self.header.start <= start + slack && self.header.end + slack >= end
    }

    /// The record of the frame at `pts` (within half a frame), decoded.
    /// None when the person isn't on that frame or it wasn't processed.
    pub fn face_at(&self, pts: Micros) -> Option<FaceRecord> {
        let half = (self.header.frame_duration / 2).max(1);
        let i = self.index.partition_point(|(p, _)| *p < pts);
        let near = [i.checked_sub(1), Some(i)]
            .into_iter()
            .flatten()
            .filter_map(|j| self.index.get(j))
            .min_by_key(|(p, _)| (p - pts).abs())?;
        if (near.0 - pts).abs() > half {
            return None;
        }
        let off = near.1;
        {
            let cache = self.decoded.lock().ok()?;
            if let Some(hit) = cache.0.get(&off) {
                return hit.clone();
            }
        }
        let rec = self.read(off).ok().flatten();
        let mut cache = self.decoded.lock().ok()?;
        let (map, order) = &mut *cache;
        map.insert(off, rec.clone());
        order.push_back(off);
        while order.len() > KEEP_DECODED {
            if let Some(old) = order.pop_front() {
                map.remove(&old);
            }
        }
        rec
    }

    fn read(&self, off: u64) -> Result<Option<FaceRecord>> {
        let mut f = self
            .file
            .lock()
            .map_err(|_| FaceSwapError::Io("result lock".into()))?;
        f.seek(SeekFrom::Start(off))?;
        let mut head = [0u8; 8 + 1 + 16 + 4];
        f.read_exact(&mut head)?;
        if head[8] == 0 {
            return Ok(None);
        }
        let v = |i: usize| f32::from_le_bytes(head[9 + i * 4..13 + i * 4].try_into().unwrap());
        let m = Similarity {
            a: v(0),
            b: v(1),
            tx: v(2),
            ty: v(3),
        };
        let n = u32::from_le_bytes(head[25..29].try_into().unwrap()) as usize;
        let mut jpeg = vec![0u8; n];
        f.read_exact(&mut jpeg)?;
        drop(f);
        let crop = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
            .map_err(|e| FaceSwapError::Io(e.to_string()))?
            .to_rgb8();
        Ok(Some(FaceRecord {
            m,
            crop: Arc::new(crop),
        }))
    }
}

/// Results opened by key, for the player, the panel and export (None:
/// not on disk, asked once until `forget`).
#[derive(Default)]
pub struct Results {
    open: std::cell::RefCell<HashMap<String, Option<Arc<ResultFile>>>>,
}

impl Results {
    pub fn get(&self, key: &str) -> Option<Arc<ResultFile>> {
        self.open
            .borrow_mut()
            .entry(key.to_string())
            .or_insert_with(|| ResultFile::open(&path_for(key)).ok().map(Arc::new))
            .clone()
    }

    /// Looks on disk again next time (a job just wrote it, or it was cleared).
    pub fn forget(&self, key: &str) {
        self.open.borrow_mut().remove(key);
    }

    pub fn clear(&self) {
        self.open.borrow_mut().clear();
    }

    /// The result `settings` make of `clip`'s media, when it's processed
    /// for the part of the media the clip plays.
    pub fn processed(
        &self,
        clip: &crate::model::Clip,
        settings: &super::config::FaceSwapSettings,
    ) -> Option<Arc<ResultFile>> {
        let media = clip.source.as_deref()?;
        let r = self.get(&settings.cache_key(media))?;
        let (start, end) = super::config::source_range(clip);
        r.covers(start, end).then_some(r)
    }

    /// What `clip` shows over its frames: its face swap's result and the
    /// blend (0–1), when the swap is on, set up and processed.
    pub fn for_clip(&self, clip: &crate::model::Clip) -> Option<(Arc<ResultFile>, f32)> {
        let s = clip.active_face_swap()?;
        let r = self.processed(clip, &s)?;
        Some((r, (s.blend / 100.) as f32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> ResultHeader {
        ResultHeader {
            key: "k".into(),
            media: "/m.mp4".into(),
            ref_width: 640,
            ref_height: 360,
            crop_size: 32,
            mask_pad: [0.; 4],
            mask_blur: 0.3,
            frame_duration: 33_333,
            start: 0,
            end: 100_000,
            track_id: 1,
            models: "test".into(),
            provider: "CPU".into(),
        }
    }

    #[test]
    fn writes_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("rsfs-{}", std::process::id()));
        let path = dir.join("r.rsfs");
        let mut w = ResultWriter::create(&path, &header()).unwrap();
        let crop = RgbImage::from_pixel(32, 32, image::Rgb([200, 100, 50]));
        let m = Similarity {
            a: 0.5,
            b: 0.1,
            tx: 3.,
            ty: -4.,
        };
        w.push(0, Some((&m, &crop))).unwrap();
        w.push(33_333, None).unwrap();
        w.push(66_667, Some((&m, &crop))).unwrap();
        // Nothing in place until it's whole.
        assert!(!path.exists());
        w.finish().unwrap();
        let r = ResultFile::open(&path).unwrap();
        assert_eq!(r.header, header());
        assert_eq!(r.frames(), 3);
        let rec = r.face_at(1_000).expect("near frame 0");
        assert_eq!(rec.m, m);
        let px = rec.crop.get_pixel(16, 16).0;
        assert!(
            px.iter()
                .zip([200u8, 100, 50])
                .all(|(a, b)| a.abs_diff(b) <= 3),
            "{px:?}"
        );
        // The person isn't on frame 1.
        assert!(r.face_at(33_333).is_none());
        // Within half a frame of frame 2; nothing far past the end.
        assert!(r.face_at(80_000).is_some());
        assert!(r.face_at(200_000).is_none());
        assert!(r.covers(0, 100_000));
        assert!(!r.covers(0, 400_000));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn abandoned_results_leave_nothing() {
        let dir = std::env::temp_dir().join(format!("rsfs-a-{}", std::process::id()));
        let path = dir.join("r.rsfs");
        let w = ResultWriter::create(&path, &header()).unwrap();
        w.abandon();
        assert!(!path.exists());
        assert!(!path.with_extension("part").exists());
        assert!(ResultFile::open(&path).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
