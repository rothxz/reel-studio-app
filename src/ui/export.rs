//! Export (header button, ⌘E): asks where to save the video, then covers
//! the editor with the export screen. The progress runs round a frame
//! of the project's shape showing the frame just written; under it, Cancel
//! while it runs (asking first), then Done / Show in Finder / Open, or
//! Retry when it failed or was stopped.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, Context, FontWeight, MouseButton, ObjectFit, Pixels, Point, RenderImage,
    SharedString, Window, canvas, div, img, point, prelude::*, px,
};
use serde_json::json;

use crate::analytics;
use crate::catalog::EFFECT_PREFIX;
use crate::editor::{Confirm, Editor, HEADER_HEIGHT, Screen};
use crate::export::{Inputs, Job, Settings, Update};
use crate::prefs::RESOLUTIONS;
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets::{icon_button, pill_button};
use crate::ui::window_chrome::{self, drag_area};

/// Room for the macOS traffic lights (as the header).
const TRAFFIC_LIGHTS: f32 = 70.;
/// The frame's widest (px) and its shape's limits (width / height).
const FRAME_MAX: f32 = 300.;
const RING_WIDTH: f32 = 4.;
const RING_RADIUS: f32 = 18.;
/// The picture sits this far inside the ring.
const RING_INSET: f32 = 10.;

#[derive(Clone, Debug, PartialEq)]
pub enum ExportState {
    Running,
    Done(Duration),
    Failed(SharedString),
    Cancelled,
}

/// The export screen's state.
pub struct ExportView {
    /// Which export the updates belong to (a retry starts another).
    id: u64,
    job: Job,
    pub settings: Settings,
    /// The header's resolution label it was started at.
    pub resolution: &'static str,
    /// The `export_*` analytics properties, fixed at the start.
    metadata: serde_json::Value,
    pub state: ExportState,
    /// 0–1.
    pub progress: f32,
    /// The frame just written.
    pub preview: Option<Arc<RenderImage>>,
    started: Instant,
}

impl ExportView {
    fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub fn running(&self) -> bool {
        self.state == ExportState::Running
    }
}

/// A file name from a project name: no path separators or characters
/// file systems refuse.
fn file_name(project: &str) -> String {
    let safe: String = project
        .chars()
        .map(|c| {
            if "\\/:*?\"<>|".contains(c) || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect();
    let safe = safe.trim();
    format!("{}.mp4", if safe.is_empty() { "Reel Studio" } else { safe })
}

/// "Show in Finder" on a Mac, the file manager's word elsewhere.
fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Show in Finder"
    } else if cfg!(target_os = "windows") {
        "Show in Explorer"
    } else {
        "Show in folder"
    }
}

fn duration_label(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 60 {
        format!("{}:{:02}", s / 60, s % 60)
    } else {
        format!("{:.1} s", d.as_secs_f32().max(0.1))
    }
}

impl Editor {
    /// ⌘E: picks where the video goes, then starts writing it.
    pub fn export(&mut self, cx: &mut Context<Self>) {
        if self.screen != Screen::Editor || self.export.is_some() {
            return;
        }
        if self.project.duration <= 0 {
            self.toast("Add something to the timeline to export", cx);
            return;
        }
        self.pause();
        cx.notify();
        let home = crate::prefs::home_dir();
        let dir = self
            .prefs
            .export_dir
            .clone()
            .map(PathBuf::from)
            .filter(|d| d.is_dir())
            .or_else(|| Some(home.join("Movies")).filter(|d| d.is_dir()))
            .unwrap_or(home);
        let rx = cx.prompt_for_new_path(&dir, Some(&file_name(&self.project.name)));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut path))) = rx.await else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension("mp4");
            }
            this.update(cx, |ed, cx| {
                if let Some(parent) = path.parent() {
                    ed.prefs.export_dir = Some(parent.to_string_lossy().into_owned());
                    ed.prefs.save();
                }
                ed.start_export(path, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Writes the project to `path` at the header's resolution and rate.
    fn start_export(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let settings = Settings::new(
            &self.project,
            self.prefs.export_resolution,
            self.export_fps,
            path,
        );
        // What the editor has loaded already: catalog rows of the effects
        // used, packages, stickers.
        let effect_keys = self
            .project
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .flat_map(|c| c.effects())
            .filter(|e| e.kind.starts_with(EFFECT_PREFIX))
            .filter_map(|e| {
                let row = self.effects.get(e.name())?;
                (!row.package_key.is_empty())
                    .then(|| (e.name().to_string(), row.package_key.clone()))
            })
            .collect();
        let stickers = self
            .stickers
            .iter()
            .filter_map(|(url, f)| Some((url.clone(), f.clone()?)))
            .collect();
        let inputs = Inputs {
            project: self.project.clone(),
            effect_keys,
            packages: self.packages.clone(),
            stickers,
        };
        let resolution = RESOLUTIONS[self.prefs.export_resolution.min(RESOLUTIONS.len() - 1)].0;
        let metadata = analytics::export_props(&self.project, resolution, settings.fps);
        analytics::track("export_started", metadata.clone());
        let (tx, rx) = async_channel::unbounded();
        let job = Job::start(inputs, settings.clone(), tx);
        self.export_seq += 1;
        let id = self.export_seq;
        if let Some(old) = self.export.take().and_then(|v| v.preview) {
            self.stale_frames.push(old);
        }
        self.export = Some(ExportView {
            id,
            job,
            settings,
            resolution,
            metadata,
            state: ExportState::Running,
            progress: 0.,
            preview: None,
            started: Instant::now(),
        });
        cx.spawn(async move |this, cx| {
            while let Ok(update) = rx.recv().await {
                if this
                    .update(cx, |ed, cx| ed.on_export_update(id, update, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn on_export_update(&mut self, id: u64, update: Update, cx: &mut Context<Self>) {
        let Some(view) = self.export.as_mut().filter(|v| v.id == id) else {
            return; // closed, or a retry replaced it
        };
        match update {
            Update::Progress {
                done,
                total,
                preview,
            } => {
                view.progress = (done as f32 / total.max(1) as f32).min(1.);
                if let Some((w, h, bgra)) = preview
                    && let Some(buffer) = image::RgbaImage::from_raw(w, h, bgra)
                {
                    let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
                    if let Some(old) = view.preview.replace(image) {
                        self.stale_frames.push(old);
                    }
                }
            }
            Update::Finished => {
                view.progress = 1.;
                view.state = ExportState::Done(view.started.elapsed());
                let size_mb = std::fs::metadata(&view.settings.path)
                    .map(|m| (m.len() as f64 / 1_048_576. * 10.).round() / 10.)
                    .ok();
                analytics::track(
                    "export_completed",
                    analytics::with(
                        view.metadata.clone(),
                        json!({ "exportTimeMs": view.elapsed_ms(), "fileSizeMB": size_mb }),
                    ),
                );
            }
            Update::Failed(e) => {
                view.state = ExportState::Failed(e.into());
                // The message can hold a path: only that it failed.
                analytics::track(
                    "export_failed",
                    analytics::with(
                        view.metadata.clone(),
                        json!({ "exportTimeMs": view.elapsed_ms(), "errorCategory": "export" }),
                    ),
                );
            }
            Update::Cancelled => view.state = ExportState::Cancelled,
        }
        cx.notify();
    }

    /// ✕ / Esc: leaves the export screen, asking first while it runs.
    pub fn close_export(&mut self, cx: &mut Context<Self>) {
        let Some(view) = &self.export else { return };
        if view.running() {
            self.confirm = Some(Confirm {
                title: "Stop exporting?".into(),
                message: "Your video isn't finished yet. If you leave now, the export will be \
                          cancelled and you'll have to start over."
                    .into(),
                confirm_label: "Stop export".into(),
                run: std::rc::Rc::new(|ed: &mut Editor, _, cx| ed.stop_export(cx)),
            });
            cx.notify();
            return;
        }
        self.dismiss_export(cx);
    }

    /// "Stop export": cancels it if it's still running (it may have
    /// finished while the question was up: then the video stays).
    fn stop_export(&mut self, cx: &mut Context<Self>) {
        let running = self.export.as_ref().is_some_and(|v| v.running());
        if let Some(view) = &self.export
            && running
        {
            view.job.cancel();
            analytics::track(
                "export_cancelled",
                analytics::with(
                    view.metadata.clone(),
                    json!({ "exportTimeMs": view.elapsed_ms() }),
                ),
            );
        }
        self.dismiss_export(cx);
        if running {
            self.toast("Export cancelled", cx);
        }
    }

    fn dismiss_export(&mut self, cx: &mut Context<Self>) {
        if let Some(old) = self.export.take().and_then(|v| v.preview) {
            self.stale_frames.push(old);
        }
        cx.notify();
    }

    fn retry_export(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.export.as_ref().map(|v| v.settings.path.clone()) else {
            return;
        };
        self.start_export(path, cx);
    }

    /// The export screen, over everything.
    pub(crate) fn render_export(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = &self.export else {
            return div().into_any_element();
        };
        let spec = format!(
            "{} · {}×{} · {} FPS",
            view.resolution, view.settings.width, view.settings.height, view.settings.fps
        );
        let folder = view
            .settings
            .path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let name = view
            .settings
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        let (title, subtitle): (String, String) = match &view.state {
            ExportState::Running => (
                format!("{:.1}%", view.progress * 100.),
                "Keep this window open until your video is ready.".into(),
            ),
            ExportState::Done(took) => (
                "Your video is ready".into(),
                format!("Saved {name} to {folder} in {}", duration_label(*took)),
            ),
            ExportState::Failed(e) => ("Export failed".into(), e.to_string()),
            ExportState::Cancelled => ("Export cancelled".into(), String::new()),
        };

        // The frame takes the project's shape (within limits).
        let aspect = (self.project.canvas.width.max(1) as f32
            / self.project.canvas.height.max(1) as f32)
            .clamp(0.45, 1.2);
        let viewport = window.viewport_size();
        let room_h = (f32::from(viewport.height) - 360.).max(160.);
        let mut ring_w = (f32::from(viewport.width) * 0.72).min(FRAME_MAX);
        if ring_w / aspect > room_h {
            ring_w = room_h * aspect;
        }
        let ring_h = ring_w / aspect;
        let progress = view.progress;

        let ring = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let track = ring_path(bounds, 1.);
                if let Some(p) = track {
                    window.paint_path(p, gpui::rgba(0xffffff1f));
                }
                if progress > 0.
                    && let Some(p) = ring_path(bounds, progress)
                {
                    window.paint_path(p, theme::text());
                }
            },
        )
        .absolute()
        .inset_0();

        let inner = div()
            .absolute()
            .inset(px(RING_INSET))
            .rounded(px(RING_RADIUS - RING_INSET / 2.))
            .bg(gpui::rgb(0x050506))
            .overflow_hidden()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .map(|d| match &view.preview {
                Some(image) => d.child(
                    img(image.clone())
                        .absolute()
                        .inset_0()
                        .size_full()
                        .object_fit(ObjectFit::Contain),
                ),
                None => d
                    .px_3()
                    .child(Icon::Film.el(40., theme::text_tertiary()))
                    .child(
                        div()
                            .text_size(px(theme::SM))
                            .font_family(theme::font_mono())
                            .text_color(theme::text_tertiary())
                            .text_center()
                            .child(self.project.name.clone()),
                    ),
            });

        let buttons = match &view.state {
            ExportState::Running => vec![
                pill_button("export-cancel", "Cancel export", false)
                    .h(px(40.))
                    .w_full()
                    .on_click(cx.listener(|ed, _, _, cx| ed.close_export(cx)))
                    .into_any_element(),
            ],
            ExportState::Done(_) => {
                let reveal = view.settings.path.clone();
                let open = view.settings.path.clone();
                vec![
                    pill_button("export-done", "Done", true)
                        .h(px(42.))
                        .w_full()
                        .text_size(px(theme::MD))
                        .on_click(cx.listener(|ed, _, _, cx| ed.dismiss_export(cx)))
                        .into_any_element(),
                    div()
                        .flex()
                        .gap_2()
                        .w_full()
                        .child(
                            pill_button("export-reveal", reveal_label(), false)
                                .h(px(38.))
                                .flex_1()
                                .on_click(move |_, _, cx| cx.reveal_path(&reveal)),
                        )
                        .child(
                            pill_button("export-open", "Play video", false)
                                .h(px(38.))
                                .flex_1()
                                .on_click(move |_, _, cx| cx.open_with_system(&open)),
                        )
                        .into_any_element(),
                ]
            }
            ExportState::Failed(_) | ExportState::Cancelled => vec![
                pill_button("export-retry", "Retry export", true)
                    .h(px(42.))
                    .w_full()
                    .text_size(px(theme::MD))
                    .on_click(cx.listener(|ed, _, _, cx| ed.retry_export(cx)))
                    .into_any_element(),
                pill_button("export-close", "Close", false)
                    .h(px(38.))
                    .w_full()
                    .on_click(cx.listener(|ed, _, _, cx| ed.dismiss_export(cx)))
                    .into_any_element(),
            ],
        };

        let full_screen = window.is_fullscreen();
        let controls = window_chrome::draws_controls(window);
        div()
            .id("export-screen")
            .absolute()
            .inset_0()
            .occlude()
            .bg(theme::app_bg())
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                // A title bar of its own: the window still moves and closes.
                div()
                    .flex_none()
                    .h(px(HEADER_HEIGHT))
                    .flex()
                    .items_center()
                    .pl(px(if cfg!(target_os = "macos") && !full_screen {
                        TRAFFIC_LIGHTS
                    } else {
                        8.
                    }))
                    .child(
                        icon_button(
                            "export-close-x",
                            Icon::Close,
                            if view.running() {
                                "Cancel export"
                            } else {
                                "Close"
                            },
                        )
                        .size(20.)
                        .on_click(cx.listener(|ed, _, _, cx| ed.close_export(cx))),
                    )
                    .child(drag_area("export-drag"))
                    .when(controls, |d| {
                        d.child(self.render_window_controls(window, cx))
                    }),
            )
            .child(
                div()
                    .id("export-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .items_center()
                    .pt(px(24.))
                    .pb(px(24.))
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(28.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::text())
                            .child(crate::i18n::ts(title)),
                    )
                    .child(
                        div()
                            .min_h(px(40.))
                            .max_w(px(520.))
                            .px_4()
                            .text_size(px(theme::MD))
                            .text_color(theme::text_secondary())
                            .text_center()
                            .child(subtitle),
                    )
                    .child(
                        div()
                            .text_size(px(theme::XS))
                            .font_family(theme::font_mono())
                            .text_color(theme::text_tertiary())
                            .child(spec),
                    )
                    .child(
                        div()
                            .mt_4()
                            .flex_none()
                            .relative()
                            .w(px(ring_w))
                            .h(px(ring_h))
                            .child(ring)
                            .child(inner),
                    )
                    .child(
                        div()
                            .mt_6()
                            .w(px(ring_w.max(260.)))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .children(buttons),
                    ),
            )
            .into_any_element()
    }
}

/// The first `fraction` of a rounded rectangle's outline inside `bounds`
/// (inset by half the ring's width), clockwise from the top centre, as a
/// stroke.
fn ring_path(bounds: Bounds<Pixels>, fraction: f32) -> Option<gpui::Path<Pixels>> {
    let half = RING_WIDTH / 2.;
    let (x0, y0) = (
        f32::from(bounds.origin.x) + half,
        f32::from(bounds.origin.y) + half,
    );
    let (w, h) = (
        f32::from(bounds.size.width) - RING_WIDTH,
        f32::from(bounds.size.height) - RING_WIDTH,
    );
    let points = outline(x0, y0, w, h, RING_RADIUS.min(w / 2.).min(h / 2.));
    let mut lengths = vec![0f32];
    for pair in points.windows(2) {
        let d = ((pair[1].0 - pair[0].0).powi(2) + (pair[1].1 - pair[0].1).powi(2)).sqrt();
        lengths.push(lengths.last().unwrap() + d);
    }
    let goal = lengths.last().copied()? * fraction.clamp(0., 1.);
    let mut builder = gpui::PathBuilder::stroke(px(RING_WIDTH));
    let at = |p: (f32, f32)| -> Point<Pixels> { point(px(p.0), px(p.1)) };
    builder.move_to(at(points[0]));
    for i in 1..points.len() {
        if lengths[i] <= goal {
            builder.line_to(at(points[i]));
            continue;
        }
        let seg = lengths[i] - lengths[i - 1];
        let k = if seg > 0. {
            (goal - lengths[i - 1]) / seg
        } else {
            0.
        };
        let (a, b) = (points[i - 1], points[i]);
        builder.line_to(at((a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k)));
        break;
    }
    builder.build().ok()
}

/// A rounded rectangle's outline as points, clockwise from the top centre
/// back to it.
fn outline(x: f32, y: f32, w: f32, h: f32, r: f32) -> Vec<(f32, f32)> {
    const STEPS: usize = 10;
    let mut pts = vec![(x + w / 2., y)];
    // Corner centres and the angle each quarter turn starts at (y down).
    let corners = [
        (x + w - r, y + r, -90f32),
        (x + w - r, y + h - r, 0.),
        (x + r, y + h - r, 90.),
        (x + r, y + r, 180.),
    ];
    for (cx, cy, from) in corners {
        for s in 0..=STEPS {
            let a = (from + 90. * s as f32 / STEPS as f32).to_radians();
            pts.push((cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    pts.push((x + w / 2., y));
    pts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("My trip: day 1/2"), "My trip  day 1 2.mp4");
        assert_eq!(file_name("  "), "Reel Studio.mp4");
    }

    #[test]
    fn outline_starts_and_ends_at_the_top_centre() {
        let pts = outline(0., 0., 100., 200., 10.);
        assert_eq!(pts.first(), Some(&(50., 0.)));
        assert_eq!(pts.last(), Some(&(50., 0.)));
        // Clockwise: the right side comes first.
        assert!(pts[5].0 > 50.);
    }
}
