//! The projects home, before the editor: a greeting and the drafts count,
//! the New video card (browse, or drop files anywhere on the window), and
//! the Recent list, newest edit first. A click opens a project; a right
//! click offers Open / Delete.

use std::rc::Rc;

use gpui::{
    Context, FontWeight, MouseButton, MouseDownEvent, ObjectFit, SharedString, Window, div, img,
    prelude::*, px, rgba,
};

use crate::editor::{Confirm, Editor, HEADER_HEIGHT, ImportMedia, PopupItem};
use crate::projects::{self, Summary};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets::mono;

const CONTENT_WIDTH: f32 = 880.;
const CARD_HEIGHT: f32 = 118.;
const THUMB_W: f32 = 60.;
const THUMB_H: f32 = 76.;

/// "Good morning" until noon, afternoon until 6 pm, evening after.
pub fn greeting() -> &'static str {
    match local_hour() {
        h if h < 12 => "Good morning",
        h if h < 18 => "Good afternoon",
        _ => "Good evening",
    }
}

/// The local hour, from `date` (std has no time zones; this runs once per
/// visit to the list, not per frame).
fn local_hour() -> u32 {
    std::process::Command::new("date")
        .arg("+%H")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(12)
}

impl Editor {
    pub fn render_home(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.home_projects.len();
        let drafts = format!(
            "{count} DRAFT{} · LOCAL ONLY",
            if count == 1 { "" } else { "S" }
        );
        let full_screen = window.is_fullscreen();

        let rows: Vec<_> = self
            .home_projects
            .iter()
            .enumerate()
            .map(|(i, p)| self.project_row(i, p, cx).into_any_element())
            .collect();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::page_bg())
            // The title bar strip: clear of the traffic lights, double-click zooms.
            .child(
                div()
                    .id("home-titlebar")
                    .flex_none()
                    .h(px(HEADER_HEIGHT))
                    .when(!full_screen, |d| d.pl(px(80.)))
                    .on_click(|e, window, _| {
                        if e.click_count() == 2 {
                            window.titlebar_double_click();
                        }
                    }),
            )
            .child(
                div()
                    .id("home-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(CONTENT_WIDTH))
                            .px(px(20.))
                            .pb(px(120.))
                            .flex()
                            .flex_col()
                            .gap(px(12.))
                            .child(
                                div()
                                    .pt_1()
                                    .pb_3()
                                    .child(
                                        div()
                                            .text_size(px(22.))
                                            .font_weight(FontWeight::BOLD)
                                            .child(self.home_greeting),
                                    )
                                    .child(div().mt(px(2.)).child(mono(
                                        drafts,
                                        9.5,
                                        theme::text_faint(),
                                    ))),
                            )
                            .child(self.new_video_card(cx))
                            .child(
                                div()
                                    .mt(px(20.))
                                    .mb_1()
                                    .text_size(px(theme::LG))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Recent"),
                            )
                            .when(rows.is_empty(), |d| {
                                d.child(
                                    div()
                                        .pt(px(60.))
                                        .flex()
                                        .justify_center()
                                        .text_size(px(15.))
                                        .text_color(theme::text_faint())
                                        .child("No projects yet. Create your first edit."),
                                )
                            })
                            .children(rows),
                    ),
            )
    }

    fn new_video_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("new-video")
            .relative()
            .h(px(CARD_HEIGHT))
            .rounded(px(16.))
            .bg(theme::accent())
            .cursor_pointer()
            .active(|s| s.opacity(0.85))
            .hover(|s| s.opacity(0.94))
            .on_click(cx.listener(|ed, _, w, cx| ed.import(&ImportMedia, w, cx)))
            .child(
                div()
                    .absolute()
                    .left(px(14.))
                    .top(px(14.))
                    .size(px(36.))
                    .rounded_full()
                    .bg(rgba(0x1a160824))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::Videocam.el(20., theme::accent_text())),
            )
            .child(
                div()
                    .absolute()
                    .left(px(14.))
                    .bottom(px(14.))
                    .child(
                        div()
                            .text_size(px(theme::LG))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::accent_text())
                            .child("New video"),
                    )
                    .child(
                        div()
                            .mt(px(2.))
                            .text_size(px(11.5))
                            .text_color(theme::accent_text())
                            .opacity(0.6)
                            .child("Browse or drop video files"),
                    ),
            )
    }

    fn project_row(&self, i: usize, p: &Summary, cx: &mut Context<Self>) -> impl IntoElement {
        let thumb = p
            .preview
            .as_ref()
            .and_then(|(src, _)| self.thumbs.get(src).cloned());
        let open_id = p.id.clone();
        let menu_id = p.id.clone();
        let name: SharedString = p.name.clone().into();
        div()
            .id(("project", i))
            .flex()
            .items_center()
            .gap(px(14.))
            .py_1()
            .px_2()
            .mx(px(-8.))
            .rounded(px(10.))
            .cursor_pointer()
            .hover(|s| s.bg(theme::fill_subtle()))
            .on_click(cx.listener(move |ed, _, _, cx| ed.open_saved_project(&open_id, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                    let (open, delete, name) = (menu_id.clone(), menu_id.clone(), name.clone());
                    let items = vec![
                        PopupItem {
                            label: "Open".into(),
                            checked: false,
                            run: Rc::new(move |ed: &mut Editor, _, cx| {
                                ed.open_saved_project(&open, cx)
                            }),
                        },
                        PopupItem {
                            label: "Delete…".into(),
                            checked: false,
                            run: Rc::new(move |ed: &mut Editor, _, cx| {
                                let id = delete.clone();
                                ed.confirm = Some(Confirm {
                                    title: "Delete 1 project?".into(),
                                    message: format!("“{name}”. This can't be undone.").into(),
                                    confirm_label: "Delete".into(),
                                    run: Rc::new(move |ed: &mut Editor, _, cx| {
                                        ed.delete_saved_project(&id, cx)
                                    }),
                                });
                                cx.notify();
                            }),
                        },
                    ];
                    ed.open_popup(e.position, items, cx);
                }),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w(px(THUMB_W))
                    .h(px(THUMB_H))
                    .rounded(px(10.))
                    .overflow_hidden()
                    .bg(theme::fill_subtle())
                    .flex()
                    .items_center()
                    .justify_center()
                    .map(|d| match thumb {
                        Some(t) => d.child(
                            img(t)
                                .absolute()
                                .inset_0()
                                .size_full()
                                .object_fit(ObjectFit::Cover),
                        ),
                        None => d.child(Icon::Film.el(20., theme::text_faint())),
                    })
                    .child(
                        div()
                            .absolute()
                            .right(px(4.))
                            .bottom(px(4.))
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(px(5.))
                            .bg(rgba(0x0000008c))
                            .text_size(px(theme::XXS))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(gpui::white())
                            .child(projects::clock(p.duration)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(theme::LG))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(p.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(theme::SM))
                            .text_color(theme::text_secondary())
                            .child(projects::relative_time(&p.updated_at)),
                    ),
            )
            .child(Icon::ChevronForward.el(18., theme::text_disabled()))
    }
}
