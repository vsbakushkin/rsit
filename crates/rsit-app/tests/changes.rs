//! Staging, single-change staging, rollback and commit against real git.

use std::path::Path;
use std::process::Command;

use rsit_git::changes;
use rsit_git::{ChangeKind, Revision};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@e")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.name", "T"]);
    git(p, &["config", "user.email", "t@e"]);
    let text: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    std::fs::write(p.join("a.txt"), text).unwrap();
    std::fs::write(p.join("gone.txt"), "x\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "base"]);
    dir
}

#[test]
fn status_stage_and_commit() {
    let dir = repo();
    let p = dir.path();
    std::fs::write(
        p.join("a.txt"),
        std::fs::read_to_string(p.join("a.txt")).unwrap().replace("line 3\n", "line three\n"),
    )
    .unwrap();
    std::fs::remove_file(p.join("gone.txt")).unwrap();
    std::fs::write(p.join("new.txt"), "n\n").unwrap();

    let s = changes::status(p).unwrap();
    assert_eq!(
        s.unstaged().map(|e| (e.path.as_str(), e.unstaged)).collect::<Vec<_>>(),
        [("a.txt", Some(ChangeKind::Modified)), ("gone.txt", Some(ChangeKind::Deleted))]
    );
    assert_eq!(s.untracked().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["new.txt"]);

    changes::stage(p, &["gone.txt".into(), "new.txt".into()]).unwrap();
    let s = changes::status(p).unwrap();
    assert_eq!(s.staged().count(), 2);
    changes::unstage(p, &["new.txt".into()], s.has_head).unwrap();
    assert_eq!(changes::status(p).unwrap().untracked().count(), 1);

    changes::commit(p, "Remove gone.txt\n\nbody", false).unwrap();
    assert_eq!(git(p, &["log", "-1", "--format=%B"]).trim(), "Remove gone.txt\n\nbody");
    changes::stage(p, &["new.txt".into()]).unwrap();
    changes::commit(p, "Remove gone, add new", true).unwrap();
    assert_eq!(git(p, &["rev-list", "--count", "HEAD"]).trim(), "2", "amended");
    assert_eq!(changes::last_commit_message(p).unwrap(), "Remove gone, add new");

    changes::rollback(p, &["a.txt".into()], false).unwrap();
    assert!(changes::status(p).unwrap().entries.is_empty());
}

#[test]
fn stage_and_unstage_one_change() {
    let dir = repo();
    let p = dir.path();
    let edited = std::fs::read_to_string(p.join("a.txt"))
        .unwrap()
        .replace("line 3\n", "line three\n")
        .replace("line 15\n", "line fifteen\nextra\n");
    std::fs::write(p.join("a.txt"), &edited).unwrap();
    let repo = rsit_git::Repo::discover(p).unwrap();
    let text = |rev| String::from_utf8(rsit_git::file_at_revision(&repo, rev, "a.txt").unwrap().unwrap()).unwrap();

    let (index, work) = (text(Revision::Index), text(Revision::WorkTree));
    let fragments = rsit_diff::compare(&index, &work, rsit_diff::WhitespacePolicy::Default);
    assert_eq!(fragments.len(), 2);
    // stage only the second change
    changes::apply_to_index(p, &rsit_diff::fragment_patch("a.txt", &index, &work, &fragments[1]), false).unwrap();
    let staged = git(p, &["diff", "--cached", "-U0"]);
    assert!(staged.contains("+line fifteen") && !staged.contains("three"), "{staged}");
    let unstaged = git(p, &["diff", "-U0"]);
    assert!(unstaged.contains("+line three") && !unstaged.contains("fifteen"), "{unstaged}");

    // unstage it again: HEAD -> index patch, reversed
    let head = git(p, &["rev-parse", "HEAD"]).trim().to_string();
    let head_id = rsit_git::ObjectId::from_hex(head.as_bytes()).unwrap();
    let (head_text, index) = (text(Revision::Commit(head_id)), text(Revision::Index));
    let fragments = rsit_diff::compare(&head_text, &index, rsit_diff::WhitespacePolicy::Default);
    changes::apply_to_index(p, &rsit_diff::fragment_patch("a.txt", &head_text, &index, &fragments[0]), true).unwrap();
    assert!(git(p, &["diff", "--cached"]).is_empty());
    assert_eq!(text(Revision::WorkTree), edited, "working tree untouched");
}
