//! The inspector's Visual sub-tabs for a video / photo (and Basic for
//! text and stickers): Basic (transform + opacity), Rotate (quarter turns,
//! mirror, flip), Remove BG, Mask, Splice (overlay lanes) and Background
//! (main lane). Each control is one undo step; on a clip with keyframes the
//! animatable numbers are shown and edited at the playhead.

use std::rc::Rc;

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Div, ElementId, MouseButton, MouseDownEvent,
    ObjectFit, PathPromptOptions, SharedString, div, img, prelude::*, px, rgb,
};

use crate::decoder;
use crate::editor::{Editor, NudgeFn, SliderFn};
use crate::effects::stack::{MaskPlan, Placement};
use crate::model::{Clip, Micros};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::timeline_edit::keyframe_eps;
use crate::ui::widgets::{
    choice_pill, icon_button, panel_note, panel_row, pick_tile, pill_button, section_with,
    slider_track_from, switch, switch_row, tooltip,
};
use crate::visual::{
    BLEND_MODES, Background, ClipMask, CutoutStroke, MASK_FIELDS, MaskShape, quarter_turn,
};

/// A control's value committed (in its read-out's units).
type CommitFn = Rc<dyn Fn(&mut Editor, f64, &mut Context<Editor>)>;

/// An edit at the playhead between diamonds adds one (the old editor's
/// default; no switch for it yet).
const AUTO_KEYFRAME: bool = true;

/// The transform fields a ◇ keys on a clip with nothing else animated.
const TRANSFORM_PROPS: [&str; 5] = [
    "transform.x",
    "transform.y",
    "transform.scale",
    "transform.rotation",
    "transform.opacity",
];

/// Zoom's limits (%), and how far its slider reaches (the field goes on).
const MIN_ZOOM: f64 = 10.;
const MAX_ZOOM: f64 = 10_000.;
const SLIDER_MAX_ZOOM: f64 = 400.;

/// The Background colours (none first is its own tile).
pub(crate) const SWATCHES: [&str; 18] = [
    "#000000", "#ffffff", "#3a3a3c", "#8e8e93", "#d1d1d6", "#ff3b30", "#ff9500", "#ffcc00",
    "#34c759", "#00c7be", "#30b0c7", "#007aff", "#5856d6", "#af52de", "#ff2d55", "#a2845e",
    "#f4e1c1", "#1c3d5a",
];

/// The Stroke colours of Remove BG.
const STROKE_SWATCHES: [&str; 8] = [
    "#ffffff", "#000000", "#ff3b30", "#ffcc00", "#34c759", "#007aff", "#af52de", "#ff2d55",
];

/// Background's pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BgPage {
    Color,
    Image,
    Blur,
}

impl BgPage {
    const ALL: [(BgPage, &'static str); 3] = [
        (BgPage::Color, "Color"),
        (BgPage::Image, "Image"),
        (BgPage::Blur, "Blur"),
    ];

    /// The page a background is on (a pattern or a photo is Image's).
    fn of(bg: Option<&Background>) -> BgPage {
        match bg {
            Some(Background::Blur { .. }) => BgPage::Blur,
            Some(Background::Image { .. } | Background::Pattern { .. }) => BgPage::Image,
            _ => BgPage::Color,
        }
    }
}

fn mask_icon(shape: MaskShape) -> Icon {
    match shape {
        MaskShape::Horizontal => Icon::MaskHorizontal,
        MaskShape::Mirror => Icon::MaskMirror,
        MaskShape::Circle => Icon::CircleOutline,
        MaskShape::Rectangle => Icon::RectangleOutline,
        MaskShape::Heart => Icon::HeartOutline,
        MaskShape::Star => Icon::StarOutline,
    }
}

pub(crate) fn hex(s: &str) -> gpui::Rgba {
    let h = s.trim_start_matches('#');
    u32::from_str_radix(h, 16)
        .ok()
        .filter(|_| h.len() == 6)
        .map_or(rgb(0x000000), rgb)
}

fn round_to(v: f64, decimals: i32) -> f64 {
    let k = 10f64.powi(decimals);
    (v * k).round() / k
}

fn body(children: impl IntoIterator<Item = AnyElement>) -> AnyElement {
    div()
        .px(px(14.))
        .pb(px(14.))
        .flex()
        .flex_col()
        .children(children)
        .into_any_element()
}

fn tiles(children: impl IntoIterator<Item = AnyElement>) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap(px(8.))
        .py(px(6.))
        .children(children)
}

impl Editor {
    // ---- reading at the playhead ------------------------------------------------

    /// The playhead in the clip's time, held inside it.
    fn clip_local(&self, clip: &Clip) -> Micros {
        (self.playhead - clip.timeline_start).clamp(0, clip.timeline_duration.max(0))
    }

    /// A property as it is at the playhead: keyframes evaluated by the
    /// engine, else the clip's own value.
    pub fn shown_property(&self, clip: &Clip, id: &str) -> Option<f64> {
        let own = clip.static_property(id);
        if clip.is_animated() {
            self.engine
                .clip_property(&clip.id, id, self.clip_local(clip))
                .or(own)
        } else {
            own
        }
    }

    /// A property at clip-local `local`: keyframes evaluated by the engine,
    /// else the clip's own value.
    pub(crate) fn property_at(&self, clip: &Clip, id: &str, local: Micros) -> Option<f64> {
        let own = clip.static_property(id);
        if clip.is_animated() {
            self.engine.clip_property(&clip.id, id, local).or(own)
        } else {
            own
        }
    }

    /// A held Visual control (slider dragged, number field scrubbed) as the
    /// property it is setting on the selected clip, so the player follows
    /// it before the edit lands on release.
    fn live_property(&self) -> Option<(&'static str, f64)> {
        let (key, v) = self.live_value?;
        let (cw, ch) = (
            self.project.canvas.width.max(1) as f64,
            self.project.canvas.height.max(1) as f64,
        );
        Some(match key {
            "basic:zoom" => ("transform.scale", v / 100.),
            "basic:x" => ("transform.x", v / cw),
            "basic:y" => ("transform.y", -v / ch),
            "basic:rotate" => ("transform.rotation", v),
            "basic:opacity" | "splice:opacity" => ("transform.opacity", v / 100.),
            "mask:x" => ("mask.centerX", v / 100.),
            "mask:y" => ("mask.centerY", v / 100.),
            "mask:w" => ("mask.width", v / 100.),
            "mask:h" => ("mask.height", v / 100.),
            "mask:rotate" => ("mask.rotation", v),
            "mask:feather" => ("mask.feather", v / 100.),
            _ => return None,
        })
    }

    /// Where `clip`'s picture lands on the canvas at timeline time `t`:
    /// its transform and mask there (keyframes evaluated), crop, mirror /
    /// flip and Splice blend.
    pub fn layer_placement(&self, clip: &Clip, t: Micros) -> Placement {
        let local = (t - clip.timeline_start).clamp(0, clip.timeline_duration.max(0));
        let live = self
            .live_property()
            .filter(|_| self.selected.primary() == Some(clip.id.as_str()));
        // A layer held on the player: its transform so far.
        let held = self
            .live_xform
            .as_ref()
            .filter(|(id, _)| *id == clip.id)
            .map(|(_, x)| *x);
        let get = |id: &str, def: f64| -> f32 {
            if let Some(x) = held {
                match id {
                    "transform.x" => return x.x,
                    "transform.y" => return x.y,
                    "transform.scale" => return x.scale,
                    "transform.rotation" => return x.rotation,
                    _ => {}
                }
            }
            match live {
                Some((p, v)) if p == id => v as f32,
                _ => self.property_at(clip, id, local).unwrap_or(def) as f32,
            }
        };
        let crop = clip.extra.get("crop").map_or([0., 0., 1., 1.], |c| {
            let f = |k: &str, d: f64| c.get(k).and_then(|v| v.as_f64()).unwrap_or(d) as f32;
            let (w, h) = (f("width", 1.), f("height", 1.));
            if w > 0. && h > 0. {
                [f("x", 0.), f("y", 0.), w, h]
            } else {
                [0., 0., 1., 1.]
            }
        });
        let crop_turn = clip
            .extra
            .get("crop")
            .and_then(|c| c.get("rotation"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.) as f32;
        let mask = clip.mask().map(|m| MaskPlan {
            shape: MaskShape::ALL
                .iter()
                .position(|s| *s == m.shape)
                .unwrap_or(2) as u32,
            center: [
                get("mask.centerX", m.center_x),
                get("mask.centerY", m.center_y),
            ],
            size: [get("mask.width", m.width), get("mask.height", m.height)],
            rotation: get("mask.rotation", m.rotation),
            feather: get("mask.feather", m.feather),
            invert: m.invert,
        });
        let blend = BLEND_MODES
            .iter()
            .position(|(id, _)| *id == clip.blend_mode())
            .unwrap_or(0) as u32;
        Placement {
            x: get("transform.x", 0.),
            y: get("transform.y", 0.),
            scale: get("transform.scale", 1.),
            rotation: get("transform.rotation", 0.),
            opacity: get("transform.opacity", 1.).clamp(0., 1.),
            flip_x: clip.flipped(false),
            flip_y: clip.flipped(true),
            crop,
            crop_turn,
            mask,
            blend,
            natural: None,
        }
    }

    /// The clip's picture size (its crop box of the source), when known.
    pub fn clip_frame_size(&self, clip: &Clip) -> Option<(f64, f64)> {
        // A sticker's size is in the document (its file is a URL: never
        // probed here, on the UI thread).
        if clip.kind == "sticker" {
            let n = |k: &str| {
                clip.extra
                    .get(k)
                    .and_then(|v| v.as_f64())
                    .filter(|v| *v > 0.)
            };
            return Some((n("sourceWidth")?, n("sourceHeight")?));
        }
        let src = clip.source.as_ref()?;
        let (w, h) = self
            .media
            .iter()
            .find(|m| &m.path == src)
            .map(|m| (m.width, m.height))
            .or_else(|| decoder::probe(src).ok().map(|m| (m.width, m.height)))?;
        if w == 0 || h == 0 {
            return None;
        }
        let crop = clip.extra.get("crop");
        let part = |k: &str| {
            crop.and_then(|c| c.get(k))
                .and_then(|v| v.as_f64())
                .filter(|v| *v > 0.)
                .unwrap_or(1.)
        };
        Some((w as f64 * part("width"), h as f64 * part("height")))
    }

    fn frame_aspect(&self, clip: &Clip) -> f64 {
        self.clip_frame_size(clip).map(|(w, h)| w / h).unwrap_or(
            self.project.canvas.width.max(1) as f64 / self.project.canvas.height.max(1) as f64,
        )
    }

    // ---- edits ---------------------------------------------------------------------

    /// One undoable change to a clip (`f` says whether it changed it).
    pub(crate) fn edit_clip(
        &mut self,
        clip_id: &str,
        label: &'static str,
        f: impl FnOnce(&mut Clip) -> bool,
        cx: &mut Context<Self>,
    ) {
        let id = clip_id.to_string();
        self.edit(label, false, None, move |p| p.update_clip(&id, f), cx);
    }

    /// Animatable numbers, at the playhead: keyed on a clip with keyframes.
    pub fn set_clip_properties(
        &mut self,
        clip_id: &str,
        label: &'static str,
        patch: &[(&'static str, f64)],
        cx: &mut Context<Self>,
    ) {
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let t = self.clip_local(clip);
        let eps = keyframe_eps(self.pps);
        let patch = patch.to_vec();
        self.edit_clip(
            clip_id,
            label,
            move |c| {
                let before = c.extra.clone();
                c.set_properties_at(&patch, t, AUTO_KEYFRAME, eps);
                c.extra != before
            },
            cx,
        );
    }

    /// ◇: deletes the diamond under the playhead, or adds one keying the
    /// clip's animated properties (its transform when nothing is), plus
    /// the mask's while Mask is open.
    fn toggle_keyframe(&mut self, clip_id: &str, with_mask: bool, cx: &mut Context<Self>) {
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let eps = keyframe_eps(self.pps);
        let local = self.playhead - clip.timeline_start;
        if local < 0 || local > clip.timeline_duration {
            return;
        }
        if clip.diamond_near(local, eps).is_some() {
            self.edit_clip(
                clip_id,
                "Delete Keyframe",
                move |c| c.delete_keyframes_at(local, eps),
                cx,
            );
            return;
        }
        let mut props: Vec<String> = TRANSFORM_PROPS.iter().map(|s| s.to_string()).collect();
        for p in clip.animated_properties() {
            if !props.contains(&p) {
                props.push(p);
            }
        }
        if with_mask && clip.mask().is_some() {
            for f in MASK_FIELDS {
                let p = format!("mask.{f}");
                if !props.contains(&p) {
                    props.push(p);
                }
            }
        }
        // Each at its value there; a property neither keyed nor known here
        // (none yet) is left out.
        let values: Vec<(String, f64)> = props
            .into_iter()
            .filter_map(|p| {
                let v = self
                    .engine
                    .clip_property(&clip.id, &p, local)
                    .or_else(|| clip.static_property(&p))?;
                Some((p, v))
            })
            .collect();
        self.edit_clip(
            clip_id,
            "Add Keyframe",
            move |c| {
                c.add_keyframe_at(local, eps, &values);
                true
            },
            cx,
        );
    }

    /// ‹ / ›: the playhead to the previous / next diamond.
    fn step_keyframe(&mut self, clip_id: &str, forward: bool, cx: &mut Context<Self>) {
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let eps = keyframe_eps(self.pps);
        let local = self.playhead - clip.timeline_start;
        let times = clip.keyframe_times(eps);
        let target = if forward {
            times.into_iter().find(|&d| d > local + eps)
        } else {
            times.into_iter().rev().find(|&d| d < local - eps)
        };
        if let Some(d) = target {
            let at = clip.timeline_start + d;
            self.pause();
            self.seek(at, cx);
        }
    }

    /// A quarter turn that keeps the clip framed; the canvas turns too when
    /// this is the main lane's only clip and the canvas had its shape.
    fn rotate_quarter(&mut self, clip_id: &str, delta: f64, cx: &mut Context<Self>) {
        let Some((track, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let frame = self.clip_frame_size(clip);
        let follows = frame.is_some()
            && self
                .project
                .main_track()
                .is_some_and(|m| m.id == track.id && m.clips.len() == 1);
        let canvas = (self.project.canvas.width, self.project.canvas.height);
        let rotation = self
            .shown_property(clip, "transform.rotation")
            .unwrap_or(0.);
        let scale = self.shown_property(clip, "transform.scale").unwrap_or(1.);
        let turn = quarter_turn(
            frame.unwrap_or((canvas.0 as f64, canvas.1 as f64)),
            canvas,
            rotation,
            scale,
            delta,
            follows,
        );
        let t = self.clip_local(clip);
        let eps = keyframe_eps(self.pps);
        let id = clip_id.to_string();
        self.edit(
            "Rotate",
            false,
            None,
            move |p| {
                let turned = p.update_clip(&id, |c| {
                    c.set_properties_at(
                        &[
                            ("transform.rotation", turn.rotation),
                            ("transform.scale", turn.scale),
                        ],
                        t,
                        AUTO_KEYFRAME,
                        eps,
                    );
                    true
                });
                if let Some((w, h)) = turn.canvas {
                    (p.canvas.width, p.canvas.height) = (w, h);
                }
                turned
            },
            cx,
        );
    }

    fn pick_mask(&mut self, clip_id: &str, shape: Option<MaskShape>, cx: &mut Context<Self>) {
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let current = clip.mask();
        let Some(shape) = shape else {
            if current.is_some() {
                self.edit_clip(
                    clip_id,
                    "Remove Mask",
                    |c| {
                        c.set_mask(None);
                        true
                    },
                    cx,
                );
            }
            return;
        };
        if current.as_ref().is_some_and(|m| m.shape == shape) {
            return;
        }
        // A new shape keeps the place, size and softness dialled in.
        let next = match current {
            Some(m) => ClipMask { shape, ..m },
            None => ClipMask::new(shape, self.frame_aspect(clip)),
        };
        self.edit_clip(
            clip_id,
            "Mask",
            move |c| {
                c.set_mask(Some(&next));
                true
            },
            cx,
        );
    }

    /// The mask back to its shape's default size and place, its keyframes
    /// gone with it.
    fn reset_mask(&mut self, clip_id: &str, cx: &mut Context<Self>) {
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let Some(m) = clip.mask() else {
            return;
        };
        let fresh = ClipMask::new(m.shape, self.frame_aspect(clip));
        let ids: Vec<String> = MASK_FIELDS.iter().map(|f| format!("mask.{f}")).collect();
        self.edit_clip(
            clip_id,
            "Reset Mask",
            move |c| {
                let before = c.extra.clone();
                c.set_mask(Some(&fresh));
                c.clear_tracks(&ids);
                c.extra != before
            },
            cx,
        );
    }

    fn set_background(&mut self, clip_id: &str, bg: Option<Background>, cx: &mut Context<Self>) {
        self.edit_clip(
            clip_id,
            "Background",
            move |c| {
                if Background::same(c.background().as_ref(), bg.as_ref()) {
                    return false;
                }
                c.set_background(bg.as_ref());
                true
            },
            cx,
        );
    }

    /// The clip's Background on every main-lane clip, as one step.
    fn background_to_all(&mut self, clip_id: &str, cx: &mut Context<Self>) {
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let bg = clip.background();
        self.edit(
            "Background to All",
            false,
            None,
            move |p| {
                let Some(main) = p.tracks.iter_mut().find(|t| p_main(t)) else {
                    return false;
                };
                let mut changed = false;
                for c in &mut main.clips {
                    if !Background::same(c.background().as_ref(), bg.as_ref()) {
                        c.set_background(bg.as_ref());
                        changed = true;
                    }
                }
                changed
            },
            cx,
        );
        self.toast("Background applied to all clips", cx);
    }

    /// Upload photo: a picture from disk as the clip's Background.
    fn upload_background(&mut self, clip_id: &str, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Upload photo".into()),
        });
        let id = clip_id.to_string();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.first().map(|p| p.to_string_lossy().into_owned()) else {
                return;
            };
            this.update(cx, |ed, cx| match decoder::probe(&path) {
                Ok(m) if m.width > 0 => ed.set_background(
                    &id,
                    Some(Background::Image {
                        uri: path,
                        width: m.width as f64,
                        height: m.height as f64,
                    }),
                    cx,
                ),
                _ => ed.toast("That file isn't a picture", cx),
            })
            .ok();
        })
        .detach();
    }

    /// Stops removing the background (a body effect still reading a video's
    /// matte keeps it).
    fn remove_cutout(&mut self, clip_id: &str, cx: &mut Context<Self>) {
        let keep_matte = self.project.tracks.iter().any(|t| {
            t.clips.iter().any(|c| {
                c.kind == "effect" && c.effects().iter().any(|e| e.kind.starts_with("Body:"))
            })
        });
        self.edit_clip(
            clip_id,
            "Remove Background",
            move |c| c.remove_cutout(keep_matte),
            cx,
        );
    }

    fn set_stroke(&mut self, clip_id: &str, stroke: Option<CutoutStroke>, cx: &mut Context<Self>) {
        self.edit_clip(
            clip_id,
            "Stroke",
            move |c| {
                if c.cutout_stroke() == stroke {
                    return false;
                }
                c.set_cutout_stroke(stroke.as_ref())
            },
            cx,
        );
    }

    // ---- controls ----------------------------------------------------------------------

    /// "Label | slider | read-out": live while dragged, `commit` once on
    /// release (in the read-out's units).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn fx_slider(
        &self,
        key: &'static str,
        label: &str,
        value: f64,
        (min, max): (f64, f64),
        centred: bool,
        suffix: &'static str,
        commit: impl Fn(&mut Editor, f64, &mut Context<Editor>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (shown, dragging) = match self.live_value {
            Some((k, v)) if k == key => (v, true),
            _ => (value, false),
        };
        let start = value;
        let apply: SliderFn = Rc::new(move |ed, f, done, cx| {
            let v = (min + f as f64 * (max - min)).round();
            if done {
                ed.live_value = None;
                if v != start {
                    commit(ed, v, cx);
                }
                cx.notify();
            } else {
                ed.live_value = Some((key, v));
                cx.notify();
            }
        });
        let fraction = ((shown - min) / (max - min)) as f32;
        let origin = if centred {
            ((0. - min) / (max - min)) as f32
        } else {
            0.
        };
        panel_row(label.to_string())
            .child(
                div()
                    .id(key)
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .cursor_pointer()
                    .child(self.measure(key))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            ed.start_slider(key, e.position.x, apply.clone(), cx);
                        }),
                    )
                    .child(slider_track_from(fraction, origin, dragging)),
            )
            .child(self.number_field(key, None, value, 0, suffix, 1., (min, max), None, cx))
            .into_any_element()
    }

    /// A number box (prefix, value, unit): drag it sideways to change the
    /// value (`per_px` a pixel), committed once on release. `commit` None:
    /// it only reads out (a slider's).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn number_field(
        &self,
        key: &'static str,
        prefix: Option<&'static str>,
        value: f64,
        decimals: i32,
        suffix: &'static str,
        per_px: f64,
        (min, max): (f64, f64),
        commit: Option<CommitFn>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let shown = match self.live_value {
            Some((k, v)) if k == key => v,
            _ => value,
        };
        let text = if decimals > 0 {
            format!("{:.*}", decimals as usize, shown)
        } else {
            format!("{}", shown.round() as i64)
        };
        let field_id: ElementId = ElementId::Name(SharedString::from(format!("{key}:field")));
        div()
            .id(field_id)
            .flex_none()
            .w(px(if prefix.is_some() { 96. } else { 76. }))
            .h(px(26.))
            .px_2()
            .rounded(px(5.))
            .flex()
            .items_center()
            .gap_1()
            .bg(theme::chip_bg())
            .border_1()
            .border_color(theme::chip_border())
            .font_family(theme::FONT_MONO)
            .text_size(px(theme::XS))
            .when_some(prefix, |d, p| {
                d.child(div().text_color(theme::text_tertiary()).child(p))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .justify_end()
                    .text_color(theme::text())
                    .child(text),
            )
            .when(!suffix.is_empty(), |d| {
                d.child(div().text_color(theme::text_tertiary()).child(suffix))
            })
            .when_some(commit, |d, commit| {
                let start = value;
                let apply: NudgeFn = Rc::new(move |ed, dx, done, cx| {
                    let v = round_to(start + dx as f64 * per_px, decimals).clamp(min, max);
                    if done {
                        ed.live_value = None;
                        if v != start {
                            commit(ed, v, cx);
                        }
                    } else {
                        ed.live_value = Some((key, v));
                    }
                    cx.notify();
                });
                d.cursor(CursorStyle::ResizeLeftRight)
                    .hover(|s| s.border_color(theme::text_tertiary()))
                    .tooltip(tooltip("Drag sideways to change", None))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            ed.start_nudge(e.position.x, apply.clone(), cx);
                        }),
                    )
            })
            .into_any_element()
    }

    /// The section's reset (greyed when already at the defaults).
    pub(crate) fn reset_button(
        &self,
        id: &'static str,
        enabled: bool,
        f: impl Fn(&mut Editor, &mut Context<Editor>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        icon_button(id, Icon::Reload, "Reset")
            .size(14.)
            .disabled(!enabled)
            .on_click(cx.listener(move |ed, _, _, cx| f(ed, cx)))
            .into_any_element()
    }

    /// ‹ ◇ ›: previous diamond, add / delete one at the playhead, next.
    fn keyframe_controls(
        &self,
        clip: &Clip,
        with_mask: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let eps = keyframe_eps(self.pps);
        let local = self.playhead - clip.timeline_start;
        let inside = local >= 0 && local <= clip.timeline_duration;
        let times = clip.keyframe_times(eps);
        let has_prev = times.iter().any(|&d| d < local - eps);
        let has_next = times.iter().any(|&d| d > local + eps);
        let on = inside && clip.diamond_near(local, eps).is_some();
        let (a, b, c) = (clip.id.clone(), clip.id.clone(), clip.id.clone());
        div()
            .flex()
            .items_center()
            .child(
                icon_button("kf-prev", Icon::ChevronBack, "Previous keyframe")
                    .size(13.)
                    .disabled(!has_prev)
                    .on_click(cx.listener(move |ed, _, _, cx| ed.step_keyframe(&a, false, cx))),
            )
            .child(
                icon_button(
                    "kf-toggle",
                    if on {
                        Icon::Rhombus
                    } else {
                        Icon::RhombusOutline
                    },
                    if on {
                        "Delete keyframe"
                    } else {
                        "Add keyframe"
                    },
                )
                .size(14.)
                .disabled(!inside)
                .on_click(cx.listener(move |ed, _, _, cx| ed.toggle_keyframe(&b, with_mask, cx))),
            )
            .child(
                icon_button("kf-next", Icon::ChevronForward, "Next keyframe")
                    .size(13.)
                    .disabled(!has_next)
                    .on_click(cx.listener(move |ed, _, _, cx| ed.step_keyframe(&c, true, cx))),
            )
            .into_any_element()
    }

    /// A row of colour dots (`none` first when given); the chosen one ringed.
    pub(crate) fn swatch_row(
        &self,
        key: &'static str,
        colors: &[&'static str],
        chosen: Option<&str>,
        none: bool,
        pick: impl Fn(&mut Editor, Option<&'static str>, &mut Context<Editor>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pick = Rc::new(pick);
        let dot = |id: ElementId, on: bool| {
            div()
                .id(id)
                .flex_none()
                .size(px(24.))
                .rounded_full()
                .p(px(2.))
                .border_1()
                .border_color(if on {
                    theme::text().into()
                } else {
                    gpui::transparent_black()
                })
                .cursor_pointer()
        };
        let mut items: Vec<AnyElement> = Vec::new();
        if none {
            let p = pick.clone();
            items.push(
                dot((key, usize::MAX).into(), chosen.is_none())
                    .flex()
                    .items_center()
                    .justify_center()
                    .tooltip(tooltip("None", None))
                    .child(Icon::BanOutline.el(18., theme::text_secondary()))
                    .on_click(cx.listener(move |ed, _, _, cx| p(ed, None, cx)))
                    .into_any_element(),
            );
        }
        for (i, &c) in colors.iter().enumerate() {
            let p = pick.clone();
            let on = chosen.is_some_and(|x| x.eq_ignore_ascii_case(c));
            items.push(
                dot((key, i).into(), on)
                    .child(
                        div()
                            .size_full()
                            .rounded_full()
                            .bg(hex(c))
                            .border_1()
                            .border_color(theme::chip_border()),
                    )
                    .on_click(cx.listener(move |ed, _, _, cx| p(ed, Some(c), cx)))
                    .into_any_element(),
            );
        }
        div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .py(px(6.))
            .children(items)
            .into_any_element()
    }

    // ---- panels ---------------------------------------------------------------------------

    /// Basic: Transform (Zoom, Position, Rotate) with reset and ‹ ◇ ›, and
    /// on a video / photo, Opacity.
    pub fn render_basic_panel(&self, clip: &Clip, cx: &mut Context<Self>) -> AnyElement {
        let (cw, ch) = (
            self.project.canvas.width.max(1) as f64,
            self.project.canvas.height.max(1) as f64,
        );
        let get = |id: &str, def: f64| self.shown_property(clip, id).unwrap_or(def);
        let (tx, ty, scale, rot) = (
            get("transform.x", 0.),
            get("transform.y", 0.),
            get("transform.scale", 1.),
            get("transform.rotation", 0.),
        );
        let opacity = (get("transform.opacity", 1.) * 100.).round();
        let at_default = tx == 0. && ty == 0. && scale == 1. && rot == 0.;
        let id = clip.id.clone();
        let commit_with = |label: &'static str, f: fn(f64, f64, f64) -> (&'static str, f64)| {
            let id = id.clone();
            Rc::new(move |ed: &mut Editor, v: f64, cx: &mut Context<Editor>| {
                ed.set_clip_properties(&id, label, &[f(v, cw, ch)], cx)
            }) as CommitFn
        };

        let reset_id = clip.id.clone();
        let mut out: Vec<AnyElement> = vec![
            section_with(
                "Transform",
                true,
                [
                    self.reset_button(
                        "basic-reset",
                        !at_default,
                        move |ed, cx| {
                            ed.set_clip_properties(
                                &reset_id,
                                "Reset Transform",
                                &[
                                    ("transform.x", 0.),
                                    ("transform.y", 0.),
                                    ("transform.scale", 1.),
                                    ("transform.rotation", 0.),
                                ],
                                cx,
                            )
                        },
                        cx,
                    ),
                    self.keyframe_controls(clip, false, cx),
                ],
            )
            .into_any_element(),
        ];

        // Zoom: the slider reaches 400 %, the field goes on to 10 000 %.
        let zoom = (scale * 100.).round();
        let zoom_commit = commit_with("Zoom", |v, _, _| ("transform.scale", v / 100.));
        let zc = zoom_commit.clone();
        let (zoom_shown, dragging) = match self.live_value {
            Some(("basic:zoom", v)) => (v, true),
            _ => (zoom, false),
        };
        let apply: SliderFn = Rc::new(move |ed, f, done, cx| {
            let v = (MIN_ZOOM + f as f64 * (SLIDER_MAX_ZOOM - MIN_ZOOM)).round();
            if done {
                ed.live_value = None;
                if v != zoom {
                    zc(ed, v, cx);
                }
            } else {
                ed.live_value = Some(("basic:zoom", v));
            }
            cx.notify();
        });
        out.push(
            panel_row("Zoom")
                .child(
                    div()
                        .id("basic:zoom")
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .cursor_pointer()
                        .child(self.measure("basic:zoom"))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                ed.start_slider("basic:zoom", e.position.x, apply.clone(), cx);
                            }),
                        )
                        .child(slider_track_from(
                            ((zoom_shown - MIN_ZOOM) / (SLIDER_MAX_ZOOM - MIN_ZOOM)) as f32,
                            0.,
                            dragging,
                        )),
                )
                .child(self.number_field(
                    "basic:zoom",
                    None,
                    zoom,
                    0,
                    "%",
                    1.,
                    (MIN_ZOOM, MAX_ZOOM),
                    Some(zoom_commit),
                    cx,
                ))
                .into_any_element(),
        );
        out.push(
            panel_row("Position")
                .child(self.number_field(
                    "basic:x",
                    Some("X"),
                    (tx * cw).round(),
                    0,
                    "",
                    1.,
                    (-cw * 2., cw * 2.),
                    Some(commit_with("Position", |v, w, _| ("transform.x", v / w))),
                    cx,
                ))
                .child(self.number_field(
                    "basic:y",
                    Some("Y"),
                    (-ty * ch).round(),
                    0,
                    "",
                    1.,
                    (-ch * 2., ch * 2.),
                    Some(commit_with("Position", |v, _, h| ("transform.y", -v / h))),
                    cx,
                ))
                .into_any_element(),
        );
        out.push(
            panel_row("Rotate")
                .child(self.number_field(
                    "basic:rotate",
                    None,
                    round_to(rot, 1),
                    1,
                    "°",
                    0.5,
                    (-360., 360.),
                    Some(commit_with("Rotate", |v, _, _| ("transform.rotation", v))),
                    cx,
                ))
                .into_any_element(),
        );

        if clip.is_visual() {
            let (a, b) = (clip.id.clone(), clip.id.clone());
            out.push(
                section_with(
                    "Opacity",
                    false,
                    [self.reset_button(
                        "opacity-reset",
                        opacity != 100.,
                        move |ed, cx| {
                            ed.set_clip_properties(&a, "Opacity", &[("transform.opacity", 1.)], cx)
                        },
                        cx,
                    )],
                )
                .into_any_element(),
            );
            out.push(self.fx_slider(
                "basic:opacity",
                "Opacity",
                opacity,
                (0., 100.),
                false,
                "%",
                move |ed, v, cx| {
                    ed.set_clip_properties(&b, "Opacity", &[("transform.opacity", v / 100.)], cx)
                },
                cx,
            ));
        }
        body(out)
    }

    /// Rotate: a quarter turn either way, Mirror, Flip (lit when on), Crop.
    pub fn render_rotate_panel(
        &self,
        clip: &Clip,
        _main_lane: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tool = |id: &'static str, icon: Icon, label: &'static str, lit: bool, enabled: bool| {
            div()
                .id(id)
                .flex_none()
                .w(px(60.))
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .size(px(40.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(if lit {
                            theme::text()
                        } else {
                            theme::button_bg()
                        })
                        .when(enabled && !lit, |d| {
                            d.hover(|s| s.bg(theme::button_hover()))
                        })
                        .child(icon.el(
                            18.,
                            if !enabled {
                                theme::text_disabled()
                            } else if lit {
                                theme::app_bg()
                            } else {
                                theme::text()
                            },
                        )),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(if enabled {
                            theme::text_secondary()
                        } else {
                            theme::text_disabled()
                        })
                        .child(label),
                )
                .when(enabled, |d| d.cursor_pointer())
        };
        let vrule = || {
            div()
                .flex_none()
                .w(px(1.))
                .h(px(32.))
                .mt(px(4.))
                .bg(theme::chip_border())
        };
        let ids = [
            clip.id.clone(),
            clip.id.clone(),
            clip.id.clone(),
            clip.id.clone(),
        ];
        let [a, b, c, d] = ids;
        let (mirrored, flipped) = (clip.flipped(false), clip.flipped(true));
        let row = div()
            .flex()
            .items_start()
            .gap(px(4.))
            .py(px(6.))
            .child(
                tool("rotate-left", Icon::RotateLeft, "Left", false, true)
                    .on_click(cx.listener(move |ed, _, _, cx| ed.rotate_quarter(&a, -90., cx))),
            )
            .child(
                tool("rotate-right", Icon::RotateRight, "Right", false, true)
                    .on_click(cx.listener(move |ed, _, _, cx| ed.rotate_quarter(&b, 90., cx))),
            )
            .child(vrule())
            .child(
                tool("mirror", Icon::SwapHorizontal, "Mirror", mirrored, true).on_click(
                    cx.listener(move |ed, _, _, cx| {
                        ed.edit_clip(
                            &c,
                            "Mirror",
                            |clip| {
                                clip.set_flipped(false, !mirrored);
                                true
                            },
                            cx,
                        )
                    }),
                ),
            )
            .child(
                tool("flip", Icon::SwapVertical, "Flip", flipped, true).on_click(cx.listener(
                    move |ed, _, _, cx| {
                        ed.edit_clip(
                            &d,
                            "Flip",
                            |clip| {
                                clip.set_flipped(true, !flipped);
                                true
                            },
                            cx,
                        )
                    },
                )),
            )
            .child(vrule())
            .child({
                let id = clip.id.clone();
                tool("crop", Icon::Crop, "Crop", false, true)
                    .on_click(cx.listener(move |ed, _, _, cx| ed.open_crop(&id, cx)))
            });
        body([
            section_with("Rotate", true, []).into_any_element(),
            row.into_any_element(),
        ])
    }

    /// Remove BG: Auto removal (its switch; the modes), then Stroke with
    /// its colour, size and spacing. Removing needs the on-device model,
    /// which this app doesn't have yet: a cutout already in the project can
    /// be turned off and its stroke edited.
    pub fn render_cutout_panel(&self, clip: &Clip, cx: &mut Context<Self>) -> AnyElement {
        let cutout = clip.cutout();
        let has = cutout.is_some();
        let mode = cutout
            .and_then(|c| c.get("mode"))
            .and_then(|m| m.as_str())
            .unwrap_or("");
        let id = clip.id.clone();
        let mut out: Vec<AnyElement> = vec![
            section_with(
                "Auto removal",
                true,
                [switch("cutout-switch", has, !has)
                    .when(has, |d| {
                        d.on_click(cx.listener(move |ed, _, _, cx| ed.remove_cutout(&id, cx)))
                    })
                    .into_any_element()],
            )
            .into_any_element(),
        ];
        if clip.kind == "video" {
            out.push(
                panel_row("Keep")
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(choice_pill(
                                "cutout-person",
                                "Person",
                                mode == "person",
                                false,
                            ))
                            .child(choice_pill("cutout-auto", "Auto", mode == "subject", false)),
                    )
                    .into_any_element(),
            );
        }
        if !has {
            out.push(
                panel_note(
                    "Removing a background isn't in the native editor yet. A clip cut out \
                     earlier keeps its cutout, and you can turn it off here.",
                )
                .into_any_element(),
            );
        }

        let stroke = clip.cutout_stroke();
        let sid = clip.id.clone();
        out.push(
            section_with(
                "Stroke",
                false,
                [switch("stroke-switch", stroke.is_some(), stroke.is_none())
                    .when(stroke.is_some(), |d| {
                        d.on_click(cx.listener(move |ed, _, _, cx| ed.set_stroke(&sid, None, cx)))
                    })
                    .into_any_element()],
            )
            .into_any_element(),
        );
        match (&stroke, has) {
            (_, false) => out.push(
                panel_note("Remove the background first, then add a stroke").into_any_element(),
            ),
            (None, true) => out.push(
                panel_note("The stroke looks aren't in the native editor yet.").into_any_element(),
            ),
            (Some(s), true) => {
                let (c1, c2, c3) = (clip.id.clone(), clip.id.clone(), clip.id.clone());
                let (s1, s2, s3) = (s.clone(), s.clone(), s.clone());
                out.push(
                    panel_row("Color")
                        .h_auto()
                        .child(div().flex_1().min_w_0().child(self.swatch_row(
                            "stroke-color",
                            &STROKE_SWATCHES,
                            Some(&s.color),
                            false,
                            move |ed, c, cx| {
                                if let Some(c) = c {
                                    ed.set_stroke(
                                        &c1,
                                        Some(CutoutStroke {
                                            color: c.into(),
                                            ..s1.clone()
                                        }),
                                        cx,
                                    )
                                }
                            },
                            cx,
                        )))
                        .into_any_element(),
                );
                out.push(self.fx_slider(
                    "stroke:size",
                    "Size",
                    s.size.round(),
                    (0., 100.),
                    false,
                    "",
                    move |ed, v, cx| {
                        ed.set_stroke(
                            &c2,
                            Some(CutoutStroke {
                                size: v,
                                ..s2.clone()
                            }),
                            cx,
                        )
                    },
                    cx,
                ));
                out.push(self.fx_slider(
                    "stroke:spacing",
                    "Spacing",
                    s.spacing.round(),
                    (0., 100.),
                    false,
                    "",
                    move |ed, v, cx| {
                        ed.set_stroke(
                            &c3,
                            Some(CutoutStroke {
                                spacing: v,
                                ..s3.clone()
                            }),
                            cx,
                        )
                    },
                    cx,
                ));
            }
        }
        body(out)
    }

    /// Mask: the shapes, and under them the chosen shape's controls (Invert,
    /// Position, Size, Rotate, Feather) with reset and ‹ ◇ ›.
    pub fn render_mask_panel(&self, clip: &Clip, cx: &mut Context<Self>) -> AnyElement {
        let mask = clip.mask();
        let current = mask.as_ref().map(|m| m.shape);
        let mut shape_tiles: Vec<AnyElement> = Vec::new();
        let none_id = clip.id.clone();
        shape_tiles.push(
            pick_tile(
                "mask:none",
                52.,
                current.is_none(),
                Icon::BanOutline.el(20., theme::text_secondary()),
                Some("None".into()),
            )
            .on_click(cx.listener(move |ed, _, _, cx| ed.pick_mask(&none_id, None, cx)))
            .into_any_element(),
        );
        for (i, shape) in MaskShape::ALL.into_iter().enumerate() {
            let id = clip.id.clone();
            shape_tiles.push(
                pick_tile(
                    ("mask", i),
                    52.,
                    current == Some(shape),
                    mask_icon(shape).el(22., theme::text()),
                    Some(shape.label().into()),
                )
                .on_click(cx.listener(move |ed, _, _, cx| ed.pick_mask(&id, Some(shape), cx)))
                .into_any_element(),
            );
        }
        let mut out = vec![
            section_with("Mask", true, []).into_any_element(),
            tiles(shape_tiles).into_any_element(),
        ];
        let Some(m) = mask else {
            out.push(panel_note("Pick a shape to reveal part of the clip").into_any_element());
            return body(out);
        };

        let get = |f: &str, own: f64| {
            self.shown_property(clip, &format!("mask.{f}"))
                .unwrap_or(own)
        };
        let pct = |v: f64| (v * 100.).round();
        let prop = |f: &'static str, scale: f64| {
            let id = clip.id.clone();
            Rc::new(move |ed: &mut Editor, v: f64, cx: &mut Context<Editor>| {
                let key: &'static str = match f {
                    "centerX" => "mask.centerX",
                    "centerY" => "mask.centerY",
                    "width" => "mask.width",
                    "height" => "mask.height",
                    "rotation" => "mask.rotation",
                    _ => "mask.feather",
                };
                ed.set_clip_properties(&id, "Mask", &[(key, v / scale)], cx)
            }) as CommitFn
        };
        let (reset_id, invert_id) = (clip.id.clone(), clip.id.clone());
        let invert = m.invert;
        out.push(
            section_with(
                m.shape.label(),
                false,
                [
                    self.reset_button(
                        "mask-reset",
                        true,
                        move |ed, cx| ed.reset_mask(&reset_id, cx),
                        cx,
                    ),
                    self.keyframe_controls(clip, true, cx),
                ],
            )
            .into_any_element(),
        );
        out.push(
            switch_row(
                "Invert",
                switch("mask-invert", invert, false).on_click(cx.listener(move |ed, _, _, cx| {
                    ed.edit_clip(
                        &invert_id,
                        "Invert Mask",
                        |c| match c.mask() {
                            Some(mut m) => {
                                m.invert = !invert;
                                c.set_mask(Some(&m));
                                true
                            }
                            None => false,
                        },
                        cx,
                    )
                })),
            )
            .into_any_element(),
        );
        out.push(
            panel_row("Position")
                .child(self.number_field(
                    "mask:x",
                    Some("X"),
                    pct(get("centerX", m.center_x)),
                    0,
                    "%",
                    0.5,
                    (0., 100.),
                    Some(prop("centerX", 100.)),
                    cx,
                ))
                .child(self.number_field(
                    "mask:y",
                    Some("Y"),
                    pct(get("centerY", m.center_y)),
                    0,
                    "%",
                    0.5,
                    (0., 100.),
                    Some(prop("centerY", 100.)),
                    cx,
                ))
                .into_any_element(),
        );
        if m.shape.has_height() {
            out.push(
                panel_row("Size")
                    .when(m.shape.has_width(), |d| {
                        d.child(self.number_field(
                            "mask:w",
                            Some("W"),
                            pct(get("width", m.width)),
                            0,
                            "%",
                            0.5,
                            (5., 100.),
                            Some(prop("width", 100.)),
                            cx,
                        ))
                    })
                    .child(self.number_field(
                        "mask:h",
                        Some("H"),
                        pct(get("height", m.height)),
                        0,
                        "%",
                        0.5,
                        (5., 100.),
                        Some(prop("height", 100.)),
                        cx,
                    ))
                    .into_any_element(),
            );
        }
        let (rot, feather) = (prop("rotation", 1.), prop("feather", 100.));
        out.push(self.fx_slider(
            "mask:rotate",
            "Rotate",
            get("rotation", m.rotation).round(),
            (-180., 180.),
            true,
            "°",
            move |ed, v, cx| rot(ed, v, cx),
            cx,
        ));
        out.push(self.fx_slider(
            "mask:feather",
            "Feather",
            pct(get("feather", m.feather)),
            (0., 100.),
            false,
            "%",
            move |ed, v, cx| feather(ed, v, cx),
            cx,
        ));
        body(out)
    }

    /// Splice (overlay lanes): how the clip blends with what's under it,
    /// and its opacity.
    pub fn render_splice_panel(&self, clip: &Clip, cx: &mut Context<Self>) -> AnyElement {
        let current = clip.blend_mode().to_string();
        let modes = div().flex().flex_wrap().gap(px(6.)).py(px(6.)).children(
            BLEND_MODES.iter().enumerate().map(|(i, &(mode, label))| {
                let id = clip.id.clone();
                let on = current == mode;
                choice_pill(("blend", i), label, on, true).when(!on, |d| {
                    d.on_click(cx.listener(move |ed, _, _, cx| {
                        ed.edit_clip(
                            &id,
                            "Blend",
                            |c| {
                                c.set_blend_mode(mode);
                                true
                            },
                            cx,
                        )
                    }))
                })
            }),
        );
        let opacity = (self.shown_property(clip, "transform.opacity").unwrap_or(1.) * 100.).round();
        let (a, b) = (clip.id.clone(), clip.id.clone());
        body([
            section_with("Blend", true, []).into_any_element(),
            modes.into_any_element(),
            section_with(
                "Opacity",
                false,
                [self.reset_button(
                    "splice-opacity-reset",
                    opacity != 100.,
                    move |ed, cx| {
                        ed.set_clip_properties(&a, "Opacity", &[("transform.opacity", 1.)], cx)
                    },
                    cx,
                )],
            )
            .into_any_element(),
            self.fx_slider(
                "splice:opacity",
                "Opacity",
                opacity,
                (0., 100.),
                false,
                "%",
                move |ed, v, cx| {
                    ed.set_clip_properties(&b, "Opacity", &[("transform.opacity", v / 100.)], cx)
                },
                cx,
            ),
        ])
    }

    /// Background (main lane): Color / Image / Blur, then Apply to all.
    pub fn render_background_panel(&self, clip: &Clip, cx: &mut Context<Self>) -> AnyElement {
        let bg = clip.background();
        let page = match &self.background_page {
            Some((id, page)) if *id == clip.id => *page,
            _ => BgPage::of(bg.as_ref()),
        };
        let reset_id = clip.id.clone();
        let mut out: Vec<AnyElement> = vec![
            section_with(
                "Background",
                true,
                [self.reset_button(
                    "bg-reset",
                    bg.is_some(),
                    move |ed, cx| ed.set_background(&reset_id, None, cx),
                    cx,
                )],
            )
            .into_any_element(),
            panel_row("Type")
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .children(BgPage::ALL.iter().enumerate().map(|(i, &(p, label))| {
                            let id = clip.id.clone();
                            choice_pill(("bg-page", i), label, p == page, true).on_click(
                                cx.listener(move |ed, _: &ClickEvent, _, cx| {
                                    ed.background_page = Some((id.clone(), p));
                                    cx.notify();
                                }),
                            )
                        })),
                )
                .into_any_element(),
        ];

        match page {
            BgPage::Color => {
                let chosen = match &bg {
                    Some(Background::Color { color }) => Some(color.as_str()),
                    _ => None,
                };
                let id = clip.id.clone();
                out.push(self.swatch_row(
                    "bg-color",
                    &SWATCHES,
                    chosen,
                    true,
                    move |ed, c, cx| {
                        ed.set_background(&id, c.map(|c| Background::Color { color: c.into() }), cx)
                    },
                    cx,
                ));
            }
            BgPage::Image => {
                let mut items: Vec<AnyElement> = Vec::new();
                let none_id = clip.id.clone();
                items.push(
                    pick_tile(
                        "bg-image-none",
                        52.,
                        bg.is_none(),
                        Icon::BanOutline.el(20., theme::text_secondary()),
                        None,
                    )
                    .tooltip(tooltip("No background", None))
                    .on_click(
                        cx.listener(move |ed, _, _, cx| ed.set_background(&none_id, None, cx)),
                    )
                    .into_any_element(),
                );
                let up_id = clip.id.clone();
                items.push(
                    pick_tile(
                        "bg-upload",
                        52.,
                        false,
                        Icon::Add.el(22., theme::text()),
                        None,
                    )
                    .tooltip(tooltip("Upload photo", None))
                    .on_click(cx.listener(move |ed, _, _, cx| ed.upload_background(&up_id, cx)))
                    .into_any_element(),
                );
                match &bg {
                    Some(Background::Image { uri, .. }) => {
                        let picture = self.thumbs.get(uri).cloned();
                        items.push(
                            pick_tile(
                                "bg-photo",
                                52.,
                                true,
                                match picture {
                                    Some(p) => img(p)
                                        .absolute()
                                        .inset_0()
                                        .size_full()
                                        .object_fit(ObjectFit::Cover)
                                        .into_any_element(),
                                    None => Icon::Image.el(20., theme::text()).into_any_element(),
                                },
                                None,
                            )
                            .tooltip(tooltip(
                                std::path::Path::new(uri).file_name().map_or_else(
                                    || uri.clone(),
                                    |n| n.to_string_lossy().into_owned(),
                                ),
                                None,
                            ))
                            .into_any_element(),
                        );
                    }
                    Some(Background::Pattern { id }) => items.push(
                        pick_tile(
                            "bg-pattern",
                            52.,
                            true,
                            Icon::Palette.el(20., theme::text()),
                            None,
                        )
                        .tooltip(tooltip(format!("Pattern {id}"), None))
                        .into_any_element(),
                    ),
                    _ => {}
                }
                out.push(tiles(items).into_any_element());
                out.push(
                    panel_note("The pattern backgrounds aren't in the native editor yet.")
                        .into_any_element(),
                );
            }
            BgPage::Blur => {
                let thumb = clip
                    .source
                    .as_ref()
                    .and_then(|s| self.thumbs.get(s))
                    .cloned();
                let level = match &bg {
                    Some(Background::Blur { level }) => Some(*level),
                    _ => None,
                };
                let mut items: Vec<AnyElement> = Vec::new();
                let none_id = clip.id.clone();
                items.push(
                    pick_tile(
                        "bg-blur-none",
                        52.,
                        bg.is_none(),
                        Icon::BanOutline.el(20., theme::text_secondary()),
                        None,
                    )
                    .tooltip(tooltip("No background", None))
                    .on_click(
                        cx.listener(move |ed, _, _, cx| ed.set_background(&none_id, None, cx)),
                    )
                    .into_any_element(),
                );
                for l in 1..=4u8 {
                    let id = clip.id.clone();
                    // No blur in GPUI's image element: the clip's picture,
                    // dimmed more for a stronger blur, with the level on it.
                    let content = div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when_some(thumb.clone(), |d, t| {
                            d.child(
                                img(t)
                                    .absolute()
                                    .inset_0()
                                    .size_full()
                                    .object_fit(ObjectFit::Cover)
                                    .opacity(1. - 0.17 * l as f32),
                            )
                        })
                        .child(
                            div()
                                .font_family(theme::FONT_MONO)
                                .text_size(px(theme::SM))
                                .text_color(theme::text())
                                .child(l.to_string()),
                        );
                    items.push(
                        pick_tile(
                            ("bg-blur", l as usize),
                            52.,
                            level == Some(l),
                            content,
                            None,
                        )
                        .tooltip(tooltip(format!("Blur strength {l}"), None))
                        .on_click(cx.listener(move |ed, _, _, cx| {
                            ed.set_background(&id, Some(Background::Blur { level: l }), cx)
                        }))
                        .into_any_element(),
                    );
                }
                out.push(tiles(items).into_any_element());
            }
        }

        let all_id = clip.id.clone();
        out.push(
            div()
                .mt(px(14.))
                .child(
                    pill_button("bg-apply-all", "Apply to all", false)
                        .w_full()
                        .gap_1()
                        .on_click(
                            cx.listener(move |ed, _, _, cx| ed.background_to_all(&all_id, cx)),
                        ),
                )
                .into_any_element(),
        );
        body(out)
    }
}

fn p_main(t: &crate::model::Track) -> bool {
    t.kind == crate::model::TrackType::Video
}
