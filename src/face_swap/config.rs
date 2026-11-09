//! A face swap's settings: what the clip stores (`Clip::extra["faceSwap"]`,
//! next to the app's other per-clip looks) and the processing
//! configuration derived from it.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::error::{FaceSwapError, Result};
use super::models;
use crate::model::{Clip, Micros};

/// Bumped when the pipeline's output changes for the same inputs (part
/// of every result's cache key).
pub const PIPELINE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    /// Frames read at up to 1280 px, no restoration: quick to process.
    #[default]
    Fast,
    /// Frames read at full size (up to 4K), restoration when asked.
    High,
}

/// Which person in the clip: where they were picked (a source time and
/// their box there, 0–1 of the frame). The job finds them again there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TargetAnchor {
    pub time: Micros,
    #[serde(rename = "box")]
    pub bbox: [f32; 4],
}

/// The clip's face swap, as the document stores it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceSwapSettings {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// The face photo (an absolute path, like media).
    #[serde(default)]
    pub source: String,
    /// SHA-256 of the photo's bytes (first 16 hex): a replaced file is a
    /// new source.
    #[serde(default)]
    pub source_hash: String,
    #[serde(default)]
    pub target: Option<TargetAnchor>,
    /// 0–100: how much of the swapped face shows (paste time, no
    /// processing).
    #[serde(default = "full")]
    pub blend: f64,
    #[serde(default)]
    pub quality: Quality,
    /// Face restoration (High quality only).
    #[serde(default)]
    pub restore: bool,
    /// The user confirmed they may use this face.
    #[serde(default)]
    pub consent: bool,
}

fn yes() -> bool {
    true
}

fn full() -> f64 {
    100.
}

impl Default for FaceSwapSettings {
    fn default() -> Self {
        FaceSwapSettings {
            enabled: true,
            source: String::new(),
            source_hash: String::new(),
            target: None,
            blend: 100.,
            quality: Quality::Fast,
            restore: false,
            consent: false,
        }
    }
}

pub const KEY: &str = "faceSwap";

impl Clip {
    pub fn face_swap(&self) -> Option<FaceSwapSettings> {
        serde_json::from_value(self.extra.get(KEY)?.clone()).ok()
    }

    pub fn set_face_swap(&mut self, s: Option<&FaceSwapSettings>) {
        match s.and_then(|s| serde_json::to_value(s).ok()) {
            Some(v) => {
                self.extra.insert(KEY.into(), v);
            }
            None => {
                self.extra.remove(KEY);
            }
        }
    }

    /// A face swap that's on and set up (shown, exported).
    pub fn active_face_swap(&self) -> Option<FaceSwapSettings> {
        self.face_swap().filter(|s| s.enabled && s.ready().is_ok())
    }
}

impl FaceSwapSettings {
    /// Everything a job needs is set.
    pub fn ready(&self) -> Result<()> {
        if self.source.is_empty() || self.source_hash.is_empty() {
            return Err(FaceSwapError::InvalidConfig(
                "Pick a face photo first".into(),
            ));
        }
        if self.target.is_none() {
            return Err(FaceSwapError::InvalidConfig(
                "Pick the person to swap first".into(),
            ));
        }
        if !self.consent {
            return Err(FaceSwapError::InvalidConfig(
                "Confirm you may use this face first".into(),
            ));
        }
        if !(0. ..=100.).contains(&self.blend) {
            return Err(FaceSwapError::InvalidConfig("Blend is 0–100".into()));
        }
        Ok(())
    }

    /// The processing configuration these settings ask for.
    pub fn config(&self) -> FaceSwapConfig {
        FaceSwapConfig::new(self.quality, self.restore && self.quality == Quality::High)
    }

    /// Names the processed result of `media` with these settings: the
    /// media file (path, size, modified time), the face photo's hash, the
    /// chosen person, the quality, the models and the pipeline. Not the
    /// clip's id: a split, copied or undone clip reuses the same result.
    /// Blend isn't in it (applied when pasting).
    pub fn cache_key(&self, media: &str) -> String {
        let meta = std::fs::metadata(media).ok();
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        let target = self
            .target
            .as_ref()
            .map(|t| {
                format!(
                    "{}:{:?}",
                    t.time,
                    t.bbox.map(|v| (v * 10_000.).round() as i64)
                )
            })
            .unwrap_or_default();
        let cfg = self.config();
        let key = format!(
            "v{PIPELINE_VERSION}|{media}|{}|{modified}|{}|{target}|{:?}|{}|{}",
            meta.map_or(0, |m| m.len()),
            self.source_hash,
            cfg.quality,
            cfg.restore,
            models::versions(),
        );
        models::hex(&Sha256::digest(key.as_bytes()))[..24].to_string()
    }
}

/// The source times `clip` plays (start, end): its segment of the media,
/// `sourceStart` to its length × speed (a speed curve or reverse stays
/// inside it; a freeze is one frame of it).
pub fn source_range(clip: &Clip) -> (Micros, Micros) {
    let start = clip.source_start.unwrap_or(0).max(0);
    let speed = if clip.speed > 0. { clip.speed } else { 1. };
    let len = if clip.freeze() {
        0
    } else {
        (clip.timeline_duration as f64 * speed).round() as Micros
    };
    let mut end = start + len;
    if let Some(d) = clip.source_duration.filter(|d| *d > 0) {
        end = end.min(d);
    }
    (start, end.max(start))
}

/// The first 16 hex of the SHA-256 of `path`'s bytes.
pub fn photo_hash(path: &str) -> Result<String> {
    Ok(models::sha256_file(std::path::Path::new(path))?[..16].to_string())
}

/// How the person is followed from frame to frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackerConfig {
    /// A detection overlapping the last box at least this much is the
    /// same face (no identity check needed between checks).
    pub iou_min: f32,
    /// The identity is checked again at least every this many frames.
    pub verify_every: u32,
    /// Cosine similarity to the chosen person's identity above which a
    /// tracked face is still them.
    pub keep_similarity: f32,
    /// Higher bar to pick them up again after they were lost.
    pub recover_similarity: f32,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        TrackerConfig {
            iou_min: 0.3,
            verify_every: 8,
            keep_similarity: 0.3,
            recover_similarity: 0.42,
        }
    }
}

/// How a job processes frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceSwapConfig {
    pub quality: Quality,
    pub restore: bool,
    /// Detector score (0–1) a face needs.
    pub detect_threshold: f32,
    /// Frames are read at most this wide.
    pub decode_width: u32,
    pub tracker: TrackerConfig,
    /// Run the models on the GPU / Neural Engine when there is one.
    pub gpu: bool,
}

impl FaceSwapConfig {
    pub fn new(quality: Quality, restore: bool) -> Self {
        FaceSwapConfig {
            quality,
            restore,
            detect_threshold: 0.6,
            decode_width: match quality {
                Quality::Fast => 1280,
                Quality::High => 3840,
            },
            tracker: TrackerConfig {
                verify_every: match quality {
                    Quality::Fast => 10,
                    Quality::High => 5,
                },
                ..TrackerConfig::default()
            },
            gpu: true,
        }
    }

    pub fn validate(&self) -> Result<()> {
        let t = &self.tracker;
        let bad = |m: &str| Err(FaceSwapError::InvalidConfig(m.into()));
        if !(0.05..=0.99).contains(&self.detect_threshold) {
            return bad("detector threshold out of range");
        }
        if self.decode_width < 320 {
            return bad("decode width too small");
        }
        if !(0.05..=0.95).contains(&t.iou_min) {
            return bad("tracking overlap out of range");
        }
        if t.verify_every == 0 {
            return bad("identity check interval is 0");
        }
        if t.recover_similarity < t.keep_similarity {
            return bad("recovery must be stricter than keeping");
        }
        if self.restore && self.quality != Quality::High {
            return bad("restoration is for High quality");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_up() -> FaceSwapSettings {
        FaceSwapSettings {
            source: "/faces/me.jpg".into(),
            source_hash: "0123456789abcdef".into(),
            target: Some(TargetAnchor {
                time: 1_000_000,
                bbox: [0.4, 0.2, 0.2, 0.3],
            }),
            consent: true,
            ..Default::default()
        }
    }

    #[test]
    fn configs_validate() {
        assert!(FaceSwapConfig::new(Quality::Fast, false).validate().is_ok());
        assert!(FaceSwapConfig::new(Quality::High, true).validate().is_ok());
        assert!(FaceSwapConfig::new(Quality::Fast, true).validate().is_err());
        let mut c = FaceSwapConfig::new(Quality::Fast, false);
        c.tracker.recover_similarity = 0.1;
        assert!(c.validate().is_err());
        c = FaceSwapConfig::new(Quality::Fast, false);
        c.tracker.verify_every = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn settings_need_a_photo_a_person_and_consent() {
        assert!(set_up().ready().is_ok());
        assert!(
            FaceSwapSettings {
                consent: false,
                ..set_up()
            }
            .ready()
            .is_err()
        );
        assert!(
            FaceSwapSettings {
                target: None,
                ..set_up()
            }
            .ready()
            .is_err()
        );
        assert!(
            FaceSwapSettings {
                source: String::new(),
                ..set_up()
            }
            .ready()
            .is_err()
        );
        // Restoration only counts at High.
        assert!(
            !FaceSwapSettings {
                restore: true,
                ..set_up()
            }
            .config()
            .restore
        );
        let high = FaceSwapSettings {
            restore: true,
            quality: Quality::High,
            ..set_up()
        };
        assert!(high.config().restore);
    }

    #[test]
    fn round_trips_through_the_clip() {
        let mut clip: Clip = serde_json::from_value(serde_json::json!({
            "id": "c1", "kind": "video", "timelineStart": 0, "timelineDuration": 1000000
        }))
        .unwrap();
        assert!(clip.face_swap().is_none());
        clip.set_face_swap(Some(&set_up()));
        let json = serde_json::to_value(&clip).unwrap();
        assert_eq!(json["faceSwap"]["target"]["box"][0], 0.4f32 as f64);
        assert_eq!(json["faceSwap"]["sourceHash"], "0123456789abcdef");
        assert_eq!(clip.face_swap(), Some(set_up()));
        assert!(clip.active_face_swap().is_some());
        clip.set_face_swap(Some(&FaceSwapSettings {
            enabled: false,
            ..set_up()
        }));
        assert!(clip.active_face_swap().is_none());
        clip.set_face_swap(None);
        assert!(!clip.extra.contains_key(KEY));
    }

    /// Anything that changes the processed frames changes the key; blend
    /// and on/off don't.
    #[test]
    fn cache_key_invalidation() {
        let base = set_up();
        let k = base.cache_key("/m/a.mp4");
        assert_eq!(k, set_up().cache_key("/m/a.mp4"));
        assert_ne!(k, base.cache_key("/m/b.mp4"));
        let changed = [
            FaceSwapSettings {
                source_hash: "fedcba9876543210".into(),
                ..set_up()
            },
            FaceSwapSettings {
                target: Some(TargetAnchor {
                    time: 2_000_000,
                    bbox: [0.4, 0.2, 0.2, 0.3],
                }),
                ..set_up()
            },
            FaceSwapSettings {
                target: Some(TargetAnchor {
                    time: 1_000_000,
                    bbox: [0.1, 0.2, 0.2, 0.3],
                }),
                ..set_up()
            },
            FaceSwapSettings {
                quality: Quality::High,
                ..set_up()
            },
        ];
        for c in changed {
            assert_ne!(k, c.cache_key("/m/a.mp4"), "{c:?}");
        }
        let same = [
            FaceSwapSettings {
                blend: 40.,
                ..set_up()
            },
            FaceSwapSettings {
                enabled: false,
                ..set_up()
            },
            // Restore is ignored at Fast.
            FaceSwapSettings {
                restore: true,
                ..set_up()
            },
        ];
        for c in same {
            assert_eq!(k, c.cache_key("/m/a.mp4"), "{c:?}");
        }
    }
}
