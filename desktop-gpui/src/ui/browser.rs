//! The left browser: a tab row (icon over label) for Media, Audio, Text,
//! Sticker, Effects, Transitions, Captions and Filter; a sub-nav down the
//! left when a tab has several views; the view itself on the right.
//! Media is the library: Import (or files dropped on the window) puts files
//! here, not on the timeline; a click on a tile adds it at the playhead.

use gpui::{Context, FontWeight, ObjectFit, div, img, prelude::*, px, rgba};

use crate::editor::Editor;
use crate::model::{MediaItem, Micros, SECOND};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets::{label_text, mono};

pub struct BrowserTab {
    pub label: &'static str,
    pub icon: Icon,
    pub views: &'static [&'static str],
}

pub const BROWSER_TABS: &[BrowserTab] = &[
    BrowserTab {
        label: "Media",
        icon: Icon::Film,
        views: &["Media"],
    },
    BrowserTab {
        label: "Audio",
        icon: Icon::MusicalNotes,
        views: &["Music", "Sound", "Voiceover"],
    },
    BrowserTab {
        label: "Text",
        icon: Icon::Text,
        views: &["Add text", "Text template"],
    },
    BrowserTab {
        label: "Sticker",
        icon: Icon::Happy,
        views: &["Stickers", "GIFs"],
    },
    BrowserTab {
        label: "Effects",
        icon: Icon::Sparkle4,
        views: &["Video effects", "Body effects"],
    },
    BrowserTab {
        label: "Transitions",
        icon: Icon::Transition,
        views: &["Transitions"],
    },
    BrowserTab {
        label: "Captions",
        icon: Icon::ClosedCaption,
        views: &["Auto captions", "Style"],
    },
    BrowserTab {
        label: "Filter",
        icon: Icon::ColorFilter,
        views: &["Filter"],
    },
];

const TAB_MIN_WIDTH: f32 = 54.;
const SUBNAV_WIDTH: f32 = 104.;
const TILE_GAP: f32 = 10.;
const TILE_MIN_WIDTH: f32 = 104.;

/// "MM:SS" for a tile's length.
fn short_duration(t: Micros) -> String {
    let s = (t.max(0) + SECOND / 2) / SECOND;
    format!("{:02}:{:02}", s / 60, s % 60)
}

impl Editor {
    pub fn render_browser(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab_ix = self.browser_tab.min(BROWSER_TABS.len() - 1);
        let tab = &BROWSER_TABS[tab_ix];
        let view_ix = self.browser_views[tab_ix].min(tab.views.len() - 1);

        let tabs = div()
            .id("browser-tabs")
            .flex_none()
            .flex()
            .px_1()
            .overflow_x_scroll()
            .border_b_1()
            .border_color(theme::card_border())
            .children(BROWSER_TABS.iter().enumerate().map(|(i, t)| {
                let on = i == tab_ix;
                div()
                    .id(("browser-tab", i))
                    .flex_grow()
                    .min_w(px(TAB_MIN_WIDTH))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(2.))
                    .px(px(6.))
                    .pt(px(6.))
                    .pb(px(5.))
                    .cursor_pointer()
                    .group("tab")
                    .text_color(if on {
                        theme::text()
                    } else {
                        theme::text_tertiary()
                    })
                    .when(!on, |d| d.hover(|s| s.text_color(theme::text_secondary())))
                    .on_click(cx.listener(move |ed, _, _, cx| {
                        ed.browser_tab = i;
                        cx.notify();
                    }))
                    .child(t.icon.el(
                        17.,
                        if on {
                            theme::text()
                        } else {
                            theme::text_tertiary()
                        },
                    ))
                    .child(
                        div()
                            .text_size(px(theme::XS))
                            .when(on, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .whitespace_nowrap()
                            .child(t.label),
                    )
            }));

        let subnav = (tab.views.len() > 1).then(|| {
            div()
                .flex_none()
                .w(px(SUBNAV_WIDTH))
                .p_2()
                .flex()
                .flex_col()
                .gap(px(2.))
                .border_r_1()
                .border_color(theme::card_border())
                .children(tab.views.iter().enumerate().map(|(i, label)| {
                    let on = i == view_ix;
                    div()
                        .id(("browser-view", i))
                        .h(px(26.))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded(px(5.))
                        .cursor_pointer()
                        .text_size(px(theme::XS))
                        .when(on, |d| d.bg(theme::button_bg()).text_color(theme::text()))
                        .when(!on, |d| {
                            d.text_color(theme::text_secondary())
                                .hover(|s| s.bg(theme::fill_subtle()))
                        })
                        .on_click(cx.listener(move |ed, _, _, cx| {
                            ed.browser_views[tab_ix] = i;
                            cx.notify();
                        }))
                        .child(*label)
                }))
        });

        let body = if tab_ix == 0 {
            self.render_media_library(cx).into_any_element()
        } else {
            not_yet(tab.views[view_ix]).into_any_element()
        };

        div().size_full().flex().flex_col().child(tabs).child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .children(subnav)
                .child(div().flex_1().min_w_0().flex().flex_col().child(body)),
        )
    }

    fn render_media_library(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let used: std::collections::HashSet<&str> = self
            .project
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter_map(|c| c.source.as_deref())
            .collect();
        let count = self.media.len();

        let width = self
            .measured("media-grid")
            .map_or(0., |b| f32::from(b.size.width) - 24.);
        let columns = ((width + TILE_GAP) / (TILE_MIN_WIDTH + TILE_GAP))
            .floor()
            .max(1.);
        let tile_w = if width > 0. {
            (width - TILE_GAP * (columns - 1.)) / columns
        } else {
            TILE_MIN_WIDTH
        };
        let tile_w = tile_w.floor();

        let header = div()
            .flex_none()
            .h(px(40.))
            .px(px(12.))
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("import")
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(px(30.))
                    .px(px(12.))
                    .rounded(px(6.))
                    .bg(theme::text())
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.88))
                    .on_click(
                        cx.listener(|ed, _, w, cx| ed.import(&crate::editor::ImportMedia, w, cx)),
                    )
                    .child(Icon::Add.el(16., theme::app_bg()))
                    .child(
                        div()
                            .text_size(px(theme::XS))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::app_bg())
                            .child("Import"),
                    ),
            )
            .child(div().flex_1())
            .child(label_text(
                format!("{count} item{}", if count == 1 { "" } else { "s" }),
                theme::text_tertiary(),
            ));

        let content = if self.media.is_empty() {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .px(px(16.))
                .child(Icon::CloudUpload.el(34., theme::text_tertiary()))
                .child(
                    div()
                        .text_size(px(theme::SM))
                        .text_color(theme::text_secondary())
                        .text_center()
                        .child(
                            "Import or drop videos and photos — drag one onto the timeline, or click it to add at the playhead",
                        ),
                )
                .into_any_element()
        } else {
            div()
                .id("media-grid")
                .relative()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(self.measure("media-grid"))
                .child(
                    div()
                        .p(px(12.))
                        .flex()
                        .flex_wrap()
                        .gap(px(TILE_GAP))
                        .children(self.media.iter().enumerate().map(|(i, item)| {
                            self.media_tile(i, item, tile_w, used.contains(item.path.as_str()), cx)
                        })),
                )
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(content)
    }

    fn media_tile(
        &self,
        i: usize,
        item: &MediaItem,
        width: f32,
        added: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let height = (width * 0.62).round();
        let thumb = self.thumbs.get(&item.path).cloned();
        let for_click = item.clone();
        let badge = |s: String| {
            div()
                .absolute()
                .top(px(4.))
                .px(px(5.))
                .py(px(1.))
                .rounded(px(4.))
                .bg(rgba(0x00000099))
                .text_size(px(theme::XXS))
                .text_color(theme::text())
                .child(s)
        };
        div()
            .id(("media", i))
            .w(px(width))
            .flex()
            .flex_col()
            .gap(px(5.))
            .cursor_pointer()
            .group("tile")
            .on_click(cx.listener(move |ed, _, _, cx| ed.add_at_playhead(&for_click, cx)))
            .child(
                div()
                    .relative()
                    .h(px(height))
                    .rounded(px(6.))
                    .overflow_hidden()
                    .bg(theme::button_bg())
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .group_hover("tile", |s| s.border_color(theme::text()))
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
                        None if !item.has_video => {
                            d.child(Icon::MusicalNote.el(22., theme::text_tertiary()))
                        }
                        None => d,
                    })
                    .when(added, |d| d.child(badge("Added".into()).left(px(4.))))
                    .when(
                        item.duration > 0 && item.has_video || !item.has_video,
                        |d| {
                            d.child(badge(String::new()).right(px(4.)).child(mono(
                                short_duration(item.duration),
                                theme::XXS,
                                theme::text(),
                            )))
                        },
                    )
                    .child(
                        div()
                            .absolute()
                            .right(px(5.))
                            .bottom(px(5.))
                            .size(px(22.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(theme::text())
                            .invisible()
                            .group_hover("tile", |s| s.visible())
                            .child(Icon::Add.el(16., theme::app_bg())),
                    ),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(theme::XS))
                    .text_color(theme::text_secondary())
                    .child(item.name.clone()),
            )
    }
}

/// A browser view that isn't ported yet.
fn not_yet(view: &str) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .px(px(16.))
        .child(
            div()
                .text_size(px(theme::SM))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text_secondary())
                .child(view.to_string()),
        )
        .child(
            div()
                .text_size(px(theme::XS))
                .text_color(theme::text_tertiary())
                .text_center()
                .child("Not in the native editor yet"),
        )
}
