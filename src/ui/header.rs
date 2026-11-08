//! The title bar: ✕ and save state on the left, the project name (with a
//! Beta badge) in the middle, Shortcuts · export resolution · Export on the
//! right. It is the window's title bar too: padded clear of the traffic
//! lights (macOS), with the app menu and the window's buttons elsewhere
//! (`window_chrome`), and its empty parts move the window.

use gpui::{Context, FontWeight, MouseButton, Window, div, prelude::*, px};

use crate::editor::{Editor, HEADER_HEIGHT, PopupItem};
use crate::export;
use crate::i18n::t;
use crate::prefs::RESOLUTIONS;
use crate::shortcuts::{self, Shortcut};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets::{chip, icon_button, label_text, tooltip};
use crate::ui::window_chrome::{self, drag_area};

/// Room for the macOS traffic lights (placed at 12, 12).
const TRAFFIC_LIGHTS: f32 = 70.;

impl Editor {
    pub fn render_header(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dirty = self.dirty;
        let resolution = RESOLUTIONS[self.prefs.export_resolution.min(RESOLUTIONS.len() - 1)].0;
        let full_screen = window.is_fullscreen();
        let controls = window_chrome::draws_controls(window);

        div()
            .id("header")
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .flex()
            .items_center()
            .pl(px(if cfg!(target_os = "macos") && !full_screen {
                TRAFFIC_LIGHTS
            } else {
                8.
            }))
            .pr(px(if controls { 0. } else { 8. }))
            .border_b_1()
            .border_color(theme::card_border())
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(window_chrome::draws_menu(), |d| {
                        d.child(self.render_menu_button(cx))
                    })
                    .child(
                        icon_button("close-project", Icon::Close, "Back to projects")
                            .size(17.)
                            .on_click(cx.listener(|ed, _, _, cx| ed.close_project(cx))),
                    )
                    .child(div().w(px(1.)).h(px(18.)).bg(theme::chip_border()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .child(if dirty {
                                Icon::Ellipse.el(8., theme::text_tertiary())
                            } else {
                                Icon::CheckmarkCircle.el(14., theme::success())
                            })
                            .child(label_text(
                                if dirty {
                                    "Unsaved changes"
                                } else {
                                    "Saved · Local"
                                },
                                theme::text_tertiary(),
                            )),
                    )
                    .child(drag_area("header-drag-left")),
            )
            .child(
                div()
                    .id("project-name")
                    .flex()
                    .items_center()
                    .gap_1()
                    .max_w(px(360.))
                    .h(px(26.))
                    .px(px(10.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::button_hover()))
                    .tooltip(tooltip("Edit name", None))
                    .on_click(cx.listener(|ed, _, _, cx| {
                        // The project sheet (rename, details) isn't ported
                        // yet: the inspector's Details shows the project.
                        ed.selected.clear();
                        cx.notify();
                    }))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(theme::SM))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.project.name.clone()),
                    )
                    .child(Icon::ChevronDown.el(12., theme::text_secondary())),
            )
            .child(
                div()
                    .h(px(18.))
                    .px(px(6.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .border_1()
                    .border_color(theme::chip_border())
                    .bg(theme::chip_bg())
                    .child(
                        div()
                            .font_family(theme::font_mono())
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(theme::XXS))
                            .child(t("BETA")),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(drag_area("header-drag-right"))
                    .child(
                        chip("ai-agent")
                            .when(self.agent.settings.open, |d| {
                                d.bg(theme::button_hover())
                                    .border_color(theme::text_tertiary())
                            })
                            .tooltip(tooltip("AI agent: edit by asking", None))
                            .on_click(cx.listener(|ed, _, w, cx| ed.agent_toggle(w, cx)))
                            .child(Icon::Sparkles.el(15., theme::text()))
                            .child(
                                div()
                                    .text_size(px(theme::XS))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(t("AI agent")),
                            ),
                    )
                    .child(
                        chip("shortcuts")
                            .tooltip(tooltip(
                                "Keyboard shortcuts",
                                Some(shortcuts::hint(Shortcut::ShowShortcuts)),
                            ))
                            .on_click(
                                cx.listener(|ed, _, w, cx| ed.run(Shortcut::ShowShortcuts, w, cx)),
                            )
                            .child(Icon::Keyboard.el(16., theme::text()))
                            .child(
                                div()
                                    .text_size(px(theme::XS))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(t("Shortcuts")),
                            ),
                    )
                    .child(
                        chip("resolution")
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|ed, e: &gpui::ClickEvent, _, cx| {
                                let mut items: Vec<PopupItem> = RESOLUTIONS
                                    .iter()
                                    .enumerate()
                                    .map(|(i, (label, _))| {
                                        PopupItem::new(*label, move |ed: &mut Editor, _, cx| {
                                            ed.prefs.export_resolution = i;
                                            ed.prefs.save();
                                            cx.notify();
                                        })
                                        .checked(ed.prefs.export_resolution == i)
                                    })
                                    .collect();
                                // The frame rate: the project's own (snapped)
                                // until one is picked.
                                let fps = ed
                                    .export_fps
                                    .unwrap_or_else(|| export::snap_fps(ed.project.canvas.fps));
                                items.push(PopupItem::separator());
                                items.extend(export::FRAME_RATES.iter().map(|&rate| {
                                    PopupItem::new(
                                        format!("{rate} fps"),
                                        move |ed: &mut Editor, _, cx| {
                                            ed.export_fps = Some(rate);
                                            cx.notify();
                                        },
                                    )
                                    .checked(fps == rate)
                                }));
                                ed.open_popup(e.position(), items, cx);
                            }))
                            .child(
                                div()
                                    .text_size(px(theme::XS))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(resolution),
                            )
                            .child(Icon::ChevronDown.el(12., theme::text())),
                    )
                    .child(
                        div()
                            .id("export")
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .h(px(26.))
                            .px(px(14.))
                            .rounded(px(6.))
                            .bg(theme::text())
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.88))
                            .tooltip(tooltip(
                                "Export video",
                                Some(shortcuts::hint(Shortcut::Export)),
                            ))
                            .on_click(cx.listener(|ed, _, w, cx| ed.run(Shortcut::Export, w, cx)))
                            .child(Icon::Share.el(14., theme::app_bg()))
                            .child(
                                div()
                                    .text_size(px(theme::XS))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::app_bg())
                                    .child(t("Export")),
                            ),
                    ),
            )
            .when(controls, |d| {
                d.child(div().w(px(8.)))
                    .child(self.render_window_controls(window, cx))
            })
    }
}
