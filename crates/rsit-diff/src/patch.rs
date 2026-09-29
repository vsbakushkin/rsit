//! Zero-context patches for single changes, applied with
//! `git apply --unidiff-zero` to stage or unstage one change.

use crate::compare::{LineFragment, Lines};

/// A patch turning `left` into `right` only inside `fragment`.
pub fn fragment_patch(path: &str, left: &str, right: &str, fragment: &LineFragment) -> String {
    let (l, r) = (Lines::new(left), Lines::new(right));
    let range = |start: u32, len: u32| {
        // for an empty side, git's -U0 convention names the line before the gap
        if len == 0 { format!("{start},0") } else { format!("{},{len}", start + 1) }
    };
    let mut out = format!(
        "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -{} +{} @@\n",
        range(fragment.left.start, fragment.left.len() as u32),
        range(fragment.right.start, fragment.right.len() as u32),
    );
    let mut push = |prefix: char, line: &str| {
        out.push(prefix);
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push_str("\n\\ No newline at end of file\n");
        }
    };
    for i in fragment.left.clone() {
        push('-', l.line(i as usize));
    }
    for i in fragment.right.clone() {
        push('+', r.line(i as usize));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::{WhitespacePolicy, compare};

    #[test]
    fn patches() {
        let (left, right) = ("a\nb\nc\n", "a\nB\nc\nd");
        let f = compare(left, right, WhitespacePolicy::Default);
        assert_eq!(
            fragment_patch("x", left, right, &f[0]),
            "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -2,1 +2,1 @@\n-b\n+B\n"
        );
        assert_eq!(
            fragment_patch("x", left, right, &f[1]),
            "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -3,0 +4,1 @@\n+d\n\\ No newline at end of file\n"
        );
    }
}
