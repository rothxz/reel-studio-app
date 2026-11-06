//! The desktop editor window: media library | player | inspector over a
//! timeline, the same four-panel layout as the web/Electron desktop editor.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, ClickEvent, Context, ElementId, ExternalPaths, FocusHandle,
    MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, PathPromptOptions, Pixels, RenderImage,
    ScrollWheelEvent, SharedString, Window, actions, canvas, div, img, prelude::*, px,
};

use crate::decoder::{self, DecodedFrame, FrameServer};
use crate::engine::{self, Engine};
use crate::model::{MediaItem, Micros, Project, SECOND, TrackType};
use crate::theme;

actions!(
    editor,
    [
        TogglePlay,
        SplitClip,
        DeleteClip,
        StepBack,
        StepForward,
        GoToStart,
        ZoomIn,
        ZoomOut,
        ImportMedia,
        OpenProject,
        SaveProject,
        Quit
    ]
);

/// Preview frames are decoded at most this wide (the player is ~800 px).
const PREVIEW_MAX_WIDTH: u32 = 1280;
const TRACK_HEIGHT: f32 = 56.;
const RULER_HEIGHT: f32 = 26.;
const LANE_LABEL_WIDTH: f32 = 84.;

pub struct Editor {
    project: Project,
    engine: Engine,
    media: Vec<MediaItem>,
    selected: Option<String>,
    playhead: Micros,
    /// Timeline zoom, pixels per second.
    pps: f32,
    scroll_x: f32,
    scrubbing: bool,
    timeline_bounds: Rc<Cell<Bounds<Pixels>>>,

    servers: HashMap<String, FrameServer>,
    frame_tx: async_channel::Sender<DecodedFrame>,
    wanted: Option<(String, Micros)>,
    frame: Option<Arc<RenderImage>>,
    stale_frames: Vec<Arc<RenderImage>>,
    frame_info: Option<(u32, u32, Micros)>,
    shown_count: u32,
    fps_window: (Instant, u32),
    preview_fps: f32,

    status: SharedString,
    focus: FocusHandle,
}

impl Editor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (frame_tx, frame_rx) = async_channel::bounded::<DecodedFrame>(2);
        cx.spawn(async move |this, cx| {
            while let Ok(frame) = frame_rx.recv().await {
                if this.update(cx, |ed, cx| ed.on_frame(frame, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        let focus = cx.focus_handle();
        window.focus(&focus);
        let project = Project::empty();
        let mut engine = Engine::new();
        let status = match engine.load(&project) {
            Ok(()) => format!("Engine: {}", engine::BACKEND),
            Err(e) => format!("Engine refused the project: {e}"),
        };
        Editor {
            project,
            engine,
            media: vec![],
            selected: None,
            playhead: 0,
            pps: 80.,
            scroll_x: 0.,
            scrubbing: false,
            timeline_bounds: Rc::new(Cell::new(Bounds::default())),
            servers: HashMap::new(),
            frame_tx,
            wanted: None,
            frame: None,
            stale_frames: vec![],
            frame_info: None,
            shown_count: 0,
            fps_window: (Instant::now(), 0),
            preview_fps: 0.,
            status: status.into(),
            focus,
        }
    }

    // ---- project edits ---------------------------------------------------

    /// Every edit goes through here: the engine gets the new document (as
    /// the phone app's store → VideoEngine.updateProject does).
    fn commit(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.engine.load(&self.project) {
            self.status = format!("Engine refused the project: {e}").into();
        }
        self.playhead = self.playhead.min(self.project.duration);
        self.engine.seek(self.playhead);
        self.request_preview();
        cx.notify();
    }

    pub fn open_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        for path in paths {
            if path.ends_with(".json") {
                self.load_project_file(&path, cx);
                continue;
            }
            match decoder::probe(&path) {
                Ok(item) => {
                    let item_clone = item.clone();
                    self.media.push(item);
                    // An empty timeline takes the first import straight away.
                    if self.project.duration == 0 {
                        self.add_to_timeline(&item_clone, cx);
                    }
                }
                Err(e) => self.status = format!("Can't open {path}: {e}").into(),
            }
        }
        cx.notify();
    }

    fn add_to_timeline(&mut self, item: &MediaItem, cx: &mut Context<Self>) {
        let id = self.project.append_media(item);
        self.selected = Some(id);
        self.commit(cx);
    }

    fn load_project_file(&mut self, path: &str, cx: &mut Context<Self>) {
        let parsed = std::fs::read_to_string(path)
            .map_err(anyhow::Error::from)
            .and_then(|s| Ok(serde_json::from_str::<Project>(&s)?));
        match parsed {
            Ok(project) => {
                self.project = project;
                self.selected = None;
                self.playhead = 0;
                self.status = format!("Opened {}", self.project.name).into();
                self.commit(cx);
            }
            Err(e) => self.status = format!("Not a project file: {e}").into(),
        }
    }

    // ---- actions ---------------------------------------------------------

    fn toggle_play(&mut self, _: &TogglePlay, _: &mut Window, cx: &mut Context<Self>) {
        if self.engine.is_playing() {
            self.engine.pause();
            self.playhead = self.engine.tick();
        } else {
            if self.playhead >= self.project.duration {
                self.playhead = 0;
                self.engine.seek(0);
            }
            // render() keeps ticking while the engine plays.
            self.engine.play();
        }
        cx.notify();
    }

    fn split(&mut self, _: &SplitClip, _: &mut Window, cx: &mut Context<Self>) {
        // The selected clip, else the main-track clip under the playhead.
        let target = self.selected.clone().or_else(|| {
            self.project.tracks.first().and_then(|t| {
                t.clips
                    .iter()
                    .find(|c| self.playhead > c.timeline_start && self.playhead < c.end())
                    .map(|c| c.id.clone())
            })
        });
        if let Some(id) = target
            && self.project.split_at(&id, self.playhead)
        {
            self.commit(cx);
        }
    }

    fn delete(&mut self, _: &DeleteClip, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.selected.take() {
            self.project.delete_clip(&id);
            self.commit(cx);
        }
    }

    fn step(&mut self, frames: i64, cx: &mut Context<Self>) {
        let frame = (SECOND as f64 / self.project.canvas.fps) as Micros;
        self.seek(self.playhead + frames * frame, cx);
    }

    fn import(&mut self, _: &ImportMedia, _: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Import".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let paths = paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                this.update(cx, |ed, cx| ed.open_paths(paths, cx)).ok();
            }
        })
        .detach();
    }

    fn open_project(&mut self, _: &OpenProject, _: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open project JSON".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.first()
            {
                let path = path.to_string_lossy().into_owned();
                this.update(cx, |ed, cx| ed.load_project_file(&path, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn save_project(&mut self, _: &SaveProject, _: &mut Window, cx: &mut Context<Self>) {
        let dir = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
        let rx = cx.prompt_for_new_path(&dir, Some("project.json"));
        let json = serde_json::to_string_pretty(&self.project).unwrap_or_default();
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let msg = match std::fs::write(&path, json) {
                    Ok(()) => format!("Saved {}", path.display()),
                    Err(e) => format!("Save failed: {e}"),
                };
                this.update(cx, |ed, cx| {
                    ed.status = msg.into();
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    // ---- playback + preview ---------------------------------------------

    fn seek(&mut self, t: Micros, cx: &mut Context<Self>) {
        self.playhead = t.clamp(0, self.project.duration);
        self.engine.seek(self.playhead);
        self.request_preview();
        cx.notify();
    }

    /// Asks the decoder of the top visible clip for the frame at the playhead.
    fn request_preview(&mut self) {
        let top = self
            .engine
            .active_layers(self.playhead)
            .into_iter()
            .rfind(|l| l.is_visual && !l.source.is_empty());
        let Some(layer) = top else {
            self.wanted = None;
            if let Some(old) = self.frame.take() {
                self.stale_frames.push(old);
            }
            self.frame_info = None;
            return;
        };
        if self.wanted.as_ref() == Some(&(layer.source.clone(), layer.source_time)) {
            return;
        }
        let server = self.servers.entry(layer.source.clone()).or_insert_with(|| {
            FrameServer::spawn(
                layer.source.clone(),
                PREVIEW_MAX_WIDTH,
                self.frame_tx.clone(),
            )
        });
        server.request(layer.source_time);
        self.wanted = Some((layer.source, layer.source_time));
    }

    fn on_frame(&mut self, f: DecodedFrame, cx: &mut Context<Self>) {
        if self.wanted.as_ref().is_none_or(|(src, _)| *src != f.source) {
            return; // a cut moved on to another file meanwhile
        }
        let Some(buffer) = image::RgbaImage::from_raw(f.width, f.height, f.bgra) else {
            return;
        };
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
        if let Some(old) = self.frame.replace(image) {
            self.stale_frames.push(old);
        }
        self.frame_info = Some((f.width, f.height, f.pts));
        self.shown_count += 1;
        let (since, n) = &mut self.fps_window;
        *n += 1;
        let elapsed = since.elapsed().as_secs_f32();
        if elapsed >= 1. {
            self.preview_fps = *n as f32 / elapsed;
            self.fps_window = (Instant::now(), 0);
        }
        cx.notify();
    }

    // ---- timeline geometry ----------------------------------------------

    fn x_of(&self, t: Micros) -> f32 {
        t as f32 / SECOND as f32 * self.pps - self.scroll_x
    }

    fn time_at(&self, window_x: Pixels) -> Micros {
        let b = self.timeline_bounds.get();
        let x = f32::from(window_x - b.origin.x) - LANE_LABEL_WIDTH + self.scroll_x;
        ((x / self.pps) * SECOND as f32) as Micros
    }

    fn scrub_to(&mut self, window_x: Pixels, cx: &mut Context<Self>) {
        if self.engine.is_playing() {
            self.engine.pause();
        }
        let t = self.time_at(window_x);
        self.seek(t, cx);
    }

    fn zoom(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.pps = (self.pps * factor).clamp(4., 1200.);
        cx.notify();
    }
}

// ---- small UI parts ------------------------------------------------------

fn timecode(t: Micros, fps: f64) -> String {
    // Whole frames first, so 0.1 s at 30 fps reads frame 03, not 02.
    let fps_i = fps.round().max(1.) as i64;
    let frames = (t.max(0) as f64 * fps / SECOND as f64).round() as i64;
    let (secs, f) = (frames / fps_i, frames % fps_i);
    format!("{:02}:{:02}:{:02}", secs / 60, secs % 60, f)
}

fn seconds(t: Micros) -> String {
    format!("{:.2}s", t as f64 / SECOND as f64)
}

fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(theme::border())
        .text_xs()
        .text_color(theme::text())
        .cursor_pointer()
        .hover(|s| s.bg(theme::hover()))
        .on_click(on_click)
        .child(label.into())
}

fn section_title(label: &str) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(theme::text_dim())
        .child(label.to_uppercase())
}

fn row(label: &str, value: String) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .text_xs()
        .child(div().text_color(theme::text_dim()).child(label.to_string()))
        .child(div().text_color(theme::text()).child(value))
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for old in self.stale_frames.drain(..) {
            let _ = window.drop_image(old);
        }
        if self.engine.is_playing() {
            self.playhead = self.engine.tick();

            self.request_preview();
            window.request_animation_frame();
        }

        div()
            .key_context("Editor")
            .track_focus(&self.focus)
            // Clicks anywhere keep the shortcuts (Space, S, ⌫…) live.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|ed, _, window, _| window.focus(&ed.focus)),
            )
            .on_action(cx.listener(Self::toggle_play))
            .on_action(cx.listener(Self::split))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(|ed, _: &StepBack, _, cx| ed.step(-1, cx)))
            .on_action(cx.listener(|ed, _: &StepForward, _, cx| ed.step(1, cx)))
            .on_action(cx.listener(|ed, _: &GoToStart, _, cx| ed.seek(0, cx)))
            .on_action(cx.listener(|ed, _: &ZoomIn, _, cx| ed.zoom(1.25, cx)))
            .on_action(cx.listener(|ed, _: &ZoomOut, _, cx| ed.zoom(0.8, cx)))
            .on_action(cx.listener(Self::import))
            .on_action(cx.listener(Self::open_project))
            .on_action(cx.listener(Self::save_project))
            .on_drop(cx.listener(|ed, paths: &ExternalPaths, _, cx| {
                let paths = paths
                    .paths()
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                ed.open_paths(paths, cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::bg())
            .text_color(theme::text())
            .text_sm()
            .child(self.render_header(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_library(cx))
                    .child(self.render_player(cx))
                    .child(self.render_inspector(cx)),
            )
            .child(self.render_timeline(cx))
    }
}

impl Editor {
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(40.))
            .flex()
            .items_center()
            .gap_2()
            .pl(px(84.)) // traffic lights
            .pr_3()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::panel())
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(self.project.name.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_dim())
                    .child(self.status.clone()),
            )
            .child(div().flex_1())
            .child(button(
                "open",
                "Open project",
                cx.listener(|ed, _, w, cx| ed.open_project(&OpenProject, w, cx)),
            ))
            .child(button(
                "save",
                "Save project",
                cx.listener(|ed, _, w, cx| ed.save_project(&SaveProject, w, cx)),
            ))
    }

    fn render_library(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let items: Vec<AnyElement> = self
            .media
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let item_for_click = item.clone();
                div()
                    .id(("media", i))
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .p_2()
                    .rounded_md()
                    .bg(theme::panel_alt())
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::hover()))
                    .on_click(
                        cx.listener(move |ed, _, _, cx| ed.add_to_timeline(&item_for_click, cx)),
                    )
                    .child(div().text_xs().truncate().child(item.name.clone()))
                    .child(div().text_xs().text_color(theme::text_dim()).child(format!(
                        "{} · {}×{}",
                        seconds(item.duration),
                        item.width,
                        item.height
                    )))
                    .into_any_element()
            })
            .collect();

        div()
            .id("library")
            .w(px(260.))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_r_1()
            .border_color(theme::border())
            .bg(theme::panel())
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(section_title("Media"))
                    .child(button(
                        "import",
                        "Import",
                        cx.listener(|ed, _, w, cx| ed.import(&ImportMedia, w, cx)),
                    )),
            )
            .when(items.is_empty(), |d| {
                d.child(div().mt_4().text_xs().text_color(theme::text_dim()).child(
                    "Drop videos here or press ⌘I. Click an item to add it to the timeline.",
                ))
            })
            .children(items)
    }

    fn render_player(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let fps = self.project.canvas.fps;
        let playing = self.engine.is_playing();
        let picture: AnyElement = match &self.frame {
            // Absolute, so the picture's own size never drives the layout.
            Some(frame) => img(frame.clone())
                .absolute()
                .inset_0()
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme::text_dim())
                .child(if self.project.duration == 0 {
                    "Import a video to start"
                } else {
                    ""
                })
                .into_any_element(),
        };
        let info = match self.frame_info {
            Some((w, h, pts)) => format!(
                "{w}×{h} · src {} · {:.0} fps decoded",
                seconds(pts),
                self.preview_fps
            ),
            None => String::new(),
        };

        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div().flex_1().min_h_0().p_3().child(
                    div()
                        .relative()
                        .size_full()
                        .overflow_hidden()
                        .bg(gpui::black())
                        .child(picture),
                ),
            )
            .child(
                div()
                    .h(px(40.))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(div().font_family("Menlo").text_xs().child(format!(
                        "{} / {}",
                        timecode(self.playhead, fps),
                        timecode(self.project.duration, fps)
                    )))
                    .child(div().flex_1())
                    .child(button(
                        "play",
                        if playing { "Pause" } else { "Play" },
                        cx.listener(|ed, _, w, cx| ed.toggle_play(&TogglePlay, w, cx)),
                    ))
                    .child(div().flex_1())
                    .child(div().text_xs().text_color(theme::text_dim()).child(info)),
            )
    }

    fn render_inspector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div().flex().flex_col().gap_2();
        match self
            .selected
            .as_deref()
            .and_then(|id| self.project.find_clip(id))
        {
            Some((track, clip)) => {
                let id = clip.id.clone();
                body = body
                    .child(section_title("Clip"))
                    .child(div().text_sm().truncate().child(clip.label()))
                    .child(row("Lane", track.name.clone()))
                    .child(row("Kind", clip.kind.clone()))
                    .child(row("Start", seconds(clip.timeline_start)))
                    .child(row("Duration", seconds(clip.timeline_duration)))
                    .child(row("Source in", seconds(clip.source_start.unwrap_or(0))))
                    .child(row("Speed", format!("{}×", clip.speed)))
                    .child(div().h(px(8.)))
                    .child(section_title("Speed"))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .children([0.5, 1.0, 2.0, 4.0].into_iter().map(|s| {
                                let id = id.clone();
                                button(
                                    SharedString::from(format!("speed-{s}")),
                                    format!("{s}×"),
                                    cx.listener(move |ed, _, _, cx| {
                                        ed.project.set_speed(&id, s);
                                        ed.commit(cx);
                                    }),
                                )
                            })),
                    )
                    .child(div().h(px(8.)))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(button(
                                "split",
                                "Split (S)",
                                cx.listener(|ed, _, w, cx| ed.split(&SplitClip, w, cx)),
                            ))
                            .child(button(
                                "delete",
                                "Delete (⌫)",
                                cx.listener(|ed, _, w, cx| ed.delete(&DeleteClip, w, cx)),
                            )),
                    );
            }
            None => {
                let p = &self.project;
                let clips: usize = p.tracks.iter().map(|t| t.clips.len()).sum();
                body =
                    body.child(section_title("Project"))
                        .child(row(
                            "Canvas",
                            format!("{}×{}", p.canvas.width, p.canvas.height),
                        ))
                        .child(row("Frame rate", format!("{} fps", p.canvas.fps)))
                        .child(row("Duration", seconds(p.duration)))
                        .child(row("Lanes", p.tracks.len().to_string()))
                        .child(row("Clips", clips.to_string()))
                        .child(row("Engine", engine::BACKEND.to_string()))
                        .child(div().h(px(8.)))
                        .child(
                            div().text_xs().text_color(theme::text_dim()).child(
                                "Space play · S split · ⌫ delete · ←/→ frame · ⌘= / ⌘- zoom",
                            ),
                        );
            }
        }
        div()
            .w(px(280.))
            .p_3()
            .border_l_1()
            .border_color(theme::border())
            .bg(theme::panel())
            .child(body)
    }

    fn render_timeline(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds_cell = self.timeline_bounds.clone();
        let fps = self.project.canvas.fps;

        // Ruler ticks: pick a step that keeps labels ~80 px apart.
        let steps = [0.5, 1., 2., 5., 10., 15., 30., 60., 120., 300.];
        let step = steps
            .iter()
            .copied()
            .find(|s| s * self.pps as f64 >= 80.)
            .unwrap_or(600.);
        let visible_secs = 4000. / self.pps as f64;
        let first = (self.scroll_x as f64 / self.pps as f64 / step).floor() * step;
        let mut ticks = vec![];
        let mut s = first;
        while s < first + visible_secs {
            let x = self.x_of((s * SECOND as f64) as Micros);
            ticks.push(
                div()
                    .absolute()
                    .left(px(x))
                    .top_0()
                    .h_full()
                    .border_l_1()
                    .border_color(theme::border())
                    .pl_1()
                    .text_xs()
                    .text_color(theme::text_dim())
                    .child(timecode((s * SECOND as f64) as Micros, fps)),
            );
            s += step;
        }

        let mut lanes = vec![];
        let mut labels = vec![];
        let mut tracks: Vec<_> = self.project.tracks.iter().collect();
        tracks.sort_by_key(|t| (t.kind != TrackType::Video || t.order != 0, -t.order));
        for (ti, track) in tracks.iter().enumerate() {
            labels.push(
                div()
                    .h(px(TRACK_HEIGHT))
                    .flex()
                    .items_center()
                    .px_2()
                    .text_xs()
                    .text_color(theme::text_dim())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(track.name.clone()),
            );
            let clips = track.clips.iter().map(|clip| {
                let selected = self.selected.as_deref() == Some(clip.id.as_str());
                let color = match clip.kind.as_str() {
                    "video" | "image" => theme::clip_video(),
                    "audio" => theme::clip_audio(),
                    _ => theme::clip_other(),
                };
                let x = self.x_of(clip.timeline_start);
                let w = clip.timeline_duration as f32 / SECOND as f32 * self.pps;
                let id = clip.id.clone();
                div()
                    .id(SharedString::from(format!("clip-{}", clip.id)))
                    .absolute()
                    .left(px(x))
                    .top(px(4.))
                    .w(px((w - 1.).max(2.)))
                    .h(px(TRACK_HEIGHT - 8.))
                    .rounded_md()
                    .bg(color)
                    .border_2()
                    .border_color(if selected { theme::selected() } else { color })
                    .overflow_hidden()
                    .px_1()
                    .text_xs()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            ed.selected = Some(id.clone());
                            ed.scrub_to(e.position.x, cx);
                        }),
                    )
                    .child(div().truncate().child(clip.label()))
                    .when(clip.speed != 1., |d| {
                        d.child(
                            div()
                                .text_color(theme::text_dim())
                                .child(format!("{}×", clip.speed)),
                        )
                    })
            });
            lanes.push(
                div()
                    .id(("lane", ti))
                    .relative()
                    .h(px(TRACK_HEIGHT))
                    .border_b_1()
                    .border_color(theme::border())
                    .children(clips),
            );
        }

        let playhead_x = self.x_of(self.playhead) + LANE_LABEL_WIDTH;

        div()
            .id("timeline")
            .h(px(RULER_HEIGHT + TRACK_HEIGHT * tracks.len() as f32 + 46.))
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::panel())
            .child(
                div()
                    .h(px(36.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(button(
                        "tl-split",
                        "Split",
                        cx.listener(|ed, _, w, cx| ed.split(&SplitClip, w, cx)),
                    ))
                    .child(button(
                        "tl-delete",
                        "Delete",
                        cx.listener(|ed, _, w, cx| ed.delete(&DeleteClip, w, cx)),
                    ))
                    .child(div().flex_1())
                    .child(button(
                        "zoom-out",
                        "−",
                        cx.listener(|ed, _, _, cx| ed.zoom(0.8, cx)),
                    ))
                    .child(button(
                        "zoom-in",
                        "+",
                        cx.listener(|ed, _, _, cx| ed.zoom(1.25, cx)),
                    )),
            )
            .child(
                div()
                    .id("timeline-body")
                    .relative()
                    .flex_1()
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|ed, e: &MouseDownEvent, _, cx| {
                            ed.selected = None;
                            ed.scrubbing = true;
                            ed.scrub_to(e.position.x, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|ed, e: &MouseMoveEvent, _, cx| {
                        if ed.scrubbing && e.pressed_button == Some(MouseButton::Left) {
                            ed.scrub_to(e.position.x, cx);
                        } else {
                            ed.scrubbing = false;
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|ed, _, _, _| ed.scrubbing = false),
                    )
                    .on_scroll_wheel(cx.listener(|ed, e: &ScrollWheelEvent, _, cx| {
                        let d = e.delta.pixel_delta(px(20.));
                        if e.modifiers.platform || e.modifiers.control {
                            ed.zoom(if f32::from(d.y) > 0. { 1.1 } else { 0.9 }, cx);
                        } else {
                            let dx = if f32::from(d.x).abs() > f32::from(d.y).abs() {
                                d.x
                            } else {
                                d.y
                            };
                            ed.scroll_x = (ed.scroll_x - f32::from(dx)).max(0.);
                            cx.notify();
                        }
                    }))
                    // Records where the timeline sits, to map mouse x → time.
                    .child(
                        canvas(move |bounds, _, _| bounds_cell.set(bounds), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .child(
                        div()
                            .flex()
                            .child(
                                div()
                                    .w(px(LANE_LABEL_WIDTH))
                                    .flex()
                                    .flex_col()
                                    .border_r_1()
                                    .border_color(theme::border())
                                    .child(
                                        div()
                                            .h(px(RULER_HEIGHT))
                                            .border_b_1()
                                            .border_color(theme::border()),
                                    )
                                    .children(labels),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .relative()
                                    .overflow_hidden()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .relative()
                                            .h(px(RULER_HEIGHT))
                                            .border_b_1()
                                            .border_color(theme::border())
                                            .children(ticks),
                                    )
                                    .children(lanes),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(px(playhead_x))
                            .w(px(2.))
                            .bg(theme::playhead()),
                    ),
            )
    }
}
