//! Interactive rebase plans executed by real git.

use std::path::Path;
use std::process::Command;

use rsit_git::ops::{self, Operation};
use rsit_git::rebase::{Action, RebasePlan, run_interactive};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").current_dir(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// base, then c1..c5 each adding its own file.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.name", "T"]);
    git(p, &["config", "user.email", "t@e"]);
    std::fs::write(p.join("base"), "b\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "base"]);
    for i in 1..=5 {
        std::fs::write(p.join(format!("f{i}")), format!("{i}\n")).unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-qm", &format!("c{i}")]);
    }
    dir
}

fn subjects(p: &Path) -> Vec<String> {
    git(p, &["log", "--format=%s"]).lines().map(str::to_string).collect()
}

fn plan_from(p: &Path, rev: &str) -> RebasePlan {
    let id = git(p, &["rev-parse", rev]).trim().to_string();
    RebasePlan::from_commit(p, rsit_git::ObjectId::from_hex(id.as_bytes()).unwrap()).unwrap()
}

#[test]
fn reword_squash_fixup_drop_reorder() {
    let dir = repo();
    let p = dir.path();
    let mut plan = plan_from(p, "HEAD~4"); // c1..c5
    assert_eq!(plan.entries.iter().map(|e| e.subject().to_string()).collect::<Vec<_>>(), ["c1", "c2", "c3", "c4", "c5"]);

    plan.entries[0].action = Action::Reword;
    plan.entries[0].new_message = Some("c1 reworded".into());
    plan.entries[1].action = Action::Squash; // c2 into c1
    plan.entries[0].new_message = Some("c1+c2 together".into());
    plan.entries[2].action = Action::Drop; // c3
    plan.entries.swap(3, 4); // c5 before c4
    plan.entries[4].action = Action::Fixup; // c4 into c5
    run_interactive(p, &plan).unwrap();

    assert_eq!(subjects(p), ["c5", "c1+c2 together", "base"]);
    assert!(!p.join("f3").exists(), "c3 dropped");
    assert!(p.join("f4").exists() && p.join("f2").exists(), "melded changes kept");
    assert_eq!(ops::operation_in_progress(&rsit_git::Repo::discover(p).unwrap()), None);
}

#[test]
fn edit_stops_and_continues() {
    let dir = repo();
    let p = dir.path();
    let mut plan = plan_from(p, "HEAD~1");
    plan.entries[0].action = Action::Edit;
    run_interactive(p, &plan).unwrap();
    let repo = rsit_git::Repo::discover(p).unwrap();
    assert_eq!(ops::operation_in_progress(&repo), Some(Operation::Rebase), "stopped at edit");
    std::fs::write(p.join("f4"), "edited\n").unwrap();
    git(p, &["commit", "-qa", "--amend", "--no-edit"]);
    ops::continue_operation(p, Operation::Rebase).unwrap();
    assert_eq!(ops::operation_in_progress(&repo), None);
    assert_eq!(std::fs::read_to_string(p.join("f4")).unwrap(), "edited\n");
    assert_eq!(subjects(p)[..2], ["c5", "c4"]);
}

#[test]
fn root_rebase_and_validation() {
    let dir = repo();
    let p = dir.path();
    let mut plan = plan_from(p, "HEAD~5"); // from the root commit
    assert!(plan.base.is_none());
    plan.entries[0].action = Action::Fixup;
    assert!(run_interactive(p, &plan).is_err(), "nothing to meld the root into");
    plan.entries[0].action = Action::Reword;
    plan.entries[0].new_message = Some("root".into());
    run_interactive(p, &plan).unwrap();
    assert_eq!(subjects(p).last().map(String::as_str), Some("root"));
}

#[test]
fn merges_are_rejected() {
    let dir = repo();
    let p = dir.path();
    git(p, &["checkout", "-qb", "side", "HEAD~2"]);
    std::fs::write(p.join("side"), "s\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "side"]);
    git(p, &["checkout", "-q", "main"]);
    git(p, &["merge", "-q", "--no-edit", "side"]);
    let id = git(p, &["rev-parse", "HEAD~3"]).trim().to_string();
    let err = RebasePlan::from_commit(p, rsit_git::ObjectId::from_hex(id.as_bytes()).unwrap()).unwrap_err();
    assert!(err.to_string().contains("merge"), "{err}");
}
