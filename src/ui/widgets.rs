//! Small building blocks shared by every panel: the compact icon button
//! with its hover tooltip, header chips, dividers, key caps, and the
//! inspector's section / row / segment parts.

use gpui::{
    AnyView, App, ClickEvent, Div, ElementId, FontWeight, Hsla, IntoElement, ParentElement,
    RenderOnce, SharedString, Stateful, Styled, Window, div, prelude::*, px,
};

use crate::shortcuts::{self, Shortcut};
use crate::theme;
use crate::ui::icons::Icon;

pub type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

// ---- text -----------------------------------------------------------------

/// Small uppercase mono metadata ("SAVED · LOCAL").
pub fn label_text(s: impl Into<SharedString>, color: impl Into<Hsla>) -> Div {
    div()
        .font_family(theme::FONT_MONO)
        .font_weight(FontWeight::MEDIUM)
        .text_size(px(theme::XS))
        .text_color(color.into())
        .child(s.into().to_uppercase())
}

pub fn mono(s: impl Into<SharedString>, size: f32, color: impl Into<Hsla>) -> Div {
    div()
        .font_family(theme::FONT_MONO)
        .text_size(px(size))
        .text_color(color.into())
        .child(s.into())
}

// ---- tooltip ----------------------------------------------------------------

/// The hover tooltip: a label, and its shortcut dimmer after it.
pub struct Tooltip {
    name: SharedString,
    hint: Option<SharedString>,
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .font_family(theme::FONT_BODY)
            .text_size(px(theme::XS))
            .line_height(px(15.))
            .px_2()
            .py_1()
            .rounded(px(5.))
            .bg(theme::button_bg())
            .border_1()
            .border_color(theme::chip_border())
            .shadow_md()
            .text_color(theme::text())
            .flex()
            .gap_2()
            .child(self.name.clone())
            .when_some(self.hint.clone(), |d, hint| {
                d.child(div().text_color(theme::text_tertiary()).child(hint))
            })
    }
}

pub fn tooltip(
    name: impl Into<SharedString>,
    hint: Option<String>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let name = name.into();
    let hint: Option<SharedString> = hint.map(Into::into);
    move |_, cx| {
        let (name, hint) = (name.clone(), hint.clone());
        cx.new(|_| Tooltip { name, hint }).into()
    }
}

// ---- icon button --------------------------------------------------------------

/// The compact 28 px icon button of the toolbar and transport: hover fill,
/// a tooltip (label + shortcut), and optionally a toggle look (lit when on,
/// dimmed when off).
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: Icon,
    icon_size: f32,
    label: SharedString,
    hint: Option<String>,
    active: Option<bool>,
    disabled: bool,
    on_click: Option<OnClick>,
}

pub fn icon_button(
    id: impl Into<ElementId>,
    icon: Icon,
    label: impl Into<SharedString>,
) -> IconButton {
    IconButton {
        id: id.into(),
        icon,
        icon_size: 16.,
        label: label.into(),
        hint: None,
        active: None,
        disabled: false,
        on_click: None,
    }
}

impl IconButton {
    pub fn size(mut self, size: f32) -> Self {
        self.icon_size = size;
        self
    }
    pub fn shortcut(mut self, id: Shortcut) -> Self {
        self.hint = Some(shortcuts::hint(id));
        self
    }
    pub fn shortcut_opt(mut self, id: Option<Shortcut>) -> Self {
        self.hint = id.map(shortcuts::hint);
        self
    }
    /// A toggle: lit when on, dimmed when off.
    pub fn active(mut self, on: bool) -> Self {
        self.active = Some(on);
        self
    }
    /// A switch that is only highlighted while on (off reads as normal).
    pub fn lit(mut self, on: bool) -> Self {
        self.active = on.then_some(true);
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let color: Hsla = if self.disabled {
            theme::text_disabled().into()
        } else if self.active == Some(false) {
            theme::text_tertiary().into()
        } else {
            theme::text().into()
        };
        let disabled = self.disabled;
        div()
            .id(self.id)
            .flex_none()
            .size(px(28.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .justify_center()
            .when(self.active == Some(true) && !disabled, |d| {
                d.bg(theme::button_bg())
            })
            .when(!disabled, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(theme::button_hover()))
                    .active(|s| s.bg(theme::button_hover()))
            })
            .child(self.icon.el(self.icon_size, color))
            .tooltip(tooltip(self.label, self.hint))
            .when_some(self.on_click.filter(|_| !disabled), |d, f| d.on_click(f))
    }
}

// ---- chips, dividers, buttons ----------------------------------------------------

/// The header's bordered chip (Shortcuts, 1080p ▾).
pub fn chip(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(5.))
        .h(px(26.))
        .px(px(10.))
        .rounded(px(6.))
        .bg(theme::chip_bg())
        .border_1()
        .border_color(theme::chip_border())
        .cursor_pointer()
        .hover(|s| s.bg(theme::button_hover()))
}

/// A small pill button (Modify, Reset); `primary` is drawn white.
pub fn pill_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(56.))
        .h(px(30.))
        .px(px(16.))
        .rounded(px(6.))
        .text_size(px(theme::XS))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .map(|d| {
            if primary {
                d.bg(theme::text())
                    .text_color(theme::app_bg())
                    .hover(|s| s.opacity(0.88))
            } else {
                d.bg(theme::button_bg())
                    .text_color(theme::text())
                    .hover(|s| s.bg(theme::button_hover()))
            }
        })
        .child(label.into())
}

/// The thin vertical rule between toolbar groups.
pub fn vdivider() -> Div {
    div()
        .flex_none()
        .w(px(1.))
        .h(px(18.))
        .mx(px(6.))
        .bg(theme::chip_border())
}

/// One key of a shortcut, as a cap.
pub fn keycap(key: String) -> Div {
    div()
        .min_w(px(24.))
        .h(px(22.))
        .px(px(6.))
        .rounded(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .bg(theme::button_bg())
        .border_1()
        .border_color(theme::chip_border())
        .text_size(px(theme::SM))
        .text_color(theme::text())
        .child(key)
}

/// A panel's title strip ("Player", "Details").
pub fn panel_header(title: impl Into<SharedString>, height: f32) -> Div {
    div()
        .flex_none()
        .h(px(height))
        .px(px(14.))
        .flex()
        .items_center()
        .border_b_1()
        .border_color(theme::card_border())
        .text_size(px(theme::SM))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::text())
        .child(title.into())
}

// ---- inspector parts ----------------------------------------------------------------

/// A section heading: bold title, then whatever sits right of it.
pub fn section(title: impl Into<SharedString>, first: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .when(!first, |d| {
            d.child(
                div()
                    .h(px(1.))
                    .mt(px(8.))
                    .mb(px(2.))
                    .bg(theme::card_border()),
            )
        })
        .child(
            div()
                .h(px(34.))
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(theme::XS))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text())
                .child(title.into()),
        )
}

/// "Label  [controls…]": one row of a section.
pub fn panel_row(label: impl Into<SharedString>) -> Div {
    div().h(px(34.)).flex().items_center().gap_2().child(
        div()
            .flex_none()
            .w(px(72.))
            .truncate()
            .text_size(px(theme::XS))
            .text_color(theme::text_secondary())
            .child(label.into()),
    )
}

/// The read-out box at the end of a slider row.
pub fn value_box(s: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .w(px(76.))
        .h(px(26.))
        .px_2()
        .rounded(px(5.))
        .flex()
        .items_center()
        .justify_end()
        .bg(theme::chip_bg())
        .border_1()
        .border_color(theme::chip_border())
        .font_family(theme::FONT_MONO)
        .text_size(px(theme::XS))
        .text_color(theme::text())
        .child(s.into())
}

/// The inspector's sub-tab pills ("Normal | Curve").
pub fn segment_tabs(tabs: Vec<(SharedString, bool, OnClick)>) -> Div {
    div()
        .mx(px(14.))
        .mt_2()
        .mb(px(2.))
        .p(px(2.))
        .gap(px(2.))
        .rounded(px(6.))
        .bg(theme::chip_bg())
        .flex()
        .children(tabs.into_iter().enumerate().map(|(i, (label, on, f))| {
            div()
                .id(("segment", i))
                .flex_grow()
                .flex()
                .justify_center()
                .py_1()
                .px_1()
                .rounded(px(5.))
                .cursor_pointer()
                .text_size(px(theme::XS))
                .when(on, |d| d.bg(theme::button_bg()).text_color(theme::text()))
                .when(!on, |d| d.text_color(theme::text_secondary()))
                .on_click(f)
                .child(label)
        }))
}

/// A slider's track, fill and thumb at `fraction` (0–1). The caller makes
/// it draggable (it records the bounds and handles mouse down).
pub fn slider_track(fraction: f32, dragging: bool) -> Div {
    slider_track_from(fraction, 0., dragging)
}

/// A slider whose fill grows from `origin` (0.5: a centre tick, for a
/// value that goes both ways).
pub fn slider_track_from(fraction: f32, origin: f32, dragging: bool) -> Div {
    let f = fraction.clamp(0., 1.);
    let (lo, hi) = (f.min(origin), f.max(origin));
    div()
        .relative()
        .h(px(20.))
        .mx(px(5.))
        .flex()
        .items_center()
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .h(px(2.))
                .rounded(px(1.))
                .bg(theme::button_hover()),
        )
        .child(
            div()
                .absolute()
                .left(gpui::relative(lo))
                .w(gpui::relative(hi - lo))
                .h(px(2.))
                .rounded(px(1.))
                .bg(theme::text_secondary()),
        )
        .when(origin > 0., |d| {
            d.child(
                div()
                    .absolute()
                    .left(gpui::relative(origin))
                    .ml(px(-0.5))
                    .w(px(1.))
                    .h(px(8.))
                    .bg(theme::text_tertiary()),
            )
        })
        .child(
            div()
                .absolute()
                .left(gpui::relative(f))
                .ml(px(if dragging { -6. } else { -5. }))
                .size(px(if dragging { 12. } else { 10. }))
                .rounded_full()
                .bg(theme::text()),
        )
}

/// A section heading with controls on its right (reset, ‹ ◇ ›, a switch).
pub fn section_with(
    title: impl Into<SharedString>,
    first: bool,
    right: impl IntoIterator<Item = gpui::AnyElement>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .when(!first, |d| {
            d.child(
                div()
                    .h(px(1.))
                    .mt(px(8.))
                    .mb(px(2.))
                    .bg(theme::card_border()),
            )
        })
        .child(
            div()
                .h(px(34.))
                .flex()
                .items_center()
                .gap(px(2.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(theme::XS))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::text())
                        .child(title.into()),
                )
                .children(right),
        )
}

/// "Label ……… [switch]": an on/off row.
pub fn switch_row(label: impl Into<SharedString>, switch: impl IntoElement) -> Div {
    div()
        .h(px(34.))
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(theme::XS))
                .text_color(theme::text_secondary())
                .child(label.into()),
        )
        .child(switch)
}

/// The inspector's small black-and-white switch.
pub fn switch(id: impl Into<ElementId>, on: bool, disabled: bool) -> Stateful<Div> {
    let (track, thumb): (Hsla, Hsla) = if on {
        (theme::text().into(), theme::app_bg().into())
    } else {
        (theme::button_bg().into(), theme::text().into())
    };
    div()
        .id(id)
        .flex_none()
        .w(px(30.))
        .h(px(18.))
        .p(px(2.))
        .rounded_full()
        .bg(track)
        .flex()
        .when(on, |d| d.justify_end())
        .when(disabled, |d| d.opacity(0.4))
        .when(!disabled, |d| d.cursor_pointer())
        .child(div().size(px(14.)).rounded_full().bg(thumb))
}

/// A greyed note under a section (a hint, what a value means).
pub fn panel_note(text: impl Into<SharedString>) -> Div {
    div()
        .mt(px(2.))
        .mb_1()
        .text_size(px(theme::XS))
        .line_height(px(16.))
        .text_color(theme::text_tertiary())
        .child(text.into())
}

/// A small pill (a choice among a few: Color / Image / Blur); `on` is
/// drawn white, a disabled one greyed.
pub fn choice_pill(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    enabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(56.))
        .h(px(24.))
        .px(px(10.))
        .rounded(px(6.))
        .text_size(px(theme::XS))
        .font_weight(FontWeight::MEDIUM)
        .map(|d| {
            if on {
                d.bg(theme::text()).text_color(theme::app_bg())
            } else if enabled {
                d.bg(theme::button_bg())
                    .text_color(theme::text())
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::button_hover()))
            } else {
                d.bg(theme::button_bg()).text_color(theme::text_disabled())
            }
        })
        .child(label.into())
}

/// A square picker tile: `content` centred, a white ring when chosen, the
/// label under it.
pub fn pick_tile(
    id: impl Into<ElementId>,
    size: f32,
    on: bool,
    content: impl IntoElement,
    label: Option<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .w(px(size))
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .child(
            div()
                .relative()
                .size(px(size))
                .rounded(px(8.))
                .overflow_hidden()
                .bg(theme::button_bg())
                .border_2()
                .border_color(if on {
                    theme::text().into()
                } else {
                    gpui::transparent_black()
                })
                .hover(|s| s.bg(theme::button_hover()))
                .flex()
                .items_center()
                .justify_center()
                .child(content),
        )
        .when_some(label, |d, label| {
            d.child(
                div()
                    .w_full()
                    .truncate()
                    .text_center()
                    .text_size(px(10.))
                    .text_color(if on {
                        theme::text()
                    } else {
                        theme::text_tertiary()
                    })
                    .child(label),
            )
        })
}
