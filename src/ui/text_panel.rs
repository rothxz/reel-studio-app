//! The inspector's Text › Basic for a text clip: its words, then every
//! look as always-open rows, as the old desktop panel: Text (font, size,
//! pattern, colour, opacity, alignment), Stroke / Glow / Background /
//! Shadow switched on and off at their title, Spacing and Curve. Sliders
//! show in the player while held and are one undo step on release.

use std::rc::Rc;

use gpui::{
    AnyElement, ClickEvent, Context, MouseButton, MouseDownEvent, SharedString, div, prelude::*, px,
};
use serde_json::Value;

use crate::editor::{Editor, PopupItem, SliderFn};
use crate::model::Clip;
use crate::text_render::{
    self, FONTS, LETTER_SPACING_MAX, LETTER_SPACING_MIN, TextStyle, defaults as d,
};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::visual::SWATCHES;
use crate::ui::widgets::{
    choice_pill, icon_button, panel_note, panel_row, section_with, slider_track_from, switch,
};

/// The colour a look gets when its switch turns it on.
const ON_COLORS: [(&str, &str); 4] = [
    ("outlineColor", "#000000"),
    ("glowColor", "#ffffff"),
    ("backgroundColor", "#000000"),
    ("shadowColor", "#000000"),
];

/// The Size slider's range (text units).
const MIN_SIZE: f64 = 12.;
const MAX_SIZE: f64 = 46.;

impl Editor {
    /// One undoable change to a text clip's fields.
    fn patch_text(
        &mut self,
        clip_id: &str,
        label: &'static str,
        patch: Vec<(&'static str, Option<Value>)>,
        cx: &mut Context<Self>,
    ) {
        self.edit_clip(
            clip_id,
            label,
            move |c| {
                let before = c.extra.clone();
                for (k, v) in patch {
                    match v {
                        Some(v) => {
                            c.extra.insert(k.into(), v);
                        }
                        None => {
                            c.extra.remove(k);
                        }
                    }
                }
                c.extra != before
            },
            cx,
        );
    }

    /// "Label | slider | read-out" over a text field shown `scale`× (a
    /// 0–1 field as 0–100). Held: the player shows it; released: one edit.
    #[allow(clippy::too_many_arguments)]
    fn text_slider(
        &self,
        clip: &Clip,
        key: &'static str,
        field: &'static str,
        label: &str,
        stored: f64,
        (min, max): (f64, f64),
        scale: f64,
        suffix: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let value = (stored * scale).round();
        let (shown, dragging) = match self.live_value {
            Some((k, v)) if k == key => (v, true),
            _ => (value, false),
        };
        let id = clip.id.clone();
        let start = value;
        let apply: SliderFn = Rc::new(move |ed, f, done, cx| {
            let v = (min + f as f64 * (max - min)).round();
            let stored = v / scale;
            let json = if scale == 1. {
                Value::from(v)
            } else {
                Value::from((stored * 1000.).round() / 1000.)
            };
            if done {
                ed.live_value = None;
                ed.live_text = None;
                if v != start {
                    ed.patch_text(&id, "Text Style", vec![(field, Some(json))], cx);
                } else {
                    ed.request_preview();
                }
            } else {
                ed.live_value = Some((key, v));
                ed.live_text = Some((id.clone(), field, json));
                ed.request_preview();
            }
            cx.notify();
        });
        let fraction = ((shown - min) / (max - min)).clamp(0., 1.) as f32;
        let origin = if min < 0. {
            ((0. - min) / (max - min)) as f32
        } else {
            0.
        };
        panel_row(label.to_string())
            .child(
                div()
                    .id(key)
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .cursor_pointer()
                    .child(self.measure(key))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            ed.start_slider(key, e.position.x, apply.clone(), cx);
                        }),
                    )
                    .child(slider_track_from(fraction, origin, dragging)),
            )
            .child(self.number_field(key, None, value, 0, suffix, 1., (min, max), None, cx))
            .into_any_element()
    }

    /// A colour row for a text field.
    fn text_colors(
        &self,
        clip: &Clip,
        key: &'static str,
        field: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chosen = clip
            .extra
            .get(field)
            .and_then(Value::as_str)
            .map(str::to_string);
        let id = clip.id.clone();
        self.swatch_row(
            key,
            &SWATCHES,
            chosen.as_deref(),
            false,
            move |ed, c, cx| {
                if let Some(c) = c {
                    ed.patch_text(&id, "Text Color", vec![(field, Some(c.into()))], cx);
                }
            },
            cx,
        )
    }

    /// A look's section: its title with an on/off switch.
    fn look_section(
        &self,
        clip: &Clip,
        title: &'static str,
        field: &'static str,
        cx: &mut Context<Self>,
    ) -> (AnyElement, bool) {
        let on = clip
            .extra
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty());
        let id = clip.id.clone();
        let color = ON_COLORS
            .iter()
            .find(|(f, _)| *f == field)
            .map_or("#000000", |(_, c)| c);
        let toggle = switch(SharedString::from(format!("look:{field}")), on, false).on_click(
            cx.listener(move |ed, _, _, cx| {
                let mut patch = vec![(field, if on { None } else { Some(Value::from(color)) })];
                if field == "backgroundColor" && !on {
                    patch.push(("backgroundStyle", Some("box".into())));
                }
                ed.patch_text(&id, if on { "Look Off" } else { "Look On" }, patch, cx);
            }),
        );
        (
            section_with(title, false, [toggle.into_any_element()]).into_any_element(),
            on,
        )
    }

    /// Text › Basic.
    pub fn render_text_panel(&self, clip: &Clip, cx: &mut Context<Self>) -> AnyElement {
        let style = TextStyle::from_clip(clip);
        let id = clip.id.clone();
        let mut rows: Vec<AnyElement> = vec![];

        // The words.
        rows.push(
            div()
                .mt(px(12.))
                .min_h(px(18. * 3. + 16.))
                .px(px(10.))
                .py(px(8.))
                .rounded(px(6.))
                .bg(theme::lane_bg())
                .border_1()
                .border_color(theme::chip_border())
                .text_size(px(13.))
                .line_height(px(18.))
                .text_color(theme::text())
                .child(self.text_field.clone())
                .into_any_element(),
        );

        rows.push(section_with("Text", false, []).into_any_element());
        // Font: a select (the fonts in a menu).
        let font_id = id.clone();
        let family = style.family.clone();
        rows.push(
            panel_row("Font")
                .child(
                    div()
                        .id("text-font")
                        .flex_1()
                        .min_w_0()
                        .h(px(26.))
                        .px_2()
                        .rounded(px(5.))
                        .flex()
                        .items_center()
                        .gap_1()
                        .bg(theme::chip_bg())
                        .border_1()
                        .border_color(theme::chip_border())
                        .cursor_pointer()
                        .hover(|s| s.border_color(theme::text_tertiary()))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |ed, e: &ClickEvent, _, cx| {
                            let items = FONTS
                                .iter()
                                .map(|(f, label)| {
                                    let (cid, f) = (font_id.clone(), *f);
                                    PopupItem::new(*label, move |ed, _, cx| {
                                        ed.patch_text(
                                            &cid,
                                            "Font",
                                            vec![("fontFamily", Some(f.into()))],
                                            cx,
                                        );
                                    })
                                    .checked(f == family)
                                })
                                .collect();
                            ed.open_popup(e.position(), items, cx);
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(theme::XS))
                                .text_color(theme::text())
                                .child(text_render::font_label(&style.family).to_string()),
                        )
                        .child(Icon::ChevronDown.el(12., theme::text_tertiary())),
                )
                .into_any_element(),
        );
        rows.push(self.text_slider(
            clip,
            "text:size",
            "fontSize",
            "Size",
            style.size,
            (MIN_SIZE, MAX_SIZE),
            1.,
            "",
            cx,
        ));
        // Pattern: bold, italic, underline.
        let pattern = [
            ("bold", Icon::FormatBold, "Bold", style.bold),
            ("italic", Icon::FormatItalic, "Italic", style.italic),
            (
                "underline",
                Icon::FormatUnderline,
                "Underline",
                style.underline,
            ),
        ];
        rows.push(
            panel_row("Pattern")
                .children(pattern.into_iter().map(|(field, icon, label, on)| {
                    let id = id.clone();
                    icon_button(SharedString::from(format!("text-{field}")), icon, label)
                        .size(15.)
                        .active(on)
                        .on_click(cx.listener(move |ed, _, _, cx| {
                            ed.patch_text(&id, label, vec![(field, Some((!on).into()))], cx)
                        }))
                }))
                .into_any_element(),
        );
        rows.push(panel_row("Color").into_any_element());
        rows.push(self.text_colors(clip, "text:color", "color", cx));
        rows.push(self.text_slider(
            clip,
            "text:opacity",
            "textOpacity",
            "Opacity",
            style.opacity,
            (0., 100.),
            100.,
            "%",
            cx,
        ));
        let aligns = [
            ("left", Icon::AlignLeft, "Left", text_render::Align::Left),
            (
                "center",
                Icon::AlignCenter,
                "Center",
                text_render::Align::Center,
            ),
            (
                "right",
                Icon::AlignRight,
                "Right",
                text_render::Align::Right,
            ),
        ];
        rows.push(
            panel_row("Alignment")
                .children(aligns.into_iter().map(|(value, icon, label, a)| {
                    let id = id.clone();
                    icon_button(
                        SharedString::from(format!("text-align-{value}")),
                        icon,
                        label,
                    )
                    .size(15.)
                    .active(style.align == a)
                    .on_click(cx.listener(move |ed, _, _, cx| {
                        ed.patch_text(
                            &id,
                            "Alignment",
                            vec![("alignment", Some(value.into()))],
                            cx,
                        )
                    }))
                }))
                .into_any_element(),
        );

        // Stroke.
        let (head, on) = self.look_section(clip, "Stroke", "outlineColor", cx);
        rows.push(head);
        if on {
            rows.push(self.text_colors(clip, "stroke:color", "outlineColor", cx));
            let w = style.stroke.as_ref().map_or(d::OUTLINE_WIDTH, |(_, w)| *w);
            rows.push(self.text_slider(
                clip,
                "stroke:width",
                "outlineWidth",
                "Thickness",
                w,
                (0., 100.),
                100.,
                "",
                cx,
            ));
        }

        // Glow.
        let (head, on) = self.look_section(clip, "Glow", "glowColor", cx);
        rows.push(head);
        if on {
            rows.push(self.text_colors(clip, "glow:color", "glowColor", cx));
            let (k, range) = style
                .glow
                .as_ref()
                .map_or((d::GLOW_INTENSITY, d::GLOW_RANGE), |(_, k, r)| (*k, *r));
            rows.push(self.text_slider(
                clip,
                "glow:k",
                "glowIntensity",
                "Intensity",
                k,
                (0., 100.),
                100.,
                "",
                cx,
            ));
            rows.push(self.text_slider(
                clip,
                "glow:range",
                "glowRange",
                "Range",
                range,
                (0., 100.),
                100.,
                "",
                cx,
            ));
        }

        // Background.
        let (head, on) = self.look_section(clip, "Background", "backgroundColor", cx);
        rows.push(head);
        if let (true, Some(bg)) = (on, style.background.as_ref()) {
            let shapes = [
                ("box", "Box", text_render::BgShape::Box),
                ("lines", "Per line", text_render::BgShape::Lines),
            ];
            rows.push(
                panel_row("Shape")
                    .child(div().flex().gap(px(6.)).children(shapes.into_iter().map(
                        |(v, label, shape)| {
                            let id = id.clone();
                            choice_pill(
                                SharedString::from(format!("bg-shape-{v}")),
                                label,
                                bg.shape == shape,
                                true,
                            )
                            .on_click(cx.listener(
                                move |ed, _, _, cx| {
                                    ed.patch_text(
                                        &id,
                                        "Background Shape",
                                        vec![("backgroundStyle", Some(v.into()))],
                                        cx,
                                    )
                                },
                            ))
                        },
                    )))
                    .into_any_element(),
            );
            rows.push(self.text_colors(clip, "bg:color", "backgroundColor", cx));
            rows.push(self.text_slider(
                clip,
                "bg:opacity",
                "backgroundOpacity",
                "Opacity",
                bg.opacity,
                (0., 100.),
                100.,
                "%",
                cx,
            ));
            rows.push(self.text_slider(
                clip,
                "bg:radius",
                "backgroundRadius",
                "Roundness",
                bg.radius,
                (0., 100.),
                100.,
                "",
                cx,
            ));
            rows.push(self.text_slider(
                clip,
                "bg:height",
                "backgroundHeight",
                "Height",
                bg.height,
                (0., 100.),
                100.,
                "",
                cx,
            ));
            rows.push(self.text_slider(
                clip,
                "bg:width",
                "backgroundWidth",
                "Width",
                bg.width,
                (0., 100.),
                100.,
                "",
                cx,
            ));
        }

        // Shadow.
        let (head, on) = self.look_section(clip, "Shadow", "shadowColor", cx);
        rows.push(head);
        if let (true, Some(sh)) = (on, style.shadow.as_ref()) {
            rows.push(self.text_colors(clip, "shadow:color", "shadowColor", cx));
            rows.push(self.text_slider(
                clip,
                "shadow:opacity",
                "shadowOpacity",
                "Opacity",
                sh.opacity,
                (0., 100.),
                100.,
                "%",
                cx,
            ));
            rows.push(self.text_slider(
                clip,
                "shadow:blur",
                "shadowBlur",
                "Blur",
                sh.blur,
                (0., 100.),
                100.,
                "",
                cx,
            ));
            rows.push(self.text_slider(
                clip,
                "shadow:distance",
                "shadowDistance",
                "Distance",
                sh.distance,
                (0., 100.),
                100.,
                "",
                cx,
            ));
            rows.push(self.text_slider(
                clip,
                "shadow:angle",
                "shadowAngle",
                "Angle",
                sh.angle,
                (-180., 180.),
                1.,
                "°",
                cx,
            ));
        }

        // Spacing.
        rows.push(section_with("Spacing", false, []).into_any_element());
        rows.push(self.text_slider(
            clip,
            "text:letter",
            "letterSpacing",
            "Letter",
            style.letter_spacing,
            (LETTER_SPACING_MIN * 100., LETTER_SPACING_MAX * 100.),
            100.,
            "",
            cx,
        ));
        rows.push(self.text_slider(
            clip,
            "text:line",
            "lineSpacing",
            "Line",
            style.line_spacing,
            (0., 100.),
            100.,
            "",
            cx,
        ));

        // Curve.
        rows.push(section_with("Curve", false, []).into_any_element());
        rows.push(self.text_slider(
            clip,
            "text:curve",
            "curve",
            "Curve",
            style.curve,
            (-100., 100.),
            100.,
            "",
            cx,
        ));
        rows.push(panel_note("Curved text sits on one line.").into_any_element());

        div()
            .px(px(14.))
            .pb(px(14.))
            .flex()
            .flex_col()
            .children(rows)
            .into_any_element()
    }
}
