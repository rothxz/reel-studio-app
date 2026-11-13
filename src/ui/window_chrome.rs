//! The window's title-bar parts where the system draws none.
//!
//! macOS draws its traffic lights over our header and has the menu bar, so
//! nothing here shows there. On Windows the title bar is ours (it's hidden
//! for the header): we draw minimize / maximize / close and mark the
//! header's empty space as the caption, so moving, snapping and double-click
//! maximize are the system's own. On Linux the window asks for client-side
//! decorations (main.rs: GNOME on Wayland draws none of its own), so we
//! draw the same buttons, move the window from the empty space and add
//! resize edges; only X11 without a compositor keeps the system's. Neither
//! has a menu bar, so a ☰ button opens the app menu (built from the same
//! menus macOS shows).

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    App, Context, CursorStyle, Decorations, Div, ElementId, MouseButton, OwnedMenuItem, ResizeEdge,
    Stateful, Window, WindowControlArea, div, prelude::*, px, rgb,
};

use crate::editor::{Editor, PopupItem};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets::tooltip;

/// Whether the app draws the window's buttons.
pub fn draws_controls(window: &Window) -> bool {
    if cfg!(target_os = "windows") {
        !window.is_fullscreen()
    } else if cfg!(target_os = "macos") {
        false
    } else {
        matches!(window.window_decorations(), Decorations::Client { .. }) && !window.is_fullscreen()
    }
}

/// Whether the app menu needs a button (no system menu bar).
pub fn draws_menu() -> bool {
    !cfg!(target_os = "macos")
}

/// A press in a drag area, waiting for the pointer to move (Linux: the
/// move is handed to the compositor then, so a double-click still zooms).
static ARMED: AtomicBool = AtomicBool::new(false);

/// Empty title-bar space: moves the window, double-click zooms it (macOS:
/// the system's title-bar action), right-click shows the window menu
/// (Linux).
pub fn drag_area(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .h_full()
        .window_control_area(WindowControlArea::Drag)
        .on_mouse_down(MouseButton::Left, |e, window, _| {
            if cfg!(target_os = "linux") {
                if e.click_count >= 2 {
                    ARMED.store(false, Ordering::Relaxed);
                    window.zoom_window();
                } else {
                    ARMED.store(true, Ordering::Relaxed);
                }
            }
        })
        .on_mouse_up(MouseButton::Left, |_, _, _| {
            ARMED.store(false, Ordering::Relaxed)
        })
        .on_mouse_move(|e, window, _| {
            if e.dragging() && ARMED.swap(false, Ordering::Relaxed) {
                window.start_window_move();
            }
        })
        .on_mouse_down(MouseButton::Right, |e, window, _| {
            if cfg!(target_os = "linux") {
                window.show_window_menu(e.position);
            }
        })
        .on_click(|e, window, _| {
            if cfg!(target_os = "macos") && e.click_count() == 2 {
                window.titlebar_double_click();
            }
        })
}

/// A title-bar button: 46 px wide, the full bar high.
fn control(id: &'static str, area: WindowControlArea, danger: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .w(px(46.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .window_control_area(area)
        .when(danger, |d| {
            d.hover(|s| s.bg(rgb(0xc42b1c)).text_color(rgb(0xffffff)))
        })
        .when(!danger, |d| d.hover(|s| s.bg(theme::button_hover())))
        .text_color(theme::text_secondary())
}

/// A line glyph (the minimize bar, the maximize box's edges).
fn line(w: f32, h: f32) -> Div {
    div().w(px(w)).h(px(h)).bg(gpui::transparent_black())
}

impl Editor {
    /// Minimize, maximize / restore, close: at the right end of the title
    /// bar. On Windows the system handles their clicks (they're its caption
    /// buttons); elsewhere they call the window. Closing saves first.
    pub fn render_window_controls(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let maximized = window.is_maximized();
        let system = cfg!(target_os = "windows");
        let glyph = theme::text_secondary();
        let min = control("win-min", WindowControlArea::Min, false)
            .child(line(10., 1.).bg(glyph))
            .when(!system, |d| {
                d.on_click(|_, window, _| window.minimize_window())
            });
        let max_glyph = if maximized {
            // Restore: a box with another behind it.
            div()
                .relative()
                .size(px(10.))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left(px(2.))
                        .size(px(8.))
                        .border_1()
                        .border_color(glyph),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(2.))
                        .left_0()
                        .size(px(8.))
                        .border_1()
                        .border_color(glyph)
                        .bg(theme::app_bg()),
                )
        } else {
            line(10., 10.).border_1().border_color(glyph)
        };
        let max = control("win-max", WindowControlArea::Max, false)
            .child(max_glyph)
            .when(!system, |d| d.on_click(|_, window, _| window.zoom_window()));
        let close = control("win-close", WindowControlArea::Close, true)
            .child(Icon::Close.el(16., glyph))
            .when(!system, |d| {
                d.on_click(cx.listener(|ed, _, window, cx| {
                    ed.save_now(cx);
                    window.remove_window();
                }))
            });
        div()
            .flex_none()
            .h_full()
            .flex()
            .child(min)
            .child(max)
            .child(close)
    }

    /// The ☰ button opening the app menu.
    pub fn render_menu_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let bar = || {
            div()
                .w(px(14.))
                .h(px(1.5))
                .rounded(px(1.))
                .bg(theme::text())
        };
        div()
            .id("app-menu")
            .flex_none()
            .size(px(28.))
            .rounded(px(6.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(3.5))
            .cursor_pointer()
            .hover(|s| s.bg(theme::button_hover()))
            .tooltip(tooltip("Menu", None))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|ed, e: &gpui::ClickEvent, _, cx| {
                ed.open_app_menu(e.position(), cx);
            }))
            .child(bar())
            .child(bar())
            .child(bar())
    }

    /// The menus by name; picking one opens its items in the same place.
    fn open_app_menu(&mut self, at: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let menus = cx.get_menus().unwrap_or_default();
        let items = menus
            .into_iter()
            .map(|menu| {
                let name = menu.name.clone();
                PopupItem::new(
                    format!("{name}  ›"),
                    move |ed: &mut Editor, window, cx| {
                        let items = menu_items(&menu.items, window);
                        ed.open_popup(at, items, cx);
                    },
                )
            })
            .collect();
        self.open_popup(at, items, cx);
    }
}

/// A menu's items as popup rows: each runs its action, with its shortcut.
fn menu_items(items: &[OwnedMenuItem], window: &Window) -> Vec<PopupItem> {
    let mut rows = vec![];
    for item in items {
        match item {
            OwnedMenuItem::Separator => rows.push(PopupItem::separator()),
            OwnedMenuItem::Action { name, action, .. } => {
                let hint = window
                    .highest_precedence_binding_for_action(action.as_ref())
                    .map(|b| {
                        b.keystrokes()
                            .iter()
                            .map(|k| key_text(k.modifiers(), k.key()))
                            .collect::<Vec<_>>()
                            .join(" ")
                    });
                let action = action.boxed_clone();
                let mut row = PopupItem::new(name.clone(), move |_, window, cx| {
                    window.dispatch_action(action.boxed_clone(), cx);
                });
                row.hint = hint.map(Into::into);
                rows.push(row);
            }
            OwnedMenuItem::Submenu(menu) => {
                // One level: its items, set off by separators.
                rows.push(PopupItem::separator());
                rows.extend(menu_items(&menu.items, window));
                rows.push(PopupItem::separator());
            }
            OwnedMenuItem::SystemMenu(_) => {}
        }
    }
    rows
}

/// "Ctrl+Shift+S" for a keystroke.
fn key_text(m: &gpui::Modifiers, key: &str) -> String {
    let mut parts: Vec<String> = vec![];
    if m.control {
        parts.push("Ctrl".into());
    }
    if m.alt {
        parts.push("Alt".into());
    }
    if m.shift {
        parts.push("Shift".into());
    }
    if m.platform {
        parts.push(
            if cfg!(target_os = "windows") {
                "Win"
            } else {
                "Super"
            }
            .into(),
        );
    }
    let key = match key {
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "space" => "Space".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" => "Del".to_string(),
        "escape" => "Esc".to_string(),
        "enter" => "Enter".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "tab" => "Tab".to_string(),
        other => {
            let mut c = other.chars();
            match c.next() {
                Some(first) => first.to_uppercase().chain(c).collect(),
                None => String::new(),
            }
        }
    };
    parts.push(key);
    parts.join("+")
}

/// Resize edges around the window when it decorates itself on Linux (the
/// compositor resizes it from there), none when maximized or tiled.
pub fn resize_edges(window: &Window) -> Option<Div> {
    if !cfg!(target_os = "linux") || window.is_maximized() || window.is_fullscreen() {
        return None;
    }
    let Decorations::Client { tiling } = window.window_decorations() else {
        return None;
    };
    const T: f32 = 5.;
    let edge = |id: &'static str, edge: ResizeEdge, cursor: CursorStyle| {
        div().id(id).absolute().cursor(cursor).on_mouse_down(
            MouseButton::Left,
            move |_, window: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                window.start_window_resize(edge);
            },
        )
    };
    let mut layer = div().absolute().inset_0().size_full();
    // The layer itself takes no pointer: only its edge strips do.
    if !tiling.top {
        layer = layer.child(
            edge("rz-t", ResizeEdge::Top, CursorStyle::ResizeUpDown)
                .top_0()
                .left(px(T))
                .right(px(T))
                .h(px(T)),
        );
    }
    if !tiling.bottom {
        layer = layer.child(
            edge("rz-b", ResizeEdge::Bottom, CursorStyle::ResizeUpDown)
                .bottom_0()
                .left(px(T))
                .right(px(T))
                .h(px(T)),
        );
    }
    if !tiling.left {
        layer = layer.child(
            edge("rz-l", ResizeEdge::Left, CursorStyle::ResizeLeftRight)
                .left_0()
                .top(px(T))
                .bottom(px(T))
                .w(px(T)),
        );
    }
    if !tiling.right {
        layer = layer.child(
            edge("rz-r", ResizeEdge::Right, CursorStyle::ResizeLeftRight)
                .right_0()
                .top(px(T))
                .bottom(px(T))
                .w(px(T)),
        );
    }
    if !tiling.top && !tiling.left {
        layer = layer.child(
            edge(
                "rz-tl",
                ResizeEdge::TopLeft,
                CursorStyle::ResizeUpLeftDownRight,
            )
            .top_0()
            .left_0()
            .size(px(T)),
        );
    }
    if !tiling.top && !tiling.right {
        layer = layer.child(
            edge(
                "rz-tr",
                ResizeEdge::TopRight,
                CursorStyle::ResizeUpRightDownLeft,
            )
            .top_0()
            .right_0()
            .size(px(T)),
        );
    }
    if !tiling.bottom && !tiling.left {
        layer = layer.child(
            edge(
                "rz-bl",
                ResizeEdge::BottomLeft,
                CursorStyle::ResizeUpRightDownLeft,
            )
            .bottom_0()
            .left_0()
            .size(px(T)),
        );
    }
    if !tiling.bottom && !tiling.right {
        layer = layer.child(
            edge(
                "rz-br",
                ResizeEdge::BottomRight,
                CursorStyle::ResizeUpLeftDownRight,
            )
            .bottom_0()
            .right_0()
            .size(px(T)),
        );
    }
    Some(layer)
}
