//! Helpers for drawing highlighted source lines (diff and annotate views).

use std::ops::Range;

use gpui_kit::HighlightStyle;

pub const TAB_WIDTH: usize = 4;

/// Highlights overlapping `range`, shifted to be relative to its start.
pub fn clip<'a>(
    styles: &'a [(Range<usize>, HighlightStyle)],
    range: &'a Range<usize>,
) -> impl Iterator<Item = (Range<usize>, HighlightStyle)> + 'a {
    let first = styles.partition_point(|(r, _)| r.end <= range.start);
    styles[first..]
        .iter()
        .take_while(|(r, _)| r.start < range.end)
        .filter(|(r, _)| r.end > range.start)
        .map(|(r, s)| (r.start.max(range.start) - range.start..r.end.min(range.end) - range.start, *s))
}

/// Replaces tabs with spaces up to the next tab stop, remapping highlight ranges.
pub fn expand_tabs(
    text: &str,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
    if !text.contains('\t') {
        return (text.to_string(), highlights);
    }
    let mut out = String::with_capacity(text.len() + 16);
    let mut map = vec![0usize; text.len() + 1];
    let mut column = 0;
    for (i, ch) in text.char_indices() {
        for b in 0..ch.len_utf8() {
            map[i + b] = out.len();
        }
        if ch == '\t' {
            let n = TAB_WIDTH - column % TAB_WIDTH;
            out.extend(std::iter::repeat_n(' ', n));
            column += n;
        } else {
            out.push(ch);
            column += 1;
        }
    }
    map[text.len()] = out.len();
    let highlights = highlights.into_iter().map(|(r, s)| (map[r.start]..map[r.end], s)).collect();
    (out, highlights)
}

/// Widest line in display columns (tabs expanded to the next stop).
pub fn max_columns(text: &str) -> usize {
    text.lines()
        .map(|line| line.chars().fold(0, |col, c| if c == '\t' { col + TAB_WIDTH - col % TAB_WIDTH } else { col + 1 }))
        .max()
        .unwrap_or(0)
}
