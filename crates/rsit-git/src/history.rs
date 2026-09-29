//! History of one file across renames (IntelliJ "Show History"), and line
//! annotations (IntelliJ "Annotate with Git Blame").

use std::path::Path;

use anyhow::{Context as _, Result, bail};

use crate::cli::git;
use crate::{ChangeKind, ObjectId};

/// One commit touching the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRevision {
    pub commit: ObjectId,
    pub parents: Vec<ObjectId>,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub subject: String,
    pub kind: ChangeKind,
    /// Path of the file in this commit.
    pub path: String,
    /// Path before the commit, for renames and copies.
    pub old_path: Option<String>,
}

/// Commits that changed `path`, newest first, following renames. `rev` limits
/// the history to what is reachable from it (default: HEAD).
pub fn file_history(cwd: &Path, path: &str, rev: Option<&str>) -> Result<Vec<FileRevision>> {
    let mut cmd = git(cwd);
    cmd.args(["log", "--follow", "-M", "--name-status", "-z", "--format=%x1e%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s"]);
    cmd.arg(rev.unwrap_or("HEAD"));
    cmd.args(["--", path]);
    let out = cmd.output().context("cannot run git log")?;
    if !out.status.success() {
        bail!("git log failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    parse_history(&out.stdout)
}

fn parse_history(bytes: &[u8]) -> Result<Vec<FileRevision>> {
    let mut revisions = Vec::new();
    for record in bytes.split(|&b| b == 0x1e).filter(|r| !r.is_empty()) {
        let mut parts = record.split(|&b| b == 0);
        let header = String::from_utf8_lossy(parts.next().unwrap_or_default()).into_owned();
        let fields: Vec<&str> = header.trim_end_matches('\n').split('\x1f').collect();
        if fields.len() < 6 {
            continue;
        }
        let commit = ObjectId::from_hex(fields[0].as_bytes()).ok().context("bad commit id")?;
        let parents = fields[1].split_whitespace().filter_map(|p| ObjectId::from_hex(p.as_bytes()).ok()).collect();
        let mut tokens =
            parts.map(|t| String::from_utf8_lossy(t).trim_start_matches('\n').to_string()).filter(|t| !t.is_empty());
        // merge commits may list no files; keep them out like IntelliJ does
        let Some(status) = tokens.next() else { continue };
        let (kind, old_path, path) = match status.as_bytes().first() {
            Some(b'R' | b'C') => {
                let kind = if status.starts_with('R') { ChangeKind::Renamed } else { ChangeKind::Copied };
                let old = tokens.next().context("rename without source")?;
                (kind, Some(old), tokens.next().context("rename without target")?)
            }
            Some(b'A') => (ChangeKind::Added, None, tokens.next().context("missing path")?),
            Some(b'D') => (ChangeKind::Deleted, None, tokens.next().context("missing path")?),
            _ => (ChangeKind::Modified, None, tokens.next().context("missing path")?),
        };
        revisions.push(FileRevision {
            commit,
            parents,
            author: fields[2].to_string(),
            email: fields[3].to_string(),
            time: fields[4].parse().unwrap_or_default(),
            subject: fields[5].to_string(),
            kind,
            path,
            old_path,
        });
    }
    Ok(revisions)
}

/// Author information of one commit in a blame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameCommit {
    pub id: ObjectId,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub summary: String,
    /// Parent commit and path to annotate the previous revision of these lines.
    pub previous: Option<(ObjectId, String)>,
    /// Path of the file in this commit.
    pub filename: String,
    /// Lines that are not committed yet (`0000000…`).
    pub uncommitted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    /// Index into [`Blame::commits`].
    pub commit: usize,
    /// 1-based line number in the commit that introduced it.
    pub orig_line: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Blame {
    pub commits: Vec<BlameCommit>,
    /// One entry per line of the annotated text.
    pub lines: Vec<BlameLine>,
    /// The annotated text.
    pub text: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlameOptions {
    pub ignore_whitespace: bool,
    /// Detect lines moved or copied within the file (`-M`).
    pub detect_moves: bool,
    /// ... and across files of the same commit (`-C`).
    pub detect_copies: bool,
}

/// Annotates `path` at `rev`, or the working tree version when `rev` is `None`.
pub fn blame(cwd: &Path, path: &str, rev: Option<&str>, options: BlameOptions) -> Result<Blame> {
    let mut cmd = git(cwd);
    cmd.args(["blame", "--porcelain"]);
    if options.ignore_whitespace {
        cmd.arg("-w");
    }
    if options.detect_moves {
        cmd.arg("-M");
    }
    if options.detect_copies {
        cmd.arg("-C");
    }
    if let Some(rev) = rev {
        cmd.arg(rev);
    }
    cmd.args(["--", path]);
    let out = cmd.output().context("cannot run git blame")?;
    if !out.status.success() {
        bail!("git blame failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    parse_blame(&out.stdout)
}

pub fn parse_blame(bytes: &[u8]) -> Result<Blame> {
    let mut blame = Blame::default();
    let mut index_of: std::collections::HashMap<ObjectId, usize> = std::collections::HashMap::new();
    let mut current: Option<(usize, u32)> = None;
    for raw in bytes.split(|&b| b == b'\n') {
        if let Some(content) = raw.strip_prefix(b"\t") {
            let (commit, orig_line) = current.context("content before header")?;
            blame.lines.push(BlameLine { commit, orig_line });
            blame.text.push_str(&String::from_utf8_lossy(content));
            blame.text.push('\n');
            continue;
        }
        let line = String::from_utf8_lossy(raw);
        let mut words = line.splitn(2, ' ');
        let key = words.next().unwrap_or_default();
        let value = words.next().unwrap_or_default();
        if key.len() == 40 && key.bytes().all(|b| b.is_ascii_hexdigit()) {
            let id = ObjectId::from_hex(key.as_bytes()).ok().context("bad commit id")?;
            let orig_line = value.split(' ').next().and_then(|n| n.parse().ok()).unwrap_or(0);
            let index = *index_of.entry(id).or_insert_with(|| {
                blame.commits.push(BlameCommit {
                    id,
                    author: String::new(),
                    email: String::new(),
                    time: 0,
                    summary: String::new(),
                    previous: None,
                    filename: String::new(),
                    uncommitted: id.is_null(),
                });
                blame.commits.len() - 1
            });
            current = Some((index, orig_line));
            continue;
        }
        let Some((index, _)) = current else { continue };
        let commit = &mut blame.commits[index];
        match key {
            "author" => commit.author = value.to_string(),
            "author-mail" => commit.email = value.trim_matches(['<', '>']).to_string(),
            "author-time" => commit.time = value.parse().unwrap_or_default(),
            "summary" => commit.summary = value.to_string(),
            "filename" => commit.filename = value.to_string(),
            "previous" => {
                let mut it = value.splitn(2, ' ');
                if let (Some(id), Some(path)) = (it.next(), it.next()) {
                    commit.previous = ObjectId::from_hex(id.as_bytes()).ok().map(|id| (id, path.to_string()));
                }
            }
            _ => {}
        }
    }
    Ok(blame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_history_with_renames() {
        let a = "1111111111111111111111111111111111111111";
        let b = "2222222222222222222222222222222222222222";
        let raw = format!(
            "\x1e{a}\x1f{b}\x1fAnn\x1fann@x\x1f100\x1frename it\0\nR090\0old name.rs\0new name.rs\0\
             \x1e{b}\x1f\x1fBob\x1fbob@x\x1f50\x1fcreate\0\nA\0old name.rs\0"
        );
        let h = parse_history(raw.as_bytes()).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].kind, ChangeKind::Renamed);
        assert_eq!((h[0].old_path.as_deref(), h[0].path.as_str()), (Some("old name.rs"), "new name.rs"));
        assert_eq!(h[0].parents.len(), 1);
        assert_eq!((h[1].kind, h[1].path.as_str(), h[1].author.as_str()), (ChangeKind::Added, "old name.rs", "Bob"));
    }

    #[test]
    fn parses_porcelain_blame() {
        let a = "1111111111111111111111111111111111111111";
        let z = "0000000000000000000000000000000000000000";
        let raw = format!(
            "{a} 1 1 2\nauthor Ann\nauthor-mail <ann@x>\nauthor-time 100\nsummary first\nprevious {a} old.rs\nfilename f.rs\n\tone\n\
             {a} 2 2\n\ttwo\n\
             {z} 3 3 1\nauthor Not Committed Yet\nauthor-time 200\nsummary Version of f.rs from f.rs\nfilename f.rs\n\tthree\n"
        );
        let b = parse_blame(raw.as_bytes()).unwrap();
        assert_eq!(b.text, "one\ntwo\nthree\n");
        assert_eq!(b.commits.len(), 2);
        assert_eq!(b.lines.iter().map(|l| l.commit).collect::<Vec<_>>(), [0, 0, 1]);
        assert_eq!(b.commits[0].email, "ann@x");
        assert_eq!(b.commits[0].previous.as_ref().map(|p| p.1.as_str()), Some("old.rs"));
        assert!(b.commits[1].uncommitted);
    }
}
