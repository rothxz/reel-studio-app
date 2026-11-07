//! The selected clip's one-shot actions on the timeline toolbar that need
//! more than a shortcut: Replace (a file pick), Freeze, Extract audio,
//! Overlay (main lane ⇄ overlay lane), and opening Crop and Layers. The
//! document side of each is in `clip_edits.rs`.

use gpui::{Context, PathPromptOptions};

use crate::clip_edits::{FREEZE_HOLD, replace_source_len};
use crate::decoder;
use crate::editor::Editor;
use crate::model::{self, Micros, SECOND};

impl Editor {
    /// The selected clip when it's a video or photo: (id, kind).
    fn selected_picture(&self) -> Option<(String, String)> {
        self.selected_clip()
            .filter(|(_, c)| matches!(c.kind.as_str(), "video" | "image"))
            .map(|(_, c)| (c.id.clone(), c.kind.clone()))
    }

    /// Replace: pick a video or photo; it takes the clip's place, keeping
    /// its edits.
    pub fn replace_selected(&mut self, cx: &mut Context<Self>) {
        let Some((id, _)) = self.selected_picture() else {
            return;
        };
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Replace".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.first()
            {
                let path = path.to_string_lossy().into_owned();
                this.update(cx, |ed, cx| ed.replace_clip_media(&id, &path, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn replace_clip_media(&mut self, clip_id: &str, path: &str, cx: &mut Context<Self>) {
        let item = match decoder::probe(path) {
            Ok(item) => item,
            Err(e) => return self.toast(format!("Can't open {path}: {e}"), cx),
        };
        let Some(needed) = self
            .project
            .find_clip(clip_id)
            .map(|(_, c)| replace_source_len(c))
        else {
            return; // gone while the dialog was up
        };
        let mut result = Ok(());
        self.edit(
            "Replace Clip",
            false,
            None,
            |p| {
                result = p.replace_media(clip_id, &item);
                result.is_ok()
            },
            cx,
        );
        match result {
            Ok(()) => {
                self.add_to_library(item, cx);
                self.toast("Clip replaced", cx);
            }
            Err("too short") => self.toast(
                format!(
                    "Pick a video at least {:.1}s long",
                    needed as f64 / SECOND as f64
                ),
                cx,
            ),
            Err(why) => self.toast(why, cx),
        }
    }

    /// Freeze: the picture under the playhead held for 3 s, between the
    /// clip's halves (what comes after moves right).
    pub fn freeze_selected(&mut self, cx: &mut Context<Self>) {
        let Some((_, clip)) = self.selected_clip() else {
            return;
        };
        let (id, at) = (clip.id.clone(), self.playhead);
        // The source time on screen there: the engine's (speed, curve,
        // reverse), else worked out at constant speed.
        let offset = (at - clip.timeline_start).clamp(0, clip.timeline_duration);
        let walked = if clip.reversed() {
            clip.timeline_duration - offset
        } else {
            offset
        };
        let fallback = clip.source_start.unwrap_or(0) + (walked as f64 * clip.speed) as Micros;
        let frozen = self
            .engine
            .active_layers(at)
            .into_iter()
            .find(|l| l.clip_id == id)
            .map_or(fallback, |l| l.source_time);
        let mut result = Err("");
        self.edit(
            "Freeze Frame",
            false,
            None,
            |p| {
                result = p.freeze_frame(&id, at, frozen, FREEZE_HOLD);
                result.is_ok()
            },
            cx,
        );
        match result {
            Ok(_) => self.toast("Freeze frame added", cx),
            Err(why) => self.toast(why, cx),
        }
    }

    /// Extract audio: the video's sound onto the audio lane as its own
    /// clip; the video goes silent.
    pub fn extract_audio_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected.primary().map(str::to_owned) else {
            return;
        };
        let mut result = Err("");
        self.edit(
            "Extract Audio",
            false,
            None,
            |p| {
                result = p.extract_audio(&id);
                result.is_ok()
            },
            cx,
        );
        match result {
            Ok(_) => self.toast("Audio extracted", cx),
            Err(why) => self.toast(why, cx),
        }
    }

    /// Overlay: a main-lane picture moves onto an overlay lane free over
    /// its time (a new one when none is), the main lane closing up; an
    /// overlay picture goes back into the main lane at the cut nearest its
    /// start (magnet off: where it is).
    pub fn toggle_overlay_selected(&mut self, cx: &mut Context<Self>) {
        let Some((track, clip)) = self.selected_clip() else {
            return;
        };
        let (id, start, end) = (clip.id.clone(), clip.timeline_start, clip.end());
        if self.project.is_main_track(&track.id) {
            let lane = self.project.overlay_lane_for(start, end);
            self.edit(
                "Move Clip",
                true,
                None,
                |p| p.move_out_of_main(&id, lane, start),
                cx,
            );
        } else if track.is_overlay_video(&self.project.id) {
            if self.prefs.main_track_magnet {
                let main = self.project.ordered_main_clips();
                let i = model::nearest_seam_index(&main, start);
                self.edit("Move Clip", true, None, |p| p.move_into_main(&id, i), cx);
            } else if let Some(main) = self.project.main_track().map(|t| t.id.clone()) {
                self.edit(
                    "Move Clip",
                    false,
                    None,
                    |p| p.move_clip(&id, &main, start),
                    cx,
                );
            }
        }
        self.selected.set(id);
        cx.notify();
    }
}
