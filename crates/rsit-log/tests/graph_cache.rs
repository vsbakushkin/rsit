//! The graph cache must give the same history as a fresh walk after commits,
//! deleted branches and rewritten history.

use std::path::Path;
use std::process::Command;

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

fn commit(dir: &Path, msg: &str, t: u32) {
    let date = format!("@{} +0000", 1_700_000_000 + t * 60);
    let out = Command::new("git")
        .current_dir(dir)
        .args(["commit", "-q", "--allow-empty", "-m", msg])
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@e")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .output()
        .unwrap();
    assert!(out.status.success());
}

fn expected(dir: &Path) -> Vec<String> {
    git(dir, &["log", "--branches", "--remotes", "--tags", "HEAD", "--date-order", "--format=%H"])
        .lines()
        .map(str::to_string)
        .collect()
}

fn loaded(dir: &Path) -> Vec<String> {
    let repo = rsit_git::Repo::discover(dir).unwrap();
    let data = rsit_log::LogData::load(repo, None).unwrap();
    data.commits.ids.iter().map(|id| id.to_string()).collect()
}

#[test]
fn cache_follows_repository_changes() {
    let cache = tempfile::tempdir().unwrap();
    // SAFETY: the only test in this binary; nothing else reads the environment concurrently
    unsafe { std::env::set_var("XDG_CACHE_HOME", cache.path()) };
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    commit(p, "c1", 1);
    commit(p, "c2", 2);
    git(p, &["checkout", "-q", "-b", "tmp"]);
    commit(p, "t1", 3);
    git(p, &["checkout", "-q", "main"]);

    assert_eq!(loaded(p), expected(p));
    let repo = rsit_git::Repo::discover(p).unwrap();
    assert!(rsit_index::graph_path(&repo).unwrap().exists(), "cache written");

    // new commits, a deleted branch and a merge
    git(p, &["branch", "-D", "-q", "tmp"]);
    commit(p, "c3", 4);
    git(p, &["checkout", "-q", "-b", "side", "HEAD~1"]);
    commit(p, "s1", 5);
    git(p, &["checkout", "-q", "main"]);
    git(p, &["merge", "-q", "--no-ff", "side", "-m", "merge"]);
    let from_cache = loaded(p);
    assert_eq!(from_cache, expected(p));
    assert_eq!(from_cache.len(), 5, "t1 is gone, c3, s1 and the merge are new");

    // rewritten history
    git(p, &["reset", "-q", "--hard", "HEAD~2"]);
    git(p, &["branch", "-D", "-q", "side"]);
    commit(p, "c3b", 6);
    assert_eq!(loaded(p), expected(p));
}
