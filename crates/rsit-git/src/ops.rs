//! Branch, remote and history operations (IntelliJ Git menu), all through the
//! `git` CLI. Long operations report git's progress lines.

use std::io::Read as _;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context as _, Result, bail};

use crate::Repo;
use crate::cli::{git, run};

// ---- branches ----

/// Checks out a local branch, or creates a tracking branch for `origin/x`.
pub fn checkout_branch(cwd: &Path, name: &str, remote: bool) -> Result<()> {
    if remote {
        let local = name.split_once('/').map_or(name, |(_, b)| b);
        let exists = run(cwd, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{local}")]).is_ok();
        if exists {
            // IntelliJ offers "checkout and reset"; switching to the existing branch is the safe choice
            return run(cwd, &["checkout", local]).map(drop);
        }
        return run(cwd, &["checkout", "--track", name]).map(drop);
    }
    run(cwd, &["checkout", name]).map(drop)
}

pub fn rename_branch(cwd: &Path, old: &str, new: &str) -> Result<()> {
    run(cwd, &["branch", "-m", old, new]).map(drop)
}

/// Deletes a local branch; `force` deletes it even if it is not merged.
pub fn delete_branch(cwd: &Path, name: &str, force: bool) -> Result<()> {
    run(cwd, &["branch", if force { "-D" } else { "-d" }, name]).map(drop)
}

/// Deletes `origin/x` on the remote.
pub fn delete_remote_branch(cwd: &Path, remote_branch: &str, progress: impl FnMut(&str)) -> Result<()> {
    let (remote, branch) = remote_branch.split_once('/').context("not a remote branch")?;
    run_with_progress(cwd, &["push", "--progress", remote, "--delete", branch], progress).map(drop)
}

pub fn delete_tag(cwd: &Path, name: &str) -> Result<()> {
    run(cwd, &["tag", "-d", name]).map(drop)
}

pub fn merge(cwd: &Path, rev: &str) -> Result<String> {
    run(cwd, &["merge", "--no-edit", rev])
}

pub fn rebase(cwd: &Path, onto: &str) -> Result<String> {
    run(cwd, &["rebase", onto])
}

pub fn cherry_pick(cwd: &Path, commits: &[String]) -> Result<String> {
    let mut args = vec!["cherry-pick", "-x"];
    args.extend(commits.iter().map(String::as_str));
    run(cwd, &args)
}

pub fn revert(cwd: &Path, commits: &[String]) -> Result<String> {
    let mut args = vec!["revert", "--no-edit"];
    args.extend(commits.iter().map(String::as_str));
    run(cwd, &args)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
    Keep,
}

impl ResetMode {
    pub fn flag(self) -> &'static str {
        match self {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
            ResetMode::Keep => "--keep",
        }
    }
}

pub fn reset(cwd: &Path, rev: &str, mode: ResetMode) -> Result<()> {
    run(cwd, &["reset", mode.flag(), rev]).map(drop)
}

/// IntelliJ "Undo Commit": moves HEAD back, keeping the changes staged.
pub fn undo_last_commit(cwd: &Path) -> Result<()> {
    run(cwd, &["reset", "--soft", "HEAD~1"]).map(drop)
}

// ---- remotes ----

pub fn fetch(cwd: &Path, progress: impl FnMut(&str)) -> Result<String> {
    run_with_progress(cwd, &["fetch", "--all", "--prune", "--progress"], progress)
}

/// IntelliJ "Update Project": pull the current branch, rebasing or merging as
/// configured by `pull.rebase` (merge by default).
pub fn pull(cwd: &Path, progress: impl FnMut(&str)) -> Result<String> {
    run_with_progress(cwd, &["pull", "--progress", "--no-edit"], progress)
}

/// Pushes the current branch; sets the upstream on its first push.
pub fn push(cwd: &Path, remote: &str, branch: &str, set_upstream: bool, force: bool, progress: impl FnMut(&str)) -> Result<String> {
    let mut args = vec!["push", "--progress", "--porcelain"];
    if set_upstream {
        args.push("--set-upstream");
    }
    if force {
        args.push("--force-with-lease");
    }
    let refspec = format!("{branch}:{branch}");
    args.push(remote);
    args.push(&refspec);
    run_with_progress(cwd, &args, progress)
}

pub fn remotes(cwd: &Path) -> Result<Vec<String>> {
    Ok(run(cwd, &["remote"])?.lines().map(str::to_string).collect())
}

/// Upstream of a local branch (`origin/main`), if configured.
pub fn upstream(cwd: &Path, branch: &str) -> Option<String> {
    run(cwd, &["rev-parse", "--abbrev-ref", &format!("{branch}@{{upstream}}")]).ok().map(|s| s.trim().to_string())
}

/// Commits ahead/behind of `branch` relative to its upstream.
pub fn ahead_behind(cwd: &Path, branch: &str) -> Option<(u32, u32)> {
    let out = run(cwd, &["rev-list", "--left-right", "--count", &format!("{branch}...{branch}@{{upstream}}")]).ok()?;
    let mut it = out.split_whitespace().filter_map(|n| n.parse().ok());
    Some((it.next()?, it.next()?))
}

/// Runs git and reports each progress line (git ends them with `\r` or `\n`).
pub fn run_with_progress(cwd: &Path, args: &[&str], mut progress: impl FnMut(&str)) -> Result<String> {
    let mut child = git(cwd).args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let mut stdout = child.stdout.take().context("no stdout")?;
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).ok();
        buf
    });
    let mut stderr = child.stderr.take().context("no stderr")?;
    let mut all = Vec::new();
    let mut line = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stderr.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        for &b in &chunk[..n] {
            all.push(b);
            if b == b'\r' || b == b'\n' {
                if !line.is_empty() {
                    progress(String::from_utf8_lossy(&line).trim());
                    line.clear();
                }
            } else {
                line.push(b);
            }
        }
    }
    let status = child.wait()?;
    let stdout = out_thread.join().unwrap_or_default();
    if !status.success() {
        let stderr = String::from_utf8_lossy(&all);
        // the last lines carry the error; progress noise comes first
        let tail: Vec<&str> =
            stderr.lines().flat_map(|l| l.split('\r')).map(str::trim).filter(|l| !l.is_empty()).collect();
        let tail = tail[tail.len().saturating_sub(6)..].join("\n");
        bail!("git {} failed:\n{tail}", args.first().copied().unwrap_or(""));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

// ---- operations in progress ----

/// A merge/rebase/cherry-pick/revert stopped half-way (usually on conflicts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Other,
}

impl Operation {
    pub fn name(self) -> &'static str {
        match self {
            Operation::Merge => "Merge",
            Operation::Rebase => "Rebase",
            Operation::CherryPick => "Cherry-pick",
            Operation::Revert => "Revert",
            Operation::Other => "Operation",
        }
    }

    fn command(self) -> Option<&'static str> {
        match self {
            Operation::Merge => Some("merge"),
            Operation::Rebase => Some("rebase"),
            Operation::CherryPick => Some("cherry-pick"),
            Operation::Revert => Some("revert"),
            Operation::Other => None,
        }
    }
}

pub fn operation_in_progress(repo: &Repo) -> Option<Operation> {
    use gix::state::InProgress as P;
    Some(match repo.local().state()? {
        P::Merge => Operation::Merge,
        P::Rebase | P::RebaseInteractive | P::ApplyMailboxRebase => Operation::Rebase,
        P::CherryPick | P::CherryPickSequence => Operation::CherryPick,
        P::Revert | P::RevertSequence => Operation::Revert,
        _ => Operation::Other,
    })
}

/// `git <op> --continue` (merge continues by committing).
pub fn continue_operation(cwd: &Path, op: Operation) -> Result<String> {
    let cmd = op.command().context("cannot continue this operation")?;
    // no editor: keep git's prepared message
    let out = git(cwd).env("GIT_EDITOR", "true").args([cmd, "--continue"]).output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        bail!("{}", if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn abort_operation(cwd: &Path, op: Operation) -> Result<()> {
    let cmd = op.command().context("cannot abort this operation")?;
    run(cwd, &[cmd, "--abort"]).map(drop)
}
