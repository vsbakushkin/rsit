//! Filters agree with `git log` on a small repository.

use std::path::Path;
use std::process::Command;

use rsit_log::{LogData, LogFilter};

fn git(dir: &Path, args: &[&str], author: &str) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", author)
        .env("GIT_AUTHOR_EMAIL", format!("{}@example.com", author.to_lowercase()))
        .env("GIT_COMMITTER_NAME", "Committer")
        .env("GIT_COMMITTER_EMAIL", "c@example.com")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn subjects(data: &LogData, filter: &LogFilter) -> Vec<String> {
    let visible = rsit_log::filter::visible_rows(data, filter).unwrap().unwrap();
    let local = data.repo.local();
    (0..data.len() as u32)
        .filter(|&r| visible[r as usize])
        .map(|r| rsit_git::read_commit(&local, data.id(r)).unwrap().subject().to_string())
        .collect()
}

#[test]
fn filters() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"], "A");
    git(p, &["commit", "-q", "--allow-empty", "-m", "Fix Parser crash"], "Alice");
    git(p, &["commit", "-q", "--allow-empty", "-m", "add feature", "-m", "body mentions parser"], "Bob");
    git(p, &["checkout", "-q", "-b", "side"], "A");
    std::fs::write(p.join("file.txt"), "x").unwrap();
    git(p, &["add", "file.txt"], "A");
    git(p, &["commit", "-q", "-m", "touch file"], "Alice");
    git(p, &["checkout", "-q", "main"], "A");

    unsafe { std::env::set_var("XDG_CACHE_HOME", dir.path().join("cache")) };
    let data = LogData::load(rsit_git::Repo::discover(p).unwrap(), None).unwrap();
    let f = |text: &str, user: &str| LogFilter { text: text.into(), user: user.into(), ..Default::default() };

    let mut by_text = subjects(&data, &f("parser", ""));
    by_text.sort();
    assert_eq!(by_text, ["Fix Parser crash", "add feature"], "case-insensitive, whole message");
    assert_eq!(subjects(&data, &LogFilter { match_case: true, ..f("Parser", "") }), ["Fix Parser crash"]);
    assert_eq!(subjects(&data, &LogFilter { regex: true, ..f("^fix .*crash$", "") }), ["Fix Parser crash"]);
    let mut alice = subjects(&data, &f("", "alice"));
    alice.sort();
    assert_eq!(alice, ["Fix Parser crash", "touch file"], "author only, not committer");
    assert!(subjects(&data, &f("", "committer")).is_empty());
    let mut side = subjects(&data, &LogFilter { branches: vec!["side".into()], ..Default::default() });
    side.sort();
    assert_eq!(side, ["Fix Parser crash", "add feature", "touch file"]);
    assert_eq!(subjects(&data, &LogFilter { paths: vec!["file.txt".into()], ..Default::default() }), ["touch file"]);
    let hash = data.id(0).to_hex_with_len(7).to_string();
    assert_eq!(subjects(&data, &f(&hash, "")).len(), 1, "hash prefix");
}
