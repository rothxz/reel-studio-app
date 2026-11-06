//! The bar over the timeline: undo / redo, the Select / Split tool and
//! Delete left / right, the selected clip's one-shot actions, then on the
//! right the keyframe ◇, the timeline switches (main track magnet, linkage,
//! preview axis, auto snapping, fit) and zoom.

use gpui::{Context, MouseButton, MouseDownEvent, div, prelude::*, px};

use crate::editor::{Drag, Editor, MAX_PPS, MIN_PPS, Tool};
use crate::shortcuts::Shortcut;
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets::{icon_button, slider_track, vdivider};

/// A clip action on the toolbar: the clip tools that aren't settings (the
/// inspector has those). Only the ones the native editor can run yet.
struct ClipAction {
    key: &'static str,
    label: &'static str,
    icon: Icon,
    shortcut: Option<Shortcut>,
}

/// The selected clip's actions, in the old editor's order for its kind.
fn clip_actions(kind: &str) -> Vec<ClipAction> {
    let split = ClipAction {
        key: "split",
        label: "Split",
        icon: Icon::Split,
        shortcut: Some(Shortcut::SplitClip),
    };
    let delete = ClipAction {
        key: "delete",
        label: "Delete",
        icon: Icon::Trash,
        shortcut: Some(Shortcut::DeleteClip),
    };
    let duplicate = ClipAction {
        key: "duplicate",
        label: "Duplicate",
        icon: Icon::Copy,
        shortcut: Some(Shortcut::Duplicate),
    };
    match kind {
        "sticker" => vec![delete, duplicate],
        "effect" | "drawing" => vec![split, duplicate, delete],
        "video" | "image" | "audio" | "text" => vec![split, delete, duplicate],
        _ => vec![delete, duplicate],
    }
}

/// Zoom ↔ slider position: logarithmic, so each step moves it equally.
fn pps_to_fraction(pps: f32) -> f32 {
    ((pps / MIN_PPS).ln() / (MAX_PPS / MIN_PPS).ln()).clamp(0., 1.)
}

impl Editor {
    pub fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let split_tool = self.tool == Tool::Split;
        let selected_kind = self.selected_clip().map(|(_, c)| c.kind.clone());
        let p = &self.prefs;

        let clip_buttons: Vec<_> = match &selected_kind {
            Some(kind) => clip_actions(kind)
                .into_iter()
                .map(|a| {
                    let key = a.key;
                    icon_button(
                        gpui::SharedString::from(format!("clip-action-{key}")),
                        a.icon,
                        a.label,
                    )
                    .shortcut_opt(a.shortcut)
                    .on_click(cx.listener(move |ed, _, w, cx| match key {
                        "split" => ed.run(Shortcut::SplitClip, w, cx),
                        "delete" => ed.run(Shortcut::DeleteClip, w, cx),
                        "duplicate" => ed.run(Shortcut::Duplicate, w, cx),
                        _ => {}
                    }))
                    .into_any_element()
                })
                .collect(),
            // Nothing selected: Split cuts the main clip under the playhead,
            // Delete waits for a selection.
            None => vec![
                icon_button("idle-split", Icon::Split, "Split")
                    .shortcut(Shortcut::SplitClip)
                    .on_click(cx.listener(|ed, _, w, cx| ed.run(Shortcut::SplitClip, w, cx)))
                    .into_any_element(),
                icon_button("idle-delete", Icon::Trash, "Delete")
                    .shortcut(Shortcut::DeleteClip)
                    .disabled(true)
                    .into_any_element(),
            ],
        };

        let zoom_dragging = matches!(self.drag, Some(Drag::Zoom));

        div()
            .flex_none()
            .h(px(36.))
            .px_2()
            .flex()
            .items_center()
            .gap(px(2.))
            .border_b_1()
            .border_color(theme::card_border())
            .child(
                icon_button("undo", Icon::Undo, "Undo")
                    .size(17.)
                    .shortcut(Shortcut::Undo)
                    .disabled(!self.history.can_undo())
                    .on_click(cx.listener(|ed, _, w, cx| ed.run(Shortcut::Undo, w, cx))),
            )
            .child(
                icon_button("redo", Icon::Redo, "Redo")
                    .size(17.)
                    .shortcut(Shortcut::Redo)
                    .disabled(!self.history.can_redo())
                    .on_click(cx.listener(|ed, _, w, cx| ed.run(Shortcut::Redo, w, cx))),
            )
            .child(vdivider())
            .child(
                icon_button(
                    "tool",
                    if split_tool {
                        Icon::Blade
                    } else {
                        Icon::Cursor
                    },
                    if split_tool { "Split tool" } else { "Select" },
                )
                .shortcut(if split_tool {
                    Shortcut::SelectTool
                } else {
                    Shortcut::SplitTool
                })
                .active(split_tool)
                .on_click(cx.listener(move |ed, _, w, cx| {
                    ed.run(
                        if split_tool {
                            Shortcut::SelectTool
                        } else {
                            Shortcut::SplitTool
                        },
                        w,
                        cx,
                    )
                })),
            )
            .child(
                icon_button("delete-left", Icon::DeleteLeft, "Delete left")
                    .shortcut(Shortcut::DeleteLeft)
                    .on_click(cx.listener(|ed, _, w, cx| ed.run(Shortcut::DeleteLeft, w, cx))),
            )
            .child(
                icon_button("delete-right", Icon::DeleteRight, "Delete right")
                    .shortcut(Shortcut::DeleteRight)
                    .on_click(cx.listener(|ed, _, w, cx| ed.run(Shortcut::DeleteRight, w, cx))),
            )
            .children(clip_buttons)
            .child(div().flex_1())
            // Keyframes aren't editable yet: the ◇ shows, disabled.
            .child(
                icon_button("keyframe", Icon::RhombusOutline, "Add keyframe")
                    .size(15.)
                    .disabled(true),
            )
            .child(vdivider())
            .child(
                icon_button("magnet", Icon::Magnet, "Main track magnet")
                    .size(15.)
                    .shortcut(Shortcut::MainMagnet)
                    .active(p.main_track_magnet)
                    .on_click(cx.listener(|ed, _, w, cx| ed.run(Shortcut::MainMagnet, w, cx))),
            )
            .child(
                icon_button(
                    "linkage",
                    if p.linkage { Icon::Link } else { Icon::LinkOff },
                    "Linkage",
                )
                .active(p.linkage)
                .on_click(cx.listener(|ed, _, _, cx| {
                    ed.prefs.linkage = !ed.prefs.linkage;
                    ed.prefs.save();
                    cx.notify();
                })),
            )
            .child(
                icon_button("preview-axis", Icon::PreviewAxis, "Preview axis")
                    .shortcut(Shortcut::PreviewAxis)
                    .active(p.preview_axis)
                    .on_click(cx.listener(|ed, _, w, cx| ed.run(Shortcut::PreviewAxis, w, cx))),
            )
            .child(
                icon_button("snapping", Icon::Snapping, "Auto snapping")
                    .shortcut(Shortcut::Snapping)
                    .active(p.snapping)
                    .on_click(cx.listener(|ed, _, w, cx| ed.run(Shortcut::Snapping, w, cx))),
            )
            .child(
                icon_button("zoom-fit", Icon::ZoomFit, "Zoom to fit")
                    .shortcut(Shortcut::ZoomFit)
                    .on_click(cx.listener(|ed, _, w, cx| ed.run(Shortcut::ZoomFit, w, cx))),
            )
            .child(vdivider())
            .child(
                icon_button("zoom-out", Icon::ZoomOut, "Zoom out")
                    .shortcut(Shortcut::ZoomOut)
                    .on_click(cx.listener(|ed, _, w, cx| ed.run(Shortcut::ZoomOut, w, cx))),
            )
            .child(
                div()
                    .id("zoom")
                    .relative()
                    .w(px(96.))
                    .cursor_pointer()
                    .child(self.measure("zoom"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|ed, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            ed.drag = Some(Drag::Zoom);
                            ed.zoom_slider_to(e.position.x, cx);
                        }),
                    )
                    .child(slider_track(pps_to_fraction(self.pps), zoom_dragging)),
            )
            .child(
                icon_button("zoom-in", Icon::ZoomIn, "Zoom in")
                    .shortcut(Shortcut::ZoomIn)
                    .on_click(cx.listener(|ed, _, w, cx| ed.run(Shortcut::ZoomIn, w, cx))),
            )
    }
}
