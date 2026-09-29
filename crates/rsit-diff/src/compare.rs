//! Line and word comparison, modelled on IntelliJ's `ComparisonManagerImpl`:
//! a line diff first, then a word diff inside every modified block.

use std::collections::HashMap;
use std::ops::Range;

use imara_diff::{Algorithm, Diff, Token};

/// How whitespace takes part in the comparison (IntelliJ `ComparisonPolicy`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WhitespacePolicy {
    #[default]
    Default,
    /// Ignore whitespace at line ends.
    TrimWhitespaces,
    /// Ignore all whitespace.
    IgnoreWhitespaces,
}

/// A changed block: `left`/`right` are line ranges; one of them may be empty
/// (pure insertion or deletion).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineFragment {
    pub left: Range<u32>,
    pub right: Range<u32>,
    /// Changed words, as byte ranges relative to the start of the block on each
    /// side. `None` when the block is an insertion/deletion or too fragmented.
    pub inner: Option<Vec<WordFragment>>,
}

impl LineFragment {
    pub fn kind(&self) -> ChangeKind {
        match (self.left.is_empty(), self.right.is_empty()) {
            (true, _) => ChangeKind::Inserted,
            (_, true) => ChangeKind::Deleted,
            _ => ChangeKind::Modified,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    Inserted,
    Deleted,
    Modified,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordFragment {
    pub left: Range<usize>,
    pub right: Range<usize>,
}

/// A text split into lines; each line keeps its terminator.
pub struct Lines<'a> {
    pub text: &'a str,
    /// Byte offset of each line start, plus `text.len()` at the end.
    pub starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    pub fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1).filter(|&i| i < text.len()));
        starts.push(text.len());
        Self { text, starts }
    }

    pub fn len(&self) -> usize {
        if self.text.is_empty() { 0 } else { self.starts.len() - 1 }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Line `i` including its terminator.
    pub fn line(&self, i: usize) -> &'a str {
        &self.text[self.starts[i]..self.starts[i + 1]]
    }

    /// Line `i` without the trailing `\n` / `\r\n`.
    pub fn content(&self, i: usize) -> &'a str {
        let l = self.line(i);
        let l = l.strip_suffix('\n').unwrap_or(l);
        l.strip_suffix('\r').unwrap_or(l)
    }

    /// Byte range of lines `range` in `text`.
    pub fn byte_range(&self, range: Range<u32>) -> Range<usize> {
        self.starts[range.start as usize]..self.starts[range.end as usize]
    }
}

pub fn compare(left: &str, right: &str, policy: WhitespacePolicy) -> Vec<LineFragment> {
    let (l, r) = (Lines::new(left), Lines::new(right));
    compare_lines(&l, &r, policy)
}

pub fn compare_lines(left: &Lines<'_>, right: &Lines<'_>, policy: WhitespacePolicy) -> Vec<LineFragment> {
    let key = |line: &str| -> String {
        let content = line.trim_end_matches(['\n', '\r']);
        match policy {
            WhitespacePolicy::Default => content.to_string(),
            WhitespacePolicy::TrimWhitespaces => content.trim().to_string(),
            WhitespacePolicy::IgnoreWhitespaces => content.chars().filter(|c| !c.is_whitespace()).collect(),
        }
    };
    let mut interner = Interner::default();
    let before: Vec<Token> = (0..left.len()).map(|i| interner.intern(key(left.line(i)))).collect();
    let after: Vec<Token> = (0..right.len()).map(|i| interner.intern(key(right.line(i)))).collect();
    let mut diff = Diff::default();
    diff.compute_with(Algorithm::Histogram, &before, &after, interner.len());
    let hunks: Vec<(Range<u32>, Range<u32>)> = diff.hunks().map(|h| (h.before, h.after)).collect();

    hunks
        .into_iter()
        .map(|(l, r)| {
            let inner = if l.is_empty() || r.is_empty() {
                None
            } else {
                let lt = &left.text[left.byte_range(l.clone())];
                let rt = &right.text[right.byte_range(r.clone())];
                compare_words(lt, rt, policy)
            };
            LineFragment { left: l, right: r, inner }
        })
        .collect()
}

/// Word diff of two blocks (IntelliJ `ByWord`); `None` if the blocks share too
/// little to make word highlighting useful.
pub fn compare_words(left: &str, right: &str, policy: WhitespacePolicy) -> Option<Vec<WordFragment>> {
    let lt = tokenize(left);
    let rt = tokenize(right);
    let mut interner = Interner::default();
    let ignore_ws = policy != WhitespacePolicy::Default;
    // with whitespace ignored, whitespace runs are equal to each other
    let tok = |interner: &mut Interner, text: &str, r: &Range<usize>| {
        let s = &text[r.clone()];
        if ignore_ws && s.chars().all(char::is_whitespace) { interner.intern(" ".into()) } else { interner.intern(s.into()) }
    };
    let before: Vec<Token> = lt.iter().map(|r| tok(&mut interner, left, r)).collect();
    let after: Vec<Token> = rt.iter().map(|r| tok(&mut interner, right, r)).collect();
    let mut diff = Diff::default();
    diff.compute_with(Algorithm::Myers, &before, &after, interner.len());

    let mut fragments = Vec::new();
    let mut changed_bytes = 0usize;
    for h in diff.hunks() {
        let span = |tokens: &[Range<usize>], r: Range<u32>, anchor_len: usize| -> Range<usize> {
            if r.is_empty() {
                let at = tokens.get(r.start as usize).map_or(anchor_len, |t| t.start);
                at..at
            } else {
                tokens[r.start as usize].start..tokens[r.end as usize - 1].end
            }
        };
        let l = span(&lt, h.before.clone(), left.len());
        let r = span(&rt, h.after.clone(), right.len());
        let whitespace_only = left[l.clone()].trim().is_empty() && right[r.clone()].trim().is_empty();
        if ignore_ws && whitespace_only {
            continue;
        }
        changed_bytes += l.len() + r.len();
        fragments.push(WordFragment { left: l, right: r });
    }
    // mostly rewritten blocks read better without word highlighting
    let total = left.trim().len() + right.trim().len();
    if total > 0 && changed_bytes * 10 > total * 8 {
        return None;
    }
    Some(fragments)
}

/// Words (letters, digits, `_`), whitespace runs and single punctuation chars.
fn tokenize(text: &str) -> Vec<Range<usize>> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Space,
        Newline,
        Punct,
    }
    let class = |c: char| {
        if c == '\n' || c == '\r' {
            Class::Newline
        } else if c.is_whitespace() {
            Class::Space
        } else if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else {
            Class::Punct
        }
    };
    let mut tokens: Vec<Range<usize>> = Vec::new();
    let mut prev: Option<Class> = None;
    for (i, c) in text.char_indices() {
        let cl = class(c);
        let joins = matches!((&prev, &cl), (Some(Class::Word), Class::Word) | (Some(Class::Space), Class::Space));
        match tokens.last_mut() {
            Some(last) if joins => last.end = i + c.len_utf8(),
            _ => tokens.push(i..i + c.len_utf8()),
        }
        prev = Some(cl);
    }
    tokens
}

#[derive(Default)]
struct Interner {
    map: HashMap<String, u32>,
}

impl Interner {
    fn intern(&mut self, s: String) -> Token {
        let next = self.map.len() as u32;
        Token(*self.map.entry(s).or_insert(next))
    }

    fn len(&self) -> u32 {
        self.map.len() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines() {
        let l = Lines::new("a\nb\r\nc");
        assert_eq!(l.len(), 3);
        assert_eq!(l.content(1), "b");
        assert_eq!(l.line(2), "c");
        assert_eq!(Lines::new("").len(), 0);
        assert_eq!(Lines::new("x\n").len(), 1);
    }

    #[test]
    fn insert_delete_modify() {
        let f = compare("a\nb\nc\n", "a\nB\nc\nd\n", WhitespacePolicy::Default);
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].left.clone(), f[0].right.clone(), f[0].kind()), (1..2, 1..2, ChangeKind::Modified));
        assert_eq!((f[1].left.clone(), f[1].right.clone(), f[1].kind()), (3..3, 3..4, ChangeKind::Inserted));
    }

    #[test]
    fn words() {
        let f = compare("let x = foo(a, b);\n", "let x = bar(a, c);\n", WhitespacePolicy::Default);
        let inner = f[0].inner.as_ref().unwrap();
        let left = "let x = foo(a, b);\n";
        let right = "let x = bar(a, c);\n";
        let words: Vec<(&str, &str)> = inner.iter().map(|w| (&left[w.left.clone()], &right[w.right.clone()])).collect();
        assert_eq!(words, [("foo", "bar"), ("b", "c")]);
    }

    #[test]
    fn whitespace_policies() {
        let (l, r) = ("  a  \nb\n", "a\nb \n");
        assert_eq!(compare(l, r, WhitespacePolicy::Default).len(), 1);
        assert!(compare(l, r, WhitespacePolicy::TrimWhitespaces).is_empty());
        assert!(compare("a b\n", "ab\n", WhitespacePolicy::IgnoreWhitespaces).is_empty());
        assert_eq!(compare("a b\n", "ab\n", WhitespacePolicy::TrimWhitespaces).len(), 1);
    }

    #[test]
    fn rewritten_block_has_no_words() {
        let f = compare("completely different\n", "nothing alike here\n", WhitespacePolicy::Default);
        assert!(f[0].inner.is_none());
    }
}
