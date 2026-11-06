//! The timeline's mouse edits, after the old desktop editor's
//! (`TimelineClip.tsx`, `desktopTimelineEdits.ts`, `useDesktopTimelineMouse.ts`,
//! `useDesktopTimelineDrop.ts`, `DesktopTimelineContextMenu.tsx`):
//!
//! - press-drag on a clip moves it (it gets selected); a picture on the
//!   main or an overlay video lane can also be pulled between them, and on
//!   the main lane with the magnet on it goes in at the nearest cut;
//! - the selected clip's handles trim either edge (never past its media);
//! - moves and trims snap to 00:00 and other clips' edges (auto snapping);
//! - held near the lane area's edges, a drag or scrub scrolls the timeline;
//! - library tiles and files from the desktop drop onto it: on the ruler
//!   or main lane they go in at the cut nearest the pointer, below it on an
//!   overlay lane from that time;
//! - right-click menus for a clip, empty lane space and a lane's "…";
//! - the wheel scrolls (the lane stack vertically when it overflows), ⇧ +
//!   wheel sideways, ⌘ / ctrl + wheel zooms around the pointer.

use std::time::Instant;

use gpui::{
    Context, FontWeight, Pixels, Point, ScrollWheelEvent, SharedString, Window, div, prelude::*, px,
};

use crate::editor::{Drag, Editor, PopupItem};
use crate::model::{self, Clip, MediaItem, Micros, SECOND, Side};
use crate::selection::Selection;
use crate::shortcuts::Shortcut;
use crate::theme;
use crate::ui::timeline::{
    BOTTOM_PAD, LANE_HEIGHT, MAIN_BOTTOM, MAIN_TOP, MIN_CLIP, SNAP_PX, STACK_TOP, lane_layouts,
};

/// Pointer travel (px) before a press on a clip becomes a move.
const MOVE_THRESHOLD: f32 = 4.;
/// Slack (px) around the main lane before a drag leaves or enters it.
const MAIN_LANE_SLOP: f32 = 6.;
/// Edge auto-scroll: the zone at each side, and its speed range (px/s).
const EDGE_ZONE: f32 = 56.;
const EDGE_MIN_SPEED: f32 = 120.;
const EDGE_MAX_SPEED: f32 = 620.;
/// ⌘ + wheel: delta → zoom factor, and the largest delta one event counts.
const WHEEL_ZOOM_RATE: f32 = 0.012;
const MAX_ZOOM_DELTA: f32 = 40.;
/// Ghost length for files from the desktop (not probed until the drop).
const UNKNOWN_DROP_LENGTH: Micros = 3 * SECOND;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipDragMode {
    Move,
    Trim(Side),
}

/// Where a moved clip lands when it's more than a plain move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipDrop {
    /// A main clip to cut `n` among the other main clips (magnet on).
    Reorder(usize),
    /// An overlay picture into the main lane at cut `n` (magnet on).
    IntoMain(usize),
    /// An overlay picture onto the main lane where it was let go (magnet off).
    IntoMainAt,
    /// A main clip down onto an overlay lane, where it was let go.
    OutOfMain,
}

/// A clip being moved or trimmed with the mouse.
pub struct ClipDrag {
    pub clip_id: String,
    pub track_id: String,
    pub mode: ClipDragMode,
    origin: Point<Pixels>,
    /// The horizontal scroll when it began (an edge auto-scroll adds to the move).
    origin_scroll: f32,
    /// The clip as committed.
    pub start: Micros,
    pub duration: Micros,
    /// Where it is while dragged (snapped).
    pub live_start: Micros,
    pub live_duration: Micros,
    /// How far the pointer moved down (a picture following it between lanes).
    pub dy: f32,
    /// Past the move threshold (a trim starts at once).
    pub started: bool,
    /// A picture on the main or an overlay video lane: can change lanes.
    pub lane_movable: bool,
    pub from_main: bool,
    /// The edge time the drag snapped to (the guide line).
    pub snapped: Option<Micros>,
    pub drop: Option<ClipDrop>,
    /// The insert line on the main lane for `drop`.
    pub insert_at: Option<Micros>,
    targets: Vec<Micros>,
    /// Longest the clip may get (its media runs out).
    max_duration: Option<Micros>,
}

impl ClipDrag {
    pub fn live_end(&self) -> Micros {
        self.live_start + self.live_duration
    }
}

/// A rubber band: from where the press was to the pointer (window
/// points); the clips it touches join what was selected before.
pub struct Marquee {
    pub origin: Point<Pixels>,
    pub to: Point<Pixels>,
    base: Selection,
}

/// Where a file dragged over the timeline would land.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MediaDrop {
    /// Into the main lane as clip `index` (None: at the end); the cut is at `at`.
    Main { index: Option<usize>, at: Micros },
    /// Off the main lane (overlay or audio) from `at`, `length` long.
    Lane { at: Micros, length: Micros },
}

/// A media library tile being dragged.
#[derive(Clone)]
pub struct MediaDrag(pub MediaItem);

/// What follows the pointer while a library tile is dragged.
pub struct DragChip {
    name: SharedString,
}

impl DragChip {
    pub fn new(item: &MediaItem) -> Self {
        DragChip {
            name: item.name.clone().into(),
        }
    }
}

impl Render for DragChip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(10.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(theme::button_bg())
            .border_1()
            .border_color(theme::text_secondary())
            .opacity(0.9)
            .text_size(px(theme::XS))
            .font_family(theme::FONT_BODY)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::text())
            .child(self.name.clone())
    }
}

/// The nearest target within `threshold` of `t`, if any.
pub fn snap(t: Micros, targets: &[Micros], threshold: Micros) -> Option<Micros> {
    targets
        .iter()
        .copied()
        .filter(|&s| (s - t).abs() <= threshold)
        .min_by_key(|&s| (s - t).abs())
}

/// Edge auto-scroll speed (px/s, negative = left) for the pointer at `x` in
/// a lane area `width` wide.
pub fn edge_speed(x: f32, width: f32) -> f32 {
    let ramp = |depth: f32| {
        EDGE_MIN_SPEED + (EDGE_MAX_SPEED - EDGE_MIN_SPEED) * (depth / EDGE_ZONE).min(1.)
    };
    if x < EDGE_ZONE {
        -ramp(EDGE_ZONE - x)
    } else if x > width - EDGE_ZONE {
        ramp(x - (width - EDGE_ZONE))
    } else {
        0.
    }
}

impl Editor {
    fn snap_threshold(&self) -> Micros {
        (SNAP_PX / self.pps * SECOND as f32) as Micros
    }

    /// What a dragged edge snaps to: 00:00 and every other clip's edges.
    fn snap_targets(&self, exclude: &str) -> Vec<Micros> {
        let mut targets = vec![0];
        for c in self.project.tracks.iter().flat_map(|t| t.clips.iter()) {
            if c.id != exclude {
                targets.extend([c.timeline_start, c.end()]);
            }
        }
        targets
    }

    /// The longest a trim may make `clip` from `side`: its source window
    /// can't run before the media's start or past its end. None: no limit
    /// (stills, text…, or media whose length isn't known).
    fn trim_limit(&self, clip: &Clip, side: Side) -> Option<Micros> {
        let has_window = clip.kind == "audio" || clip.kind == "video" && !clip.freeze();
        if !has_window {
            return None;
        }
        let speed = if clip.speed > 0. { clip.speed } else { 1. };
        let source_start = clip.source_start.unwrap_or(0);
        // Extending this side pulls the source start earlier (a reversed
        // clip walks its segment backwards, so the sides swap).
        let reversed = clip.kind == "video" && clip.reversed();
        let pulls_start = if reversed {
            side == Side::End
        } else {
            side == Side::Start
        };
        let limit = if pulls_start {
            clip.timeline_duration + (source_start as f64 / speed) as Micros
        } else {
            let length = self
                .media
                .iter()
                .find(|m| Some(&m.path) == clip.source.as_ref())?
                .duration;
            ((length - source_start).max(0) as f64 / speed) as Micros
        };
        Some(limit.max(clip.timeline_duration))
    }

    /// A press on a clip (Move) or on one of its trim handles.
    pub fn start_clip_drag(
        &mut self,
        clip_id: &str,
        mode: ClipDragMode,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some((track, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let from_main = self.project.is_main_track(&track.id);
        let lane_movable = matches!(clip.kind.as_str(), "video" | "image")
            && (from_main || track.is_overlay_video(&self.project.id));
        let max_duration = match mode {
            ClipDragMode::Trim(side) => self.trim_limit(clip, side),
            ClipDragMode::Move => None,
        };
        let drag = ClipDrag {
            clip_id: clip.id.clone(),
            track_id: track.id.clone(),
            mode,
            origin: at,
            origin_scroll: self.scroll_x,
            start: clip.timeline_start,
            duration: clip.timeline_duration,
            live_start: clip.timeline_start,
            live_duration: clip.timeline_duration,
            dy: 0.,
            started: matches!(mode, ClipDragMode::Trim(_)),
            lane_movable,
            from_main,
            snapped: None,
            drop: None,
            insert_at: None,
            targets: if self.prefs.snapping {
                self.snap_targets(clip_id)
            } else {
                vec![]
            },
            max_duration,
        };
        self.end_hover();
        self.drag = Some(Drag::Clip(Box::new(drag)));
        cx.notify();
    }

    /// The pointer moved (or the timeline scrolled under it) mid drag.
    pub fn update_clip_drag(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(Drag::Clip(mut d)) = self.drag.take() else {
            return;
        };
        if !d.started {
            let moved = f32::from(at.x - d.origin.x)
                .abs()
                .max(f32::from(at.y - d.origin.y).abs());
            if moved < MOVE_THRESHOLD {
                self.drag = Some(Drag::Clip(d));
                return;
            }
            d.started = true;
        }
        let dx = f32::from(at.x - d.origin.x) + (self.scroll_x - d.origin_scroll);
        let dt = (dx / self.pps * SECOND as f32) as Micros;
        let threshold = self.snap_threshold();
        let snap = |t: Micros| snap(t, &d.targets, threshold);
        let floor = MIN_CLIP.min(d.duration);
        let ceiling = d.max_duration.unwrap_or(Micros::MAX);
        match d.mode {
            ClipDragMode::Move => {
                // Either edge may snap; the nearer one wins.
                let start = (d.start + dt).max(0);
                let by_start = snap(start).map(|s| (s - start, s));
                let by_end = snap(start + d.duration).map(|e| (e - start - d.duration, e));
                let best = match (by_start, by_end) {
                    (Some(a), Some(b)) => Some(if a.0.abs() <= b.0.abs() { a } else { b }),
                    (a, b) => a.or(b),
                };
                d.live_start = (start + best.map_or(0, |b| b.0)).max(0);
                d.snapped = best.map(|b| b.1);
            }
            ClipDragMode::Trim(Side::End) => {
                let end = d.start + d.duration + dt;
                let snapped = snap(end);
                d.live_duration = (snapped.unwrap_or(end) - d.start).clamp(floor, ceiling);
                d.snapped = snapped.filter(|&e| e == d.live_end());
            }
            ClipDragMode::Trim(Side::Start) => {
                let end = d.start + d.duration;
                let start = (d.start + dt).max(0);
                let snapped = snap(start);
                d.live_duration = (end - snapped.unwrap_or(start)).clamp(floor, ceiling);
                d.live_start = end - d.live_duration;
                d.snapped = snapped.filter(|&s| s == d.live_start);
            }
        }
        if d.mode == ClipDragMode::Move && d.lane_movable {
            d.dy = f32::from(at.y - d.origin.y);
            (d.drop, d.insert_at) = self.resolve_clip_drop(&d, at);
        }
        self.drag = Some(Drag::Clip(d));
        cx.notify();
    }

    /// The old editor's drop rules for a picture dragged with the pointer at
    /// `at`: a main clip stays on the main lane (at the nearest cut while the
    /// magnet is on) unless pulled below it onto an overlay lane; an overlay
    /// picture pulled onto the main lane goes in at the nearest cut. Anything
    /// else is a plain move (None).
    fn resolve_clip_drop(
        &self,
        d: &ClipDrag,
        at: Point<Pixels>,
    ) -> (Option<ClipDrop>, Option<Micros>) {
        let Some(b) = self.measured("lanes") else {
            return (None, None);
        };
        let y = f32::from(at.y - b.origin.y);
        let time = self.time_at(at.x).max(0);
        let magnet = self.prefs.main_track_magnet;
        if d.from_main {
            if y > MAIN_BOTTOM + MAIN_LANE_SLOP {
                return (Some(ClipDrop::OutOfMain), None);
            }
            if !magnet {
                return (None, None);
            }
            let others: Vec<&Clip> = self
                .project
                .ordered_main_clips()
                .into_iter()
                .filter(|c| c.id != d.clip_id)
                .collect();
            let i = model::nearest_seam_index(&others, time);
            return (Some(ClipDrop::Reorder(i)), Some(model::seam_at(&others, i)));
        }
        if !(MAIN_TOP - MAIN_LANE_SLOP..=MAIN_BOTTOM + MAIN_LANE_SLOP).contains(&y) {
            return (None, None);
        }
        if !magnet {
            return (Some(ClipDrop::IntoMainAt), None);
        }
        let main = self.project.ordered_main_clips();
        let i = model::nearest_seam_index(&main, time);
        (Some(ClipDrop::IntoMain(i)), Some(model::seam_at(&main, i)))
    }

    /// The release: one undoable edit for the whole drag.
    pub fn commit_clip_drag(&mut self, d: ClipDrag, cx: &mut Context<Self>) {
        if !d.started {
            return; // a click: the press already selected it
        }
        let id = d.clip_id.clone();
        match d.mode {
            ClipDragMode::Move => {
                let start = d.live_start;
                match d.drop {
                    Some(ClipDrop::Reorder(i)) => {
                        self.edit("Move Clip", true, None, |p| p.reorder_main(&id, i), cx);
                    }
                    Some(ClipDrop::IntoMain(i)) => {
                        self.edit("Move Clip", true, None, |p| p.move_into_main(&id, i), cx);
                    }
                    Some(ClipDrop::IntoMainAt) => {
                        let Some(main) = self.project.main_track().map(|t| t.id.clone()) else {
                            return;
                        };
                        self.edit(
                            "Move Clip",
                            false,
                            None,
                            |p| p.move_clip(&id, &main, start),
                            cx,
                        );
                    }
                    Some(ClipDrop::OutOfMain) => {
                        let lane = self.project.overlay_lane_for(start, start + d.duration);
                        self.edit(
                            "Move Clip",
                            true,
                            None,
                            |p| p.move_out_of_main(&id, lane, start),
                            cx,
                        );
                    }
                    None => {
                        let track = d.track_id.clone();
                        self.edit(
                            "Move Clip",
                            false,
                            None,
                            |p| p.move_clip(&id, &track, start),
                            cx,
                        );
                    }
                }
                self.selected.set(id);
            }
            ClipDragMode::Trim(side) => {
                let duration = d.live_duration;
                self.edit(
                    "Trim Clip",
                    d.from_main,
                    None,
                    |p| p.trim_edge(&id, side, duration),
                    cx,
                );
            }
        }
        cx.notify();
    }

    // ---- rubber band ---------------------------------------------------------

    /// ⇧ / ⌘ + press on empty lane space below the ruler starts a rubber
    /// band (false: on the ruler, which keeps scrubbing).
    pub fn start_marquee(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some(b) = self.measured("lanes") else {
            return false;
        };
        if f32::from(at.y - b.origin.y) < MAIN_TOP - 4. {
            return false;
        }
        self.drag = Some(Drag::Marquee(Box::new(Marquee {
            origin: at,
            to: at,
            base: self.selected.clone(),
        })));
        cx.notify();
        true
    }

    pub fn update_marquee(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(Drag::Marquee(mut m)) = self.drag.take() else {
            return;
        };
        m.to = at;
        let mut sel = m.base.clone();
        for id in self.clips_in_rect(m.origin, m.to) {
            if !sel.contains(&id) {
                sel.add(id);
            }
        }
        self.selected = sel;
        self.drag = Some(Drag::Marquee(m));
        cx.notify();
    }

    /// The unlocked clips whose boxes the rectangle between window points
    /// `a` and `b` touches.
    pub fn clips_in_rect(&self, a: Point<Pixels>, b: Point<Pixels>) -> Vec<String> {
        let Some(lanes) = self.measured("lanes") else {
            return vec![];
        };
        let top = f32::from(a.y.min(b.y) - lanes.origin.y);
        let bottom = f32::from(a.y.max(b.y) - lanes.origin.y);
        let t0 = self.time_at(a.x.min(b.x));
        let t1 = self.time_at(a.x.max(b.x));
        let p = &self.project;
        let hits = |c: &Clip, y0: f32, y1: f32| {
            y0 < bottom && y1 > top && c.timeline_start < t1 && c.end() > t0
        };
        let usable = |t: &model::Track, c: &Clip| !t.locked && !c.locked();
        let (main, stack, _) = lane_layouts(p);
        let mut out = vec![];
        for tid in &main.track_ids {
            if let Some(t) = p.tracks.iter().find(|t| &t.id == tid) {
                for c in &t.clips {
                    if usable(t, c) && hits(c, MAIN_TOP, MAIN_BOTTOM) {
                        out.push(c.id.clone());
                    }
                }
            }
        }
        // The stack only counts where its window shows it.
        let (top, bottom) = (top.max(STACK_TOP), bottom);
        if bottom > top {
            for lane in &stack {
                for t in p.tracks.iter().filter(|t| lane.track_ids.contains(&t.id)) {
                    for c in &t.clips {
                        let y = STACK_TOP + lane.clip_top(&c.id) - self.lane_scroll_y;
                        if usable(t, c)
                            && y.max(STACK_TOP) < bottom
                            && hits(c, y, y + lane.row_height)
                        {
                            out.push(c.id.clone());
                        }
                    }
                }
            }
        }
        out
    }

    /// How far right the timeline may scroll: until the end (or a dragged
    /// clip's end) sits mid lane area.
    pub fn max_scroll(&self) -> f32 {
        let width = self
            .measured("lanes")
            .map_or(800., |b| f32::from(b.size.width));
        let mut end = self.project.duration;
        if let Some(Drag::Clip(d)) = &self.drag {
            end = end.max(d.live_end());
        }
        (end as f32 / SECOND as f32 * self.pps - width * 0.5).max(0.)
    }

    pub fn clamp_scroll(&mut self) {
        self.scroll_x = self.scroll_x.clamp(0., self.max_scroll());
    }

    /// The lane stack's scroll range (0 while every lane fits).
    pub fn max_lane_scroll(&self) -> f32 {
        let height = self
            .measured("lanes")
            .map_or(0., |b| f32::from(b.size.height));
        let (_, _, content) = lane_layouts(&self.project);
        let window = (height - STACK_TOP - BOTTOM_PAD).max(LANE_HEIGHT);
        (content - window).max(0.)
    }

    /// Zooms to `pps`, keeping the time under lane-area x `x` where it is.
    pub fn zoom_at(&mut self, pps: f32, x: f32, cx: &mut Context<Self>) {
        let t = (x + self.scroll_x) / self.pps;
        self.pps = pps.clamp(crate::editor::MIN_PPS, crate::editor::MAX_PPS);
        self.scroll_x = t * self.pps - x;
        self.clamp_scroll();
        cx.notify();
    }

    /// Runs every frame: while a drag or scrub is held near either edge of
    /// the lane area, the timeline scrolls under it.
    pub fn tick_edge_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = match &self.drag {
            Some(Drag::Clip(d)) => d.started,
            Some(Drag::Scrub) => true,
            _ => false,
        };
        let Some(b) = self.measured("lanes").filter(|_| active) else {
            self.edge_scroll_at = None;
            return;
        };
        let x = f32::from(self.pointer.x - b.origin.x);
        let speed = edge_speed(x, f32::from(b.size.width));
        if speed == 0. {
            self.edge_scroll_at = None;
            return;
        }
        let now = Instant::now();
        let dt = self
            .edge_scroll_at
            .map_or(1. / 60., |t| (now - t).as_secs_f32())
            .min(0.05);
        self.edge_scroll_at = Some(now);
        let before = self.scroll_x;
        self.scroll_x = (self.scroll_x + speed * dt).max(0.);
        self.clamp_scroll();
        if self.scroll_x != before {
            let at = self.pointer;
            match &self.drag {
                Some(Drag::Clip(_)) => self.update_clip_drag(at, cx),
                _ => self.scrub_to(at.x, cx),
            }
        }
        window.request_animation_frame();
    }

    pub fn on_timeline_wheel(&mut self, e: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let d = e.delta.pixel_delta(px(20.));
        let (dx, dy) = (f32::from(d.x), f32::from(d.y));
        let left = self.measured("lanes").map_or(0., |b| f32::from(b.origin.x));
        if e.modifiers.platform || e.modifiers.control {
            let f = (dy.clamp(-MAX_ZOOM_DELTA, MAX_ZOOM_DELTA) * WHEEL_ZOOM_RATE).exp();
            self.zoom_at(self.pps * f, f32::from(e.position.x) - left, cx);
        } else if e.modifiers.shift {
            self.scroll_x -= if dx != 0. { dx } else { dy };
        } else if dy.abs() > dx.abs() && self.max_lane_scroll() > 0. {
            self.lane_scroll_y = (self.lane_scroll_y - dy).clamp(0., self.max_lane_scroll());
        } else {
            self.scroll_x -= if dx != 0. { dx } else { dy };
        }
        self.clamp_scroll();
        // The time under a still pointer changed.
        if self.hover.is_some() {
            self.hover_at(e.position.x, cx);
        }
        cx.notify();
    }

    // ---- hover (preview axis) --------------------------------------------

    /// The pointer over the lanes: the hover line, and with the preview
    /// axis on the player shows that frame (the playhead stays put).
    pub fn hover_at(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let t = self.time_at(x).clamp(0, self.project.duration);
        if self.hover != Some(t) {
            self.hover = Some(t);
            self.request_preview();
            cx.notify();
        }
    }

    /// Puts the player back on the playhead.
    pub fn end_hover(&mut self) {
        if self.hover.take().is_some() {
            self.request_preview();
        }
    }

    // ---- drops from the library / desktop -------------------------------------

    /// Where something dragged to window point `at` would land; None
    /// outside the lane area. `length` is the dragged media's (if known).
    pub fn media_drop_at(
        &self,
        at: Point<Pixels>,
        length: Option<Micros>,
        sound_only: bool,
    ) -> Option<MediaDrop> {
        let b = self.measured("lanes")?;
        if !b.contains(&at) {
            return None;
        }
        let y = f32::from(at.y - b.origin.y);
        let time = self.time_at(at.x).max(0);
        if sound_only || y > MAIN_BOTTOM {
            return Some(MediaDrop::Lane {
                at: time,
                length: length.unwrap_or(UNKNOWN_DROP_LENGTH),
            });
        }
        // Before the first main clip whose middle is past the pointer.
        let main = self.project.ordered_main_clips();
        let index = main
            .iter()
            .position(|c| c.timeline_start + c.timeline_duration / 2 > time);
        let at = match index {
            Some(i) => main[i].timeline_start,
            None => main.last().map_or(0, |c| c.end()),
        };
        Some(MediaDrop::Main { index, at })
    }

    pub fn set_media_drop(&mut self, target: Option<MediaDrop>, cx: &mut Context<Self>) {
        if self.media_drop != target {
            self.media_drop = target;
            cx.notify();
        }
    }

    /// Lands dropped media where `target` says, as one undo step; the last
    /// clip added ends up selected.
    pub fn drop_media(&mut self, items: Vec<MediaItem>, target: MediaDrop, cx: &mut Context<Self>) {
        self.media_drop = None;
        if items.is_empty() {
            return;
        }
        for item in &items {
            self.add_to_library(item.clone(), cx);
        }
        let mut last = None;
        let main = matches!(target, MediaDrop::Main { .. });
        self.edit(
            "Add Clip",
            main,
            None,
            |p| {
                match target {
                    MediaDrop::Main { index, at } => {
                        let mut index = index;
                        for item in &items {
                            if item.has_video {
                                last = Some(p.insert_media(item, index));
                                index = index.map(|i| i + 1);
                            } else {
                                last = Some(p.insert_media_at(item, at));
                            }
                        }
                    }
                    MediaDrop::Lane { at, .. } => {
                        let mut at = at;
                        for item in &items {
                            last = Some(p.insert_media_at(item, at));
                            at += item.duration;
                        }
                    }
                }
                true
            },
            cx,
        );
        self.selected.set_opt(last);
        cx.notify();
    }

    // ---- right-click menus ------------------------------------------------------

    /// On a clip (the right-click selected it): Copy / Cut / Paste / Delete,
    /// Split / Duplicate, Deactivate.
    pub fn open_clip_menu(&mut self, clip_id: &str, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let now = self.playhead;
        let can_split = !matches!(clip.kind.as_str(), "sticker")
            && now > clip.timeline_start
            && now < clip.end();
        let hidden = clip.hidden;
        let can_paste = self.clipboard.is_some();
        let run = |id: Shortcut| {
            move |ed: &mut Editor, w: &mut Window, cx: &mut Context<Editor>| ed.run(id, w, cx)
        };
        let items = vec![
            PopupItem::new("Copy", run(Shortcut::CopyClip)).hint(Shortcut::CopyClip),
            PopupItem::new("Cut", run(Shortcut::CutClip)).hint(Shortcut::CutClip),
            PopupItem::new("Paste", run(Shortcut::PasteClip))
                .hint(Shortcut::PasteClip)
                .disabled(!can_paste),
            PopupItem::new("Delete", run(Shortcut::DeleteClip))
                .hint(Shortcut::DeleteClip)
                .destructive(),
            PopupItem::separator(),
            PopupItem::new("Split", run(Shortcut::SplitClip))
                .hint(Shortcut::SplitClip)
                .disabled(!can_split),
            PopupItem::new("Duplicate", run(Shortcut::Duplicate)).hint(Shortcut::Duplicate),
            PopupItem::new(
                if hidden {
                    "Activate clip"
                } else {
                    "Deactivate clip"
                },
                run(Shortcut::Deactivate),
            )
            .hint(Shortcut::Deactivate),
        ];
        self.open_popup(at, items, cx);
    }

    /// On empty lane space: Paste, Split at the playhead and the timeline's
    /// switches.
    pub fn open_lane_menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let p = &self.prefs;
        let run = |id: Shortcut| {
            move |ed: &mut Editor, w: &mut Window, cx: &mut Context<Editor>| ed.run(id, w, cx)
        };
        let items = vec![
            PopupItem::new("Paste", run(Shortcut::PasteClip))
                .hint(Shortcut::PasteClip)
                .disabled(self.clipboard.is_none()),
            PopupItem::new("Split", run(Shortcut::SplitClip)).hint(Shortcut::SplitClip),
            PopupItem::separator(),
            PopupItem::new("Main track magnet", run(Shortcut::MainMagnet))
                .hint(Shortcut::MainMagnet)
                .checked(p.main_track_magnet),
            PopupItem::new(
                "Linkage",
                |ed: &mut Editor, _: &mut Window, cx: &mut Context<Editor>| {
                    ed.prefs.linkage = !ed.prefs.linkage;
                    ed.prefs.save();
                    cx.notify();
                },
            )
            .checked(p.linkage),
            PopupItem::new("Preview axis", run(Shortcut::PreviewAxis))
                .hint(Shortcut::PreviewAxis)
                .checked(p.preview_axis),
            PopupItem::new("Auto snapping", run(Shortcut::Snapping))
                .hint(Shortcut::Snapping)
                .checked(p.snapping),
            PopupItem::new("Zoom to fit", run(Shortcut::ZoomFit)).hint(Shortcut::ZoomFit),
        ];
        self.open_popup(at, items, cx);
    }

    /// A lane's "…": delete every clip on it (one undo step).
    pub fn open_lane_options(
        &mut self,
        track_ids: Vec<String>,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let tracks: Vec<_> = self
            .project
            .tracks
            .iter()
            .filter(|t| track_ids.contains(&t.id))
            .collect();
        let locked = !tracks.is_empty() && tracks.iter().all(|t| t.locked);
        let ids: Vec<String> = tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.id.clone()))
            .collect();
        let main = tracks.iter().any(|t| self.project.is_main_track(&t.id));
        let empty = ids.is_empty();
        let select_ids = ids.clone();
        let items = vec![
            PopupItem::new(
                "Select all on track",
                move |ed: &mut Editor, _: &mut Window, cx: &mut Context<Editor>| {
                    ed.selected.set_all(select_ids.clone());
                    cx.notify();
                },
            )
            .disabled(locked || empty),
            PopupItem::separator(),
            PopupItem::new(
                "Delete track",
                move |ed: &mut Editor, _: &mut Window, cx: &mut Context<Editor>| {
                    let ids = ids.clone();
                    ed.selected.clear();
                    if ed.edit("Delete Track", main, None, |p| p.delete_clips(&ids), cx) {
                        ed.toast("Track deleted", cx);
                    }
                },
            )
            .disabled(locked || empty)
            .destructive(),
        ];
        self.open_popup(at, items, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_the_nearest_target_in_reach() {
        let targets = [0, 1_000_000, 1_100_000];
        assert_eq!(snap(1_060_000, &targets, 100_000), Some(1_100_000));
        assert_eq!(snap(1_040_000, &targets, 100_000), Some(1_000_000));
        assert_eq!(snap(500_000, &targets, 100_000), None);
    }

    #[test]
    fn edge_scroll_ramps_up_into_the_edges() {
        assert_eq!(edge_speed(400., 800.), 0.);
        assert!(edge_speed(50., 800.) < 0.);
        assert!(edge_speed(790., 800.) > edge_speed(750., 800.));
        assert_eq!(edge_speed(-100., 800.), -EDGE_MAX_SPEED);
    }
}
