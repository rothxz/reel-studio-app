//! The playback engine the editor commands. With `--features cpp-engine` it
//! is the shared C++ core (the same `videoengine::Engine` the phone apps
//! link), through `engine-shim/reel_engine.cpp`. Without it, a small Rust
//! stand-in with the same surface keeps the spike buildable from this repo.

use crate::model::{Micros, Project};

/// What a visible / audible clip is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerKind {
    Video,
    Audio,
    Image,
    Text,
    Sticker,
    Effect,
    /// Drawings and anything newer.
    Other,
}

impl LayerKind {
    /// The stand-in engine reads kinds from the document.
    #[cfg_attr(reel_cpp_engine, allow(dead_code))]
    fn of(kind: &str) -> Self {
        match kind {
            "video" => LayerKind::Video,
            "audio" => LayerKind::Audio,
            "image" => LayerKind::Image,
            "text" => LayerKind::Text,
            "sticker" => LayerKind::Sticker,
            "effect" => LayerKind::Effect,
            _ => LayerKind::Other,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ActiveLayer {
    pub clip_id: String,
    pub source: String,
    /// Source time to show, already through speed / curve / reverse / freeze.
    pub source_time: Micros,
    /// How far into the clip (timeline time).
    pub offset: Micros,
    /// A video / photo layer (decoded from its file).
    pub is_visual: bool,
    pub kind: LayerKind,
}

#[cfg(reel_cpp_engine)]
mod imp {
    use super::*;
    use std::ffi::{CStr, CString, c_char};

    #[repr(C)]
    struct RawEngine {
        _private: [u8; 0],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct RawLayer {
        clip_id: *const c_char,
        source: *const c_char,
        source_time: i64,
        offset_into_clip: i64,
        kind: i32,
        track_type: i32,
        track_order: i32,
    }

    unsafe extern "C" {
        fn reel_engine_new() -> *mut RawEngine;
        fn reel_engine_free(e: *mut RawEngine);
        fn reel_engine_load_project(e: *mut RawEngine, json: *const c_char) -> i32;
        fn reel_engine_last_error(e: *mut RawEngine) -> *const c_char;
        fn reel_engine_play(e: *mut RawEngine);
        fn reel_engine_pause(e: *mut RawEngine);
        fn reel_engine_seek(e: *mut RawEngine, micros: i64);
        fn reel_engine_tick(e: *mut RawEngine) -> i64;
        fn reel_engine_state(e: *mut RawEngine) -> i32;
        fn reel_engine_active_layers(
            e: *mut RawEngine,
            micros: i64,
            out: *mut RawLayer,
            cap: usize,
        ) -> usize;
        fn reel_filter_color_matrix(name: *const c_char, intensity: f64, out: *mut f64) -> i32;
        fn reel_adjust_color_matrix(values: *const f64, out: *mut f64) -> i32;
        fn reel_engine_clip_property(
            e: *mut RawEngine,
            clip_id: *const c_char,
            property: *const c_char,
            local: i64,
            out: *mut f64,
        ) -> i32;
        fn reel_beats_new(sample_rate: i32) -> *mut RawBeats;
        fn reel_beats_process(
            b: *mut RawBeats,
            interleaved: *const f32,
            frames: usize,
            channels: i32,
        );
        fn reel_beats_finish(b: *mut RawBeats, duration_micros: i64) -> *mut c_char;
        fn reel_beats_free(b: *mut RawBeats);
        fn reel_string_free(s: *mut c_char);
        fn reel_beats_version() -> i32;
    }

    #[repr(C)]
    struct RawBeats {
        _private: [u8; 0],
    }

    /// The engine's streaming beat analyzer (`AudioBeatAnalyzer`): PCM in
    /// chunks, then the result in its JSON wire format.
    pub struct BeatAnalyzer(*mut RawBeats);

    // The analyzer is plain data owned by this handle.
    unsafe impl Send for BeatAnalyzer {}

    impl BeatAnalyzer {
        /// The `analysisVersion` its results carry.
        pub fn version() -> String {
            unsafe { reel_beats_version() }.to_string()
        }

        pub fn new(sample_rate: u32) -> Self {
            BeatAnalyzer(unsafe { reel_beats_new(sample_rate as i32) })
        }

        /// Interleaved samples, `channels` per frame.
        pub fn process(&mut self, interleaved: &[f32], channels: usize) {
            let channels = channels.max(1);
            let frames = interleaved.len() / channels;
            unsafe { reel_beats_process(self.0, interleaved.as_ptr(), frames, channels as i32) }
        }

        pub fn finish(mut self, duration: Micros) -> String {
            let raw = std::mem::replace(&mut self.0, std::ptr::null_mut());
            let json = unsafe { reel_beats_finish(raw, duration) };
            if json.is_null() {
                return String::new();
            }
            let out = unsafe { CStr::from_ptr(json) }
                .to_string_lossy()
                .into_owned();
            unsafe { reel_string_free(json) };
            out
        }
    }

    impl Drop for BeatAnalyzer {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { reel_beats_free(self.0) }
            }
        }
    }

    /// A Classic filter's colour matrix at `intensity` (0–100), from the
    /// engine's preset table; None when it's the identity.
    pub fn filter_color_matrix(name: &str, intensity: f64) -> Option<[f32; 20]> {
        let name = CString::new(name).ok()?;
        let mut out = [0f64; 20];
        let on = unsafe { reel_filter_color_matrix(name.as_ptr(), intensity, out.as_mut_ptr()) };
        (on != 0).then(|| out.map(|v| v as f32))
    }

    /// The Adjust sliders' colour matrix (values in `adjust::CONTROLS`
    /// order, as stored), built by the engine; None when they're all 0.
    pub fn adjust_color_matrix(values: &[f64; 7]) -> Option<[f32; 20]> {
        let mut out = [0f64; 20];
        let on = unsafe { reel_adjust_color_matrix(values.as_ptr(), out.as_mut_ptr()) };
        (on != 0).then(|| out.map(|v| v as f32))
    }

    // videoengine::PlaybackState
    const PLAYING: i32 = 2;
    // videoengine::ClipKind: Video, Audio, Image, Text, Sticker, Effect
    const KINDS: [LayerKind; 6] = [
        LayerKind::Video,
        LayerKind::Audio,
        LayerKind::Image,
        LayerKind::Text,
        LayerKind::Sticker,
        LayerKind::Effect,
    ];

    pub struct Engine(*mut RawEngine);

    impl Engine {
        pub fn new() -> Self {
            Engine(unsafe { reel_engine_new() })
        }

        pub fn load(&mut self, project: &Project) -> Result<(), String> {
            let json = CString::new(project.to_json()).map_err(|e| e.to_string())?;
            if unsafe { reel_engine_load_project(self.0, json.as_ptr()) } == 0 {
                Ok(())
            } else {
                Err(unsafe { CStr::from_ptr(reel_engine_last_error(self.0)) }
                    .to_string_lossy()
                    .into_owned())
            }
        }

        pub fn play(&mut self) {
            unsafe { reel_engine_play(self.0) }
        }
        pub fn pause(&mut self) {
            unsafe { reel_engine_pause(self.0) }
        }
        pub fn seek(&mut self, t: Micros) {
            unsafe { reel_engine_seek(self.0, t) }
        }
        pub fn tick(&mut self) -> Micros {
            unsafe { reel_engine_tick(self.0) }
        }
        pub fn is_playing(&self) -> bool {
            unsafe { reel_engine_state(self.0) == PLAYING }
        }

        pub fn active_layers(&self, t: Micros) -> Vec<ActiveLayer> {
            let mut raw = [RawLayer {
                clip_id: std::ptr::null(),
                source: std::ptr::null(),
                source_time: 0,
                offset_into_clip: 0,
                kind: 0,
                track_type: 0,
                track_order: 0,
            }; 32];
            let n = unsafe { reel_engine_active_layers(self.0, t, raw.as_mut_ptr(), raw.len()) };
            raw[..n]
                .iter()
                .map(|l| {
                    let kind = KINDS
                        .get(l.kind as usize)
                        .copied()
                        .unwrap_or(LayerKind::Other);
                    ActiveLayer {
                        clip_id: unsafe { CStr::from_ptr(l.clip_id) }
                            .to_string_lossy()
                            .into_owned(),
                        source: unsafe { CStr::from_ptr(l.source) }
                            .to_string_lossy()
                            .into_owned(),
                        source_time: l.source_time,
                        offset: l.offset_into_clip,
                        is_visual: matches!(kind, LayerKind::Video | LayerKind::Image),
                        kind,
                    }
                })
                .collect()
        }

        /// A clip's keyframed property (`transform.x`, `mask.feather`, …)
        /// at clip-local `local`, eased as the renderers ease it; None when
        /// the clip has no keyframes for it.
        pub fn clip_property(&self, clip_id: &str, property: &str, local: Micros) -> Option<f64> {
            let (id, prop) = (CString::new(clip_id).ok()?, CString::new(property).ok()?);
            let mut out = 0f64;
            let found = unsafe {
                reel_engine_clip_property(self.0, id.as_ptr(), prop.as_ptr(), local, &mut out)
            };
            (found == 1).then_some(out)
        }
    }

    impl Drop for Engine {
        fn drop(&mut self) {
            unsafe { reel_engine_free(self.0) }
        }
    }
}

#[cfg(not(reel_cpp_engine))]
mod imp {
    use super::*;
    use crate::model::TrackType;
    use std::time::Instant;

    /// Wall-clock anchored like Engine::TickPlaybackClock, constant speed only.
    pub struct Engine {
        project: Option<Project>,
        time: Micros,
        anchor: Option<(Instant, Micros)>,
    }

    /// The Classic filters' matrices live in the C++ engine; the stand-in
    /// has none (those looks show unfiltered).
    pub fn filter_color_matrix(_name: &str, _intensity: f64) -> Option<[f32; 20]> {
        None
    }

    /// Without the engine, beats come from our own small analyzer
    /// (`beats::standin`), same wire format.
    pub use crate::beats::standin::Analyzer as BeatAnalyzer;

    /// Likewise the Adjust matrix: the stand-in shows the clip unadjusted.
    pub fn adjust_color_matrix(_values: &[f64; 7]) -> Option<[f32; 20]> {
        None
    }

    impl Engine {
        pub fn new() -> Self {
            Engine {
                project: None,
                time: 0,
                anchor: None,
            }
        }
        pub fn load(&mut self, project: &Project) -> Result<(), String> {
            // Like Engine::LoadProject: paused at 0 (the editor seeks after).
            self.project = Some(project.clone());
            self.time = 0;
            self.anchor = None;
            Ok(())
        }
        pub fn play(&mut self) {
            self.anchor = Some((Instant::now(), self.time));
        }
        pub fn pause(&mut self) {
            self.tick();
            self.anchor = None;
        }
        /// Like Engine::Seek: lands paused.
        pub fn seek(&mut self, t: Micros) {
            let end = self.project.as_ref().map_or(0, |p| p.duration);
            self.time = t.clamp(0, end);
            self.anchor = None;
        }
        pub fn tick(&mut self) -> Micros {
            if let Some((wall, at)) = self.anchor {
                let end = self.project.as_ref().map_or(0, |p| p.duration);
                self.time = at + wall.elapsed().as_micros() as Micros;
                if self.time >= end {
                    self.time = end;
                    self.anchor = None;
                }
            }
            self.time
        }
        pub fn is_playing(&self) -> bool {
            self.anchor.is_some()
        }
        pub fn active_layers(&self, t: Micros) -> Vec<ActiveLayer> {
            let Some(p) = &self.project else {
                return vec![];
            };
            let mut tracks: Vec<_> = p.tracks.iter().filter(|t| !t.hidden).collect();
            tracks.sort_by(|a, b| a.order.total_cmp(&b.order));
            tracks
                .into_iter()
                .flat_map(|track| {
                    track
                        .clips
                        .iter()
                        .filter(move |c| !c.hidden && t >= c.timeline_start && t < c.end())
                        .map(move |c| ActiveLayer {
                            clip_id: c.id.clone(),
                            source: c.source.clone().unwrap_or_default(),
                            // Constant speed only; a still has no source time.
                            source_time: c.source_start.unwrap_or(0)
                                + if c.kind == "image" {
                                    0
                                } else {
                                    ((t - c.timeline_start) as f64 * c.speed) as Micros
                                },
                            offset: t - c.timeline_start,
                            is_visual: matches!(track.kind, TrackType::Video | TrackType::Overlay)
                                && (c.kind == "video" || c.kind == "image"),
                            kind: LayerKind::of(&c.kind),
                        })
                })
                .collect()
        }

        /// The C++ engine eases keyframes; the stand-in interpolates
        /// linearly (holds hold).
        pub fn clip_property(&self, clip_id: &str, property: &str, local: Micros) -> Option<f64> {
            let (_, clip) = self.project.as_ref()?.find_clip(clip_id)?;
            crate::visual::linear_track_value(clip, property, local)
        }
    }
}

pub use imp::{BeatAnalyzer, Engine, adjust_color_matrix, filter_color_matrix};

/// A clip's property at clip-local `local`: its keyframes as the engine
/// eases them, else the clip's own value.
pub fn clip_property_at(
    engine: &Engine,
    clip: &crate::model::Clip,
    id: &str,
    local: Micros,
) -> Option<f64> {
    let own = clip.static_property(id);
    if clip.is_animated() {
        engine.clip_property(&clip.id, id, local).or(own)
    } else {
        own
    }
}

/// The pictures among `active` (videos and photos on picture lanes, text,
/// stickers), back to front by stack order: the clip's own, else its
/// lane's (a text / sticker lane can sit under an overlay video).
pub fn picture_layers(project: &Project, mut active: Vec<ActiveLayer>) -> Vec<ActiveLayer> {
    let order = |clip_id: &str| {
        project.find_clip(clip_id).map_or(0., |(t, c)| {
            c.extra
                .get("stackOrder")
                .and_then(|v| v.as_f64())
                .unwrap_or(t.order)
        })
    };
    // One lane's clips by time: through a transition the incoming clip
    // is drawn over the outgoing one.
    let start = |clip_id: &str| {
        project
            .find_clip(clip_id)
            .map_or(0, |(_, c)| c.timeline_start)
    };
    active.sort_by(|a, b| {
        order(&a.clip_id)
            .total_cmp(&order(&b.clip_id))
            .then_with(|| start(&a.clip_id).cmp(&start(&b.clip_id)))
    });
    active.retain(|l| match l.kind {
        LayerKind::Video | LayerKind::Image | LayerKind::Sticker => {
            (l.is_visual || l.kind == LayerKind::Sticker) && !l.source.is_empty()
        }
        LayerKind::Text => true,
        _ => false,
    });
    active
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MediaItem, SECOND};

    /// Classic filters come from the engine's preset table.
    #[cfg(reel_cpp_engine)]
    #[test]
    fn classic_filter_matrices() {
        assert!(filter_color_matrix("VHS", 100.).is_some());
        assert!(filter_color_matrix("VHS", 0.).is_none());
        assert!(filter_color_matrix("No Such Look", 100.).is_none());
        assert!(adjust_color_matrix(&[0.; 7]).is_none());
        // Brightness +50 lifts every channel by half.
        let m = adjust_color_matrix(&[50., 0., 0., 0., 0., 0., 0.]).unwrap();
        assert_eq!((m[0], m[4], m[9], m[14]), (1., 0.5, 0.5, 0.5));
    }

    /// Runs against whichever backend is built: `cargo test --features
    /// cpp-engine` checks that the C++ core accepts the JSON this app writes.
    #[test]
    fn engine_accepts_project_and_maps_speed() {
        let mut p = Project::empty();
        let item = MediaItem {
            path: "/tmp/a.mp4".into(),
            name: "a.mp4".into(),
            duration: 4 * SECOND,
            width: 1920,
            height: 1080,
            has_video: true,
            still: false,
        };
        let a = p.insert_media(&item, None);
        p.insert_media(&item, None);
        let before = p.clone();
        p.set_speed(&a, 2.0); // first clip now 2 s long
        let p = crate::model::with_magnet(&before, p, true);
        let mut e = Engine::new();
        e.load(&p).expect("engine parses the project");
        let at_1s = e.active_layers(SECOND);
        assert_eq!(at_1s.len(), 1);
        assert!(at_1s[0].is_visual);
        assert_eq!(at_1s[0].source_time, 2 * SECOND);
        // Second clip starts at 2 s, so 3 s is 1 s into it.
        assert_eq!(e.active_layers(3 * SECOND)[0].source_time, SECOND);
        e.seek(5 * SECOND);
        assert_eq!(e.tick(), 5 * SECOND);
        assert!(!e.is_playing());
        e.seek(0);
        e.play();
        assert!(e.is_playing());
        std::thread::sleep(std::time::Duration::from_millis(120));
        let t = e.tick();
        assert!(t >= 100_000 && t < 400_000, "clock advanced to {t}");
    }

    /// A ramp re-times the clip to its average speed, and the C++ engine
    /// plays its source through the curve: Flash In runs fast first, so
    /// halfway through its length more than half the source has played.
    #[test]
    fn engine_plays_a_speed_curve() {
        let mut p = Project::empty();
        let item = MediaItem {
            path: "/tmp/a.mp4".into(),
            name: "a.mp4".into(),
            duration: 4 * SECOND,
            width: 1920,
            height: 1080,
            has_video: true,
            still: false,
        };
        let a = p.insert_media(&item, None);
        let ramp = crate::speed_curve::preset_curve("flashIn").unwrap();
        assert!(p.retime_clip(&a, 1., Some(ramp)));
        let clip = p.find_clip(&a).unwrap().1.clone();
        assert!(clip.speed > 1. && clip.speed_curve().is_some());
        assert!(clip.timeline_duration < 4 * SECOND);
        let mut e = Engine::new();
        e.load(&p).expect("engine parses the ramp");
        let half = e.active_layers(clip.timeline_duration / 2);
        assert_eq!(half.len(), 1);
        if cfg!(reel_cpp_engine) {
            assert!(half[0].source_time > 2 * SECOND, "{}", half[0].source_time);
        }
        // A constant speed drops the ramp.
        assert!(p.set_speed(&a, 1.));
        assert!(p.find_clip(&a).unwrap().1.speed_curve().is_none());
        // Back to its source length (to the µs the rounding keeps).
        let back = p.find_clip(&a).unwrap().1.timeline_duration;
        assert!((back - 4 * SECOND).abs() <= 1, "{back}");
    }

    /// Both backends answer a keyframed property at a clip-local time (the
    /// inspector shows these at the playhead); a property without keyframes
    /// has no answer.
    #[test]
    fn engine_evaluates_keyframed_properties() {
        let mut p = Project::empty();
        let item = MediaItem {
            path: "/tmp/a.mp4".into(),
            name: "a.mp4".into(),
            duration: 4 * SECOND,
            width: 1920,
            height: 1080,
            has_video: true,
            still: false,
        };
        let a = p.insert_media(&item, None);
        p.update_clip(&a, |c| {
            c.add_keyframe_at(0, 50_000, &[("transform.x".into(), 0.)]);
            c.add_keyframe_at(2 * SECOND, 50_000, &[("transform.x".into(), 1.)]);
            true
        });
        let mut e = Engine::new();
        e.load(&p).expect("engine parses the keyframes");
        let mid = e.clip_property(&a, "transform.x", SECOND).unwrap();
        assert!((mid - 0.5).abs() < 1e-6, "linear midpoint, got {mid}");
        assert_eq!(e.clip_property(&a, "transform.x", 3 * SECOND), Some(1.));
        assert_eq!(e.clip_property(&a, "transform.y", SECOND), None);
        assert_eq!(e.clip_property("nope", "transform.x", SECOND), None);
    }
}
