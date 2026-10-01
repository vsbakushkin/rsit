//! Data behind the diff viewer: both versions of a file, their syntax
//! highlighting and the comparison result.

use std::ops::Range;
use std::sync::Arc;

use anyhow::Result;
use gpui_kit::HighlightStyle;
use gpui_kit::SharedString;
use gpui_kit::component::highlighter::{HighlightTheme, Language, SyntaxHighlighter};
use gpui_kit::component::input::Rope;
use rsit_diff::{LineFragment, Lines, WhitespacePolicy};
use rsit_git::{ChangeKind, FileChange, ObjectId, Repo, Revision};

/// Files larger than this are compared but not syntax highlighted.
const MAX_HIGHLIGHT_BYTES: usize = 2 * 1024 * 1024;

pub struct DiffSide {
    pub text: String,
    pub line_starts: Vec<usize>,
    /// Syntax highlighting over the whole text, sorted by start.
    pub syntax: Vec<(Range<usize>, HighlightStyle)>,
    /// Absolute byte ranges of changed words, sorted.
    pub words: Vec<Range<usize>>,
    pub exists: bool,
}

impl DiffSide {
    pub fn new(bytes: Option<Vec<u8>>, language: &str, theme: &HighlightTheme) -> Self {
        let exists = bytes.is_some();
        let text = String::from_utf8_lossy(&bytes.unwrap_or_default()).into_owned();
        let line_starts = Lines::new(&text).starts;
        let mut side = Self { text, line_starts, syntax: Vec::new(), words: Vec::new(), exists };
        side.highlight(language, theme);
        side
    }

    /// Recomputes the syntax highlighting, e.g. for another theme.
    pub fn highlight(&mut self, language: &str, theme: &HighlightTheme) {
        self.syntax = if self.text.len() <= MAX_HIGHLIGHT_BYTES && language != "text" {
            let mut highlighter = SyntaxHighlighter::new(language);
            highlighter.update(None, &Rope::from_str(&self.text), None);
            highlighter.styles(&(0..self.text.len()), theme)
        } else {
            Vec::new()
        };
    }

    pub fn line_count(&self) -> u32 {
        if self.text.is_empty() { 0 } else { self.line_starts.len() as u32 - 1 }
    }

    /// Byte range of line `i` without its terminator.
    pub fn line_range(&self, i: u32) -> Range<usize> {
        let start = self.line_starts[i as usize];
        let mut end = self.line_starts[i as usize + 1];
        let bytes = self.text.as_bytes();
        while end > start && (bytes[end - 1] == b'\n' || bytes[end - 1] == b'\r') {
            end -= 1;
        }
        start..end
    }
}

pub struct FileDiff {
    pub left: DiffSide,
    pub right: DiffSide,
    pub fragments: Vec<LineFragment>,
    pub binary: bool,
    pub language: &'static str,
}

impl FileDiff {
    /// Reads both sides of `item` and compares them.
    pub fn load(repo: &Repo, item: &DiffItem, policy: WhitespacePolicy, theme: &HighlightTheme) -> Result<Self> {
        let left_path = item.change.old_path.as_deref().unwrap_or(&item.change.path);
        let read = |rev: Option<Revision>, path: &str| -> Result<Option<Vec<u8>>> {
            match rev {
                Some(rev) => rsit_git::file_at_revision(repo, rev, path),
                None => Ok(None),
            }
        };
        let left = read(item.left, left_path)?;
        let right = read(item.right, &item.change.path)?;
        let binary = [&left, &right].iter().any(|b| b.as_ref().is_some_and(|b| is_binary(b)));
        let language = if binary { "text" } else { language_for(&item.change.path) };
        let (left_exists, right_exists) = (left.is_some(), right.is_some());
        let mut diff = Self {
            left: DiffSide::new(if binary { None } else { left }, language, theme),
            right: DiffSide::new(if binary { None } else { right }, language, theme),
            fragments: Vec::new(),
            binary,
            language,
        };
        diff.left.exists = left_exists;
        diff.right.exists = right_exists;
        diff.recompare(policy);
        Ok(diff)
    }

    pub fn recompare(&mut self, policy: WhitespacePolicy) {
        let (l, r) = (Lines::new(&self.left.text), Lines::new(&self.right.text));
        self.fragments = rsit_diff::compare_lines(&l, &r, policy);
        self.left.words.clear();
        self.right.words.clear();
        for f in &self.fragments {
            let Some(inner) = &f.inner else { continue };
            let (lb, rb) = (l.starts[f.left.start as usize], r.starts[f.right.start as usize]);
            for w in inner {
                if !w.left.is_empty() {
                    self.left.words.push(lb + w.left.start..lb + w.left.end);
                }
                if !w.right.is_empty() {
                    self.right.words.push(rb + w.right.start..rb + w.right.end);
                }
            }
        }
    }

    pub fn identical(&self) -> bool {
        self.fragments.is_empty()
    }
}

pub type SharedDiff = Arc<FileDiff>;

/// What a single change of a diff can be sent to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeAction {
    /// Left is the index, right the working tree: move a change into the index.
    Stage,
    /// Left is HEAD, right the index: take a change out of the index.
    Unstage,
}

/// One file of a diff window: which revisions to compare and how to label them.
#[derive(Clone, Debug)]
pub struct DiffItem {
    pub change: FileChange,
    /// `None` shows an absent file (added/deleted).
    pub left: Option<Revision>,
    pub right: Option<Revision>,
    pub left_label: SharedString,
    pub right_label: SharedString,
    pub action: Option<ChangeAction>,
}

impl DiffItem {
    /// `change` in `commit` against its first parent.
    pub fn for_commit(change: FileChange, parent: Option<ObjectId>, commit: ObjectId) -> Self {
        let short = |id: ObjectId| SharedString::from(id.to_hex_with_len(8).to_string());
        Self {
            left: parent.filter(|_| change.kind != ChangeKind::Added).map(Revision::Commit),
            right: (change.kind != ChangeKind::Deleted).then_some(Revision::Commit(commit)),
            left_label: parent.map(short).unwrap_or_else(|| "(no parent)".into()),
            right_label: short(commit),
            action: None,
            change,
        }
    }
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8000)].contains(&0)
}

/// Highlighter language for a path (by file name, then by extension).
pub fn language_for(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    let by_name = match name {
        "Makefile" | "makefile" | "GNUmakefile" => Some("make"),
        "CMakeLists.txt" => Some("cmake"),
        "Cargo.lock" => Some("toml"),
        _ => None,
    };
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let alias = match ext.as_str() {
        "h" => "c",
        "cc" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "jsx" | "mjs" | "cjs" => "javascript",
        "mts" | "cts" => "typescript",
        "sh" | "bash" | "zsh" => "bash",
        "htm" | "xhtml" => "html",
        "scss" => "css",
        "exs" => "elixir",
        "mk" => "make",
        "cmake" => "cmake",
        "jsonc" | "json5" => "json",
        e => e,
    };
    let lang = Language::from_str(by_name.unwrap_or(alias));
    if matches!(lang, Language::Plain) { "text" } else { lang.name() }
}
