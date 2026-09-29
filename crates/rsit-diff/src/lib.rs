//! Text comparison for the diff and merge viewers (after IntelliJ `diff-impl`).

pub mod compare;
pub mod merge;
pub mod patch;
pub mod rows;

pub use compare::{ChangeKind, LineFragment, Lines, WhitespacePolicy, WordFragment, compare, compare_lines};
pub use patch::fragment_patch;
pub use rows::{Change, DiffRows, Folding, Layout, Row, build_rows};
