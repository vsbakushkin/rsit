//! Local changes: status of the index and the working tree, staging, rollback
//! and commit. Everything goes through the `git` CLI so hooks, filters and
//! fsmonitor behave exactly as in the terminal.

use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context as _, Result, bail};

use crate::ChangeKind;
use crate::cli::{git, run};

/// One path in `git status`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    /// Source path of a staged rename or copy.
    pub orig_path: Option<String>,
    /// Change between HEAD and the index.
    pub staged: Option<ChangeKind>,
    /// Change between the index and the working tree.
    pub unstaged: Option<ChangeKind>,
    pub untracked: bool,
    /// Unmerged path (merge/rebase conflict).
    pub conflicted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub entries: Vec<StatusEntry>,
    /// `false` before the first commit.
    pub has_head: bool,
}

impl Status {
    pub fn staged(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.staged.is_some() && !e.conflicted)
    }

    pub fn unstaged(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.unstaged.is_some() && !e.conflicted)
    }

    pub fn untracked(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.untracked)
    }

    pub fn conflicted(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.conflicted)
    }
}

pub fn status(cwd: &Path) -> Result<Status> {
    let out = git(cwd)
        .args(["status", "--porcelain=v2", "-z", "--untracked-files=all", "--branch"])
        .output()
        .context("cannot run git status")?;
    if !out.status.success() {
        bail!("git status failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    parse_status(&out.stdout)
}

fn kind(code: u8) -> Option<ChangeKind> {
    match code {
        b'M' | b'T' => Some(ChangeKind::Modified),
        b'A' => Some(ChangeKind::Added),
        b'D' => Some(ChangeKind::Deleted),
        b'R' => Some(ChangeKind::Renamed),
        b'C' => Some(ChangeKind::Copied),
        _ => None,
    }
}

/// Parses `git status --porcelain=v2 -z --branch`.
pub fn parse_status(bytes: &[u8]) -> Result<Status> {
    let mut status = Status { entries: Vec::new(), has_head: true };
    let mut records = bytes.split(|&b| b == 0).filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        let text = String::from_utf8_lossy(record);
        let field = |n: usize| -> Result<(&str, &str)> {
            // the first `n` space-separated fields, then the rest (a path may contain spaces)
            let mut split = text.splitn(n + 1, ' ');
            let fields: Vec<&str> = (&mut split).take(n).collect();
            let rest = split.next().context("short status record")?;
            Ok((fields.get(1).copied().unwrap_or(""), rest))
        };
        match record[0] {
            b'#' => {
                if text == "# branch.oid (initial)" {
                    status.has_head = false;
                }
            }
            b'1' => {
                let (xy, path) = field(8)?;
                status.entries.push(entry(xy, path.to_string(), None));
            }
            b'2' => {
                let (xy, path) = field(9)?;
                let orig = records.next().map(|o| String::from_utf8_lossy(o).into_owned());
                status.entries.push(entry(xy, path.to_string(), orig));
            }
            b'u' => {
                let (_, path) = field(10)?;
                status.entries.push(StatusEntry {
                    path: path.to_string(),
                    orig_path: None,
                    staged: None,
                    unstaged: None,
                    untracked: false,
                    conflicted: true,
                });
            }
            b'?' => status.entries.push(StatusEntry {
                path: text[2..].to_string(),
                orig_path: None,
                staged: None,
                unstaged: None,
                untracked: true,
                conflicted: false,
            }),
            _ => {} // '!' ignored files
        }
    }
    status.entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(status)
}

fn entry(xy: &str, path: String, orig_path: Option<String>) -> StatusEntry {
    let xy = xy.as_bytes();
    StatusEntry {
        path,
        orig_path,
        staged: xy.first().copied().and_then(kind),
        unstaged: xy.get(1).copied().and_then(kind),
        untracked: false,
        conflicted: false,
    }
}

fn with_paths<'a>(args: &[&'a str], paths: &'a [String]) -> Vec<&'a str> {
    let mut all = args.to_vec();
    all.push("--");
    all.extend(paths.iter().map(String::as_str));
    all
}

/// `git add` (also stages deletions and untracked files).
pub fn stage(cwd: &Path, paths: &[String]) -> Result<()> {
    run(cwd, &with_paths(&["add", "--all"], paths)).map(drop)
}

/// Removes paths from the index, keeping the working tree.
pub fn unstage(cwd: &Path, paths: &[String], has_head: bool) -> Result<()> {
    if has_head {
        run(cwd, &with_paths(&["restore", "--staged"], paths)).map(drop)
    } else {
        run(cwd, &with_paths(&["rm", "--cached", "-r", "-q"], paths)).map(drop)
    }
}

/// Discards working tree changes of tracked paths (IntelliJ "Rollback").
/// With `staged` also resets the index to HEAD for them.
pub fn rollback(cwd: &Path, paths: &[String], staged: bool) -> Result<()> {
    let args: &[&str] = if staged { &["restore", "--staged", "--worktree", "--source=HEAD"] } else { &["restore", "--worktree"] };
    run(cwd, &with_paths(args, paths)).map(drop)
}

/// Deletes untracked files from disk.
pub fn delete_untracked(cwd: &Path, paths: &[String]) -> Result<()> {
    for p in paths {
        std::fs::remove_file(cwd.join(p)).with_context(|| format!("cannot delete {p}"))?;
    }
    Ok(())
}

/// Commits the index with `message` (passed on stdin, so any text is safe).
pub fn commit(cwd: &Path, message: &str, amend: bool) -> Result<String> {
    let mut args = vec!["commit", "--file=-", "--cleanup=strip"];
    if amend {
        args.push("--amend");
    }
    run_with_stdin(cwd, &args, message.as_bytes())
}

/// Full message of the last commit (to prefill "Amend").
pub fn last_commit_message(cwd: &Path) -> Result<String> {
    Ok(run(cwd, &["log", "-1", "--format=%B"])?.trim_end().to_string())
}

/// Applies a patch to the index only (`git apply --cached`); used to stage or
/// unstage single changes.
pub fn apply_to_index(cwd: &Path, patch: &str, reverse: bool) -> Result<()> {
    let mut args = vec!["apply", "--cached", "--unidiff-zero", "--whitespace=nowarn"];
    if reverse {
        args.push("--reverse");
    }
    args.push("-");
    run_with_stdin(cwd, &args, patch.as_bytes()).map(drop)
}

pub fn run_with_stdin(cwd: &Path, args: &[&str], input: &[u8]) -> Result<String> {
    let mut child = git(cwd).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    child.stdin.take().context("no stdin")?.write_all(input)?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        // hooks print to either stream
        let detail = if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() };
        bail!("git {} failed: {detail}", args.first().copied().unwrap_or(""));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2() {
        let raw = b"# branch.oid 1234\0# branch.head main\0\
1 M. N... 100644 100644 100644 aaa bbb staged.rs\0\
1 .M N... 100644 100644 100644 aaa aaa dir/with space.rs\0\
1 AM N... 000000 100644 100644 000 bbb both.rs\0\
2 R. N... 100644 100644 100644 aaa aaa R100 new.rs\0old.rs\0\
u UU N... 100644 100644 100644 100644 a b c conflict.rs\0\
? new file.txt\0";
        let s = parse_status(raw).unwrap();
        assert!(s.has_head);
        let get = |p: &str| s.entries.iter().find(|e| e.path == p).unwrap().clone();
        assert_eq!(get("staged.rs").staged, Some(ChangeKind::Modified));
        assert_eq!(get("staged.rs").unstaged, None);
        assert_eq!(get("dir/with space.rs").unstaged, Some(ChangeKind::Modified));
        let both = get("both.rs");
        assert_eq!((both.staged, both.unstaged), (Some(ChangeKind::Added), Some(ChangeKind::Modified)));
        let renamed = get("new.rs");
        assert_eq!((renamed.staged, renamed.orig_path.as_deref()), (Some(ChangeKind::Renamed), Some("old.rs")));
        assert!(get("conflict.rs").conflicted);
        assert!(get("new file.txt").untracked);
        assert_eq!(s.staged().count(), 3);
        assert_eq!(s.unstaged().count(), 2);

        let initial = parse_status(b"# branch.oid (initial)\0? a\0").unwrap();
        assert!(!initial.has_head);
    }
}
