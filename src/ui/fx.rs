//! Effects and Filter: the tile grids in the browser, the effect clip's
//! settings in the inspector (its Adjust sliders), and the edits a pick
//! makes. Looks are only ever picked in the browser; the inspector only
//! holds settings, never a second grid.
//!
//! - **Video effects** are global: a pick adds a 3 s clip on the effect
//!   lane at the playhead and selects it, so the next pick replaces it
//!   (None deletes it). The list is the served catalog (`catalog.rs`),
//!   fetched a page at a time as a tab is shown and scrolled.
//! - **Filters** go on the clip the tools act on (the selected video or
//!   photo, else the main clip under the playhead), one per clip; a new
//!   pick keeps the intensity already dialled in.
//!
//! Tiles show the hosted preview (`assets.rs`): the first frame, and the
//! animation while the pointer is over it. A picked effect's sliders are
//! in the inspector (its clip is selected); the picked filter tile wears
//! Adjust and opens its sliders over the grid (one undo step per drag).

use std::rc::Rc;

use gpui::{
    AnyElement, Context, Div, FontWeight, MouseButton, MouseDownEvent, ObjectFit, ScrollHandle,
    SharedString, div, img, prelude::*, px, rgba,
};

use crate::catalog::{
    self, DEFAULT_FILTER_INTENSITY, EFFECT_PREFIX, EFFECT_TABS, FILTER_PREFIX, FILTER_TABS,
    ParamSpec, default_effect_params, default_parameters, filter_params,
};
use crate::editor::{Editor, SliderFn};
use crate::effects::stack::LiveParam;
use crate::i18n::t;
use crate::model::{EffectInstance, TrackType};
use crate::theme;
use crate::ui::browser::{EFFECTS_TAB, FILTER_TAB};
use crate::ui::icons::Icon;
use crate::ui::widgets::{icon_button, panel_row, pill_button, value_box};

const TILE_MIN: f32 = 92.;
const TILE_GAP: f32 = 8.;
const GRID_INSET: f32 = 12.;

/// Which picker a grid belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Picker {
    Effects,
    Filters,
}

impl Picker {
    fn id(self) -> &'static str {
        match self {
            Picker::Effects => "fx-effects",
            Picker::Filters => "fx-filters",
        }
    }
}

/// Where a set of sliders sits (their bounds are measured per place).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SliderPlace {
    Browser,
    Inspector,
}

/// A slider's measure key: one per place and parameter.
fn slider_key(place: SliderPlace, id: &str) -> &'static str {
    use SliderPlace::*;
    match (place, id) {
        (Browser, "intensity") => "fxb:intensity",
        (Browser, "speed") => "fxb:speed",
        (Browser, "p1") => "fxb:p1",
        (Browser, "p2") => "fxb:p2",
        (Browser, "p3") => "fxb:p3",
        (Browser, "p4") => "fxb:p4",
        (Browser, _) => "fxb:other",
        (Inspector, "intensity") => "fxi:intensity",
        (Inspector, "speed") => "fxi:speed",
        (Inspector, "p1") => "fxi:p1",
        (Inspector, "p2") => "fxi:p2",
        (Inspector, "p3") => "fxi:p3",
        (Inspector, "p4") => "fxi:p4",
        (Inspector, _) => "fxi:other",
    }
}

/// One tile: a look (None: the "None" tile that removes it).
struct Tile {
    name: Option<String>,
    picture: Option<String>,
    on: bool,
}

/// A picked effect and the clip it's on, with its sliders.
pub struct Applied {
    pub clip_id: String,
    pub effect: EffectInstance,
    pub specs: Vec<ParamSpec>,
}

impl Editor {
    // ---- what the pickers act on -----------------------------------------

    /// The clip Filter acts on: the selected video / photo (main or
    /// overlay lane), else the main lane's clip under the playhead.
    pub fn active_video_clip(&self) -> Option<String> {
        let picture = |kind: &str| kind == "video" || kind == "image";
        for t in &self.project.tracks {
            if !matches!(t.kind, TrackType::Video | TrackType::Overlay) {
                continue;
            }
            if let Some(c) = t
                .clips
                .iter()
                .find(|c| picture(&c.kind) && self.selected.contains(&c.id))
            {
                return Some(c.id.clone());
            }
        }
        let main = self.project.main_track()?;
        main.clips
            .iter()
            .find(|c| {
                picture(&c.kind) && self.playhead >= c.timeline_start && self.playhead < c.end()
            })
            .map(|c| c.id.clone())
    }

    /// The selected effect-lane clip (a pick replaces its effect).
    fn selected_effect_clip(&self) -> Option<String> {
        self.selected_clip()
            .filter(|(_, c)| c.kind == "effect")
            .map(|(_, c)| c.id.clone())
    }

    /// The selected effect clip's video effect, with its sliders.
    pub fn applied_effect(&self) -> Option<Applied> {
        let id = self.selected_effect_clip()?;
        let (_, clip) = self.project.find_clip(&id)?;
        let effect = clip.effect_with(EFFECT_PREFIX)?;
        let specs = self
            .effects
            .get(effect.name())
            .map(|e| e.params())
            .unwrap_or_else(default_effect_params);
        Some(Applied {
            clip_id: id,
            effect,
            specs,
        })
    }

    /// The active clip's filter.
    pub fn applied_filter(&self) -> Option<Applied> {
        let id = self.active_video_clip()?;
        let (_, clip) = self.project.find_clip(&id)?;
        let effect = clip.effect_with(FILTER_PREFIX)?;
        Some(Applied {
            clip_id: id,
            effect,
            specs: filter_params(),
        })
    }

    // ---- edits -------------------------------------------------------------

    /// A video effect picked (None: the None tile).
    pub fn pick_effect(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        // A pick retries looks that couldn't be fetched before (offline).
        self.packages.retry_failed();
        let target = self.selected_effect_clip();
        let Some(name) = name else {
            if let Some(id) = target
                && self.edit(
                    "Delete Effect",
                    false,
                    None,
                    |p| p.delete_clip(&id).is_some(),
                    cx,
                )
            {
                self.selected.clear();
                self.fx_adjust_open = false;
                self.toast("Effect removed", cx);
            }
            return;
        };
        // Every slider starts at this effect's own default.
        let specs = self
            .effects
            .get(&name)
            .map(|e| e.params())
            .unwrap_or_else(default_effect_params);
        let effect = EffectInstance::new(
            format!("{EFFECT_PREFIX}{name}"),
            &default_parameters(&specs),
        );
        if let Some(id) = target {
            self.edit(
                "Replace Effect",
                false,
                None,
                |p| p.set_clip_effect(&id, EFFECT_PREFIX, Some(effect)),
                cx,
            );
        } else {
            if self.project.duration <= 0 {
                self.toast("Add a video clip first", cx);
                return;
            }
            // A new clip on the effect lane, selected: the next pick
            // replaces it instead of stacking another.
            let at = self.playhead;
            let mut added = None;
            self.edit(
                "Add Effect",
                false,
                None,
                |p| {
                    added = Some(p.add_effect_clip(at, effect));
                    true
                },
                cx,
            );
            self.selected.set_opt(added);
        }
        self.fx_adjust_open = false;
        self.toast(format!("{name} on"), cx);
    }

    /// A filter picked (None: the None tile).
    pub fn pick_filter(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        self.packages.retry_failed();
        let Some(clip) = self.active_video_clip() else {
            self.toast("Add a video clip first", cx);
            return;
        };
        let Some(name) = name else {
            if self.edit(
                "Remove Filter",
                false,
                None,
                |p| p.set_clip_effect(&clip, FILTER_PREFIX, None),
                cx,
            ) {
                self.fx_adjust_open = false;
                self.toast("Filter removed", cx);
            }
            return;
        };
        // A new pick keeps the strength already dialled in.
        let intensity = self
            .applied_filter()
            .filter(|a| a.clip_id == clip)
            .and_then(|a| a.effect.param("intensity"))
            .filter(|v| *v > 0.)
            .unwrap_or(DEFAULT_FILTER_INTENSITY);
        let effect = EffectInstance::new(
            format!("{FILTER_PREFIX}{name}"),
            &[("intensity".into(), intensity)],
        );
        self.edit(
            "Filter",
            false,
            None,
            |p| p.set_clip_effect(&clip, FILTER_PREFIX, Some(effect)),
            cx,
        );
        self.toast(format!("{name} on"), cx);
    }

    /// Sets effect sliders (a drag's release; Reset sets them all).
    pub fn set_fx_params(
        &mut self,
        clip_id: &str,
        effect_id: &str,
        params: Vec<(String, f64)>,
        cx: &mut Context<Self>,
    ) {
        let key = format!("fx:{clip_id}:{effect_id}");
        self.edit(
            "Adjust Effect",
            false,
            Some(key),
            |p| p.set_effect_params(clip_id, effect_id, &params),
            cx,
        );
    }

    // ---- browser views -----------------------------------------------------

    /// Effects › Video effects: the shown category's tiles.
    pub fn render_effects_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let i = self.browser_categories[EFFECTS_TAB].min(EFFECT_TABS.len() - 1);
        let (tab, label) = EFFECT_TABS[i];
        let applied = self.applied_effect();
        let grid = self.effect_grid(tab, applied.as_ref(), cx);
        // Its sliders are in the inspector (the effect clip is selected).
        browser_view(label, grid, None)
    }

    /// Filter: the shown category's looks.
    pub fn render_filters_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let i = self.browser_categories[FILTER_TAB].min(FILTER_TABS.len() - 1);
        let (tab, label) = FILTER_TABS[i];
        let applied = self.applied_filter();
        let grid = if !catalog::has_filters() {
            message("Filters aren't in this build", None).into_any_element()
        } else {
            let current = applied.as_ref().map(|a| a.effect.name().to_string());
            let mut tiles = vec![];
            if tab == "all" {
                tiles.push(Tile {
                    name: None,
                    picture: None,
                    on: current.is_none(),
                });
            }
            tiles.extend(catalog::filters_in(tab).into_iter().map(|f| Tile {
                on: current.as_deref() == Some(f.name.as_str()),
                picture: catalog::filter_preview_key(&f.name).map(str::to_string),
                name: Some(f.name.clone()),
            }));
            self.fx_grid(Picker::Filters, tiles, None, None, cx)
                .into_any_element()
        };
        let adjust = applied
            .filter(|_| self.fx_adjust_open)
            .map(|a| self.adjust_sheet(a, cx));
        browser_view(label, grid, adjust)
    }

    /// The selected effect clip's settings in the inspector: which effect
    /// it is (Replace sends you to the browser's Effects grid, where a pick
    /// replaces it) and its Adjust sliders.
    pub fn render_effect_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(a) = self.applied_effect() else {
            return div()
                .p(px(16.))
                .text_size(px(theme::XS))
                .text_color(theme::text_tertiary())
                .child("No effect on this clip. Pick one in Effects on the left.")
                .into_any_element();
        };
        let name = a.effect.name().to_string();
        let still = self
            .effects
            .get(&name)
            .and_then(|e| e.preview_key.clone())
            .and_then(|k| {
                self.want_picture(&k, false);
                self.stills.get(&k).cloned().flatten()
            });
        let thumb = div()
            .relative()
            .flex_none()
            .size(px(44.))
            .rounded(px(6.))
            .overflow_hidden()
            .bg(theme::button_bg())
            .border_1()
            .border_color(rgba(0xffffff12))
            .flex()
            .items_center()
            .justify_center()
            .map(|d| match still {
                Some(s) => d.child(
                    img(s)
                        .object_fit(ObjectFit::Cover)
                        .absolute()
                        .inset_0()
                        .size_full(),
                ),
                None => d.child(Icon::Sparkle4.el(18., theme::text_tertiary())),
            });
        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .px(px(14.))
            .py(px(12.))
            .border_b_1()
            .border_color(theme::card_border())
            .child(thumb)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(theme::SM))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text())
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(theme::XS))
                            .text_color(theme::text_tertiary())
                            .child("Video effect"),
                    ),
            )
            .child(
                pill_button("fx-replace", "Replace", false)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|ed, _, _, cx| {
                        // The browser's Video effects grid: a pick there
                        // replaces this clip's effect.
                        ed.effects.refresh_stale();
                        ed.browser_tab = EFFECTS_TAB;
                        ed.browser_views[EFFECTS_TAB] = 0;
                        ed.fx_adjust_open = false;
                        cx.notify();
                    })),
            );
        div()
            .flex()
            .flex_col()
            .child(header)
            .child(
                self.render_fx_sliders(&a, SliderPlace::Inspector, cx)
                    .pt(px(8.)),
            )
            .into_any_element()
    }

    /// An Effects tab's grid: its fetched page(s), None first on All.
    fn effect_grid(
        &self,
        tab: &'static str,
        applied: Option<&Applied>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page = self.effects.page(tab);
        let names = page.map_or(&[][..], |p| p.names.as_slice());
        let retry = move |ed: &mut Editor, cx: &mut Context<Editor>| {
            ed.effects.reset(tab);
            ed.load_effect_page(tab, cx);
        };
        if names.is_empty() {
            return match page {
                Some(p) if p.offline => message(
                    "Couldn't load it. Check your connection and try again.",
                    Some(
                        pill_button("fx-retry", "Retry", false)
                            .on_click(cx.listener(move |ed, _, _, cx| retry(ed, cx)))
                            .into_any_element(),
                    ),
                ),
                Some(p) if p.done => message("Nothing here yet", None),
                _ => message("Loading…", None),
            }
            .into_any_element();
        }
        let current = applied.map(|a| a.effect.name().to_string());
        let mut tiles = vec![];
        if tab == "all" {
            tiles.push(Tile {
                name: None,
                picture: None,
                on: current.is_none(),
            });
        }
        tiles.extend(names.iter().map(|n| Tile {
            on: current.as_deref() == Some(n.as_str()),
            picture: self.effects.get(n).and_then(|e| e.preview_key.clone()),
            name: Some(n.clone()),
        }));
        let footer = page.and_then(|p| {
            if p.loading {
                Some(footnote("Loading…").into_any_element())
            } else if p.offline {
                Some(
                    footnote("Offline: effects seen before")
                        .child(
                            div()
                                .id("fx-retry-small")
                                .cursor_pointer()
                                .text_color(theme::text())
                                .hover(|s| s.opacity(0.8))
                                .on_click(cx.listener(move |ed, _, _, cx| retry(ed, cx)))
                                .child("Retry"),
                        )
                        .into_any_element(),
                )
            } else {
                None
            }
        });
        self.fx_grid(
            Picker::Effects,
            tiles,
            footer,
            Some(self.fx_scroll.clone()),
            cx,
        )
        .into_any_element()
    }

    /// Tiles sized to the panel (as many ≥ 92 px columns as fit).
    fn fx_grid(
        &self,
        picker: Picker,
        tiles: Vec<Tile>,
        footer: Option<AnyElement>,
        scroll: Option<ScrollHandle>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = picker.id();
        let width = self
            .measured(key)
            .map_or(0., |b| f32::from(b.size.width) - GRID_INSET * 2.);
        let columns = ((width + TILE_GAP) / (TILE_MIN + TILE_GAP)).floor().max(1.);
        let tile = if width > 0. {
            ((width - TILE_GAP * (columns - 1.)) / columns).floor()
        } else {
            TILE_MIN
        };
        div()
            .id(key)
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .when_some(scroll, |d, s| d.track_scroll(&s))
            .child(self.measure(key))
            .child(
                div()
                    .p(px(GRID_INSET))
                    .flex()
                    .flex_wrap()
                    .gap(px(TILE_GAP))
                    .children(tiles.into_iter().map(|t| self.fx_tile(picker, t, tile, cx))),
            )
            .children(footer)
    }

    fn fx_tile(&self, picker: Picker, t: Tile, size: f32, cx: &mut Context<Self>) -> AnyElement {
        let id = SharedString::from(format!(
            "{}:{}",
            picker.id(),
            t.name.as_deref().unwrap_or("")
        ));
        let hovered = self.fx_hover.as_ref() == Some(&id.to_string());
        let still = t.picture.as_deref().and_then(|k| {
            self.want_picture(k, false);
            self.stills.get(k).cloned().flatten()
        });
        let anim = t.picture.as_deref().filter(|_| hovered).and_then(|k| {
            self.want_picture(k, true);
            self.anim(k)
        });
        let on = t.on;
        let none = t.name.is_none();
        // Only a filter's sliders open over the grid; an effect's are in
        // the inspector.
        let adjustable = on && !none && picker == Picker::Filters;
        let name = t.name.clone();
        let hover_id = id.to_string();

        let thumb = div()
            .relative()
            .size(px(size))
            .rounded(px(6.))
            .overflow_hidden()
            .bg(theme::button_bg())
            .border(px(1.5))
            .border_color(if on { theme::text() } else { rgba(0xffffff12) })
            .when(!on, |d| {
                d.group_hover("fx-tile", |s| s.border_color(rgba(0xffffff59)))
            })
            .map(|d| match (anim, still) {
                (Some(a), _) => d.child(
                    img(a)
                        .object_fit(ObjectFit::Cover)
                        .id(SharedString::from(format!("{id}:anim")))
                        .absolute()
                        .inset_0()
                        .size_full(),
                ),
                (None, Some(s)) => d.child(
                    img(s)
                        .object_fit(ObjectFit::Cover)
                        .absolute()
                        .inset_0()
                        .size_full(),
                ),
                _ => d,
            })
            .when(none, |d| {
                d.flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::Ban.el(30., rgba(0xffffff99)))
            })
            .when(adjustable, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(2.))
                        .bg(rgba(0x00000080))
                        .child(Icon::Options.el(18., theme::text()))
                        .child(
                            div()
                                .text_size(px(theme::XS))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::text())
                                .child(crate::i18n::t("Adjust")),
                        ),
                )
            })
            .when(on && none, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(4.))
                        .right(px(4.))
                        .size(px(15.))
                        .rounded(px(4.))
                        .bg(theme::text())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::Checkmark.el(11., theme::app_bg())),
                )
            })
            .when(!on && !none, |d| {
                d.child(
                    div()
                        .absolute()
                        .right(px(5.))
                        .bottom(px(5.))
                        .size(px(20.))
                        .rounded_full()
                        .bg(rgba(0x00000099))
                        .flex()
                        .items_center()
                        .justify_center()
                        .invisible()
                        .group_hover("fx-tile", |s| s.visible())
                        .child(Icon::Add.el(14., theme::text())),
                )
            });

        div()
            .id(id)
            .w(px(size))
            .flex()
            .flex_col()
            .gap(px(5.))
            .cursor_pointer()
            .group("fx-tile")
            .on_hover(cx.listener(move |ed, over: &bool, _, cx| {
                if *over {
                    ed.fx_hover = Some(hover_id.clone());
                } else if ed.fx_hover.as_ref() == Some(&hover_id) {
                    ed.fx_hover = None;
                }
                cx.notify();
            }))
            .on_click(cx.listener(move |ed, _, _, cx| {
                if adjustable {
                    // The picked tile opens its sliders.
                    ed.fx_adjust_open = true;
                    cx.notify();
                    return;
                }
                if on {
                    // Already on: its settings are in the inspector.
                    return;
                }
                match picker {
                    Picker::Filters => ed.pick_filter(name.clone(), cx),
                    _ => ed.pick_effect(name.clone(), cx),
                }
            }))
            .child(thumb)
            .child(
                div()
                    .truncate()
                    .text_size(px(theme::XS))
                    .text_color(if on {
                        theme::text()
                    } else {
                        theme::text_secondary()
                    })
                    .child(t.name.unwrap_or_else(|| "None".into())),
            )
            .into_any_element()
    }

    /// The Adjust card over the browser grid: Reset, close, a slider per
    /// parameter.
    fn adjust_sheet(&self, a: Applied, cx: &mut Context<Self>) -> AnyElement {
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .bg(theme::sheet())
            .border_t_1()
            .border_color(theme::chip_border())
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .h(px(36.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(theme::SM))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text())
                            .child(t("Adjust")),
                    )
                    .child(
                        icon_button("fx-adjust-close", Icon::Close, "Close")
                            .size(14.)
                            .on_click(cx.listener(|ed, _, _, cx| {
                                ed.fx_adjust_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(self.render_fx_sliders(&a, SliderPlace::Browser, cx))
            .into_any_element()
    }

    /// Reset, then one 0–100 slider per parameter (live while dragged,
    /// one undo step on release).
    pub fn render_fx_sliders(
        &self,
        a: &Applied,
        place: SliderPlace,
        cx: &mut Context<Self>,
    ) -> Div {
        let defaults: Vec<(String, f64)> = a.specs.iter().map(|p| (p.id.clone(), p.def)).collect();
        let changed = a
            .specs
            .iter()
            .any(|p| a.effect.param(&p.id).is_some_and(|v| v != p.def));
        let (clip, effect) = (a.clip_id.clone(), a.effect.id.clone());
        let reset = div()
            .id(("fx-reset", place as usize))
            .flex()
            .items_center()
            .gap_1()
            .h(px(26.))
            .text_size(px(theme::XS))
            .text_color(if changed {
                theme::text_secondary()
            } else {
                theme::text_tertiary()
            })
            .when(changed, |d| {
                d.cursor_pointer()
                    .hover(|s| s.text_color(theme::text()))
                    .on_click(cx.listener(move |ed, _, _, cx| {
                        ed.set_fx_params(&clip, &effect, defaults.clone(), cx)
                    }))
            })
            .child(Icon::Reload.el(13., theme::text_secondary()))
            .child(t("Reset"));

        div()
            .px(px(14.))
            .pb(px(14.))
            .flex()
            .flex_col()
            .child(reset)
            .when(a.specs.is_empty(), |d| {
                d.child(
                    div()
                        .py_2()
                        .text_size(px(theme::XS))
                        .text_color(theme::text_tertiary())
                        .child(t("This effect has nothing to adjust.")),
                )
            })
            .children(a.specs.iter().map(|spec| {
                let key = slider_key(place, &spec.id);
                let stored = a.effect.param(&spec.id).unwrap_or(spec.def);
                let (shown, dragging) = match self.live_value {
                    Some((k, v)) if k == key => (v, true),
                    _ => (stored, false),
                };
                let (clip, effect, param) =
                    (a.clip_id.clone(), a.effect.id.clone(), spec.id.clone());
                let apply: SliderFn = Rc::new(move |ed, f, done, cx| {
                    let v = (f as f64 * 100.).round();
                    if done {
                        ed.live_value = None;
                        ed.live_fx = None;
                        ed.set_fx_params(&clip, &effect, vec![(param.clone(), v)], cx);
                    } else {
                        ed.live_value = Some((key, v));
                        // The preview follows the slider while it's held.
                        ed.live_fx = Some(LiveParam {
                            clip_id: clip.clone(),
                            effect_id: effect.clone(),
                            param: param.clone(),
                            value: v,
                        });
                        cx.notify();
                    }
                });
                panel_row(spec.label.clone())
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
                            .child(crate::ui::widgets::slider_track(
                                (shown / 100.) as f32,
                                dragging,
                            )),
                    )
                    .child(value_box(format!("{}", shown.round() as i64)))
            }))
    }
}

/// A browser view: the category's heading, the grid, the Adjust card.
fn browser_view(label: &str, grid: AnyElement, adjust: Option<AnyElement>) -> AnyElement {
    div()
        .relative()
        .size_full()
        .flex()
        .flex_col()
        .child(
            div()
                .flex_none()
                .h(px(36.))
                .px(px(GRID_INSET))
                .flex()
                .items_end()
                .text_size(px(theme::SM))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text())
                .child(crate::i18n::ts(label.to_string())),
        )
        .child(grid)
        .children(adjust)
        .into_any_element()
}

/// A centred note in place of the grid (loading, offline…).
fn message(text: &str, action: Option<AnyElement>) -> Div {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .px(px(16.))
        .child(
            div()
                .text_size(px(theme::XS))
                .text_color(theme::text_tertiary())
                .text_center()
                .child(crate::i18n::ts(text.to_string())),
        )
        .children(action)
}

/// A line under the grid ("Loading…").
fn footnote(text: &str) -> Div {
    div()
        .px(px(GRID_INSET))
        .pb(px(12.))
        .flex()
        .justify_center()
        .gap_2()
        .text_size(px(theme::XS))
        .text_color(theme::text_tertiary())
        .child(crate::i18n::ts(text.to_string()))
}
