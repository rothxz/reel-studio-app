//! Settings (app menu → Settings…, ⌘, or the home screen's gear): the old
//! desktop Settings dialog. So far its Updates group (check, download,
//! restart), the usage-data switch (Privacy) and the version; language, appearance and the editing
//! defaults are still to come (docs/PARITY.md). Found and downloaded
//! versions also show in the update dialog (ui/update_dialog.rs).

use gpui::{
    AnyElement, Context, FontWeight, MouseButton, Pixels, SharedString, Size, div, prelude::*, px,
};

use crate::editor::Editor;
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets::{pill_button, switch};
use crate::updater::{self, Status};

const MAX_WIDTH: f32 = 600.;
const MAX_HEIGHT: f32 = 720.;

impl Editor {
    /// Opens Settings (and, when asked, checks for updates).
    pub fn open_settings(&mut self, check: bool, cx: &mut Context<Self>) {
        self.settings_open = true;
        self.popup = None;
        if check {
            self.updater.update(cx, |u, cx| u.check(cx));
        }
        cx.notify();
    }

    pub(crate) fn render_settings(
        &mut self,
        viewport: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let close = |ed: &mut Editor,
                     _: &gpui::MouseDownEvent,
                     _: &mut gpui::Window,
                     cx: &mut Context<Editor>| {
            ed.settings_open = false;
            cx.notify();
        };
        let width = MAX_WIDTH.min(f32::from(viewport.width) * 0.92);
        let height = MAX_HEIGHT.min(f32::from(viewport.height) * 0.88);
        let version = updater::VERSION;
        div()
            .id("settings-backdrop")
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
                    .id("settings")
                    .w(px(width))
                    .max_h(px(height))
                    .flex()
                    .flex_col()
                    .rounded(px(12.))
                    .bg(theme::app_bg())
                    .border_1()
                    .border_color(theme::card_border())
                    .shadow_lg()
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
                            .border_color(theme::hairline())
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(theme::LG))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Settings"),
                            )
                            .child(
                                div()
                                    .id("settings-close")
                                    .size(px(28.))
                                    .rounded(px(6.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme::fill_subtle()))
                                    .on_mouse_down(MouseButton::Left, cx.listener(close))
                                    .child(Icon::Close.el(18., theme::text_secondary())),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p(px(20.))
                            .pb(px(4.))
                            .child(group("UPDATES", vec![self.update_row(cx)]))
                            .child(group("PRIVACY", vec![self.usage_data_row(cx)]))
                            .child(group(
                                "ABOUT",
                                vec![
                                    row(Icon::InformationCircle, "Version", None)
                                        .child(
                                            div()
                                                .text_size(px(theme::SM))
                                                .text_color(theme::text_secondary())
                                                .child(version),
                                        )
                                        .into_any_element(),
                                ],
                            )),
                    ),
            )
            .into_any_element()
    }

    /// Anonymous usage events (src/analytics.rs): on by default, off stops
    /// them at once and drops what's still waiting.
    fn usage_data_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.prefs.share_usage_data;
        row(
            Icon::Speedometer,
            "Share anonymous usage data",
            Some(
                "Which features get used, to make Reel Studio better. Never your \
                 media, projects or text."
                    .into(),
            ),
        )
        .child(
            switch("settings-usage-data", on, false).on_click(cx.listener(move |ed, _, _, cx| {
                ed.prefs.share_usage_data = !on;
                ed.prefs.save();
                crate::analytics::set_enabled(!on);
                cx.notify();
            })),
        )
        .into_any_element()
    }

    /// App updates: what the updater is doing, and the one button for it.
    fn update_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let u = self.updater.read(cx);
        let version = u.version.clone().unwrap_or_default();
        let sub: SharedString = match &u.status {
            Status::Checking => "Checking for updates…".into(),
            Status::Available => format!("Version {version} is available").into(),
            Status::UpToDate => "You're on the latest version".into(),
            Status::Downloading(p) => format!("Downloading… {}%", p.round() as u32).into(),
            Status::Downloaded | Status::Installed => {
                format!("Version {version} is downloaded").into()
            }
            Status::Failed => "Couldn't update. Check your connection and try again.".into(),
            Status::Unsupported => "Updates are only available in the installed app".into(),
            Status::Idle => format!("You're on version {}", updater::VERSION).into(),
        };
        let action: Option<(&str, bool, fn(&mut Editor, &mut Context<Editor>))> = match &u.status {
            Status::Available => Some(("Download", true, |ed, cx| {
                ed.updater.update(cx, |u, cx| u.download(cx))
            })),
            Status::Downloaded => Some(("Restart now", true, |ed, cx| ed.restart_to_update(cx))),
            Status::Failed => Some(("Try again", false, |ed, cx| {
                // A failed download retries the download; a failed check, the check.
                ed.updater.update(cx, |u, cx| {
                    if u.version.is_some() {
                        u.download(cx)
                    } else {
                        u.check(cx)
                    }
                })
            })),
            Status::Idle | Status::UpToDate => Some(("Check for updates", false, |ed, cx| {
                ed.updater.update(cx, |u, cx| u.check(cx))
            })),
            _ => None,
        };
        let busy = matches!(u.status, Status::Checking | Status::Downloading(_));
        row(Icon::CloudDownload, "App updates", Some(sub))
            .when(busy, |d| {
                d.child(
                    div()
                        .text_size(px(theme::SM))
                        .text_color(theme::text_tertiary())
                        .child("…"),
                )
            })
            .when_some(action, |d, (label, primary, run)| {
                d.child(
                    pill_button("settings-update", label, primary)
                        .rounded_full()
                        .on_click(cx.listener(move |ed, _, _, cx| {
                            run(ed, cx);
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }
}

/// A titled card of rows.
fn group(title: &'static str, rows: Vec<AnyElement>) -> gpui::Div {
    let count = rows.len();
    div()
        .mb(px(18.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .px(px(2.))
                .font_family(theme::FONT_MONO)
                .text_size(px(theme::SM))
                .text_color(theme::text_faint())
                .child(title),
        )
        .child(
            div()
                .rounded(px(12.))
                .bg(theme::sheet())
                .border_1()
                .border_color(theme::card_border())
                .overflow_hidden()
                .children(rows.into_iter().enumerate().map(move |(i, r)| {
                    div()
                        .when(i + 1 < count, |d| {
                            d.border_b_1().border_color(theme::hairline())
                        })
                        .child(r)
                })),
        )
}

/// A settings row: icon, label (and a line under it), then what follows.
fn row(icon: Icon, label: &'static str, sub: Option<SharedString>) -> gpui::Div {
    div()
        .min_h(px(48.))
        .px(px(16.))
        .py(px(10.))
        .flex()
        .items_center()
        .gap(px(12.))
        .child(icon.el(17., theme::text_secondary()))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_size(px(theme::MD)).child(label))
                .when_some(sub, |d, sub| {
                    d.child(
                        div()
                            .text_size(px(theme::SM))
                            .text_color(theme::text_secondary())
                            .child(sub),
                    )
                }),
        )
}
