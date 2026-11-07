//! The player's on-canvas manipulator, as the old editor's: every text,
//! sticker and picture-in-picture layer on screen can be picked (topmost
//! first) and dragged to move it; the main video only once it's selected
//! (pan), so a stray click never nudges it. The selected layer gets a
//! white box with handles: ✕ delete (text, stickers), ✎ edit the words
//! (text), and scale + turn about its centre (bottom right; not on the
//! main video). A move snaps to the canvas centre lines and to the other
//! layers' centres, a turn to every 45°, with a guide line while it sits
//! there. A click on empty canvas clears the selection, a second click on
//! selected text edits its words, ⇧ / ⌘ click adds to or takes from the
//! selection. The whole drag is one undo step, keyed at the playhead on a
//! keyframed clip. The math is `crate::gizmo`.

use gpui::{
    AnyElement, Context, CursorStyle, MouseButton, MouseDownEvent, PathBuilder, Pixels, Point,
    Rgba, canvas, div, point, prelude::*, px, rgba,
};

use crate::editor::{Drag, Editor};
use crate::engine::LayerKind;
use crate::gizmo::{self, GizmoBox, HIT_SLOP, OVERLAY_PAD, Pt, SNAP_PX, Shape, Snapped, Xform};
use crate::ui::icons::Icon;
use crate::ui::widgets::tooltip;

/// A handle's white disc, and the square around it that picks it (px).
const HANDLE: f32 = 22.;
const HANDLE_HIT: f32 = 32.;
const HANDLE_ICON: f32 = 13.;
/// How far the pointer travels before a press becomes a move (px).
const DRAG_SLOP: f32 = 3.;
/// The corner handle never measures its scale from closer than this (px).
const MIN_PIVOT_RADIUS: f32 = 8.;

fn guide_color() -> Rgba {
    rgba(0xffffffd9)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    Picture,
    Text,
    Sticker,
}

/// One layer on screen the gizmo can pick.
pub struct Target {
    pub clip_id: String,
    pub kind: TargetKind,
    /// Its transform at the preview time (keyframes evaluated).
    pub value: Xform,
    pub shape: Shape,
    /// The main lane's video: picked only once selected, no corner handle.
    pub main: bool,
    /// Selectable, not movable.
    pub locked: bool,
}

/// What the pointer is over on the player (its cursor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoHover {
    Movable,
    Locked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoMode {
    Move,
    ScaleRotate,
}

/// The canvas on screen: its top-left (window px), screen px per canvas
/// px, and the canvas size.
#[derive(Clone, Copy, Debug)]
pub struct View {
    left: f32,
    top: f32,
    k: f32,
    cw: f32,
    ch: f32,
}

impl View {
    fn to_canvas(self, p: Point<Pixels>) -> Pt {
        Pt::new(
            (f32::from(p.x) - self.left) / self.k,
            (f32::from(p.y) - self.top) / self.k,
        )
    }

    fn canvas(self) -> (f32, f32) {
        (self.cw, self.ch)
    }
}

/// A press on a layer, until the release.
pub struct GizmoDrag {
    clip_id: String,
    mode: GizmoMode,
    /// The transform at the press, and its box (canvas px).
    base: Xform,
    start_box: GizmoBox,
    /// Where the press landed (window).
    origin: Point<Pixels>,
    view: View,
    /// The other layers' centres a move snaps to (transform units).
    xs: Vec<f32>,
    ys: Vec<f32>,
    /// Past the slop: moving (a corner handle starts at once).
    started: bool,
    /// The layer was selected before this press (a click then edits text).
    was_selected: bool,
    text: bool,
    locked: bool,
}

impl Editor {
    /// Where the canvas is drawn in the player (as `render_player` fits it).
    fn canvas_view(&self) -> Option<View> {
        let area = self.measured("player-area")?;
        let (cw, ch) = (
            self.project.canvas.width.max(1) as f32,
            self.project.canvas.height.max(1) as f32,
        );
        let (aw, ah) = (f32::from(area.size.width), f32::from(area.size.height));
        let k = (aw / cw).min(ah / ch);
        (k > 0.).then(|| View {
            left: f32::from(area.origin.x) + (aw - cw * k) / 2.,
            top: f32::from(area.origin.y) + (ah - ch * k) / 2.,
            k,
            cw,
            ch,
        })
    }

    /// Every layer on screen, back to front.
    pub(crate) fn gizmo_targets(&self) -> Vec<Target> {
        let t = self.preview_time();
        let (cw, ch) = (
            self.project.canvas.width.max(1) as f32,
            self.project.canvas.height.max(1) as f32,
        );
        self.shown_layers()
            .filter_map(|(w, f)| {
                let (track, clip) = self.project.find_clip(&w.clip_id)?;
                let local = (t - clip.timeline_start).clamp(0, clip.timeline_duration.max(0));
                let get =
                    |id: &str, def: f64| self.property_at(clip, id, local).unwrap_or(def) as f32;
                let value = Xform {
                    x: get("transform.x", 0.),
                    y: get("transform.y", 0.),
                    scale: get("transform.scale", 1.),
                    rotation: get("transform.rotation", 0.),
                };
                let kind = match w.kind {
                    LayerKind::Text => TargetKind::Text,
                    LayerKind::Sticker => TargetKind::Sticker,
                    _ => TargetKind::Picture,
                };
                let shape = match (w.content, self.layer_natural(w, clip, f)) {
                    // Text: its words, not the glow around them.
                    (Some([w_px, h_px]), _) => Shape::Natural {
                        width: w_px / w.raster_px / cw,
                        height: h_px / w.raster_px / ch,
                    },
                    (None, Some([width, height])) => Shape::Natural { width, height },
                    (None, None) => {
                        let crop = self.layer_placement(clip, t).crop;
                        Shape::Fit {
                            width: f.width as f32 * crop[2],
                            height: f.height as f32 * crop[3],
                        }
                    }
                };
                Some(Target {
                    clip_id: clip.id.clone(),
                    kind,
                    value,
                    shape,
                    main: kind == TargetKind::Picture && self.project.is_main_track(&track.id),
                    locked: track.locked || clip.locked(),
                })
            })
            .collect()
    }

    /// A target's transform as shown: the held one while it's dragged.
    fn shown_xform(&self, t: &Target) -> Xform {
        match &self.live_xform {
            Some((id, x)) if *id == t.clip_id => *x,
            _ => t.value,
        }
    }

    fn target_box(&self, t: &Target, view: &View) -> GizmoBox {
        gizmo::layer_box(&self.shown_xform(t), t.shape, view.canvas())
    }

    /// The topmost target under `p` (canvas px); the main video only
    /// while it's selected.
    fn gizmo_hit(&self, targets: &[Target], p: Pt, view: &View) -> Option<usize> {
        targets
            .iter()
            .enumerate()
            .rev()
            .find(|(_, t)| {
                (!t.main || self.selected.contains(&t.clip_id))
                    && self.target_box(t, view).contains(p, HIT_SLOP / view.k)
            })
            .map(|(i, _)| i)
    }

    /// The selected layer the chrome is drawn on: the primary selection
    /// when it's on screen, else the first selected one.
    fn gizmo_selected<'a>(&self, targets: &'a [Target]) -> Option<&'a Target> {
        let primary = self.selected.primary();
        targets
            .iter()
            .find(|t| Some(t.clip_id.as_str()) == primary)
            .or_else(|| targets.iter().find(|t| self.selected.contains(&t.clip_id)))
    }

    /// A press on the player: picks the layer under it (and starts moving
    /// it), or clears the selection on empty canvas.
    pub fn gizmo_mouse_down(&mut self, e: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(view) = self.canvas_view() else {
            return;
        };
        let targets = self.gizmo_targets();
        let toggle = e.modifiers.shift || e.modifiers.platform;
        let Some(i) = self.gizmo_hit(&targets, view.to_canvas(e.position), &view) else {
            if !toggle && !self.selected.ids().is_empty() {
                self.selected.clear();
                cx.notify();
            }
            return;
        };
        let t = &targets[i];
        if toggle {
            self.selected.toggle(&t.clip_id);
            cx.notify();
            return;
        }
        let was_selected = self.selected.contains(&t.clip_id);
        if !was_selected || self.selected.ids().len() > 1 {
            self.selected.set(t.clip_id.clone());
        }
        self.start_gizmo(
            t,
            &targets,
            GizmoMode::Move,
            e.position,
            view,
            was_selected,
            cx,
        );
    }

    /// A press on the selected layer's corner handle: scale and turn.
    fn gizmo_corner_down(&mut self, clip_id: &str, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(view) = self.canvas_view() else {
            return;
        };
        let targets = self.gizmo_targets();
        if let Some(t) = targets.iter().find(|t| t.clip_id == clip_id) {
            self.start_gizmo(t, &targets, GizmoMode::ScaleRotate, at, view, true, cx);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start_gizmo(
        &mut self,
        t: &Target,
        targets: &[Target],
        mode: GizmoMode,
        at: Point<Pixels>,
        view: View,
        was_selected: bool,
        cx: &mut Context<Self>,
    ) {
        let base = t.value;
        // Every other layer's centre lines up with this one (the main video
        // fills the frame: its centre is the canvas's, always snapped).
        let (xs, ys) = targets
            .iter()
            .filter(|o| o.clip_id != t.clip_id && !o.main)
            .map(|o| (o.value.x, o.value.y))
            .unzip();
        self.live_xform = None;
        self.gizmo_snap = Snapped::default();
        self.drag = Some(Drag::Gizmo(Box::new(GizmoDrag {
            clip_id: t.clip_id.clone(),
            mode,
            base,
            start_box: gizmo::layer_box(&base, t.shape, view.canvas()),
            origin: at,
            view,
            xs,
            ys,
            started: mode == GizmoMode::ScaleRotate && !t.locked,
            was_selected,
            text: t.kind == TargetKind::Text,
            locked: t.locked,
        })));
        cx.notify();
    }

    /// The pointer moved mid press: the layer follows (shown live).
    pub fn update_gizmo_drag(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(Drag::Gizmo(mut d)) = self.drag.take() else {
            return;
        };
        let (dx, dy) = (f32::from(at.x - d.origin.x), f32::from(at.y - d.origin.y));
        if !d.locked && (d.started || dx.hypot(dy) >= DRAG_SLOP) {
            if !d.started {
                d.started = true;
                // The layer holds still under the pointer.
                self.pause();
            }
            let v = d.view;
            let (x, snapped) = match d.mode {
                GizmoMode::Move => gizmo::moved(
                    &d.base,
                    (dx / v.k, dy / v.k),
                    v.canvas(),
                    (&d.xs, &d.ys),
                    SNAP_PX / v.k,
                ),
                GizmoMode::ScaleRotate => gizmo::scaled_and_turned(
                    &d.base,
                    &d.start_box,
                    v.to_canvas(d.origin),
                    v.to_canvas(at),
                    MIN_PIVOT_RADIUS / v.k,
                ),
            };
            self.live_xform = Some((d.clip_id.clone(), x));
            self.gizmo_snap = snapped;
            cx.notify();
        }
        self.drag = Some(Drag::Gizmo(d));
    }

    /// The release: one undo step with what changed (keyed at the playhead
    /// on a keyframed clip); a click on selected text edits its words.
    pub fn commit_gizmo_drag(&mut self, d: GizmoDrag, cx: &mut Context<Self>) {
        let live = self
            .live_xform
            .take()
            .filter(|(id, _)| *id == d.clip_id)
            .map(|(_, x)| x);
        self.gizmo_snap = Snapped::default();
        if !d.started {
            if d.was_selected && d.text {
                self.edit_text_words(cx);
            }
            cx.notify();
            return;
        }
        let Some(end) = live else {
            return;
        };
        // The old editor's rounding: 4 decimals, hundredths of a degree.
        let r4 = |v: f32| (v as f64 * 1e4).round() / 1e4;
        let mut patch: Vec<(&'static str, f64)> = vec![];
        for (id, to, from, eps) in [
            ("transform.x", r4(end.x), d.base.x, 1e-5),
            ("transform.y", r4(end.y), d.base.y, 1e-5),
            ("transform.scale", r4(end.scale), d.base.scale, 1e-5),
            (
                "transform.rotation",
                (end.rotation as f64 * 100.).round() / 100.,
                d.base.rotation,
                5e-3,
            ),
        ] {
            if (to - from as f64).abs() > eps {
                patch.push((id, to));
            }
        }
        if !patch.is_empty() {
            self.set_clip_properties(&d.clip_id, "Change Transform", &patch, cx);
        }
        cx.notify();
    }

    /// Text › Basic with the words' field taking the keyboard.
    fn edit_text_words(&mut self, cx: &mut Context<Self>) {
        self.inspector_entry = Some("text:basic");
        self.focus_text_field = true;
        cx.notify();
    }

    /// ✕ on the selected layer.
    fn gizmo_delete(&mut self, clip_id: &str, cx: &mut Context<Self>) {
        self.selected.set(clip_id.to_string());
        self.delete_selected(cx);
    }

    /// The pointer moved over the player: its cursor follows what's under it.
    pub fn gizmo_hover_at(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let hover = self.canvas_view().and_then(|view| {
            let targets = self.gizmo_targets();
            let i = self.gizmo_hit(&targets, view.to_canvas(at), &view)?;
            Some(if targets[i].locked {
                GizmoHover::Locked
            } else {
                GizmoHover::Movable
            })
        });
        if hover != self.gizmo_hover {
            self.gizmo_hover = hover;
            cx.notify();
        }
    }

    /// The player's cursor.
    pub fn gizmo_cursor(&self) -> CursorStyle {
        match &self.drag {
            Some(Drag::Gizmo(d)) if d.started && d.mode == GizmoMode::Move => {
                CursorStyle::ClosedHand
            }
            Some(Drag::Gizmo(d)) if d.started => CursorStyle::Crosshair,
            _ => match self.gizmo_hover {
                Some(GizmoHover::Movable) => CursorStyle::OpenHand,
                _ => CursorStyle::Arrow,
            },
        }
    }

    /// Over the player area: the guide lines while a layer sits on one,
    /// and the selected layer's box and handles.
    pub fn render_gizmo(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let area = self.measured("player-area")?;
        let view = self.canvas_view()?;
        let targets = self.gizmo_targets();
        let (ax, ay) = (f32::from(area.origin.x), f32::from(area.origin.y));
        // Canvas px → px inside the player area.
        let to_area = move |p: Pt| (view.left - ax + p.x * view.k, view.top - ay + p.y * view.k);
        let (left, top) = (view.left - ax, view.top - ay);
        let (width, height) = (view.cw * view.k, view.ch * view.k);
        let mut children: Vec<AnyElement> = vec![];

        // Guides: a line through the centre the move sits on, or a level
        // cross through the layer while its turn sits on a snap angle.
        let dragging = matches!(&self.drag, Some(Drag::Gizmo(d)) if d.started);
        let vline = |x: f32| {
            div()
                .absolute()
                .left(px(x.round()))
                .top(px(top))
                .w(px(1.))
                .h(px(height))
                .bg(guide_color())
                .into_any_element()
        };
        let hline = |y: f32| {
            div()
                .absolute()
                .left(px(left))
                .top(px(y.round()))
                .w(px(width))
                .h(px(1.))
                .bg(guide_color())
                .into_any_element()
        };
        if dragging {
            let snap = self.gizmo_snap;
            if let Some(x) = snap.x {
                children.push(vline(left + (0.5 + x) * width));
            }
            if let Some(y) = snap.y {
                children.push(hline(top + (0.5 + y) * height));
            }
            if let (true, Some((_, live))) = (snap.level, &self.live_xform) {
                children.push(vline(left + (0.5 + live.x) * width));
                children.push(hline(top + (0.5 + live.y) * height));
            }
        }

        if let Some(t) = self.gizmo_selected(&targets) {
            let b = self.target_box(t, &view);
            let pad = if t.kind == TargetKind::Picture {
                0.
            } else {
                OVERLAY_PAD / view.k
            };
            let corners: Vec<(f32, f32)> = [(-1., -1.), (1., -1.), (1., 1.), (-1., 1.)]
                .iter()
                .map(|&(sx, sy)| to_area(b.corner(sx, sy, pad)))
                .collect();
            let outline = corners.clone();
            children.push(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let o = bounds.origin;
                        // A dark underlay keeps the white line readable on
                        // bright footage.
                        for (w, color) in [(3., rgba(0x00000059)), (1.5, rgba(0xffffffff))] {
                            let mut pb = PathBuilder::stroke(px(w));
                            for (i, &(x, y)) in outline.iter().enumerate() {
                                let p = o + point(px(x), px(y));
                                if i == 0 {
                                    pb.move_to(p);
                                } else {
                                    pb.line_to(p);
                                }
                            }
                            pb.close();
                            if let Ok(path) = pb.build() {
                                window.paint_path(path, color);
                            }
                        }
                    },
                )
                .absolute()
                .size_full()
                .into_any_element(),
            );

            let turn = b.rotation;
            let handle = |id: &'static str, icon: Icon, label: &'static str, (x, y): (f32, f32)| {
                div()
                    .id(id)
                    .absolute()
                    .left(px(x - HANDLE_HIT / 2.))
                    .top(px(y - HANDLE_HIT / 2.))
                    .size(px(HANDLE_HIT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .tooltip(tooltip(label, None))
                    .child(
                        div()
                            .size(px(HANDLE))
                            .rounded_full()
                            .bg(rgba(0xffffffff))
                            .shadow_sm()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::turned(icon, HANDLE_ICON, rgba(0x1c1c1eff), turn)),
                    )
            };
            let id = t.clip_id.clone();
            if t.kind != TargetKind::Picture {
                let id = id.clone();
                children.push(
                    handle("gizmo-delete", Icon::MdiClose, "Delete", corners[0])
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ed, _, _, cx| {
                                cx.stop_propagation();
                                ed.gizmo_delete(&id, cx);
                            }),
                        )
                        .into_any_element(),
                );
            }
            if t.kind == TargetKind::Text {
                children.push(
                    handle("gizmo-edit", Icon::MdiPencil, "Edit text", corners[1])
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ed, _, _, cx| {
                                cx.stop_propagation();
                                ed.edit_text_words(cx);
                            }),
                        )
                        .into_any_element(),
                );
            }
            if !t.main && !t.locked {
                children.push(
                    handle(
                        "gizmo-transform",
                        Icon::MdiRotateRight,
                        "Scale and rotate",
                        corners[2],
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            ed.gizmo_corner_down(&id, e.position, cx);
                        }),
                    )
                    .into_any_element(),
                );
            }
        }

        Some(
            div()
                .absolute()
                .inset_0()
                .children(children)
                .into_any_element(),
        )
    }
}
