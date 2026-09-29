//! File history across renames and blame against real git.

use std::path::Path;
use std::process::Command;

use rsit_git::ChangeKind;
use rsit_git::history::{self, BlameOptions};

fn git(dir: &Path, args: &[&str], author: &str, t: u32) -> String {
    let date = format!("@{} +0000", 1_700_000_000 + t * 3600);
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", author)
        .env("GIT_AUTHOR_EMAIL", format!("{}@x", author.to_lowercase()))
        .env("GIT_COMMITTER_NAME", author)
        .env("GIT_COMMITTER_EMAIL", "c@x")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn history_follows_renames_and_blame_attributes_lines() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"], "A", 0);
    std::fs::write(p.join("old.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
    git(p, &["add", "."], "Ann", 1);
    git(p, &["commit", "-qm", "create"], "Ann", 1);
    git(p, &["mv", "old.rs", "new.rs"], "Bob", 2);
    git(p, &["commit", "-qm", "rename"], "Bob", 2);
    std::fs::write(p.join("new.rs"), "fn a() {}\nfn b2() {}\nfn c() {}\n").unwrap();
    git(p, &["commit", "-qam", "edit b"], "Bob", 3);
    std::fs::write(p.join("new.rs"), "fn a() {}\nfn b2() {}\nfn c() {}\nfn d() {}\n").unwrap();

    let h = history::file_history(p, "new.rs", None).unwrap();
    let summary: Vec<(&str, ChangeKind, &str)> = h.iter().map(|r| (r.subject.as_str(), r.kind, r.path.as_str())).collect();
    assert_eq!(summary, [
        ("edit b", ChangeKind::Modified, "new.rs"),
        ("rename", ChangeKind::Renamed, "new.rs"),
        ("create", ChangeKind::Added, "old.rs"),
    ]);
    assert_eq!(h[1].old_path.as_deref(), Some("old.rs"));

    // working tree: the new line is not committed yet
    let b = history::blame(p, "new.rs", None, BlameOptions::default()).unwrap();
    let who: Vec<(&str, bool)> = b
        .lines
        .iter()
        .map(|l| (b.commits[l.commit].summary.as_str(), b.commits[l.commit].uncommitted))
        .collect();
    assert_eq!(who[0], ("create", false), "survives the rename");
    assert_eq!(who[1], ("edit b", false));
    assert!(who[3].1, "fn d() is not committed");
    assert_eq!(b.text.lines().count(), 4);
    let create = &b.commits[b.lines[0].commit];
    assert_eq!((create.author.as_str(), create.filename.as_str()), ("Ann", "old.rs"));

    // annotate the previous revision of "edit b": back to the renamed file
    let edit = &b.commits[b.lines[1].commit];
    let (prev, prev_path) = edit.previous.clone().unwrap();
    let before = history::blame(p, &prev_path, Some(&prev.to_string()), BlameOptions::default()).unwrap();
    assert_eq!(before.text, "fn a() {}\nfn b() {}\nfn c() {}\n");
}
