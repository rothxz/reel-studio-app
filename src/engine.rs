//! The playback engine the editor commands. With `--features cpp-engine` it
//! is the shared C++ core (the same `videoengine::Engine` the phone apps
//! link), through `engine-shim/reel_engine.cpp`. Without it, a small Rust
//! stand-in with the same surface keeps the spike buildable from this repo.

use crate::model::{Micros, Project};

#[derive(Clone, Debug)]
pub struct ActiveLayer {
    pub source: String,
    /// Source time to show, already through speed / curve / reverse / freeze.
    pub source_time: Micros,
    pub is_visual: bool,
}

#[cfg(feature = "cpp-engine")]
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
    }

    // videoengine::PlaybackState
    const PLAYING: i32 = 2;
    // videoengine::ClipKind: Video, Audio, Image, Text, Sticker, Effect
    const KIND_VIDEO: i32 = 0;
    const KIND_IMAGE: i32 = 2;

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
                .map(|l| ActiveLayer {
                    source: unsafe { CStr::from_ptr(l.source) }
                        .to_string_lossy()
                        .into_owned(),
                    source_time: l.source_time,
                    is_visual: l.kind == KIND_VIDEO || l.kind == KIND_IMAGE,
                })
                .collect()
        }
    }

    impl Drop for Engine {
        fn drop(&mut self) {
            unsafe { reel_engine_free(self.0) }
        }
    }
}

#[cfg(not(feature = "cpp-engine"))]
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
                            source: c.source.clone().unwrap_or_default(),
                            // Constant speed only; a still has no source time.
                            source_time: c.source_start.unwrap_or(0)
                                + if c.kind == "image" {
                                    0
                                } else {
                                    ((t - c.timeline_start) as f64 * c.speed) as Micros
                                },
                            is_visual: matches!(track.kind, TrackType::Video | TrackType::Overlay)
                                && (c.kind == "video" || c.kind == "image"),
                        })
                })
                .collect()
        }
    }
}

pub use imp::Engine;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MediaItem, SECOND};

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
}
