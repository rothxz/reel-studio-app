//! Settings → Data and Storage, as pages inside the Settings dialog (a back
//! arrow in its header): the hub with both totals, Storage Usage (a donut of
//! what's kept, the cache categories ticked over one Clear button, the
//! user's own data, then the auto-remove limits), Data Usage (what the
//! app's downloads cost, reset) and AI models (delete the downloaded ones).
//! Measuring walks the disk off the UI thread; the last totals stay shown
//! while a page refreshes them.

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    AnyElement, Context, FontWeight, MouseButton, Pixels, Point, Rgba, SharedString, canvas, div,
    point, prelude::*, px, relative, rgb,
};

use crate::data_usage::{self, Kind};
use crate::editor::{Confirm, Editor};
use crate::i18n::{t, tf};
use crate::storage::{
    self, CACHE, Category, KEPT, KeepCache, LIMITS, Report, format_bytes, format_share,
};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::settings::{group_with, row};
use crate::ui::widgets::pill_button;

/// A page of the Settings dialog past its first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsPage {
    DataStorage,
    StorageUsage,
    DataUsage,
    AiModels,
}

impl SettingsPage {
    pub fn title(self) -> &'static str {
        match self {
            SettingsPage::DataStorage => "Data and Storage",
            SettingsPage::StorageUsage => "Storage Usage",
            SettingsPage::DataUsage => "Data Usage",
            SettingsPage::AiModels => "AI models",
        }
    }
}

/// A downloaded model, as Settings → AI models lists it.
#[derive(Clone, Debug)]
struct ModelFile {
    /// The speech-to-text model's language (English label), or None for
    /// the natural voice's setup.
    language: Option<&'static str>,
    path: PathBuf,
    bytes: u64,
}

impl ModelFile {
    fn name(&self) -> String {
        match self.language {
            Some(l) => tf("Auto captions · {language}", &[("language", &t(l))]),
            None => t("Natural voice").to_string(),
        }
    }
}

fn list_models() -> Vec<ModelFile> {
    let mut list: Vec<ModelFile> = crate::speech::downloaded_models()
        .into_iter()
        .map(|(language, path)| ModelFile {
            language: Some(language),
            bytes: storage::dir_bytes(&path),
            path,
        })
        .collect();
    let voice = crate::agent::local_voice::setup_dir();
    let bytes = storage::dir_bytes(&voice);
    if bytes > 0 {
        list.push(ModelFile {
            language: None,
            path: voice,
            bytes,
        });
    }
    list
}

/// What the storage pages show (kept while Settings is closed, so a second
/// visit shows the last totals at once).
#[derive(Default)]
pub struct StorageView {
    report: Option<Report>,
    models: Option<Vec<ModelFile>>,
    measuring: bool,
    /// Cache categories left out of Clear (all are ticked at first).
    unticked: HashSet<Category>,
    clearing: bool,
}

// System-style hues that stay apart on the donut; the same hue is the same
// kind of thing on both pages.
const BLUE: u32 = 0x3478f6;
const SKY: u32 = 0x5ac8fa;
const PURPLE: u32 = 0xaf52de;
const ORANGE: u32 = 0xff9500;
const RED: u32 = 0xff3b30;
const PINK: u32 = 0xff2d55;
const GREY: u32 = 0x8e8e93;

fn category_style(c: Category) -> (Rgba, Icon) {
    match c {
        Category::Projects => (rgb(BLUE), Icon::FilmSolid),
        Category::Sounds => (rgb(PINK), Icon::VolumeHighSolid),
        Category::Music => (rgb(RED), Icon::MusicalNotesSolid),
        Category::AiModels => (rgb(PURPLE), Icon::SparklesSolid),
        Category::Effects => (rgb(ORANGE), Icon::ColorWand),
        Category::Previews => (rgb(SKY), Icon::ImagesSolid),
        Category::Other => (rgb(GREY), Icon::FolderOpenSolid),
    }
}

fn kind_style(k: Kind) -> (Rgba, Icon) {
    match k {
        Kind::Effects => (rgb(ORANGE), Icon::ColorWand),
        Kind::Stickers => (rgb(SKY), Icon::HappySolid),
        Kind::Music => (rgb(RED), Icon::MusicalNotesSolid),
        Kind::Sounds => (rgb(PINK), Icon::VolumeHighSolid),
        Kind::AiModels => (rgb(PURPLE), Icon::SparklesSolid),
    }
}

impl Editor {
    /// Opens a page of Settings over the current one.
    pub fn settings_push(&mut self, page: SettingsPage, cx: &mut Context<Self>) {
        self.settings_pages.push(page);
        self.measure_storage(cx);
        cx.notify();
    }

    /// Back one page (Esc, or the header's arrow).
    pub fn settings_back(&mut self, cx: &mut Context<Self>) {
        self.settings_pages.pop();
        // Back on a page with totals: they may have changed (a delete).
        if !self.settings_pages.is_empty() {
            self.measure_storage(cx);
        }
        cx.notify();
    }

    /// Measures every category (and the models) off the UI thread.
    fn measure_storage(&mut self, cx: &mut Context<Self>) {
        if self.storage.measuring {
            return;
        }
        self.storage.measuring = true;
        let task = cx
            .background_executor()
            .spawn(async move { (storage::measure(), list_models()) });
        cx.spawn(async move |this, cx| {
            let (report, models) = task.await;
            this.update(cx, |ed, cx| {
                ed.storage.report = Some(report);
                ed.storage.models = Some(models);
                ed.storage.measuring = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// An export or an AI job may be reading cached files or models.
    fn storage_busy(&self) -> bool {
        self.export.is_some() || self.caption_job.is_some() || self.agent.busy
    }

    /// The body of a storage page.
    pub(crate) fn render_storage_page(
        &mut self,
        page: SettingsPage,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        match page {
            SettingsPage::DataStorage => self.data_storage_page(cx),
            SettingsPage::StorageUsage => self.storage_usage_page(cx),
            SettingsPage::DataUsage => self.data_usage_page(cx),
            SettingsPage::AiModels => self.ai_models_page(cx),
        }
    }

    // ---- the hub ------------------------------------------------------------

    fn data_storage_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let report = self.storage.report.as_ref();
        let storage_total = report.map(|r| format_bytes(r.total()));
        let models = report.map(|r| format_bytes(r.get(Category::AiModels)));
        let data_total = format_bytes(data_usage::snapshot().total());
        vec![
            group_with(
                Some("USAGE"),
                vec![
                    nav_row(
                        "storage-usage",
                        Icon::PieChart,
                        "Storage Usage",
                        storage_total,
                        SettingsPage::StorageUsage,
                        cx,
                    ),
                    nav_row(
                        "data-usage",
                        Icon::StatsChart,
                        "Data Usage",
                        Some(data_total),
                        SettingsPage::DataUsage,
                        cx,
                    ),
                ],
                None,
            )
            .into_any_element(),
            group_with(
                Some("DOWNLOADS"),
                vec![nav_row(
                    "ai-models",
                    Icon::Sparkles,
                    "AI models",
                    models,
                    SettingsPage::AiModels,
                    cx,
                )],
                Some(
                    "On-device AI models are the biggest downloads. Delete the ones you don't use.",
                ),
            )
            .into_any_element(),
        ]
    }

    // ---- Storage Usage ------------------------------------------------------

    fn storage_usage_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = vec![];
        let Some(report) = self.storage.report.clone() else {
            out.push(
                div()
                    .h(px(220.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme::text_tertiary())
                    .child("…")
                    .into_any_element(),
            );
            out.extend(self.cache_limit_groups(cx));
            return out;
        };
        let total = report.total();
        let unticked = self.storage.unticked.clone();
        let shown: Vec<Category> = CACHE.into_iter().filter(|&c| report.get(c) > 0).collect();
        let picked: Vec<Category> = shown
            .iter()
            .copied()
            .filter(|c| !unticked.contains(c))
            .collect();
        let picked_bytes: u64 = picked.iter().map(|&c| report.get(c)).sum();

        let entries: Vec<(u64, Rgba, bool)> = KEPT
            .iter()
            .chain(CACHE.iter())
            .map(|&c| (report.get(c), category_style(c).0, unticked.contains(&c)))
            .collect();
        let mut head = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.))
            .mb(px(18.))
            .child(donut(entries, format_bytes(total)));
        if report.free > 0 {
            let all = total + report.free;
            head = head
                .child(
                    div()
                        .mt(px(8.))
                        .text_size(px(theme::SM))
                        .text_color(theme::text_secondary())
                        .child(tf(
                            "Reel Studio uses {percent} of your free space.",
                            &[("percent", &format_share(total, all))],
                        )),
                )
                .child(
                    div()
                        .w(px(120.))
                        .h(px(4.))
                        .rounded(px(2.))
                        .bg(theme::fill_muted())
                        .overflow_hidden()
                        .child(
                            div()
                                .h_full()
                                .w(relative((total as f32 / all as f32).max(0.02)))
                                .bg(rgb(BLUE)),
                        ),
                );
        }
        out.push(head.into_any_element());

        // The cache: tick what to clear.
        let cache_rows = if shown.is_empty() {
            vec![
                div()
                    .min_h(px(48.))
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .text_size(px(theme::MD))
                    .child(t("The cache is empty"))
                    .into_any_element(),
            ]
        } else {
            let mut rows: Vec<AnyElement> = shown
                .iter()
                .map(|&c| {
                    let (color, icon) = category_style(c);
                    usage_row(
                        format!("cache-{c:?}"),
                        c.label(),
                        format_share(report.get(c), total),
                        format_bytes(report.get(c)),
                        color,
                        icon,
                        Some(!unticked.contains(&c)),
                        false,
                    )
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::fill_subtle()))
                    .on_click(cx.listener(move |ed, _, _, cx| {
                        if !ed.storage.unticked.remove(&c) {
                            ed.storage.unticked.insert(c);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
                })
                .collect();
            let size = format_bytes(picked_bytes);
            let disabled = picked_bytes == 0 || self.storage.clearing;
            let label = if self.storage.clearing {
                "…".to_string()
            } else {
                tf("Clear Cache {size}", &[("size", &size)])
            };
            rows.push(
                div()
                    .px(px(16.))
                    .py(px(12.))
                    .child(
                        pill_button("storage-clear", label, true)
                            .w_full()
                            .h(px(36.))
                            .rounded(px(8.))
                            .text_size(px(theme::MD))
                            .when(disabled, |d| d.opacity(0.4).cursor_default())
                            .on_click(cx.listener(move |ed, _, _, cx| {
                                if !disabled {
                                    ed.ask_clear_cache(picked.clone(), picked_bytes, cx);
                                }
                            })),
                    )
                    .into_any_element(),
            );
            rows
        };
        out.push(
            group_with(
                Some("CACHE"),
                cache_rows,
                Some(
                    "Cached files come back on their own: effects, fonts and music download again, \
                     and thumbnails are made again when you need them.",
                ),
            )
            .into_any_element(),
        );

        // The user's own data.
        let kept: Vec<AnyElement> =
            KEPT.iter()
                .map(|&c| {
                    let (color, icon) = category_style(c);
                    let models = c == Category::AiModels;
                    let el = usage_row(
                        format!("kept-{c:?}"),
                        c.label(),
                        format_share(report.get(c), total),
                        format_bytes(report.get(c)),
                        color,
                        icon,
                        None,
                        models,
                    );
                    if models {
                        el.cursor_pointer()
                            .hover(|s| s.bg(theme::fill_subtle()))
                            .on_click(cx.listener(|ed, _, _, cx| {
                                ed.settings_push(SettingsPage::AiModels, cx)
                            }))
                            .into_any_element()
                    } else {
                        el.into_any_element()
                    }
                })
                .collect();
        out.push(
            group_with(
                Some("ON THIS DEVICE"),
                kept,
                Some(
                    "Your own work and downloads. Delete projects, scans and sounds where you made them.",
                ),
            )
            .into_any_element(),
        );
        out.extend(self.cache_limit_groups(cx));
        out
    }

    /// Auto-remove cache and Maximum cache size (applied at launch).
    fn cache_limit_groups(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let keep = self.prefs.keep_cache;
        let keep_rows: Vec<AnyElement> = KeepCache::ALL
            .into_iter()
            .map(|k| {
                div()
                    .id(SharedString::from(format!("keep-{k:?}")))
                    .min_h(px(44.))
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::fill_subtle()))
                    .child(div().flex_1().text_size(px(theme::MD)).child(t(k.label())))
                    .when(k == keep, |d| {
                        d.child(Icon::Checkmark.el(16., theme::text()))
                    })
                    .on_click(cx.listener(move |ed, _, _, cx| {
                        ed.prefs.keep_cache = k;
                        ed.prefs.save();
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();

        let at = self.prefs.cache_limit.min(LIMITS.len() - 1);
        let set = move |i: usize| {
            move |ed: &mut Editor,
                  _: &gpui::ClickEvent,
                  _: &mut gpui::Window,
                  cx: &mut Context<Editor>| {
                ed.prefs.cache_limit = i;
                ed.prefs.save();
                cx.notify();
            }
        };
        let last = (LIMITS.len() - 1) as f32;
        let labels = div()
            .flex()
            .justify_between()
            .children(LIMITS.iter().enumerate().map(|(i, stop)| {
                let label: SharedString = match stop {
                    Some(b) => format_bytes(*b).into(),
                    None => t("No limit").into(),
                };
                div()
                    .id(SharedString::from(format!("limit-label-{i}")))
                    .cursor_pointer()
                    .text_size(px(theme::SM))
                    .map(|d| {
                        if i == at {
                            d.font_weight(FontWeight::BOLD).text_color(theme::text())
                        } else {
                            d.text_color(theme::text_tertiary())
                        }
                    })
                    .child(label)
                    .on_click(cx.listener(set(i)))
            }));
        // The stepped track: nearest-stop click zones over a line, a dot per
        // stop and the knob on the chosen one.
        let fraction = at as f32 / last;
        let zones = div()
            .absolute()
            .inset_0()
            .flex()
            .children((0..LIMITS.len()).map(|i| {
                let w = if i == 0 || i == LIMITS.len() - 1 {
                    0.5 / last
                } else {
                    1. / last
                };
                div()
                    .id(SharedString::from(format!("limit-zone-{i}")))
                    .h_full()
                    .w(relative(w))
                    .cursor_pointer()
                    .on_click(cx.listener(set(i)))
            }));
        let track = div()
            .relative()
            .h(px(24.))
            .mx(px(8.))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(10.))
                    .h(px(4.))
                    .rounded(px(2.))
                    .bg(theme::fill_muted())
                    .child(
                        div()
                            .h_full()
                            .w(relative(fraction))
                            .rounded(px(2.))
                            .bg(theme::text()),
                    ),
            )
            .children((0..LIMITS.len()).map(|i| {
                let on = i == at;
                let size = if on { 18. } else { 8. };
                div()
                    .absolute()
                    .left(relative(i as f32 / last))
                    .top(px(12. - size / 2.))
                    .ml(px(-size / 2.))
                    .size(px(size))
                    .rounded_full()
                    .bg(if on || i < at {
                        theme::text()
                    } else {
                        theme::fill_strong()
                    })
                    .when(on, |d| d.shadow_md())
            }))
            .child(zones);
        let limit = div()
            .px(px(16.))
            .pt(px(12.))
            .pb(px(8.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(labels)
            .child(track)
            .into_any_element();

        vec![
            group_with(
                Some("AUTO-REMOVE CACHE"),
                keep_rows,
                Some("Cached files older than this are removed from this device when the app opens."),
            )
            .into_any_element(),
            group_with(
                Some("MAXIMUM CACHE SIZE"),
                vec![limit],
                Some(
                    "If the cache grows past this, the oldest files are removed when the app opens. \
                     Image previews keep their own limit.",
                ),
            )
            .into_any_element(),
        ]
    }

    fn ask_clear_cache(&mut self, categories: Vec<Category>, bytes: u64, cx: &mut Context<Self>) {
        if self.storage_busy() {
            self.toast(t("Wait for the export or AI job to finish first"), cx);
            return;
        }
        let size = format_bytes(bytes);
        self.confirm = Some(Confirm {
            title: tf("Clear {size} of cache?", &[("size", &size)]).into(),
            message: t(
                "Your projects aren't touched. Effects and fonts download again the next time you use them.",
            )
            .into(),
            confirm_label: t("Clear").into(),
            run: Rc::new(move |ed: &mut Editor, _, cx| {
                ed.clear_cache(categories.clone(), size.clone(), cx)
            }),
        });
        cx.notify();
    }

    fn clear_cache(&mut self, categories: Vec<Category>, size: String, cx: &mut Context<Self>) {
        if self.storage_busy() {
            self.toast(t("Wait for the export or AI job to finish first"), cx);
            return;
        }
        self.storage.clearing = true;
        let task = cx
            .background_executor()
            .spawn(async move { storage::clear(&categories) });
        cx.spawn(async move |this, cx| {
            task.await;
            this.update(cx, |ed, cx| {
                ed.storage.clearing = false;
                ed.toast(tf("Cleared {size}", &[("size", &size)]), cx);
                ed.measure_storage(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    // ---- Data Usage -----------------------------------------------------------

    fn data_usage_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let usage = data_usage::snapshot();
        let total = usage.total();
        let entries = Kind::ALL
            .iter()
            .map(|&k| (usage.get(k), kind_style(k).0, false))
            .collect();
        let rows: Vec<AnyElement> = Kind::ALL
            .iter()
            .map(|&k| {
                let (color, icon) = kind_style(k);
                usage_row(
                    format!("data-{k:?}"),
                    k.label(),
                    format_share(usage.get(k), total),
                    format_bytes(usage.get(k)),
                    color,
                    icon,
                    None,
                    false,
                )
                .into_any_element()
            })
            .collect();
        vec![
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .mb(px(18.))
                .child(donut(entries, format_bytes(total)))
                .child(
                    div()
                        .mt(px(8.))
                        .text_size(px(theme::SM))
                        .text_color(theme::text_secondary())
                        .child(tf(
                            "Your data usage since {date}",
                            &[("date", &data_usage::format_since(usage.since))],
                        )),
                )
                .into_any_element(),
            group_with(
                None,
                rows,
                Some(
                    "Counts what Reel Studio downloads for its features. Previews that stream while \
                     you browse aren't included.",
                ),
            )
            .into_any_element(),
            group_with(
                Some("TOTAL NETWORK USAGE"),
                vec![
                    row(Icon::ArrowDownCircle, "Bytes received", None)
                        .child(
                            div()
                                .text_size(px(theme::MD))
                                .text_color(theme::text_secondary())
                                .child(format_bytes(total)),
                        )
                        .into_any_element(),
                ],
                None,
            )
            .into_any_element(),
            group_with(
                None,
                vec![
                    row(Icon::Refresh, "Reset statistics", None)
                        .id("data-reset")
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::fill_subtle()))
                        .on_click(cx.listener(|ed, _, _, cx| {
                            ed.confirm = Some(Confirm {
                                title: t("Reset statistics?").into(),
                                message: t("The counts start again from zero.").into(),
                                confirm_label: t("Reset statistics").into(),
                                run: Rc::new(|ed: &mut Editor, _, cx| {
                                    data_usage::reset();
                                    ed.toast(t("Statistics reset"), cx);
                                }),
                            });
                            cx.notify();
                        }))
                        .into_any_element(),
                ],
                None,
            )
            .into_any_element(),
        ]
    }

    // ---- AI models --------------------------------------------------------------

    fn ai_models_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(models) = self.storage.models.clone() else {
            return vec![];
        };
        if models.is_empty() {
            return vec![
                div()
                    .py(px(40.))
                    .px(px(24.))
                    .text_center()
                    .text_size(px(theme::MD))
                    .text_color(theme::text_secondary())
                    .child(t(
                        "No models downloaded yet. Auto captions and the natural voice download one \
                         the first time you use them.",
                    ))
                    .into_any_element(),
            ];
        }
        let mut rows: Vec<AnyElement> = models
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let name = m.name();
                let one = m.clone();
                div()
                    .min_h(px(52.))
                    .px(px(16.))
                    .py(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(theme::MD))
                                    .truncate()
                                    .child(name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::SM))
                                    .text_color(theme::text_secondary())
                                    .child(format_bytes(m.bytes)),
                            ),
                    )
                    .child(
                        delete_button(format!("model-delete-{i}")).on_click(cx.listener(
                            move |ed, _, _, cx| {
                                ed.ask_delete_models(Some(name.clone()), vec![one.clone()], cx)
                            },
                        )),
                    )
                    .into_any_element()
            })
            .collect();
        if models.len() > 1 {
            rows.push(
                div()
                    .min_h(px(48.))
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(theme::MD))
                            .child(t("Delete all")),
                    )
                    .child(
                        delete_button("model-delete-all".into()).on_click(cx.listener(
                            move |ed, _, _, cx| ed.ask_delete_models(None, models.clone(), cx),
                        )),
                    )
                    .into_any_element(),
            );
        }
        vec![
            group_with(
                Some("DOWNLOADED"),
                rows,
                Some("Models stay on this computer and only run when you start a job."),
            )
            .into_any_element(),
        ]
    }

    fn ask_delete_models(
        &mut self,
        name: Option<String>,
        models: Vec<ModelFile>,
        cx: &mut Context<Self>,
    ) {
        if self.storage_busy() {
            self.toast(t("Can't delete while an AI task is running"), cx);
            return;
        }
        let all = name.is_none();
        self.confirm = Some(Confirm {
            title: match &name {
                Some(n) => tf("Delete {name}?", &[("name", n)]).into(),
                None => t("Delete all models?").into(),
            },
            message: t(
                "This frees up space on this computer. It downloads again the next time you use \
                 the feature.",
            )
            .into(),
            confirm_label: t("Delete").into(),
            run: Rc::new(move |ed: &mut Editor, _, cx| {
                if ed.storage_busy() {
                    ed.toast(t("Can't delete while an AI task is running"), cx);
                    return;
                }
                let models = models.clone();
                let task = cx.background_executor().spawn(async move {
                    for m in &models {
                        if m.language.is_none() {
                            crate::agent::local_voice::stop();
                        }
                        storage::remove(&m.path);
                    }
                });
                cx.spawn(async move |this, cx| {
                    task.await;
                    this.update(cx, |ed, cx| {
                        ed.toast(
                            t(if all {
                                "All models deleted"
                            } else {
                                "Model deleted"
                            }),
                            cx,
                        );
                        ed.measure_storage(cx);
                    })
                    .ok();
                })
                .detach();
            }),
        });
        cx.notify();
    }
}

/// A row that opens another page: icon, label, its total (… while
/// measuring) and a chevron.
fn nav_row(
    id: &'static str,
    icon: Icon,
    label: &'static str,
    value: Option<String>,
    page: SettingsPage,
    cx: &mut Context<Editor>,
) -> AnyElement {
    row(icon, label, None)
        .id(id)
        .cursor_pointer()
        .hover(|s| s.bg(theme::fill_subtle()))
        .child(
            div()
                .text_size(px(theme::MD))
                .text_color(if value.is_some() {
                    theme::text_secondary()
                } else {
                    theme::text_tertiary()
                })
                .child(value.unwrap_or_else(|| "…".into())),
        )
        .child(Icon::ChevronForward.el(14., theme::text_tertiary()))
        .on_click(cx.listener(move |ed, _, _, cx| ed.settings_push(page, cx)))
        .into_any_element()
}

/// One legend row under the donut: the category's colour (a tick circle
/// when it can be cleared, else an icon tile), name + share, size.
#[allow(clippy::too_many_arguments)]
fn usage_row(
    id: String,
    label: &'static str,
    share: String,
    value: String,
    color: Rgba,
    icon: Icon,
    ticked: Option<bool>,
    chevron: bool,
) -> gpui::Stateful<gpui::Div> {
    let white = gpui::white();
    let mark = match ticked {
        Some(true) => div()
            .size(px(22.))
            .flex_none()
            .rounded_full()
            .bg(color)
            .flex()
            .items_center()
            .justify_center()
            .child(Icon::Checkmark.el(14., white)),
        Some(false) => div()
            .size(px(22.))
            .flex_none()
            .rounded_full()
            .border_2()
            .border_color(color),
        None => div()
            .size(px(28.))
            .flex_none()
            .rounded(px(7.))
            .bg(color)
            .flex()
            .items_center()
            .justify_center()
            .child(icon.el(16., white)),
    };
    div()
        .id(SharedString::from(id))
        .min_h(px(48.))
        .px(px(16.))
        .py(px(8.))
        .flex()
        .items_center()
        .gap(px(12.))
        .child(mark)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .gap(px(6.))
                .text_size(px(theme::MD))
                .child(div().truncate().child(t(label)))
                .child(div().text_color(theme::text_secondary()).child(share)),
        )
        .child(
            div()
                .text_size(px(theme::MD))
                .text_color(theme::text_secondary())
                .child(value),
        )
        .when(chevron, |d| {
            d.child(Icon::ChevronForward.el(14., theme::text_tertiary()))
        })
}

fn delete_button(id: String) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id))
        .px(px(10.))
        .py(px(4.))
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(theme::SM))
        .text_color(theme::danger())
        .hover(|s| s.bg(theme::fill_subtle()))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(t("Delete"))
}

const DONUT: f32 = 200.;
/// Ring thickness as a share of the diameter.
const RING: f32 = 0.27;
/// The dark seam between neighbouring slices.
const GAP_DEGREES: f32 = 1.4;
/// A slice this wide carries its share.
const LABEL_MIN_SWEEP: f32 = 26.;

/// The donut: one ring of slices (value, colour, faded), a seam between
/// each, the total in the hole and each wide slice's share on it.
fn donut(entries: Vec<(u64, Rgba, bool)>, center: String) -> impl IntoElement {
    let values: Vec<u64> = entries.iter().map(|e| e.0).collect();
    let total: u64 = values.iter().sum();
    let slices = storage::pie_slices(&values, 2.5);
    let ring = DONUT * RING;
    let radius = DONUT / 2. - ring / 2.;
    let paint = {
        let slices = slices.clone();
        let entries = entries.clone();
        move |bounds: gpui::Bounds<Pixels>, _: (), window: &mut gpui::Window, _: &mut gpui::App| {
            let c = (
                f32::from(bounds.origin.x) + DONUT / 2.,
                f32::from(bounds.origin.y) + DONUT / 2.,
            );
            for (slice, (_, color, faded)) in slices.iter().zip(&entries) {
                let gap = if slice.sweep > GAP_DEGREES * 3. {
                    GAP_DEGREES
                } else {
                    0.
                };
                let sweep = (slice.sweep - gap).min(359.99);
                if sweep <= 0.01 {
                    continue;
                }
                let color = if *faded {
                    Rgba { a: 0.25, ..*color }
                } else {
                    *color
                };
                if let Some(p) = arc(c, radius, ring, slice.start + gap / 2., sweep) {
                    window.paint_path(p, color);
                }
            }
        }
    };
    let labels = slices
        .iter()
        .zip(&entries)
        .filter(|(s, e)| s.sweep >= LABEL_MIN_SWEEP && !e.2)
        .map(|(s, e)| {
            let a = (s.start + s.sweep / 2. - 90.).to_radians();
            let (x, y) = (DONUT / 2. + radius * a.cos(), DONUT / 2. + radius * a.sin());
            div()
                .absolute()
                .left(px(x - 28.))
                .top(px(y - 9.))
                .w(px(56.))
                .text_center()
                .text_size(px(theme::SM))
                .font_weight(FontWeight::BOLD)
                .text_color(gpui::white())
                .child(format_share(e.0, total))
        })
        .collect::<Vec<_>>();
    div()
        .relative()
        .flex_none()
        .size(px(DONUT))
        // The empty ring under the slices (a disc: a stroked circle shows a
        // seam where its ends meet), then the hole over them.
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded_full()
                .bg(theme::fill_subtle()),
        )
        .child(canvas(|_, _, _| {}, paint).absolute().inset_0())
        .child(
            div()
                .absolute()
                .inset(px(ring))
                .rounded_full()
                .bg(theme::app_bg()),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(22.))
                .font_weight(FontWeight::BOLD)
                .child(center),
        )
        .children(labels)
}

/// An arc of a ring as a stroke, `start` and `sweep` in degrees clockwise
/// from 12 o'clock.
fn arc(c: (f32, f32), r: f32, width: f32, start: f32, sweep: f32) -> Option<gpui::Path<Pixels>> {
    let steps = ((sweep / 3.).ceil() as usize).max(2);
    let mut b = gpui::PathBuilder::stroke(px(width));
    for i in 0..=steps {
        let a = (start + sweep * i as f32 / steps as f32 - 90.).to_radians();
        let p: Point<Pixels> = point(px(c.0 + r * a.cos()), px(c.1 + r * a.sin()));
        if i == 0 {
            b.move_to(p);
        } else {
            b.line_to(p);
        }
    }
    b.build().ok()
}
