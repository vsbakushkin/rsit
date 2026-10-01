//! Merge conflicts: the three versions of a conflicted file, whole-file
//! resolutions and saving a resolved result.

use std::path::Path;

use anyhow::{Context as _, Result};

use crate::Repo;
use crate::cli::run;
use crate::ops::{Operation, operation_in_progress};

/// Versions of a conflicted file from the index stages; `None` when a side
/// does not have the file (added by one side, deleted by the other).
#[derive(Clone, Debug, Default)]
pub struct ConflictVersions {
    pub base: Option<Vec<u8>>,
    /// Stage 2: the checked out side.
    pub ours: Option<Vec<u8>>,
    /// Stage 3: the side being merged in.
    pub theirs: Option<Vec<u8>>,
}

impl ConflictVersions {
    pub fn is_binary(&self) -> bool {
        [&self.base, &self.ours, &self.theirs]
            .iter()
            .any(|v| v.as_ref().is_some_and(|b| b[..b.len().min(8000)].contains(&0)))
    }

    /// Both sides have the file: a text merge is possible.
    pub fn mergeable(&self) -> bool {
        self.ours.is_some() && self.theirs.is_some() && !self.is_binary()
    }
}

pub fn conflict_versions(repo: &Repo, path: &str) -> Result<ConflictVersions> {
    use gix::index::entry::Stage;
    let local = repo.local();
    let index = local.index_or_empty()?;
    let mut versions = ConflictVersions::default();
    for entry in index.entries() {
        if entry.path(&index) != path {
            continue;
        }
        let data = Some(local.find_object(entry.id)?.detach().data);
        match entry.stage() {
            Stage::Base => versions.base = data,
            Stage::Ours => versions.ours = data,
            Stage::Theirs => versions.theirs = data,
            Stage::Unconflicted => {}
        }
    }
    Ok(versions)
}

/// Labels for the two sides, the way IntelliJ names them for the running operation.
pub fn side_labels(repo: &Repo) -> (String, String) {
    let cwd = repo.cwd();
    let short = |rev: &str| run(cwd, &["rev-parse", "--short", rev]).ok().map(|s| s.trim().to_string());
    let branch = run(cwd, &["branch", "--show-current"]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    match operation_in_progress(repo) {
        // during a rebase "ours" is the branch being rebased onto
        Some(Operation::Rebase) => {
            let onto = short("HEAD").unwrap_or_default();
            let commit = short("REBASE_HEAD").unwrap_or_default();
            (format!("Upstream ({onto})"), format!("Your commit ({commit})"))
        }
        Some(Operation::CherryPick) => (
            format!("Yours ({})", branch.unwrap_or_else(|| "HEAD".into())),
            format!("Cherry-picked ({})", short("CHERRY_PICK_HEAD").unwrap_or_default()),
        ),
        Some(Operation::Revert) => (
            format!("Yours ({})", branch.unwrap_or_else(|| "HEAD".into())),
            format!("Reverted ({})", short("REVERT_HEAD").unwrap_or_default()),
        ),
        _ => {
            let theirs = merge_head_name(repo).or_else(|| short("MERGE_HEAD")).unwrap_or_else(|| "theirs".into());
            (format!("Yours ({})", branch.unwrap_or_else(|| "HEAD".into())), format!("Theirs ({theirs})"))
        }
    }
}

/// The branch named in MERGE_MSG ("Merge branch 'x'").
fn merge_head_name(repo: &Repo) -> Option<String> {
    let msg = std::fs::read_to_string(repo.git_dir().join("MERGE_MSG")).ok()?;
    let first = msg.lines().next()?;
    let start = first.find('\'')? + 1;
    let end = start + first[start..].find('\'')?;
    Some(first[start..end].to_string())
}

/// Resolves by taking one side for the whole file (`ours` = stage 2).
pub fn accept_side(repo: &Repo, path: &str, ours: bool) -> Result<()> {
    let cwd = repo.cwd();
    let versions = conflict_versions(repo, path)?;
    let side = if ours { &versions.ours } else { &versions.theirs };
    match side {
        Some(_) => {
            run(cwd, &["checkout", if ours { "--ours" } else { "--theirs" }, "--", path])?;
            run(cwd, &["add", "--", path])?;
        }
        // that side deleted the file
        None => {
            run(cwd, &["rm", "--quiet", "--", path])?;
        }
    }
    Ok(())
}

/// Writes the merged text and marks the file resolved.
pub fn save_resolved(repo: &Repo, path: &str, content: &str) -> Result<()> {
    let workdir = repo.workdir().context("bare repository")?;
    let content = crate::to_worktree(&repo.local(), path, content.as_bytes())?;
    std::fs::write(workdir.join(path), content).with_context(|| format!("cannot write {path}"))?;
    run(repo.cwd(), &["add", "--", path])?;
    Ok(())
}

/// Paths that are still conflicted.
pub fn conflicted_paths(cwd: &Path) -> Result<Vec<String>> {
    Ok(crate::changes::status(cwd)?.conflicted().map(|e| e.path.clone()).collect())
}
