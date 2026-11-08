//! The projects home, before the editor, laid out like the old desktop
//! shell: a left sidebar (Edit, Projects, Settings) beside the page.
//!
//! Edit: the greeting and drafts count, the New video card (a blank
//! project in the editor; files dropped anywhere on the window start one
//! with them) pinned over the list, shrinking to a slim bar as the list
//! scrolls, then Recent: the newest drafts, with "See all" into Projects.
//! Projects: every draft. A click opens a draft; a right click offers
//! Open / Select / Delete. Select mode ticks drafts for one Delete.

use std::rc::Rc;

use gpui::{
    AnyElement, Context, ExternalPaths, FontWeight, MouseButton, MouseDownEvent, ObjectFit,
    SharedString, Window, div, img, prelude::*, px, rgba,
};

use crate::editor::{Confirm, Editor, HEADER_HEIGHT, HomeTab, PopupItem};
use crate::projects::{self, Summary};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets::mono;
use crate::ui::window_chrome;

const SIDEBAR_WIDTH: f32 = 220.;
/// The page's side padding.
const PAGE_PAD: f32 = 20.;
/// Drafts the Edit page lists before "See all".
const HOME_RECENT_LIMIT: usize = 5;

const CARD_HEIGHT: f32 = 118.;
const COLLAPSED_HEIGHT: f32 = 52.;
/// Scroll distance over which the card collapses (the height it loses, so
/// the list under it never gaps).
const COLLAPSE_RANGE: f32 = CARD_HEIGHT - COLLAPSED_HEIGHT;
const CARD_PAD: f32 = 14.;
const COLLAPSED_PAD: f32 = 10.;
const ICON: f32 = 36.;
const ICON_SMALL: f32 = 30.;
const TITLE_LINE: f32 = 20.;
const SUB_LINE: f32 = 16.;
const TEXT_GAP: f32 = 8.;

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

/// The local hour (std has no time zones).
fn local_hour() -> u32 {
    use chrono::Timelike as _;
    chrono::Local::now().hour()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// A title-bar strip: its empty space moves the window (double-click
/// zooms it), with what goes before (the app menu) and after (the window's
/// buttons) where the system draws none.
fn title_strip(
    id: &'static str,
    lead: Option<gpui::AnyElement>,
    trail: Option<gpui::AnyElement>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(HEADER_HEIGHT))
        .flex()
        .items_center()
        .when_some(lead, |d, lead| d.pl(px(10.)).child(lead))
        .child(window_chrome::drag_area(gpui::SharedString::from(format!(
            "{id}-drag"
        ))))
        .children(trail)
}

impl Editor {
    pub fn render_home(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("home")
            .group("home")
            .relative()
            .size_full()
            .flex()
            .bg(theme::page_bg())
            .child(self.home_sidebar(window.is_fullscreen(), cx))
            .child(self.home_page(window, cx))
            .child(drop_overlay())
    }

    // ---- sidebar ------------------------------------------------------------

    fn home_sidebar(&self, full_screen: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.home_tab;
        div()
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme::hairline())
            // Clear of the traffic lights (none in full screen), or the app
            // menu where there's no menu bar.
            .child(if window_chrome::draws_menu() {
                title_strip(
                    "home-sidebar-titlebar",
                    Some(self.render_menu_button(cx).into_any_element()),
                    None,
                )
            } else {
                title_strip("home-sidebar-titlebar", None, None).when(full_screen, |d| d.h(px(6.)))
            })
            .child(
                div()
                    .px(px(10.))
                    .pt(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(sidebar_row(
                        "home-tab-edit",
                        if tab == HomeTab::Edit {
                            Icon::CutFilled
                        } else {
                            Icon::Cut
                        },
                        "Edit",
                        tab == HomeTab::Edit,
                        cx.listener(|ed, _, _, cx| ed.set_home_tab(HomeTab::Edit, cx)),
                    ))
                    .child(sidebar_row(
                        "home-tab-projects",
                        if tab == HomeTab::Projects {
                            Icon::Folder
                        } else {
                            Icon::FolderOutline
                        },
                        "Projects",
                        tab == HomeTab::Projects,
                        cx.listener(|ed, _, _, cx| ed.set_home_tab(HomeTab::Projects, cx)),
                    ))
                    // A dialog over the page, not a page of its own.
                    .child(sidebar_row(
                        "home-settings",
                        Icon::Settings,
                        "Settings",
                        false,
                        cx.listener(|ed, _, _, cx| ed.open_settings(false, cx)),
                    )),
            )
    }

    fn set_home_tab(&mut self, tab: HomeTab, cx: &mut Context<Self>) {
        if self.home_tab != tab {
            self.home_tab = tab;
            self.home_selection = None;
            self.home_scroll.set_offset(gpui::Point::default());
        }
        cx.notify();
    }

    // ---- page ---------------------------------------------------------------

    fn home_page(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_home = self.home_tab == HomeTab::Edit;
        let count = self.home_projects.len();
        let drafts = format!(
            "{count} DRAFT{} · LOCAL ONLY",
            if count == 1 { "" } else { "S" }
        );
        let title: SharedString = if is_home {
            self.home_greeting.into()
        } else {
            "Projects".into()
        };
        // How far the New video card has collapsed (0 open, 1 slim).
        let scrolled = (-f32::from(self.home_scroll.offset().y)).max(0.);
        let collapse = (scrolled / COLLAPSE_RANGE).clamp(0., 1.);

        let shown = if is_home {
            count.min(HOME_RECENT_LIMIT)
        } else {
            count
        };
        let rows: Vec<AnyElement> = self.home_projects[..shown]
            .iter()
            .enumerate()
            .map(|(i, p)| self.project_row(i, p, cx).into_any_element())
            .collect();

        let list = div()
            .id("home-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.home_scroll)
            // Re-render as it scrolls: the card's collapse follows the offset.
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .child(
                div()
                    .w_full()
                    .px(px(PAGE_PAD))
                    .pb(px(120.))
                    .when(is_home, |d| d.pt(px(CARD_HEIGHT)))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(self.list_header(is_home, count > shown, cx))
                    .when(rows.is_empty(), |d| {
                        d.child(
                            div()
                                .pt(px(80.))
                                .flex()
                                .justify_center()
                                .text_size(px(15.))
                                .text_color(theme::text_faint())
                                .child("No projects yet. Create your first edit."),
                        )
                    })
                    .children(rows),
            );

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(title_strip(
                "home-titlebar",
                None,
                window_chrome::draws_controls(window)
                    .then(|| self.render_window_controls(window, cx).into_any_element()),
            ))
            // Greeting (or the page title) + drafts count.
            .child(
                div()
                    .flex_none()
                    .px(px(PAGE_PAD))
                    .pt_1()
                    .pb_3()
                    .child(
                        div()
                            .text_size(px(22.))
                            .font_weight(FontWeight::BOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .mt(px(2.))
                            .child(mono(drafts, 9.5, theme::text_faint())),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(list)
                    // Opaque, so rows scroll under the card, not through it.
                    .when(is_home, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .right_0()
                                .px(px(PAGE_PAD))
                                .pb_2()
                                .bg(theme::page_bg())
                                .child(self.new_video_card(collapse, cx)),
                        )
                    }),
            )
    }

    /// "Recent" (or "N selected" with Cancel / Delete while selecting) and
    /// the Videos tab with its count, once there's a draft.
    fn list_header(&self, is_home: bool, has_more: bool, cx: &mut Context<Self>) -> AnyElement {
        let count = self.home_projects.len();
        if count == 0 {
            return div().into_any_element();
        }
        let selecting = self.home_selection.as_ref();
        let title: SharedString = match selecting {
            Some(s) => format!("{} selected", s.len()).into(),
            None => "Recent".into(),
        };
        let show_title = is_home || selecting.is_some();
        div()
            .pt(px(12.))
            .flex()
            .flex_col()
            .when(show_title, |d| {
                d.child(
                    div()
                        .mb(px(12.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_weight(FontWeight::BOLD)
                                .child(title),
                        )
                        .map(|d| match selecting {
                            Some(sel) => d.child(self.selection_actions(sel.len(), cx)),
                            None if has_more => d.child(
                                div()
                                    .id("home-see-all")
                                    .flex()
                                    .items_center()
                                    .gap(px(2.))
                                    .cursor_pointer()
                                    .text_color(theme::text_secondary())
                                    .hover(|s| s.text_color(theme::text()))
                                    .on_click(cx.listener(|ed, _, _, cx| {
                                        ed.set_home_tab(HomeTab::Projects, cx)
                                    }))
                                    .child("See all")
                                    .child(Icon::ChevronForward.el(14., theme::text_secondary())),
                            ),
                            None => d,
                        }),
                )
            })
            .child(
                div()
                    .mb(px(12.))
                    .flex()
                    .gap_2()
                    .child(recent_tab_pill("Videos", count, true)),
            )
            .into_any_element()
    }

    fn selection_actions(&self, picked: usize, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .id("home-cancel-selection")
                    .cursor_pointer()
                    .text_color(theme::text_secondary())
                    .hover(|s| s.text_color(theme::text()))
                    .on_click(cx.listener(|ed, _, _, cx| {
                        ed.home_selection = None;
                        cx.notify();
                    }))
                    .child("Cancel"),
            )
            .child(
                div()
                    .id("home-delete-selected")
                    .size(px(26.))
                    .rounded(px(9.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(picked > 0, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(rgba(0xff5c5c26)))
                            .on_click(cx.listener(|ed, _, _, cx| ed.confirm_delete_selected(cx)))
                    })
                    .child(Icon::Trash.el(
                        15.,
                        if picked > 0 {
                            theme::danger()
                        } else {
                            theme::text_disabled()
                        },
                    )),
            )
    }

    fn confirm_delete_selected(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.home_selection.iter().flatten().cloned().collect();
        if ids.is_empty() {
            return;
        }
        self.confirm_delete(ids, "This can't be undone.".into(), cx);
    }

    fn confirm_delete(&mut self, ids: Vec<String>, message: SharedString, cx: &mut Context<Self>) {
        let n = ids.len();
        self.confirm = Some(Confirm {
            title: format!("Delete {n} project{}?", if n == 1 { "" } else { "s" }).into(),
            message,
            confirm_label: "Delete".into(),
            run: Rc::new(move |ed: &mut Editor, _, cx| ed.delete_saved_projects(&ids, cx)),
        });
        cx.notify();
    }

    // ---- New video ----------------------------------------------------------

    /// The accent card: icon top-left and title bottom-left, sliding into a
    /// slim bar (icon centred, title beside it, subtitle faded) as `p` → 1.
    fn new_video_card(&self, p: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let icon = lerp(ICON, ICON_SMALL, p);
        let text_top = lerp(
            CARD_HEIGHT - CARD_PAD - TITLE_LINE - SUB_LINE,
            (COLLAPSED_HEIGHT - TITLE_LINE) / 2.,
            p,
        );
        let text_left = lerp(CARD_PAD, COLLAPSED_PAD + ICON_SMALL + TEXT_GAP, p);
        // Gone in the first half, before the title reaches the icon.
        let sub_opacity = (1. - p * 2.).clamp(0., 1.);
        div()
            .id("new-video")
            .relative()
            .overflow_hidden()
            .h(px(lerp(CARD_HEIGHT, COLLAPSED_HEIGHT, p)))
            .rounded(px(16.))
            .bg(theme::accent())
            .cursor_pointer()
            .hover(|s| s.opacity(0.94))
            .active(|s| s.opacity(0.85))
            .on_click(cx.listener(|ed, _, _, cx| ed.new_blank_project(cx)))
            .child(
                div()
                    .absolute()
                    .left(px(lerp(CARD_PAD, COLLAPSED_PAD, p)))
                    .top(px(lerp(CARD_PAD, (COLLAPSED_HEIGHT - ICON_SMALL) / 2., p)))
                    .size(px(icon))
                    .rounded(px(10.))
                    .bg(rgba(0x1a160824))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::Videocam.el(20., theme::accent_text())),
            )
            .child(
                div()
                    .absolute()
                    .left(px(text_left))
                    .right(px(CARD_PAD - 4.))
                    .top(px(text_top))
                    .child(
                        div()
                            .h(px(TITLE_LINE))
                            .flex()
                            .items_center()
                            .truncate()
                            .text_size(px(lerp(16., 14., p)))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::accent_text())
                            .child("New video"),
                    )
                    .child(
                        div()
                            .h(px(SUB_LINE))
                            .truncate()
                            .text_size(px(11.5))
                            .text_color(theme::accent_text())
                            .opacity(0.6 * sub_opacity)
                            .child("Clips, music & text"),
                    ),
            )
    }

    // ---- rows ---------------------------------------------------------------

    fn project_row(&self, i: usize, p: &Summary, cx: &mut Context<Self>) -> impl IntoElement {
        let thumb = p
            .preview
            .as_ref()
            .and_then(|(src, _)| self.thumbs.get(src).cloned());
        let picked = self.home_selection.as_ref().map(|s| s.contains(&p.id));
        let click_id = p.id.clone();
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
            .active(|s| s.opacity(0.6))
            .on_click(
                cx.listener(move |ed, _, _, cx| match ed.home_selection.as_mut() {
                    Some(sel) => {
                        if !sel.remove(&click_id) {
                            sel.insert(click_id.clone());
                        }
                        cx.notify();
                    }
                    None => ed.open_saved_project(&click_id, cx),
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                    let (open, select, delete, name) = (
                        menu_id.clone(),
                        menu_id.clone(),
                        menu_id.clone(),
                        name.clone(),
                    );
                    let items = vec![
                        PopupItem::new("Open", move |ed: &mut Editor, _, cx| {
                            ed.open_saved_project(&open, cx)
                        }),
                        PopupItem::new("Select", move |ed: &mut Editor, _, cx| {
                            ed.home_selection
                                .get_or_insert_default()
                                .insert(select.clone());
                            cx.notify();
                        }),
                        PopupItem::new("Delete…", move |ed: &mut Editor, _, cx| {
                            ed.confirm_delete(
                                vec![delete.clone()],
                                format!("“{name}”. This can't be undone.").into(),
                                cx,
                            )
                        })
                        .destructive(),
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
                    // Always light on dark: it sits on the picture.
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
                            .truncate()
                            .text_size(px(theme::SM))
                            .text_color(theme::text_secondary())
                            .child(projects::relative_time(&p.updated_at)),
                    ),
            )
            .child(match picked {
                Some(true) => Icon::CheckmarkCircle.el(24., theme::text()),
                Some(false) => Icon::EllipseOutline.el(24., theme::text_faint()),
                None => Icon::ChevronForward.el(18., theme::text_disabled()),
            })
    }
}

/// One sidebar entry: icon, the label beside it, the current page on a
/// soft fill.
fn sidebar_row(
    id: &'static str,
    icon: Icon,
    label: &'static str,
    on: bool,
    click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let color = if on {
        theme::text()
    } else {
        theme::text_secondary()
    };
    div()
        .id(id)
        .h(px(40.))
        .px(px(12.))
        .rounded(px(9.))
        .flex()
        .items_center()
        .gap(px(12.))
        .cursor_pointer()
        .text_size(px(theme::MD))
        .text_color(color)
        .when(on, |d| {
            d.bg(theme::fill_subtle()).font_weight(FontWeight::SEMIBOLD)
        })
        .when(!on, |d| {
            d.font_weight(FontWeight::MEDIUM)
                .hover(|s| s.bg(rgba(0xffffff0a)).text_color(theme::text()))
        })
        .on_click(click)
        .child(icon.el(20., color))
        .child(label)
}

/// A monochrome tab pill over the drafts: the selected one filled, its
/// count in a quieter weight.
fn recent_tab_pill(label: &'static str, count: usize, on: bool) -> impl IntoElement {
    div()
        .h(px(34.))
        .px(px(14.))
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(theme::SM))
        .bg(if on {
            theme::text()
        } else {
            theme::fill_subtle()
        })
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if on {
                    theme::page_bg()
                } else {
                    theme::text_secondary()
                })
                .child(label),
        )
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(if on {
                    rgba(0x1414168c)
                } else {
                    theme::text_faint()
                })
                .child(count.to_string()),
        )
}

/// "Drop to start", shown only while files are dragged over the window
/// (the window root takes the drop).
fn drop_overlay() -> impl IntoElement {
    div()
        .absolute()
        .inset(px(12.))
        .invisible()
        .group_drag_over::<ExternalPaths>("home", |s| s.visible())
        .rounded(px(16.))
        .border_2()
        .border_dashed()
        .border_color(theme::accent())
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .child(div().absolute().inset_0().bg(theme::page_bg()).opacity(0.9))
        .child(
            div()
                .relative()
                .text_size(px(theme::LG))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Drop videos or photos to start a new video"),
        )
}
