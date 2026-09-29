//! Display rows of a diff: side-by-side with both sides aligned (one scroll
//! position for both, like IntelliJ's "Align changes" mode) or unified, with
//! unchanged regions optionally folded.

use std::collections::HashSet;
use std::ops::Range;

use crate::compare::{ChangeKind, LineFragment};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// A line of the left and/or the right text (0-based line numbers).
    Line { left: Option<u32>, right: Option<u32>, change: Option<Change> },
    /// Hidden unchanged lines; `left` and `right` have the same length.
    Fold { left: Range<u32>, right: Range<u32> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    /// Index into the fragment list.
    pub fragment: usize,
    pub kind: ChangeKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    SideBySide,
    Unified,
}

/// Folding of unchanged regions: `context` lines stay visible around changes;
/// folds whose first hidden left line is in `expanded` stay open.
#[derive(Clone, Debug, Default)]
pub struct Folding {
    pub context: u32,
    pub expanded: HashSet<u32>,
}

pub struct DiffRows {
    pub rows: Vec<Row>,
    /// First row of every fragment, for next/previous change navigation.
    pub change_starts: Vec<usize>,
}

pub fn build_rows(
    fragments: &[LineFragment],
    left_len: u32,
    right_len: u32,
    layout: Layout,
    folding: Option<&Folding>,
) -> DiffRows {
    let mut out = DiffRows { rows: Vec::new(), change_starts: Vec::new() };
    let (mut l, mut r) = (0u32, 0u32);
    for (i, f) in fragments.iter().enumerate() {
        equal_run(&mut out.rows, l..f.left.start, r..f.right.start, i == 0, false, folding);
        out.change_starts.push(out.rows.len());
        let change = Some(Change { fragment: i, kind: f.kind() });
        match layout {
            Layout::SideBySide => {
                let height = f.left.len().max(f.right.len()) as u32;
                for k in 0..height {
                    let left = (f.left.start + k < f.left.end).then_some(f.left.start + k);
                    let right = (f.right.start + k < f.right.end).then_some(f.right.start + k);
                    out.rows.push(Row::Line { left, right, change });
                }
            }
            Layout::Unified => {
                out.rows.extend(f.left.clone().map(|left| Row::Line { left: Some(left), right: None, change }));
                out.rows.extend(f.right.clone().map(|right| Row::Line { left: None, right: Some(right), change }));
            }
        }
        l = f.left.end;
        r = f.right.end;
    }
    equal_run(&mut out.rows, l..left_len, r..right_len, fragments.is_empty(), true, folding);
    out
}

/// Unchanged lines between changes, folded to `context` lines on each side.
fn equal_run(
    rows: &mut Vec<Row>,
    left: Range<u32>,
    right: Range<u32>,
    at_start: bool,
    at_end: bool,
    folding: Option<&Folding>,
) {
    debug_assert_eq!(left.len(), right.len());
    let len = left.len() as u32;
    let line = |k: u32| Row::Line { left: Some(left.start + k), right: Some(right.start + k), change: None };
    let Some(folding) = folding else {
        rows.extend((0..len).map(line));
        return;
    };
    let head = if at_start { 0 } else { folding.context };
    let tail = if at_end { 0 } else { folding.context };
    // folding fewer than a couple of lines saves nothing; expanded folds are
    // keyed by their first hidden left line
    if len <= head + tail + 1 || folding.expanded.contains(&(left.start + head)) {
        rows.extend((0..len).map(line));
        return;
    }
    rows.extend((0..head).map(line));
    rows.push(Row::Fold { left: left.start + head..left.end - tail, right: right.start + head..right.end - tail });
    rows.extend((len - tail..len).map(line));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::{WhitespacePolicy, compare};

    fn text(n: u32) -> String {
        (0..n).map(|i| format!("line {i}\n")).collect()
    }

    #[test]
    fn side_by_side_aligns_blocks() {
        let f = compare("a\nb\nc\n", "a\nx\ny\nc\n", WhitespacePolicy::Default);
        let rows = build_rows(&f, 3, 4, Layout::SideBySide, None).rows;
        let pairs: Vec<(Option<u32>, Option<u32>)> = rows
            .iter()
            .map(|r| match r {
                Row::Line { left, right, .. } => (*left, *right),
                Row::Fold { .. } => unreachable!(),
            })
            .collect();
        assert_eq!(pairs, [(Some(0), Some(0)), (Some(1), Some(1)), (None, Some(2)), (Some(2), Some(3))]);
    }

    #[test]
    fn unified_puts_deletions_first() {
        let f = compare("a\nb\n", "a\nc\n", WhitespacePolicy::Default);
        let rows = build_rows(&f, 2, 2, Layout::Unified, None).rows;
        assert_eq!(rows.len(), 3);
        assert!(matches!(rows[1], Row::Line { left: Some(1), right: None, .. }));
        assert!(matches!(rows[2], Row::Line { left: None, right: Some(1), .. }));
    }

    #[test]
    fn folding_keeps_context() {
        let left = text(30);
        let right = left.replace("line 15\n", "changed\n");
        let f = compare(&left, &right, WhitespacePolicy::Default);
        let folding = Folding { context: 4, expanded: HashSet::new() };
        let rows = build_rows(&f, 30, 30, Layout::SideBySide, Some(&folding));
        // fold(0..11) + 4 context + change + 4 context + fold(20..30)
        assert_eq!(rows.rows.len(), 1 + 4 + 1 + 4 + 1);
        assert_eq!(rows.rows[0], Row::Fold { left: 0..11, right: 0..11 });
        assert_eq!(rows.change_starts, [5]);
        let expanded = Folding { context: 4, expanded: [0].into_iter().collect() };
        let rows = build_rows(&f, 30, 30, Layout::SideBySide, Some(&expanded));
        assert_eq!(rows.change_starts, [15]);
    }
}
