//! The inspector's Audio › Duration for a music clip: how long it plays,
//! lengthened or shortened from its end (its start stays put), from half a
//! second up to what is left of the song. The slider's read-out follows
//! the drag and the edit lands on release (one undo step); with auto
//! snapping on it catches the song's beats and the end of the video.
//! Fit to video, Full song and End on a beat set it in one click. Fades
//! that no longer fit are shortened with it.

use gpui::{AnyElement, Context, div, prelude::*, px};

use crate::beats;
use crate::editor::{Editor, timecode};
use crate::model::{Clip, Micros, SECOND, Side, TrackType};
use crate::theme;
use crate::ui::widgets::{panel_note, panel_row, pill_button, section_with};

/// The shortest a music clip gets here.
const MIN_DURATION: Micros = SECOND / 2;
/// The slider catches a target within this share of its range.
const SNAP_SHARE: f64 = 0.015;
const KEY: &str = "audio:duration";

fn secs(t: Micros) -> f64 {
    t as f64 / SECOND as f64
}

/// The target nearest `d` within `reach`, if any.
fn snap_to(d: Micros, targets: &[Micros], reach: Micros) -> Option<Micros> {
    targets
        .iter()
        .copied()
        .filter(|t| (t - d).abs() <= reach)
        .min_by_key(|t| (t - d).abs())
}

/// Fades shortened in proportion when the clip gets shorter than both.
fn fitted_fades(fade_in: Micros, fade_out: Micros, duration: Micros) -> (Micros, Micros) {
    let total = fade_in + fade_out;
    if total <= duration || total == 0 {
        return (fade_in, fade_out);
    }
    let k = duration as f64 / total as f64;
    (
        (fade_in as f64 * k) as Micros,
        (fade_out as f64 * k) as Micros,
    )
}

impl Editor {
    /// The song's whole length: from the library, else its decoded
    /// waveform, else its beat analysis, else what the clip uses.
    fn song_length(&self, clip: &Clip) -> Micros {
        let used = clip.source_start.unwrap_or(0) + clip.source_duration.unwrap_or(0);
        let Some(src) = clip.source.as_ref() else {
            return used;
        };
        let known = self
            .media
            .iter()
            .find(|m| &m.path == src)
            .map(|m| m.duration)
            .or_else(|| {
                let w = self.waves.get(src)?.as_ref()?;
                Some((w.peaks.len() as f64 / w.per_second * SECOND as f64) as Micros)
            })
            .or_else(|| beats::of(clip).map(|m| m.duration).filter(|&d| d > 0));
        known.unwrap_or(used).max(used)
    }

    /// The clip's longest length: to the song's end at its speed.
    fn audio_max_duration(&self, clip: &Clip) -> Micros {
        let speed = if clip.speed > 0. { clip.speed } else { 1. };
        let left = (self.song_length(clip) - clip.source_start.unwrap_or(0)).max(0);
        ((left as f64 / speed) as Micros)
            .max(clip.timeline_duration)
            .max(MIN_DURATION)
    }

    /// Where the main video ends (Fit to video), if after the clip starts.
    fn video_end_after(&self, clip: &Clip) -> Option<Micros> {
        self.project
            .tracks
            .iter()
            .filter(|t| t.kind == TrackType::Video)
            .flat_map(|t| t.clips.iter())
            .map(Clip::end)
            .max()
            .filter(|&end| end > clip.timeline_start)
            .map(|end| end - clip.timeline_start)
    }

    /// Lengths at which the clip ends on one of its song's beats.
    fn beat_lengths(clip: &Clip, max: Micros) -> Vec<Micros> {
        let Some(map) = beats::of(clip) else {
            return vec![];
        };
        let speed = if clip.speed > 0. { clip.speed } else { 1. };
        let start = clip.source_start.unwrap_or(0);
        map.markers
            .iter()
            .map(|m| ((m.time - start) as f64 / speed).round() as Micros)
            .filter(|&d| d >= MIN_DURATION && d <= max)
            .collect()
    }

    /// Sets the clip's length from its end (one undo step).
    fn set_audio_duration(&mut self, clip_id: &str, duration: Micros, cx: &mut Context<Self>) {
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let d = duration.clamp(MIN_DURATION, self.audio_max_duration(clip));
        if d == clip.timeline_duration {
            return;
        }
        let id = clip_id.to_string();
        self.edit(
            "Duration",
            false,
            None,
            move |p| {
                if !p.trim_edge(&id, Side::End, d) {
                    return false;
                }
                if let Some(c) = p.clip_mut(&id) {
                    let (fi, fo) = c.audio_fade();
                    let (nfi, nfo) = fitted_fades(fi, fo, d);
                    if (nfi, nfo) != (fi, fo) {
                        c.extra.insert(
                            "audioFade".into(),
                            serde_json::json!({ "inDurationMicros": nfi, "outDurationMicros": nfo }),
                        );
                    }
                }
                true
            },
            cx,
        );
    }

    /// Audio › Duration.
    pub fn render_audio_duration_panel(&self, clip: &Clip, cx: &mut Context<Self>) -> AnyElement {
        let current = clip.timeline_duration;
        let max = self.audio_max_duration(clip);
        let fit = self
            .video_end_after(clip)
            .map(|d| d.clamp(MIN_DURATION, max));
        let beat_lengths = Self::beat_lengths(clip, max);
        let snapping = self.prefs.snapping;
        let mut targets = beat_lengths.clone();
        targets.extend(fit);
        let span = (max - MIN_DURATION).max(1);
        let reach = (span as f64 * SNAP_SHARE) as Micros;
        let shown = match self.live_value {
            Some((k, v)) if k == KEY => (v * SECOND as f64) as Micros,
            _ => current,
        };
        let id = clip.id.clone();

        let slider = {
            let id = id.clone();
            let (lo, hi) = (secs(MIN_DURATION), secs(max));
            self.scaled_slider(
                KEY,
                "Duration",
                secs(current),
                move |v| ((v - lo) / (hi - lo).max(1e-9)) as f32,
                move |f| {
                    let d = MIN_DURATION + (f.clamp(0., 1.) as f64 * span as f64) as Micros;
                    let d = if snapping {
                        snap_to(d, &targets, reach).unwrap_or(d)
                    } else {
                        d
                    };
                    // 0.1 s steps, unless it caught a beat or the video's end.
                    if snapping && targets.contains(&d) {
                        secs(d)
                    } else {
                        (secs(d) * 10.).round() / 10.
                    }
                },
                |v| format!("{v:.1}s"),
                max <= MIN_DURATION,
                move |ed, v, cx| {
                    ed.set_audio_duration(&id, (v * SECOND as f64).round() as Micros, cx)
                },
                cx,
            )
        };

        let fps = self.fps();
        let info = |label: &str, value: String| {
            panel_row(label.to_string()).child(
                div()
                    .flex_1()
                    .flex()
                    .justify_end()
                    .font_family(theme::font_mono())
                    .text_size(px(theme::XS))
                    .text_color(theme::text_secondary())
                    .child(value),
            )
        };

        let button = |key: &'static str,
                      label: &'static str,
                      enabled: bool,
                      to: Option<Micros>,
                      cx: &mut Context<Self>| {
            let id = id.clone();
            pill_button(key, label, false)
                .flex_1()
                .px(px(8.))
                .when(!enabled, |b| b.opacity(0.4).cursor_default())
                .when_some(to.filter(|_| enabled), |b, d| {
                    b.on_click(cx.listener(move |ed, _, _, cx| ed.set_audio_duration(&id, d, cx)))
                })
        };
        let end_on_beat = snap_to(current, &beat_lengths, Micros::MAX);
        let presets = div()
            .mt(px(6.))
            .flex()
            .gap(px(6.))
            .child(button(
                "dur-fit",
                "Fit to video",
                fit.is_some_and(|d| d != current),
                fit,
                cx,
            ))
            .child(button(
                "dur-full",
                "Full song",
                max != current,
                Some(max),
                cx,
            ));
        let beat_row = div().mt(px(6.)).flex().child(button(
            "dur-beat",
            "End on a beat",
            // Already within a frame of one: nothing to do.
            end_on_beat.is_some_and(|d| (d - current).abs() >= self.frame_micros()),
            end_on_beat,
            cx,
        ));

        let note = if beat_lengths.is_empty() {
            "Mark the song's beats (Audio › Beats) to end the clip on one."
        } else if snapping {
            "The slider catches the beats and the end of the video while auto snapping is on."
        } else {
            "Turn on auto snapping (N) for the slider to catch the beats."
        };

        div()
            .px(px(14.))
            .pb(px(14.))
            .flex()
            .flex_col()
            .child(section_with("Duration", true, []))
            .child(slider)
            .child(info("Ends at", timecode(clip.timeline_start + shown, fps)))
            .child(info("Song left", format!("{:.1}s", secs(max))))
            .child(presets)
            .child(beat_row)
            .child(panel_note(note))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_the_nearest_target_in_reach() {
        let t = [2 * SECOND, 3 * SECOND];
        assert_eq!(snap_to(2_100_000, &t, 200_000), Some(2 * SECOND));
        assert_eq!(snap_to(2_600_000, &t, 200_000), None);
        assert_eq!(snap_to(2_600_000, &t, Micros::MAX), Some(3 * SECOND));
        assert_eq!(snap_to(SECOND, &[], Micros::MAX), None);
    }

    #[test]
    fn fades_shrink_with_a_shorter_clip() {
        assert_eq!(fitted_fades(SECOND, SECOND, 4 * SECOND), (SECOND, SECOND));
        assert_eq!(
            fitted_fades(3 * SECOND, SECOND, 2 * SECOND),
            (1_500_000, 500_000)
        );
        assert_eq!(fitted_fades(0, 0, SECOND), (0, 0));
    }
}
