//! A native text field (GPUI has none built in): one line (search boxes)
//! or several, soft-wrapped (a text clip's words). It owns its text,
//! caret and selection, takes typing through the platform input handler
//! (so IME composition and the character palette work), and tells its
//! owner what changed through `InputEvent`s.
//!
//! The editor's single-key shortcuts (Space, Q, W, ⌫…) are bound in the
//! `Editor && !TextInput` context, so they stand aside while a field has
//! the keyboard.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions,
    div, fill, point, prelude::*, px, relative, size,
};

use crate::theme;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        DocStart,
        DocEnd,
        WordLeft,
        WordRight,
        Enter,
        Escape,
        Paste,
        Cut,
        Copy,
        ShowCharacterPalette,
    ]
);

pub const CONTEXT: &str = "TextInput";

/// The field's key bindings (registered once, app-wide).
pub fn bindings() -> Vec<KeyBinding> {
    let c = Some(CONTEXT);
    vec![
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("up", Up, c),
        KeyBinding::new("down", Down, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("secondary-a", SelectAll, c),
        KeyBinding::new("home", Home, c),
        KeyBinding::new("end", End, c),
        KeyBinding::new("cmd-left", Home, c),
        KeyBinding::new("cmd-right", End, c),
        KeyBinding::new("shift-home", SelectHome, c),
        KeyBinding::new("shift-end", SelectEnd, c),
        KeyBinding::new("cmd-shift-left", SelectHome, c),
        KeyBinding::new("cmd-shift-right", SelectEnd, c),
        KeyBinding::new("cmd-up", DocStart, c),
        KeyBinding::new("cmd-down", DocEnd, c),
        KeyBinding::new("alt-left", WordLeft, c),
        KeyBinding::new("alt-right", WordRight, c),
        KeyBinding::new("enter", Enter, c),
        KeyBinding::new("escape", Escape, c),
        KeyBinding::new("secondary-v", Paste, c),
        KeyBinding::new("secondary-c", Copy, c),
        KeyBinding::new("secondary-x", Cut, c),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, c),
    ]
}

/// What a field tells its owner.
#[derive(Clone, Debug)]
pub enum InputEvent {
    /// The text changed (typed, pasted, cut).
    Changed(String),
    /// Enter in a one-line field.
    Submit(String),
    /// The field lost the keyboard (Esc, or a click elsewhere).
    Blur,
}

pub struct TextInput {
    focus: FocusHandle,
    content: String,
    placeholder: SharedString,
    multiline: bool,
    max_len: usize,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    /// The last layout: each hard line's start offset and its shaping.
    lines: Vec<(usize, WrappedLine)>,
    bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    selecting: bool,
    was_focused: bool,
}

impl EventEmitter<InputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextInput {
    pub fn new(
        placeholder: impl Into<SharedString>,
        multiline: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        TextInput {
            focus: cx.focus_handle(),
            content: String::new(),
            placeholder: placeholder.into(),
            multiline,
            max_len: 2000,
            selected: 0..0,
            reversed: false,
            marked: None,
            lines: vec![],
            bounds: None,
            line_height: px(18.),
            selecting: false,
            was_focused: false,
        }
    }

    pub fn max_len(mut self, n: usize) -> Self {
        self.max_len = n;
        self
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// Replaces the text from outside (no event); the caret goes to the end.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.content == text {
            return;
        }
        self.content = text.to_string();
        self.selected = self.content.len()..self.content.len();
        self.reversed = false;
        self.marked = None;
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    /// Selects all of it (typing then replaces it).
    pub fn select_everything(&mut self, cx: &mut Context<Self>) {
        self.selected = 0..self.content.len();
        self.reversed = false;
        cx.notify();
    }

    // ---- editing --------------------------------------------------------

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Changed(self.content.clone()));
        cx.notify();
    }

    fn cursor(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = offset.min(self.content.len());
        self.selected = offset..offset;
        self.reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = offset.min(self.content.len());
        if self.reversed {
            self.selected.start = offset;
        } else {
            self.selected.end = offset;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        cx.notify();
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        previous_boundary(&self.content, offset)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        next_boundary(&self.content, offset)
    }

    fn word_left(&self, offset: usize) -> usize {
        word_left(&self.content, offset)
    }

    fn word_right(&self, offset: usize) -> usize {
        word_right(&self.content, offset)
    }

    fn line_start(&self, offset: usize) -> usize {
        line_start(&self.content, offset)
    }

    fn line_end(&self, offset: usize) -> usize {
        line_end(&self.content, offset)
    }

    /// Replaces `range` (the selection, or what's marked) with `text`.
    fn insert(&mut self, range: Range<usize>, text: &str) {
        let at = splice(&mut self.content, range, text, self.multiline, self.max_len);
        self.selected = at..at;
        self.reversed = false;
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let from = self.previous_boundary(self.cursor());
            self.selected = from..self.selected.end;
        }
        self.insert(self.selected.clone(), "");
        self.changed(cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let to = self.next_boundary(self.cursor());
            self.selected = self.selected.start..to;
        }
        self.insert(self.selected.clone(), "");
        self.changed(cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.move_to(self.previous_boundary(self.cursor()), cx);
        } else {
            self.move_to(self.selected.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.move_to(self.next_boundary(self.cursor()), cx);
        } else {
            self.move_to(self.selected.end, cx);
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor()), cx);
    }

    /// The offset a row up (-1) or down (+1) from the caret, at its x.
    fn vertical(&self, dir: i32) -> usize {
        let at = self.cursor();
        let Some(pos) = self.position_of(at) else {
            return if dir < 0 { 0 } else { self.content.len() };
        };
        let target = point(
            pos.x,
            pos.y + self.line_height * dir as f32 + self.line_height / 2.,
        );
        self.index_at(target)
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.multiline { self.vertical(-1) } else { 0 };
        self.move_to(to, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.multiline {
            self.vertical(1)
        } else {
            self.content.len()
        };
        self.move_to(to, cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.multiline { self.vertical(-1) } else { 0 };
        self.select_to(to, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.multiline {
            self.vertical(1)
        } else {
            self.content.len()
        };
        self.select_to(to, cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = 0..self.content.len();
        self.reversed = false;
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_start(self.cursor()), cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_end(self.cursor()), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_start(self.cursor()), cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_end(self.cursor()), cx);
    }

    fn doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn word_left_action(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.word_left(self.cursor()), cx);
    }

    fn word_right_action(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.word_right(self.cursor()), cx);
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.insert(self.selected.clone(), "\n");
            self.changed(cx);
        } else {
            cx.emit(InputEvent::Submit(self.content.clone()));
        }
    }

    fn escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        // The owner moves the keyboard back to the editor.
        window.blur();
        self.was_focused = false;
        cx.emit(InputEvent::Blur);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.insert(self.selected.clone(), &text);
            self.changed(cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected.clone()].to_string(),
            ));
            self.insert(self.selected.clone(), "");
            self.changed(cx);
        }
    }

    fn character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    // ---- mouse ----------------------------------------------------------

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // The editor's root would take the keyboard back on this press.
        cx.stop_propagation();
        window.focus(&self.focus);
        self.selecting = true;
        let at = self.index_at(e.position);
        if e.click_count >= 2 {
            let (from, to) = (
                self.word_left(at.min(self.content.len())),
                self.word_right(at),
            );
            self.selected = from..to;
            self.reversed = false;
            cx.notify();
        } else if e.modifiers.shift {
            self.select_to(at, cx);
        } else {
            self.move_to(at, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting && e.pressed_button == Some(MouseButton::Left) {
            self.select_to(self.index_at(e.position), cx);
        }
    }

    // ---- layout queries -------------------------------------------------

    /// The window point of the caret at `offset` (top of its row).
    fn position_of(&self, offset: usize) -> Option<Point<Pixels>> {
        let bounds = self.bounds?;
        let mut y = bounds.top();
        for (start, line) in &self.lines {
            let end = start + line.len();
            if offset <= end {
                let p = line.position_for_index(offset - start, self.line_height)?;
                return Some(point(bounds.left() + p.x, y + p.y));
            }
            y += line.size(self.line_height).height;
        }
        None
    }

    /// The text offset nearest a window point.
    fn index_at(&self, p: Point<Pixels>) -> usize {
        let Some(bounds) = self.bounds else {
            return self.content.len();
        };
        if self.content.is_empty() {
            return 0;
        }
        let mut y = bounds.top();
        for (i, (start, line)) in self.lines.iter().enumerate() {
            let h = line.size(self.line_height).height;
            let last = i + 1 == self.lines.len();
            if p.y < y + h || last {
                let local = point(p.x - bounds.left(), (p.y - y).max(px(0.)).min(h - px(1.)));
                let ix = match line.closest_index_for_position(local, self.line_height) {
                    Ok(ix) | Err(ix) => ix,
                };
                return start + ix.min(line.len());
            }
            y += h;
        }
        self.content.len()
    }

    // ---- utf-16 (the platform's offsets) -----------------------------------

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in self.content.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.content[..offset.min(self.content.len())]
            .chars()
            .map(char::len_utf16)
            .sum()
    }

    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(r.start)..self.offset_to_utf16(r.end)
    }

    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(r.start)..self.offset_from_utf16(r.end)
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range);
        actual.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected),
            reversed: self.reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        self.insert(range, text);
        self.marked = None;
        self.changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        new_selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        self.content.replace_range(range.clone(), text);
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        self.selected = new_selected
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .map(|r| r.start + range.start..r.end + range.start)
            .unwrap_or(range.start + text.len()..range.start + text.len());
        self.changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range);
        let a = self.position_of(range.start)?;
        let b = self.position_of(range.end)?;
        Some(Bounds::from_corners(
            a,
            point(b.x.max(a.x), b.y + self.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.index_at(p)))
    }
}

/// The painted text: shapes the lines to the field's width, then draws
/// the selection, the text and the caret.
struct TextElement {
    input: Entity<TextInput>,
}

struct Prepaint {
    lines: Vec<(usize, WrappedLine)>,
    quads: Vec<PaintQuad>,
    caret: Option<PaintQuad>,
    placeholder: bool,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let input = self.input.read(cx);
        let line_height = window.line_height();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        if input.multiline {
            // Grows with its rows (measured last frame), within the
            // field's min / max height set by the owner.
            let rows: f32 = input
                .lines
                .iter()
                .map(|(_, l)| (l.wrap_boundaries().len() + 1) as f32)
                .sum::<f32>()
                .max(1.);
            style.size.height = (line_height * rows).into();
        } else {
            style.size.height = line_height.into();
        }
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let input = self.input.read(cx);
        let style = window.text_style();
        let line_height = window.line_height();
        let placeholder = input.content.is_empty();
        let text: SharedString = if placeholder {
            input.placeholder.clone()
        } else {
            input.content.clone().into()
        };
        let color: gpui::Hsla = if placeholder {
            theme::text_tertiary().into()
        } else {
            style.color
        };
        let base = TextRun {
            len: text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match (&input.marked, placeholder) {
            (Some(m), false) => [
                (0..m.start, None),
                (
                    m.clone(),
                    Some(UnderlineStyle {
                        color: Some(color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                ),
                (m.end..text.len(), None),
            ]
            .into_iter()
            .filter(|(r, _)| !r.is_empty())
            .map(|(r, underline)| TextRun {
                len: r.len(),
                underline,
                ..base.clone()
            })
            .collect(),
            _ => vec![base],
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let wrap = input.multiline.then_some(bounds.size.width);
        let shaped = window
            .text_system()
            .shape_text(text, font_size, &runs, wrap, None)
            .unwrap_or_default();
        let mut lines = vec![];
        let mut start = 0;
        for line in shaped {
            let len = line.len();
            lines.push((start, line));
            start += len + 1;
        }

        // Selection rectangles and the caret, from the new layout.
        let pos = |offset: usize| -> Option<Point<Pixels>> {
            let mut y = bounds.top();
            for (s, line) in &lines {
                if offset <= s + line.len() {
                    let p = line.position_for_index(offset - s, line_height)?;
                    return Some(point(bounds.left() + p.x, y + p.y));
                }
                y += line.size(line_height).height;
            }
            None
        };
        let mut quads = vec![];
        let mut caret = None;
        if !placeholder {
            let sel = input.selected.clone();
            if sel.is_empty() {
                if let Some(p) = pos(input.cursor()) {
                    caret = Some(fill(
                        Bounds::new(p, size(px(1.5), line_height)),
                        theme::text(),
                    ));
                }
            } else if let (Some(a), Some(b)) = (pos(sel.start), pos(sel.end)) {
                let tint = theme::selection();
                if a.y == b.y {
                    quads.push(fill(
                        Bounds::from_corners(a, point(b.x, b.y + line_height)),
                        tint,
                    ));
                } else {
                    quads.push(fill(
                        Bounds::from_corners(a, point(bounds.right(), a.y + line_height)),
                        tint,
                    ));
                    if b.y > a.y + line_height {
                        quads.push(fill(
                            Bounds::from_corners(
                                point(bounds.left(), a.y + line_height),
                                point(bounds.right(), b.y),
                            ),
                            tint,
                        ));
                    }
                    quads.push(fill(
                        Bounds::from_corners(
                            point(bounds.left(), b.y),
                            point(b.x, b.y + line_height),
                        ),
                        tint,
                    ));
                }
            }
        } else if let Some(p) = pos(0) {
            caret = Some(fill(
                Bounds::new(p, size(px(1.5), line_height)),
                theme::text(),
            ));
        }
        Prepaint {
            lines,
            quads,
            caret,
            placeholder,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus.clone();
        let focused = focus.is_focused(window);
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if focused {
            for q in prepaint.quads.drain(..) {
                window.paint_quad(q);
            }
        }
        let line_height = window.line_height();
        let mut y = bounds.top();
        for (_, line) in &prepaint.lines {
            let _ = line.paint(
                point(bounds.left(), y),
                line_height,
                gpui::TextAlign::Left,
                Some(bounds),
                window,
                cx,
            );
            y += line.size(line_height).height;
        }
        if focused && let Some(c) = prepaint.caret.take() {
            window.paint_quad(c);
        }
        let lines = std::mem::take(&mut prepaint.lines);
        let placeholder = prepaint.placeholder;
        self.input.update(cx, |input, _| {
            // Hit-testing reads the layout of the real text only.
            input.lines = if placeholder { vec![] } else { lines };
            input.bounds = Some(bounds);
            input.line_height = line_height;
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        if self.was_focused && !focused {
            cx.emit(InputEvent::Blur);
        }
        self.was_focused = focused;
        div()
            .id("text-input")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .w_full()
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::word_left_action))
            .on_action(cx.listener(Self::word_right_action))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::character_palette))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextElement { input: cx.entity() })
    }
}

fn previous_boundary(s: &str, offset: usize) -> usize {
    s[..offset.min(s.len())]
        .char_indices()
        .next_back()
        .map_or(0, |(i, _)| i)
}

fn next_boundary(s: &str, offset: usize) -> usize {
    let offset = offset.min(s.len());
    s[offset..]
        .char_indices()
        .nth(1)
        .map_or(s.len(), |(i, _)| offset + i)
}

/// The start of the word before `offset` (skipping the gap before it).
fn word_left(s: &str, offset: usize) -> usize {
    let trimmed = s[..offset].trim_end_matches(|c: char| !c.is_alphanumeric());
    match trimmed
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_alphanumeric())
    {
        Some((i, c)) => i + c.len_utf8(),
        None => 0,
    }
}

/// The end of the word after `offset` (skipping the gap before it).
fn word_right(s: &str, offset: usize) -> usize {
    let after = &s[offset..];
    let skip = after.len()
        - after
            .trim_start_matches(|c: char| !c.is_alphanumeric())
            .len();
    let rest = &after[skip..];
    offset
        + skip
        + rest
            .find(|c: char| !c.is_alphanumeric())
            .unwrap_or(rest.len())
}

/// The start / end of the hard line holding `offset`.
fn line_start(s: &str, offset: usize) -> usize {
    s[..offset].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(s: &str, offset: usize) -> usize {
    s[offset..].find('\n').map_or(s.len(), |i| offset + i)
}

/// Replaces `range` of `content` with `text` (newlines flattened in a
/// one-line field, cut to `max_len` characters in all); the caret after it.
fn splice(
    content: &mut String,
    range: Range<usize>,
    text: &str,
    multiline: bool,
    max_len: usize,
) -> usize {
    let text = if multiline {
        text.replace("\r\n", "\n")
    } else {
        text.replace(['\n', '\r'], " ")
    };
    let kept = content.chars().count() - content[range.clone()].chars().count();
    let text: String = text.chars().take(max_len.saturating_sub(kept)).collect();
    content.replace_range(range.clone(), &text);
    range.start + text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_and_lines() {
        let s = "hello big\nworld";
        assert_eq!(word_left(s, 9), 6);
        assert_eq!(word_left(s, 3), 0);
        assert_eq!(word_right(s, 0), 5);
        assert_eq!(word_right(s, 5), 9);
        assert_eq!(line_start(s, 12), 10);
        assert_eq!(line_end(s, 2), 9);
        assert_eq!(previous_boundary(s, 10), 9);
        assert_eq!(next_boundary(s, 9), 10);
        assert_eq!(next_boundary("é", 0), 2);
    }

    #[test]
    fn splice_keeps_the_length_limit_and_one_line() {
        let mut s = "abcdefgh".to_string();
        assert_eq!(splice(&mut s, 8..8, "xyz", true, 10), 10);
        assert_eq!(s, "abcdefghxy");
        let mut one = "ab".to_string();
        splice(&mut one, 1..1, "x\ny", false, 100);
        assert_eq!(one, "ax yb");
    }
}
