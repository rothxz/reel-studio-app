//! Arrange layers (the toolbar's Layers), docked in the inspector under
//! the clip's tabs: what's on screen at the playhead (when it opened) that
//! can be stacked, front-most on the left. Front / Back send the picked
//! layer all the way; press-drag a tile to put it anywhere. A click picks
//! the layer (and selects its clip). One undo step per change.

use gpui::{
    AnyElement, Context, FontWeight, MouseButton, MouseDownEvent, SharedString, div, img,
    prelude::*, px,
};

use crate::clip_edits::{LayerItem, LayerKey};
use crate::editor::{Drag, Editor};
use crate::model::{Clip, Micros};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets::pill_button;

const TILE: f32 = 60.;
const GAP: f32 = 12.;
const STEP: f32 = TILE + GAP;
/// A press that moves less than this is a click.
const DRAG_SLOP: f32 = 4.;

/// The open panel: the time it lists the layers at.
pub struct LayersPanel {
    pub time: Micros,
}

/// A tile held down: which (its place when pressed) and where the
/// pointer was.
pub struct LayerDrag {
    from: usize,
    origin_x: f32,
    pub(crate) moved: bool,
}

impl LayerDrag {
    /// The place the tile would drop at, and how far it has been pulled.
    fn target(&self, pointer_x: f32, count: usize) -> (usize, f32) {
        let dx = pointer_x - self.origin_x;
        let at = (self.from as f32 + dx / STEP).round();
        (at.clamp(0., count.saturating_sub(1) as f32) as usize, dx)
    }
}

impl Editor {
    /// Opens the panel for what's on screen at the playhead.
    pub fn open_layers_panel(&mut self, cx: &mut Context<Self>) {
        self.layers_panel = Some(LayersPanel {
            time: self.playhead,
        });
        cx.notify();
    }

    fn layers_now(&self) -> Vec<LayerItem> {
        self.layers_panel
            .as_ref()
            .map(|p| self.project.arrangeable_layers_at(p.time))
            .unwrap_or_default()
    }

    /// The picked layer: the selected clip's, else its lane's.
    fn active_layer(&self, layers: &[LayerItem]) -> Option<usize> {
        let id = self.selected.primary()?;
        let lane = self.project.track_of(id).map(|t| t.id.clone());
        layers
            .iter()
            .position(|l| l.key == LayerKey::Clip(id.to_owned()))
            .or_else(|| {
                layers.iter().position(|l| {
                    Some(&l.key) == lane.as_ref().map(|t| LayerKey::Track(t.clone())).as_ref()
                })
            })
    }

    fn arrange_layers(&mut self, order: Vec<LayerKey>, cx: &mut Context<Self>) {
        self.edit(
            "Arrange Layers",
            false,
            None,
            |p| p.reorder_layers(&order),
            cx,
        );
    }

    /// Front / Back: the picked layer all the way.
    fn send_layer(&mut self, front: bool, cx: &mut Context<Self>) {
        let layers = self.layers_now();
        let Some(i) = self.active_layer(&layers) else {
            return;
        };
        let mut keys: Vec<LayerKey> = layers.into_iter().map(|l| l.key).collect();
        let key = keys.remove(i);
        if front {
            keys.insert(0, key);
        } else {
            keys.push(key);
        }
        self.arrange_layers(keys, cx);
    }

    /// The release of a held tile: a click picks it, a drag re-stacks.
    pub fn commit_layer_drag(&mut self, d: LayerDrag, cx: &mut Context<Self>) {
        let layers = self.layers_now();
        if !d.moved {
            if let Some(l) = layers.get(d.from) {
                self.selected.set(l.clip_id.clone());
            }
            cx.notify();
            return;
        }
        let (to, _) = d.target(f32::from(self.pointer.x), layers.len());
        if to == d.from {
            return;
        }
        let mut keys: Vec<LayerKey> = layers.into_iter().map(|l| l.key).collect();
        let key = keys.remove(d.from);
        keys.insert(to, key);
        self.arrange_layers(keys, cx);
    }

    pub fn update_layer_drag(&mut self, cx: &mut Context<Self>) {
        let x = f32::from(self.pointer.x);
        if let Some(Drag::Layer(d)) = self.drag.as_mut()
            && (x - d.origin_x).abs() > DRAG_SLOP
        {
            d.moved = true;
        }
        cx.notify();
    }

    pub fn render_layers_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let layers = self.layers_now();
        let active = self.active_layer(&layers);
        let count = layers.len();
        let is_front = active.is_none_or(|i| i == 0);
        let is_back = active.is_none_or(|i| i + 1 == count);
        let drag = match &self.drag {
            Some(Drag::Layer(d)) if d.moved => Some(d.as_ref()),
            _ => None,
        };
        // Where each tile sits now: the held one follows the pointer, the
        // rest make room where it would drop.
        let (target, pulled) = drag.map_or((None, 0.), |d| {
            let (to, dx) = d.target(f32::from(self.pointer.x), count);
            (Some((d.from, to)), dx)
        });
        let slot_of = |i: usize| -> f32 {
            match target {
                Some((from, _)) if i == from => from as f32 * STEP + pulled,
                Some((from, to)) if from < to && i > from && i <= to => (i - 1) as f32 * STEP,
                Some((from, to)) if to < from && i >= to && i < from => (i + 1) as f32 * STEP,
                _ => i as f32 * STEP,
            }
        };

        let tiles = layers.iter().enumerate().map(|(i, layer)| {
            let held = target.is_some_and(|(from, _)| from == i);
            let selected = active == Some(i);
            div()
                .id(("layer-tile", i))
                .absolute()
                .top_0()
                .left(px(slot_of(i)))
                .size(px(TILE))
                .p(px(4.))
                .rounded(px(18.))
                .border_2()
                .border_color(if selected {
                    theme::text().into()
                } else {
                    gpui::transparent_black()
                })
                .cursor_pointer()
                .when(held, |d| d.opacity(0.92).shadow_lg())
                .child(self.layer_artwork(layer))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |ed, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        ed.drag = Some(Drag::Layer(Box::new(LayerDrag {
                            from: i,
                            origin_x: f32::from(e.position.x),
                            moved: false,
                        })));
                        cx.notify();
                    }),
                )
        });
        // The held tile on top of the others.
        let mut tiles: Vec<_> = tiles.collect();
        if let Some((from, _)) = target {
            let held = tiles.remove(from);
            tiles.push(held);
        }

        let order_button =
            |id: &'static str, icon: Icon, label: &'static str, disabled: bool, front: bool| {
                div()
                    .id(id)
                    .flex_none()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .size(px(32.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(gpui::rgba(0xffffff1f))
                            .when(disabled, |d| d.opacity(0.28))
                            .child(icon.el(18., theme::text())),
                    )
                    .child(
                        div()
                            .text_size(px(theme::SM))
                            .text_color(if disabled {
                                theme::text_disabled()
                            } else {
                                theme::text()
                            })
                            .child(label),
                    )
                    .when(!disabled, |d| {
                        d.cursor_pointer()
                            .on_click(cx.listener(move |ed, _, _, cx| ed.send_layer(front, cx)))
                    })
            };

        let row = if count == 0 {
            div()
                .flex_1()
                .h(px(TILE))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(theme::SM))
                .text_color(theme::text_tertiary())
                .child("Nothing to arrange here")
                .into_any_element()
        } else {
            div()
                .id("layer-row")
                .flex_1()
                .min_w_0()
                .overflow_x_scroll()
                .child(
                    div()
                        .relative()
                        .mx(px(4.))
                        .w(px(count as f32 * STEP - GAP))
                        .h(px(TILE))
                        .children(tiles),
                )
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .h(px(40.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(theme::card_border())
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(theme::SM))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Arrange layers"),
                    )
                    .child(
                        pill_button("layers-done", "Done", false)
                            .min_w(px(0.))
                            .h(px(26.))
                            .px(px(12.))
                            .on_click(cx.listener(|ed, _, _, cx| {
                                ed.layers_panel = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .px(px(14.))
                    .pt(px(16.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(order_button(
                        "layers-front",
                        Icon::SkipBackOutline,
                        "Front",
                        is_front,
                        true,
                    ))
                    .child(row)
                    .child(order_button(
                        "layers-back",
                        Icon::SkipForwardOutline,
                        "Back",
                        is_back,
                        false,
                    )),
            )
            .child(
                div()
                    .mt(px(16.))
                    .flex()
                    .justify_center()
                    .text_size(px(theme::SM))
                    .text_color(theme::text_secondary())
                    .child("Press and drag to arrange layers"),
            )
            .into_any_element()
    }

    /// A tile's picture: a frame for a video / photo, the words for text,
    /// an icon otherwise; and what it is, top left.
    fn layer_artwork(&self, layer: &LayerItem) -> AnyElement {
        let clip: Option<&Clip> = self.project.find_clip(&layer.clip_id).map(|(_, c)| c);
        let label = match clip.map(|c| c.kind.as_str()) {
            Some("text") => "Text",
            Some("sticker") => "Sticker",
            Some("drawing") => "Draw",
            _ => "Overlay",
        };
        let picture = clip
            .filter(|c| matches!(c.kind.as_str(), "video" | "image"))
            .and_then(|c| {
                let src = c.source.as_deref()?;
                let want = c.source_start.unwrap_or(0);
                // The nearest filmstrip tile the timeline already has.
                self.tiles
                    .iter()
                    .filter(|((s, _), _)| s == src)
                    .min_by_key(|((_, t), _)| (t - want).abs())
                    .map(|(_, image)| image.clone())
                    .or_else(|| self.thumbs.get(src).cloned())
            });
        let inner = div()
            .relative()
            .size_full()
            .rounded(px(12.))
            .overflow_hidden()
            .bg(theme::panel())
            .flex()
            .items_center()
            .justify_center();
        let inner = match (picture, clip) {
            (Some(image), _) => inner.child(img(image).absolute().inset_0().size_full()),
            (None, Some(c)) if c.kind == "text" => inner.p(px(4.)).child(
                div()
                    .text_size(px(10.))
                    .line_height(px(12.))
                    .text_color(theme::text())
                    .overflow_hidden()
                    .child(SharedString::from(c.text.clone().unwrap_or_default())),
            ),
            (None, c) => inner.child(
                match c.map(|c| c.kind.as_str()) {
                    Some("drawing") => Icon::Brush,
                    Some("sticker") => Icon::Sticker,
                    _ => Icon::Image,
                }
                .el(18., theme::text_secondary()),
            ),
        };
        inner
            .child(
                div()
                    .absolute()
                    .top(px(2.))
                    .left(px(2.))
                    .max_w(gpui::relative(0.9))
                    .px(px(3.))
                    .rounded(px(5.))
                    .bg(gpui::rgba(0x00000085))
                    .text_size(px(9.))
                    .text_color(theme::text())
                    .truncate()
                    .child(label),
            )
            .into_any_element()
    }
}
