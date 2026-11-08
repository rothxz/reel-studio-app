//! The update dialog. The app checks for a new version on launch (and every
//! few hours); when one is found this opens by itself: the version, when
//! it was released, what's new (the release notes), then Download & install
//! with its progress, then Restart now. "Later" closes it until the next
//! step (a finished download asks again). The menu's Check for Updates…
//! opens it on demand, also to say "You're up to date".

use gpui::{AnyElement, Context, FontWeight, Pixels, SharedString, Size, div, prelude::*, px};

use crate::editor::Editor;
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets::pill_button;
use crate::updater::{self, Status};

const WIDTH: f32 = 440.;
const NOTES_MAX_HEIGHT: f32 = 260.;

impl Editor {
    /// The menu's Check for Updates…: checks and shows the dialog.
    pub fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        self.update_dialog_open = true;
        self.popup = None;
        self.updater.update(cx, |u, cx| u.check(cx));
        cx.notify();
    }

    /// Saves the open project, then installs the download and relaunches.
    pub(crate) fn restart_to_update(&mut self, cx: &mut Context<Self>) {
        self.save_now(cx);
        self.updater.update(cx, |u, cx| u.restart(cx));
    }

    /// The dialog is up: opened from the menu, or a check found a version
    /// (or finished downloading it) that wasn't put off with "Later". It
    /// doesn't open by itself over an export.
    pub(crate) fn update_dialog_shown(&self, cx: &gpui::App) -> bool {
        if self.update_dialog_open {
            return true;
        }
        let u = self.updater.read(cx);
        let Some(version) = &u.version else {
            return false;
        };
        let found = matches!(
            u.status,
            Status::Available | Status::Downloading(_) | Status::Downloaded | Status::Installed
        );
        found
            && self.export.is_none()
            && self.update_hidden.as_ref() != Some(&(version.clone(), is_downloaded(&u.status)))
    }

    /// Later (or Esc, or Close): hides it until the next step.
    pub(crate) fn update_later(&mut self, cx: &mut Context<Self>) {
        let u = self.updater.read(cx);
        self.update_hidden = u.version.clone().map(|v| (v, is_downloaded(&u.status)));
        self.update_dialog_open = false;
        cx.notify();
    }

    fn download_update(&mut self, cx: &mut Context<Self>) {
        // Stays up through the download (and a failure), not only while
        // the found version is new.
        self.update_dialog_open = true;
        self.updater.update(cx, |u, cx| u.download(cx));
        cx.notify();
    }

    pub(crate) fn render_update_dialog(
        &mut self,
        viewport: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let u = self.updater.read(cx);
        let status = u.status.clone();
        let version = u.version.clone().unwrap_or_default();
        let notes = u.notes.clone();
        let released = u.released.clone();
        let current = updater::VERSION;

        let (icon, title, sub): (Icon, SharedString, SharedString) = match &status {
            Status::Checking => (
                Icon::CloudDownload,
                "Checking for updates…".into(),
                format!("You're on version {current}").into(),
            ),
            Status::Idle | Status::UpToDate => (
                Icon::CheckmarkCircle,
                "You're up to date".into(),
                format!("Reel Studio {current} is the newest version.").into(),
            ),
            Status::Unsupported => (
                Icon::InformationCircle,
                "Updates aren't available".into(),
                "Updates only work in the installed app.".into(),
            ),
            Status::Failed => (
                Icon::InformationCircle,
                "Couldn't update".into(),
                "Check your connection and try again.".into(),
            ),
            Status::Available | Status::Downloading(_) => (
                Icon::CloudDownload,
                "A new version is available".into(),
                match &released {
                    Some(date) => {
                        format!("Version {version} · released {date} · you have {current}")
                    }
                    None => format!("Version {version} · you have {current}"),
                }
                .into(),
            ),
            Status::Downloaded | Status::Installed => (
                Icon::RefreshCircle,
                format!("Version {version} is ready").into(),
                "Restart Reel Studio to finish updating. Your project is saved first.".into(),
            ),
        };
        let show_notes = !version.is_empty()
            && matches!(
                status,
                Status::Available
                    | Status::Downloading(_)
                    | Status::Downloaded
                    | Status::Installed
                    | Status::Failed
            );

        div()
            .id("update-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .bg(theme::backdrop())
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("update-dialog")
                    .w(px(WIDTH.min(f32::from(viewport.width) * 0.92)))
                    .p(px(24.))
                    .flex()
                    .flex_col()
                    .gap(px(18.))
                    .rounded(px(14.))
                    .bg(theme::app_bg())
                    .border_1()
                    .border_color(theme::card_border())
                    .shadow_lg()
                    // Header: the icon in a soft tile, title and the line under it.
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(14.))
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(44.))
                                    .rounded(px(12.))
                                    .bg(theme::fill_subtle())
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(icon.el(24., theme::text())),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.))
                                    .child(
                                        div()
                                            .text_size(px(theme::LG))
                                            .font_weight(FontWeight::BOLD)
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::SM))
                                            .text_color(theme::text_secondary())
                                            .child(sub),
                                    ),
                            ),
                    )
                    .when(show_notes, |d| d.child(notes_box(notes)))
                    .when_some(
                        match status {
                            Status::Downloading(p) => Some(p),
                            _ => None,
                        },
                        |d, p| d.child(progress(p)),
                    )
                    .child(self.update_buttons(&status, cx)),
            )
            .into_any_element()
    }

    /// The dialog's buttons for where the updater is.
    fn update_buttons(&self, status: &Status, cx: &mut Context<Self>) -> impl IntoElement {
        let later = |label: &'static str| {
            pill_button("update-later", label, false)
                .flex_1()
                .h(px(34.))
                .rounded_full()
                .on_click(cx.listener(|ed, _, _, cx| ed.update_later(cx)))
        };
        let row = div().flex().gap(px(8.));
        match status {
            Status::Available => row.child(later("Later")).child(
                pill_button("update-download", "Download & install", true)
                    .flex_1()
                    .h(px(34.))
                    .rounded_full()
                    .on_click(cx.listener(|ed, _, _, cx| ed.download_update(cx))),
            ),
            Status::Downloading(_) => row.child(later("Hide")).child(
                pill_button("update-downloading", "Downloading…", true)
                    .flex_1()
                    .h(px(34.))
                    .rounded_full()
                    .opacity(0.5)
                    .cursor_default(),
            ),
            Status::Downloaded | Status::Installed => row.child(later("Later")).child(
                pill_button("update-restart", "Restart now", true)
                    .flex_1()
                    .h(px(34.))
                    .rounded_full()
                    .on_click(cx.listener(|ed, _, _, cx| {
                        ed.restart_to_update(cx);
                        cx.notify();
                    })),
            ),
            Status::Failed => row.child(later("Close")).child(
                pill_button("update-retry", "Try again", true)
                    .flex_1()
                    .h(px(34.))
                    .rounded_full()
                    .on_click(cx.listener(|ed, _, _, cx| {
                        // A failed download retries the download; a failed
                        // check, the check.
                        if ed.updater.read(cx).version.is_some() {
                            ed.download_update(cx);
                        } else {
                            ed.check_for_updates(cx);
                        }
                    })),
            ),
            Status::Checking | Status::Idle | Status::UpToDate | Status::Unsupported => {
                row.child(later(if *status == Status::Checking {
                    "Cancel"
                } else {
                    "OK"
                }))
            }
        }
    }
}

fn is_downloaded(status: &Status) -> bool {
    matches!(status, Status::Downloaded | Status::Installed)
}

/// "What's new": the release notes, a little Markdown (headings, bullets,
/// paragraphs) in a scrolling card.
fn notes_box(notes: Option<SharedString>) -> impl IntoElement {
    let lines: Vec<AnyElement> = match &notes {
        Some(text) => text
            .lines()
            .filter_map(|line| note_line(line.trim_end()))
            .collect(),
        None => vec![
            div()
                .text_color(theme::text_tertiary())
                .child("No release notes for this version.")
                .into_any_element(),
        ],
    };
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .font_family(theme::FONT_MONO)
                .text_size(px(theme::XS))
                .text_color(theme::text_faint())
                .child("WHAT'S NEW"),
        )
        .child(
            div()
                .id("update-notes")
                .max_h(px(NOTES_MAX_HEIGHT))
                .overflow_y_scroll()
                .p(px(14.))
                .rounded(px(10.))
                .bg(theme::sheet())
                .border_1()
                .border_color(theme::card_border())
                .flex()
                .flex_col()
                .gap(px(6.))
                .text_size(px(theme::SM))
                .text_color(theme::text_secondary())
                .children(lines),
        )
}

/// One line of the notes; None for a blank one. The notes' own "What's new"
/// heading is dropped (the box has its label).
fn note_line(line: &str) -> Option<AnyElement> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(heading) = trimmed.strip_prefix('#') {
        let heading = plain(heading.trim_start_matches('#').trim());
        if heading.eq_ignore_ascii_case("what's new") {
            return None;
        }
        return Some(
            div()
                .pt(px(4.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text())
                .child(heading)
                .into_any_element(),
        );
    }
    if let Some(item) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
    {
        return Some(
            div()
                .flex()
                .gap(px(8.))
                .child(div().flex_none().child("•"))
                .child(div().flex_1().min_w_0().child(plain(item)))
                .into_any_element(),
        );
    }
    Some(div().child(plain(trimmed)).into_any_element())
}

/// The text without Markdown's emphasis and code marks.
fn plain(text: &str) -> String {
    text.replace("**", "").replace('`', "")
}

/// The download's progress bar.
fn progress(percent: f32) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .flex_1()
                .h(px(6.))
                .rounded_full()
                .bg(theme::fill_subtle())
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .rounded_full()
                        .bg(theme::text())
                        .w(gpui::relative((percent / 100.).clamp(0., 1.))),
                ),
        )
        .child(
            div()
                .w(px(36.))
                .text_right()
                .font_family(theme::FONT_MONO)
                .text_size(px(theme::XS))
                .text_color(theme::text_secondary())
                .child(format!("{}%", percent.round() as u32)),
        )
}
