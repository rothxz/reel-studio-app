//! The project document: the same JSON shape the app saves and the C++
//! engine parses (`src/types/project.ts` / `Project.h`). Only the fields
//! this spike edits are typed; everything else rides along in `extra`, so a
//! project exported from the app round-trips untouched.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Microseconds on the timeline (the app's `TimeMicros`).
pub type Micros = i64;

pub const SECOND: Micros = 1_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub version: u32,
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
    pub canvas: Canvas,
    pub duration: Micros,
    pub tracks: Vec<Track>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub background_color: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackType {
    Video,
    Audio,
    Text,
    Overlay,
    Effect,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: TrackType,
    pub name: String,
    pub clips: Vec<Clip>,
    pub order: i32,
    pub muted: bool,
    pub hidden: bool,
    pub locked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub id: String,
    /// "video" | "audio" | "image" | "text" | "sticker" | "effect" | "drawing"
    pub kind: String,
    pub timeline_start: Micros,
    pub timeline_duration: Micros,
    #[serde(default = "one")]
    pub speed: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_start: Option<Micros>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_duration: Option<Micros>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn one() -> f64 {
    1.0
}

impl Clip {
    pub fn end(&self) -> Micros {
        self.timeline_start + self.timeline_duration
    }

    pub fn label(&self) -> String {
        if let Some(text) = &self.text {
            return text.clone();
        }
        self.source
            .as_deref()
            .and_then(|s| s.rsplit('/').next())
            .map(str::to_owned)
            .unwrap_or_else(|| self.kind.clone())
    }
}

/// A file imported into the media library.
#[derive(Clone, Debug)]
pub struct MediaItem {
    pub path: String,
    pub name: String,
    pub duration: Micros,
    pub width: u32,
    pub height: u32,
    pub has_video: bool,
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn new_id(prefix: &str) -> String {
    let n = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{prefix}-{nanos:x}{n}")
}

impl Project {
    pub fn empty() -> Self {
        let now = "2026-01-01T00:00:00.000Z".to_string();
        Project {
            version: 1,
            id: new_id("project"),
            name: "Untitled".into(),
            created_at: now.clone(),
            updated_at: now,
            canvas: Canvas {
                width: 1080,
                height: 1920,
                fps: 30.0,
                background_color: "#000000".into(),
            },
            duration: 0,
            tracks: vec![
                Track {
                    id: new_id("track"),
                    kind: TrackType::Video,
                    name: "Main".into(),
                    clips: vec![],
                    order: 0,
                    muted: false,
                    hidden: false,
                    locked: false,
                },
                Track {
                    id: new_id("track"),
                    kind: TrackType::Audio,
                    name: "Audio".into(),
                    clips: vec![],
                    order: 1,
                    muted: false,
                    hidden: false,
                    locked: false,
                },
            ],
            extra: Map::new(),
        }
    }

    pub fn recompute_duration(&mut self) {
        self.duration = self
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .map(Clip::end)
            .max()
            .unwrap_or(0);
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("project serializes")
    }

    pub fn find_clip(&self, id: &str) -> Option<(&Track, &Clip)> {
        self.tracks
            .iter()
            .find_map(|t| t.clips.iter().find(|c| c.id == id).map(|c| (t, c)))
    }

    /// Appends `media` to the end of the main (first video) track.
    pub fn append_media(&mut self, media: &MediaItem) -> String {
        let kind = if media.has_video {
            TrackType::Video
        } else {
            TrackType::Audio
        };
        let track = self
            .tracks
            .iter_mut()
            .find(|t| t.kind == kind)
            .expect("project has video and audio tracks");
        let start = track.clips.iter().map(Clip::end).max().unwrap_or(0);
        let id = new_id("clip");
        track.clips.push(Clip {
            id: id.clone(),
            kind: if media.has_video { "video" } else { "audio" }.into(),
            timeline_start: start,
            timeline_duration: media.duration,
            speed: 1.0,
            source: Some(media.path.clone()),
            source_start: Some(0),
            source_duration: Some(media.duration),
            text: None,
            extra: Map::new(),
        });
        if self.tracks[0].clips.len() == 1 && media.has_video && media.width > 0 {
            // First clip sets the canvas, like a new project in the app.
            self.canvas.width = media.width;
            self.canvas.height = media.height;
        }
        self.recompute_duration();
        id
    }

    /// Splits the clip under `at` into two halves (main-track ripple is not
    /// needed: the halves cover the same span).
    pub fn split_at(&mut self, clip_id: &str, at: Micros) -> bool {
        for track in &mut self.tracks {
            let Some(i) = track.clips.iter().position(|c| c.id == clip_id) else {
                continue;
            };
            let clip = &track.clips[i];
            if at <= clip.timeline_start || at >= clip.end() {
                return false;
            }
            let mut left = clip.clone();
            let mut right = clip.clone();
            let offset = at - clip.timeline_start;
            left.timeline_duration = offset;
            right.id = new_id("clip");
            right.timeline_start = at;
            right.timeline_duration = clip.timeline_duration - offset;
            // The played segment is [sourceStart, sourceStart + duration * speed]
            // (ClipOffsetToSourceTime); sourceDuration stays the media's length.
            if let Some(ss) = clip.source_start {
                right.source_start = Some(ss + (offset as f64 * clip.speed) as Micros);
            }
            track.clips.splice(i..=i, [left, right]);
            return true;
        }
        false
    }

    /// Removes a clip; on the main track later clips close the gap.
    pub fn delete_clip(&mut self, clip_id: &str) {
        for (ti, track) in self.tracks.iter_mut().enumerate() {
            if let Some(i) = track.clips.iter().position(|c| c.id == clip_id) {
                let removed = track.clips.remove(i);
                if ti == 0 {
                    for c in track.clips.iter_mut().skip(i) {
                        c.timeline_start -= removed.timeline_duration;
                    }
                }
            }
        }
        self.recompute_duration();
    }

    /// Changes a clip's speed, keeping its source segment (so its timeline
    /// length scales), and ripples the main track.
    pub fn set_speed(&mut self, clip_id: &str, speed: f64) {
        for (ti, track) in self.tracks.iter_mut().enumerate() {
            let Some(i) = track.clips.iter().position(|c| c.id == clip_id) else {
                continue;
            };
            let clip = &mut track.clips[i];
            let source_len = (clip.timeline_duration as f64 * clip.speed) as Micros;
            let old = clip.timeline_duration;
            clip.speed = speed;
            clip.timeline_duration = (source_len as f64 / speed) as Micros;
            let delta = clip.timeline_duration - old;
            if ti == 0 {
                for c in track.clips.iter_mut().skip(i + 1) {
                    c.timeline_start += delta;
                }
            }
        }
        self.recompute_duration();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(secs: i64) -> MediaItem {
        MediaItem {
            path: "/tmp/a.mp4".into(),
            name: "a.mp4".into(),
            duration: secs * SECOND,
            width: 1920,
            height: 1080,
            has_video: true,
        }
    }

    #[test]
    fn append_split_speed_delete() {
        let mut p = Project::empty();
        let a = p.append_media(&media(4));
        let b = p.append_media(&media(2));
        assert_eq!(p.duration, 6 * SECOND);
        assert!(p.split_at(&a, SECOND));
        assert_eq!(p.tracks[0].clips.len(), 3);
        assert_eq!(p.tracks[0].clips[1].source_start, Some(SECOND));
        p.set_speed(&b, 2.0);
        assert_eq!(p.duration, 5 * SECOND);
        p.delete_clip(&a);
        assert_eq!(p.duration, 4 * SECOND);
        assert_eq!(p.tracks[0].clips[0].timeline_start, 0);
    }

    #[test]
    fn unknown_fields_round_trip() {
        let json = r##"{"version":1,"id":"p","name":"n","createdAt":"a","updatedAt":"b",
          "canvas":{"width":1,"height":2,"fps":30,"backgroundColor":"#000"},"duration":5,
          "layerStack":true,"tracks":[{"id":"t","type":"video","name":"Main","order":0,
          "muted":false,"hidden":false,"locked":false,"clips":[{"id":"c","kind":"video",
          "timelineStart":0,"timelineDuration":5,"speed":1,"source":"x","sourceStart":0,
          "sourceDuration":5,"keyframes":{"transform.x":[]}}]}]}"##;
        let p: Project = serde_json::from_str(json).unwrap();
        let back: Value = serde_json::from_str(&p.to_json()).unwrap();
        assert_eq!(back["layerStack"], true);
        assert!(back["tracks"][0]["clips"][0]["keyframes"].is_object());
    }
}
