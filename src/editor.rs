//! The desktop editor window: header, then browser | player | inspector over
//! the timeline toolbar + timeline, with draggable gutters between panels.
//! This file holds the editor's state, its edits and the window layout;
//! each panel renders from `src/ui/*`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, Context, CursorStyle, ExternalPaths, FocusHandle, FontWeight, MouseButton,
    MouseDownEvent, MouseMoveEvent, PathPromptOptions, Pixels, Point, RenderImage, SharedString,
    Size, Window, actions, canvas, div, prelude::*, px,
};

use crate::assets::{self, Picture, PictureJob, PictureServer};
use crate::audio::Audio;
use crate::catalog::EffectCatalog;
use crate::decoder::{self, DecodedFrame, FrameServer, TileServer, Waveform};
use crate::effects::package::{self as fx_package, Fetched, Packages};
use crate::effects::stack::{self as fx_stack, LiveParam, Needs, Sources, Stack};
use crate::effects::{self, Rendered};
use crate::engine::{self, Engine, LayerKind};
use crate::history::History;
use crate::model::{self, Clip, MediaItem, Micros, Project, SECOND, Side, TrackType};
use crate::prefs::Prefs;
use crate::projects;
use crate::selection::Selection;
use crate::shortcuts::{self, Shortcut};
use crate::stickers::{self, StickerFrames};
use crate::text_render::{self, RasterCache, TextStyle};
use crate::theme;
use crate::ui::library::LibraryUi;
use crate::ui::text_input::{InputEvent, TextInput};
use crate::ui::timeline_edit::{ClipDrag, KeyframeDrag, Marquee, MediaDrop};

actions!(
    editor,
    [
        ImportMedia,
        OpenProject,
        SaveProject,
        SaveProjectAs,
        Quit,
        OpenSettings,
        CheckForUpdates
    ]
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

/// A number field dragged sideways: gets the px moved since the press, and
/// `true` on release.
pub type NudgeFn = Rc<dyn Fn(&mut Editor, f32, bool, &mut Context<Editor>)>;

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
    /// A number field scrubbed sideways from `origin` (window x).
    Nudge {
        origin: f32,
        apply: NudgeFn,
    },
    /// A timeline clip moved or trimmed (`ui/timeline_edit.rs`).
    Clip(Box<ClipDrag>),
    /// A rubber band over the lanes (⇧ / ⌘ + drag on empty space).
    Marquee(Box<Marquee>),
    /// A keyframe diamond on the selected clip: a click seeks, a drag moves.
    Keyframe(Box<KeyframeDrag>),
    /// A layer moved, or scaled and turned, on the player (`ui/gizmo.rs`).
    Gizmo(Box<crate::ui::gizmo::GizmoDrag>),
    /// The Crop dialog's box moved or one of its handles (`ui/crop.rs`).
    Crop(Box<crate::ui::crop::CropDrag>),
    /// A tile held in the Layers panel (`ui/layers.rs`).
    Layer(Box<crate::ui::layers::LayerDrag>),
    /// A beat held on the Speed › Curve graph (`ui/speed_curve.rs`).
    Beat(Box<crate::ui::speed_curve::BeatDrag>),
    /// A colour row's custom-colour field (`ui/color_row.rs`).
    Spectrum(crate::ui::color_row::ColorFn),
}

/// A picture the player shows: its clip, and the frame server decoding it.
pub struct WantedLayer {
    pub key: String,
    pub source: String,
    pub clip_id: String,
    pub kind: LayerKind,
    /// A text layer's picture: its pixels per canvas px.
    pub raster_px: f32,
    /// A text layer's words and background, px of its picture (the
    /// on-canvas selection box; the picture also holds glow and shadow).
    pub content: Option<[f32; 2]>,
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

/// The home sidebar's pages: Edit (start a video, the newest drafts) and
/// Projects (every draft).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HomeTab {
    Edit,
    Projects,
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
    /// Where the playhead sat in the lanes last frame while playing
    /// (`None` when stopped): the timeline pages when it crosses the edge.
    pub(crate) play_follow_x: Option<f32>,

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
    pub(crate) home_tab: HomeTab,
    /// The drafts picked for deletion, while the list is in select mode.
    pub(crate) home_selection: Option<std::collections::BTreeSet<String>>,
    /// The list's scroll (the New video card shrinks as it scrolls).
    pub(crate) home_scroll: gpui::ScrollHandle,
    /// A question over everything (delete a project?).
    pub(crate) confirm: Option<Confirm>,
    /// The export screen, while it's up.
    pub(crate) export: Option<crate::ui::export::ExportView>,
    /// Counts exports (the screen drops updates of one it replaced).
    pub(crate) export_seq: u64,
    /// The export's frame rate (None: the project's own, snapped).
    pub(crate) export_fps: Option<u32>,
    autosave_token: u64,

    pub(crate) fullscreen: bool,
    pub(crate) shortcuts_open: bool,
    /// Settings, and the app updater it shows.
    pub(crate) settings_open: bool,
    pub(crate) updater: gpui::Entity<crate::updater::Updater>,
    /// The update dialog, opened from the menu's Check for Updates… (it
    /// also opens by itself when a check finds a new version).
    pub(crate) update_dialog_open: bool,
    /// The version (and whether it was downloaded) whose dialog got
    /// "Later": it stays closed until that changes.
    pub(crate) update_hidden: Option<(SharedString, bool)>,
    pub(crate) popup: Option<Popup>,
    pub(crate) drag: Option<Drag>,
    /// A slider's value while it is dragged (committed on release).
    pub(crate) live_value: Option<(&'static str, f64)>,
    /// Where the dragged slider was last (0–1); its release commits this.
    last_slider_fraction: Option<f32>,
    /// How far (px) the scrubbed number field has moved.
    nudge_dx: f32,
    /// A colour row's eyedropper waiting for a click on the player, the
    /// open custom-colour field, and each row's swatch scroll.
    pub(crate) color_pick: Option<crate::ui::color_row::ColorPick>,
    pub(crate) spectrum: Option<crate::ui::color_row::Spectrum>,
    pub(crate) color_scrolls: RefCell<HashMap<&'static str, gpui::ScrollHandle>>,
    /// Set while a held control edits continuously: its edits join one
    /// undo step.
    pub(crate) coalesce_edits: Option<String>,
    /// Background's Color / Image / Blur page, per clip (else the page of
    /// the clip's own background).
    pub(crate) background_page: Option<(String, crate::ui::visual::BgPage)>,
    pub(crate) toast: Option<SharedString>,
    toast_token: u64,
    /// The browser's tab, and each tab's chosen view.
    pub(crate) browser_tab: usize,
    pub(crate) browser_views: [usize; 8],
    /// Media tile pictures, by file path.
    pub(crate) thumbs: HashMap<String, Arc<RenderImage>>,
    /// The home rows' copies of `thumbs`, cropped to the row's tile so its
    /// rounded corners show (made on first draw).
    pub(crate) home_thumbs: RefCell<HashMap<String, Arc<RenderImage>>>,
    /// The category shown in each browser tab with categories (Effects,
    /// Filter).
    pub(crate) browser_categories: [usize; 8],
    /// The Adjust sliders over the browser's filter grid.
    pub(crate) fx_adjust_open: bool,
    /// The served video effects (tabs fetched as they're shown).
    pub(crate) effects: EffectCatalog,
    /// Effect / filter rows asked for by name (a project's), once each.
    effects_named: HashSet<String>,
    /// Effect / filter tile pictures: first frames by key (None: failed),
    /// the few animations played on hover, and what's been asked for.
    pub(crate) stills: HashMap<String, Option<Arc<RenderImage>>>,
    pub(crate) anims: Vec<(String, Arc<RenderImage>)>,
    pub(crate) pictures_pending: RefCell<HashSet<PictureJob>>,
    pub(crate) picture_server: PictureServer,
    /// Tile pictures by URL (stickers, album covers).
    remote_pictures: PictureServer,
    /// The browser's Audio / Text / Sticker views.
    pub(crate) lib: LibraryUi,
    /// The AI agent (its panel takes the inspector's place) and its fields.
    pub(crate) agent: crate::agent::Agent,
    pub(crate) agent_ui: crate::ui::agent::AgentUi,
    /// Sound out (None: no output device).
    pub(crate) audio: Option<Audio>,
    /// When the sound was last (re)started with the picture.
    audio_synced_at: Instant,
    /// The inspector's field for a text clip's words, the clip it shows,
    /// and a request to give it the keyboard (a new text clip).
    pub(crate) text_field: gpui::Entity<TextInput>,
    text_field_clip: Option<String>,
    pub(crate) focus_text_field: bool,
    /// The effect / filter tile under the pointer (its animation plays).
    pub(crate) fx_hover: Option<String>,
    /// The effect grid's scroll (more rows load near the end).
    pub(crate) fx_scroll: gpui::ScrollHandle,
    /// The inspector's open sub-tab (entry key) for the selected clip.
    pub(crate) inspector_entry: Option<&'static str>,
    /// Arrange layers, open in the inspector over the clip's sub-tab.
    pub(crate) layers_panel: Option<crate::ui::layers::LayersPanel>,
    /// The Crop dialog, while it's open.
    pub(crate) crop: Option<crate::ui::crop::CropSession>,
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
    /// The pictures the player shows now, back to front.
    wanted: Vec<WantedLayer>,
    /// The source time last asked of each frame server.
    requested: HashMap<String, Micros>,
    /// The last frame each wanted layer's server decoded, as it came (it's
    /// composed again when the time or the document changes), and a count
    /// of frames decoded.
    layer_frames: HashMap<String, Arc<DecodedFrame>>,
    /// Background photos, decoded once (None: on its way, or it failed).
    backdrop_images: HashMap<String, Option<Arc<DecodedFrame>>>,
    /// Text clips drawn as pictures, by look.
    text_rasters: RasterCache,
    /// Sticker / GIF clips' frames by URL (None: loading, or it failed).
    pub(crate) stickers: HashMap<String, Option<Arc<StickerFrames>>>,
    /// Stickers the preview wanted but nobody has loaded yet.
    stickers_wanted: Vec<String>,
    /// A held text slider: (clip, field, value) shown before release.
    pub(crate) live_text: Option<(String, &'static str, serde_json::Value)>,
    /// A layer being moved / scaled / turned on the player: its transform
    /// so far (shown before the edit lands on release), and what it
    /// snapped to (the guide lines).
    pub(crate) live_xform: Option<(String, crate::gizmo::Xform)>,
    pub(crate) gizmo_snap: crate::gizmo::Snapped,
    /// What the pointer is over on the player (its cursor).
    pub(crate) gizmo_hover: Option<crate::ui::gizmo::GizmoHover>,
    frame_seq: u64,
    /// Effect / filter shaders downloaded this session.
    pub(crate) packages: Packages,
    /// A held effect slider: the preview follows it before the edit lands.
    pub(crate) live_fx: Option<LiveParam>,
    fx_worker: effects::Worker,
    /// The last (frame, stack) sent to be drawn, the newest job number,
    /// and the job on screen (older results are dropped).
    fx_last: Option<(u64, Stack, bool)>,
    fx_seq: u64,
    fx_shown: u64,
    pub(crate) frame: Option<Arc<RenderImage>>,
    pub(crate) stale_frames: Vec<Arc<RenderImage>>,
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

        let (fx_tx, fx_rx) = async_channel::bounded::<Rendered>(2);
        cx.spawn(async move |this, cx| {
            while let Ok(r) = fx_rx.recv().await {
                if this.update(cx, |ed, cx| ed.on_rendered(r, cx)).is_err() {
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

        let (picture_tx, picture_rx) = async_channel::unbounded::<Picture>();
        cx.spawn(async move |this, cx| {
            while let Ok(p) = picture_rx.recv().await {
                if this.update(cx, |ed, cx| ed.on_picture(p, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        let pinch = crate::pinch::listen();
        cx.spawn(async move |this, cx| {
            while let Ok(m) = pinch.recv().await {
                if this.update(cx, |ed, cx| ed.on_pinch(m, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        let focus = cx.focus_handle();
        window.focus(&focus);
        let text_field = cx.new(|cx| TextInput::new("Enter text", true, cx).max_len(500));
        cx.subscribe(&text_field, |ed: &mut Editor, _, ev: &InputEvent, cx| {
            ed.on_text_field(ev, cx)
        })
        .detach();
        let updater = crate::updater::Updater::new(cx);
        cx.observe(&updater, |_, _, cx| cx.notify()).detach();
        let project = Project::empty();
        let mut engine = Engine::new();
        if let Err(e) = engine.load(&project) {
            eprintln!("engine refused the empty project: {e}");
        }
        let mut editor = Editor {
            project,
            engine,
            media: vec![],
            selected: Selection::default(),
            playhead: 0,
            pps: 80.,
            scroll_x: 0.,
            play_follow_x: None,
            history: History::default(),
            prefs: Prefs::load(),
            tool: Tool::Select,
            clipboard: None,
            dirty: false,
            screen: Screen::Home,
            home_projects: vec![],
            home_greeting: "",
            home_tab: HomeTab::Edit,
            home_selection: None,
            home_scroll: gpui::ScrollHandle::new(),
            confirm: None,
            export: None,
            export_seq: 0,
            export_fps: None,
            autosave_token: 0,
            fullscreen: false,
            shortcuts_open: false,
            settings_open: false,
            updater,
            update_dialog_open: false,
            update_hidden: None,
            popup: None,
            drag: None,
            live_value: None,
            nudge_dx: 0.,
            color_pick: None,
            spectrum: None,
            color_scrolls: RefCell::default(),
            coalesce_edits: None,
            background_page: None,
            last_slider_fraction: None,
            toast: None,
            toast_token: 0,
            browser_tab: 0,
            browser_views: [0; 8],
            thumbs: HashMap::new(),
            home_thumbs: RefCell::default(),
            browser_categories: [0; 8],
            fx_adjust_open: false,
            effects: EffectCatalog::load(),
            effects_named: HashSet::new(),
            stills: HashMap::new(),
            anims: vec![],
            pictures_pending: RefCell::default(),
            picture_server: PictureServer::new(assets::EFFECT_PREVIEWS, picture_tx.clone()),
            remote_pictures: PictureServer::new(assets::REMOTE, picture_tx),
            lib: LibraryUi::new(cx),
            agent: crate::agent::Agent::new(),
            agent_ui: crate::ui::agent::AgentUi::new(cx),
            audio: Audio::new(),
            audio_synced_at: Instant::now(),
            text_field,
            text_field_clip: None,
            focus_text_field: false,
            fx_hover: None,
            fx_scroll: gpui::ScrollHandle::new(),
            inspector_entry: None,
            layers_panel: None,
            crop: None,
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
            wanted: vec![],
            requested: HashMap::new(),
            layer_frames: HashMap::new(),
            backdrop_images: HashMap::new(),
            text_rasters: RasterCache::default(),
            stickers: HashMap::new(),
            live_xform: None,
            gizmo_snap: Default::default(),
            gizmo_hover: None,
            stickers_wanted: vec![],
            live_text: None,
            frame_seq: 0,
            packages: Packages::default(),
            live_fx: None,
            fx_worker: effects::Worker::spawn(fx_tx),
            fx_last: None,
            fx_seq: 0,
            fx_shown: 0,
            frame: None,
            stale_frames: vec![],
            frame_info: None,
            fps_window: (Instant::now(), 0),
            preview_fps: 0.,
            focus,
        };
        // The agent panel opens with the app: load its model meanwhile.
        if editor.agent.settings.open {
            editor.agent_warm_up(cx);
        }
        editor
    }

    // ---- small helpers ----------------------------------------------------

    /// Asks for a tile picture (first frame, or the whole animation)
    /// unless it's here or on its way.
    pub fn want_picture(&self, key: &str, animated: bool) {
        let job = PictureJob {
            key: key.to_string(),
            animated,
        };
        let have = if animated {
            self.anims.iter().any(|(k, _)| k == key)
        } else {
            self.stills.contains_key(key)
        };
        if have || !self.pictures_pending.borrow_mut().insert(job.clone()) {
            return;
        }
        if key.starts_with("https://") || key.starts_with("http://") {
            self.remote_pictures.request(job);
        } else {
            self.picture_server.request(job);
        }
    }

    fn on_picture(&mut self, p: Picture, cx: &mut Context<Self>) {
        self.pictures_pending.borrow_mut().remove(&p.job);
        if p.job.animated {
            // A handful of animations at a time (a few MB each).
            const KEEP: usize = 6;
            if let Some(image) = p.image {
                self.anims.retain(|(k, _)| *k != p.job.key);
                self.anims.push((p.job.key, image));
                while self.anims.len() > KEEP {
                    let (_, old) = self.anims.remove(0);
                    self.stale_frames.push(old);
                }
            }
        } else {
            self.stills.insert(p.job.key, p.image);
        }
        cx.notify();
    }

    /// The hovered tile's animation, if it's decoded.
    pub fn anim(&self, key: &str) -> Option<Arc<RenderImage>> {
        self.anims
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, i)| i.clone())
    }

    /// Fetches the next page of an Effects tab off the UI thread.
    pub fn load_effect_page(&mut self, tab: &'static str, cx: &mut Context<Self>) {
        let Some(offset) = self.effects.begin_page(tab) else {
            return;
        };
        let task = cx
            .background_executor()
            .spawn(async move { crate::catalog::fetch_page(tab, offset) });
        cx.spawn(async move |this, cx| {
            let rows = task.await;
            this.update(cx, |ed, cx| {
                ed.effects.end_page(tab, offset, rows);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Effects in the project this catalog hasn't seen (their sliders).
    fn load_named_effects(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        let names: Vec<String> = names
            .into_iter()
            .filter(|n| self.effects.get(n).is_none() && self.effects_named.insert(n.clone()))
            .collect();
        if names.is_empty() {
            return;
        }
        let task = cx
            .background_executor()
            .spawn(async move { crate::catalog::fetch_named(&names) });
        cx.spawn(async move |this, cx| {
            if let Ok(rows) = task.await {
                this.update(cx, |ed, cx| {
                    ed.effects.remember(&rows);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Before drawing: the effect lists on screen get their first page, or
    /// the next one when scrolled near their end; a selected effect clip's
    /// entry is fetched for its sliders.
    fn prefetch_effects(&mut self, cx: &mut Context<Self>) {
        use crate::catalog::EFFECT_TABS;
        use crate::ui::browser::EFFECTS_TAB;
        let mut shown: Vec<(&'static str, gpui::ScrollHandle)> = vec![];
        if self.screen == Screen::Editor
            && self.browser_tab == EFFECTS_TAB
            && self.browser_views[EFFECTS_TAB] == 0
        {
            let i = self.browser_categories[EFFECTS_TAB].min(EFFECT_TABS.len() - 1);
            shown.push((EFFECT_TABS[i].0, self.fx_scroll.clone()));
        }
        let selected_effect = self
            .selected_clip()
            .filter(|(_, c)| c.kind == "effect")
            .map(|(_, c)| {
                c.effects()
                    .into_iter()
                    .map(|e| e.name().to_string())
                    .collect::<Vec<_>>()
            });
        if let Some(names) = selected_effect {
            self.load_named_effects(names, cx);
        }
        for (tab, scroll) in shown {
            let near_end = {
                let max = f32::from(scroll.max_offset().height);
                let at = -f32::from(scroll.offset().y);
                max - at < 400.
            };
            match self.effects.page(tab) {
                None => self.load_effect_page(tab, cx),
                Some(p) if !p.done && !p.loading && near_end => self.load_effect_page(tab, cx),
                _ => {}
            }
        }
    }

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
        // A sticker / GIF by URL: its first frame, once it's loaded.
        if path.starts_with("http://") || path.starts_with("https://") {
            self.want_sticker(path, cx);
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
        self.forget_layers();
        if let Some(old) = self.frame.take() {
            self.stale_frames.push(old);
        }
        self.frame_info = None;
        self.media.clear();
        let sources: Vec<String> = project
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            // Stickers are online files, not library media.
            .filter(|c| c.kind != "sticker")
            .filter_map(|c| c.source.clone())
            .filter(|s| !s.is_empty() && !s.starts_with("http://") && !s.starts_with("https://"))
            .collect();
        for src in sources {
            if let Ok(item) = decoder::probe(&src) {
                self.add_to_library(item, cx);
            }
        }
        self.project = project;
        self.history.clear();
        // A different project: the agent's chat and ids start over.
        self.agent.reset();
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

    /// New video: an empty project straight into the editor. It's written
    /// only once something is edited, so backing out leaves no empty draft.
    pub fn new_blank_project(&mut self, cx: &mut Context<Self>) {
        let mut project = Project::empty();
        project.created_at = projects::now_iso();
        project.updated_at = project.created_at.clone();
        self.home_selection = None;
        self.start_session(project, cx);
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
        self.forget_layers();
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

    /// Deletes these drafts from the projects folder (ends select mode).
    pub fn delete_saved_projects(&mut self, ids: &[String], cx: &mut Context<Self>) {
        let mut deleted = 0;
        let mut failed = None;
        for id in ids {
            match projects::delete(id) {
                Ok(()) => deleted += 1,
                Err(e) => failed = Some(e),
            }
        }
        self.home_selection = None;
        self.refresh_home(cx);
        match failed {
            Some(e) => self.toast(format!("Couldn't delete it: {e}"), cx),
            None => self.toast(
                format!(
                    "Deleted {deleted} project{}",
                    if deleted == 1 { "" } else { "s" }
                ),
                cx,
            ),
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
        let dir = crate::prefs::home_dir();
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
        // The export screen takes nothing but Esc (close it).
        if self.export.is_some() {
            if id == Shortcut::Deselect {
                self.close_export(cx);
            }
            return;
        }
        if self.update_dialog_shown(cx) {
            if id == Shortcut::Deselect {
                self.update_later(cx);
            }
            return;
        }
        if self.settings_open {
            if id == Shortcut::Deselect {
                self.settings_open = false;
                cx.notify();
            }
            return;
        }
        if self.screen == Screen::Home && id != Shortcut::ShowShortcuts {
            return;
        }
        if self.crop.is_some() {
            if id == Shortcut::Deselect {
                self.cancel_crop(cx);
            }
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
            |p| p.set_speed(&id, speed),
            cx,
        );
    }

    /// Speed › Curve: a ramp on the clip (None: back to 1×), re-timing it;
    /// on the main lane the clips after it close up. One undo step each.
    pub fn set_speed_curve(
        &mut self,
        clip_id: &str,
        curve: Option<crate::speed_curve::SpeedCurve>,
        cx: &mut Context<Self>,
    ) {
        let id = clip_id.to_string();
        let main = self.on_main_lane(&id);
        self.edit(
            "Speed Curve",
            main,
            None,
            |p| p.retime_clip(&id, 1., curve),
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

    /// Keeps the sound with the picture: playing from the engine's time
    /// while it plays (restarted if they drift apart), quiet otherwise.
    fn sync_audio(&mut self) {
        let playing = self.engine.is_playing();
        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        if playing {
            let drifted = audio
                .position()
                .is_some_and(|p| (p - self.playhead).abs() > 120_000);
            let settled = self.audio_synced_at.elapsed() > Duration::from_millis(600);
            if !audio.is_playing() || (drifted && settled) {
                audio.play(Arc::new(self.project.clone()), self.playhead);
                self.audio_synced_at = Instant::now();
            }
        } else if audio.is_playing() {
            audio.stop();
        }
    }

    /// The inspector's text field follows the selected text clip (its
    /// words, unless it's being typed in); a new clip's field takes the
    /// keyboard with its words selected.
    fn sync_text_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let clip = self
            .selected_clip()
            .filter(|(_, c)| c.kind == "text")
            .map(|(_, c)| (c.id.clone(), c.text.clone().unwrap_or_default()));
        let Some((id, text)) = clip else {
            self.text_field_clip = None;
            return;
        };
        let focused = self.text_field.read(cx).is_focused(window);
        let other = self.text_field_clip.as_deref() != Some(id.as_str());
        if other || !focused {
            self.text_field.update(cx, |f, cx| f.set_text(&text, cx));
        }
        self.text_field_clip = Some(id);
        if std::mem::take(&mut self.focus_text_field) {
            self.text_field.update(cx, |f, cx| {
                f.focus(window);
                f.select_everything(cx);
            });
        }
    }

    /// The text field's typing: the selected text clip's words (a quick
    /// run of typing is one undo step); Esc gives the keyboard back.
    fn on_text_field(&mut self, ev: &InputEvent, cx: &mut Context<Self>) {
        match ev {
            InputEvent::Changed(text) => {
                let Some(id) = self.text_field_clip.clone() else {
                    return;
                };
                let text = text.clone();
                let key = format!("text:{id}");
                self.edit(
                    "Edit Text",
                    false,
                    Some(key),
                    |p| {
                        p.update_clip(&id, |c| {
                            if c.text.as_deref() == Some(text.as_str()) {
                                return false;
                            }
                            c.text = Some(text);
                            true
                        })
                    },
                    cx,
                );
            }
            InputEvent::Submit(_) => {}
            InputEvent::Blur => cx.notify(),
        }
    }

    /// Drops every picture the player was showing.
    fn forget_layers(&mut self) {
        self.wanted.clear();
        self.requested.clear();
        self.layer_frames.clear();
    }

    /// Asks for every visible layer's picture at the preview time: the
    /// decoders of videos and photos (each layer its own, keyed by file and
    /// its turn among the layers of that file, so two layers of one file
    /// don't fight over one decoder), text drawn from its look, a sticker's
    /// frame (loaded first: until then it's left out).
    pub(crate) fn request_preview(&mut self) {
        let layers = engine::picture_layers(
            &self.project,
            self.engine.active_layers(self.preview_time()),
        );
        let mut wanted = Vec::with_capacity(layers.len());
        for layer in layers {
            match layer.kind {
                LayerKind::Text => {
                    let Some((key, frame, raster_px, content)) =
                        self.text_layer(&layer.clip_id, layer.offset)
                    else {
                        continue; // nothing to write yet
                    };
                    self.set_layer_frame(&key, frame);
                    wanted.push(WantedLayer {
                        key,
                        source: String::new(),
                        clip_id: layer.clip_id,
                        kind: LayerKind::Text,
                        raster_px,
                        content: Some(content),
                    });
                }
                LayerKind::Sticker => {
                    let frames = match self.stickers.get(&layer.source) {
                        Some(Some(f)) => f.clone(),
                        Some(None) => continue, // loading, or it failed
                        None => {
                            if !self.stickers_wanted.contains(&layer.source) {
                                self.stickers_wanted.push(layer.source.clone());
                            }
                            continue;
                        }
                    };
                    let key = format!("sticker:{}", layer.clip_id);
                    self.set_layer_frame(&key, frames.at(layer.offset));
                    wanted.push(WantedLayer {
                        key,
                        source: layer.source,
                        clip_id: layer.clip_id,
                        kind: LayerKind::Sticker,
                        raster_px: 1.,
                        content: None,
                    });
                }
                _ => {
                    let turn = wanted
                        .iter()
                        .filter(|w: &&WantedLayer| w.source == layer.source)
                        .count();
                    let key = format!("{}#{turn}", layer.source);
                    if self.requested.get(&key) != Some(&layer.source_time) {
                        let server = self.servers.entry(key.clone()).or_insert_with(|| {
                            FrameServer::spawn(
                                layer.source.clone(),
                                key.clone(),
                                PREVIEW_MAX_WIDTH,
                                self.frame_tx.clone(),
                            )
                        });
                        server.request(layer.source_time);
                        self.requested.insert(key.clone(), layer.source_time);
                    }
                    wanted.push(WantedLayer {
                        key,
                        source: layer.source,
                        clip_id: layer.clip_id,
                        kind: layer.kind,
                        raster_px: 1.,
                        content: None,
                    });
                }
            }
        }
        if wanted.is_empty() {
            self.forget_layers();
            if let Some(old) = self.frame.take() {
                self.stale_frames.push(old);
            }
            self.frame_info = None;
            return;
        }
        // Layers gone from view: their frames and decoders go too.
        let keys: HashSet<&str> = wanted.iter().map(|w| w.key.as_str()).collect();
        self.layer_frames.retain(|k, _| keys.contains(k.as_str()));
        self.requested.retain(|k, _| keys.contains(k.as_str()));
        self.servers.retain(|k, _| keys.contains(k.as_str()));
        self.wanted = wanted;
    }

    /// A picture made here (text, a sticker frame) as a layer's frame; a
    /// new one is composed on the next render.
    fn set_layer_frame(&mut self, key: &str, frame: Arc<DecodedFrame>) {
        if self
            .layer_frames
            .get(key)
            .is_some_and(|f| Arc::ptr_eq(f, &frame))
        {
            return;
        }
        self.layer_frames.insert(key.to_string(), frame);
        self.frame_seq += 1;
    }

    /// A text clip drawn `offset` into it: its layer key, picture and
    /// pixels per canvas px. Drawn at the preview's canvas size times its
    /// zoom (in quarter octaves), so it stays sharp when scaled up; the
    /// compositor scales the rest.
    fn text_layer(
        &mut self,
        clip_id: &str,
        offset: Micros,
    ) -> Option<(String, Arc<DecodedFrame>, f32, [f32; 2])> {
        let (_, clip) = self.project.find_clip(clip_id)?;
        let mut style = TextStyle::from_clip(clip);
        if let Some((id, field, value)) = &self.live_text
            && id == clip_id
        {
            let mut patched = clip.clone();
            patched.extra.insert((*field).to_string(), value.clone());
            style = TextStyle::from_clip(&patched);
        }
        let zoom = self
            .property_at(clip, "transform.scale", offset)
            .unwrap_or(1.) as f32;
        let canvas = (
            self.project.canvas.width.max(1),
            self.project.canvas.height.max(1),
        );
        let fx_scale = self.fx_canvas_px().0 as f32 / canvas.0 as f32;
        let raster_px = text_render::raster_px(zoom, fx_scale);
        let px_per_unit = text_render::unit_px(canvas.0, canvas.1) * raster_px;
        let (frame, content) = self.text_rasters.get_or_draw(&style, canvas, px_per_unit)?;
        Some((format!("text:{clip_id}"), frame, raster_px, content))
    }

    /// A text / sticker layer's own size at zoom 1 (canvas widths,
    /// heights) from its picture `f`; None for videos and photos, which
    /// are contain-fitted.
    pub(crate) fn layer_natural(
        &self,
        w: &WantedLayer,
        c: &Clip,
        f: &DecodedFrame,
    ) -> Option<[f32; 2]> {
        fx_stack::natural_size(
            c,
            (f.width, f.height),
            w.raster_px,
            (self.project.canvas.width, self.project.canvas.height),
        )
    }

    /// The pictures the player shows now (back to front) with their last
    /// frame, for the on-canvas gizmo.
    pub(crate) fn shown_layers(&self) -> impl Iterator<Item = (&WantedLayer, &Arc<DecodedFrame>)> {
        self.wanted
            .iter()
            .filter_map(|w| Some((w, self.layer_frames.get(&w.key)?)))
    }

    /// Starts loading the stickers the preview asked for.
    fn load_stickers(&mut self, cx: &mut Context<Self>) {
        for url in std::mem::take(&mut self.stickers_wanted) {
            self.want_sticker(url, cx);
        }
    }

    /// Downloads and decodes a sticker unless it's here or on its way;
    /// its first frame becomes its timeline picture.
    pub(crate) fn want_sticker(&mut self, url: String, cx: &mut Context<Self>) {
        if self.stickers.contains_key(&url) {
            return;
        }
        self.stickers.insert(url.clone(), None);
        let u = url.clone();
        let task = cx
            .background_executor()
            .spawn(async move { stickers::load(&u) });
        cx.spawn(async move |this, cx| {
            let loaded = task.await;
            this.update(cx, |ed, cx| {
                match loaded {
                    Ok(frames) => {
                        let (w, h, bgra) = frames.thumb.clone();
                        if let Some(buffer) = image::RgbaImage::from_raw(w, h, bgra) {
                            let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
                            ed.thumbs.insert(url.clone(), image);
                        }
                        ed.stickers.insert(url, Some(Arc::new(frames)));
                    }
                    Err(e) => eprintln!("sticker {url}: {e:#}"),
                }
                ed.request_preview();
                cx.notify();
            })
            .ok();
        })
        .detach();
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
        let Some(top) = self.wanted.last() else {
            return;
        };
        if !self.wanted.iter().any(|w| w.key == f.key) {
            return; // a cut moved on to another file meanwhile
        }
        if top.key == f.key {
            self.frame_info = Some((f.width, f.height, f.pts));
            let (since, n) = &mut self.fps_window;
            *n += 1;
            let elapsed = since.elapsed().as_secs_f32();
            if elapsed >= 1. {
                self.preview_fps = *n as f32 / elapsed;
                self.fps_window = (Instant::now(), 0);
            }
        }
        self.frame_seq += 1;
        // Composed on the next render, with the looks and places that apply.
        self.layer_frames.insert(f.key.clone(), Arc::new(f));
        cx.notify();
    }

    // ---- effects + filters in the preview ---------------------------------

    /// The size the canvas is drawn at under global effects: the project's
    /// shape, at most the preview's decode width on its long side.
    fn fx_canvas_px(&self) -> (u32, u32) {
        let (w, h) = (
            self.project.canvas.width.max(2) as f64,
            self.project.canvas.height.max(2) as f64,
        );
        let scale = (PREVIEW_MAX_WIDTH as f64 / w.max(h)).min(1.);
        let even = |n: f64| ((n * scale / 2.).round() as u32 * 2).max(2);
        (even(w), even(h))
    }

    /// Sends the frame on screen through the looks that apply at the
    /// preview time (when that changed), or shows it as it is when none
    /// do. Fetches what's missing (catalog rows, packages); it's drawn
    /// again when they land.
    fn refresh_picture(&mut self, cx: &mut Context<Self>) {
        // Every visible picture needs a frame first (else the last
        // composition stays up, rather than one missing a layer).
        let Some(frames) = self
            .wanted
            .iter()
            .map(|w| self.layer_frames.get(&w.key).cloned())
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        if frames.is_empty() {
            return;
        }
        let t = self.preview_time();
        let mut needs = Needs::default();
        let stack = {
            let clips: Vec<(&Clip, fx_stack::Placement)> = self
                .wanted
                .iter()
                .zip(&frames)
                .filter_map(|(w, f)| {
                    let (_, c) = self.project.find_clip(&w.clip_id)?;
                    let mut place = self.layer_placement(c, t);
                    place.natural = self.layer_natural(w, c, f);
                    Some((c, place))
                })
                .collect();
            if clips.len() != frames.len() {
                return; // the document moved on; the next request catches up
            }
            let effects = &self.effects;
            let effect_key = |name: &str| {
                effects
                    .get(name)
                    .map(|e| e.package_key.clone())
                    .filter(|k| !k.is_empty())
            };
            let src = Sources {
                packages: &self.packages,
                effect_key: &effect_key,
                filter_matrix: &engine::filter_color_matrix,
                adjust_matrix: &|c: &Clip| self.adjust_matrix_at(c, t),
                live: self.live_fx.as_ref(),
            };
            fx_stack::build(
                &self.project,
                &clips,
                t,
                self.fx_canvas_px(),
                &src,
                &mut needs,
            )
        };
        self.load_named_effects(needs.rows, cx);
        for key in needs.effect_packages {
            self.fetch_package(key, false, cx);
        }
        for key in needs.filter_packages {
            self.fetch_package(key, true, cx);
        }
        let backdrop = match stack.canvas.as_ref().and_then(|c| c.backdrop.as_ref()) {
            Some(fx_stack::Backdrop::Image(path)) => self.backdrop_image(path, cx),
            _ => None,
        };

        if self.fx_last.as_ref().is_some_and(|(seq, last, had)| {
            *seq == self.frame_seq && *last == stack && *had == backdrop.is_some()
        }) {
            return;
        }
        self.fx_seq += 1;
        if stack.is_empty() {
            self.fx_shown = self.fx_seq;
            let top = &frames[frames.len() - 1];
            self.show_picture(top.width, top.height, top.bgra.clone());
        } else {
            self.fx_worker.submit(effects::Job {
                seq: self.fx_seq,
                frames,
                backdrop: backdrop.clone(),
                stack: stack.clone(),
            });
        }
        self.fx_last = Some((self.frame_seq, stack, backdrop.is_some()));
    }

    /// A Background photo for the compositor: decoded once, off the UI
    /// thread; None until it's in (the canvas colour shows meanwhile).
    fn backdrop_image(&mut self, path: &str, cx: &mut Context<Self>) -> Option<Arc<DecodedFrame>> {
        if let Some(loaded) = self.backdrop_images.get(path) {
            return loaded.clone();
        }
        self.backdrop_images.insert(path.to_string(), None);
        let p = path.to_string();
        let task = cx
            .background_executor()
            .spawn(async move { decoder::thumbnail(&p, 0, PREVIEW_MAX_WIDTH) });
        let key = path.to_string();
        cx.spawn(async move |this, cx| {
            let Ok(frame) = task.await else { return };
            this.update(cx, |ed, cx| {
                ed.backdrop_images.insert(key, Some(Arc::new(frame)));
                cx.notify();
            })
            .ok();
        })
        .detach();
        None
    }

    fn show_picture(&mut self, width: u32, height: u32, bgra: Vec<u8>) {
        let Some(buffer) = image::RgbaImage::from_raw(width, height, bgra) else {
            return;
        };
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
        if let Some(old) = self.frame.replace(image) {
            self.stale_frames.push(old);
        }
    }

    fn on_rendered(&mut self, r: Rendered, cx: &mut Context<Self>) {
        // Superseded meanwhile (or the frame went away).
        if r.seq <= self.fx_shown || self.wanted.is_empty() {
            return;
        }
        self.fx_shown = r.seq;
        self.show_picture(r.width, r.height, r.bgra);
        cx.notify();
    }

    /// Downloads a package unless it's here or on its way.
    fn fetch_package(&mut self, key: String, filter: bool, cx: &mut Context<Self>) {
        if !self.packages.begin(&key) {
            return;
        }
        let task = cx
            .background_executor()
            .spawn(async move { fx_package::fetch(key, filter) });
        cx.spawn(async move |this, cx| {
            let fetched: Fetched = task.await;
            this.update(cx, |ed, cx| {
                ed.packages.end(fetched);
                cx.notify();
            })
            .ok();
        })
        .detach();
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

    pub(crate) fn on_root_mouse_move(
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
            Drag::Keyframe(_) => self.update_keyframe_drag(e.position, cx),
            Drag::Gizmo(_) => self.update_gizmo_drag(e.position, cx),
            Drag::Crop(_) => self.update_crop_drag(e.position, cx),
            Drag::Layer(_) => self.update_layer_drag(cx),
            Drag::Beat(_) => self.update_beat_drag(e.position, cx),
            Drag::Zoom => self.zoom_slider_to(e.position.x, cx),
            Drag::Spectrum(_) => self.spectrum_to(e.position, cx),
            Drag::Nudge { origin, apply } => {
                let (dx, apply) = (f32::from(e.position.x) - *origin, apply.clone());
                self.nudge_dx = dx;
                apply(self, dx, false, cx);
            }
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

    /// Starts scrubbing a number field pressed at `x`.
    pub fn start_nudge(&mut self, x: Pixels, apply: NudgeFn, cx: &mut Context<Self>) {
        self.nudge_dx = 0.;
        self.drag = Some(Drag::Nudge {
            origin: f32::from(x),
            apply,
        });
        cx.notify();
    }

    pub fn zoom_slider_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        if let Some(f) = self.slider_fraction("zoom", x) {
            self.set_zoom(MIN_PPS * (MAX_PPS / MIN_PPS).powf(f), cx);
        }
    }

    pub(crate) fn end_drag(&mut self, cx: &mut Context<Self>) {
        match self.drag.take() {
            Some(Drag::Splitter { .. }) => self.prefs.save(),
            Some(Drag::Slider { apply, .. }) => {
                if let Some(f) = self.last_slider_fraction.take() {
                    apply(self, f, true, cx);
                }
                self.live_value = None;
                self.live_fx = None;
            }
            Some(Drag::Nudge { apply, .. }) => {
                let dx = std::mem::take(&mut self.nudge_dx);
                apply(self, dx, true, cx);
                self.live_value = None;
            }
            Some(Drag::Clip(d)) => self.commit_clip_drag(*d, cx),
            Some(Drag::Keyframe(d)) => self.commit_keyframe_drag(*d, cx),
            Some(Drag::Gizmo(d)) => self.commit_gizmo_drag(*d, cx),
            Some(Drag::Layer(d)) => self.commit_layer_drag(*d, cx),
            Some(Drag::Beat(d)) => self.commit_beat_drag(*d, cx),
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
        self.follow_playhead();
        // A text field let go of the keyboard (Esc): the shortcuts get it back.
        if self.screen == Screen::Editor && window.focused(cx).is_none() {
            window.focus(&self.focus);
        }
        // Arrange layers goes with the selection.
        if self.selected.primary().is_none() {
            self.layers_panel = None;
        }
        self.load_stickers(cx);
        self.sync_audio();
        self.sync_text_field(window, cx);
        self.refresh_picture(cx);
        self.tick_edge_scroll(window, cx);
        self.prefetch_effects(cx);
        self.prefetch_library(cx);
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
            // Return confirms the Crop dialog.
            .on_key_down(cx.listener(|ed, e: &gpui::KeyDownEvent, _, cx| {
                if ed.crop.is_some() && e.keystroke.key == "enter" {
                    cx.stop_propagation();
                    ed.confirm_crop(cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|ed, _, _, cx| ed.end_drag(cx)),
            )
            .on_action(cx.listener(Self::import))
            .on_action(cx.listener(Self::open_project))
            .on_action(cx.listener(Self::save_project))
            .on_action(cx.listener(Self::save_project_as))
            .on_action(cx.listener(|ed, _: &OpenSettings, _, cx| ed.open_settings(false, cx)))
            .on_action(cx.listener(|ed, _: &CheckForUpdates, _, cx| ed.check_for_updates(cx)))
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
                        .child(panel_box().w(px(sizes.inspector)).bg(theme::sheet()).child(
                            if self.agent.settings.open {
                                self.render_agent(cx)
                            } else {
                                self.render_inspector(cx).into_any_element()
                            },
                        )),
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
        root.children(self.render_overlays(window, cx))
            // Linux, when the window decorates itself: its resize edges.
            .children(crate::ui::window_chrome::resize_edges(window))
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
    fn render_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let viewport = window.viewport_size();
        let mut out = vec![];
        if let Some(popup) = &self.popup {
            out.push(crate::ui::dialogs::popup_menu(popup, viewport, cx).into_any_element());
        }
        if self.shortcuts_open {
            out.push(crate::ui::dialogs::shortcuts_dialog(cx).into_any_element());
        }
        if let Some(dialog) = self.render_crop_dialog(viewport, cx) {
            out.push(dialog);
        }
        if self.settings_open {
            out.push(self.render_settings(viewport, cx));
        }
        if self.export.is_some() {
            out.push(self.render_export(window, cx));
        }
        if let Some(confirm) = &self.confirm {
            out.push(crate::ui::dialogs::confirm_dialog(confirm, cx).into_any_element());
        }
        if self.update_dialog_shown(cx) {
            out.push(self.render_update_dialog(viewport, cx));
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
