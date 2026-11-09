//! Visual › Face Swap (video clips): a face from a photo on a person in the
//! clip, on this computer.
//!
//! The panel walks through it: the models (downloaded once, licences
//! shown first), the source photo (after the user confirms they may use
//! that face), the person to swap (found in the clip, picked from
//! thumbnails), the settings, and processing (progress, cancel, errors).
//! The clip stores the settings (`faceSwap`); the processed result lives
//! in the cache and is pasted over the clip's frames in the player and
//! export. The original media is never changed.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    AnyElement, Context, MouseButton, PathPromptOptions, RenderImage, SharedString, div, img,
    prelude::*, px,
};
use image::RgbImage;

use crate::decoder::DecodedFrame;
use crate::editor::{Confirm, Editor};
use crate::engine::LayerKind;
use crate::face_swap::cache::Results;
use crate::face_swap::config::{self, photo_hash, source_range};
use crate::face_swap::models::{self, ModelSpec};
use crate::face_swap::{
    FaceSwapJob, FaceSwapProgress, FaceSwapSettings, Quality, TargetAnchor, job, pipeline,
};
use crate::i18n::{t, tf};
use crate::model::Clip;
use crate::storage::format_bytes;
use crate::theme;
use crate::ui::widgets::{
    choice_pill, panel_note, panel_row, pick_tile, pill_button, section, section_with, switch,
};

/// A model download running (one at a time, all the missing ones).
pub struct ModelDownload {
    cancel: Arc<AtomicBool>,
    pub name: &'static str,
    pub got: u64,
    pub total: u64,
}

pub enum PeopleState {
    Finding(Arc<AtomicBool>),
    Found(Vec<PersonTile>),
    Failed(String),
}

pub struct PersonTile {
    pub anchor: TargetAnchor,
    pub image: Arc<RenderImage>,
    pub sightings: usize,
}

pub struct ProcessJob {
    pub progress: FaceSwapProgress,
    cancel: Arc<AtomicBool>,
}

/// A frame with its face pasted in, kept while the same frame and result
/// show (the player composes again on every look change).
struct Composed {
    raw: usize,
    key: String,
    opacity: u16,
    frame: Arc<DecodedFrame>,
}

#[derive(Default)]
pub struct FaceSwapUi {
    pub results: Results,
    composed: HashMap<String, Composed>,
    pub download: Option<ModelDownload>,
    pub download_error: Option<String>,
    /// By clip id.
    pub people: HashMap<String, PeopleState>,
    pub jobs: HashMap<String, ProcessJob>,
    pub errors: HashMap<String, String>,
    /// Source photos' thumbnails by path (None: loading or unreadable).
    photos: HashMap<String, Option<Arc<RenderImage>>>,
}

/// An RGB picture as gpui wants it (BGRA in an RGBA buffer).
fn render_image(rgb: &RgbImage) -> Option<Arc<RenderImage>> {
    let (w, h) = rgb.dimensions();
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for p in rgb.pixels() {
        bgra.extend_from_slice(&[p.0[2], p.0[1], p.0[0], 255]);
    }
    let buffer = image::RgbaImage::from_raw(w, h, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// A face photo's thumbnail: a square around its face when one is found
/// (the models loaded), else its middle.
fn photo_thumb(path: &str) -> Option<RgbImage> {
    let (w, h, bgra) = pipeline::load_photo(path).ok()?;
    let frame = crate::face_swap::types::FrameView {
        width: w,
        height: h,
        bgra: &bgra,
    };
    let face = if models::all_present(&models::REQUIRED) {
        pipeline::with_models(true, |m| pipeline::photo_face(m, &frame)).ok()
    } else {
        None
    };
    let bbox = face.map(|f| f.bbox).unwrap_or_else(|| {
        let side = w.min(h) as f32 / 1.5;
        crate::face_swap::types::FaceBox {
            x: (w as f32 - side) / 2.,
            y: (h as f32 - side) / 2.,
            w: side,
            h: side,
        }
    });
    Some(pipeline::face_thumb(&frame, &bbox, 96))
}

impl Editor {
    // ---- the player and export ---------------------------------------------------------

    /// The player's frames (one per wanted layer) with face swaps pasted
    /// in where a video clip has a processed one.
    pub(crate) fn face_swap_frames(
        &mut self,
        layers: &[(String, String, LayerKind)],
        frames: Vec<Arc<DecodedFrame>>,
    ) -> Vec<Arc<DecodedFrame>> {
        let mut out = Vec::with_capacity(frames.len());
        for ((key, clip_id, kind), frame) in layers.iter().zip(frames) {
            let clip = (*kind == LayerKind::Video)
                .then(|| self.project.find_clip(clip_id))
                .flatten()
                .map(|(_, c)| c)
                .filter(|c| c.extra.contains_key(config::KEY));
            let Some((result, opacity)) = clip.and_then(|c| self.face_swap.results.for_clip(c))
            else {
                out.push(frame);
                continue;
            };
            let raw = Arc::as_ptr(&frame) as usize;
            let op = (opacity * 1000.) as u16;
            let cached = self
                .face_swap
                .composed
                .get(key)
                .filter(|c| c.raw == raw && c.key == result.header.key && c.opacity == op);
            if let Some(c) = cached {
                out.push(c.frame.clone());
                continue;
            }
            let shown = job::composite(&frame, &result, opacity).map(Arc::new);
            match shown {
                Some(f) => {
                    self.face_swap.composed.insert(
                        key.clone(),
                        Composed {
                            raw,
                            key: result.header.key.clone(),
                            opacity: op,
                            frame: f.clone(),
                        },
                    );
                    out.push(f);
                }
                None => out.push(frame),
            }
        }
        out
    }

    /// A face swap changed (settings, a new result): the player composes
    /// again.
    pub(crate) fn face_swap_changed(&mut self, cx: &mut Context<Self>) {
        self.face_swap.composed.clear();
        self.bump_frame_seq();
        cx.notify();
    }

    /// Selects `clip_id` with its Face Swap tab open.
    pub(crate) fn select_clip_for_face_swap(&mut self, clip_id: String, cx: &mut Context<Self>) {
        self.selected.set(clip_id);
        self.inspector_entry = Some("faceSwap");
        self.layers_panel = None;
        cx.notify();
    }

    /// Loads the selected clip's face photo thumbnail when it's missing.
    pub(crate) fn load_face_swap_photo(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self
            .selected_clip()
            .and_then(|(_, c)| c.face_swap())
            .map(|s| s.source)
            .filter(|p| !p.is_empty())
        else {
            return;
        };
        if self.face_swap.photos.contains_key(&path) {
            return;
        }
        self.face_swap.photos.insert(path.clone(), None);
        let p = path.clone();
        let task = cx
            .background_executor()
            .spawn(async move { photo_thumb(&p) });
        cx.spawn(async move |this, cx| {
            let thumb = task.await;
            this.update(cx, |ed, cx| {
                ed.face_swap
                    .photos
                    .insert(path, thumb.as_ref().and_then(render_image));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- edits ------------------------------------------------------------------------

    fn face_swap_of(&self, clip_id: &str) -> FaceSwapSettings {
        self.project
            .find_clip(clip_id)
            .and_then(|(_, c)| c.face_swap())
            .unwrap_or_default()
    }

    /// One undoable change to a clip's face swap.
    fn update_face_swap(
        &mut self,
        clip_id: &str,
        label: &'static str,
        f: impl FnOnce(&mut FaceSwapSettings),
        cx: &mut Context<Self>,
    ) {
        let mut s = self.face_swap_of(clip_id);
        f(&mut s);
        self.face_swap.errors.remove(clip_id);
        self.edit_clip(
            clip_id,
            label,
            move |c| {
                if c.face_swap().as_ref() == Some(&s) {
                    return false;
                }
                c.set_face_swap(Some(&s));
                true
            },
            cx,
        );
        self.face_swap_changed(cx);
    }

    fn remove_face_swap(&mut self, clip_id: &str, cx: &mut Context<Self>) {
        self.cancel_face_swap(clip_id);
        self.face_swap.people.remove(clip_id);
        self.face_swap.errors.remove(clip_id);
        self.edit_clip(
            clip_id,
            "Remove Face Swap",
            |c| {
                let had = c.face_swap().is_some();
                c.set_face_swap(None);
                had
            },
            cx,
        );
        self.face_swap_changed(cx);
    }

    /// Choose photo: a picture from disk, used once the user confirms they
    /// may use that face.
    fn pick_face_photo(&mut self, clip_id: &str, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("Choose photo").into()),
        });
        let id = clip_id.to_string();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.first().map(|p| p.to_string_lossy().into_owned()) else {
                return;
            };
            this.update(cx, |ed, cx| ed.confirm_face_photo(id, path, cx))
                .ok();
        })
        .detach();
    }

    fn confirm_face_photo(&mut self, clip_id: String, path: String, cx: &mut Context<Self>) {
        if image::ImageReader::open(&path)
            .and_then(|r| r.with_guessed_format())
            .ok()
            .and_then(|r| r.format())
            .is_none()
        {
            self.toast("That file isn't a photo", cx);
            return;
        }
        self.confirm = Some(Confirm {
            title: t("Use this face?").into(),
            message: t(
                "Only use a face you have the right to use: your own, or that of someone who agreed \
                 to it. Don't use face swap to pass as someone else or to mislead. Videos with a \
                 swapped face are marked as AI-edited.",
            )
            .into(),
            confirm_label: t("I have permission").into(),
            run: Rc::new(move |ed: &mut Editor, _, cx| {
                let hash = match photo_hash(&path) {
                    Ok(h) => h,
                    Err(e) => {
                        ed.toast(e.to_string(), cx);
                        return;
                    }
                };
                let p = path.clone();
                ed.update_face_swap(
                    &clip_id,
                    "Face Swap Photo",
                    move |s| {
                        s.source = p;
                        s.source_hash = hash;
                        s.consent = true;
                    },
                    cx,
                );
            }),
        });
        cx.notify();
    }

    /// Finds the people in the clip (a few frames across it), for the panel.
    fn find_face_swap_people(&mut self, clip_id: &str, cx: &mut Context<Self>) {
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let Some(media) = clip.source.clone() else {
            return;
        };
        let (start, end) = source_range(clip);
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(PeopleState::Finding(old)) = self
            .face_swap
            .people
            .insert(clip_id.to_string(), PeopleState::Finding(cancel.clone()))
        {
            old.store(true, Ordering::Relaxed);
        }
        let flag = cancel.clone();
        let task = cx
            .background_executor()
            .spawn(async move { job::find_people(&media, start, end, true, &flag) });
        let id = clip_id.to_string();
        cx.spawn(async move |this, cx| {
            let found = task.await;
            this.update(cx, |ed, cx| {
                let ours = matches!(ed.face_swap.people.get(&id), Some(PeopleState::Finding(c)) if Arc::ptr_eq(c, &cancel));
                if !ours {
                    return;
                }
                let state = match found {
                    Ok(people) => PeopleState::Found(
                        people
                            .into_iter()
                            .filter_map(|p| {
                                Some(PersonTile {
                                    image: render_image(&p.thumb)?,
                                    anchor: p.anchor,
                                    sightings: p.sightings,
                                })
                            })
                            .collect(),
                    ),
                    Err(e) => PeopleState::Failed(e.to_string()),
                };
                if let Some(PeopleState::Found(old)) = ed.face_swap.people.insert(id, state) {
                    ed.stale_frames.extend(old.into_iter().map(|t| t.image));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Processes the clip's face swap in the background.
    fn process_face_swap(&mut self, clip_id: &str, cx: &mut Context<Self>) {
        let Some((_, clip)) = self.project.find_clip(clip_id) else {
            return;
        };
        let (Some(media), Some(settings)) = (clip.source.clone(), clip.face_swap()) else {
            return;
        };
        if let Err(e) = settings.ready() {
            self.toast(e.to_string(), cx);
            return;
        }
        let mut need: Vec<&'static ModelSpec> = models::REQUIRED.to_vec();
        if settings.config().restore {
            need.push(&models::RESTORER);
        }
        if !models::all_present(&need) {
            self.ask_face_models(settings.config().restore, cx);
            return;
        }
        let (start, end) = source_range(clip);
        let key = settings.cache_key(&media);
        let job = FaceSwapJob {
            media,
            settings,
            start,
            end,
            key,
        };
        self.cancel_face_swap(clip_id);
        self.face_swap.errors.remove(clip_id);
        let cancel = Arc::new(AtomicBool::new(false));
        self.face_swap.jobs.insert(
            clip_id.to_string(),
            ProcessJob {
                progress: FaceSwapProgress::LoadingModels,
                cancel: cancel.clone(),
            },
        );
        enum Msg {
            Progress(FaceSwapProgress),
            Done(Result<crate::face_swap::FaceSwapResult, crate::face_swap::FaceSwapError>),
        }
        let (tx, rx) = async_channel::unbounded();
        let flag = cancel.clone();
        std::thread::Builder::new()
            .name("face swap".into())
            .spawn(move || {
                let progress_tx = tx.clone();
                let r = job::process(&job, &flag, &|p| {
                    let _ = progress_tx.try_send(Msg::Progress(p));
                });
                let _ = tx.send_blocking(Msg::Done(r));
            })
            .ok();
        let id = clip_id.to_string();
        cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                let gone = this
                    .update(cx, |ed, cx| {
                        let ours = ed
                            .face_swap
                            .jobs
                            .get(&id)
                            .is_some_and(|j| Arc::ptr_eq(&j.cancel, &cancel));
                        if !ours {
                            return true;
                        }
                        match msg {
                            Msg::Progress(p) => {
                                if let Some(j) = ed.face_swap.jobs.get_mut(&id) {
                                    j.progress = p;
                                }
                            }
                            Msg::Done(r) => {
                                ed.face_swap.jobs.remove(&id);
                                match r {
                                    Ok(done) => {
                                        ed.face_swap.results.forget(&done.key);
                                        eprintln!(
                                            "face swap: {} frames ({} with the face) in {:.1} s on {}",
                                            done.frames, done.swapped, done.seconds, done.provider
                                        );
                                        if done.swapped == 0 {
                                            ed.face_swap.errors.insert(
                                                id.clone(),
                                                t("The person wasn't found in this clip").into(),
                                            );
                                        } else {
                                            ed.toast(t("Face swap ready"), cx);
                                        }
                                        ed.face_swap_changed(cx);
                                    }
                                    Err(crate::face_swap::FaceSwapError::Cancelled) => {}
                                    Err(e) => {
                                        ed.face_swap.errors.insert(id.clone(), e.to_string());
                                    }
                                }
                                cx.notify();
                                return true;
                            }
                        }
                        cx.notify();
                        false
                    })
                    .unwrap_or(true);
                if gone {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn cancel_face_swap(&mut self, clip_id: &str) {
        if let Some(j) = self.face_swap.jobs.remove(clip_id) {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Asks before downloading the models (their sizes and licences), then
    /// downloads the missing ones.
    pub(crate) fn ask_face_models(&mut self, restorer: bool, cx: &mut Context<Self>) {
        let mut specs: Vec<&'static ModelSpec> = models::REQUIRED.to_vec();
        if restorer {
            specs.push(&models::RESTORER);
        }
        let missing: Vec<&'static ModelSpec> = specs.into_iter().filter(|m| !m.present()).collect();
        if missing.is_empty() {
            return;
        }
        let bytes: u64 = missing.iter().map(|m| m.bytes).sum();
        let lines: Vec<String> = missing
            .iter()
            .map(|m| {
                format!(
                    "• {} by {} ({}): {}",
                    t(m.name),
                    m.author,
                    m.home.trim_start_matches("https://"),
                    t(m.license.label())
                )
            })
            .collect();
        let non_commercial = missing.iter().any(|m| !m.license.commercial());
        let mut message = format!(
            "{}\n\n{}",
            tf(
                "Face swap runs on this computer with these models ({size}), downloaded once:",
                &[("size", &format_bytes(bytes))]
            ),
            lines.join("\n")
        );
        if non_commercial {
            message.push_str("\n\n");
            message.push_str(t(
                "Models marked non-commercial may only be used for personal and research \
                 projects, not for commercial work. By downloading you accept their licences.",
            ));
        }
        self.confirm = Some(Confirm {
            title: t("Download the face models?").into(),
            message: message.into(),
            confirm_label: t("Download").into(),
            run: Rc::new(move |ed: &mut Editor, _, cx| {
                ed.download_face_models(missing.clone(), cx)
            }),
        });
        cx.notify();
    }

    fn download_face_models(&mut self, specs: Vec<&'static ModelSpec>, cx: &mut Context<Self>) {
        if self.face_swap.download.is_some() || specs.is_empty() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let total: u64 = specs.iter().map(|m| m.bytes).sum();
        self.face_swap.download_error = None;
        self.face_swap.download = Some(ModelDownload {
            cancel: cancel.clone(),
            name: specs[0].name,
            got: 0,
            total,
        });
        enum Msg {
            At(&'static str, u64),
            Done(Result<(), crate::face_swap::FaceSwapError>),
        }
        let (tx, rx) = async_channel::unbounded();
        let flag = cancel.clone();
        std::thread::Builder::new()
            .name("face models".into())
            .spawn(move || {
                let mut before = 0u64;
                let mut result = Ok(());
                for m in specs {
                    let at = before;
                    let t2 = tx.clone();
                    let r = models::download(m, &flag, &|got| {
                        let _ = t2.try_send(Msg::At(m.name, at + got));
                    });
                    if let Err(e) = r {
                        result = Err(e);
                        break;
                    }
                    before += m.bytes;
                }
                let _ = tx.send_blocking(Msg::Done(result));
            })
            .ok();
        cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                let gone = this
                    .update(cx, |ed, cx| {
                        let ours = ed
                            .face_swap
                            .download
                            .as_ref()
                            .is_some_and(|d| Arc::ptr_eq(&d.cancel, &cancel));
                        if !ours {
                            return true;
                        }
                        let done = match msg {
                            Msg::At(name, got) => {
                                if let Some(d) = ed.face_swap.download.as_mut() {
                                    d.name = name;
                                    d.got = got;
                                }
                                false
                            }
                            Msg::Done(r) => {
                                ed.face_swap.download = None;
                                match r {
                                    Ok(()) => ed.toast(t("Face models downloaded"), cx),
                                    Err(crate::face_swap::FaceSwapError::Cancelled) => {}
                                    Err(e) => ed.face_swap.download_error = Some(e.to_string()),
                                }
                                // Thumbnails can now centre on the face.
                                ed.face_swap.photos.clear();
                                true
                            }
                        };
                        cx.notify();
                        done
                    })
                    .unwrap_or(true);
                if gone {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    // ---- the panel --------------------------------------------------------------------

    pub fn render_face_swap_panel(&self, clip: &Clip, cx: &mut Context<Self>) -> AnyElement {
        let s = clip.face_swap();
        let has = s.is_some();
        let s = s.unwrap_or_default();
        let id = clip.id.clone();
        let mut out: Vec<AnyElement> = vec![];

        let toggle_id = id.clone();
        out.push(
            section_with(
                "Face Swap",
                true,
                [switch("face-swap-on", has && s.enabled, !has)
                    .when(has, |d| {
                        let on = s.enabled;
                        d.on_click(cx.listener(move |ed, _, _, cx| {
                            ed.update_face_swap(&toggle_id, "Face Swap", |s| s.enabled = !on, cx)
                        }))
                    })
                    .into_any_element()],
            )
            .into_any_element(),
        );
        out.push(
            panel_note(
                "Puts a face from a photo on a person in this clip. It runs on this computer; \
                 nothing is uploaded.",
            )
            .into_any_element(),
        );

        // The models first.
        if let Some(d) = &self.face_swap.download {
            out.push(self.face_model_progress(d, cx));
            return column(out);
        }
        if !models::all_present(&models::REQUIRED) {
            out.push(self.face_model_offer(cx));
            return column(out);
        }

        // Source face.
        out.push(section("Source face", false).into_any_element());
        let photo = (!s.source.is_empty())
            .then(|| self.face_swap.photos.get(&s.source).cloned().flatten())
            .flatten();
        let pick_id = id.clone();
        out.push(
            div()
                .flex()
                .items_center()
                .gap_3()
                .py_1()
                .child(face_square("face-photo", photo, s.source.is_empty()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            pill_button(
                                "face-photo-pick",
                                if s.source.is_empty() {
                                    "Choose photo"
                                } else {
                                    "Replace"
                                },
                                s.source.is_empty(),
                            )
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(
                                cx.listener(move |ed, _, _, cx| ed.pick_face_photo(&pick_id, cx)),
                            ),
                        )
                        .when(!s.source.is_empty(), |d| {
                            d.child(
                                div()
                                    .max_w(px(150.))
                                    .truncate()
                                    .text_size(px(theme::XS))
                                    .text_color(theme::text_tertiary())
                                    .child(
                                        std::path::Path::new(&s.source)
                                            .file_name()
                                            .map(|n| n.to_string_lossy().into_owned())
                                            .unwrap_or_default(),
                                    ),
                            )
                        }),
                )
                .into_any_element(),
        );

        // The person to swap.
        let find_id = id.clone();
        let finding = matches!(
            self.face_swap.people.get(&clip.id),
            Some(PeopleState::Finding(_))
        );
        out.push(
            section_with(
                "Person to swap",
                false,
                [pill_button(
                    "face-find",
                    if finding {
                        "Finding…"
                    } else {
                        "Detect faces"
                    },
                    false,
                )
                .when(!finding, |d| {
                    d.on_click(
                        cx.listener(move |ed, _, _, cx| ed.find_face_swap_people(&find_id, cx)),
                    )
                })
                .into_any_element()],
            )
            .into_any_element(),
        );
        match self.face_swap.people.get(&clip.id) {
            Some(PeopleState::Found(people)) if !people.is_empty() => {
                let tiles = people.iter().enumerate().map(|(i, p)| {
                    let on = s.target.as_ref().is_some_and(|t| same_person(t, &p.anchor));
                    let anchor = p.anchor.clone();
                    let cid = id.clone();
                    pick_tile(
                        ("face-person", i),
                        64.,
                        on,
                        div()
                            .relative()
                            .size_full()
                            .overflow_hidden()
                            .child(img(p.image.clone()).absolute().inset_0().size_full()),
                        Some(SharedString::from(tf(
                            "Seen {count}×",
                            &[("count", &p.sightings)],
                        ))),
                    )
                    .on_click(cx.listener(move |ed, _, _, cx| {
                        let a = anchor.clone();
                        ed.update_face_swap(
                            &cid,
                            "Face Swap Person",
                            move |s| s.target = Some(a),
                            cx,
                        )
                    }))
                    .into_any_element()
                });
                out.push(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .py_1()
                        .children(tiles)
                        .into_any_element(),
                );
            }
            Some(PeopleState::Found(_)) => {
                out.push(panel_note("No faces found in this clip.").into_any_element())
            }
            Some(PeopleState::Failed(e)) => out.push(error_note(e.clone())),
            Some(PeopleState::Finding(_)) => {}
            None => out.push(
                panel_note(if s.target.is_some() {
                    "A person is picked. Detect faces to pick someone else."
                } else {
                    "Detect faces, then pick the person whose face is swapped."
                })
                .into_any_element(),
            ),
        }

        // Settings.
        out.push(section("Settings", false).into_any_element());
        let blend_id = id.clone();
        out.push(self.fx_slider(
            "faceSwap:blend",
            "Blend",
            s.blend.round(),
            (0., 100.),
            false,
            "",
            move |ed, v, cx| ed.update_face_swap(&blend_id, "Face Swap Blend", |s| s.blend = v, cx),
            cx,
        ));
        let (fast_id, high_id) = (id.clone(), id.clone());
        out.push(
            panel_row("Quality")
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(
                            choice_pill("face-q-fast", "Fast", s.quality == Quality::Fast, true)
                                .on_click(cx.listener(move |ed, _, _, cx| {
                                    ed.update_face_swap(
                                        &fast_id,
                                        "Face Swap Quality",
                                        |s| s.quality = Quality::Fast,
                                        cx,
                                    )
                                })),
                        )
                        .child(
                            choice_pill("face-q-high", "High", s.quality == Quality::High, true)
                                .on_click(cx.listener(move |ed, _, _, cx| {
                                    ed.update_face_swap(
                                        &high_id,
                                        "Face Swap Quality",
                                        |s| s.quality = Quality::High,
                                        cx,
                                    )
                                })),
                        ),
                )
                .into_any_element(),
        );
        let high = s.quality == Quality::High;
        let restore_id = id.clone();
        let restore_on = s.restore && high;
        out.push(
            panel_row("Restore")
                .child(div().flex_1().flex().justify_end().child(
                    switch("face-restore", restore_on, !high).when(high, |d| {
                        d.on_click(cx.listener(move |ed, _, _, cx| {
                            if !restore_on && !models::RESTORER.present() {
                                ed.ask_face_models(true, cx);
                                return;
                            }
                            ed.update_face_swap(
                                &restore_id,
                                "Face Swap Restoration",
                                |s| s.restore = !restore_on,
                                cx,
                            )
                        }))
                    }),
                ))
                .into_any_element(),
        );
        out.push(
            panel_note(if high {
                "High reads frames at full size; restoration sharpens the face (slower)."
            } else {
                "Fast reads frames at up to 1280 px. Choose High for restoration."
            })
            .into_any_element(),
        );

        // Processing.
        out.push(section("Processing", false).into_any_element());
        let result = (has && s.ready().is_ok())
            .then(|| self.face_swap.results.processed(clip, &s))
            .flatten();
        let processed = result.is_some();
        match self.face_swap.jobs.get(&clip.id) {
            Some(job) => out.push(self.face_job_progress(&clip.id, job, cx)),
            None => {
                if let Some(e) = self.face_swap.errors.get(&clip.id) {
                    out.push(error_note(e.clone()));
                }
                let status = match s.ready() {
                    Err(e) => e.to_string(),
                    Ok(()) if processed => tf(
                        "Processed ({count} frames): the player and export show the swapped face.",
                        &[("count", &result.as_ref().map_or(0, |r| r.frames()))],
                    ),
                    Ok(()) => t("Not processed yet. The player shows the original until it is.")
                        .to_string(),
                };
                out.push(panel_note(status).into_any_element());
                let go_id = id.clone();
                let ready = has && s.ready().is_ok();
                out.push(
                    div()
                        .flex()
                        .gap_2()
                        .py_1()
                        .child(
                            pill_button(
                                "face-process",
                                if processed {
                                    "Process again"
                                } else {
                                    "Process"
                                },
                                ready && !processed,
                            )
                            .when(!ready, |d| d.opacity(0.4).cursor_default())
                            .when(ready, |d| {
                                d.on_click(
                                    cx.listener(move |ed, _, _, cx| {
                                        ed.process_face_swap(&go_id, cx)
                                    }),
                                )
                            }),
                        )
                        .into_any_element(),
                );
            }
        }

        if has {
            out.push(
                div()
                    .mt_2()
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .bg(theme::fill_subtle())
                    .text_size(px(theme::XS))
                    .text_color(theme::text_secondary())
                    .child(t(
                        "AI-edited: exports of this project say in their details that a face was \
                         swapped.",
                    ))
                    .into_any_element(),
            );
            let rm_id = id.clone();
            out.push(
                div()
                    .py_2()
                    .child(
                        pill_button("face-remove", "Remove face swap", false).on_click(
                            cx.listener(move |ed, _, _, cx| ed.remove_face_swap(&rm_id, cx)),
                        ),
                    )
                    .into_any_element(),
            );
        }
        column(out)
    }

    /// The models aren't here: what they are and a Download button.
    fn face_model_offer(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut rows: Vec<AnyElement> = vec![section("AI models", false).into_any_element()];
        for m in models::REQUIRED {
            rows.push(model_row(m));
        }
        rows.push(
            panel_note(
                "The face swap and identity models are licensed for non-commercial use only.",
            )
            .into_any_element(),
        );
        if let Some(e) = &self.face_swap.download_error {
            rows.push(error_note(e.clone()));
        }
        let size = format_bytes(models::missing_bytes(&models::REQUIRED));
        rows.push(
            div()
                .py_1()
                .child(
                    pill_button(
                        "face-models-get",
                        tf("Download ({size})", &[("size", &size)]),
                        true,
                    )
                    .on_click(cx.listener(|ed, _, _, cx| ed.ask_face_models(false, cx))),
                )
                .into_any_element(),
        );
        // Inside the panel's column already: no padding of its own.
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .children(rows)
            .into_any_element()
    }

    fn face_model_progress(&self, d: &ModelDownload, cx: &mut Context<Self>) -> AnyElement {
        let fraction = d.got as f32 / d.total.max(1) as f32;
        progress_card(
            tf("Downloading {name}…", &[("name", &d.name)]),
            Some(fraction),
            format!("{} / {}", format_bytes(d.got), format_bytes(d.total)),
            pill_button("face-models-cancel", "Cancel", false)
                .on_click(cx.listener(|ed, _, _, cx| {
                    if let Some(d) = ed.face_swap.download.take() {
                        d.cancel.store(true, Ordering::Relaxed);
                    }
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    fn face_job_progress(
        &self,
        clip_id: &str,
        job: &ProcessJob,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let detail = match &job.progress {
            FaceSwapProgress::Processing { frames, .. } => {
                tf("{count} frames", &[("count", frames)])
            }
            _ => String::new(),
        };
        let id = clip_id.to_string();
        progress_card(
            t(job.progress.label()).to_string(),
            job.progress.fraction(),
            detail,
            pill_button("face-cancel", "Cancel", false)
                .on_click(cx.listener(move |ed, _, _, cx| {
                    ed.cancel_face_swap(&id);
                    cx.notify();
                }))
                .into_any_element(),
        )
    }
}

/// The same pick (within a hair: the anchors come from the same search).
fn same_person(a: &TargetAnchor, b: &TargetAnchor) -> bool {
    a.time == b.time
        && a.bbox
            .iter()
            .zip(&b.bbox)
            .all(|(x, y)| (x - y).abs() < 1e-4)
}

fn column(children: Vec<AnyElement>) -> AnyElement {
    div()
        .px(px(14.))
        .pb(px(14.))
        .flex()
        .flex_col()
        .gap(px(6.))
        .children(children)
        .into_any_element()
}

fn model_row(m: &ModelSpec) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .py(px(3.))
        .text_size(px(theme::XS))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(if m.present() {
                    theme::text_tertiary()
                } else {
                    theme::text()
                })
                .child(t(m.name)),
        )
        .child(
            div()
                .flex_none()
                .text_color(if m.license.commercial() {
                    theme::text_tertiary()
                } else {
                    theme::text_secondary()
                })
                .child(t(m.license.label())),
        )
        .child(
            div()
                .flex_none()
                .w(px(54.))
                .text_right()
                .text_color(theme::text_tertiary())
                .child(if m.present() {
                    t("Ready").to_string()
                } else {
                    format_bytes(m.bytes)
                }),
        )
        .into_any_element()
}

fn error_note(e: String) -> AnyElement {
    div()
        .text_size(px(theme::XS))
        .line_height(theme::line_height(16., theme::XS))
        .text_color(theme::danger())
        .child(e)
        .into_any_element()
}

fn face_square(id: &'static str, image: Option<Arc<RenderImage>>, empty: bool) -> impl IntoElement {
    div()
        .id(id)
        .flex_none()
        .relative()
        .size(px(64.))
        .rounded(px(8.))
        .overflow_hidden()
        .bg(theme::button_bg())
        .flex()
        .items_center()
        .justify_center()
        .map(|d| match image {
            Some(i) => d.child(img(i).absolute().inset_0().size_full()),
            None => d.child(
                div()
                    .text_size(px(theme::XS))
                    .text_color(theme::text_tertiary())
                    .child(if empty { t("No photo") } else { "…" }),
            ),
        })
}

fn progress_card(
    label: String,
    fraction: Option<f32>,
    detail: String,
    cancel: AnyElement,
) -> AnyElement {
    div()
        .p(px(12.))
        .rounded(px(8.))
        .border_1()
        .border_color(theme::chip_border())
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .truncate()
                        .text_size(px(theme::SM))
                        .text_color(theme::text())
                        .child(label),
                )
                .children(fraction.map(|p| {
                    crate::ui::widgets::mono(
                        format!("{}%", (p * 100.).round()),
                        theme::XS,
                        theme::text_secondary(),
                    )
                })),
        )
        .child(
            div()
                .h(px(4.))
                .w_full()
                .rounded(px(2.))
                .bg(theme::fill_muted())
                .child(
                    div()
                        .h_full()
                        .rounded(px(2.))
                        .bg(theme::text())
                        .w(gpui::relative(fraction.unwrap_or(0.).clamp(0., 1.))),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(theme::XS))
                        .text_color(theme::text_tertiary())
                        .child(detail),
                )
                .child(cancel),
        )
        .into_any_element()
}
