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
    /// Deactivated: stays on the timeline (dimmed), preview and export skip it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn one() -> f64 {
    1.0
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Clip {
    pub fn end(&self) -> Micros {
        self.timeline_start + self.timeline_duration
    }

    fn flag(&self, key: &str) -> bool {
        self.extra
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// A freeze frame (holds one source frame for its whole length).
    pub fn freeze(&self) -> bool {
        self.flag("freeze")
    }

    /// Plays its source segment end to start.
    pub fn reversed(&self) -> bool {
        self.flag("reversed")
    }

    /// How long the transition into the next main clip overlaps it.
    pub fn out_transition(&self) -> Micros {
        self.extra
            .get("outTransition")
            .and_then(|t| t.get("durationMicros"))
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .min(self.timeline_duration)
    }

    /// The source window left when the clip is trimmed to `duration` from
    /// `side` (start or end): only video and audio have one, and a freeze
    /// frame keeps its single frame. A reversed clip walks its segment end
    /// to start, so trimming its end moves the source start instead.
    pub fn trim_source(&mut self, duration: Micros, side: Side) {
        if self.kind != "video" && self.kind != "audio" {
            return;
        }
        if self.kind == "video" && self.freeze() {
            return;
        }
        let speed = if self.speed > 0. { self.speed } else { 1. };
        let removed = ((self.timeline_duration - duration) as f64 * speed) as Micros;
        self.source_duration = Some((duration as f64 * speed).round() as Micros);
        let advances_start = if self.kind == "video" && self.reversed() {
            side == Side::End
        } else {
            side == Side::Start
        };
        if advances_start {
            self.source_start = Some((self.source_start.unwrap_or(0) + removed).max(0));
        }
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

/// Which end of a clip an edit acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Start,
    End,
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
    /// A photo: one picture, no running time of its own.
    pub still: bool,
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
            id: new_id("proj"),
            name: "New Project".into(),
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
    /// Adds `media` as a clip. Pictures go into the main lane as clip number
    /// `index` (None: at the end), the clips after it moving right; sound
    /// goes on the audio lane at the end. The first picture sets the canvas
    /// shape, as a new project does.
    pub fn insert_media(&mut self, media: &MediaItem, index: Option<usize>) -> String {
        let id = new_id("clip");
        let kind = if !media.has_video {
            "audio"
        } else if media.still {
            "image"
        } else {
            "video"
        };
        let clip = Clip {
            id: id.clone(),
            kind: kind.into(),
            timeline_start: 0,
            timeline_duration: media.duration,
            speed: 1.0,
            source: Some(media.path.clone()),
            source_start: (!media.still).then_some(0),
            source_duration: (!media.still).then_some(media.duration),
            text: None,
            hidden: false,
            extra: Map::new(),
        };
        let lane = if media.has_video {
            TrackType::Video
        } else {
            TrackType::Audio
        };
        let track = self
            .tracks
            .iter_mut()
            .find(|t| t.kind == lane)
            .expect("project has video and audio tracks");
        let first_picture = media.has_video && track.clips.is_empty();
        let mut ordered: Vec<(Micros, Micros)> = track
            .clips
            .iter()
            .map(|c| (c.timeline_start, c.end()))
            .collect();
        ordered.sort_unstable();
        let at = match index {
            Some(i) if media.has_video && i < ordered.len() => {
                if i == 0 {
                    0
                } else {
                    ordered[i - 1].1
                }
            }
            _ => ordered.iter().map(|c| c.1).max().unwrap_or(0),
        };
        if media.has_video {
            for c in track.clips.iter_mut() {
                if c.timeline_start >= at {
                    c.timeline_start += media.duration;
                }
            }
        }
        track.clips.push(Clip {
            timeline_start: at,
            ..clip
        });
        if first_picture && media.width > 0 {
            self.canvas.width = media.width;
            self.canvas.height = media.height;
        }
        self.recompute_duration();
        id
    }

    /// Where a click in the library inserts on the main lane for the
    /// playhead at `now`: before the first clip whose middle is past it
    /// (None: at the end).
    pub fn insert_index_at(&self, now: Micros) -> Option<usize> {
        self.ordered_main_clips()
            .iter()
            .position(|c| c.timeline_start + c.timeline_duration / 2 > now)
    }

    /// The main lane: the first video track (always present).
    pub fn main_track(&self) -> Option<&Track> {
        self.tracks.iter().find(|t| t.kind == TrackType::Video)
    }

    pub fn is_main_track(&self, track_id: &str) -> bool {
        self.main_track().is_some_and(|t| t.id == track_id)
    }

    pub fn track_of(&self, clip_id: &str) -> Option<&Track> {
        self.tracks
            .iter()
            .find(|t| t.clips.iter().any(|c| c.id == clip_id))
    }

    fn clip_mut(&mut self, clip_id: &str) -> Option<&mut Clip> {
        self.tracks
            .iter_mut()
            .flat_map(|t| t.clips.iter_mut())
            .find(|c| c.id == clip_id)
    }

    /// Splits a clip at `at` into two (the halves cover the same span, so
    /// nothing moves). False when `at` isn't inside it.
    // TODO: keyframes, motion path, speed curve and drawing animations are
    // per-half in the old editor (splitClipHalves); they ride along whole.
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
            // (ClipOffsetToSourceTime). A reversed clip plays it backwards,
            // so its left half is the later source.
            if let Some(ss) = clip.source_start
                && !clip.freeze()
            {
                let advance = (offset as f64 * clip.speed) as Micros;
                let rest = ((clip.timeline_duration - offset) as f64 * clip.speed) as Micros;
                if clip.kind == "video" && clip.reversed() {
                    left.source_start = Some(ss + rest);
                } else {
                    right.source_start = Some(ss + advance);
                }
            }
            track.clips.splice(i..=i, [left, right]);
            return true;
        }
        false
    }

    /// Removes a clip. Nothing else moves: on the main lane the caller runs
    /// the edit through `with_magnet` to close the gap.
    pub fn delete_clip(&mut self, clip_id: &str) -> Option<Clip> {
        let mut removed = None;
        for track in &mut self.tracks {
            if let Some(i) = track.clips.iter().position(|c| c.id == clip_id) {
                removed = Some(track.clips.remove(i));
            }
        }
        self.recompute_duration();
        removed
    }

    /// Changes a clip's speed, keeping its source segment (so its timeline
    /// length scales).
    pub fn set_speed(&mut self, clip_id: &str, speed: f64) {
        if let Some(clip) = self.clip_mut(clip_id) {
            let source_len = (clip.timeline_duration as f64 * clip.speed) as Micros;
            clip.speed = speed;
            clip.timeline_duration = (source_len as f64 / speed).round() as Micros;
        }
        self.recompute_duration();
    }

    /// Delete left / right: trims away the part of the clip before / after
    /// `at`. The kept part stays where it was on the timeline.
    pub fn trim_at(&mut self, clip_id: &str, at: Micros, side: Side) -> bool {
        let Some(clip) = self.clip_mut(clip_id) else {
            return false;
        };
        if at <= clip.timeline_start || at >= clip.end() {
            return false;
        }
        match side {
            Side::End => {
                let duration = at - clip.timeline_start;
                clip.trim_source(duration, Side::End);
                clip.timeline_duration = duration;
            }
            Side::Start => {
                let duration = clip.end() - at;
                clip.trim_source(duration, Side::Start);
                clip.timeline_duration = duration;
                clip.timeline_start = at;
            }
        }
        self.recompute_duration();
        true
    }

    /// A copy of the clip right after it on its lane; returns the copy's id.
    /// On the main lane it is inserted after the original (the magnet then
    /// re-packs the clips after it).
    pub fn duplicate_clip(&mut self, clip_id: &str) -> Option<String> {
        let main_id = self.main_track().map(|t| t.id.clone());
        for track in &mut self.tracks {
            let Some(i) = track.clips.iter().position(|c| c.id == clip_id) else {
                continue;
            };
            let mut copy = track.clips[i].clone();
            copy.id = new_id("clip");
            copy.timeline_start = track.clips[i].end();
            let id = copy.id.clone();
            if Some(&track.id) == main_id.as_ref() {
                // Everything after it moves right to make room.
                let d = copy.timeline_duration;
                for c in track.clips.iter_mut() {
                    if c.timeline_start >= copy.timeline_start {
                        c.timeline_start += d;
                    }
                }
            }
            track.clips.insert(i + 1, copy);
            self.recompute_duration();
            return Some(id);
        }
        None
    }

    /// Pastes `clip` (a copy) at `at`: on the main lane at the cut after the
    /// clip under `at`, the later clips moving right to make room; on any
    /// other lane at `at`. Lands on `track_id`, or the first lane of `kind`
    /// when that lane is gone. Returns the new clip's id.
    pub fn paste_clip(
        &mut self,
        clip: &Clip,
        track_id: &str,
        kind: TrackType,
        at: Micros,
    ) -> Option<String> {
        let ti = self
            .tracks
            .iter()
            .position(|t| t.id == track_id)
            .or_else(|| self.tracks.iter().position(|t| t.kind == kind))?;
        let main = self.tracks[ti].kind == TrackType::Video;
        let track = &mut self.tracks[ti];
        let mut copy = clip.clone();
        copy.id = new_id("clip");
        if main {
            let mut ordered: Vec<&Clip> = track.clips.iter().collect();
            ordered.sort_by_key(|c| c.timeline_start);
            let under = ordered
                .iter()
                .find(|c| at >= c.timeline_start && at < c.end());
            let seam = match (under, ordered.last()) {
                (Some(c), _) => c.end(),
                (None, Some(last)) if at >= last.timeline_start => last.end(),
                _ => 0,
            };
            for c in track.clips.iter_mut() {
                if c.timeline_start >= seam {
                    c.timeline_start += copy.timeline_duration;
                }
            }
            copy.timeline_start = seam;
        } else {
            copy.timeline_start = at;
        }
        let id = copy.id.clone();
        track.clips.push(copy);
        self.recompute_duration();
        Some(id)
    }

    pub fn set_clip_hidden(&mut self, clip_id: &str, hidden: bool) {
        if let Some(clip) = self.clip_mut(clip_id) {
            clip.hidden = hidden;
        }
    }

    /// Every clip start and end on any lane, plus 0 and the end, sorted.
    pub fn edit_points(&self) -> Vec<Micros> {
        let mut points: Vec<Micros> = self
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .flat_map(|c| [c.timeline_start, c.end()])
            .chain([0, self.duration])
            .collect();
        points.sort_unstable();
        points.dedup();
        points
    }

    /// The main clips in timeline order.
    pub fn ordered_main_clips(&self) -> Vec<&Clip> {
        let mut clips: Vec<&Clip> = self
            .main_track()
            .map(|t| t.clips.iter().collect())
            .unwrap_or_default();
        clips.sort_by_key(|c| c.timeline_start);
        clips
    }
}

/// The main-track magnet: `after` (an edit of `before`) with its main lane
/// packed back to back from 00:00 (each clip overlapping the next by its
/// out-transition), and, with `linkage`, every clip on another lane whose
/// start lay inside a main clip moved by as much as that main clip moved.
/// Clips the edit itself moved, and clips over a main clip that went away,
/// stay where they are.
pub fn with_magnet(before: &Project, mut after: Project, linkage: bool) -> Project {
    let Some(main_ix) = after.tracks.iter().position(|t| t.kind == TrackType::Video) else {
        return after;
    };
    // Pack the main lane.
    let main = &mut after.tracks[main_ix];
    main.clips.sort_by_key(|c| c.timeline_start);
    let mut at = 0;
    let mut start_of = std::collections::HashMap::new();
    for clip in main.clips.iter_mut() {
        clip.timeline_start = at;
        start_of.insert(clip.id.clone(), at);
        at += clip.timeline_duration - clip.out_transition();
    }
    let main_id = main.id.clone();

    if linkage {
        // Each non-main clip's main clip (where its start lay), before.
        let owners = before.ordered_main_clips();
        let mut links = std::collections::HashMap::new();
        let mut was = std::collections::HashMap::new();
        for track in &before.tracks {
            for clip in &track.clips {
                was.insert(clip.id.clone(), (track.id.clone(), clip.timeline_start));
                if track.kind == TrackType::Video {
                    continue;
                }
                if let Some(owner) = owners.iter().find(|m| {
                    clip.timeline_start >= m.timeline_start && clip.timeline_start < m.end()
                }) {
                    links.insert(clip.id.clone(), (owner.id.clone(), owner.timeline_start));
                }
            }
        }
        for track in after.tracks.iter_mut().filter(|t| t.id != main_id) {
            for clip in track.clips.iter_mut() {
                let (Some((owner, owner_was)), Some((was_track, was_start))) =
                    (links.get(&clip.id), was.get(&clip.id))
                else {
                    continue;
                };
                if *was_track != track.id || *was_start != clip.timeline_start {
                    continue; // the edit moved it itself
                }
                if let Some(now) = start_of.get(owner) {
                    clip.timeline_start = (clip.timeline_start + now - owner_was).max(0);
                }
            }
        }
    }
    after.recompute_duration();
    after
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
            still: false,
        }
    }

    #[test]
    fn append_split_speed_delete() {
        let mut p = Project::empty();
        let a = p.insert_media(&media(4), None);
        let b = p.insert_media(&media(2), None);
        assert_eq!(p.duration, 6 * SECOND);
        assert!(p.split_at(&a, SECOND));
        assert_eq!(p.tracks[0].clips.len(), 3);
        assert_eq!(p.tracks[0].clips[1].source_start, Some(SECOND));
        // Speed and delete leave gaps; the magnet closes them.
        let before = p.clone();
        p.set_speed(&b, 2.0);
        p = with_magnet(&before, p, true);
        assert_eq!(p.duration, 5 * SECOND);
        let before = p.clone();
        p.delete_clip(&a);
        p = with_magnet(&before, p, true);
        assert_eq!(p.duration, 4 * SECOND);
        assert_eq!(p.tracks[0].clips[0].timeline_start, 0);
    }

    #[test]
    fn magnet_carries_linked_clips() {
        let mut p = Project::empty();
        let a = p.insert_media(&media(4), None);
        p.insert_media(&media(4), None);
        // An audio clip starting inside the second main clip (at 5 s).
        let mut music = p.tracks[0].clips[1].clone();
        music.id = "m".into();
        music.kind = "audio".into();
        music.timeline_start = 5 * SECOND;
        p.tracks[1].clips.push(music);
        let before = p.clone();
        p.delete_clip(&a);
        let linked = with_magnet(&before, p.clone(), true);
        assert_eq!(linked.tracks[1].clips[0].timeline_start, SECOND);
        let unlinked = with_magnet(&before, p, false);
        assert_eq!(unlinked.tracks[1].clips[0].timeline_start, 5 * SECOND);
    }

    #[test]
    fn delete_left_right_trims_source() {
        let mut p = Project::empty();
        let a = p.insert_media(&media(4), None);
        assert!(p.trim_at(&a, SECOND, Side::Start));
        let c = &p.tracks[0].clips[0];
        assert_eq!(
            (c.timeline_start, c.timeline_duration),
            (SECOND, 3 * SECOND)
        );
        assert_eq!(c.source_start, Some(SECOND));
        assert_eq!(c.source_duration, Some(3 * SECOND));
        assert!(p.trim_at(&a, 3 * SECOND, Side::End));
        let c = &p.tracks[0].clips[0];
        assert_eq!(c.timeline_duration, 2 * SECOND);
        assert_eq!(c.source_start, Some(SECOND));
    }

    #[test]
    fn duplicate_and_paste_on_main_make_room() {
        let mut p = Project::empty();
        let a = p.insert_media(&media(2), None);
        let b = p.insert_media(&media(3), None);
        let copy = p.duplicate_clip(&a).unwrap();
        let start = |p: &Project, id: &str| p.find_clip(id).unwrap().1.timeline_start;
        assert_eq!(start(&p, &copy), 2 * SECOND);
        assert_eq!(start(&p, &b), 4 * SECOND);
        let clip = p.find_clip(&a).unwrap().1.clone();
        let main = p.tracks[0].id.clone();
        // Playhead inside b (4–7 s): pasted at the cut after b.
        let pasted = p
            .paste_clip(&clip, &main, TrackType::Video, 5 * SECOND)
            .unwrap();
        assert_eq!(start(&p, &pasted), 7 * SECOND);
        assert_eq!(p.duration, 9 * SECOND);
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
