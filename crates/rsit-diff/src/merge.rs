//! Three-way merge (after IntelliJ `ComparisonMergeUtil` / `MergeLineFragment`):
//! changes of both sides against the base, grouped into chunks where they
//! overlap, and the resolution state the merge window edits.

use std::ops::Range;

use crate::compare::{LineFragment, Lines, WhitespacePolicy, compare_lines};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkKind {
    /// Only the left side (yours) changed.
    Left,
    /// Only the right side (theirs) changed.
    Right,
    /// Both sides made the same change.
    Both,
    /// Both sides changed it differently.
    Conflict,
}

/// One region where at least one side differs from the base; line ranges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeChunk {
    pub base: Range<u32>,
    pub left: Range<u32>,
    pub right: Range<u32>,
    pub kind: ChunkKind,
}

/// Line-level three-way comparison.
pub fn merge_lines(base: &Lines<'_>, left: &Lines<'_>, right: &Lines<'_>, policy: WhitespacePolicy) -> Vec<MergeChunk> {
    let a = compare_lines(base, left, policy);
    let b = compare_lines(base, right, policy);
    group(&a, &b, |l, r| {
        let lt = &left.text[left.byte_range(l.clone())];
        let rt = &right.text[right.byte_range(r.clone())];
        lt == rt
    })
}

/// Groups two sorted change lists (both against the base) into chunks.
fn group(a: &[LineFragment], b: &[LineFragment], same: impl Fn(&Range<u32>, &Range<u32>) -> bool) -> Vec<MergeChunk> {
    // changes that overlap or merely touch are one chunk: git (xdiff) treats
    // adjacent changes from both sides as a conflict too
    let touches = |x: &Range<u32>, y: &Range<u32>| x.start <= y.end && y.start <= x.end;
    let (mut i, mut j) = (0, 0);
    // side offsets: position in the side text minus position in the base, before the current chunk
    let (mut da, mut db) = (0i64, 0i64);
    let mut chunks = Vec::new();
    while i < a.len() || j < b.len() {
        // start with whichever change comes first in the base
        let start_a = a.get(i).map(|f| f.left.start);
        let start_b = b.get(j).map(|f| f.left.start);
        let mut base = match (start_a, start_b) {
            (Some(x), Some(y)) if x <= y => a[i].left.clone(),
            (Some(_), None) => a[i].left.clone(),
            _ => b[j].left.clone(),
        };
        let (ai, bj) = (i, j);
        // absorb every change of either side touching the chunk
        loop {
            let mut grew = false;
            while i < a.len() && touches(&a[i].left, &base) {
                base = base.start.min(a[i].left.start)..base.end.max(a[i].left.end);
                i += 1;
                grew = true;
            }
            while j < b.len() && touches(&b[j].left, &base) {
                base = base.start.min(b[j].left.start)..base.end.max(b[j].left.end);
                j += 1;
                grew = true;
            }
            if !grew {
                break;
            }
        }
        let side_range = |fragments: &[LineFragment], offset: i64| -> Range<u32> {
            let growth: i64 = fragments.iter().map(|f| f.right.len() as i64 - f.left.len() as i64).sum();
            (base.start as i64 + offset) as u32..(base.end as i64 + offset + growth) as u32
        };
        let left = side_range(&a[ai..i], da);
        let right = side_range(&b[bj..j], db);
        let kind = match (i > ai, j > bj) {
            (true, false) => ChunkKind::Left,
            (false, true) => ChunkKind::Right,
            _ if same(&left, &right) => ChunkKind::Both,
            _ => ChunkKind::Conflict,
        };
        da += (left.len() as i64) - (base.len() as i64);
        db += (right.len() as i64) - (base.len() as i64);
        chunks.push(MergeChunk { base, left, right, kind });
    }
    chunks
}

/// What the user decided for a chunk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolution {
    pub left_applied: bool,
    pub right_applied: bool,
    /// Resolved without taking a side (IntelliJ "Ignore"): the base stays.
    pub ignored: bool,
    /// Replacement text from "Resolve Simple Conflicts".
    pub merged: Option<String>,
}

impl Resolution {
    pub fn is_resolved(&self) -> bool {
        self.left_applied || self.right_applied || self.ignored || self.merged.is_some()
    }
}

/// The state of a merge: texts, chunks and resolutions; renders the result.
#[derive(Clone, Debug)]
pub struct MergeModel {
    pub base: String,
    pub left: String,
    pub right: String,
    pub chunks: Vec<MergeChunk>,
    pub resolutions: Vec<Resolution>,
}

impl MergeModel {
    pub fn new(base: String, left: String, right: String, policy: WhitespacePolicy) -> Self {
        let chunks = merge_lines(&Lines::new(&base), &Lines::new(&left), &Lines::new(&right), policy);
        let resolutions = vec![Resolution::default(); chunks.len()];
        Self { base, left, right, chunks, resolutions }
    }

    pub fn unresolved(&self) -> usize {
        self.resolutions.iter().filter(|r| !r.is_resolved()).count()
    }

    pub fn unresolved_conflicts(&self) -> usize {
        self.chunks
            .iter()
            .zip(&self.resolutions)
            .filter(|(c, r)| c.kind == ChunkKind::Conflict && !r.is_resolved())
            .count()
    }

    /// IntelliJ "Apply All Non-Conflicting Changes".
    pub fn apply_non_conflicting(&mut self) {
        for (c, r) in self.chunks.iter().zip(self.resolutions.iter_mut()) {
            if r.is_resolved() {
                continue;
            }
            match c.kind {
                ChunkKind::Left | ChunkKind::Both => r.left_applied = true,
                ChunkKind::Right => r.right_applied = true,
                ChunkKind::Conflict => {}
            }
        }
    }

    /// IntelliJ "Resolve Simple Conflicts": conflicts whose word-level
    /// changes do not overlap are merged automatically. Returns how many.
    pub fn resolve_simple_conflicts(&mut self) -> usize {
        let mut resolved = 0;
        for index in 0..self.chunks.len() {
            let c = &self.chunks[index];
            if c.kind != ChunkKind::Conflict || self.resolutions[index].is_resolved() {
                continue;
            }
            let (base, left, right) = (
                self.chunk_text(Side::Base, index),
                self.chunk_text(Side::Left, index),
                self.chunk_text(Side::Right, index),
            );
            if let Some(merged) = merge_words(base, left, right) {
                self.resolutions[index].merged = Some(merged);
                resolved += 1;
            }
        }
        resolved
    }

    pub fn chunk_text(&self, side: Side, index: usize) -> &str {
        let c = &self.chunks[index];
        let (text, range) = match side {
            Side::Base => (&self.base, c.base.clone()),
            Side::Left => (&self.left, c.left.clone()),
            Side::Right => (&self.right, c.right.clone()),
        };
        let lines = Lines::new(text);
        &text[lines.byte_range(range)]
    }

    /// Text of chunk `index` in the result.
    pub fn result_chunk(&self, index: usize) -> String {
        let r = &self.resolutions[index];
        if let Some(merged) = &r.merged {
            return merged.clone();
        }
        let mut out = String::new();
        if r.left_applied {
            out.push_str(self.chunk_text(Side::Left, index));
        }
        // a side applied after the other is appended (IntelliJ "append")
        if r.right_applied && !(r.left_applied && self.chunks[index].kind == ChunkKind::Both) {
            push_line_aware(&mut out, self.chunk_text(Side::Right, index));
        }
        if !r.left_applied && !r.right_applied {
            out.push_str(self.chunk_text(Side::Base, index));
        }
        out
    }

    /// The merged file: unchanged base text plus each chunk's result.
    pub fn result(&self) -> String {
        let lines = Lines::new(&self.base);
        let mut out = String::with_capacity(self.base.len() + self.left.len() / 4);
        let mut pos = 0u32;
        for (i, c) in self.chunks.iter().enumerate() {
            out.push_str(&self.base[lines.byte_range(pos..c.base.start)]);
            push_line_aware(&mut out, &self.result_chunk(i));
            pos = c.base.end;
        }
        out.push_str(&self.base[lines.byte_range(pos..lines.len() as u32)]);
        out
    }
}

/// Appends `text`, making sure the previous content ended its last line.
fn push_line_aware(out: &mut String, text: &str) {
    if !out.is_empty() && !out.ends_with('\n') && !text.is_empty() {
        out.push('\n');
    }
    out.push_str(text);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Base,
    Left,
    Right,
}

/// Word-level three-way merge of one conflict; `None` if the words conflict too.
pub fn merge_words(base: &str, left: &str, right: &str) -> Option<String> {
    let (bt, lt, rt) = (tokens(base), tokens(left), tokens(right));
    let fa = token_fragments(&bt, &lt);
    let fb = token_fragments(&bt, &rt);
    let chunks = group(&fa, &fb, |l, r| {
        let a: Vec<&str> = lt[l.start as usize..l.end as usize].to_vec();
        let b: Vec<&str> = rt[r.start as usize..r.end as usize].to_vec();
        a == b
    });
    if chunks.iter().any(|c| c.kind == ChunkKind::Conflict) {
        return None;
    }
    let mut out = String::new();
    let mut pos = 0usize;
    for c in &chunks {
        out.extend(bt[pos..c.base.start as usize].iter().copied());
        match c.kind {
            ChunkKind::Right => out.extend(rt[c.right.start as usize..c.right.end as usize].iter().copied()),
            _ => out.extend(lt[c.left.start as usize..c.left.end as usize].iter().copied()),
        }
        pos = c.base.end as usize;
    }
    out.extend(bt[pos..].iter().copied());
    Some(out)
}

fn tokens(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<u8> = None;
    let class = |c: char| -> u8 {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c == '\n' {
            3
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    };
    for (i, c) in text.char_indices() {
        let k = class(c);
        if let Some(p) = prev
            && !(p == k && (k == 0 || k == 1))
        {
            out.push(&text[start..i]);
            start = i;
        }
        prev = Some(k);
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Token diff as fragments (`left` = base tokens, `right` = side tokens).
fn token_fragments(base: &[&str], side: &[&str]) -> Vec<LineFragment> {
    let mut ids = std::collections::HashMap::new();
    let mut intern = |t: &str| {
        let n = ids.len() as u32;
        imara_diff::Token(*ids.entry(t.to_string()).or_insert(n))
    };
    let before: Vec<imara_diff::Token> = base.iter().map(|t| intern(t)).collect();
    let after: Vec<imara_diff::Token> = side.iter().map(|t| intern(t)).collect();
    let mut diff = imara_diff::Diff::default();
    diff.compute_with(imara_diff::Algorithm::Myers, &before, &after, ids.len() as u32);
    diff.hunks().map(|h| LineFragment { left: h.before, right: h.after, inner: None }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(base: &str, left: &str, right: &str) -> MergeModel {
        MergeModel::new(base.into(), left.into(), right.into(), WhitespacePolicy::Default)
    }

    #[test]
    fn classifies_chunks() {
        let m = model("a\nb\nc\nd\ne\n", "A\nb\nc\nD\ne\n", "a\nb\nC\nD2\ne\n");
        let kinds: Vec<ChunkKind> = m.chunks.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, [ChunkKind::Left, ChunkKind::Conflict]);
        assert_eq!(m.chunks[1].base, 2..4);
        assert_eq!((m.chunks[1].left.clone(), m.chunks[1].right.clone()), (2..4, 2..4));
    }

    #[test]
    fn same_change_on_both_sides() {
        let m = model("a\nb\n", "a\nB\n", "a\nB\n");
        assert_eq!(m.chunks[0].kind, ChunkKind::Both);
    }

    #[test]
    fn non_conflicting_merge_matches_git() {
        let mut m = model("1\n2\n3\n4\n5\n6\n", "1\nleft\n2\n3\n4\n5\n6\n", "1\n2\n3\n4\n5\n6\nright\n");
        assert_eq!(m.unresolved_conflicts(), 0);
        m.apply_non_conflicting();
        assert_eq!(m.unresolved(), 0);
        assert_eq!(m.result(), "1\nleft\n2\n3\n4\n5\n6\nright\n");
    }

    #[test]
    fn conflict_resolutions() {
        let mut m = model("a\nx\nb\n", "a\nL\nb\n", "a\nR\nb\n");
        assert_eq!(m.result(), "a\nx\nb\n", "unresolved keeps the base");
        m.resolutions[0].left_applied = true;
        assert_eq!(m.result(), "a\nL\nb\n");
        m.resolutions[0].right_applied = true;
        assert_eq!(m.result(), "a\nL\nR\nb\n", "both sides appended");
        m.resolutions[0] = Resolution { ignored: true, ..Default::default() };
        assert_eq!(m.result(), "a\nx\nb\n");
    }

    #[test]
    fn simple_conflicts_merge_by_words() {
        let mut m = model("let x = foo(a, b);\n", "let y = foo(a, b);\n", "let x = foo(a, c);\n");
        assert_eq!(m.chunks[0].kind, ChunkKind::Conflict);
        assert_eq!(m.resolve_simple_conflicts(), 1);
        assert_eq!(m.result(), "let y = foo(a, c);\n");
        let mut hard = model("x = 1\n", "x = 2\n", "x = 3\n");
        assert_eq!(hard.resolve_simple_conflicts(), 0);
    }

    #[test]
    fn insertions_at_the_same_place_conflict() {
        let m = model("a\nb\n", "a\nL\nb\n", "a\nR\nb\n");
        assert_eq!(m.chunks.len(), 1);
        assert_eq!(m.chunks[0].kind, ChunkKind::Conflict);
    }
}
