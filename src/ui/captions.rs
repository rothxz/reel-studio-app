//! The browser's Captions tab.
//!
//! - **Auto captions**: the spoken language, the style (a click opens the
//!   Style view), what will be captioned (the selected clips with sound,
//!   else every video clip) and Generate. While the job runs (`speech.rs`,
//!   on its own thread: switching tabs doesn't stop it) the same spot shows
//!   its phase, progress and Cancel. Under it, Upload file adds the cues of
//!   an .srt / .vtt / .ass at their own times.
//! - **Style**: the caption styles (All / Monoline / Multiline in the side
//!   nav). A pick restyles every caption, one undo step, and is the look
//!   the next captions take.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    AnyElement, Context, FontWeight, PathPromptOptions, SharedString, div, prelude::*, px, rgba,
};

use crate::caption_styles::{self, CaptionStyle, TABS};
use crate::captions;
use crate::editor::{Editor, PopupItem};
use crate::i18n::{plural, t};
use crate::speech::{self, Phase, Update};
use crate::theme;
use crate::ui::browser::CAPTIONS_TAB;
use crate::ui::icons::Icon;
use crate::ui::widgets::pill_button;

const INSET: f32 = 16.;
const TILE_MIN: f32 = 92.;
const TILE_GAP: f32 = 8.;
const STYLE_GRID: &str = "caption-styles";

/// A running auto-captions job.
pub struct CaptionJob {
    pub phase: Phase,
    pub progress: Option<f32>,
    cancel: Arc<AtomicBool>,
}

fn heading(label: &'static str) -> gpui::Div {
    div()
        .flex_none()
        .h(px(36.))
        .px(px(INSET))
        .flex()
        .items_end()
        .text_size(px(theme::SM))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::text())
        .child(t(label))
}

/// A grey label over its control.
fn field(label: &'static str, control: impl IntoElement) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(theme::XS))
                .text_color(theme::text_secondary())
                .child(t(label)),
        )
        .child(control)
}

/// A dropdown-looking button: its value and a chevron.
fn select_box(id: &'static str, value: impl Into<SharedString>) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(32.))
        .px(px(10.))
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(6.))
        .bg(theme::chip_bg())
        .border_1()
        .border_color(theme::chip_border())
        .cursor_pointer()
        .hover(|s| s.bg(theme::button_hover()))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(theme::XS))
                .text_color(theme::text())
                .child(value.into()),
        )
        .child(Icon::ChevronDown.el(12., theme::text_secondary()))
}

fn rule() -> gpui::Div {
    div().h(px(1.)).bg(theme::card_border())
}

fn small(text: impl Into<SharedString>, color: gpui::Hsla) -> gpui::Div {
    div()
        .text_size(px(theme::XS))
        .text_color(color)
        .child(text.into())
}

impl Editor {
    pub fn render_auto_captions_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let body = match &self.caption_job {
            Some(job) => self.caption_progress(job, cx).into_any_element(),
            None => self.caption_form(cx).into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(heading("Auto captions"))
            .child(
                div()
                    .id("auto-captions")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(INSET))
                    .pt(px(6.))
                    .pb(px(INSET))
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(small(
                        t("Turn what's said in your video into captions on the timeline."),
                        theme::text_tertiary().into(),
                    ))
                    .child(body)
                    .child(rule())
                    .child(self.subtitle_upload(cx)),
            )
            .into_any_element()
    }

    fn caption_form(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let language = self.prefs.caption_language.clone();
        let style = caption_styles::get(&self.prefs.caption_style);
        let (clips, selected) = captions::targets(&self.project, self.selected.ids());
        let can = !clips.is_empty();
        let scope: String = if !can {
            t("No clips with sound to caption").into()
        } else if selected {
            t("Selected clip").into()
        } else if clips.len() == 1 {
            crate::i18n::tf("{count} video clip", &[("count", &1)])
        } else {
            crate::i18n::tf("All {count} video clips", &[("count", &clips.len())])
        };
        let hint = !can && captions::has_audio_to_pick(&self.project);

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(field(
                "Speech language",
                select_box("caption-language", speech::language_name(&language)).on_click(
                    cx.listener(move |ed, e: &gpui::ClickEvent, _, cx| {
                        let items = speech::LANGUAGES
                            .iter()
                            .map(|&(code, name)| {
                                PopupItem::verbatim(name, move |ed, _, cx| {
                                    ed.prefs.caption_language = code.to_string();
                                    ed.prefs.save();
                                    cx.notify();
                                })
                                .checked(language == code)
                            })
                            .collect();
                        ed.open_popup(e.position(), items, cx);
                    }),
                ),
            ))
            .when(self.prefs.caption_language == "km", |d| {
                d.child(small(
                    t("Khmer uses a dedicated Khmer speech model (by iAny). It's good but not perfect — check captions before exporting."),
                    theme::text_secondary().into(),
                ))
            })
            .child(field(
                "Style",
                select_box("caption-style", style.label).on_click(cx.listener(|ed, _, _, cx| {
                    ed.browser_views[CAPTIONS_TAB] = 1;
                    cx.notify();
                })),
            ))
            .child(rule())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        if can { Icon::Film } else { Icon::Warning }
                            .el(14., theme::text_secondary()),
                    )
                    .child(small(scope, theme::text_secondary().into()).flex_1()),
            )
            .when(hint, |d| {
                d.child(small(
                    t("To caption an audio clip, select it on the timeline first."),
                    theme::text_tertiary().into(),
                ))
            })
            .child(
                pill_button("caption-generate", "Generate captions", true)
                    .w_full()
                    .when(!can, |d| d.opacity(0.4).cursor_default())
                    .when(can, |d| {
                        d.on_click(cx.listener(|ed, _, _, cx| ed.generate_captions(cx)))
                    }),
            )
            .child(small(
                t("Runs on this computer — nothing is uploaded. The first time downloads the speech model."),
                theme::text_tertiary().into(),
            ))
    }

    fn caption_progress(&self, job: &CaptionJob, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = match job.phase {
            Phase::Checking | Phase::Applying => None,
            _ => job.progress,
        };
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
                            .child(t(job.phase.label())),
                    )
                    .children(shown.map(|p| {
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
                    .rounded(px(2.))
                    .overflow_hidden()
                    .bg(rgba(0xffffff1a))
                    .child(
                        div()
                            .h_full()
                            .w(gpui::relative(shown.unwrap_or(0.08).clamp(0.02, 1.)))
                            .bg(theme::text()),
                    ),
            )
            .child(small(
                t("Keep editing — captions keep generating."),
                theme::text_tertiary().into(),
            ))
            .child(
                pill_button("caption-cancel", "Cancel", false)
                    .w_full()
                    .on_click(cx.listener(|ed, _, _, cx| {
                        if let Some(job) = ed.caption_job.take() {
                            job.cancel.store(true, Ordering::Relaxed);
                        }
                        cx.notify();
                    })),
            )
    }

    fn subtitle_upload(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .text_size(px(theme::SM))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text())
                    .child(t("Upload file")),
            )
            .child(
                div()
                    .id("caption-upload")
                    .h(px(64.))
                    .rounded(px(8.))
                    .border_1()
                    .border_dashed()
                    .border_color(theme::chip_border())
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::fill_subtle()))
                    .on_click(cx.listener(|ed, _, _, cx| ed.upload_subtitles(cx)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(Icon::CloudUpload.el(15., theme::text()))
                            .child(small(t("Choose a subtitle file"), theme::text().into())),
                    )
                    .child(crate::ui::widgets::mono(
                        ".srt · .vtt · .ass",
                        theme::XXS,
                        theme::text_tertiary(),
                    )),
            )
            .child(small(
                t("Captions keep the times in the file and replace captions already there."),
                theme::text_tertiary().into(),
            ))
    }

    /// Starts captioning what the form shows.
    fn generate_captions(&mut self, cx: &mut Context<Self>) {
        if self.caption_job.is_some() {
            self.toast(t("Captions are already being generated"), cx);
            return;
        }
        let (clips, _) = captions::targets(&self.project, self.selected.ids());
        if clips.is_empty() {
            return;
        }
        let language = self.prefs.caption_language.clone();
        let style = *caption_styles::get(&self.prefs.caption_style);
        let job = speech::Job {
            clips,
            language: language.clone(),
            // A short line: Khmer's syllables are wide.
            max_chars: if language == "km" { 26 } else { 32 } * style.lines.max(1) as usize,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.caption_job = Some(CaptionJob {
            phase: Phase::Checking,
            progress: None,
            cancel: cancel.clone(),
        });
        let (tx, rx) = async_channel::unbounded();
        let flag = cancel.clone();
        std::thread::spawn(move || {
            speech::run(job, flag, |u| {
                let _ = tx.send_blocking(u);
            })
        });
        cx.spawn(async move |this, cx| {
            while let Ok(update) = rx.recv().await {
                let gone = this
                    .update(cx, |ed, cx| {
                        // Cancelled (or replaced) meanwhile: not ours any more.
                        let ours = ed
                            .caption_job
                            .as_ref()
                            .is_some_and(|j| Arc::ptr_eq(&j.cancel, &cancel));
                        if !ours {
                            return true;
                        }
                        match update {
                            Update::Progress(phase, p) => {
                                if let Some(j) = ed.caption_job.as_mut() {
                                    j.phase = phase;
                                    j.progress = p;
                                }
                                cx.notify();
                                false
                            }
                            Update::Done(cues) => {
                                ed.caption_job = None;
                                ed.add_captions(&cues, &style, &language, cx);
                                true
                            }
                            Update::Failed(e) => {
                                ed.caption_job = None;
                                ed.toast(e, cx);
                                true
                            }
                        }
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

    /// Cues onto the timeline as one undo step, with a toast.
    fn add_captions(
        &mut self,
        cues: &[captions::Cue],
        style: &CaptionStyle,
        language: &str,
        cx: &mut Context<Self>,
    ) {
        if cues.is_empty() {
            self.toast(t("No speech found"), cx);
            return;
        }
        let mut n = 0;
        self.edit(
            "Add Captions",
            false,
            None,
            |p| {
                n = captions::place(p, cues, style, language);
                n > 0
            },
            cx,
        );
        if n == 0 {
            self.toast(
                t("The file's captions are all after the end of the video"),
                cx,
            );
        } else {
            self.toast(
                plural(
                    "captions.added",
                    n,
                    "Added {count} caption",
                    "Added {count} captions",
                ),
                cx,
            );
        }
    }

    /// Upload file: picks a subtitle file and adds its cues.
    fn upload_subtitles(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("Choose a subtitle file").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.first().cloned() else {
                return;
            };
            this.update(cx, |ed, cx| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let Ok(bytes) = std::fs::read(&path) else {
                    ed.toast(t("Couldn't read that file"), cx);
                    return;
                };
                let text = String::from_utf8_lossy(&bytes);
                let Some(format) = captions::subtitle_format(&name, &text) else {
                    ed.toast(t("That's not a subtitle file — use .srt, .vtt or .ass"), cx);
                    return;
                };
                let cues = captions::parse_subtitles(&text, format);
                if cues.is_empty() {
                    ed.toast(t("No captions found in that file"), cx);
                    return;
                }
                let style = *caption_styles::get(&ed.prefs.caption_style);
                let language = ed.prefs.caption_language.clone();
                ed.add_captions(&cues, &style, &language, cx);
            })
            .ok();
        })
        .detach();
    }

    // ---- Style ------------------------------------------------------------

    pub fn render_caption_styles_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let ci = self.browser_categories[CAPTIONS_TAB].min(TABS.len() - 1);
        let (key, _) = TABS[ci];
        let width = self
            .measured(STYLE_GRID)
            .map_or(0., |b| f32::from(b.size.width) - INSET * 2.);
        let columns = ((width + TILE_GAP) / (TILE_MIN + TILE_GAP)).floor().max(1.);
        let size = if width > 0. {
            ((width - TILE_GAP * (columns - 1.)) / columns).floor()
        } else {
            TILE_MIN
        };
        let current = self.prefs.caption_style.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(heading("Caption styles"))
            .child(
                div()
                    .id(STYLE_GRID)
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.measure(STYLE_GRID))
                    .child(
                        div()
                            .p(px(INSET))
                            .flex()
                            .flex_wrap()
                            .gap(px(TILE_GAP))
                            .children(caption_styles::in_tab(key).into_iter().map(|s| {
                                style_tile(s, s.id == current, size).on_click(
                                    cx.listener(move |ed, _, _, cx| ed.pick_caption_style(s, cx)),
                                )
                            })),
                    ),
            )
            .into_any_element()
    }

    /// Remembered for the next captions, and put on the current ones.
    fn pick_caption_style(&mut self, style: &'static CaptionStyle, cx: &mut Context<Self>) {
        self.prefs.caption_style = style.id.into();
        self.prefs.save();
        self.edit(
            "Caption Style",
            false,
            None,
            |p| captions::restyle(p, style),
            cx,
        );
        cx.notify();
    }
}

/// A style's tile: its sample in its font, colour and box.
fn style_tile(s: &'static CaptionStyle, on: bool, size: f32) -> gpui::Stateful<gpui::Div> {
    let (family, weight) = face(s.font);
    let sample = div()
        .px(px(6.))
        .py(px(2.))
        .rounded(px(4.))
        .font_family(family)
        .font_weight(if s.bold { FontWeight::BOLD } else { weight })
        .when(s.italic, |d| d.italic())
        .text_size(px(15.))
        .text_color(hex(s.color, 1.))
        .when_some(s.boxed, |d, (c, o, _)| d.bg(hex(c, o as f32)))
        .child("Aa");
    // Outline, shadow or glow: a ring of its colour under the sample.
    let halo = s.outline.map(|(c, _)| c).or(s.glow.map(|(c, _)| c));
    div()
        .id(SharedString::from(format!("caption-style:{}", s.id)))
        .w(px(size))
        .flex()
        .flex_col()
        .gap(px(5.))
        .cursor_pointer()
        .group("cs-tile")
        .child(
            div()
                .relative()
                .size(px(size))
                .rounded(px(6.))
                .overflow_hidden()
                .bg(rgba(0x3a3a40ff))
                .border(px(1.5))
                .border_color(if on { theme::text() } else { rgba(0xffffff12) })
                .when(!on, |d| {
                    d.group_hover("cs-tile", |s| s.border_color(rgba(0xffffff59)))
                })
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .rounded(px(6.))
                        .p(px(2.))
                        .when_some(halo, |d, c| d.bg(hex(c, 0.55)))
                        .child(sample),
                )
                .when(s.lines > 1, |d| {
                    d.child(div().absolute().bottom(px(5.)).left(px(5.)).child(
                        crate::ui::widgets::mono("2 LINES", theme::XXS, theme::text_tertiary()),
                    ))
                }),
        )
        .child(
            div()
                .truncate()
                .text_size(px(theme::XS))
                .text_color(if on {
                    theme::text()
                } else {
                    theme::text_secondary()
                })
                .child(s.label),
        )
}

/// A bundled face id (`Nunito_700Bold`) → its family and weight.
fn face(font: &str) -> (&'static str, FontWeight) {
    let family = if font.starts_with("IBMPlexMono") {
        "IBM Plex Mono"
    } else {
        "Nunito"
    };
    let weight = match font.rsplit('_').next().unwrap_or("") {
        w if w.starts_with("700") => FontWeight::BOLD,
        w if w.starts_with("600") => FontWeight::SEMIBOLD,
        w if w.starts_with("500") => FontWeight::MEDIUM,
        _ => FontWeight::NORMAL,
    };
    (family, weight)
}

fn hex(s: &str, alpha: f32) -> gpui::Rgba {
    let [r, g, b, _] = crate::effects::stack::hex_rgba(s);
    gpui::Rgba {
        r,
        g,
        b,
        a: alpha.clamp(0., 1.),
    }
}
