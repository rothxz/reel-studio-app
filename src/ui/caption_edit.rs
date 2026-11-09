//! The inspector's Edit captions tab (a caption selected): every caption
//! in time order, the one under the playhead lit, the selected one open
//! in the text field. Tools over the list: Merge up / Merge down with the
//! neighbour, Split at the text cursor (time cut by the words' share),
//! Delete, and Find & replace (inline under the tools). Each is one undo
//! step; the timeline's own move / trim / Split work on captions too.

use gpui::{Context, Entity, FontWeight, IntoElement, ParentElement, Styled, div, prelude::*, px};

use crate::captions;
use crate::editor::Editor;
use crate::i18n::{plural, t};
use crate::model::{Clip, SECOND};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::text_input::{InputEvent, TextInput};
use crate::ui::widgets::{icon_button, mono, pill_button};

/// The tab's own state: Find & replace's fields and whether it's open.
pub struct CaptionEditUi {
    pub find_open: bool,
    pub find: Entity<TextInput>,
    pub replace: Entity<TextInput>,
}

impl CaptionEditUi {
    pub fn new(cx: &mut Context<Editor>) -> Self {
        let find = cx.new(|cx| TextInput::new(t("Find"), false, cx).max_len(200));
        let replace = cx.new(|cx| TextInput::new(t("Replace with"), false, cx).max_len(200));
        for field in [&find, &replace] {
            // The match count follows the typing.
            cx.subscribe(field, |_: &mut Editor, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Changed(_)) {
                    cx.notify();
                }
            })
            .detach();
        }
        CaptionEditUi {
            find_open: false,
            find,
            replace,
        }
    }
}

/// mm:ss of a timeline time: the list's timestamps.
fn clock(t: i64) -> String {
    let s = (t.max(0) / SECOND) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

impl Editor {
    pub fn render_caption_edit(&self, clip: &Clip, cx: &mut Context<Self>) -> gpui::AnyElement {
        let list: Vec<Clip> = captions::all(&self.project).into_iter().cloned().collect();
        let index = list.iter().position(|c| c.id == clip.id);
        let now = self.playhead;
        let active = list
            .iter()
            .find(|c| now >= c.timeline_start && now < c.end())
            .map(|c| c.id.clone());
        let caret = self.text_field.read(cx).caret_chars();
        let text = clip.text.clone().unwrap_or_default();
        let can_split = captions::cut_text(&text, captions::syllable_edge(&text, caret)).is_some();
        let n = list.len();

        let tools = div()
            .flex_none()
            .h(px(36.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(2.))
            .child(
                icon_button("cap-merge-up", Icon::MergeUp, "Merge up")
                    .disabled(index.is_none_or(|i| i == 0))
                    .on_click(cx.listener(|ed, _, _, cx| ed.merge_caption(true, cx))),
            )
            .child(
                icon_button("cap-merge-down", Icon::MergeDown, "Merge down")
                    .disabled(index.is_none_or(|i| i + 1 >= n))
                    .on_click(cx.listener(|ed, _, _, cx| ed.merge_caption(false, cx))),
            )
            .child(
                icon_button("cap-split", Icon::Split, "Split")
                    .disabled(!can_split)
                    .on_click(cx.listener(|ed, _, _, cx| ed.split_caption_at_caret(cx))),
            )
            .child(
                icon_button("cap-delete", Icon::Trash, "Delete")
                    .on_click(cx.listener(|ed, _, _, cx| ed.delete_caption(cx))),
            )
            .child(div().flex_1())
            .child(
                icon_button("cap-find", Icon::Search, "Find & replace")
                    .active(self.caption_edit.find_open)
                    .on_click(cx.listener(|ed, _, window, cx| {
                        ed.caption_edit.find_open = !ed.caption_edit.find_open;
                        if ed.caption_edit.find_open {
                            ed.caption_edit.find.read(cx).focus(window);
                        }
                        cx.notify();
                    })),
            );

        let find = self.caption_edit.find_open.then(|| {
            let query = self.caption_edit.find.read(cx).text().to_string();
            let count = captions::count_matches(&self.project, &query);
            let field = |f: &Entity<TextInput>| {
                div()
                    .h(px(30.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .bg(theme::lane_bg())
                    .border_1()
                    .border_color(theme::chip_border())
                    .text_size(px(theme::SM))
                    .text_color(theme::text())
                    .child(f.clone())
            };
            div()
                .flex_none()
                .px(px(12.))
                .pb(px(10.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(field(&self.caption_edit.find))
                .child(field(&self.caption_edit.replace))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(mono(
                            if query.is_empty() {
                                String::new()
                            } else {
                                plural(
                                    "captionEdit.matches",
                                    count,
                                    "{count} match",
                                    "{count} matches",
                                )
                            },
                            theme::XS,
                            theme::text_tertiary(),
                        ))
                        .child(
                            pill_button("cap-replace-all", "Replace all", count > 0)
                                .when(count > 0, |d| {
                                    d.on_click(
                                        cx.listener(|ed, _, _, cx| ed.replace_in_captions(cx)),
                                    )
                                })
                                .when(count == 0, |d| d.opacity(0.5)),
                        ),
                )
        });

        let rows = list.iter().enumerate().map(|(i, c)| {
            let editing = c.id == clip.id;
            let bright = editing || active.as_deref() == Some(c.id.as_str());
            let id = c.id.clone();
            let start = c.timeline_start;
            div()
                .id(("caption-row", i))
                .flex()
                .gap(px(10.))
                .px(px(12.))
                .py(px(8.))
                .when(editing, |d| d.bg(theme::button_bg()))
                .when(!editing, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.bg(theme::button_hover()))
                        .on_click(cx.listener(move |ed, _, _, cx| {
                            ed.pause();
                            ed.seek(start, cx);
                            ed.selected.set(id.clone());
                            ed.focus_text_field = true;
                            cx.notify();
                        }))
                })
                .child(div().flex_none().w(px(40.)).pt(px(1.)).child(mono(
                    clock(c.timeline_start),
                    theme::XS,
                    if bright {
                        theme::text()
                    } else {
                        theme::text_tertiary()
                    },
                )))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.))
                        .line_height(theme::line_height(18., 13.))
                        .text_color(if bright {
                            theme::text()
                        } else {
                            theme::text_secondary()
                        })
                        .when(editing, |d| d.font_weight(FontWeight::MEDIUM))
                        .map(|d| {
                            if editing {
                                d.child(self.text_field.clone())
                            } else {
                                d.child(c.text.clone().unwrap_or_default())
                            }
                        }),
                )
        });

        div()
            .flex()
            .flex_col()
            .child(tools)
            .children(find)
            .child(
                div()
                    .border_t_1()
                    .border_color(theme::card_border())
                    .children(rows),
            )
            .into_any_element()
    }

    /// The selected caption and its neighbours in time order.
    fn caption_neighbours(&self) -> Option<(Option<String>, String, Option<String>)> {
        let (_, clip) = self.selected_clip().filter(|(_, c)| c.is_caption())?;
        let list = captions::all(&self.project);
        let i = list.iter().position(|c| c.id == clip.id)?;
        Some((
            i.checked_sub(1).map(|j| list[j].id.clone()),
            clip.id.clone(),
            list.get(i + 1).map(|c| c.id.clone()),
        ))
    }

    /// Merge up / down: the selected caption and its neighbour as one.
    pub fn merge_caption(&mut self, up: bool, cx: &mut Context<Self>) {
        let Some((prev, id, next)) = self.caption_neighbours() else {
            return;
        };
        let (first, second) = if up {
            let Some(p) = prev else { return };
            (p, id)
        } else {
            let Some(n) = next else { return };
            (id, n)
        };
        let mut kept = None;
        self.edit(
            "Merge Captions",
            false,
            None,
            |p| {
                kept = captions::merge(p, &first, &second);
                kept.is_some()
            },
            cx,
        );
        if let Some(k) = kept {
            self.selected.set(k);
            self.focus_text_field = true;
            cx.notify();
        }
    }

    /// Split: the selected caption cut at the text cursor.
    pub fn split_caption_at_caret(&mut self, cx: &mut Context<Self>) {
        let Some((_, id, _)) = self.caption_neighbours() else {
            return;
        };
        let caret = self.text_field.read(cx).caret_chars();
        let mut right = None;
        self.edit(
            "Split Caption",
            false,
            None,
            |p| {
                right = captions::split_at_char(p, &id, caret);
                right.is_some()
            },
            cx,
        );
        match right {
            Some(r) => {
                self.selected.set(r);
                self.focus_text_field = true;
                cx.notify();
            }
            None => self.toast(t("Can't split here — put the cursor between two words"), cx),
        }
    }

    /// Delete: the selected caption; the next one (else the one before)
    /// is selected.
    pub fn delete_caption(&mut self, cx: &mut Context<Self>) {
        let Some((prev, id, next)) = self.caption_neighbours() else {
            return;
        };
        if self.edit(
            "Delete Caption",
            false,
            None,
            |p| p.delete_clip(&id).is_some(),
            cx,
        ) {
            match next.or(prev) {
                Some(n) => self.selected.set(n),
                None => self.selected.clear(),
            }
            cx.notify();
        }
    }

    /// Replace all: every match in every caption, one undo step.
    pub fn replace_in_captions(&mut self, cx: &mut Context<Self>) {
        let find = self.caption_edit.find.read(cx).text().to_string();
        let replace = self.caption_edit.replace.read(cx).text().to_string();
        let mut n = 0;
        self.edit(
            "Replace in Captions",
            false,
            None,
            |p| {
                n = captions::replace_all(p, &find, &replace);
                n > 0
            },
            cx,
        );
        if n > 0 {
            self.toast(
                plural(
                    "captionEdit.replaced",
                    n,
                    "Replaced in {count} caption",
                    "Replaced in {count} captions",
                ),
                cx,
            );
        }
    }
}
