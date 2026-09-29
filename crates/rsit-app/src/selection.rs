//! Mouse text selection for views that draw source lines themselves (diff,
//! annotate, merge). The selection lives in model coordinates — pane, line of
//! that pane's text, byte offset in the line — so it survives virtualized
//! scrolling, folding and aligned filler rows.

use std::ops::Range;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::text::{TAB_WIDTH, expand_tabs};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPos {
    pub line: u32,
    /// Byte offset in the line's source text (tabs not expanded).
    pub offset: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    /// Which text of the view (e.g. left or right side of a diff).
    pub pane: usize,
    pub anchor: TextPos,
    pub head: TextPos,
}

impl Selection {
    pub fn ordered(&self) -> (TextPos, TextPos) {
        if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Selected byte range of `line` (whose text is `line_len` bytes) in `pane`.
    /// `full` is true when the selection continues past the line end.
    pub fn line_range(&self, pane: usize, line: u32, line_len: usize) -> Option<(Range<usize>, bool)> {
        if pane != self.pane || self.is_empty() {
            return None;
        }
        let (start, end) = self.ordered();
        if line < start.line || line > end.line {
            return None;
        }
        let from = if line == start.line { start.offset.min(line_len) } else { 0 };
        let (to, full) = if line == end.line { (end.offset.min(line_len), false) } else { (line_len, true) };
        (from < to || full).then_some((from..to, full))
    }

    /// The selected text; `line_text` gives a line of the pane without its terminator.
    pub fn text<'a>(&self, line_text: impl Fn(u32) -> Option<&'a str>) -> String {
        let (start, end) = self.ordered();
        let mut out = String::new();
        for line in start.line..=end.line {
            let Some(text) = line_text(line) else { continue };
            let from = if line == start.line { floor_char(text, start.offset) } else { 0 };
            let to = if line == end.line { floor_char(text, end.offset) } else { text.len() };
            if from <= to {
                out.push_str(&text[from..to]);
            }
            if line != end.line {
                out.push('\n');
            }
        }
        out
    }
}

/// Selection with drag state, embedded in a view.
#[derive(Default)]
pub struct SelectionState {
    pub selection: Option<Selection>,
    dragging: bool,
}

impl SelectionState {
    /// Mouse press at `pos`: starts a selection, extends it with Shift, or
    /// selects a word (double click) / line (triple click).
    pub fn mouse_down(&mut self, pane: usize, pos: TextPos, click_count: usize, shift: bool, line_text: &str) {
        self.dragging = true;
        match click_count {
            2 => {
                let word = word_at(line_text, pos.offset);
                self.selection = Some(Selection {
                    pane,
                    anchor: TextPos { line: pos.line, offset: word.start },
                    head: TextPos { line: pos.line, offset: word.end },
                });
            }
            n if n >= 3 => {
                self.selection = Some(Selection {
                    pane,
                    anchor: TextPos { line: pos.line, offset: 0 },
                    head: TextPos { line: pos.line + 1, offset: 0 },
                });
            }
            _ => match &mut self.selection {
                Some(sel) if shift && sel.pane == pane => sel.head = pos,
                _ => self.selection = Some(Selection { pane, anchor: pos, head: pos }),
            },
        }
    }

    /// Mouse moved over `pane` with the button held.
    pub fn drag(&mut self, pane: usize, pos: TextPos) -> bool {
        match &mut self.selection {
            Some(sel) if self.dragging && sel.pane == pane && sel.head != pos => {
                sel.head = pos;
                true
            }
            _ => false,
        }
    }

    pub fn mouse_up(&mut self) {
        self.dragging = false;
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    pub fn select_all(&mut self, pane: usize, line_count: u32) {
        self.selection = Some(Selection {
            pane,
            anchor: TextPos { line: 0, offset: 0 },
            head: TextPos { line: line_count, offset: 0 },
        });
    }

    pub fn clear(&mut self) {
        self.selection = None;
        self.dragging = false;
    }

    pub fn pane(&self) -> Option<usize> {
        self.selection.map(|s| s.pane)
    }
}

/// A view with selectable lines.
pub trait SelectableText: Sized + 'static {
    fn selection_state(&mut self) -> &mut SelectionState;
    /// A line of `pane` without its terminator.
    fn line_text(&self, pane: usize, line: u32) -> Option<&str>;
    fn line_count(&self, pane: usize) -> u32;
    fn focus_handle(&self) -> &FocusHandle;

    /// The selected text, if any.
    fn selected_text(&mut self) -> Option<String> {
        let sel = self.selection_state().selection.filter(|s| !s.is_empty())?;
        Some(sel.text(|line| self.line_text(sel.pane, line)))
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Selects the whole pane that has the selection (or `default_pane`).
    fn select_all_text(&mut self, default_pane: usize, cx: &mut Context<Self>) {
        let pane = self.selection_state().pane().unwrap_or(default_pane);
        let count = self.line_count(pane);
        self.selection_state().select_all(pane, count);
        cx.notify();
    }
}

/// A source line drawn with its highlights plus the selection, tabs expanded.
pub struct RenderedLine {
    pub text: StyledText,
    pub layout: TextLayout,
    pub display: SharedString,
    /// The selection continues past the line end: draw a newline marker.
    pub selected_newline: bool,
}

/// `highlights` are relative to `source`; `selected` comes from [`Selection::line_range`].
pub fn render_line(
    source: &str,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    selected: Option<(Range<usize>, bool)>,
    selection_bg: Hsla,
) -> RenderedLine {
    let (highlights, selected_newline) = match selected {
        Some((range, full)) => {
            let sel = (range, HighlightStyle { background_color: Some(selection_bg), ..Default::default() });
            (combine_highlights(highlights, [sel]).collect(), full)
        }
        None => (highlights, false),
    };
    let (display, highlights) = expand_tabs(source, highlights);
    let display: SharedString = display.into();
    let text = StyledText::new(display.clone()).with_highlights(highlights);
    let layout = text.layout().clone();
    RenderedLine { text, layout, display, selected_newline }
}

/// Makes `cell` start, extend and drag selections of `line` in `pane`.
pub fn attach<V: SelectableText>(
    cell: Div,
    pane: usize,
    line: u32,
    rendered: &RenderedLine,
    source: SharedString,
    cx: &mut Context<V>,
) -> Div {
    let (layout_down, display_down, source_down) = (rendered.layout.clone(), rendered.display.clone(), source.clone());
    let (layout_move, display_move, source_move) = (rendered.layout.clone(), rendered.display.clone(), source);
    cell.cursor_text()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                let offset = hit_test(&layout_down, &display_down, &source_down, event.position);
                let pos = TextPos { line, offset };
                view.selection_state().mouse_down(pane, pos, event.click_count, event.modifiers.shift, &source_down);
                let focus = view.focus_handle().clone();
                window.focus(&focus, cx);
                cx.notify();
            }),
        )
        .on_mouse_move(cx.listener(move |view, event: &MouseMoveEvent, _, cx| {
            if event.pressed_button != Some(MouseButton::Left) {
                return;
            }
            let offset = hit_test(&layout_move, &display_move, &source_move, event.position);
            if view.selection_state().drag(pane, TextPos { line, offset }) {
                cx.notify();
            }
        }))
}

/// Ends drags anywhere in the view (put on its root element).
pub fn release<V: SelectableText>(root: Stateful<Div>, cx: &mut Context<V>) -> Stateful<Div> {
    root.on_mouse_up(MouseButton::Left, cx.listener(|view, _: &MouseUpEvent, _, _| view.selection_state().mouse_up()))
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|view, _: &MouseUpEvent, _, _| view.selection_state().mouse_up()),
        )
}

/// Child element for a line: the text plus a marker when its newline is selected.
pub fn line_content(rendered: RenderedLine, selection_bg: Hsla, h_offset: f32) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .ml(px(-h_offset))
        .child(rendered.text)
        .when(rendered.selected_newline, |d| d.child(div().w(px(7.)).h(px(15.)).bg(selection_bg)))
}

/// Source byte offset under `position` in a line drawn by `layout` from
/// `display` (the tab-expanded form of `source`), rounded to the nearest
/// character boundary.
pub fn hit_test(layout: &TextLayout, display: &str, source: &str, position: Point<Pixels>) -> usize {
    let index = match layout.index_for_position(position) {
        Ok(i) | Err(i) => i.min(display.len()),
    };
    let index = floor_char(display, index);
    // past the middle of a glyph selects after it
    let next = display[index..].chars().next().map_or(index, |c| index + c.len_utf8());
    let index = match (layout.position_for_index(index), layout.position_for_index(next)) {
        (Some(a), Some(b)) if next > index && position.x > (a.x + b.x) / 2. => next,
        _ => index,
    };
    display_to_source(source, index)
}

/// Offset in `source` for a byte offset in its tab-expanded display form.
pub fn display_to_source(source: &str, display_offset: usize) -> usize {
    let mut display = 0;
    let mut column = 0;
    for (i, ch) in source.char_indices() {
        let width = if ch == '\t' { TAB_WIDTH - column % TAB_WIDTH } else { 1 };
        let bytes = if ch == '\t' { width } else { ch.len_utf8() };
        if display_offset < display + bytes {
            // inside a tab: nearer to its start or its end
            return if ch == '\t' && display_offset - display > bytes / 2 { i + 1 } else { i };
        }
        display += bytes;
        column += width;
    }
    source.len()
}

/// Byte offset in the tab-expanded display form of `source`.
pub fn source_to_display(source: &str, source_offset: usize) -> usize {
    let mut display = 0;
    let mut column = 0;
    for (i, ch) in source.char_indices() {
        if i >= source_offset {
            return display;
        }
        if ch == '\t' {
            let n = TAB_WIDTH - column % TAB_WIDTH;
            display += n;
            column += n;
        } else {
            display += ch.len_utf8();
            column += 1;
        }
    }
    display
}

/// The word (letters, digits, `_`) or run of other characters around `offset`.
pub fn word_at(text: &str, offset: usize) -> Range<usize> {
    let offset = floor_char(text, offset.min(text.len()));
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    };
    let Some(here) = text[offset..].chars().next().or_else(|| text[..offset].chars().next_back()) else {
        return offset..offset;
    };
    let k = class(here);
    let start =
        text[..offset].char_indices().rev().take_while(|(_, c)| class(*c) == k).last().map_or(offset, |(i, _)| i);
    let end = text[offset..].char_indices().find(|(_, c)| class(*c) != k).map_or(text.len(), |(i, _)| offset + i);
    start..end
}

fn floor_char(text: &str, mut i: usize) -> usize {
    i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    // not `super::*`: gpui's `test` attribute would shadow the standard one
    use super::{Selection, SelectionState, TextPos, display_to_source, source_to_display, word_at};

    fn pos(line: u32, offset: usize) -> TextPos {
        TextPos { line, offset }
    }

    #[test]
    fn copies_across_lines_in_either_direction() {
        let lines = ["first line", "second", "third line"];
        let text = |l: u32| lines.get(l as usize).copied();
        let sel = Selection { pane: 0, anchor: pos(2, 5), head: pos(0, 6) };
        assert_eq!(sel.text(text), "line\nsecond\nthird");
        assert_eq!(sel.line_range(0, 0, 10), Some((6..10, true)));
        assert_eq!(sel.line_range(0, 1, 6), Some((0..6, true)));
        assert_eq!(sel.line_range(0, 2, 10), Some((0..5, false)));
        assert_eq!(sel.line_range(1, 1, 6), None, "other pane");
    }

    #[test]
    fn clicks_select_words_and_lines() {
        let mut s = SelectionState::default();
        s.mouse_down(1, pos(3, 9), 2, false, "let value_1 = foo();");
        assert_eq!(s.selection.unwrap().text(|_| Some("let value_1 = foo();")), "value_1");
        s.mouse_down(1, pos(3, 0), 3, false, "abc");
        let lines = ["", "", "", "abc", "next"];
        assert_eq!(s.selection.unwrap().text(|l| lines.get(l as usize).copied()), "abc\n");
        s.mouse_down(1, pos(0, 1), 1, false, "");
        assert!(s.drag(1, pos(0, 3)));
        assert!(!s.drag(0, pos(0, 5)), "drag stays in its pane");
        s.mouse_up();
        assert!(!s.drag(1, pos(0, 4)), "no drag after release");
        s.mouse_down(1, pos(1, 2), 1, true, "");
        assert_eq!(s.selection.unwrap().ordered(), (pos(0, 1), pos(1, 2)), "shift extends");
    }

    #[test]
    fn tabs_map_between_source_and_display() {
        let src = "\tab\tc";
        // display: "    ab  c"
        assert_eq!(source_to_display(src, 1), 4);
        assert_eq!(source_to_display(src, 4), 8);
        assert_eq!(display_to_source(src, 4), 1);
        assert_eq!(display_to_source(src, 1), 0);
        assert_eq!(display_to_source(src, 3), 1, "past the middle of a tab");
        assert_eq!(display_to_source(src, 9), 5);
        assert_eq!(word_at("a  bc", 3), 3..5);
        assert_eq!(word_at("a  bc", 1), 1..3);
    }
}
