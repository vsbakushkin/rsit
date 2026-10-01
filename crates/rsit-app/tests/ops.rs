//! Branch, remote and history operations against real repositories.

use std::path::Path;
use std::process::Command;

use rsit_git::ops::{self, Operation, ResetMode};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").current_dir(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn init(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "T"]);
    git(dir, &["config", "user.email", "t@e"]);
}

fn commit_file(dir: &Path, file: &str, text: &str, msg: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-qm", msg]);
}

#[test]
fn remote_round_trip() {
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "-q", "--bare", "-b", "main"]);
    let a = tempfile::tempdir().unwrap();
    init(a.path());
    git(a.path(), &["remote", "add", "origin", remote.path().to_str().unwrap()]);
    commit_file(a.path(), "f", "1\n", "one");
    let mut lines = Vec::new();
    ops::push(a.path(), "origin", "main", true, false, |l| lines.push(l.to_string())).unwrap();
    assert_eq!(ops::upstream(a.path(), "main").as_deref(), Some("origin/main"));

    let b = tempfile::tempdir().unwrap();
    git(b.path(), &["clone", "-q", remote.path().to_str().unwrap(), "."]);
    git(b.path(), &["config", "user.name", "T"]);
    git(b.path(), &["config", "user.email", "t@e"]);
    commit_file(a.path(), "f", "2\n", "two");
    ops::push(a.path(), "origin", "main", false, false, |_| {}).unwrap();

    ops::fetch(b.path(), |_| {}).unwrap();
    assert_eq!(ops::ahead_behind(b.path(), "main"), Some((0, 1)));
    ops::pull(b.path(), |_| {}).unwrap();
    // written by git, so CRLF under core.autocrlf=true (Git for Windows' default)
    assert_eq!(std::fs::read_to_string(b.path().join("f")).unwrap().replace("\r\n", "\n"), "2\n");

    // a branch pushed from a, checked out as a tracking branch in b
    git(a.path(), &["checkout", "-qb", "feature"]);
    commit_file(a.path(), "g", "g\n", "feature");
    ops::push(a.path(), "origin", "feature", true, false, |_| {}).unwrap();
    ops::fetch(b.path(), |_| {}).unwrap();
    ops::checkout_branch(b.path(), "origin/feature", true).unwrap();
    assert_eq!(git(b.path(), &["branch", "--show-current"]).trim(), "feature");
    assert_eq!(ops::upstream(b.path(), "feature").as_deref(), Some("origin/feature"));

    ops::delete_remote_branch(b.path(), "origin/feature", |_| {}).unwrap();
    assert!(!git(remote.path(), &["branch"]).contains("feature"));

    // a rejected (non-fast-forward) push reports git's error, not the progress noise
    git(a.path(), &["checkout", "-q", "main"]);
    commit_file(a.path(), "f", "3\n", "three");
    ops::push(a.path(), "origin", "main", false, false, |_| {}).unwrap();
    git(b.path(), &["checkout", "-q", "main"]);
    commit_file(b.path(), "h", "h\n", "b-side");
    let err = ops::push(b.path(), "origin", "main", false, false, |_| {}).unwrap_err().to_string();
    assert!(err.contains("git push failed") && err.contains("rejected"), "{err}");
    // force-with-lease still refuses because b has not fetched a's commit
    assert!(ops::push(b.path(), "origin", "main", false, true, |_| {}).is_err());
}

#[test]
fn merge_conflict_abort_and_continue() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    init(p);
    commit_file(p, "f", "base\n", "base");
    git(p, &["checkout", "-qb", "other"]);
    commit_file(p, "f", "other\n", "other");
    git(p, &["checkout", "-q", "main"]);
    commit_file(p, "f", "main\n", "main");
    let repo = rsit_git::Repo::discover(p).unwrap();

    assert!(ops::merge(p, "other").is_err());
    assert_eq!(ops::operation_in_progress(&repo), Some(Operation::Merge));
    ops::abort_operation(p, Operation::Merge).unwrap();
    assert_eq!(ops::operation_in_progress(&repo), None);

    assert!(ops::merge(p, "other").is_err());
    std::fs::write(p.join("f"), "resolved\n").unwrap();
    git(p, &["add", "f"]);
    ops::continue_operation(p, Operation::Merge).unwrap();
    assert_eq!(ops::operation_in_progress(&repo), None);
    assert_eq!(git(p, &["rev-list", "--parents", "-n1", "HEAD"]).split_whitespace().count(), 3, "merge commit");
}

#[test]
fn history_operations() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    init(p);
    commit_file(p, "f", "1\n", "one");
    git(p, &["checkout", "-qb", "side"]);
    commit_file(p, "g", "g\n", "side commit");
    let side = git(p, &["rev-parse", "HEAD"]).trim().to_string();
    git(p, &["checkout", "-q", "main"]);

    ops::cherry_pick(p, std::slice::from_ref(&side)).unwrap();
    assert!(git(p, &["log", "-1", "--format=%B"]).contains("cherry picked from commit"));
    ops::revert(p, &["HEAD".to_string()]).unwrap();
    assert!(!p.join("g").exists());

    ops::undo_last_commit(p).unwrap();
    assert_eq!(git(p, &["status", "--porcelain"]).trim(), "D  g", "undone revert stays staged");
    ops::reset(p, "HEAD~1", ResetMode::Hard).unwrap();
    assert_eq!(git(p, &["log", "--format=%s"]).trim(), "one");

    ops::rename_branch(p, "side", "renamed").unwrap();
    assert!(ops::delete_branch(p, "renamed", false).is_err(), "not merged");
    ops::delete_branch(p, "renamed", true).unwrap();
    ops::rebase(p, "main").unwrap();
}

#[test]
fn conflict_versions_and_resolutions() {
    use rsit_git::conflicts;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    init(p);
    commit_file(p, "f", "base\n", "base");
    commit_file(p, "gone", "g\n", "gone");
    git(p, &["checkout", "-qb", "feature"]);
    commit_file(p, "f", "theirs\n", "theirs");
    commit_file(p, "gone", "changed\n", "change gone");
    git(p, &["checkout", "-q", "main"]);
    commit_file(p, "f", "ours\n", "ours");
    git(p, &["rm", "-q", "gone"]);
    git(p, &["commit", "-qm", "delete gone"]);
    assert!(ops::merge(p, "feature").is_err());
    let repo = rsit_git::Repo::discover(p).unwrap();

    let v = conflicts::conflict_versions(&repo, "f").unwrap();
    assert_eq!(
        (v.base.as_deref(), v.ours.as_deref(), v.theirs.as_deref()),
        (Some(&b"base\n"[..]), Some(&b"ours\n"[..]), Some(&b"theirs\n"[..]))
    );
    assert!(v.mergeable());
    let gone = conflicts::conflict_versions(&repo, "gone").unwrap();
    assert!(gone.ours.is_none() && gone.theirs.is_some() && !gone.mergeable(), "modify/delete");
    let (left, right) = conflicts::side_labels(&repo);
    assert_eq!((left.as_str(), right.as_str()), ("Yours (main)", "Theirs (feature)"));

    conflicts::save_resolved(&repo, "f", "merged\n").unwrap();
    conflicts::accept_side(&repo, "gone", true).unwrap(); // ours deleted it
    assert!(conflicts::conflicted_paths(p).unwrap().is_empty());
    assert!(!p.join("gone").exists());
    ops::continue_operation(p, Operation::Merge).unwrap();
    // with core.autocrlf=true the result is saved with CRLF, like a checkout
    assert_eq!(std::fs::read_to_string(p.join("f")).unwrap().replace("\r\n", "\n"), "merged\n");
}
