//! The Crop dialog over the editor (the toolbar's Crop, or Rotate › Crop):
//! the clip's frame on a large stage with the crop box, its thirds grid
//! and eight handles (a press inside moves it), a scrubber that picks the
//! frame of a video, a row of ratio buttons, a Rotate (straighten) slider,
//! and Reset · Cancel · Confirm. Esc cancels, Return confirms. The whole
//! session lands as one undo step.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, CursorStyle, FontWeight, MouseButton, MouseDownEvent, Pixels, Point,
    RenderImage, SharedString, Size, div, img, prelude::*, px,
};

use crate::clip_edits::crop_of;
use crate::crop::{self, CropBox, Handle, MAX_STRAIGHTEN, RATIOS};
use crate::decoder;
use crate::editor::{Drag, Editor, SliderFn};
use crate::model::{Micros, SECOND};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets::{icon_button, mono, pill_button, slider_track, slider_track_from};

const MAX_WIDTH: f32 = 1080.;
const MAX_HEIGHT: f32 = 820.;
const HEADER: f32 = 44.;
const SCRUB: f32 = 36.;
const CONTROLS: f32 = 88.;
const FOOTER: f32 = 56.;
const LABEL_WIDTH: f32 = 96.;
/// Room around the frame on the stage.
const PAD: f32 = 14.;
/// A handle's grab area: a square on a corner, a bar this thick on an edge.
const HANDLE_HIT: f32 = 44.;
const CORNER_LEN: f32 = 22.;
const CORNER_THICK: f32 = 4.;
/// The still's largest width (only its shape matters to the crop).
const FRAME_WIDTH: u32 = 960;
const GLYPH_BOX: f32 = 14.;

/// A frame decoded for the stage: BGRA pixels and size.
struct Still {
    width: u32,
    height: u32,
    bgra: Arc<Vec<u8>>,
}

/// The dialog's state while it's open.
pub struct CropSession {
    clip_id: String,
    is_video: bool,
    source: String,
    source_start: Micros,
    source_span: Micros,
    timeline_duration: Micros,
    /// What the clip had when the dialog opened.
    initial: Option<(CropBox, f64)>,
    pub(crate) b: CropBox,
    pub(crate) angle: f64,
    /// The ratio button picked (an index in `RATIOS`); None: Custom (free).
    ratio: Option<usize>,
    still: Option<Still>,
    /// The picture on the stage: the still, turned and grown about the
    /// box's centre while straightened; and what it was made for.
    shown: Option<(Arc<RenderImage>, f64, CropBox)>,
    /// The scrubber (0–1 of the clip) and the frame decoded for it.
    scrub: f32,
    load_seq: u64,
    /// The frame's place on the stage, px (x, y, width, height).
    image: [f32; 4],
}

/// A press on the crop frame: a handle (None: the box itself) and where
/// the box and the pointer were.
pub struct CropDrag {
    handle: Option<Handle>,
    start: CropBox,
    origin: Point<Pixels>,
}

impl CropSession {
    fn locked(&self) -> Option<f64> {
        self.ratio.map(|i| RATIOS[i].1)
    }

    fn source_size(&self) -> (f64, f64) {
        self.still
            .as_ref()
            .map_or((16., 9.), |s| (s.width as f64, s.height as f64))
    }
}

fn mmss(t: Micros) -> String {
    let total = (t.max(0) as f64 / SECOND as f64).round() as i64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

fn render_image(width: u32, height: u32, bgra: Vec<u8>) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(width, height, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// The still turned by `angle` (clockwise) and grown by the cover scale
/// about the box's centre, as the compositor will show it.
fn straightened(still: &Still, b: CropBox, angle: f64) -> Option<Vec<u8>> {
    let (w, h) = (still.width, still.height);
    let mut src = tiny_skia::Pixmap::new(w, h)?;
    src.data_mut().copy_from_slice(&still.bgra);
    let mut out = tiny_skia::Pixmap::new(w, h)?;
    out.fill(tiny_skia::Color::BLACK);
    let k = crop::cover_scale(b, angle, w as f64, h as f64) as f32;
    let (cx, cy) = (
        ((b.x + b.w / 2.) * w as f64) as f32,
        ((b.y + b.h / 2.) * h as f64) as f32,
    );
    let t = tiny_skia::Transform::from_translate(cx, cy)
        .pre_rotate(angle as f32)
        .pre_scale(k, k)
        .pre_translate(-cx, -cy);
    out.draw_pixmap(
        0,
        0,
        src.as_ref(),
        &tiny_skia::PixmapPaint {
            quality: tiny_skia::FilterQuality::Bilinear,
            ..Default::default()
        },
        t,
        None,
    );
    Some(out.take())
}

impl Editor {
    /// Opens the Crop dialog on a video / photo clip (playback pauses: it
    /// shows a still).
    pub fn open_crop(&mut self, clip_id: &str, cx: &mut Context<Self>) {
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        if !matches!(clip.kind.as_str(), "video" | "image") {
            return;
        }
        let Some(source) = clip.source.clone() else {
            return;
        };
        let initial = crop_of(clip);
        let speed = if clip.speed > 0. { clip.speed } else { 1. };
        let session = CropSession {
            clip_id: clip.id.clone(),
            is_video: clip.kind == "video",
            source,
            source_start: clip.source_start.unwrap_or(0),
            source_span: (clip.timeline_duration as f64 * speed) as Micros,
            timeline_duration: clip.timeline_duration,
            initial,
            b: initial.map_or(crop::FULL, |(b, _)| b),
            angle: initial.map_or(0., |(_, a)| a),
            ratio: None,
            still: None,
            shown: None,
            scrub: 0.,
            load_seq: 0,
            image: [0.; 4],
        };
        self.pause();
        self.popup = None;
        self.crop = Some(session);
        self.load_crop_frame(cx);
        cx.notify();
    }

    /// Decodes the still for the scrubber's place (a photo: the photo).
    fn load_crop_frame(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.crop.as_mut() else { return };
        s.load_seq += 1;
        let seq = s.load_seq;
        // Clear of the very last moment, where a decoder can return nothing.
        let at = if s.is_video {
            let at = s.source_start + (s.scrub as f64 * s.source_span as f64) as Micros;
            at.min(s.source_start + s.source_span - 1000)
                .max(s.source_start)
        } else {
            0
        };
        let path = s.source.clone();
        let task = cx
            .background_executor()
            .spawn(async move { decoder::thumbnail(&path, at, FRAME_WIDTH) });
        cx.spawn(async move |this, cx| {
            let frame = task.await;
            this.update(cx, |ed, cx| {
                let Some(s) = ed.crop.as_mut().filter(|s| s.load_seq == seq) else {
                    return;
                };
                match frame {
                    Ok(f) if f.width > 0 && f.height > 0 => {
                        s.still = Some(Still {
                            width: f.width,
                            height: f.height,
                            bgra: Arc::new(f.bgra),
                        });
                        if let Some((old, _, _)) = s.shown.take() {
                            ed.stale_frames.push(old);
                        }
                    }
                    _ => ed.toast("Couldn't load the frame", cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn close_crop(&mut self, cx: &mut Context<Self>) {
        if let Some(mut s) = self.crop.take()
            && let Some((old, _, _)) = s.shown.take()
        {
            self.stale_frames.push(old);
        }
        if matches!(self.drag, Some(Drag::Crop(_))) {
            self.drag = None;
        }
        cx.notify();
    }

    /// Esc / Cancel / ✕: nothing changes.
    pub fn cancel_crop(&mut self, cx: &mut Context<Self>) {
        self.close_crop(cx);
    }

    /// Return / Confirm: the crop as one undo step (none when unchanged).
    pub fn confirm_crop(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.crop.as_ref() else { return };
        let (b, angle) = crop::rounded(s.b, s.angle);
        let next = (!crop::is_identity(b, angle)).then_some((b, angle));
        let (id, changed) = (s.clip_id.clone(), next != s.initial);
        self.close_crop(cx);
        if changed {
            self.edit("Crop", false, None, |p| p.set_crop(&id, next), cx);
        }
    }

    fn reset_crop(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.crop.as_mut() {
            s.ratio = None;
            s.b = crop::FULL;
            s.angle = 0.;
        }
        cx.notify();
    }

    fn pick_crop_ratio(&mut self, ratio: Option<usize>, cx: &mut Context<Self>) {
        let Some(s) = self.crop.as_mut() else { return };
        s.ratio = ratio;
        if let Some(i) = ratio
            && s.still.is_some()
        {
            let (sw, sh) = s.source_size();
            s.b = crop::fit_ratio(s.b, RATIOS[i].1, sw, sh);
        }
        cx.notify();
    }

    fn start_crop_drag(
        &mut self,
        handle: Option<Handle>,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(s) = self.crop.as_ref() else { return };
        self.drag = Some(Drag::Crop(Box::new(CropDrag {
            handle,
            start: s.b,
            origin: at,
        })));
        cx.notify();
    }

    pub fn update_crop_drag(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let (Some(Drag::Crop(d)), Some(s)) = (&self.drag, self.crop.as_mut()) else {
            return;
        };
        let [_, _, iw, ih] = s.image;
        if iw <= 0. || ih <= 0. {
            return;
        }
        let dx = f32::from(at.x - d.origin.x) as f64 / iw as f64;
        let dy = f32::from(at.y - d.origin.y) as f64 / ih as f64;
        let (sw, sh) = s.source_size();
        s.b = match d.handle {
            None => crop::move_box(d.start, dx, dy),
            Some(h) => crop::resize_box(d.start, h, dx, dy, s.locked(), sw, sh),
        };
        cx.notify();
    }

    /// The dialog, when it's open.
    pub fn render_crop_dialog(
        &mut self,
        viewport: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let s = self.crop.as_mut()?;
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        let (w, h) = (MAX_WIDTH.min(vw * 0.92), MAX_HEIGHT.min(vh * 0.9));
        // Every row has a fixed height, so the stage's size is known here.
        let stage_w = w - 2.;
        let stage_h =
            (h - 2. - HEADER - if s.is_video { SCRUB } else { 0. } - CONTROLS - FOOTER).max(40.);

        // The picture: the still, re-turned when the straighten or the
        // box's centre changed.
        if let Some(still) = &s.still {
            let fresh = match &s.shown {
                Some((_, a, b)) => *a == s.angle && (s.angle == 0. || *b == s.b),
                None => false,
            };
            if !fresh {
                let bytes = if s.angle == 0. {
                    Some(still.bgra.as_ref().clone())
                } else {
                    straightened(still, s.b, s.angle)
                };
                if let Some(image) = bytes.and_then(|b| render_image(still.width, still.height, b))
                    && let Some((old, _, _)) = s.shown.replace((image, s.angle, s.b))
                {
                    self.stale_frames.push(old);
                }
            }
            let (fw, fh) = (still.width as f32, still.height as f32);
            let scale = ((stage_w - PAD * 2.) / fw).min((stage_h - PAD * 2.) / fh);
            let (iw, ih) = (fw * scale, fh * scale);
            s.image = [(stage_w - iw) / 2., (stage_h - ih) / 2., iw, ih];
        }
        let s = self.crop.as_ref()?;
        let stage = self.crop_stage(s, cx);
        let is_video = s.is_video;

        let scrub_row = is_video.then(|| {
            let total = s.timeline_duration;
            let apply: SliderFn = std::rc::Rc::new(|ed: &mut Editor, f, done, cx| {
                if let Some(s) = ed.crop.as_mut() {
                    s.scrub = f;
                }
                // One decode, on release (a decode per move is too costly).
                if done {
                    ed.load_crop_frame(cx);
                }
                cx.notify();
            });
            let dragging = matches!(
                self.drag,
                Some(Drag::Slider {
                    key: "crop-scrub",
                    ..
                })
            );
            div()
                .flex_none()
                .h(px(SCRUB))
                .px(px(16.))
                .flex()
                .items_center()
                .gap_2()
                .child(mono(
                    mmss((s.scrub as f64 * total as f64) as Micros),
                    theme::XS,
                    theme::text_secondary(),
                ))
                .child(
                    div()
                        .id("crop-scrub")
                        .relative()
                        .flex_1()
                        .cursor_pointer()
                        .child(self.measure("crop-scrub"))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                ed.start_slider("crop-scrub", e.position.x, apply.clone(), cx);
                            }),
                        )
                        .child(slider_track(s.scrub, dragging)),
                )
                .child(mono(mmss(total), theme::XS, theme::text_secondary()))
        });

        let label = |text: &'static str| {
            div()
                .flex_none()
                .w(px(LABEL_WIDTH))
                .text_size(px(theme::XS))
                .text_color(theme::text_secondary())
                .child(text)
        };
        let ratio_button = |i: Option<usize>, selected: bool| {
            let (text, glyph): (SharedString, AnyElement) = match i {
                None => (
                    "Custom".into(),
                    Icon::ExpandOutline
                        .el(
                            13.,
                            if selected {
                                theme::text()
                            } else {
                                theme::text_secondary()
                            },
                        )
                        .into_any_element(),
                ),
                Some(i) => {
                    let (name, r) = RATIOS[i];
                    let (gw, gh) = if r >= 1. {
                        (GLYPH_BOX, GLYPH_BOX / r as f32)
                    } else {
                        (GLYPH_BOX * r as f32, GLYPH_BOX)
                    };
                    (
                        name.into(),
                        div()
                            .w(px(gw))
                            .h(px(gh))
                            .rounded(px(2.))
                            .border_1()
                            .border_color(if selected {
                                theme::text()
                            } else {
                                theme::text_secondary()
                            })
                            .into_any_element(),
                    )
                }
            };
            div()
                .id(SharedString::from(format!(
                    "crop-ratio-{}",
                    i.map_or(-1, |i| i as i32)
                )))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(28.))
                .px(px(10.))
                .rounded(px(6.))
                .border_1()
                .border_color(if selected {
                    theme::text().into()
                } else {
                    gpui::transparent_black()
                })
                .bg(theme::button_bg())
                .cursor_pointer()
                .when(!selected, |d| {
                    d.hover(|st| st.border_color(theme::chip_border()))
                })
                .child(
                    div()
                        .size(px(GLYPH_BOX))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(glyph),
                )
                .child(
                    div()
                        .text_size(px(theme::XS))
                        .text_color(if selected {
                            theme::text()
                        } else {
                            theme::text_secondary()
                        })
                        .child(text),
                )
                .on_click(cx.listener(move |ed, _, _, cx| ed.pick_crop_ratio(i, cx)))
        };
        let ratios = div()
            .id("crop-ratios")
            .flex_1()
            .min_w_0()
            .overflow_x_scroll()
            .flex()
            .gap(px(6.))
            .child(ratio_button(None, s.ratio.is_none()))
            .children((0..RATIOS.len()).map(|i| ratio_button(Some(i), s.ratio == Some(i))));

        let rotate_apply: SliderFn = std::rc::Rc::new(|ed: &mut Editor, f, _, cx| {
            if let Some(s) = ed.crop.as_mut() {
                let deg = (f as f64 * 2. - 1.) * MAX_STRAIGHTEN;
                s.angle = (deg * 10.).round() / 10.;
            }
            cx.notify();
        });
        let rotate_dragging = matches!(
            self.drag,
            Some(Drag::Slider {
                key: "crop-rotate",
                ..
            })
        );
        let rotate_fraction = ((s.angle / MAX_STRAIGHTEN + 1.) / 2.) as f32;
        let controls = div()
            .flex_none()
            .h(px(CONTROLS))
            .px(px(16.))
            .py(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .border_t_1()
            .border_color(theme::card_border())
            .child(
                div()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(label("Aspect ratio"))
                    .child(ratios),
            )
            .child(
                div()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(label("Rotate"))
                    .child(
                        div()
                            .id("crop-rotate")
                            .relative()
                            .flex_1()
                            .max_w(px(420.))
                            .cursor_pointer()
                            .child(self.measure("crop-rotate"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    ed.start_slider(
                                        "crop-rotate",
                                        e.position.x,
                                        rotate_apply.clone(),
                                        cx,
                                    );
                                }),
                            )
                            .child(slider_track_from(rotate_fraction, 0.5, rotate_dragging)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(64.))
                            .h(px(26.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .justify_end()
                            .rounded(px(6.))
                            .bg(theme::button_bg())
                            .child(mono(format!("{:.1}°", s.angle), theme::XS, theme::text())),
                    ),
            );

        let footer = div()
            .flex_none()
            .h(px(FOOTER))
            .px(px(16.))
            .flex()
            .items_center()
            .gap_2()
            .border_t_1()
            .border_color(theme::card_border())
            .child(
                div()
                    .id("crop-reset")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(30.))
                    .px(px(10.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|st| st.bg(theme::button_bg()))
                    .child(Icon::Refresh.el(14., theme::text_secondary()))
                    .child(
                        div()
                            .text_size(px(theme::SM))
                            .text_color(theme::text_secondary())
                            .child("Reset"),
                    )
                    .on_click(cx.listener(|ed, _, _, cx| ed.reset_crop(cx))),
            )
            .child(div().flex_1())
            .child(
                pill_button("crop-cancel", "Cancel", false)
                    .min_w(px(88.))
                    .h(px(32.))
                    .text_size(px(theme::SM))
                    .on_click(cx.listener(|ed, _, _, cx| ed.cancel_crop(cx))),
            )
            .child(
                pill_button("crop-confirm", "Confirm", true)
                    .min_w(px(88.))
                    .h(px(32.))
                    .text_size(px(theme::SM))
                    .font_weight(FontWeight::SEMIBOLD)
                    .on_click(cx.listener(|ed, _, _, cx| ed.confirm_crop(cx))),
            );

        Some(
            div()
                .id("crop-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                // The backdrop hides the window root from the pointer:
                // drags started here (box, handles, sliders) run from it.
                .on_mouse_move(cx.listener(Editor::on_root_mouse_move))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|ed, _, _, cx| ed.end_drag(cx)),
                )
                .bg(gpui::rgba(0x00000099))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .id("crop-dialog")
                        .w(px(w))
                        .h(px(h))
                        .flex()
                        .flex_col()
                        .rounded(px(12.))
                        .overflow_hidden()
                        .bg(theme::sheet())
                        .border_1()
                        .border_color(theme::card_border())
                        .shadow_lg()
                        .child(
                            div()
                                .flex_none()
                                .h(px(HEADER))
                                .px(px(16.))
                                .flex()
                                .items_center()
                                .border_b_1()
                                .border_color(theme::card_border())
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(theme::MD))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("Crop"),
                                )
                                .child(
                                    icon_button("crop-close", Icon::Close, "Cancel crop")
                                        .size(18.)
                                        .on_click(cx.listener(|ed, _, _, cx| ed.cancel_crop(cx))),
                                ),
                        )
                        .child(div().flex_none().w(px(stage_w)).h(px(stage_h)).child(stage))
                        .children(scrub_row)
                        .child(controls)
                        .child(footer),
                )
                .into_any_element(),
        )
    }

    /// The stage: the picture, dimmed outside the box, the box with its
    /// grid and corner brackets, and the drag targets.
    fn crop_stage(&self, s: &CropSession, cx: &mut Context<Self>) -> AnyElement {
        let stage = div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(theme::app_bg());
        let Some((image, _, _)) = &s.shown else {
            return stage
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(theme::SM))
                        .text_color(theme::text_tertiary())
                        .child("Loading…"),
                )
                .into_any_element();
        };
        let [ix, iy, iw, ih] = s.image;
        let (bx, by) = (ix + s.b.x as f32 * iw, iy + s.b.y as f32 * ih);
        let (bw, bh) = (s.b.w as f32 * iw, s.b.h as f32 * ih);
        let dim = || div().absolute().bg(gpui::rgba(0x1414169e));
        let line = || div().absolute().bg(gpui::rgba(0xffffff8c));
        let corner = || {
            div()
                .absolute()
                .size(px(CORNER_LEN))
                .border_color(gpui::white())
        };

        let frame = div()
            .absolute()
            .left(px(bx))
            .top(px(by))
            .w(px(bw))
            .h(px(bh))
            .border_1()
            .border_color(gpui::rgba(0xfffffff2))
            .child(
                line()
                    .left(gpui::relative(1. / 3.))
                    .top_0()
                    .bottom_0()
                    .w(px(1.)),
            )
            .child(
                line()
                    .left(gpui::relative(2. / 3.))
                    .top_0()
                    .bottom_0()
                    .w(px(1.)),
            )
            .child(
                line()
                    .top(gpui::relative(1. / 3.))
                    .left_0()
                    .right_0()
                    .h(px(1.)),
            )
            .child(
                line()
                    .top(gpui::relative(2. / 3.))
                    .left_0()
                    .right_0()
                    .h(px(1.)),
            )
            .child(
                corner()
                    .left(px(-CORNER_THICK))
                    .top(px(-CORNER_THICK))
                    .border_l(px(CORNER_THICK))
                    .border_t(px(CORNER_THICK)),
            )
            .child(
                corner()
                    .right(px(-CORNER_THICK))
                    .top(px(-CORNER_THICK))
                    .border_r(px(CORNER_THICK))
                    .border_t(px(CORNER_THICK)),
            )
            .child(
                corner()
                    .left(px(-CORNER_THICK))
                    .bottom(px(-CORNER_THICK))
                    .border_l(px(CORNER_THICK))
                    .border_b(px(CORNER_THICK)),
            )
            .child(
                corner()
                    .right(px(-CORNER_THICK))
                    .bottom(px(-CORNER_THICK))
                    .border_r(px(CORNER_THICK))
                    .border_b(px(CORNER_THICK)),
            );

        let half = HANDLE_HIT / 2.;
        let handles = Handle::ALL.iter().map(|&h| {
            let (x, y, w, hh, cursor) = match h {
                Handle::TL => (
                    bx - half,
                    by - half,
                    HANDLE_HIT,
                    HANDLE_HIT,
                    CursorStyle::ResizeUpLeftDownRight,
                ),
                Handle::TR => (
                    bx + bw - half,
                    by - half,
                    HANDLE_HIT,
                    HANDLE_HIT,
                    CursorStyle::ResizeUpRightDownLeft,
                ),
                Handle::BL => (
                    bx - half,
                    by + bh - half,
                    HANDLE_HIT,
                    HANDLE_HIT,
                    CursorStyle::ResizeUpRightDownLeft,
                ),
                Handle::BR => (
                    bx + bw - half,
                    by + bh - half,
                    HANDLE_HIT,
                    HANDLE_HIT,
                    CursorStyle::ResizeUpLeftDownRight,
                ),
                Handle::T => (
                    bx + half,
                    by - half,
                    (bw - HANDLE_HIT).max(0.),
                    HANDLE_HIT,
                    CursorStyle::ResizeUpDown,
                ),
                Handle::B => (
                    bx + half,
                    by + bh - half,
                    (bw - HANDLE_HIT).max(0.),
                    HANDLE_HIT,
                    CursorStyle::ResizeUpDown,
                ),
                Handle::L => (
                    bx - half,
                    by + half,
                    HANDLE_HIT,
                    (bh - HANDLE_HIT).max(0.),
                    CursorStyle::ResizeLeftRight,
                ),
                Handle::R => (
                    bx + bw - half,
                    by + half,
                    HANDLE_HIT,
                    (bh - HANDLE_HIT).max(0.),
                    CursorStyle::ResizeLeftRight,
                ),
            };
            div()
                .id(SharedString::from(format!("crop-handle-{h:?}")))
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(w))
                .h(px(hh))
                .cursor(cursor)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        ed.start_crop_drag(Some(h), e.position, cx);
                    }),
                )
        });

        stage
            .child(
                div()
                    .absolute()
                    .left(px(ix))
                    .top(px(iy))
                    .w(px(iw))
                    .h(px(ih))
                    .overflow_hidden()
                    .child(img(image.clone()).absolute().inset_0().size_full()),
            )
            // Four panels cover everything outside the box.
            .child(dim().left_0().top_0().right_0().h(px(by)))
            .child(dim().left_0().right_0().bottom_0().top(px(by + bh)))
            .child(dim().left_0().top(px(by)).w(px(bx)).h(px(bh)))
            .child(dim().right_0().top(px(by)).left(px(bx + bw)).h(px(bh)))
            .child(frame)
            .child(
                div()
                    .id("crop-move")
                    .absolute()
                    .left(px(bx))
                    .top(px(by))
                    .w(px(bw))
                    .h(px(bh))
                    .cursor(if matches!(self.drag, Some(Drag::Crop(_))) {
                        CursorStyle::ClosedHand
                    } else {
                        CursorStyle::OpenHand
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|ed, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            ed.start_crop_drag(None, e.position, cx);
                        }),
                    ),
            )
            .children(handles)
            .into_any_element()
    }
}
