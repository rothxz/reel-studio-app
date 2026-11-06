//! The desktop editor window: header, then browser | player | inspector over
//! the timeline toolbar + timeline, with draggable gutters between panels.
//! This file holds the editor's state, its edits and the window layout;
//! each panel renders from `src/ui/*`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, Context, CursorStyle, ExternalPaths, FocusHandle, FontWeight, MouseButton,
    MouseDownEvent, MouseMoveEvent, PathPromptOptions, Pixels, Point, RenderImage, SharedString,
    Size, Window, actions, canvas, div, prelude::*, px,
};

use crate::decoder::{self, DecodedFrame, FrameServer, TileServer, Waveform};
use crate::engine::Engine;
use crate::history::History;
use crate::model::{self, Clip, MediaItem, Micros, Project, SECOND, Side, TrackType};
use crate::prefs::Prefs;
use crate::projects;
use crate::selection::Selection;
use crate::shortcuts::{self, Shortcut};
use crate::theme;
use crate::ui::timeline_edit::{ClipDrag, Marquee, MediaDrop};

actions!(
    editor,
    [ImportMedia, OpenProject, SaveProject, SaveProjectAs, Quit]
);

/// Preview frames are decoded at most this wide (the player is ~800 px).
const PREVIEW_MAX_WIDTH: u32 = 1280;
/// Filmstrip tiles are decoded this wide (a 44 px tile at 2×, some slack).
const TILE_MAX_WIDTH: u32 = 128;

// Layout (old editor's desktop layout).
pub const HEADER_HEIGHT: f32 = 40.;
pub const GUTTER: f32 = 6.;
const BROWSER_MIN: f32 = 260.;
const BROWSER_MAX: f32 = 640.;
const BROWSER_DEFAULT_MIN: f32 = 300.;
const BROWSER_DEFAULT_MAX: f32 = 440.;
const INSPECTOR_MIN: f32 = 260.;
const INSPECTOR_MAX: f32 = 560.;
const INSPECTOR_DEFAULT: f32 = 340.;
const TIMELINE_MIN: f32 = 180.;
const TIMELINE_SHARE: f32 = 0.4;
const MIN_PLAYER_WIDTH: f32 = 320.;
const MIN_TOP_HEIGHT: f32 = 240.;

/// Timeline zoom bounds (px per second) and the ⌘= / ⌘- step.
pub const MIN_PPS: f32 = 4.;
pub const MAX_PPS: f32 = 400.;
pub const ZOOM_STEP: f32 = 1.5;

/// Edits are written to disk this long after the last one.
const AUTOSAVE_MS: u64 = 1500;

/// Toasts stay up this long.
const TOAST_MS: u64 = 1600;

/// Panel sizes for one window size.
#[derive(Clone, Copy, Debug)]
pub struct PanelSizes {
    pub browser: f32,
    pub inspector: f32,
    pub timeline: f32,
}

/// `v` within lo..hi, where lo wins when the range is empty.
fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    v.max(lo).min(hi.max(lo))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Browser,
    Inspector,
    Timeline,
}

/// The timeline's pointer tool: Select, or the Split blade (a click cuts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Split,
}

pub type SliderFn = Rc<dyn Fn(&mut Editor, f32, bool, &mut Context<Editor>)>;

/// A mouse drag in progress. Moves and the release reach the window root,
/// wherever the pointer is.
pub enum Drag {
    Splitter {
        panel: Panel,
        origin: f32,
        size: f32,
    },
    Scrub,
    Zoom,
    /// A slider at `key` (its bounds are measured under that key); `apply`
    /// gets the 0–1 position, and `true` on release.
    Slider {
        key: &'static str,
        apply: SliderFn,
    },
    /// A timeline clip moved or trimmed (`ui/timeline_edit.rs`).
    Clip(Box<ClipDrag>),
    /// A rubber band over the lanes (⇧ / ⌘ + drag on empty space).
    Marquee(Box<Marquee>),
}

/// A copied clip, pasted back onto its lane (or the first of its kind).
#[derive(Clone)]
pub struct ClipboardEntry {
    pub clip: Clip,
    pub track_id: String,
    pub track_kind: TrackType,
}

pub type PopupAction = Rc<dyn Fn(&mut Editor, &mut Window, &mut Context<Editor>)>;

/// A popup menu row: a label with an optional tick (toggles), a shortcut
/// hint on the right, or a separator line.
pub struct PopupItem {
    pub label: SharedString,
    pub checked: bool,
    pub hint: Option<SharedString>,
    pub disabled: bool,
    /// Drawn in the danger colour (Delete).
    pub destructive: bool,
    pub separator: bool,
    pub run: PopupAction,
}

impl PopupItem {
    pub fn new(
        label: impl Into<SharedString>,
        run: impl Fn(&mut Editor, &mut Window, &mut Context<Editor>) + 'static,
    ) -> Self {
        PopupItem {
            label: label.into(),
            checked: false,
            hint: None,
            disabled: false,
            destructive: false,
            separator: false,
            run: Rc::new(run),
        }
    }

    pub fn separator() -> Self {
        PopupItem {
            separator: true,
            ..PopupItem::new("", |_, _, _| {})
        }
    }

    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }

    pub fn hint(mut self, id: Shortcut) -> Self {
        self.hint = Some(shortcuts::hint(id).into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }
}

/// The projects list, or a project open in the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    Editor,
}

/// A yes / no question over the window (a destructive action).
pub struct Confirm {
    pub title: SharedString,
    pub message: SharedString,
    pub confirm_label: SharedString,
    pub run: PopupAction,
}

/// A small menu opened at a point (resolution, ratio, context menus).
pub struct Popup {
    pub at: Point<Pixels>,
    pub items: Vec<PopupItem>,
}

pub struct Editor {
    pub(crate) project: Project,
    pub(crate) engine: Engine,
    pub(crate) media: Vec<MediaItem>,
    pub(crate) selected: Selection,
    pub(crate) playhead: Micros,
    /// Timeline zoom, pixels per second.
    pub(crate) pps: f32,
    pub(crate) scroll_x: f32,

    pub(crate) history: History,
    pub(crate) prefs: Prefs,
    pub(crate) tool: Tool,
    pub(crate) clipboard: Option<ClipboardEntry>,
    /// Edited since it was last saved.
    pub(crate) dirty: bool,
    /// Which screen the window shows.
    pub(crate) screen: Screen,
    /// The projects list (home screen).
    pub(crate) home_projects: Vec<projects::Summary>,
    pub(crate) home_greeting: &'static str,
    /// A question over everything (delete a project?).
    pub(crate) confirm: Option<Confirm>,
    autosave_token: u64,

    pub(crate) fullscreen: bool,
    pub(crate) shortcuts_open: bool,
    pub(crate) popup: Option<Popup>,
    pub(crate) drag: Option<Drag>,
    /// A slider's value while it is dragged (committed on release).
    pub(crate) live_value: Option<(&'static str, f64)>,
    /// Where the dragged slider was last (0–1); its release commits this.
    last_slider_fraction: Option<f32>,
    pub(crate) toast: Option<SharedString>,
    toast_token: u64,
    /// The browser's tab, and each tab's chosen view.
    pub(crate) browser_tab: usize,
    pub(crate) browser_views: [usize; 8],
    /// Media tile pictures, by file path.
    pub(crate) thumbs: HashMap<String, Arc<RenderImage>>,
    /// The inspector's open sub-tab (entry key) for the selected clip.
    pub(crate) inspector_entry: Option<&'static str>,
    /// Where measured elements sit in the window, by name.
    pub(crate) bounds: Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>,

    /// The lane stack's vertical scroll (px).
    pub(crate) lane_scroll_y: f32,
    /// The time under the pointer over the lanes (the hover line; with the
    /// preview axis on, the frame the player shows).
    pub(crate) hover: Option<Micros>,
    /// Where something dragged over the timeline would land.
    pub(crate) media_drop: Option<MediaDrop>,
    /// The pointer's last window position (edge auto-scroll).
    pub(crate) pointer: Point<Pixels>,
    pub(crate) edge_scroll_at: Option<Instant>,
    /// Filmstrip tiles by (file, source time), and the ones asked for.
    pub(crate) tiles: HashMap<(String, Micros), Arc<RenderImage>>,
    pub(crate) tiles_pending: HashSet<(String, Micros)>,
    pub(crate) tile_server: TileServer,
    /// Waveform peaks by file (None while decoding, or when it has no sound).
    pub(crate) waves: HashMap<String, Option<Arc<Waveform>>>,

    servers: HashMap<String, FrameServer>,
    frame_tx: async_channel::Sender<DecodedFrame>,
    wanted: Option<(String, Micros)>,
    pub(crate) frame: Option<Arc<RenderImage>>,
    stale_frames: Vec<Arc<RenderImage>>,
    pub(crate) frame_info: Option<(u32, u32, Micros)>,
    fps_window: (Instant, u32),
    pub(crate) preview_fps: f32,

    pub(crate) focus: FocusHandle,
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

        let (tile_tx, tile_rx) = async_channel::unbounded::<DecodedFrame>();
        cx.spawn(async move |this, cx| {
            while let Ok(tile) = tile_rx.recv().await {
                if this.update(cx, |ed, cx| ed.on_tile(tile, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        let focus = cx.focus_handle();
        window.focus(&focus);
        let project = Project::empty();
        let mut engine = Engine::new();
        if let Err(e) = engine.load(&project) {
            eprintln!("engine refused the empty project: {e}");
        }
        Editor {
            project,
            engine,
            media: vec![],
            selected: Selection::default(),
            playhead: 0,
            pps: 80.,
            scroll_x: 0.,
            history: History::default(),
            prefs: Prefs::load(),
            tool: Tool::Select,
            clipboard: None,
            dirty: false,
            screen: Screen::Home,
            home_projects: vec![],
            home_greeting: "",
            confirm: None,
            autosave_token: 0,
            fullscreen: false,
            shortcuts_open: false,
            popup: None,
            drag: None,
            live_value: None,
            last_slider_fraction: None,
            toast: None,
            toast_token: 0,
            browser_tab: 0,
            browser_views: [0; 8],
            thumbs: HashMap::new(),
            inspector_entry: None,
            bounds: Rc::default(),
            lane_scroll_y: 0.,
            hover: None,
            media_drop: None,
            pointer: Point::default(),
            edge_scroll_at: None,
            tiles: HashMap::new(),
            tiles_pending: HashSet::new(),
            tile_server: TileServer::spawn(TILE_MAX_WIDTH, tile_tx),
            waves: HashMap::new(),
            servers: HashMap::new(),
            frame_tx,
            wanted: None,
            frame: None,
            stale_frames: vec![],
            frame_info: None,
            fps_window: (Instant::now(), 0),
            preview_fps: 0.,
            focus,
        }
    }

    // ---- small helpers ----------------------------------------------------

    pub fn fps(&self) -> f64 {
        if self.project.canvas.fps > 0. {
            self.project.canvas.fps
        } else {
            30.
        }
    }

    pub fn frame_micros(&self) -> Micros {
        (SECOND as f64 / self.fps()).round() as Micros
    }

    /// Shows a short status message at the bottom of the window.
    pub fn toast(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.toast_token += 1;
        let token = self.toast_token;
        self.toast = Some(message.into());
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(TOAST_MS))
                .await;
            this.update(cx, |ed, cx| {
                if ed.toast_token == token {
                    ed.toast = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// A canvas that records where its parent sits under `key`.
    pub fn measure(&self, key: &'static str) -> impl IntoElement {
        let map = self.bounds.clone();
        canvas(
            move |bounds, _, _| {
                map.borrow_mut().insert(key, bounds);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }

    pub fn measured(&self, key: &str) -> Option<Bounds<Pixels>> {
        self.bounds.borrow().get(key).copied()
    }

    pub fn selected_clip(&self) -> Option<(&model::Track, &Clip)> {
        self.selected
            .primary()
            .and_then(|id| self.project.find_clip(id))
    }

    // ---- project edits ---------------------------------------------------

    /// Pushes the document to the engine and puts the playhead back (as the
    /// app's store → VideoEngine.updateProject does on every edit).
    fn commit(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.engine.load(&self.project) {
            self.toast(format!("Engine refused the project: {e}"), cx);
        }
        self.playhead = self.playhead.min(self.project.duration);
        self.engine.seek(self.playhead);
        let project = &self.project;
        self.selected.retain(|id| project.find_clip(id).is_some());
        self.request_preview();
        cx.notify();
    }

    /// One undoable edit. `f` changes the document and says whether it did;
    /// with `magnet`, the main lane is re-packed afterwards while the main
    /// track magnet is on. `coalesce` merges a quick run of edits to the
    /// same thing into one undo step.
    pub fn edit(
        &mut self,
        label: &'static str,
        magnet: bool,
        coalesce: Option<String>,
        f: impl FnOnce(&mut Project) -> bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let before = self.project.clone();
        let mut next = self.project.clone();
        if !f(&mut next) {
            return false;
        }
        if magnet && self.prefs.main_track_magnet {
            next = model::with_magnet(&before, next, self.prefs.linkage);
        }
        self.project = next;
        self.history.record(label, before, coalesce.as_deref());
        self.dirty = true;
        self.schedule_autosave(cx);
        self.commit(cx);
        true
    }

    fn on_main_lane(&self, clip_id: &str) -> bool {
        self.project
            .track_of(clip_id)
            .is_some_and(|t| self.project.is_main_track(&t.id))
    }

    /// Files from Import or a drop in the editor: projects open, media goes
    /// into the library (not the timeline).
    pub fn open_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        for path in paths {
            if path.ends_with(".json") {
                self.import_project_file(&path, cx);
                continue;
            }
            match decoder::probe(&path) {
                Ok(item) => self.add_to_library(item, cx),
                Err(e) => self.toast(format!("Can't open {path}: {e}"), cx),
            }
        }
        cx.notify();
    }

    /// Decodes a small picture of `path` at `at` into `thumbs` (off the UI
    /// thread); a no-op when there is one already.
    pub fn load_thumbnail(&mut self, path: String, at: Micros, cx: &mut Context<Self>) {
        if self.thumbs.contains_key(&path) {
            return;
        }
        let task = cx
            .background_executor()
            .spawn(async move { decoder::thumbnail(&path, at, 320) });
        cx.spawn(async move |this, cx| {
            let Ok(f) = task.await else { return };
            let Some(buffer) = image::RgbaImage::from_raw(f.width, f.height, f.bgra) else {
                return;
            };
            let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
            this.update(cx, |ed, cx| {
                ed.thumbs.insert(f.source, image);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn add_to_library(&mut self, item: MediaItem, cx: &mut Context<Self>) {
        if self.media.iter().any(|m| m.path == item.path) {
            return;
        }
        if item.has_video {
            let at = if item.still {
                0
            } else {
                (item.duration / 2).min(SECOND)
            };
            self.load_thumbnail(item.path.clone(), at, cx);
        }
        self.media.push(item);
    }

    /// A library tile's click: into the main lane at the playhead (before
    /// the clip whose middle is past it), selected.
    pub fn add_at_playhead(&mut self, item: &MediaItem, cx: &mut Context<Self>) {
        let index = self.project.insert_index_at(self.playhead);
        let mut id = String::new();
        self.edit(
            "Add Clip",
            false,
            None,
            |p| {
                id = p.insert_media(item, index);
                true
            },
            cx,
        );
        self.selected.set(id);
        cx.notify();
    }

    /// Files given on the command line start a new project with them.
    pub fn open_from_command_line(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.new_project_with(paths, cx);
    }

    // ---- projects -----------------------------------------------------------

    /// Puts the editor on `project` with a fresh session (no undo, nothing
    /// selected, playhead at 0), its media in the library.
    fn start_session(&mut self, project: Project, cx: &mut Context<Self>) {
        self.pause();
        self.servers.clear();
        self.wanted = None;
        if let Some(old) = self.frame.take() {
            self.stale_frames.push(old);
        }
        self.frame_info = None;
        self.media.clear();
        let sources: Vec<String> = project
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter_map(|c| c.source.clone())
            .filter(|s| !s.is_empty())
            .collect();
        for src in sources {
            if let Ok(item) = decoder::probe(&src) {
                self.add_to_library(item, cx);
            }
        }
        self.project = project;
        self.history.clear();
        self.dirty = false;
        self.selected.clear();
        self.inspector_entry = None;
        self.tool = Tool::Select;
        self.fullscreen = false;
        self.playhead = 0;
        self.scroll_x = 0.;
        self.lane_scroll_y = 0.;
        self.hover = None;
        self.drag = None;
        self.tiles.clear();
        self.tiles_pending.clear();
        self.screen = Screen::Editor;
        self.commit(cx);
    }

    /// New video: a fresh project with `paths` in its library and, in
    /// order, on its main lane (the first picture sets the canvas). Saved
    /// and opened only when something could be added.
    pub fn new_project_with(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        let items: Vec<MediaItem> = paths
            .iter()
            .filter_map(|p| match decoder::probe(p) {
                Ok(item) => Some(item),
                Err(e) => {
                    self.toast(format!("Can't open {p}: {e}"), cx);
                    None
                }
            })
            .collect();
        if items.is_empty() {
            return;
        }
        let mut project = Project::empty();
        project.created_at = projects::now_iso();
        for item in &items {
            project.insert_media(item, None);
        }
        if let Err(e) = projects::save(&mut project) {
            self.toast(format!("Couldn't save the project: {e}"), cx);
            return;
        }
        self.start_session(project, cx);
        for item in items {
            self.add_to_library(item, cx);
        }
        self.zoom_to_fit(cx);
    }

    pub fn open_saved_project(&mut self, id: &str, cx: &mut Context<Self>) {
        match projects::load(id) {
            Ok(project) => {
                self.start_session(project, cx);
                self.zoom_to_fit(cx);
            }
            Err(e) => self.toast(format!("Couldn't open the project: {e}"), cx),
        }
    }

    /// A project JSON from elsewhere (⌘O, a drop): copied into the projects
    /// folder and opened. One with the same id as a saved project gets a
    /// new id, so it never overwrites that one.
    fn import_project_file(&mut self, path: &str, cx: &mut Context<Self>) {
        let parsed = std::fs::read_to_string(path)
            .map_err(anyhow::Error::from)
            .and_then(|s| Ok(serde_json::from_str::<Project>(&s)?));
        match parsed {
            Ok(mut project) => {
                if projects::load(&project.id).is_ok() {
                    project.id = model::new_id("proj");
                }
                if let Err(e) = projects::save(&mut project) {
                    self.toast(format!("Couldn't save the project: {e}"), cx);
                    return;
                }
                let name = project.name.clone();
                self.start_session(project, cx);
                self.toast(format!("Opened {name}"), cx);
            }
            Err(e) => self.toast(format!("Not a project file: {e}"), cx),
        }
    }

    /// Writes the project to its file if it changed. True when it's on disk.
    pub fn save_now(&mut self, cx: &mut Context<Self>) -> bool {
        if self.screen != Screen::Editor || !self.dirty {
            return true;
        }
        match projects::save(&mut self.project) {
            Ok(()) => {
                self.dirty = false;
                cx.notify();
                true
            }
            Err(e) => {
                // Stays dirty, so the next save tries again.
                self.toast(format!("Save failed: {e}"), cx);
                false
            }
        }
    }

    /// Saves shortly after the last edit (the newest edit restarts it).
    fn schedule_autosave(&mut self, cx: &mut Context<Self>) {
        self.autosave_token += 1;
        let token = self.autosave_token;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(AUTOSAVE_MS))
                .await;
            this.update(cx, |ed, cx| {
                if ed.autosave_token == token {
                    ed.save_now(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// The header's ✕: saves, then back to the projects list.
    pub fn close_project(&mut self, cx: &mut Context<Self>) {
        if !self.save_now(cx) {
            return;
        }
        self.pause();
        self.servers.clear();
        self.wanted = None;
        if let Some(old) = self.frame.take() {
            self.stale_frames.push(old);
        }
        self.popup = None;
        self.screen = Screen::Home;
        self.refresh_home(cx);
    }

    /// Re-reads the projects list (and asks for the rows' thumbnails).
    pub fn refresh_home(&mut self, cx: &mut Context<Self>) {
        self.home_projects = projects::list();
        self.home_greeting = crate::ui::home::greeting();
        let previews: Vec<(String, Micros)> = self
            .home_projects
            .iter()
            .filter_map(|p| p.preview.clone())
            .collect();
        for (src, at) in previews {
            self.load_thumbnail(src, at, cx);
        }
        cx.notify();
    }

    pub fn delete_saved_project(&mut self, id: &str, cx: &mut Context<Self>) {
        match projects::delete(id) {
            Ok(()) => {
                self.refresh_home(cx);
                self.toast("Deleted 1 project", cx);
            }
            Err(e) => self.toast(format!("Couldn't delete it: {e}"), cx),
        }
    }

    // ---- file actions ---------------------------------------------------

    /// ⌘I: in the editor, files into the library; on the projects list, a
    /// new video from them.
    pub fn import(&mut self, _: &ImportMedia, _: &mut Window, cx: &mut Context<Self>) {
        let home = self.screen == Screen::Home;
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(if home { "New video" } else { "Import" }.into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let paths = paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                this.update(cx, |ed, cx| {
                    if home {
                        ed.new_project_with(paths, cx)
                    } else {
                        ed.open_paths(paths, cx)
                    }
                })
                .ok();
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
                this.update(cx, |ed, cx| {
                    if ed.save_now(cx) {
                        ed.import_project_file(&path, cx)
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn save_project(&mut self, _: &SaveProject, _: &mut Window, cx: &mut Context<Self>) {
        // The project lives in the projects folder: ⌘S just writes it now.
        self.dirty |= self.screen == Screen::Editor;
        self.save_now(cx);
    }

    /// Writes a copy of the project as a JSON file somewhere else.
    fn save_project_as(&mut self, _: &SaveProjectAs, _: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Editor {
            return;
        }
        let dir = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
        let name = format!("{}.json", self.project.name);
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        let json = serde_json::to_string_pretty(&self.project).unwrap_or_default();
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let msg = match std::fs::write(&path, json) {
                    Ok(()) => format!("Saved a copy to {}", path.display()),
                    Err(e) => format!("Save failed: {e}"),
                };
                this.update(cx, |ed, cx| ed.toast(msg, cx)).ok();
            }
        })
        .detach();
    }

    // ---- shortcuts --------------------------------------------------------

    /// Runs a shortcut (key, menu or button). While the Shortcuts dialog is
    /// open only ⌘/ and Esc (close it) do anything.
    pub fn run(&mut self, id: Shortcut, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm.is_some() {
            if id == Shortcut::Deselect {
                self.confirm = None;
                cx.notify();
            }
            return;
        }
        if self.screen == Screen::Home && id != Shortcut::ShowShortcuts {
            return;
        }
        if self.shortcuts_open {
            if matches!(id, Shortcut::ShowShortcuts | Shortcut::Deselect) {
                self.shortcuts_open = false;
                cx.notify();
            }
            return;
        }
        if self.popup.take().is_some() {
            cx.notify();
            if id == Shortcut::Deselect {
                return;
            }
        }
        match id {
            Shortcut::PlayPause => self.toggle_play(cx),
            Shortcut::PrevFrame => self.step_frames(-1, cx),
            Shortcut::NextFrame => self.step_frames(1, cx),
            Shortcut::Back1s => self.seek_by(-SECOND, cx),
            Shortcut::Forward1s => self.seek_by(SECOND, cx),
            Shortcut::PrevEdit => self.jump_to_edit_point(-1, cx),
            Shortcut::NextEdit => self.jump_to_edit_point(1, cx),
            Shortcut::GoStart => {
                self.pause();
                self.seek(0, cx)
            }
            Shortcut::GoEnd => {
                self.pause();
                self.seek(self.project.duration, cx)
            }
            Shortcut::SplitClip => self.split_at_playhead(cx),
            Shortcut::DeleteLeft => self.delete_at_playhead(Side::Start, cx),
            Shortcut::DeleteRight => self.delete_at_playhead(Side::End, cx),
            Shortcut::Deactivate => self.toggle_clip_active(cx),
            Shortcut::DeleteClip => self.delete_selected(cx),
            Shortcut::Duplicate => self.duplicate_selected(cx),
            Shortcut::CopyClip => {
                self.copy_selected(cx);
            }
            Shortcut::CutClip => {
                if self.copy_selected(cx) {
                    self.delete_selected(cx);
                }
            }
            Shortcut::PasteClip => self.paste(cx),
            Shortcut::Undo => self.undo(cx),
            Shortcut::Redo => self.redo(cx),
            Shortcut::Deselect => {
                if matches!(self.drag, Some(Drag::Clip(_))) {
                    // Esc mid drag: the clip goes back where it was.
                    self.drag = None;
                } else if self.fullscreen {
                    self.fullscreen = false;
                } else if self.tool == Tool::Split {
                    self.tool = Tool::Select;
                } else {
                    self.selected.clear();
                }
                cx.notify();
            }
            Shortcut::ZoomIn => self.zoom_step(ZOOM_STEP, cx),
            Shortcut::ZoomOut => self.zoom_step(1. / ZOOM_STEP, cx),
            Shortcut::ZoomFit => self.zoom_to_fit(cx),
            Shortcut::PreviewAxis => {
                self.prefs.preview_axis = !self.prefs.preview_axis;
                let on = self.prefs.preview_axis;
                self.prefs.save();
                self.toast(
                    if on {
                        "Preview axis on"
                    } else {
                        "Preview axis off"
                    },
                    cx,
                );
            }
            Shortcut::Snapping => {
                self.prefs.snapping = !self.prefs.snapping;
                let on = self.prefs.snapping;
                self.prefs.save();
                self.toast(
                    if on {
                        "Auto snapping on"
                    } else {
                        "Auto snapping off"
                    },
                    cx,
                );
            }
            Shortcut::MainMagnet => {
                self.prefs.main_track_magnet = !self.prefs.main_track_magnet;
                let on = self.prefs.main_track_magnet;
                self.prefs.save();
                self.toast(
                    if on {
                        "Main track magnet on"
                    } else {
                        "Main track magnet off"
                    },
                    cx,
                );
            }
            Shortcut::SelectTool => {
                self.tool = Tool::Select;
                cx.notify();
            }
            Shortcut::SplitTool => {
                self.tool = Tool::Split;
                cx.notify();
            }
            Shortcut::Fullscreen => {
                self.fullscreen = !self.fullscreen;
                cx.notify();
            }
            Shortcut::Export => self.export(cx),
            Shortcut::ShowShortcuts => {
                self.shortcuts_open = true;
                cx.notify();
            }
        }
        let _ = window;
    }

    pub fn export(&mut self, cx: &mut Context<Self>) {
        // The engine has no desktop encoder yet (docs/PARITY.md, Export).
        self.toast("Export isn't available in this build yet", cx);
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        if let Some((_, project)) = self.history.undo(self.project.clone()) {
            self.project = project;
            self.dirty = true;
            self.schedule_autosave(cx);
            self.commit(cx);
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        if let Some((_, project)) = self.history.redo(self.project.clone()) {
            self.project = project;
            self.dirty = true;
            self.schedule_autosave(cx);
            self.commit(cx);
        }
    }

    /// ⌘B: the selected clip under the playhead, else the main-lane clip
    /// under it.
    pub fn split_at_playhead(&mut self, cx: &mut Context<Self>) {
        let now = self.playhead;
        let inside = |c: &Clip| now > c.timeline_start && now < c.end();
        let target = self
            .selected_clip()
            .filter(|(_, c)| inside(c))
            .map(|(_, c)| c.id.clone())
            .or_else(|| {
                self.project
                    .main_track()
                    .and_then(|t| t.clips.iter().find(|c| inside(c)))
                    .map(|c| c.id.clone())
            });
        let Some(id) = target else {
            self.toast("Move the playhead inside the clip to split", cx);
            return;
        };
        if self.edit("Split Clip", false, None, |p| p.split_at(&id, now), cx) {
            self.toast("Split at playhead", cx);
        }
    }

    /// The Split tool's click: cuts `clip_id` at `t`, snapped to a frame.
    pub fn split_clip_at(&mut self, clip_id: &str, t: Micros, cx: &mut Context<Self>) {
        let fps = self.fps();
        let at = ((t as f64 / SECOND as f64 * fps).round() / fps * SECOND as f64).round() as Micros;
        let id = clip_id.to_string();
        self.edit("Split Clip", false, None, |p| p.split_at(&id, at), cx);
    }

    /// ⌫: every selected clip, as one undo step (the magnet closes the
    /// main lane up when any was on it).
    pub fn delete_selected(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.deletable_selection();
        if ids.is_empty() {
            return;
        }
        let main = ids.iter().any(|id| self.on_main_lane(id));
        let label = if ids.len() == 1 {
            "Delete Clip"
        } else {
            "Delete Clips"
        };
        if self.edit(label, main, None, |p| p.delete_clips(&ids), cx) {
            self.selected.clear();
            self.toast(
                if ids.len() == 1 {
                    "Clip deleted".to_string()
                } else {
                    format!("{} clips deleted", ids.len())
                },
                cx,
            );
        }
    }

    /// The selected clips an edit may touch (not on a locked lane, not
    /// locked themselves).
    fn deletable_selection(&self) -> Vec<String> {
        self.selected
            .ids()
            .iter()
            .filter(|id| {
                self.project
                    .find_clip(id)
                    .is_some_and(|(t, c)| !t.locked && !c.locked())
            })
            .cloned()
            .collect()
    }

    pub fn duplicate_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected.primary().map(str::to_owned) else {
            return;
        };
        let main = self.on_main_lane(&id);
        let mut copy = None;
        self.edit(
            "Duplicate Clip",
            main,
            None,
            |p| {
                copy = p.duplicate_clip(&id);
                copy.is_some()
            },
            cx,
        );
        if copy.is_some() {
            self.selected.set_opt(copy);
            self.toast("Clip duplicated", cx);
        }
    }

    /// Delete left (Q) / right (W): trims away the part of the clip before
    /// / after the playhead. The selected clip when the playhead is inside
    /// it, else the main clip under the playhead. On the main lane the
    /// magnet closes up the cut.
    pub fn delete_at_playhead(&mut self, side: Side, cx: &mut Context<Self>) {
        // Leaves at least a frame either side.
        let slack = self.frame_micros();
        let now = self.playhead;
        let inside = |c: &Clip| now > c.timeline_start + slack && now < c.end() - slack;
        let target = self
            .selected_clip()
            .filter(|(_, c)| inside(c))
            .map(|(_, c)| c.id.clone())
            .or_else(|| {
                self.project
                    .main_track()
                    .and_then(|t| t.clips.iter().find(|c| inside(c)))
                    .map(|c| c.id.clone())
            });
        let Some(id) = target else {
            self.toast("Move the playhead inside the clip to split", cx);
            return;
        };
        let main = self.on_main_lane(&id);
        let label = if side == Side::Start {
            "Delete Left"
        } else {
            "Delete Right"
        };
        self.edit(label, main, None, |p| p.trim_at(&id, now, side), cx);
    }

    /// V: the clip stays on the timeline, dimmed; preview and export skip
    /// it. With several selected: all off while any is on, else all on.
    pub fn toggle_clip_active(&mut self, cx: &mut Context<Self>) {
        let ids = self.deletable_selection();
        if ids.is_empty() {
            return;
        }
        let hidden = !ids
            .iter()
            .all(|id| self.project.find_clip(id).is_some_and(|(_, c)| c.hidden));
        self.edit(
            "Deactivate Clip",
            false,
            None,
            |p| {
                for id in &ids {
                    p.set_clip_hidden(id, hidden);
                }
                true
            },
            cx,
        );
        self.toast(
            if hidden {
                "Deactivate clip"
            } else {
                "Activate clip"
            },
            cx,
        );
    }

    fn copy_selected(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((track, clip)) = self.selected_clip() else {
            return false;
        };
        self.clipboard = Some(ClipboardEntry {
            clip: clip.clone(),
            track_id: track.id.clone(),
            track_kind: track.kind,
        });
        self.toast("Clip copied", cx);
        true
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.clipboard.clone() else {
            self.toast("Nothing to paste — copy a clip first", cx);
            return;
        };
        let main = self
            .project
            .tracks
            .iter()
            .find(|t| t.id == entry.track_id)
            .map_or(entry.track_kind == TrackType::Video, |t| {
                self.project.is_main_track(&t.id)
            });
        let at = self.playhead;
        let mut id = None;
        self.edit(
            "Paste Clip",
            main,
            None,
            |p| {
                id = p.paste_clip(&entry.clip, &entry.track_id, entry.track_kind, at);
                id.is_some()
            },
            cx,
        );
        match id {
            Some(id) => {
                self.selected.set(id);
                self.toast("Clip pasted", cx);
            }
            None => self.toast("Nothing to paste — copy a clip first", cx),
        }
    }

    pub fn set_speed(&mut self, clip_id: &str, speed: f64, cx: &mut Context<Self>) {
        let id = clip_id.to_string();
        let main = self.on_main_lane(&id);
        self.edit(
            "Change Speed",
            main,
            Some(format!("speed:{id}")),
            |p| {
                let changed = p.find_clip(&id).is_some_and(|(_, c)| c.speed != speed);
                p.set_speed(&id, speed);
                changed
            },
            cx,
        );
    }

    /// The canvas to `ratio` (width / height) with a 1080 px short side, or
    /// to the first main clip's own size (`None`, the Original preset).
    pub fn set_canvas_ratio(&mut self, ratio: Option<f64>, cx: &mut Context<Self>) {
        let size = match ratio {
            Some(r) => crate::ui::player::canvas_size_for_ratio(r),
            None => {
                let first = self
                    .project
                    .ordered_main_clips()
                    .first()
                    .and_then(|c| c.source.clone());
                match first.and_then(|src| self.media.iter().find(|m| m.path == src)) {
                    Some(m) if m.width > 0 => (m.width, m.height),
                    _ => return,
                }
            }
        };
        self.edit(
            "Change Ratio",
            false,
            None,
            |p| {
                let changed = (p.canvas.width, p.canvas.height) != size;
                (p.canvas.width, p.canvas.height) = size;
                changed
            },
            cx,
        );
    }

    // ---- playback + preview ---------------------------------------------

    pub fn pause(&mut self) {
        if self.engine.is_playing() {
            self.engine.pause();
            self.playhead = self.engine.tick();
        }
    }

    pub fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if self.engine.is_playing() {
            self.pause();
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

    pub fn step_frames(&mut self, frames: i64, cx: &mut Context<Self>) {
        self.pause();
        let fps = self.fps();
        let frame = (self.playhead as f64 / SECOND as f64 * fps).round() as i64 + frames;
        let t = (frame as f64 / fps * SECOND as f64).round() as Micros;
        self.seek(t, cx);
    }

    fn seek_by(&mut self, d: Micros, cx: &mut Context<Self>) {
        self.pause();
        self.seek(self.playhead + d, cx);
    }

    /// ↑ / ↓: the previous / next clip start or end on any lane.
    fn jump_to_edit_point(&mut self, dir: i32, cx: &mut Context<Self>) {
        self.pause();
        // A millisecond's slack, so a playhead sitting on a point moves on.
        const EPS: Micros = 1_000;
        let now = self.playhead;
        let points = self.project.edit_points();
        let target = if dir > 0 {
            points.into_iter().find(|&p| p > now + EPS)
        } else {
            points.into_iter().rev().find(|&p| p < now - EPS)
        };
        if let Some(t) = target {
            self.seek(t, cx);
        }
    }

    pub fn seek(&mut self, t: Micros, cx: &mut Context<Self>) {
        self.playhead = t.clamp(0, self.project.duration);
        self.engine.seek(self.playhead);
        self.request_preview();
        cx.notify();
    }

    /// Asks the decoder of the top visible clip for the frame at the playhead.
    pub(crate) fn request_preview(&mut self) {
        let top = self
            .engine
            .active_layers(self.preview_time())
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

    /// What the player shows: the hovered time while the preview axis is
    /// on (and nothing plays or drags), else the playhead.
    pub fn preview_time(&self) -> Micros {
        match self.hover {
            Some(t)
                if self.prefs.preview_axis && self.drag.is_none() && !self.engine.is_playing() =>
            {
                t
            }
            _ => self.playhead,
        }
    }

    fn on_tile(&mut self, f: DecodedFrame, cx: &mut Context<Self>) {
        let key = (f.source, f.pts);
        self.tiles_pending.remove(&key);
        let Some(buffer) = image::RgbaImage::from_raw(f.width, f.height, f.bgra) else {
            return;
        };
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
        self.tiles.insert(key, image);
        cx.notify();
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
        let (since, n) = &mut self.fps_window;
        *n += 1;
        let elapsed = since.elapsed().as_secs_f32();
        if elapsed >= 1. {
            self.preview_fps = *n as f32 / elapsed;
            self.fps_window = (Instant::now(), 0);
        }
        cx.notify();
    }

    // ---- timeline view ----------------------------------------------------

    pub fn set_zoom(&mut self, pps: f32, cx: &mut Context<Self>) {
        let x = self.zoom_anchor();
        self.zoom_at(pps, x, cx);
    }

    /// ⌘= / ⌘-: zooms around the playhead while it's in view.
    fn zoom_step(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.set_zoom(self.pps * factor, cx);
    }

    /// Where a toolbar zoom pivots: the playhead if in view, else the left.
    fn zoom_anchor(&self) -> f32 {
        let width = self
            .measured("lanes")
            .map_or(800., |b| f32::from(b.size.width));
        let x = self.x_of(self.playhead);
        if (0. ..=width).contains(&x) { x } else { 0. }
    }

    /// ⇧Z: the whole project fits the lane area, scrolled to the start.
    pub fn zoom_to_fit(&mut self, cx: &mut Context<Self>) {
        let width = self
            .measured("lanes")
            .map_or(800., |b| f32::from(b.size.width));
        let secs = (self.project.duration as f32 / SECOND as f32).max(1.);
        // Room kept right of the end, as the old editor's fit.
        self.pps = ((width - 80.).max(100.) / secs).clamp(MIN_PPS, MAX_PPS);
        self.scroll_x = 0.;
        cx.notify();
    }

    pub fn scrub_to(&mut self, window_x: Pixels, cx: &mut Context<Self>) {
        self.pause();
        let t = self.time_at(window_x);
        self.seek(t, cx);
    }

    // ---- panel layout -------------------------------------------------------

    pub fn panel_sizes(&self, viewport: Size<Pixels>) -> PanelSizes {
        let (w, h) = (f32::from(viewport.width), f32::from(viewport.height));
        let inspector = clamp(
            self.prefs.inspector_width,
            INSPECTOR_MIN,
            INSPECTOR_MAX.min(w - MIN_PLAYER_WIDTH - BROWSER_MIN - GUTTER * 4.),
        );
        let browser = clamp(
            self.prefs
                .browser_width
                .unwrap_or_else(|| clamp(w * 0.24, BROWSER_DEFAULT_MIN, BROWSER_DEFAULT_MAX)),
            BROWSER_MIN,
            BROWSER_MAX.min(w - MIN_PLAYER_WIDTH - inspector - GUTTER * 4.),
        );
        let timeline = clamp(
            self.prefs
                .timeline_height
                .unwrap_or_else(|| (h * TIMELINE_SHARE).round()),
            TIMELINE_MIN,
            h - HEADER_HEIGHT - MIN_TOP_HEIGHT - GUTTER * 3.,
        );
        PanelSizes {
            browser,
            inspector,
            timeline,
        }
    }

    fn resize_panel(
        &mut self,
        panel: Panel,
        origin: f32,
        size: f32,
        pos: Point<Pixels>,
        window: &Window,
    ) {
        let viewport = window.viewport_size();
        let (w, h) = (f32::from(viewport.width), f32::from(viewport.height));
        let now = self.panel_sizes(viewport);
        match panel {
            Panel::Browser => {
                let v = size + f32::from(pos.x) - origin;
                self.prefs.browser_width = Some(
                    clamp(
                        v,
                        BROWSER_MIN,
                        BROWSER_MAX.min(w - MIN_PLAYER_WIDTH - now.inspector - GUTTER * 4.),
                    )
                    .round(),
                );
            }
            Panel::Inspector => {
                let v = size - (f32::from(pos.x) - origin);
                self.prefs.inspector_width = clamp(
                    v,
                    INSPECTOR_MIN,
                    INSPECTOR_MAX.min(w - MIN_PLAYER_WIDTH - now.browser - GUTTER * 4.),
                )
                .round();
            }
            Panel::Timeline => {
                let v = size - (f32::from(pos.y) - origin);
                self.prefs.timeline_height = Some(
                    clamp(
                        v,
                        TIMELINE_MIN,
                        h - HEADER_HEIGHT - MIN_TOP_HEIGHT - GUTTER * 3.,
                    )
                    .round(),
                );
            }
        }
    }

    fn reset_panel(&mut self, panel: Panel) {
        match panel {
            Panel::Browser => self.prefs.browser_width = None,
            Panel::Inspector => self.prefs.inspector_width = INSPECTOR_DEFAULT,
            Panel::Timeline => self.prefs.timeline_height = None,
        }
        self.prefs.save();
    }

    /// The gap between two panels, as a grab bar: hover lights it, a drag
    /// resizes the panel beside it, a double-click puts it back to default.
    fn splitter(&self, panel: Panel, size: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let column = panel != Panel::Timeline;
        let dragging = matches!(&self.drag, Some(Drag::Splitter { panel: p, .. }) if *p == panel);
        div()
            .id(match panel {
                Panel::Browser => "split-browser",
                Panel::Inspector => "split-inspector",
                Panel::Timeline => "split-timeline",
            })
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .map(|d| {
                if column {
                    d.w(px(GUTTER))
                        .h_full()
                        .cursor(CursorStyle::ResizeLeftRight)
                } else {
                    d.h(px(GUTTER)).w_full().cursor(CursorStyle::ResizeUpDown)
                }
            })
            .group("splitter")
            .child(
                div()
                    .rounded(px(1.))
                    .map(|d| {
                        if column {
                            d.w(px(2.)).h_full()
                        } else {
                            d.h(px(2.)).w_full()
                        }
                    })
                    .when(dragging, |d| d.bg(theme::text_secondary()))
                    .group_hover("splitter", |s| s.bg(theme::text_secondary())),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if e.click_count >= 2 {
                        ed.reset_panel(panel);
                        cx.notify();
                        return;
                    }
                    let origin = if column {
                        f32::from(e.position.x)
                    } else {
                        f32::from(e.position.y)
                    };
                    ed.drag = Some(Drag::Splitter {
                        panel,
                        origin,
                        size,
                    });
                    cx.notify();
                }),
            )
    }

    fn on_root_mouse_move(
        &mut self,
        e: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pointer = e.position;
        let Some(drag) = &self.drag else {
            return;
        };
        if e.pressed_button != Some(MouseButton::Left) {
            // The release happened outside the window.
            self.end_drag(cx);
            return;
        }
        match drag {
            Drag::Splitter {
                panel,
                origin,
                size,
            } => {
                let (panel, origin, size) = (*panel, *origin, *size);
                self.resize_panel(panel, origin, size, e.position, window);
                cx.notify();
            }
            Drag::Scrub => self.scrub_to(e.position.x, cx),
            Drag::Clip(_) => self.update_clip_drag(e.position, cx),
            Drag::Marquee(_) => self.update_marquee(e.position, cx),
            Drag::Zoom => self.zoom_slider_to(e.position.x, cx),
            Drag::Slider { key, apply } => {
                let (key, apply) = (*key, apply.clone());
                if let Some(f) = self.slider_fraction(key, e.position.x) {
                    self.last_slider_fraction = Some(f);
                    apply(self, f, false, cx);
                }
            }
        }
    }

    pub fn slider_fraction(&self, key: &str, x: Pixels) -> Option<f32> {
        let b = self.measured(key)?;
        let w = f32::from(b.size.width);
        (w > 0.).then(|| ((f32::from(x - b.origin.x)) / w).clamp(0., 1.))
    }

    /// Starts a slider drag at `x` (its bounds measured under `key`).
    pub fn start_slider(
        &mut self,
        key: &'static str,
        x: Pixels,
        apply: SliderFn,
        cx: &mut Context<Self>,
    ) {
        self.last_slider_fraction = self.slider_fraction(key, x);
        if let Some(f) = self.last_slider_fraction {
            apply(self, f, false, cx);
        }
        self.drag = Some(Drag::Slider { key, apply });
        cx.notify();
    }

    pub fn zoom_slider_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        if let Some(f) = self.slider_fraction("zoom", x) {
            self.set_zoom(MIN_PPS * (MAX_PPS / MIN_PPS).powf(f), cx);
        }
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        match self.drag.take() {
            Some(Drag::Splitter { .. }) => self.prefs.save(),
            Some(Drag::Slider { apply, .. }) => {
                if let Some(f) = self.last_slider_fraction.take() {
                    apply(self, f, true, cx);
                }
                self.live_value = None;
            }
            Some(Drag::Clip(d)) => self.commit_clip_drag(*d, cx),
            _ => {}
        }
        cx.notify();
    }
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
        self.tick_edge_scroll(window, cx);
        // A drag over the timeline that ended elsewhere leaves no ghost.
        if self.media_drop.is_some() && !cx.has_active_drag() {
            self.media_drop = None;
        }

        let sizes = self.panel_sizes(window.viewport_size());
        let resizing = match &self.drag {
            Some(Drag::Splitter {
                panel: Panel::Timeline,
                ..
            }) => Some(CursorStyle::ResizeUpDown),
            Some(Drag::Splitter { .. }) => Some(CursorStyle::ResizeLeftRight),
            _ => None,
        };

        let mut root = div()
            .key_context("Editor")
            .track_focus(&self.focus)
            // Clicks anywhere keep the shortcuts live.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|ed, _, window, _| window.focus(&ed.focus)),
            )
            .on_mouse_move(cx.listener(Self::on_root_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|ed, _, _, cx| ed.end_drag(cx)),
            )
            .on_action(cx.listener(Self::import))
            .on_action(cx.listener(Self::open_project))
            .on_action(cx.listener(Self::save_project))
            .on_action(cx.listener(Self::save_project_as))
            .on_drop(cx.listener(|ed, paths: &ExternalPaths, _, cx| {
                let paths = paths
                    .paths()
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                if ed.screen == Screen::Home {
                    ed.new_project_with(paths, cx);
                } else {
                    ed.open_paths(paths, cx);
                }
            }))
            // Quitting saves first.
            .on_action(cx.listener(|ed, _: &Quit, _, cx| {
                ed.save_now(cx);
                cx.quit();
            }))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::app_bg())
            .font_family(theme::FONT_BODY)
            .text_color(theme::text())
            .text_size(px(theme::MD))
            .when_some(resizing, |d, c| d.cursor(c));

        macro_rules! bind {
            ($($id:ident),*) => {
                $(root = root.on_action(cx.listener(|ed, _: &shortcuts::$id, w, cx| ed.run(Shortcut::$id, w, cx)));)*
            };
        }
        bind!(
            PlayPause,
            PrevFrame,
            NextFrame,
            Back1s,
            Forward1s,
            PrevEdit,
            NextEdit,
            GoStart,
            GoEnd,
            SplitClip,
            DeleteLeft,
            DeleteRight,
            Deactivate,
            DeleteClip,
            Duplicate,
            CopyClip,
            CutClip,
            PasteClip,
            Undo,
            Redo,
            Deselect,
            ZoomIn,
            ZoomOut,
            ZoomFit,
            PreviewAxis,
            Snapping,
            MainMagnet,
            SelectTool,
            SplitTool,
            Fullscreen,
            Export,
            ShowShortcuts
        );

        if self.screen == Screen::Home {
            root = root.child(self.render_home(window, cx));
        } else if self.fullscreen {
            root = root.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .p(px(GUTTER))
                    .child(self.render_player(cx)),
            );
        } else {
            let timeline = self.render_timeline(cx).into_any_element();
            root = root
                .child(self.render_header(window, cx))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .px(px(GUTTER))
                        .pt(px(GUTTER))
                        .child(
                            panel_box()
                                .w(px(sizes.browser))
                                .child(self.render_browser(cx)),
                        )
                        .child(self.splitter(Panel::Browser, sizes.browser, cx))
                        .child(self.render_player(cx))
                        .child(self.splitter(Panel::Inspector, sizes.inspector, cx))
                        .child(
                            panel_box()
                                .w(px(sizes.inspector))
                                .bg(theme::sheet())
                                .child(self.render_inspector(cx)),
                        ),
                )
                .child(self.splitter(Panel::Timeline, sizes.timeline, cx))
                .child(
                    panel_box()
                        .flex_col()
                        .h(px(sizes.timeline))
                        .mx(px(GUTTER))
                        .mb(px(GUTTER))
                        .child(self.render_toolbar(cx))
                        .child(timeline),
                );
        }
        root.children(self.render_overlays(window.viewport_size(), cx))
    }
}

/// A rounded panel with a hairline border (browser, player, inspector,
/// timeline).
pub fn panel_box() -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .min_h_0()
        .overflow_hidden()
        .rounded(px(8.))
        .border_1()
        .border_color(theme::card_border())
        .bg(theme::panel())
}

impl Editor {
    /// The toast, the open popup menu and the Shortcuts dialog, over
    /// everything else.
    fn render_overlays(
        &mut self,
        viewport: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = vec![];
        if let Some(popup) = &self.popup {
            out.push(crate::ui::dialogs::popup_menu(popup, viewport, cx).into_any_element());
        }
        if self.shortcuts_open {
            out.push(crate::ui::dialogs::shortcuts_dialog(cx).into_any_element());
        }
        if let Some(confirm) = &self.confirm {
            out.push(crate::ui::dialogs::confirm_dialog(confirm, cx).into_any_element());
        }
        if let Some(msg) = &self.toast {
            out.push(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(110.))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .px(px(14.))
                            .py(px(9.))
                            .rounded(px(8.))
                            .bg(theme::toast_bg())
                            .border_1()
                            .border_color(gpui::rgba(0xffffff1f))
                            .text_size(px(theme::SM))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::text())
                            .child(msg.clone()),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    pub fn open_popup(&mut self, at: Point<Pixels>, items: Vec<PopupItem>, cx: &mut Context<Self>) {
        self.popup = Some(Popup { at, items });
        cx.notify();
    }
}

// ---- timeline geometry (shared with ui::timeline) ---------------------------

impl Editor {
    pub fn x_of(&self, t: Micros) -> f32 {
        t as f32 / SECOND as f32 * self.pps - self.scroll_x
    }

    pub fn time_at(&self, window_x: Pixels) -> Micros {
        let left = self.measured("lanes").map_or(0., |b| f32::from(b.origin.x));
        let x = f32::from(window_x) - left + self.scroll_x;
        ((x / self.pps) * SECOND as f32) as Micros
    }
}

pub fn timecode(t: Micros, fps: f64) -> String {
    // HH:MM:SS:FF, frame-accurate (whole frames first).
    let fps_i = fps.round().max(1.) as i64;
    let frames = (t.max(0) as f64 * fps / SECOND as f64).floor() as i64;
    let (secs, f) = (frames / fps_i, frames % fps_i);
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60,
        f
    )
}
