//! Line endings: with `core.autocrlf=true` (Git for Windows' default) files on
//! disk have CRLF while their blobs have LF.

use std::path::Path;
use std::process::Command;

use rsit_git::{Repo, Revision, file_at_revision, to_worktree};

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").current_dir(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn work_tree_reads_and_writes_follow_autocrlf() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.name", "T"]);
    git(p, &["config", "user.email", "t@e"]);
    git(p, &["config", "core.autocrlf", "true"]);
    std::fs::write(p.join("f.txt"), "a\r\nb\r\n").unwrap();
    git(p, &["add", "f.txt"]);
    git(p, &["commit", "-qm", "one"]);
    let repo = Repo::discover(p).unwrap();

    // an unmodified file: the work tree side matches the index, no all-lines diff
    let index = file_at_revision(&repo, Revision::Index, "f.txt").unwrap().unwrap();
    assert_eq!(index, b"a\nb\n");
    assert_eq!(file_at_revision(&repo, Revision::WorkTree, "f.txt").unwrap().unwrap(), index);

    std::fs::write(p.join("f.txt"), "a\r\nB\r\n").unwrap();
    assert_eq!(file_at_revision(&repo, Revision::WorkTree, "f.txt").unwrap().unwrap(), b"a\nB\n");

    // what rsit writes (a resolved merge) gets the endings a checkout would
    assert_eq!(to_worktree(&repo.local(), "f.txt", b"x\ny\n").unwrap(), b"x\r\ny\r\n");
}
