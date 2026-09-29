//! Interactive rebase (IntelliJ "Interactively Rebase from Here"): a todo list
//! prepared in the UI and handed to `git rebase -i` through
//! `GIT_SEQUENCE_EDITOR`, with new messages applied by `exec` lines.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::ObjectId;
use crate::cli::{git, run};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Pick,
    /// Pick with a new message.
    Reword,
    /// Stop after applying, to amend the commit.
    Edit,
    /// Meld into the previous commit, combining messages.
    Squash,
    /// Meld into the previous commit, discarding this message.
    Fixup,
    Drop,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Pick => "Pick",
            Action::Reword => "Reword",
            Action::Edit => "Edit",
            Action::Squash => "Squash",
            Action::Fixup => "Fixup",
            Action::Drop => "Drop",
        }
    }

    fn melds(self) -> bool {
        matches!(self, Action::Squash | Action::Fixup)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TodoEntry {
    pub commit: ObjectId,
    pub message: String,
    pub author: String,
    pub time: i64,
    pub action: Action,
    /// Replacement message: for Reword, or for a commit that others are squashed into.
    pub new_message: Option<String>,
}

impl TodoEntry {
    pub fn subject(&self) -> &str {
        self.new_message.as_deref().unwrap_or(&self.message).lines().next().unwrap_or("")
    }
}

/// Commits to rebase, oldest first (git's todo order).
#[derive(Clone, Debug)]
pub struct RebasePlan {
    /// Commit to rebase onto; `None` rewrites from the root commit.
    pub base: Option<ObjectId>,
    pub entries: Vec<TodoEntry>,
}

impl RebasePlan {
    /// Commits from `from` (inclusive) up to HEAD (IntelliJ "from here").
    pub fn from_commit(cwd: &Path, from: ObjectId) -> Result<Self> {
        let from = from.to_string();
        if run(cwd, &["merge-base", "--is-ancestor", &from, "HEAD"]).is_err() {
            bail!("the commit is not on the current branch");
        }
        let base = run(cwd, &["rev-parse", "--verify", "--quiet", &format!("{from}^")])
            .ok()
            .and_then(|s| ObjectId::from_hex(s.trim().as_bytes()).ok());
        Self::after(cwd, base)
    }

    /// Commits after `base` up to HEAD, like `git rebase -i <base>`; `None` from the root.
    pub fn after(cwd: &Path, base: Option<ObjectId>) -> Result<Self> {
        if let Some(base) = base {
            if run(cwd, &["merge-base", "--is-ancestor", &base.to_string(), "HEAD"]).is_err() {
                bail!("{} is not an ancestor of HEAD", base.to_hex_with_len(8));
            }
        }
        let range = match base {
            Some(base) => format!("{base}..HEAD"),
            None => "HEAD".to_string(),
        };
        let out = run(cwd, &["log", "--reverse", "--topo-order", "--format=%x1e%H%x1f%P%x1f%an%x1f%at%x1f%B", &range])?;
        let mut entries = Vec::new();
        for record in out.split('\x1e').filter(|r| !r.is_empty()) {
            let fields: Vec<&str> = record.splitn(5, '\x1f').collect();
            if fields.len() < 5 {
                continue;
            }
            if fields[1].split_whitespace().count() > 1 {
                bail!("interactive rebase of merge commits is not supported");
            }
            entries.push(TodoEntry {
                commit: ObjectId::from_hex(fields[0].as_bytes()).ok().context("bad commit id")?,
                author: fields[2].to_string(),
                time: fields[3].parse().unwrap_or_default(),
                message: fields[4].trim_end().to_string(),
                action: Action::Pick,
                new_message: None,
            });
        }
        if entries.is_empty() {
            bail!("nothing to rebase");
        }
        Ok(Self { base, entries })
    }

    /// Why the plan cannot run, if it cannot.
    pub fn validate(&self) -> Option<String> {
        let first = self.entries.iter().find(|e| e.action != Action::Drop)?;
        if first.action.melds() {
            return Some(format!("'{}' has no earlier commit to be melded into", first.subject()));
        }
        None
    }

    /// Index of the commit that `index` is melded into (for Squash/Fixup).
    pub fn meld_target(&self, index: usize) -> Option<usize> {
        (0..index).rev().find(|&i| !matches!(self.entries[i].action, Action::Drop | Action::Squash | Action::Fixup))
    }

    /// Default message of a commit with squashed commits: all messages, like git.
    pub fn combined_message(&self, target: usize) -> String {
        let mut parts =
            vec![self.entries[target].new_message.clone().unwrap_or_else(|| self.entries[target].message.clone())];
        for e in &self.entries[target + 1..] {
            match e.action {
                Action::Squash => parts.push(e.message.clone()),
                Action::Fixup | Action::Drop => {}
                _ => break,
            }
        }
        parts.join("\n\n")
    }

    /// The todo list for git, writing new messages into `dir`.
    pub fn todo(&self, dir: &Path) -> Result<String> {
        let mut todo = String::new();
        let mut pending_message: Option<PathBuf> = None;
        let flush = |todo: &mut String, pending: &mut Option<PathBuf>| {
            if let Some(file) = pending.take() {
                let quoted = file.display().to_string().replace('\'', "'\\''");
                todo.push_str(&format!("exec git commit --amend --only --allow-empty --cleanup=strip -F '{quoted}'\n"));
            }
        };
        for (i, e) in self.entries.iter().enumerate() {
            if !e.action.melds() && e.action != Action::Drop {
                flush(&mut todo, &mut pending_message);
            }
            let cmd = match e.action {
                Action::Pick | Action::Reword => "pick",
                Action::Edit => "edit",
                Action::Squash => "squash",
                Action::Fixup => "fixup",
                Action::Drop => "drop",
            };
            todo.push_str(&format!("{cmd} {} {}\n", e.commit, e.subject()));
            if let Some(message) = &e.new_message {
                if !e.action.melds() && e.action != Action::Drop {
                    let file = dir.join(format!("message-{i}.txt"));
                    std::fs::write(&file, message)?;
                    pending_message = Some(file);
                }
            }
        }
        flush(&mut todo, &mut pending_message);
        Ok(todo)
    }
}

/// Runs the rebase. Returns when git finished or stopped (edit, conflicts):
/// a stop is reported as an error carrying git's message.
pub fn run_interactive(cwd: &Path, plan: &RebasePlan) -> Result<String> {
    if let Some(problem) = plan.validate() {
        bail!(problem);
    }
    // messages must outlive this call when the rebase stops half-way; one
    // directory per run so concurrent rebases never share a todo
    static RUN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let run_id = RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("rsit-rebase-{}-{run_id}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let todo_file = dir.join("todo");
    std::fs::write(&todo_file, plan.todo(&dir)?)?;
    let quoted = todo_file.display().to_string().replace('\'', "'\\''");
    let mut cmd = git(cwd);
    cmd.env("GIT_SEQUENCE_EDITOR", format!("cp '{quoted}'"))
        .args(["-c", "rebase.abbreviateCommands=false", "-c", "rebase.missingCommitsCheck=ignore"])
        .args(["rebase", "--interactive", "--autostash"]);
    match plan.base {
        Some(base) => cmd.arg(base.to_string()),
        None => cmd.arg("--root"),
    };
    let out = cmd.output()?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    // keep the messages while the rebase is stopped (its exec lines still need them)
    let stopped =
        run(cwd, &["rev-parse", "--git-path", "rebase-merge"]).map(|p| cwd.join(p.trim()).exists()).unwrap_or(true);
    if !stopped {
        std::fs::remove_dir_all(&dir).ok();
    }
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("{}", if stderr.trim().is_empty() { stdout.trim().to_string() } else { stderr.trim().to_string() });
    }
    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: u8, action: Action) -> TodoEntry {
        TodoEntry {
            commit: ObjectId::from_bytes_or_panic(&[n; 20]),
            message: format!("commit {n}\n\nbody {n}"),
            author: "A".into(),
            time: 0,
            action,
            new_message: None,
        }
    }

    #[test]
    fn todo_lines() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = RebasePlan {
            base: None,
            entries: vec![
                entry(1, Action::Reword),
                entry(2, Action::Squash),
                entry(3, Action::Drop),
                entry(4, Action::Edit),
            ],
        };
        plan.entries[0].new_message = Some("new".into());
        let todo = plan.todo(dir.path()).unwrap();
        let lines: Vec<&str> = todo.lines().map(|l| l.split(' ').next().unwrap()).collect();
        assert_eq!(lines, ["pick", "squash", "drop", "exec", "edit"]);
        assert!(todo.contains("message-0.txt"));
        assert_eq!(std::fs::read_to_string(dir.path().join("message-0.txt")).unwrap(), "new");
        assert_eq!(plan.meld_target(1), Some(0));
        assert_eq!(plan.combined_message(0), "new\n\ncommit 2\n\nbody 2");
    }

    #[test]
    fn validation() {
        let plan = RebasePlan { base: None, entries: vec![entry(1, Action::Drop), entry(2, Action::Fixup)] };
        assert!(plan.validate().is_some());
        let plan = RebasePlan { base: None, entries: vec![entry(1, Action::Pick), entry(2, Action::Fixup)] };
        assert!(plan.validate().is_none());
    }
}
