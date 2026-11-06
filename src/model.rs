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
    /// Stacking order (higher draws on top). The app can store halves
    /// (a lane slotted between two others), so it isn't an integer.
    pub order: f64,
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

    /// Locked on its own (the lane's lock is on the track).
    pub fn locked(&self) -> bool {
        self.flag("locked")
    }

    /// "Mute clip audio" (video and audio clips).
    pub fn muted(&self) -> bool {
        self.flag("muted")
    }

    /// An auto caption: a text clip that carries its caption data. It
    /// lives on the text track but the timeline gives captions a lane of
    /// their own.
    pub fn is_caption(&self) -> bool {
        self.kind == "text" && self.extra.contains_key("caption")
    }

    /// Sets a lane switch on the clip itself (`locked`, `muted`; `hidden`
    /// is a field): the lanes that share a track switch their own clips.
    pub fn set_flag(&mut self, key: &str, on: bool) {
        if key == "hidden" {
            self.hidden = on;
        } else if on {
            self.extra.insert(key.into(), Value::Bool(true));
        } else {
            self.extra.remove(key);
        }
    }

    /// The clip's keyframe times (clip-local, sorted): every enabled
    /// property track's, with times closer than `eps` counted once (one
    /// diamond on the timeline).
    pub fn keyframe_times(&self, eps: Micros) -> Vec<Micros> {
        let mut all: Vec<Micros> = self
            .extra
            .get("keyframes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|t| t.get("enabled").and_then(Value::as_bool).unwrap_or(true))
            .flat_map(|t| {
                t.get("keyframes")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|k| k.get("time").and_then(Value::as_f64))
                    .map(|t| t.round() as Micros)
            })
            .collect();
        all.sort_unstable();
        let mut out: Vec<Micros> = vec![];
        for t in all {
            if out.last().is_none_or(|&last| t - last > eps) {
                out.push(t);
            }
        }
        out
    }

    /// Moves the diamond at `from` to `to` (clamped to the clip) on every
    /// property track: each track's keyframe within `eps` of `from` moves,
    /// and one already at the new time gives way. False when none moved.
    pub fn move_keyframe(&mut self, from: Micros, to: Micros, eps: Micros) -> bool {
        let target = to.clamp(0, self.timeline_duration.max(0));
        let Some(tracks) = self
            .extra
            .get_mut("keyframes")
            .and_then(Value::as_array_mut)
        else {
            return false;
        };
        let time_of = |k: &Value| k.get("time").and_then(Value::as_f64).unwrap_or(0.);
        let mut moved = false;
        for track in tracks {
            let Some(keys) = track.get_mut("keyframes").and_then(Value::as_array_mut) else {
                continue;
            };
            let nearest = keys
                .iter()
                .enumerate()
                .map(|(i, k)| (i, (time_of(k) - from as f64).abs()))
                .filter(|&(_, d)| d <= eps as f64)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i);
            let Some(i) = nearest else { continue };
            let mut key = keys.remove(i);
            key["time"] = Value::from(target);
            keys.retain(|k| (time_of(k) - target as f64).abs() > eps as f64);
            let at = keys
                .iter()
                .position(|k| time_of(k) > target as f64)
                .unwrap_or(keys.len());
            keys.insert(at, key);
            moved = true;
        }
        moved
    }

    /// An audio clip's beat markers inside the stretch it plays, as
    /// (timeline time, strength). Markers are in source time, so moving,
    /// trimming or re-speeding the clip never rewrites them.
    pub fn beats(&self) -> Vec<(Micros, BeatStrength)> {
        if self.kind != "audio" {
            return vec![];
        }
        let speed = if self.speed > 0. { self.speed } else { 1. };
        let source_start = self.source_start.unwrap_or(0);
        self.extra
            .get("beats")
            .and_then(|b| b.get("markers"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| {
                let offset = m.get("time")?.as_f64()? - source_start as f64;
                if offset < 0. || self.source_duration.is_some_and(|d| offset > d as f64) {
                    return None;
                }
                let local = offset / speed;
                if local > self.timeline_duration as f64 {
                    return None;
                }
                let strength = match m.get("strength").and_then(Value::as_str) {
                    Some("strong") => BeatStrength::Strong,
                    Some("medium") => BeatStrength::Medium,
                    _ => BeatStrength::Weak,
                };
                Some((self.timeline_start + local.round() as Micros, strength))
            })
            .collect()
    }

    /// Clip volume, 1 = as recorded.
    pub fn volume(&self) -> f64 {
        self.extra
            .get("volume")
            .and_then(Value::as_f64)
            .unwrap_or(1.)
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

/// How strong a beat marker is (drawn as a bigger dot).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeatStrength {
    Weak,
    Medium,
    Strong,
}

/// Overlay video lanes are `overlay` tracks with ids under this prefix (the
/// app's convention; other `overlay` tracks hold stickers).
pub fn overlay_video_prefix(project_id: &str) -> String {
    format!("{project_id}-overlay-video-")
}

impl Track {
    /// A lane of pictures over the main one (picture in picture).
    pub fn is_overlay_video(&self, project_id: &str) -> bool {
        self.kind == TrackType::Overlay && self.id.starts_with(&overlay_video_prefix(project_id))
    }

    /// Clear of every clip over [start, end).
    fn free_over(&self, start: Micros, end: Micros) -> bool {
        self.clips
            .iter()
            .all(|c| c.timeline_start >= end || c.end() <= start)
    }
}

/// The cut nearest `time` among `clips` (in order, packed): 0 = before the
/// first, `clips.len()` = after the last. Where a clip dragged onto the main
/// lane goes in (the pointer's nearest cut, not its middle).
pub fn nearest_seam_index(clips: &[&Clip], time: Micros) -> usize {
    (0..=clips.len())
        .min_by_key(|&i| (seam_at(clips, i) - time).abs())
        .unwrap_or(0)
}

/// Where cut `index` among `clips` (in order) sits on the timeline now.
pub fn seam_at(clips: &[&Clip], index: usize) -> Micros {
    match clips.get(index) {
        Some(c) => c.timeline_start,
        None => clips.last().map_or(0, |c| c.end()),
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
                    order: 0.,
                    muted: false,
                    hidden: false,
                    locked: false,
                },
                Track {
                    id: new_id("track"),
                    kind: TrackType::Audio,
                    name: "Audio".into(),
                    clips: vec![],
                    order: 1.,
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

    /// Every audio clip's beats in timeline time, sorted (what edits that
    /// align to music snap to), leaving out `exclude`'s own.
    pub fn beat_times(&self, exclude: &str) -> Vec<Micros> {
        let mut times: Vec<Micros> = self
            .tracks
            .iter()
            .filter(|t| t.kind == TrackType::Audio)
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.id != exclude)
            .flat_map(|c| c.beats().into_iter().map(|(t, _)| t))
            .collect();
        times.sort_unstable();
        times
    }

    /// Drags a clip's keyframe diamond from `from` to `to` (clip-local).
    pub fn move_keyframe(&mut self, clip_id: &str, from: Micros, to: Micros, eps: Micros) -> bool {
        from != to
            && self
                .clip_mut(clip_id)
                .is_some_and(|c| c.move_keyframe(from, to, eps))
    }

    pub fn set_clip_hidden(&mut self, clip_id: &str, hidden: bool) {
        if let Some(clip) = self.clip_mut(clip_id) {
            clip.hidden = hidden;
        }
    }

    /// Moves a clip onto `to_track` (its own or another) at `start`. False
    /// when the clip or the lane isn't there, or nothing changes.
    pub fn move_clip(&mut self, clip_id: &str, to_track: &str, start: Micros) -> bool {
        let start = start.max(0);
        let Some(to) = self.tracks.iter().position(|t| t.id == to_track) else {
            return false;
        };
        let Some((from, i)) = self.tracks.iter().enumerate().find_map(|(ti, t)| {
            t.clips
                .iter()
                .position(|c| c.id == clip_id)
                .map(|ci| (ti, ci))
        }) else {
            return false;
        };
        if from == to && self.tracks[from].clips[i].timeline_start == start {
            return false;
        }
        let mut clip = self.tracks[from].clips.remove(i);
        clip.timeline_start = start;
        if from == to {
            self.tracks[to].clips.insert(i, clip);
        } else {
            self.tracks[to].clips.push(clip);
        }
        self.recompute_duration();
        true
    }

    /// A trim handle's drag: the clip's `side` edge moves so it lasts
    /// `duration`, the other edge staying put; the source window follows
    /// (`Clip::trim_source`).
    pub fn trim_edge(&mut self, clip_id: &str, side: Side, duration: Micros) -> bool {
        let Some(clip) = self.clip_mut(clip_id) else {
            return false;
        };
        if duration <= 0 || duration == clip.timeline_duration {
            return false;
        }
        let end = clip.end();
        clip.trim_source(duration, side);
        clip.timeline_duration = duration;
        if side == Side::Start {
            clip.timeline_start = (end - duration).max(0);
        }
        self.recompute_duration();
        true
    }

    /// Puts `clip` into the main lane as clip number `index` (among the
    /// others), the lane packed back to back from 00:00 in its new order.
    fn insert_into_main(&mut self, clip: Clip, index: usize) {
        let Some(main) = self.tracks.iter_mut().find(|t| t.kind == TrackType::Video) else {
            return;
        };
        let mut ordered: Vec<Clip> = main.clips.drain(..).filter(|c| c.id != clip.id).collect();
        ordered.sort_by_key(|c| c.timeline_start);
        ordered.insert(index.min(ordered.len()), clip);
        let mut at = 0;
        for c in ordered.iter_mut() {
            c.timeline_start = at;
            at += c.timeline_duration - c.out_transition();
        }
        main.clips = ordered;
        self.recompute_duration();
    }

    /// Drags a main clip to cut `index`, counted among the other main
    /// clips (0 = first, their count = last).
    pub fn reorder_main(&mut self, clip_id: &str, index: usize) -> bool {
        let Some(clip) = self
            .main_track()
            .and_then(|t| t.clips.iter().find(|c| c.id == clip_id))
            .cloned()
        else {
            return false;
        };
        let before: Vec<String> = self
            .ordered_main_clips()
            .iter()
            .map(|c| c.id.clone())
            .collect();
        self.insert_into_main(clip, index);
        let after: Vec<String> = self
            .ordered_main_clips()
            .iter()
            .map(|c| c.id.clone())
            .collect();
        before != after
    }

    /// Moves a picture (video / image) from another lane into the main
    /// lane at cut `index`.
    pub fn move_into_main(&mut self, clip_id: &str, index: usize) -> bool {
        let Some(ti) = self
            .tracks
            .iter()
            .position(|t| t.kind != TrackType::Video && t.clips.iter().any(|c| c.id == clip_id))
        else {
            return false;
        };
        let ci = self.tracks[ti]
            .clips
            .iter()
            .position(|c| c.id == clip_id)
            .expect("found above");
        if !matches!(self.tracks[ti].clips[ci].kind.as_str(), "video" | "image") {
            return false;
        }
        let clip = self.tracks[ti].clips.remove(ci);
        self.insert_into_main(clip, index);
        true
    }

    /// Moves a main clip onto `lane` (an overlay video lane, added when it's
    /// new) at `start`. Its transition stays behind: only main-lane cuts
    /// have them. The gap it leaves is the magnet's to close.
    pub fn move_out_of_main(&mut self, clip_id: &str, lane: Track, start: Micros) -> bool {
        let Some(main) = self.tracks.iter_mut().find(|t| t.kind == TrackType::Video) else {
            return false;
        };
        let Some(i) = main.clips.iter().position(|c| c.id == clip_id) else {
            return false;
        };
        let mut clip = main.clips.remove(i);
        clip.extra.remove("outTransition");
        clip.timeline_start = start.max(0);
        match self.tracks.iter_mut().find(|t| t.id == lane.id) {
            Some(t) => t.clips.push(clip),
            None => {
                let mut lane = lane;
                lane.clips.push(clip);
                self.tracks.push(lane);
            }
        }
        self.recompute_duration();
        true
    }

    /// An overlay video lane free over [start, end), else a new one (not in
    /// the project yet: the edit that uses it adds it).
    pub fn overlay_lane_for(&self, start: Micros, end: Micros) -> Track {
        let mut overlays: Vec<&Track> = self
            .tracks
            .iter()
            .filter(|t| t.is_overlay_video(&self.id))
            .collect();
        overlays.sort_by(|a, b| a.order.total_cmp(&b.order));
        if let Some(free) = overlays.iter().find(|t| t.free_over(start, end)) {
            return (*free).clone();
        }
        let prefix = overlay_video_prefix(&self.id);
        let mut n = 1;
        while self.tracks.iter().any(|t| t.id == format!("{prefix}{n}")) {
            n += 1;
        }
        Track {
            id: format!("{prefix}{n}"),
            kind: TrackType::Overlay,
            name: format!("Overlay {}", overlays.len() + 1),
            clips: vec![],
            order: self.next_overlay_video_order(),
            muted: false,
            hidden: false,
            locked: false,
        }
    }

    /// A new overlay video lane's order: above the main and overlay video
    /// lanes, below the text / sticker lanes over them.
    fn next_overlay_video_order(&self) -> f64 {
        let base = self
            .tracks
            .iter()
            .filter(|t| t.kind == TrackType::Video || t.is_overlay_video(&self.id))
            .map(|t| t.order)
            .fold(0., f64::max);
        let above = self
            .tracks
            .iter()
            .filter(|t| {
                matches!(t.kind, TrackType::Text | TrackType::Overlay)
                    && !t.is_overlay_video(&self.id)
                    && t.order > base
            })
            .map(|t| t.order)
            .fold(f64::INFINITY, f64::min);
        if above.is_finite() {
            (base + above) / 2.
        } else {
            base + 1.
        }
    }

    /// Adds `media` at `at` off the main lane: a picture on an overlay video
    /// lane free there (a new one when none is), sound on the first audio
    /// lane. Returns the new clip's id.
    pub fn insert_media_at(&mut self, media: &MediaItem, at: Micros) -> String {
        let at = at.max(0);
        let id = new_id("clip");
        let clip = Clip {
            id: id.clone(),
            kind: if !media.has_video {
                "audio"
            } else if media.still {
                "image"
            } else {
                "video"
            }
            .into(),
            timeline_start: at,
            timeline_duration: media.duration,
            speed: 1.0,
            source: Some(media.path.clone()),
            source_start: (!media.still).then_some(0),
            source_duration: (!media.still).then_some(media.duration),
            text: None,
            hidden: false,
            extra: Map::new(),
        };
        if media.has_video {
            let lane = self.overlay_lane_for(at, at + media.duration);
            match self.tracks.iter_mut().find(|t| t.id == lane.id) {
                Some(t) => t.clips.push(clip),
                None => {
                    let mut lane = lane;
                    lane.clips.push(clip);
                    self.tracks.push(lane);
                }
            }
        } else if let Some(t) = self.tracks.iter_mut().find(|t| t.kind == TrackType::Audio) {
            t.clips.push(clip);
        } else {
            self.tracks.push(Track {
                id: new_id("track"),
                kind: TrackType::Audio,
                name: "Audio".into(),
                clips: vec![clip],
                order: 1.,
                muted: false,
                hidden: false,
                locked: false,
            });
        }
        self.recompute_duration();
        id
    }

    /// Removes these clips wherever they are (a lane's "Delete track").
    pub fn delete_clips(&mut self, ids: &[String]) -> bool {
        let mut any = false;
        for id in ids {
            any |= self.delete_clip(id).is_some();
        }
        any
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
    fn trim_edges_keep_the_other_edge() {
        let mut p = Project::empty();
        let a = p.insert_media(&media(4), None);
        // Left handle in by 1 s: the end stays at 4 s, the source starts 1 s in.
        assert!(p.trim_edge(&a, Side::Start, 3 * SECOND));
        let c = p.find_clip(&a).unwrap().1;
        assert_eq!((c.timeline_start, c.end()), (SECOND, 4 * SECOND));
        assert_eq!(c.source_start, Some(SECOND));
        // Out again by half a second: the source window grows back.
        assert!(p.trim_edge(&a, Side::Start, 3 * SECOND + SECOND / 2));
        let c = p.find_clip(&a).unwrap().1;
        assert_eq!(c.source_start, Some(SECOND / 2));
        // Right handle: the start stays put.
        assert!(p.trim_edge(&a, Side::End, 2 * SECOND));
        let c = p.find_clip(&a).unwrap().1;
        assert_eq!(c.timeline_start, SECOND / 2);
        assert_eq!(c.source_duration, Some(2 * SECOND));
    }

    #[test]
    fn reorder_and_lane_changes() {
        let mut p = Project::empty();
        let a = p.insert_media(&media(2), None);
        let b = p.insert_media(&media(3), None);
        let c = p.insert_media(&media(1), None);
        let order = |p: &Project| -> Vec<String> {
            p.ordered_main_clips()
                .iter()
                .map(|c| c.id.clone())
                .collect()
        };
        // a to the end (cut 2 among b, c).
        assert!(p.reorder_main(&a, 2));
        assert_eq!(order(&p), vec![b.clone(), c.clone(), a.clone()]);
        assert_eq!(p.find_clip(&a).unwrap().1.timeline_start, 4 * SECOND);
        assert!(!p.reorder_main(&a, 2), "same place: no edit");

        // c down onto a new overlay lane at 5 s; the magnet closes the gap.
        let before = p.clone();
        let lane = p.overlay_lane_for(5 * SECOND, 6 * SECOND);
        assert!(lane.is_overlay_video(&p.id));
        assert!(p.move_out_of_main(&c, lane, 5 * SECOND));
        let p2 = with_magnet(&before, p.clone(), false);
        assert_eq!(order(&p2), vec![b.clone(), a.clone()]);
        assert_eq!(p2.find_clip(&a).unwrap().1.timeline_start, 3 * SECOND);
        let (lane, clip) = p2.find_clip(&c).unwrap();
        assert!(lane.is_overlay_video(&p2.id));
        assert_eq!(clip.timeline_start, 5 * SECOND);
        // A second clip over the same span gets a second lane.
        let other = p2.overlay_lane_for(5 * SECOND, 6 * SECOND);
        assert_ne!(other.id, lane.id);
        assert!(other.order > lane.order - 1.);

        // Back into the main lane first.
        let mut p3 = p2.clone();
        assert!(p3.move_into_main(&c, 0));
        assert_eq!(order(&p3), vec![c.clone(), b, a]);
        assert_eq!(p3.duration, 6 * SECOND);
    }

    #[test]
    fn nearest_seam() {
        let mut p = Project::empty();
        p.insert_media(&media(2), None);
        p.insert_media(&media(4), None);
        let clips = p.ordered_main_clips();
        assert_eq!(nearest_seam_index(&clips, 0), 0);
        assert_eq!(nearest_seam_index(&clips, 2 * SECOND + 100), 1);
        assert_eq!(nearest_seam_index(&clips, 5 * SECOND), 2);
        assert_eq!(seam_at(&clips, 2), 6 * SECOND);
    }

    #[test]
    fn drop_below_main_lands_on_a_free_overlay() {
        let mut p = Project::empty();
        p.insert_media(&media(4), None);
        let x = p.insert_media_at(&media(2), SECOND);
        let y = p.insert_media_at(&media(2), 2 * SECOND);
        let lx = p.find_clip(&x).unwrap().0.id.clone();
        let ly = p.find_clip(&y).unwrap().0.id.clone();
        assert_ne!(lx, ly, "overlapping drops stack on two lanes");
        let z = p.insert_media_at(&media(1), 3 * SECOND);
        assert_eq!(p.find_clip(&z).unwrap().0.id, lx, "free again at 3 s");
    }

    #[test]
    fn fractional_order_round_trips() {
        let json = r##"{"version":1,"id":"p","name":"n","createdAt":"a","updatedAt":"b",
          "canvas":{"width":1,"height":2,"fps":30,"backgroundColor":"#000"},"duration":0,
          "tracks":[{"id":"t","type":"text","name":"Text","order":1.5,
          "muted":false,"hidden":false,"locked":false,"clips":[]}]}"##;
        let p: Project = serde_json::from_str(json).unwrap();
        assert_eq!(p.tracks[0].order, 1.5);
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

    #[test]
    fn keyframe_diamonds_merge_and_move_on_every_track() {
        let mut c: Clip = serde_json::from_value(serde_json::json!({
            "id": "k", "kind": "video", "timelineStart": 0, "timelineDuration": 5_000_000,
            "keyframes": [
                { "property": "a", "enabled": true, "keyframes": [
                    { "time": 0, "value": 1 }, { "time": 2_000_000, "value": 2 } ] },
                { "property": "b", "enabled": true, "keyframes": [
                    { "time": 2_010_000, "value": 5 }, { "time": 4_000_000, "value": 6 } ] },
                { "property": "c", "enabled": false, "keyframes": [ { "time": 3_000_000 } ] }
            ]
        }))
        .unwrap();
        assert_eq!(c.keyframe_times(50_000), [0, 2_000_000, 4_000_000]);
        assert!(c.move_keyframe(2_000_000, 3_500_000, 50_000));
        assert_eq!(c.keyframe_times(50_000), [0, 3_500_000, 4_000_000]);
        // Onto another diamond: the one there gives way; past the end clamps.
        assert!(c.move_keyframe(3_500_000, 9_000_000, 50_000));
        assert_eq!(c.keyframe_times(50_000), [0, 4_000_000, 5_000_000]);
        assert!(!c.move_keyframe(1_000_000, 0, 50_000));
    }

    #[test]
    fn beats_follow_the_clip_window_and_speed() {
        let c: Clip = serde_json::from_value(serde_json::json!({
            "id": "a", "kind": "audio", "timelineStart": 10_000_000, "timelineDuration": 2_000_000,
            "speed": 2.0, "sourceStart": 1_000_000, "sourceDuration": 4_000_000,
            "beats": { "markers": [
                { "id": "0", "time": 500_000, "strength": "strong" },
                { "id": "1", "time": 1_000_000, "strength": "strong" },
                { "id": "2", "time": 3_000_000, "strength": "medium" },
                { "id": "3", "time": 5_000_000 },
                { "id": "4", "time": 5_500_000 }
            ] }
        }))
        .unwrap();
        assert_eq!(
            c.beats(),
            [
                (10_000_000, BeatStrength::Strong),
                (11_000_000, BeatStrength::Medium),
                (12_000_000, BeatStrength::Weak),
            ]
        );
    }
}
