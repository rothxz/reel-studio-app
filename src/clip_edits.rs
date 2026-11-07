//! The timeline toolbar's one-shot clip edits on the document: freeze
//! frame, extract audio, replace media, crop, and arranging the layers
//! over the main video. Each is one function the editor runs inside
//! `Editor::edit` (one undo step), following the old editor's rules for
//! the same edit.

use serde_json::{Map, Value, json};

use crate::crop::CropBox;
use crate::model::{Clip, MediaItem, Micros, Project, Track, TrackType, new_id};

/// A split piece or freeze frame shorter than this (one 30 fps frame)
/// isn't worth making.
pub const MIN_PIECE: Micros = 33_333;

/// How long a new freeze frame holds its picture.
pub const FREEZE_HOLD: Micros = 3_000_000;

/// A video this much (about a frame) shorter than the slot still replaces
/// it: that's rounding.
const REPLACE_TOLERANCE: Micros = 40_000;

/// Smallest step between two layers given neighbouring places.
const LAYER_GAP: f64 = 0.001;

/// Why an edit didn't happen, as the toast says it.
pub type Refusal = &'static str;

/// One thing the Layers panel stacks: a whole overlay video lane, or one
/// text / sticker / drawing clip.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LayerKey {
    Track(String),
    Clip(String),
}

/// A stackable layer showing at some time, and the clip its tile shows (an
/// overlay lane's clip at that time).
#[derive(Clone, Debug)]
pub struct LayerItem {
    pub key: LayerKey,
    pub clip_id: String,
}

/// The source a picture must cover to fill `clip`'s slot at its speed.
pub fn replace_source_len(clip: &Clip) -> Micros {
    let speed = if clip.kind == "video" && clip.speed > 0. {
        clip.speed
    } else {
        1.
    };
    (clip.timeline_duration as f64 * speed).round() as Micros
}

/// The clip's stacking place: its own, else its lane's.
fn stack_order(track: &Track, clip: &Clip) -> f64 {
    clip.extra
        .get("stackOrder")
        .and_then(Value::as_f64)
        .unwrap_or(track.order)
}

/// The clip's crop box and straighten angle (None: uncropped).
pub fn crop_of(clip: &Clip) -> Option<(CropBox, f64)> {
    let c = clip.extra.get("crop")?;
    let f = |k: &str| c.get(k).and_then(Value::as_f64);
    let b = CropBox {
        x: f("x").unwrap_or(0.),
        y: f("y").unwrap_or(0.),
        w: f("width").filter(|v| *v > 0.)?,
        h: f("height").filter(|v| *v > 0.)?,
    };
    Some((b, f("rotation").unwrap_or(0.)))
}

impl Project {
    /// Freeze frame: cuts the video clip at `at` and puts a still between
    /// the halves, the picture showing at `at` (`frozen_source`, its source
    /// time) held for `hold`. The second half and everything after it on
    /// the lane move right by the hold. The still is a plain video clip
    /// flagged `freeze` (and silent), so it trims and plays like any other.
    /// Returns the still's id.
    pub fn freeze_frame(
        &mut self,
        clip_id: &str,
        at: Micros,
        frozen_source: Micros,
        hold: Micros,
    ) -> Result<String, Refusal> {
        let (ti, ci) = self.position_of(clip_id).ok_or("Select a clip first")?;
        let clip = self.tracks[ti].clips[ci].clone();
        if clip.kind != "video" {
            return Err("Only a video can freeze a frame");
        }
        if clip.freeze() {
            return Err("This clip is already a freeze frame");
        }
        if at - clip.timeline_start < MIN_PIECE || clip.end() - at < MIN_PIECE {
            return Err("Move the playhead inside the clip to freeze a frame");
        }
        let (start, end, overlap) = (clip.timeline_start, clip.end(), clip.out_transition());
        if !self.split_at(clip_id, at) {
            return Err("Move the playhead inside the clip to freeze a frame");
        }

        // The still starts clean: no in / out animation, no transition, no
        // keyframes of the clip it came from, plays forward.
        let mut still = clip;
        still.id = new_id("clip");
        for key in [
            "animations",
            "keyframes",
            "outTransition",
            "reversed",
            "reversedMedia",
        ] {
            still.extra.remove(key);
        }
        still.timeline_start = at;
        still.timeline_duration = hold;
        still.source_start = Some(frozen_source.max(0));
        still.speed = 1.;
        still.extra.insert("freeze".into(), Value::Bool(true));
        still.extra.insert("muted".into(), Value::Bool(true));
        let still_id = still.id.clone();

        let track = &mut self.tracks[ti];
        // The transition at the clip's end belongs to the last piece.
        track.clips[ci].extra.remove("outTransition");
        let right_id = track.clips[ci + 1].id.clone();
        for c in track.clips.iter_mut() {
            if c.id == right_id || (c.timeline_start >= end - overlap && c.timeline_start > start) {
                c.timeline_start += hold;
            }
        }
        track.clips.insert(ci + 1, still);
        self.recompute_duration();
        Ok(still_id)
    }

    /// Extract audio: the video clip's sound as its own clip on the first
    /// unlocked audio lane (made when the project has none), over the same
    /// stretch of time; the video is muted. Returns the new clip's id.
    pub fn extract_audio(&mut self, clip_id: &str) -> Result<String, Refusal> {
        let (_, clip) = self.find_clip(clip_id).ok_or("Select a clip first")?;
        if clip.kind != "video" {
            return Err("This clip has no audio");
        }
        if clip.freeze() {
            return Err("A freeze frame has no audio");
        }
        let has_audio_lane = self.tracks.iter().any(|t| t.kind == TrackType::Audio);
        if has_audio_lane
            && !self
                .tracks
                .iter()
                .any(|t| t.kind == TrackType::Audio && !t.locked)
        {
            return Err("No unlocked audio track to extract to");
        }
        let speed = if clip.speed > 0. { clip.speed } else { 1. };
        let segment = (clip.timeline_duration as f64 * speed).round() as Micros;
        let source_start = clip.source_start.unwrap_or(0);
        // A reversed clip with its reversed copy plays that copy forward:
        // its (already backwards) sound over the same segment.
        let reversed_copy = clip
            .reversed()
            .then(|| clip.extra.get("reversedMedia"))
            .flatten()
            .and_then(|m| {
                let uri = m.get("uri")?.as_str()?.to_string();
                let length = m.get("durationMicros")?.as_f64()? as Micros;
                Some((uri, length))
            });
        let (source, start, source_len) = match reversed_copy {
            Some((uri, length)) => (uri, (length - (source_start + segment)).max(0), segment),
            None => (
                clip.source.clone().unwrap_or_default(),
                source_start,
                clip.source_duration.unwrap_or(segment),
            ),
        };
        let number = self
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.kind == "audio")
            .filter_map(|c| c.extra.get("extractedNumber").and_then(Value::as_u64))
            .max()
            .unwrap_or(0)
            + 1;
        let mut extra = Map::new();
        extra.insert(
            "transform".into(),
            json!({ "x": 0, "y": 0, "scale": 1, "rotation": 0, "opacity": 1 }),
        );
        extra.insert("effects".into(), json!([]));
        extra.insert("volume".into(), json!(clip.volume()));
        extra.insert("muted".into(), Value::Bool(false));
        extra.insert("extractedNumber".into(), json!(number));
        let id = new_id("clip");
        let audio = Clip {
            id: id.clone(),
            kind: "audio".into(),
            timeline_start: clip.timeline_start,
            timeline_duration: clip.timeline_duration,
            speed: clip.speed,
            source: Some(source),
            source_start: Some(start),
            source_duration: Some(source_len),
            text: None,
            hidden: false,
            extra,
        };
        self.clip_mut(clip_id)
            .expect("found above")
            .set_flag("muted", true);
        match self
            .tracks
            .iter_mut()
            .find(|t| t.kind == TrackType::Audio && !t.locked)
        {
            Some(lane) => lane.clips.push(audio),
            None => self.tracks.push(Track {
                id: new_id("track"),
                kind: TrackType::Audio,
                name: "Audio".into(),
                clips: vec![audio],
                order: 1.,
                muted: false,
                hidden: false,
                locked: false,
            }),
        }
        self.recompute_duration();
        Ok(id)
    }

    /// Replace: the video / photo clip shows `media` instead, in the same
    /// slot (start, length, speed), keeping everything laid on the picture
    /// (transform, keyframes, effects, animations, transition, mask,
    /// blend, background, volume). What belonged to the old picture goes:
    /// crop, cutout, reversed copy, freeze, camera tracking (and the path
    /// it wrote). A video must be long enough to fill the slot.
    pub fn replace_media(&mut self, clip_id: &str, media: &MediaItem) -> Result<(), Refusal> {
        let clip = self.clip_mut(clip_id).ok_or("Select a clip first")?;
        if !matches!(clip.kind.as_str(), "video" | "image") {
            return Err("Only a video or photo can be replaced");
        }
        if !media.has_video {
            return Err("Pick a video or a photo");
        }
        let needed = replace_source_len(clip);
        if !media.still && media.duration + REPLACE_TOLERANCE < needed {
            return Err("too short");
        }
        let camera_tracked = clip.extra.contains_key("cameraTrack");
        for key in [
            "crop",
            "cutout",
            "reversed",
            "reversedMedia",
            "freeze",
            "cameraTrack",
        ] {
            clip.extra.remove(key);
        }
        if camera_tracked {
            clip.extra.remove("motionPath");
        }
        clip.source = Some(media.path.clone());
        if !media.still {
            if clip.kind == "image" {
                clip.extra.insert("volume".into(), json!(1));
                clip.extra.insert("muted".into(), Value::Bool(false));
            }
            clip.kind = "video".into();
            clip.source_start = Some(0);
            clip.source_duration = Some(needed.min(media.duration));
        } else if clip.kind == "video" {
            // Video -> photo: nothing left to play or hear.
            clip.kind = "image".into();
            clip.source_start = None;
            clip.source_duration = None;
            clip.speed = 1.;
            for key in ["speedCurve", "volume", "muted", "audioFade", "videoFade"] {
                clip.extra.remove(key);
            }
            if let Some(Value::Array(tracks)) = clip.extra.get_mut("keyframes") {
                tracks.retain(|t| {
                    !t.get("property")
                        .and_then(Value::as_str)
                        .is_some_and(|p| p.starts_with("audio."))
                });
            }
        }
        Ok(())
    }

    /// Sets (None: clears) a video / photo clip's crop: the box in its
    /// source (0–1) and a straighten angle in degrees.
    pub fn set_crop(&mut self, clip_id: &str, crop: Option<(CropBox, f64)>) -> bool {
        self.update_clip(clip_id, |clip| {
            if !matches!(clip.kind.as_str(), "video" | "image") {
                return false;
            }
            let before = clip.extra.get("crop").cloned();
            match crop {
                Some((b, rotation)) => {
                    let mut c = json!({ "x": b.x, "y": b.y, "width": b.w, "height": b.h });
                    if rotation != 0. {
                        c["rotation"] = json!(rotation);
                    }
                    clip.extra.insert("crop".into(), c);
                }
                None => {
                    clip.extra.remove("crop");
                }
            }
            clip.extra.get("crop").cloned() != before
        })
    }

    /// What can be stacked at `t`, front-most first, in the order the
    /// player paints it: an overlay video lane is one layer however many
    /// clips it holds; text, stickers and drawings are one layer per clip.
    /// The main video is the base and isn't listed.
    pub fn arrangeable_layers_at(&self, t: Micros) -> Vec<LayerItem> {
        let mut tracks: Vec<&Track> = self.tracks.iter().filter(|tr| !tr.hidden).collect();
        tracks.sort_by(|a, b| a.order.total_cmp(&b.order));
        let mut painted: Vec<(f64, LayerItem)> = vec![];
        for track in tracks {
            let overlay_video = track.is_overlay_video(&self.id);
            let showing = track
                .clips
                .iter()
                .filter(|c| !c.hidden && t >= c.timeline_start && t < c.end());
            for clip in showing {
                let item = match clip.kind.as_str() {
                    "video" | "image" if overlay_video => {
                        let key = LayerKey::Track(track.id.clone());
                        if painted.iter().any(|(_, l)| l.key == key) {
                            continue;
                        }
                        (
                            track.order,
                            LayerItem {
                                key,
                                clip_id: clip.id.clone(),
                            },
                        )
                    }
                    "text" | "sticker" | "drawing" if track.kind != TrackType::Video => (
                        stack_order(track, clip),
                        LayerItem {
                            key: LayerKey::Clip(clip.id.clone()),
                            clip_id: clip.id.clone(),
                        },
                    ),
                    _ => continue,
                };
                painted.push(item);
            }
        }
        painted.sort_by(|a, b| a.0.total_cmp(&b.0));
        painted.into_iter().rev().map(|(_, l)| l).collect()
    }

    /// Re-stacks the listed layers (front-most first): they're handed the
    /// same set of places they held, reshuffled (ties pulled apart by a
    /// hair). A lane moves its `order`; a clip sets its own `stackOrder`.
    /// Anything not listed keeps its place.
    pub fn reorder_layers(&mut self, front_to_back: &[LayerKey]) -> bool {
        let mut current: Vec<f64> = vec![];
        for key in front_to_back {
            let order = match key {
                LayerKey::Track(id) => self.tracks.iter().find(|t| &t.id == id).map(|t| t.order),
                LayerKey::Clip(id) => self.find_clip(id).map(|(t, c)| stack_order(t, c)),
            };
            match order {
                Some(o) => current.push(o),
                None => return false,
            }
        }
        current.sort_by(|a, b| b.total_cmp(a));
        let mut slots: Vec<f64> = Vec::with_capacity(current.len());
        for (i, order) in current.into_iter().enumerate() {
            slots.push(if i == 0 {
                order
            } else {
                order.min(slots[i - 1] - LAYER_GAP)
            });
        }
        let mut changed = false;
        for (key, slot) in front_to_back.iter().zip(slots) {
            match key {
                LayerKey::Track(id) => {
                    if let Some(t) = self.tracks.iter_mut().find(|t| &t.id == id)
                        && t.order != slot
                    {
                        t.order = slot;
                        changed = true;
                    }
                }
                LayerKey::Clip(id) => {
                    let Some(c) = self.clip_mut(id) else { continue };
                    if c.extra.get("stackOrder").and_then(Value::as_f64) != Some(slot) {
                        c.extra.insert("stackOrder".into(), json!(slot));
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    /// (lane index, clip index) of a clip.
    fn position_of(&self, clip_id: &str) -> Option<(usize, usize)> {
        self.tracks.iter().enumerate().find_map(|(ti, t)| {
            t.clips
                .iter()
                .position(|c| c.id == clip_id)
                .map(|ci| (ti, ci))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SECOND;

    fn video(path: &str, secs: i64) -> MediaItem {
        MediaItem {
            path: path.into(),
            name: path.into(),
            duration: secs * SECOND,
            width: 1920,
            height: 1080,
            has_video: true,
            still: false,
        }
    }

    fn photo(path: &str) -> MediaItem {
        MediaItem {
            still: true,
            duration: 3 * SECOND,
            ..video(path, 3)
        }
    }

    fn main_ids(p: &Project) -> Vec<(String, Micros, Micros)> {
        p.ordered_main_clips()
            .iter()
            .map(|c| (c.id.clone(), c.timeline_start, c.timeline_duration))
            .collect()
    }

    #[test]
    fn freeze_splits_holds_and_ripples() {
        let mut p = Project::empty();
        let a = p.insert_media(&video("/a.mp4", 4), None);
        let b = p.insert_media(&video("/b.mp4", 2), None);
        let still = p
            .freeze_frame(&a, SECOND, 5 * SECOND / 4, FREEZE_HOLD)
            .unwrap();
        let clips = main_ids(&p);
        assert_eq!(clips.len(), 4);
        assert_eq!(clips[0], (a.clone(), 0, SECOND));
        assert_eq!(clips[1], (still.clone(), SECOND, FREEZE_HOLD));
        assert_eq!(clips[2].1, SECOND + FREEZE_HOLD);
        assert_eq!(clips[2].2, 3 * SECOND);
        assert_eq!(clips[3], (b, 4 * SECOND + FREEZE_HOLD, 2 * SECOND));
        let (_, s) = p.find_clip(&still).unwrap();
        assert!(s.freeze() && s.muted());
        assert_eq!(s.source_start, Some(5 * SECOND / 4));
        assert_eq!(p.duration, 9 * SECOND);
        // A still doesn't freeze again; too near an edge is refused.
        assert!(p.freeze_frame(&still, 2 * SECOND, 0, FREEZE_HOLD).is_err());
        assert!(p.freeze_frame(&a, 10, 0, FREEZE_HOLD).is_err());
    }

    #[test]
    fn extract_audio_mutes_the_video_and_numbers_the_copy() {
        let mut p = Project::empty();
        let a = p.insert_media(&video("/a.mp4", 4), None);
        let first = p.extract_audio(&a).unwrap();
        let (lane, audio) = p.find_clip(&first).unwrap();
        assert_eq!(lane.kind, TrackType::Audio);
        assert_eq!(audio.source.as_deref(), Some("/a.mp4"));
        assert_eq!(
            (audio.timeline_start, audio.timeline_duration),
            (0, 4 * SECOND)
        );
        assert_eq!(audio.label(), "Extracted audio 1");
        assert!(p.find_clip(&a).unwrap().1.muted());
        let second = p.extract_audio(&a).unwrap();
        assert_eq!(p.find_clip(&second).unwrap().1.label(), "Extracted audio 2");
        for t in p.tracks.iter_mut().filter(|t| t.kind == TrackType::Audio) {
            t.locked = true;
        }
        assert!(p.extract_audio(&a).is_err());
    }

    #[test]
    fn replace_keeps_the_slot_and_checks_length() {
        let mut p = Project::empty();
        let a = p.insert_media(&video("/a.mp4", 4), None);
        p.update_clip(&a, |c| {
            c.extra.insert(
                "crop".into(),
                json!({"x": 0.1, "y": 0, "width": 0.5, "height": 1}),
            );
            c.extra.insert("volume".into(), json!(0.5));
            true
        });
        assert_eq!(
            p.replace_media(&a, &video("/short.mp4", 2)),
            Err("too short")
        );
        p.replace_media(&a, &video("/b.mp4", 9)).unwrap();
        let (_, c) = p.find_clip(&a).unwrap();
        assert_eq!(c.source.as_deref(), Some("/b.mp4"));
        assert_eq!(
            (c.source_start, c.source_duration),
            (Some(0), Some(4 * SECOND))
        );
        assert!(!c.extra.contains_key("crop"));
        assert_eq!(c.volume(), 0.5);
        // Video -> photo: no source timing, no sound.
        p.replace_media(&a, &photo("/c.jpg")).unwrap();
        let (_, c) = p.find_clip(&a).unwrap();
        assert_eq!(c.kind, "image");
        assert_eq!(c.source_duration, None);
        assert!(!c.extra.contains_key("volume"));
        assert_eq!(c.timeline_duration, 4 * SECOND);
    }

    #[test]
    fn crop_round_trips() {
        let mut p = Project::empty();
        let a = p.insert_media(&video("/a.mp4", 4), None);
        let b = CropBox {
            x: 0.25,
            y: 0.,
            w: 0.5,
            h: 1.,
        };
        assert!(p.set_crop(&a, Some((b, 4.5))));
        assert!(!p.set_crop(&a, Some((b, 4.5))));
        assert_eq!(crop_of(p.find_clip(&a).unwrap().1), Some((b, 4.5)));
        assert!(p.set_crop(&a, None));
        assert_eq!(crop_of(p.find_clip(&a).unwrap().1), None);
    }

    #[test]
    fn layers_list_front_first_and_reorder() {
        let mut p = Project::empty();
        p.insert_media(&video("/main.mp4", 6), None);
        let over = p.insert_media_at(&video("/o.mp4", 6), 0);
        let text = p.add_text_clip(0, "Hi");
        let lane = p.track_of(&over).unwrap().id.clone();
        let layers = p.arrangeable_layers_at(SECOND);
        let keys: Vec<LayerKey> = layers.iter().map(|l| l.key.clone()).collect();
        assert_eq!(
            keys,
            vec![LayerKey::Clip(text.clone()), LayerKey::Track(lane.clone())]
        );
        // The overlay to the front.
        assert!(p.reorder_layers(&[LayerKey::Track(lane.clone()), LayerKey::Clip(text.clone())]));
        let keys: Vec<LayerKey> = p
            .arrangeable_layers_at(SECOND)
            .into_iter()
            .map(|l| l.key)
            .collect();
        assert_eq!(keys, vec![LayerKey::Track(lane), LayerKey::Clip(text)]);
    }
}
