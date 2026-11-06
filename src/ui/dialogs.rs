//! Things drawn over the editor: a popup menu at the pointer, and the
//! keyboard Shortcuts dialog (header button, ⌘/), which lists the
//! shortcut table by group.

use gpui::{Context, FontWeight, MouseButton, div, prelude::*, px};

use crate::editor::{Editor, Popup};
use crate::shortcuts::{Group, SHORTCUTS, combo_keys};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets::{keycap, label_text};

const MENU_WIDTH: f32 = 220.;
const ITEM_HEIGHT: f32 = 28.;
const SEPARATOR_HEIGHT: f32 = 9.;

/// The open popup: a click outside closes it, a click on an item runs it.
pub fn popup_menu(
    popup: &Popup,
    viewport: gpui::Size<gpui::Pixels>,
    cx: &mut Context<Editor>,
) -> impl IntoElement {
    // Kept inside the window.
    let height: f32 = popup
        .items
        .iter()
        .map(|i| {
            if i.separator {
                SEPARATOR_HEIGHT
            } else {
                ITEM_HEIGHT
            }
        })
        .sum::<f32>()
        + 10.;
    let x = f32::from(popup.at.x)
        .min(f32::from(viewport.width) - MENU_WIDTH - 8.)
        .max(8.);
    let y = f32::from(popup.at.y)
        .min(f32::from(viewport.height) - height - 14.)
        .max(8.);
    let close = |ed: &mut Editor,
                 _: &gpui::MouseDownEvent,
                 _: &mut gpui::Window,
                 cx: &mut Context<Editor>| {
        ed.popup = None;
        cx.notify();
    };
    div()
        .id("popup-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .on_mouse_down(MouseButton::Left, cx.listener(close))
        .on_mouse_down(MouseButton::Right, cx.listener(close))
        .child(
            div()
                .absolute()
                .left(px(x))
                .top(px(y + 6.))
                .w(px(MENU_WIDTH))
                .p_1()
                .rounded(px(8.))
                .bg(theme::button_bg())
                .border_1()
                .border_color(theme::chip_border())
                .shadow_lg()
                .flex()
                .flex_col()
                .children(popup.items.iter().enumerate().map(|(i, item)| {
                    if item.separator {
                        return div()
                            .h(px(SEPARATOR_HEIGHT))
                            .flex()
                            .items_center()
                            .child(div().h(px(1.)).w_full().bg(theme::chip_border()))
                            .into_any_element();
                    }
                    let run = item.run.clone();
                    let disabled = item.disabled;
                    let color = if disabled {
                        theme::text_disabled()
                    } else if item.destructive {
                        theme::danger()
                    } else {
                        theme::text()
                    };
                    div()
                        .id(("popup-item", i))
                        .h(px(ITEM_HEIGHT))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(5.))
                        .text_size(px(theme::SM))
                        .text_color(color)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .when(!disabled, |d| {
                            d.cursor_pointer()
                                .hover(|s| s.bg(theme::button_hover()))
                                .on_click(cx.listener(move |ed, _, w, cx| {
                                    ed.popup = None;
                                    run(ed, w, cx);
                                    cx.notify();
                                }))
                        })
                        .child(div().w(px(14.)).child(if item.checked {
                            Icon::Checkmark.el(14., color).into_any_element()
                        } else {
                            div().into_any_element()
                        }))
                        .child(div().flex_1().child(item.label.clone()))
                        .when_some(item.hint.clone(), |d, hint| {
                            d.child(
                                div()
                                    .text_size(px(theme::XS))
                                    .text_color(theme::text_tertiary())
                                    .child(hint),
                            )
                        })
                        .into_any_element()
                })),
        )
}

pub fn shortcuts_dialog(cx: &mut Context<Editor>) -> impl IntoElement {
    fn close(
        ed: &mut Editor,
        _: &gpui::MouseDownEvent,
        _: &mut gpui::Window,
        cx: &mut Context<Editor>,
    ) {
        ed.shortcuts_open = false;
        cx.notify();
    }
    div()
        .id("shortcuts-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .bg(theme::backdrop())
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, cx.listener(close))
        .child(
            div()
                .id("shortcuts-dialog")
                .w(px(720.))
                .max_w(gpui::relative(0.92))
                .max_h(gpui::relative(0.86))
                .flex()
                .flex_col()
                .rounded(px(12.))
                .bg(theme::sheet())
                .border_1()
                .border_color(theme::card_border())
                .overflow_hidden()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex_none()
                        .h(px(52.))
                        .px(px(16.))
                        .flex()
                        .items_center()
                        .border_b_1()
                        .border_color(theme::card_border())
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(theme::LG))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Keyboard shortcuts"),
                        )
                        .child(
                            div()
                                .id("shortcuts-close")
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, cx.listener(close))
                                .child(Icon::Close.el(20., theme::text_secondary())),
                        ),
                )
                .child(
                    div()
                        .id("shortcuts-columns")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px(px(24.))
                        .py(px(16.))
                        .flex()
                        .flex_wrap()
                        .gap_x(px(40.))
                        .gap_y(px(20.))
                        .children(Group::ALL.iter().map(|group| {
                            div()
                                .w(px(300.))
                                .flex_grow()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .mb_1()
                                        .child(label_text(group.label(), theme::text_tertiary())),
                                )
                                .children(SHORTCUTS.iter().filter(|s| s.group == *group).map(|s| {
                                    div()
                                        .h(px(30.))
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .flex_1()
                                                .text_size(px(theme::SM))
                                                .text_color(theme::text_secondary())
                                                .child(s.label),
                                        )
                                        .child(div().flex().items_center().gap_1().children(
                                            combo_keys(s.combos[0]).into_iter().map(keycap),
                                        ))
                                }))
                        })),
                ),
        )
}

/// A yes / no question: Cancel, or the (destructive) confirm button.
pub fn confirm_dialog(
    confirm: &crate::editor::Confirm,
    cx: &mut Context<Editor>,
) -> impl IntoElement {
    let run = confirm.run.clone();
    div()
        .id("confirm-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .bg(theme::backdrop())
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|ed, _, _, cx| {
                ed.confirm = None;
                cx.notify();
            }),
        )
        .child(
            div()
                .id("confirm")
                .w(px(360.))
                .p(px(20.))
                .flex()
                .flex_col()
                .gap_2()
                .rounded(px(12.))
                .bg(theme::sheet())
                .border_1()
                .border_color(theme::card_border())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .text_size(px(theme::LG))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(confirm.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(theme::SM))
                        .text_color(theme::text_secondary())
                        .child(confirm.message.clone()),
                )
                .child(
                    div()
                        .mt_3()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            crate::ui::widgets::pill_button("confirm-cancel", "Cancel", false)
                                .on_click(cx.listener(|ed, _, _, cx| {
                                    ed.confirm = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            crate::ui::widgets::pill_button(
                                "confirm-ok",
                                confirm.confirm_label.clone(),
                                false,
                            )
                            .bg(theme::danger())
                            .hover(|s| s.opacity(0.88))
                            .on_click(cx.listener(
                                move |ed, _, w, cx| {
                                    ed.confirm = None;
                                    run(ed, w, cx);
                                    cx.notify();
                                },
                            )),
                        ),
                ),
        )
}
