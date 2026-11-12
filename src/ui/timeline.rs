//! The timeline, after the old desktop editor's (`Timeline.tsx`,
//! `TimelineLanes.tsx`, `TimelineClip.tsx`): the ruler on top, the main lane
//! under it, then a stack of the other lanes that hold clips (overlay
//! video, effects, audio, text) which scrolls vertically when it doesn't
//! fit. Overlapping clips on a lane stack in sub-rows. A track header
//! column on the left holds each lane's icon and lock / eye / speaker / …
//! switches.
//!
//! Clips draw by kind: video as a filmstrip under a name + length strip,
//! images as their picture, audio as a waveform, the rest as a label. The
//! selected clip gets trim handles. The interaction lives in
//! `timeline_edit.rs`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{
    AnyElement, Bounds, Context, CursorStyle, DragMoveEvent, ExternalPaths, FontWeight,
    MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, Pixels, Rgba, ScrollWheelEvent,
    SharedString, canvas, div, fill, img, point, prelude::*, px, rgb, rgba, size,
};

use crate::decoder::{self, Waveform};
use crate::editor::{Drag, Editor, Tool, timecode};
use crate::i18n::t;
use crate::model::{BeatStrength, Clip, Micros, Project, SECOND, Side, Track, TrackType};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::timeline_edit::{
    ClipDragMode, KEYFRAME_TIME_EPSILON, MediaDrag, MediaDrop, keyframe_eps,
};
use crate::ui::widgets::{icon_button, mono};

// ---- geometry (y from the top of the lane area) ------------------------------

pub const TOP_PAD: f32 = 8.;
pub const RULER_HEIGHT: f32 = 20.;
pub const MAIN_TOP: f32 = TOP_PAD + RULER_HEIGHT + 4.;
pub const MAIN_LANE_HEIGHT: f32 = 56.;
pub const MAIN_BOTTOM: f32 = MAIN_TOP + MAIN_LANE_HEIGHT;
/// Where the lane stack's window starts.
pub const STACK_TOP: f32 = MAIN_BOTTOM + 6.;
pub const LANE_HEIGHT: f32 = 32.;
pub const LANE_GAP: f32 = 6.;
/// Room under the last lane.
pub const BOTTOM_PAD: f32 = 12.;
/// The track header column (the old desktop's: its 220 px head offset
/// less the 12 px lead-in).
pub const LANE_HEADER_WIDTH: f32 = 208.;
/// How close (px) an edge must come to snap.
pub const SNAP_PX: f32 = 10.;
/// The shortest a trim leaves a clip.
pub const MIN_CLIP: Micros = 600_000;
/// The name + length strip along a video / image clip's top.
const NAME_STRIP: f32 = 17.;

// ---- filmstrip ----------------------------------------------------------------

/// Target on-screen width of a filmstrip tile.
const TILE_WIDTH: f32 = 44.;
const MAX_TILES: usize = 400;
/// Source-time grids a tile's frame snaps to, finest first: each divides
/// the next, so frames picked zoomed out are reused zoomed in.
const TILE_GRID: [Micros; 12] = [
    31_250, 62_500, 125_000, 250_000, 500_000, 1_000_000, 2_000_000, 4_000_000, 8_000_000,
    16_000_000, 32_000_000, 64_000_000,
];
/// Keeps a snapped frame off the segment's exclusive end.
const SEGMENT_END_GUARD: Micros = 33_333;
/// A keyframe diamond's size, and its press target.
const DIAMOND: f32 = 12.;
const DIAMOND_HIT: f32 = 20.;
/// How far above a clip's bottom its beat dots sit.
const BEAT_DOT_INSET: f32 = 5.;
/// Waveform column spacing (px).
const WAVE_STEP: f32 = 2.;

/// Which lane of the stack a track's clips are drawn on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneKind {
    Main,
    Captions,
    Overlay,
    Effect,
    Audio,
    Text,
}

impl LaneKind {
    fn icon(self) -> Icon {
        match self {
            LaneKind::Main => Icon::Filmstrip,
            LaneKind::Captions => Icon::ClosedCaption,
            LaneKind::Overlay => Icon::Layers,
            LaneKind::Effect => Icon::Sparkle4,
            LaneKind::Audio => Icon::MusicalNote,
            LaneKind::Text => Icon::Title,
        }
    }
    fn name(self) -> &'static str {
        match self {
            LaneKind::Main => "Main track",
            LaneKind::Captions => "Captions",
            LaneKind::Overlay => "Overlay",
            LaneKind::Effect => "Effects",
            LaneKind::Audio => "Audio",
            LaneKind::Text => "Text",
        }
    }
    /// The lane carries sound: it gets a speaker switch.
    fn has_sound(self) -> bool {
        matches!(self, LaneKind::Main | LaneKind::Overlay | LaneKind::Audio)
    }
}

/// Which of its tracks' clips a lane shows: captions sit on the text track
/// but get a lane of their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneFilter {
    All,
    Captions,
    NoCaptions,
}

impl LaneFilter {
    pub fn holds(self, clip: &Clip) -> bool {
        match self {
            LaneFilter::All => true,
            LaneFilter::Captions => clip.is_caption(),
            LaneFilter::NoCaptions => !clip.is_caption(),
        }
    }
}

/// What a lane's switches and "…" menu act on: whole tracks, and single
/// clips where lanes share a track (Captions and Text).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaneTargets {
    pub track_ids: Vec<String>,
    pub clip_ids: Vec<String>,
}

impl LaneTargets {
    /// Every clip the lane covers.
    pub fn clip_ids(&self, project: &Project) -> Vec<String> {
        project
            .tracks
            .iter()
            .flat_map(|t| {
                t.clips.iter().filter(move |c| {
                    self.track_ids.contains(&t.id) || self.clip_ids.contains(&c.id)
                })
            })
            .map(|c| c.id.clone())
            .collect()
    }

    /// The switch is on for every track and clip the lane covers.
    pub fn flag_on(&self, project: &Project, flag: LaneFlag) -> bool {
        if self.track_ids.is_empty() && self.clip_ids.is_empty() {
            return false;
        }
        project.tracks.iter().all(|t| {
            (!self.track_ids.contains(&t.id) || flag.of_track(t))
                && t.clips
                    .iter()
                    .all(|c| !self.clip_ids.contains(&c.id) || flag.of_clip(c))
        })
    }
}

/// One drawn lane: the tracks it shows and where its clips sit.
pub struct LaneLayout {
    pub kind: LaneKind,
    pub track_ids: Vec<String>,
    pub filter: LaneFilter,
    pub targets: LaneTargets,
    /// Top: in the lane area (main), or in the stack's content (others).
    pub top: f32,
    pub height: f32,
    pub row_height: f32,
    /// Sub-row per clip id (overlapping clips stack).
    pub rows: HashMap<String, usize>,
}

impl LaneLayout {
    /// The lane draws this clip (of one of its tracks).
    pub fn holds(&self, clip: &Clip) -> bool {
        self.filter.holds(clip)
    }

    pub fn clip_top(&self, clip_id: &str) -> f32 {
        let row = self.rows.get(clip_id).copied().unwrap_or(0);
        self.top + row as f32 * (self.row_height + LANE_GAP)
    }
}

/// Sub-rows for clips that overlap in time: each clip takes the first row
/// free at its start. Returns the row per clip and how many rows.
pub fn assign_rows<'a>(clips: impl Iterator<Item = &'a Clip>) -> (HashMap<String, usize>, usize) {
    let mut sorted: Vec<&Clip> = clips.collect();
    sorted.sort_by(|a, b| {
        a.timeline_start
            .cmp(&b.timeline_start)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut row_end: Vec<Micros> = vec![];
    let mut rows = HashMap::new();
    for clip in sorted {
        let row = match row_end.iter().position(|&end| end <= clip.timeline_start) {
            Some(r) => r,
            None => {
                row_end.push(0);
                row_end.len() - 1
            }
        };
        row_end[row] = clip.end();
        rows.insert(clip.id.clone(), row);
    }
    let count = row_end.len().max(1);
    (rows, count)
}

/// The main lane and the stack under it (only lanes that hold clips, in
/// the old editor's order), and the stack's content height.
pub fn lane_layouts(project: &Project) -> (LaneLayout, Vec<LaneLayout>, f32) {
    let main_id = project.main_track().map(|t| t.id.clone());
    let main = LaneLayout {
        kind: LaneKind::Main,
        track_ids: main_id.iter().cloned().collect(),
        filter: LaneFilter::All,
        targets: LaneTargets {
            track_ids: main_id.iter().cloned().collect(),
            clip_ids: vec![],
        },
        top: MAIN_TOP,
        height: MAIN_LANE_HEIGHT,
        row_height: MAIN_LANE_HEIGHT,
        rows: HashMap::new(),
    };
    let kind_of = |t: &Track| -> LaneKind {
        match t.kind {
            TrackType::Video => LaneKind::Overlay, // a second "video" track
            TrackType::Overlay if t.is_overlay_video(&project.id) => LaneKind::Overlay,
            TrackType::Overlay | TrackType::Text => LaneKind::Text,
            TrackType::Effect => LaneKind::Effect,
            TrackType::Audio => LaneKind::Audio,
        }
    };
    let mut stack = vec![];
    let mut top = 0.;
    for kind in [
        LaneKind::Captions,
        LaneKind::Overlay,
        LaneKind::Effect,
        LaneKind::Audio,
        LaneKind::Text,
    ] {
        let (filter, of_kind) = match kind {
            LaneKind::Captions => (LaneFilter::Captions, LaneKind::Text),
            LaneKind::Text => (LaneFilter::NoCaptions, LaneKind::Text),
            k => (LaneFilter::All, k),
        };
        let tracks: Vec<&Track> = project
            .tracks
            .iter()
            .filter(|t| Some(&t.id) != main_id.as_ref() && kind_of(t) == of_kind)
            .filter(|t| kind != LaneKind::Captions || t.kind == TrackType::Text)
            .collect();
        let clips: Vec<&Clip> = tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter(|c| filter.holds(c))
            .collect();
        if clips.is_empty() {
            continue;
        }
        // Captions never fan out: one row, in time order.
        let (rows, count) = if kind == LaneKind::Captions {
            (clips.iter().map(|c| (c.id.clone(), 0)).collect(), 1)
        } else {
            assign_rows(clips.iter().copied())
        };
        // The text track is shared: Captions and Text switch their own clips.
        let targets = match kind {
            LaneKind::Captions | LaneKind::Text => LaneTargets {
                track_ids: tracks
                    .iter()
                    .filter(|t| t.kind != TrackType::Text)
                    .map(|t| t.id.clone())
                    .collect(),
                clip_ids: tracks
                    .iter()
                    .filter(|t| t.kind == TrackType::Text)
                    .flat_map(|t| t.clips.iter())
                    .filter(|c| filter.holds(c))
                    .map(|c| c.id.clone())
                    .collect(),
            },
            _ => LaneTargets {
                track_ids: tracks.iter().map(|t| t.id.clone()).collect(),
                clip_ids: vec![],
            },
        };
        let height = LANE_HEIGHT * count as f32 + LANE_GAP * (count - 1) as f32;
        stack.push(LaneLayout {
            kind,
            track_ids: tracks.iter().map(|t| t.id.clone()).collect(),
            filter,
            targets,
            top,
            height,
            row_height: LANE_HEIGHT,
            rows,
        });
        top += height + LANE_GAP;
    }
    (main, stack, top)
}

/// How much of each main clip's span the box leaves to a neighbour it
/// overlaps through a transition (micros at its start, its end): the two
/// boxes meet mid-overlap, the times stay true.
pub fn transition_seams(clips: &[&Clip]) -> HashMap<String, (Micros, Micros)> {
    let mut seams: HashMap<String, (Micros, Micros)> = HashMap::new();
    for pair in clips.windows(2) {
        let (prev, clip) = (pair[0], pair[1]);
        if prev.out_transition() == 0 {
            continue;
        }
        let overlap = prev.end() - clip.timeline_start;
        if overlap <= 0 {
            continue;
        }
        seams.entry(prev.id.clone()).or_default().1 = overlap / 2;
        seams.entry(clip.id.clone()).or_default().0 = overlap / 2;
    }
    seams
}

/// The coarsest frame grid no wider than a tile's source step; the finest
/// one (about a frame) when it's denser still, so tile times never follow
/// the zoom itself (each zoom step would ask for all-new frames).
fn tile_grid(step: f64) -> Micros {
    TILE_GRID
        .iter()
        .copied()
        .take_while(|&g| g as f64 <= step)
        .last()
        .unwrap_or(TILE_GRID[0])
}

/// The source time shown `offset` into `clip` (constant speed; reversed
/// and freeze aware).
fn source_time_at(clip: &Clip, offset: Micros) -> Micros {
    let start = clip.source_start.unwrap_or(0);
    if clip.freeze() {
        return start;
    }
    let speed = if clip.speed > 0. { clip.speed } else { 1. };
    let forward = (offset as f64 * speed) as Micros;
    if clip.reversed() {
        let segment = (clip.timeline_duration as f64 * speed) as Micros;
        start + (segment - forward).max(0)
    } else {
        start + forward
    }
}

/// A video clip's filmstrip `width` px wide: the tile width and each
/// tile's source time (centred in its slot, snapped to a grid so zooming
/// reuses frames).
fn filmstrip(clip: &Clip, width: f32) -> (f32, Vec<Micros>) {
    let count = ((width / TILE_WIDTH).round() as usize).clamp(1, MAX_TILES);
    let speed = if clip.speed > 0. { clip.speed } else { 1. };
    let grid = if clip.freeze() || count == 1 {
        0
    } else {
        tile_grid(clip.timeline_duration as f64 / count as f64 * speed)
    };
    let seg_start = clip.source_start.unwrap_or(0);
    let seg_end = seg_start
        .max(seg_start + (clip.timeline_duration as f64 * speed) as Micros - SEGMENT_END_GUARD);
    let times = (0..count)
        .map(|i| {
            let offset =
                ((i as f64 + 0.5) * clip.timeline_duration as f64 / count as f64) as Micros;
            let exact = source_time_at(clip, offset);
            if grid > 0 {
                (((exact as f64 / grid as f64).round() as Micros) * grid).clamp(seg_start, seg_end)
            } else {
                exact
            }
        })
        .collect();
    (width / count as f32, times)
}

/// Where the main lane's Cover tile gets its picture.
#[derive(Clone, Debug, PartialEq)]
enum CoverSource {
    /// A picture file (the saved cover, or an image clip).
    Still(String),
    /// A video frame: (file, source time).
    Frame(String, Micros),
}

/// The saved cover, else the main lane's first frame (the clip at 00:00,
/// or the last one, at the source time it shows there).
fn cover_source(project: &Project) -> Option<CoverSource> {
    if let Some(uri) = project
        .extra
        .get("cover")
        .and_then(|c| c.get("imageUri"))
        .and_then(|u| u.as_str())
    {
        let path = uri.strip_prefix("file://").unwrap_or(uri);
        // file:///C:/… on Windows: no slash before the drive.
        let path = match path.as_bytes() {
            [b'/', _, b':', ..] if cfg!(windows) => &path[1..],
            _ => path,
        };
        if !path.is_empty() {
            return Some(CoverSource::Still(path.to_string()));
        }
    }
    let clips: Vec<&Clip> = project
        .ordered_main_clips()
        .into_iter()
        .filter(|c| matches!(c.kind.as_str(), "video" | "image"))
        .collect();
    let clip = clips.iter().find(|c| c.end() > 0).or(clips.last())?;
    let src = clip.source.clone().filter(|s| !s.is_empty())?;
    if clip.kind == "image" {
        return Some(CoverSource::Still(src));
    }
    // A frame clear of the clip's end, where a decoder can come up empty.
    let offset =
        (-clip.timeline_start).clamp(0, (clip.timeline_duration - SEGMENT_END_GUARD).max(0));
    Some(CoverSource::Frame(src, source_time_at(clip, offset)))
}

/// Clip fills by kind (the old desktop timeline's).
fn clip_color(kind: &str) -> Rgba {
    match kind {
        "video" | "image" => rgb(0x1d4a4e),
        "audio" => rgb(0x1f4a6e),
        "text" => rgb(0x9a4b32),
        "sticker" => rgb(0xa8792a),
        "drawing" => rgb(0x3d6b4d),
        "effect" => rgb(0x5a3ea3),
        _ => theme::clip_sticker(),
    }
}

/// "00:02", or "00:01.5" on a half-second step.
fn ruler_label(seconds: f64) -> String {
    let whole = seconds.floor() as i64;
    let mmss = format!("{:02}:{:02}", whole / 60, whole % 60);
    let frac = seconds - whole as f64;
    if frac > 0.001 {
        format!("{mmss}.{}", (frac * 10.).round() as i64)
    } else {
        mmss
    }
}

/// Minor ticks per labelled ruler step.
fn minor_divisions(major: f64) -> u32 {
    match major {
        m if m == 2. || m == 120. => 4,
        15. => 3,
        30. => 6,
        _ => 5,
    }
}

/// A lane switch in the track header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneFlag {
    Locked,
    Hidden,
    Muted,
}

impl LaneFlag {
    fn of_track(self, t: &Track) -> bool {
        match self {
            LaneFlag::Locked => t.locked,
            LaneFlag::Hidden => t.hidden,
            LaneFlag::Muted => t.muted,
        }
    }
    fn of_clip(self, c: &Clip) -> bool {
        match self {
            LaneFlag::Locked => c.locked(),
            LaneFlag::Hidden => c.hidden,
            LaneFlag::Muted => c.muted(),
        }
    }
    fn key(self) -> &'static str {
        match self {
            LaneFlag::Locked => "locked",
            LaneFlag::Hidden => "hidden",
            LaneFlag::Muted => "muted",
        }
    }
}

/// Where a clip is drawn this frame.
struct ClipGeom {
    /// Lane-area x of the clip's true start, and its true width (px).
    x: f32,
    w: f32,
    /// Box insets at transition seams (px, start / end).
    inset: (f32, f32),
    y: f32,
    h: f32,
    /// Being moved: drawn see-through, no handles.
    moving: bool,
}

impl Editor {
    /// Asks for whatever the visible clips draw that isn't loaded yet:
    /// filmstrip tiles, still pictures, waveforms.
    fn request_timeline_media(&mut self, width: f32, cx: &mut Context<Self>) {
        let pps = self.pps;
        let mut tiles = vec![];
        let mut stills = vec![];
        let mut sounds = vec![];
        for clip in self.project.tracks.iter().flat_map(|t| t.clips.iter()) {
            let Some(src) = clip.source.as_deref().filter(|s| !s.is_empty()) else {
                continue;
            };
            let x = self.x_of(clip.timeline_start);
            let w = clip.timeline_duration as f32 / SECOND as f32 * pps;
            if x > width || x + w < 0. {
                continue;
            }
            match clip.kind.as_str() {
                "video" => {
                    let (tw, times) = filmstrip(clip, w);
                    for (i, t) in times.into_iter().enumerate() {
                        let tx = x + i as f32 * tw;
                        if tx + tw >= 0. && tx <= width {
                            tiles.push((src.to_string(), t));
                        }
                    }
                }
                "image" | "sticker" => stills.push(src.to_string()),
                "audio" => sounds.push(src.to_string()),
                _ => {}
            }
        }
        match cover_source(&self.project) {
            Some(CoverSource::Still(path)) => stills.push(path),
            Some(CoverSource::Frame(src, t)) => tiles.push((src, t)),
            None => {}
        }
        // The visible tiles not here yet; sent again only when that set
        // changes (each send replaces what the filmstrip thread had left).
        let missing: HashSet<(String, Micros)> = tiles
            .into_iter()
            .filter(|key| !self.tiles.contains_key(key))
            .collect();
        if missing != self.tiles_pending {
            self.tile_server.want(missing.iter().cloned().collect());
            self.tiles_pending = missing;
        }
        for src in stills {
            self.load_thumbnail(src, 0, cx);
        }
        for src in sounds {
            if self.waves.contains_key(&src) {
                continue;
            }
            self.waves.insert(src.clone(), None);
            let path = src.clone();
            let task = cx
                .background_executor()
                .spawn(async move { decoder::waveform(&path) });
            cx.spawn(async move |this, cx| {
                let Ok(wave) = task.await else { return };
                this.update(cx, |ed, cx| {
                    ed.waves.insert(src, Some(Arc::new(wave)));
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    pub fn render_timeline(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let fps = self.fps();
        let (main_lane, stack, stack_height) = lane_layouts(&self.project);
        let lanes = self.measured("lanes");
        let width = lanes.map_or(2000., |b| f32::from(b.size.width));
        let height = lanes.map_or(400., |b| f32::from(b.size.height));
        let window_h = (height - STACK_TOP - BOTTOM_PAD).max(LANE_HEIGHT);
        self.lane_scroll_y = self
            .lane_scroll_y
            .clamp(0., (stack_height - window_h).max(0.));
        self.request_timeline_media(width, cx);
        let scroll_y = self.lane_scroll_y;
        let split_tool = self.tool == Tool::Split;

        // The clip being moved / trimmed, as it is right now.
        let live = match &self.drag {
            Some(Drag::Clip(d)) if d.started => Some((
                d.clip_id.clone(),
                d.mode,
                d.live_start,
                d.live_duration,
                d.dy,
                d.lane_movable,
            )),
            _ => None,
        };
        let moving_id = live
            .as_ref()
            .filter(|l| l.1 == ClipDragMode::Move)
            .map(|l| l.0.clone());

        // ---- ruler
        let steps = [0.5, 1., 2., 5., 10., 15., 30., 60., 120., 300., 600.];
        let major = steps
            .iter()
            .copied()
            .find(|s| s * self.pps as f64 >= 96.)
            .unwrap_or(1200.);
        let divisions = minor_divisions(major);
        let step = major / divisions as f64;
        let first = (self.scroll_x as f64 / self.pps as f64 / step).floor() as i64;
        let last = ((self.scroll_x + width) as f64 / self.pps as f64 / step).ceil() as i64;
        let ticks = (first.max(0)..=last).map(|k| {
            let seconds = k as f64 * step;
            let x = self.x_of((seconds * SECOND as f64) as Micros);
            let is_major = k % divisions as i64 == 0;
            div()
                .absolute()
                .left(px(x))
                .top_0()
                .h(px(RULER_HEIGHT))
                .flex()
                .items_end()
                .child(
                    div()
                        .w(px(1.))
                        .h(px(if is_major { RULER_HEIGHT } else { 4. }))
                        .bg(theme::fill_strong()),
                )
                .when(is_major, |d| {
                    d.child(
                        div()
                            .absolute()
                            .left(px(4.))
                            .top(px(3.))
                            .whitespace_nowrap()
                            .child(mono(
                                ruler_label(seconds),
                                theme::XXS,
                                theme::text_tertiary(),
                            )),
                    )
                })
        });

        // ---- main lane
        let main_track = self.project.main_track().cloned();
        let ordered: Vec<Clip> = self
            .project
            .ordered_main_clips()
            .into_iter()
            .cloned()
            .collect();
        let seams = transition_seams(&ordered.iter().collect::<Vec<_>>());
        let pps = self.pps;
        let to_px = |m: Micros| m as f32 / SECOND as f32 * pps;
        let mut main_clips: Vec<(bool, AnyElement)> = vec![];
        let mut slots: Vec<AnyElement> = vec![];
        let mut floating: Option<AnyElement> = None;

        // Places one clip: in its lane (y relative to the lane box), or,
        // while it's being moved, over everything in lane-area coordinates.
        let place = |ed: &Editor,
                     clip: &Clip,
                     track: &Track,
                     lane_y: f32,
                     area_y: f32,
                     h: f32,
                     inset: (Micros, Micros),
                     cx: &mut Context<Editor>|
         -> (Option<AnyElement>, Option<AnyElement>, Option<AnyElement>) {
            let geom = ClipGeom {
                x: ed.x_of(clip.timeline_start),
                w: to_px(clip.timeline_duration),
                inset: (to_px(inset.0), to_px(inset.1)),
                y: lane_y,
                h,
                moving: false,
            };
            match &live {
                Some((id, mode, start, dur, dy, movable)) if *id == clip.id => {
                    if *mode == ClipDragMode::Move {
                        let slot = div()
                            .absolute()
                            .left(px(geom.x))
                            .top(px(area_y))
                            .w(px((geom.w - 1.).max(2.)))
                            .h(px(h))
                            .rounded(px(4.))
                            .bg(rgba(0xffffff14))
                            .border_1()
                            .border_color(rgba(0xffffff2e))
                            .into_any_element();
                        let float = ed.clip_el(
                            clip,
                            track,
                            ClipGeom {
                                x: ed.x_of(*start),
                                y: area_y + if *movable { *dy } else { 0. },
                                inset: (0., 0.),
                                moving: true,
                                ..geom
                            },
                            cx,
                        );
                        (None, Some(slot), Some(float))
                    } else {
                        let el = ed.clip_el(
                            clip,
                            track,
                            ClipGeom {
                                x: ed.x_of(*start),
                                w: to_px(*dur),
                                ..geom
                            },
                            cx,
                        );
                        (Some(el), None, None)
                    }
                }
                _ => (Some(ed.clip_el(clip, track, geom, cx)), None, None),
            }
        };

        if let Some(track) = &main_track {
            for clip in &ordered {
                let inset = seams.get(&clip.id).copied().unwrap_or((0, 0));
                let (el, slot, float) =
                    place(self, clip, track, 0., MAIN_TOP, MAIN_LANE_HEIGHT, inset, cx);
                let selected = self.selected.contains(&clip.id);
                if let Some(el) = el {
                    main_clips.push((selected, el));
                }
                slots.extend(slot);
                if float.is_some() {
                    floating = float;
                }
            }
        }
        // The selected clip draws over its neighbours (its handles reach out).
        main_clips.sort_by_key(|(selected, _)| *selected);

        // Transition buttons between main clips (not beside the selected one).
        let seam_buttons: Vec<AnyElement> = ordered
            .windows(2)
            .filter(|pair| {
                !self.selected.contains(&pair[0].id) && !self.selected.contains(&pair[1].id)
            })
            .filter(|_| moving_id.is_none())
            .map(|pair| {
                let clip = &pair[0];
                let end_inset = seams.get(&clip.id).map_or(0, |s| s.1);
                let x = self.x_of(clip.end() - end_inset);
                let has = clip.out_transition() > 0;
                div()
                    .id(SharedString::from(format!("seam-{}", clip.id)))
                    .absolute()
                    .left(px(x - 11.))
                    .top(px(MAIN_LANE_HEIGHT / 2. - 11.))
                    .size(px(22.))
                    .rounded(px(5.))
                    .bg(theme::text())
                    .shadow_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(
                        if has { Icon::Transition } else { Icon::Shuffle }.el(12., rgb(0x141416)),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // Transitions live in the browser's Transitions tab,
                    // which follows the cut nearest the playhead.
                    .on_click({
                        let cut = clip.end() - clip.out_transition() / 2;
                        cx.listener(move |ed, _, _, cx| {
                            ed.pause();
                            ed.seek(cut, cx);
                            ed.browser_tab = crate::ui::browser::TRANSITIONS_TAB;
                            ed.browser_views[crate::ui::browser::TRANSITIONS_TAB] = 0;
                            cx.notify();
                        })
                    })
                    .into_any_element()
            })
            .collect();

        // ---- the lane stack
        let mut stack_clips: Vec<(bool, AnyElement)> = vec![];
        for lane in &stack {
            for tid in &lane.track_ids {
                let Some(track) = self.project.tracks.iter().find(|t| &t.id == tid).cloned() else {
                    continue;
                };
                for clip in track.clips.iter().filter(|c| lane.holds(c)) {
                    let y = lane.clip_top(&clip.id) - scroll_y;
                    let (el, slot, float) = place(
                        self,
                        clip,
                        &track,
                        y,
                        STACK_TOP + y,
                        lane.row_height,
                        (0, 0),
                        cx,
                    );
                    let selected = self.selected.contains(&clip.id);
                    if let Some(el) = el {
                        stack_clips.push((selected, el));
                    }
                    slots.extend(slot);
                    if float.is_some() {
                        floating = float;
                    }
                }
            }
        }
        stack_clips.sort_by_key(|(selected, _)| *selected);

        // ---- guides over the lanes
        let mut guides: Vec<AnyElement> = vec![];
        if let Some(Drag::Clip(d)) = &self.drag {
            if let Some(t) = d.snapped.filter(|_| d.started) {
                guides.push(
                    div()
                        .absolute()
                        .left(px(self.x_of(t) - 1.))
                        .top(px(TOP_PAD))
                        .bottom_0()
                        .w(px(2.))
                        .bg(rgba(0xffffffd9))
                        .into_any_element(),
                );
            }
            if let Some(t) = d.insert_at.filter(|_| d.started) {
                guides.push(insert_line(self.x_of(t)));
            }
        }
        match self.media_drop {
            Some(MediaDrop::Main { at, .. }) => guides.push(insert_line(self.x_of(at))),
            Some(MediaDrop::Lane { at, length }) => guides.push(
                div()
                    .absolute()
                    .left(px(self.x_of(at)))
                    .top(px(STACK_TOP))
                    .w(px(to_px(length).max(8.)))
                    .h(px(LANE_HEIGHT))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme::text())
                    .bg(theme::fill_strong())
                    .into_any_element(),
            ),
            None => {}
        }
        if let (Some(Drag::Marquee(m)), Some(b)) = (&self.drag, lanes) {
            let x0 = f32::from(m.origin.x.min(m.to.x) - b.origin.x);
            let y0 = f32::from(m.origin.y.min(m.to.y) - b.origin.y);
            let w = f32::from((m.origin.x - m.to.x).abs());
            let h = f32::from((m.origin.y - m.to.y).abs());
            guides.push(
                div()
                    .absolute()
                    .left(px(x0))
                    .top(px(y0))
                    .w(px(w))
                    .h(px(h))
                    .border_1()
                    .border_color(rgba(0xffffffb3))
                    .bg(rgba(0xffffff14))
                    .into_any_element(),
            );
        }
        if let Some(t) = self.hover.filter(|_| self.drag.is_none()) {
            guides.push(
                div()
                    .absolute()
                    .left(px(self.x_of(t)))
                    .top(px(TOP_PAD))
                    .bottom_0()
                    .w(px(1.))
                    .bg(rgba(0xffffff8c))
                    .into_any_element(),
            );
        }
        // The stack's scrollbar, while it overflows.
        if stack_height > window_h {
            let bar = (window_h * window_h / stack_height).max(24.);
            let at = scroll_y / (stack_height - window_h) * (window_h - bar);
            guides.push(
                div()
                    .absolute()
                    .right(px(3.))
                    .top(px(STACK_TOP + at))
                    .w(px(4.))
                    .h(px(bar))
                    .rounded(px(2.))
                    .bg(theme::fill_strong())
                    .into_any_element(),
            );
        }

        let playhead_x = self.x_of(self.playhead);
        let readout = format!(
            "{} / {}",
            timecode(self.playhead, fps),
            timecode(self.project.duration, fps)
        );

        // ---- header column
        let main_header = main_track
            .as_ref()
            .map(|_| self.lane_header(&main_lane, cx));
        let stack_headers: Vec<AnyElement> = stack
            .iter()
            .map(|lane| {
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(lane.top - scroll_y))
                    .h(px(lane.height))
                    .child(self.lane_header(lane, cx))
                    .into_any_element()
            })
            .collect();

        let cursor = if split_tool {
            CursorStyle::Crosshair
        } else {
            CursorStyle::Arrow
        };

        div()
            .id("timeline")
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .bg(theme::timeline_bg())
            .overflow_hidden()
            .child(
                div()
                    .id("lane-headers")
                    .relative()
                    .flex_none()
                    .w(px(LANE_HEADER_WIDTH))
                    .h_full()
                    .border_r_1()
                    .border_color(theme::card_border())
                    .bg(theme::panel())
                    .on_scroll_wheel(
                        cx.listener(|ed, e: &ScrollWheelEvent, _, cx| ed.on_timeline_wheel(e, cx)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(px(MAIN_TOP))
                            .px_2()
                            .flex()
                            .items_center()
                            .child(mono(readout, theme::XXS, theme::text_secondary())),
                    )
                    .children(main_header.map(|h| {
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(MAIN_TOP))
                            .h(px(MAIN_LANE_HEIGHT))
                            .child(h)
                    }))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(STACK_TOP))
                            .bottom_0()
                            .overflow_hidden()
                            .children(stack_headers),
                    ),
            )
            .child(
                div()
                    .id("lanes")
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .cursor(cursor)
                    .child(self.measure("lanes"))
                    // Empty space and the ruler: deselect and scrub; with
                    // ⇧ / ⌘ held below the ruler, a rubber band adds clips.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|ed, e: &MouseDownEvent, _, cx| {
                            ed.end_hover();
                            if (e.modifiers.shift || e.modifiers.platform)
                                && ed.start_marquee(e.position, cx)
                            {
                                return;
                            }
                            ed.selected.clear();
                            ed.drag = Some(Drag::Scrub);
                            ed.scrub_to(e.position.x, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|ed, e: &MouseDownEvent, _, cx| {
                            ed.end_hover();
                            ed.open_lane_menu(e.position, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|ed, e: &MouseMoveEvent, _, cx| {
                        if ed.drag.is_none() && e.pressed_button.is_none() {
                            ed.hover_at(e.position.x, cx);
                        }
                    }))
                    .on_hover(cx.listener(|ed, hovered: &bool, _, cx| {
                        if !*hovered {
                            ed.end_hover();
                            cx.notify();
                        }
                    }))
                    .on_scroll_wheel(
                        cx.listener(|ed, e: &ScrollWheelEvent, _, cx| ed.on_timeline_wheel(e, cx)),
                    )
                    // Library tiles and files from the desktop.
                    .on_drag_move::<MediaDrag>(cx.listener(
                        |ed, e: &DragMoveEvent<MediaDrag>, _, cx| {
                            let item = e.drag(cx).0.clone();
                            let target = ed.media_drop_at(
                                e.event.position,
                                Some(item.duration),
                                !item.has_video,
                            );
                            ed.set_media_drop(target, cx);
                        },
                    ))
                    .on_drag_move::<ExternalPaths>(cx.listener(
                        |ed, e: &DragMoveEvent<ExternalPaths>, _, cx| {
                            let target = ed.media_drop_at(e.event.position, None, false);
                            ed.set_media_drop(target, cx);
                        },
                    ))
                    .on_drop(cx.listener(|ed, drag: &MediaDrag, _, cx| {
                        if let Some(target) = ed.media_drop {
                            ed.drop_media(vec![drag.0.clone()], target, cx);
                        }
                    }))
                    .on_drop(cx.listener(|ed, paths: &ExternalPaths, _, cx| {
                        let Some(target) = ed.media_drop else {
                            return;
                        };
                        let mut items = vec![];
                        for p in paths.paths() {
                            let path = p.to_string_lossy().into_owned();
                            if path.ends_with(".json") {
                                continue;
                            }
                            match decoder::probe(&path) {
                                Ok(item) => items.push(item),
                                Err(e) => ed.toast(format!("Can't open {path}: {e}"), cx),
                            }
                        }
                        ed.drop_media(items, target, cx);
                    }))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(TOP_PAD))
                            .h(px(RULER_HEIGHT))
                            .border_b_1()
                            .border_color(theme::card_border())
                            .children(ticks),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(MAIN_TOP))
                            .h(px(MAIN_LANE_HEIGHT))
                            .children(main_clips.into_iter().map(|(_, e)| e))
                            .children(seam_buttons),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(STACK_TOP))
                            .bottom_0()
                            .overflow_hidden()
                            .children(stack_clips.into_iter().map(|(_, e)| e)),
                    )
                    .children(slots)
                    .children(floating)
                    .children(guides)
                    // The playhead: a shield on the ruler over a line.
                    .child(
                        div()
                            .absolute()
                            .top(px(TOP_PAD + 2.))
                            .bottom_0()
                            .left(px(playhead_x - 1.))
                            .w(px(2.))
                            .bg(theme::playhead()),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(TOP_PAD + 2.))
                            .left(px(playhead_x - 5.5))
                            .w(px(11.))
                            .h(px(14.))
                            .rounded_t(px(3.))
                            .rounded_b(px(5.5))
                            .bg(theme::playhead()),
                    ),
            )
    }

    /// One clip box. `g.y` is relative to whatever holds it.
    fn clip_el(
        &self,
        clip: &Clip,
        track: &Track,
        g: ClipGeom,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = clip.id.clone();
        let selected = self.selected.contains(&clip.id);
        let single = self.selected.single() == Some(clip.id.as_str());
        let locked = track.locked || clip.locked();
        let hidden = clip.hidden || track.hidden;
        let box_x = g.x + g.inset.0;
        let box_w = (g.w - g.inset.0 - g.inset.1 - 1.).max(2.);
        let picture = matches!(clip.kind.as_str(), "video" | "image");
        let lanes = self.measured("lanes");
        let lanes_w = lanes.map_or(2000., |b| f32::from(b.size.width));

        // Contents placed at clip times, so they start `inset.0` left of the box.
        let shift = -g.inset.0;
        let mut content = div()
            .absolute()
            .inset_0()
            .rounded(px(4.))
            .overflow_hidden()
            .bg(if clip.is_caption() {
                theme::clip_caption()
            } else {
                clip_color(&clip.kind)
            })
            .border_1()
            .border_color(rgba(0xffffff17));

        match clip.kind.as_str() {
            "video" => {
                let (tw, times) = filmstrip(clip, g.w);
                let src = clip.source.clone().unwrap_or_default();
                for (i, t) in times.into_iter().enumerate() {
                    let tx = i as f32 * tw;
                    if g.x + tx + tw < 0. || g.x + tx > lanes_w {
                        continue;
                    }
                    if let Some(image) = self.tiles.get(&(src.clone(), t)) {
                        content = content.child(tile(image.clone(), shift + tx, tw));
                    }
                }
                content = content.child(div().absolute().inset_0().bg(rgba(0x00000038)));
            }
            "image" | "sticker" => {
                if let Some(image) = clip.source.as_ref().and_then(|s| self.thumbs.get(s)) {
                    if clip.kind == "sticker" {
                        let side = (g.h - 12.).max(12.);
                        content = content.child(
                            div()
                                .absolute()
                                .left(px(6.))
                                .top(px(6.))
                                .size(px(side))
                                .overflow_hidden()
                                .child(
                                    img(image.clone())
                                        .absolute()
                                        .inset_0()
                                        .size_full()
                                        .object_fit(ObjectFit::Contain),
                                ),
                        );
                    } else {
                        let count = ((g.w / TILE_WIDTH).round() as usize).clamp(1, MAX_TILES);
                        let tw = g.w / count as f32;
                        for i in 0..count {
                            let tx = i as f32 * tw;
                            if g.x + tx + tw < 0. || g.x + tx > lanes_w {
                                continue;
                            }
                            content = content.child(tile(image.clone(), shift + tx, tw));
                        }
                        content = content.child(div().absolute().inset_0().bg(rgba(0x00000038)));
                    }
                }
            }
            "audio" => {
                if let Some(wave) = self.waveform(clip, g.w, shift, lanes) {
                    content = content.child(wave);
                }
                let lit = self.beat_under_playhead(clip);
                if let Some(dots) = beat_dots(clip, shift, self.pps, lanes, lit) {
                    content = content.child(dots);
                }
            }
            _ => {}
        }

        if picture {
            if box_w > 40. {
                content = content.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(NAME_STRIP))
                        .px(px(6.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .bg(rgba(0x00000059))
                        .text_size(px(theme::XS))
                        .text_color(theme::text())
                        .when(clip.speed != 1., |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .px(px(3.))
                                    .rounded(px(3.))
                                    .bg(rgba(0x00000080))
                                    .child(mono(speed_badge(clip), theme::XXS, theme::text())),
                            )
                        })
                        // A swapped face shows: the clip says it's AI-edited.
                        .when(clip.active_face_swap().is_some(), |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .px(px(3.))
                                    .rounded(px(3.))
                                    .bg(rgba(0x00000080))
                                    .child(mono("AI FACE", theme::XXS, theme::text())),
                            )
                        })
                        .child(
                            div()
                                .flex_none()
                                .max_w(px((box_w - 90.).max(20.)))
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(clip.label()),
                        )
                        .child(mono(
                            timecode(clip.timeline_duration, self.fps()),
                            theme::XXS,
                            rgba(0xffffffb8),
                        )),
                );
            }
        } else if clip.kind != "sticker" {
            let audio = clip.kind == "audio";
            content = content.child(
                div()
                    .absolute()
                    .inset_0()
                    .px(px(7.))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .max_w_full()
                            .truncate()
                            .text_size(px(theme::SM))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text())
                            .when(audio, |d| d.px(px(4.)).rounded(px(3.)).bg(rgba(0x14202a8c)))
                            .child(clip.label()),
                    ),
            );
        }

        let muted =
            matches!(clip.kind.as_str(), "video" | "audio") && (clip.muted() || track.muted);
        let badge = || {
            div()
                .absolute()
                .p(px(2.))
                .rounded(px(3.))
                .bg(rgba(0x0000008c))
        };
        let handles = single && !locked && !g.moving;
        let handle = |side: Side, cx: &mut Context<Self>| {
            let id = id.clone();
            div()
                .id(SharedString::from(format!(
                    "trim-{}-{}",
                    if side == Side::Start { "l" } else { "r" },
                    id
                )))
                .absolute()
                .top_0()
                .bottom_0()
                .w(px(10.))
                .map(|d| match side {
                    Side::Start => d.left(px(-2.)).rounded_l(px(4.)),
                    Side::End => d.right(px(-2.)).rounded_r(px(4.)),
                })
                .bg(theme::text())
                .flex()
                .items_center()
                .justify_center()
                .cursor(CursorStyle::ResizeLeftRight)
                .child(
                    div()
                        .w(px(2.))
                        .h(px(16f32.min(g.h - 8.).max(4.)))
                        .rounded(px(2.))
                        .bg(rgba(0x1a160880)),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        ed.start_clip_drag(&id, ClipDragMode::Trim(side), e.position, cx);
                    }),
                )
        };
        let (down_id, menu_id) = (id.clone(), id.clone());
        let diamonds = if selected && !g.moving {
            self.keyframe_diamonds(clip, -g.inset.0, g.h, cx)
        } else {
            vec![]
        };

        div()
            .id(SharedString::from(format!("clip-{id}")))
            .absolute()
            .left(px(box_x))
            .top(px(g.y))
            .w(px(box_w))
            .h(px(g.h))
            .group("clip")
            .when(hidden, |d| d.opacity(0.35))
            .when(g.moving, |d| d.opacity(0.72))
            .when(!locked && self.tool == Tool::Select, |d| d.cursor_pointer())
            .when(self.tool == Tool::Split && !locked, |d| {
                d.cursor(CursorStyle::Crosshair)
            })
            .child(content)
            .child(div().absolute().inset_0().rounded(px(4.)).map(|d| {
                if selected || g.moving {
                    d.border_2().border_color(theme::selected())
                } else {
                    d.border_1()
                        .border_color(gpui::transparent_black())
                        .group_hover("clip", |s| s.border_color(rgba(0xffffffb3)))
                }
            }))
            .when(locked, |d| {
                d.child(
                    badge()
                        .right(px(4.))
                        .top(px(3.))
                        .child(Icon::LockClosed.el(10., theme::text())),
                )
            })
            .when(muted, |d| {
                d.child(
                    badge()
                        .left(px(4.))
                        .bottom(px(3.))
                        .child(Icon::VolumeMute.el(10., theme::text())),
                )
            })
            .children(diamonds)
            .when(handles, |d| {
                d.child(handle(Side::Start, cx))
                    .child(handle(Side::End, cx))
            })
            // A locked clip isn't a timeline item: presses fall through to
            // the lane (seek, the lane menu), as on empty space.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                    if locked {
                        return;
                    }
                    cx.stop_propagation();
                    ed.end_hover();
                    if ed.tool == Tool::Split {
                        let t = ed.time_at(e.position.x);
                        ed.split_clip_at(&down_id, t, cx);
                        return;
                    }
                    if e.modifiers.shift || e.modifiers.platform {
                        // ⇧ / ⌘ click: in or out of the selection, no move.
                        ed.selected.toggle(&down_id);
                        cx.notify();
                        return;
                    }
                    ed.selected.set(down_id.clone());
                    ed.start_clip_drag(&down_id, ClipDragMode::Move, e.position, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                    if locked {
                        return;
                    }
                    cx.stop_propagation();
                    ed.end_hover();
                    // Inside a multi-selection it stays (the menu's Delete
                    // takes them all); otherwise it alone.
                    if ed.selected.contains(&menu_id) {
                        ed.selected.add(menu_id.clone());
                    } else {
                        ed.selected.set(menu_id.clone());
                    }
                    ed.open_clip_menu(&menu_id, e.position, cx);
                }),
            )
            .into_any_element()
    }

    /// The selected clip's keyframe diamonds (box x = `shift` + time), on
    /// its vertical middle; the one under the playhead is red. Diamonds
    /// closer than one diamond apart show as the first of the run.
    fn keyframe_diamonds(
        &self,
        clip: &Clip,
        shift: f32,
        h: f32,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let pps = self.pps;
        let mut times: Vec<Micros> = vec![];
        for t in clip.keyframe_times(KEYFRAME_TIME_EPSILON) {
            let close = times
                .last()
                .is_some_and(|&last| (t - last) as f32 / SECOND as f32 * pps < DIAMOND);
            if !close {
                times.push(t);
            }
        }
        if times.is_empty() {
            return vec![];
        }
        let dragged = match &self.drag {
            Some(Drag::Keyframe(d)) if d.clip_id == clip.id && d.started => Some((d.from, d.live)),
            _ => None,
        };
        let local = self.playhead - clip.timeline_start;
        let eps = keyframe_eps(pps);
        let active = (0..=clip.timeline_duration)
            .contains(&local)
            .then(|| {
                times
                    .iter()
                    .copied()
                    .filter(|t| (t - local).abs() <= eps)
                    .min_by_key(|t| (t - local).abs())
            })
            .flatten();
        let mut out: Vec<(bool, AnyElement)> = times
            .into_iter()
            .map(|t| {
                let shown = match dragged {
                    Some((from, live)) if from == t => live,
                    _ => t,
                };
                let on = active == Some(t) && dragged.is_none();
                let x = shift + shown as f32 / SECOND as f32 * pps;
                let id = clip.id.clone();
                let (outer, inner) = if on {
                    (theme::text(), theme::danger())
                } else {
                    (rgba(0x0000008c), theme::text())
                };
                let el = div()
                    .id(SharedString::from(format!("kf-{}-{t}", clip.id)))
                    .absolute()
                    .left(px(x - DIAMOND_HIT / 2.))
                    .top(px(h / 2. - DIAMOND_HIT / 2.))
                    .size(px(DIAMOND_HIT))
                    .cursor(CursorStyle::ResizeLeftRight)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            ed.start_keyframe_drag(&id, t, e.position, cx);
                        }),
                    )
                    .child(
                        Icon::Rhombus
                            .el(DIAMOND + 3., outer)
                            .absolute()
                            .left(px((DIAMOND_HIT - DIAMOND - 3.) / 2.))
                            .top(px((DIAMOND_HIT - DIAMOND - 3.) / 2.)),
                    )
                    .child(
                        Icon::Rhombus
                            .el(DIAMOND - 2., inner)
                            .absolute()
                            .left(px((DIAMOND_HIT - DIAMOND + 2.) / 2.))
                            .top(px((DIAMOND_HIT - DIAMOND + 2.) / 2.)),
                    )
                    .into_any_element();
                (on, el)
            })
            .collect();
        // The red one draws over its neighbours.
        out.sort_by_key(|(on, _)| *on);
        out.into_iter().map(|(_, e)| e).collect()
    }

    /// An audio clip's waveform: one bar per `WAVE_STEP` px, the loudest
    /// peak in the stretch of sound it covers, scaled by the clip volume.
    fn waveform(
        &self,
        clip: &Clip,
        width: f32,
        shift: f32,
        lanes: Option<Bounds<Pixels>>,
    ) -> Option<impl IntoElement + use<>> {
        let wave: Arc<Waveform> = self.waves.get(clip.source.as_ref()?)?.clone()?;
        let pps = self.pps;
        let speed = if clip.speed > 0. { clip.speed } else { 1. };
        let source_start = clip.source_start.unwrap_or(0) as f64;
        let gain = if clip.muted() {
            0.
        } else {
            clip.volume() as f32
        };
        let color = theme::clip_audio_wave();
        Some(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let left = f32::from(bounds.origin.x) + shift;
                    let mid = f32::from(bounds.center().y);
                    let half = (f32::from(bounds.size.height) / 2. - 3.).max(0.75);
                    let (lo, hi) = lanes.map_or((f32::MIN, f32::MAX), |b| {
                        (f32::from(b.origin.x), f32::from(b.origin.x + b.size.width))
                    });
                    let per_px = SECOND as f64 / pps as f64;
                    let peak_at =
                        |t: f64| (source_start + t * speed) * wave.per_second / SECOND as f64;
                    let columns = (width / WAVE_STEP).ceil() as usize;
                    for i in 0..columns {
                        let x0 = i as f32 * WAVE_STEP;
                        let wx = left + x0;
                        if wx + WAVE_STEP < lo || wx > hi {
                            continue;
                        }
                        let first = peak_at(x0 as f64 * per_px).floor().max(0.) as usize;
                        let last = (peak_at((x0 + WAVE_STEP) as f64 * per_px).ceil() as usize)
                            .saturating_sub(1)
                            .max(first);
                        let peak = wave
                            .peaks
                            .get(first..=last.min(wave.peaks.len().saturating_sub(1)))
                            .map_or(0., |s| s.iter().copied().fold(0., f32::max));
                        let a = ((peak * gain).min(1.) * half).max(0.75);
                        window.paint_quad(fill(
                            Bounds::new(
                                point(px(wx), px(mid - a)),
                                size(px(WAVE_STEP), px(a * 2.)),
                            ),
                            color,
                        ));
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
    }

    /// A lane's track header: its icon, then lock / eye / speaker / "…".
    fn lane_header(&self, lane: &LaneLayout, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let targets = lane.targets.clone();
        let locked = targets.flag_on(&self.project, LaneFlag::Locked);
        let hidden = targets.flag_on(&self.project, LaneFlag::Hidden);
        let muted = targets.flag_on(&self.project, LaneFlag::Muted);
        let key = format!(
            "{}-{:?}",
            lane.track_ids.first().cloned().unwrap_or_default(),
            lane.kind
        );
        let cover = (lane.kind == LaneKind::Main).then(|| self.cover_tile(cx));
        let toggle = |flag: LaneFlag, on: bool, icon: Icon, label: &'static str| {
            let targets = targets.clone();
            icon_button(SharedString::from(format!("{label}-{key}")), icon, label)
                .size(15.)
                .lit(on)
                .on_click(
                    cx.listener(move |ed, _, _, cx| ed.toggle_lane_flag(targets.clone(), flag, cx)),
                )
        };
        let more_targets = targets.clone();
        let captions = lane.kind == LaneKind::Captions;
        div()
            .size_full()
            .pl_2()
            .pr_1()
            .flex()
            .items_center()
            .gap(px(1.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .map(|d| match cover {
                        Some(tile) => d.child(tile),
                        None => d.child(lane.kind.icon().el(16., theme::text_tertiary())),
                    })
                    .id(SharedString::from(format!("lane-icon-{key}")))
                    .tooltip(crate::ui::widgets::tooltip(lane.kind.name(), None))
                    // The Captions icon picks the caption under the playhead.
                    .when(captions, |d| {
                        d.cursor_pointer()
                            .on_click(cx.listener(|ed, _, _, cx| ed.select_caption_at_playhead(cx)))
                    }),
            )
            .child(toggle(
                LaneFlag::Locked,
                locked,
                if locked {
                    Icon::LockClosed
                } else {
                    Icon::LockOpen
                },
                if locked { "Unlock track" } else { "Lock track" },
            ))
            .child(toggle(
                LaneFlag::Hidden,
                hidden,
                if hidden { Icon::EyeOff } else { Icon::Eye },
                if hidden { "Show track" } else { "Hide track" },
            ))
            .when(lane.kind.has_sound(), |d| {
                d.child(toggle(
                    LaneFlag::Muted,
                    muted,
                    if muted {
                        Icon::VolumeMute
                    } else {
                        Icon::VolumeHigh
                    },
                    if muted { "Unmute track" } else { "Mute track" },
                ))
            })
            .child(
                div()
                    .id(SharedString::from(format!("lane-more-{key}")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            ed.open_lane_options(more_targets.clone(), e.position, cx);
                        }),
                    )
                    .child(
                        icon_button(
                            SharedString::from(format!("more-{key}")),
                            Icon::Ellipsis,
                            "More",
                        )
                        .size(15.),
                    ),
            )
    }

    /// The main lane's Cover tile: the cover (or first frame) under a shade
    /// with ✎ Cover. The cover editor isn't ported yet.
    fn cover_tile(&self, cx: &mut Context<Self>) -> AnyElement {
        let picture = match cover_source(&self.project) {
            Some(CoverSource::Still(path)) => self.thumbs.get(&path).cloned(),
            Some(CoverSource::Frame(src, t)) => self.tiles.get(&(src, t)).cloned(),
            None => None,
        };
        let size = MAIN_LANE_HEIGHT - 8.;
        div()
            .id("cover-tile")
            .relative()
            .flex_none()
            .size(px(size))
            .rounded(px(6.))
            .overflow_hidden()
            .bg(theme::clip_video())
            .cursor_pointer()
            .active(|s| s.opacity(0.6))
            .tooltip(crate::ui::widgets::tooltip("Edit cover", None))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|ed, _, _, cx| {
                ed.toast("Cover editor isn't in the native editor yet", cx);
            }))
            .when_some(picture, |d, image| {
                d.child(
                    img(image)
                        .absolute()
                        .inset_0()
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(2.))
                    .bg(rgba(0x00000059))
                    .child(Icon::Pencil.el(13., theme::text()))
                    .child(
                        div()
                            .text_size(px(theme::XXS))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::text())
                            .child(t("Cover")),
                    ),
            )
            .into_any_element()
    }

    /// The Captions lane icon: selects the caption under the playhead,
    /// else the next one, else the last.
    pub fn select_caption_at_playhead(&mut self, cx: &mut Context<Self>) {
        let now = self.playhead;
        let mut captions: Vec<&Clip> = self
            .project
            .tracks
            .iter()
            .filter(|t| !t.locked)
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.is_caption() && !c.locked())
            .collect();
        captions.sort_by_key(|c| c.timeline_start);
        let pick = captions
            .iter()
            .find(|c| now < c.end())
            .or(captions.last())
            .map(|c| c.id.clone());
        if let Some(id) = pick {
            self.selected.set(id);
            cx.notify();
        }
    }

    /// A header switch over everything a lane covers: on for all when any
    /// is off, else off. One undo step; locking drops the lane's clips from
    /// the selection.
    pub fn toggle_lane_flag(
        &mut self,
        targets: LaneTargets,
        flag: LaneFlag,
        cx: &mut Context<Self>,
    ) {
        let on = !targets.flag_on(&self.project, flag);
        if flag == LaneFlag::Locked && on {
            let lane = targets.clip_ids(&self.project);
            self.selected.retain(|id| !lane.iter().any(|c| c == id));
        }
        let label = match flag {
            LaneFlag::Locked => "Lock Track",
            LaneFlag::Hidden => "Hide Track",
            LaneFlag::Muted => "Mute Track",
        };
        self.edit(
            label,
            false,
            None,
            |p| {
                for t in &mut p.tracks {
                    if targets.track_ids.contains(&t.id) {
                        *match flag {
                            LaneFlag::Locked => &mut t.locked,
                            LaneFlag::Hidden => &mut t.hidden,
                            LaneFlag::Muted => &mut t.muted,
                        } = on;
                    }
                    for c in &mut t.clips {
                        if targets.clip_ids.contains(&c.id) {
                            c.set_flag(flag.key(), on);
                        }
                    }
                }
                true
            },
            cx,
        );
    }
}

/// An audio clip's beat markers as dots along its bottom edge, bigger for
/// stronger beats (monochrome, like the rest of the editor). `lit`: the
/// selected clip's beat under the playhead (what Delete beat removes),
/// drawn bigger in red.
fn beat_dots(
    clip: &Clip,
    shift: f32,
    pps: f32,
    lanes: Option<Bounds<Pixels>>,
    lit: Option<Micros>,
) -> Option<impl IntoElement + use<>> {
    let beats = clip.beats();
    if beats.is_empty() {
        return None;
    }
    let start = clip.timeline_start;
    Some(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let left = f32::from(bounds.origin.x) + shift;
                let y = f32::from(bounds.origin.y + bounds.size.height) - BEAT_DOT_INSET;
                let (lo, hi) = lanes.map_or((f32::MIN, f32::MAX), |b| {
                    (f32::from(b.origin.x), f32::from(b.origin.x + b.size.width))
                });
                for &(t, strength) in &beats {
                    let x = left + (t - start) as f32 / SECOND as f32 * pps;
                    let (r, color) = match strength {
                        _ if Some(t) == lit => (3.5, theme::danger()),
                        BeatStrength::Weak => (1.6, rgba(0xffffffbf)),
                        BeatStrength::Medium => (2.3, rgba(0xffffffff)),
                        BeatStrength::Strong => (3., rgba(0xffffffff)),
                    };
                    if x + r < lo || x - r > hi {
                        continue;
                    }
                    window.paint_quad(
                        fill(
                            Bounds::new(point(px(x - r), px(y - r)), size(px(r * 2.), px(r * 2.))),
                            color,
                        )
                        .corner_radii(px(r)),
                    );
                }
            },
        )
        .absolute()
        .inset_0(),
    )
}

/// One filmstrip tile at clip-box x `x`, `w` wide.
fn tile(image: Arc<gpui::RenderImage>, x: f32, w: f32) -> impl IntoElement {
    div()
        .absolute()
        .left(px(x))
        .top_0()
        .bottom_0()
        .w(px(w + 0.5))
        .overflow_hidden()
        .child(
            img(image)
                .absolute()
                .inset_0()
                .size_full()
                .object_fit(ObjectFit::Cover),
        )
}

/// The white insert line at a main-lane cut.
fn insert_line(x: f32) -> AnyElement {
    div()
        .absolute()
        .left(px(x - 1.))
        .top(px(MAIN_TOP - 4.))
        .h(px(MAIN_LANE_HEIGHT + 8.))
        .w(px(2.))
        .rounded(px(1.))
        .bg(theme::text())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MediaItem, Project};

    fn clip(id: &str, start: i64, dur: i64) -> Clip {
        serde_json::from_value(serde_json::json!({
            "id": id, "kind": "text", "timelineStart": start * SECOND,
            "timelineDuration": dur * SECOND
        }))
        .unwrap()
    }

    #[test]
    fn overlapping_clips_get_sub_rows() {
        let clips = [
            clip("a", 0, 4),
            clip("b", 2, 4),
            clip("c", 4, 2),
            clip("d", 3, 1),
        ];
        let (rows, count) = assign_rows(clips.iter());
        assert_eq!(count, 3);
        assert_eq!(rows["a"], 0);
        assert_eq!(rows["b"], 1);
        assert_eq!(rows["d"], 2);
        assert_eq!(rows["c"], 0, "a has ended by 4 s");
    }

    #[test]
    fn stack_shows_only_lanes_with_clips() {
        let mut p = Project::empty();
        let (_, stack, h) = lane_layouts(&p);
        assert!(stack.is_empty());
        assert_eq!(h, 0.);
        let sound = MediaItem {
            path: "/tmp/a.m4a".into(),
            name: "a.m4a".into(),
            duration: 2 * SECOND,
            width: 0,
            height: 0,
            has_video: false,
            still: false,
        };
        p.insert_media_at(&sound, 0);
        p.insert_media_at(&sound, SECOND);
        let (_, stack, h) = lane_layouts(&p);
        assert_eq!(stack.len(), 1);
        assert_eq!(stack[0].kind, LaneKind::Audio);
        assert_eq!(stack[0].height, LANE_HEIGHT * 2. + LANE_GAP);
        assert_eq!(h, stack[0].height + LANE_GAP);
    }

    #[test]
    fn captions_get_their_own_one_row_lane() {
        let mut p = Project::empty();
        p.tracks.push(
            serde_json::from_value(serde_json::json!({
                "id": "text", "type": "text", "name": "Text", "clips": [], "order": 3,
                "muted": false, "hidden": false, "locked": false
            }))
            .unwrap(),
        );
        let text = p.tracks.len() - 1;
        let cap = |id: &str, start: i64| {
            let mut c = clip(id, start, 2);
            c.extra.insert("caption".into(), serde_json::json!({}));
            c
        };
        let (a, b) = (cap("c1", 0), cap("c2", 1)); // overlapping: still one row
        let title = clip("t1", 0, 3);
        p.tracks[text].clips.extend([a, b, title]);
        let (_, stack, _) = lane_layouts(&p);
        let kinds: Vec<LaneKind> = stack.iter().map(|l| l.kind).collect();
        assert_eq!(kinds, [LaneKind::Captions, LaneKind::Text]);
        assert_eq!(stack[0].height, LANE_HEIGHT);
        assert_eq!(stack[0].targets.clip_ids, ["c1", "c2"]);
        assert!(stack[0].targets.track_ids.is_empty());
        assert_eq!(stack[1].targets.clip_ids, ["t1"]);
        assert!(stack[1].holds(&p.tracks[text].clips[2]));
        assert!(!stack[1].holds(&p.tracks[text].clips[0]));

        // Locking the Captions lane locks the captions only.
        for c in &mut p.tracks[text].clips[..2] {
            c.set_flag("locked", true);
        }
        assert!(stack[0].targets.flag_on(&p, LaneFlag::Locked));
        assert!(!stack[1].targets.flag_on(&p, LaneFlag::Locked));
    }

    #[test]
    fn filmstrip_frames_snap_to_a_grid_inside_the_segment() {
        let mut c = clip("v", 0, 10);
        c.kind = "video".into();
        c.source_start = Some(2 * SECOND);
        let (tw, times) = filmstrip(&c, 440.);
        assert_eq!(times.len(), 10);
        assert!((tw - 44.).abs() < 0.01);
        // On the 1 s grid, except the last one, kept off the segment's end.
        assert!(times[..9].iter().all(|t| t % 1_000_000 == 0));
        assert_eq!(times[9], 12 * SECOND - SEGMENT_END_GUARD);
        assert!(times[0] >= 2 * SECOND && *times.last().unwrap() < 12 * SECOND);
    }

    #[test]
    fn deep_zoom_frames_stay_on_a_grid() {
        let mut c = clip("v", 0, 10);
        c.kind = "video".into();
        // ~400 and ~420 px a second: tiles closer than the 125 ms grid.
        let (_, a) = filmstrip(&c, 4000.);
        let (_, b) = filmstrip(&c, 4200.);
        let on_grid = |t: &Micros| t % 31_250 == 0 || *t == 10 * SECOND - SEGMENT_END_GUARD;
        assert!(a.iter().all(on_grid) && b.iter().all(on_grid));
        // A zoom step mostly reuses the frames it already has.
        let shared = b.iter().filter(|t| a.contains(t)).count();
        assert!(shared * 2 > b.len(), "{shared} of {}", b.len());
    }

    #[test]
    fn seams_split_a_transition_overlap() {
        let mut a = clip("a", 0, 4);
        a.extra.insert(
            "outTransition".into(),
            serde_json::json!({ "durationMicros": SECOND }),
        );
        let b = clip("b", 3, 4);
        let seams = transition_seams(&[&a, &b]);
        assert_eq!(seams["a"], (0, SECOND / 2));
        assert_eq!(seams["b"], (SECOND / 2, 0));
    }

    #[test]
    fn cover_is_the_saved_one_else_the_first_frame() {
        let mut p = Project::empty();
        assert_eq!(cover_source(&p), None);
        let mut v = clip("v", 0, 4);
        v.kind = "video".into();
        v.source = Some("/m/a.mp4".into());
        v.source_start = Some(2 * SECOND);
        p.tracks[0].clips.push(v);
        assert_eq!(
            cover_source(&p),
            Some(CoverSource::Frame("/m/a.mp4".into(), 2 * SECOND))
        );
        p.extra.insert(
            "cover".into(),
            serde_json::json!({ "imageUri": "file:///covers/c.jpg" }),
        );
        assert_eq!(
            cover_source(&p),
            Some(CoverSource::Still("/covers/c.jpg".into()))
        );
    }

    #[test]
    fn ruler_labels() {
        assert_eq!(ruler_label(2.), "00:02");
        assert_eq!(ruler_label(61.5), "01:01.5");
    }
}

/// A sped clip's tag: its ramp's name, else its speed ("2×", "0.45×").
fn speed_badge(clip: &Clip) -> String {
    if let Some(c) = clip.speed_curve() {
        return crate::speed_curve::label(&c.preset).to_string();
    }
    let s = clip.speed;
    let text = if s < 1. {
        format!("{s:.2}")
    } else {
        format!("{s:.1}")
    };
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("{text}×")
}
