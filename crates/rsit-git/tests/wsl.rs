//! A repository inside WSL, opened through `\\wsl.localhost`. Needs Windows
//! with a WSL distribution that has git, named in RSIT_TEST_WSL_DISTRO; skipped
//! otherwise (CI runners have none).
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use rsit_git::rebase::{Action, RebasePlan, run_interactive};
use rsit_git::{Repo, Revision, changes, cli, file_at_revision, to_worktree};

/// Runs `script` in a fresh repository under the distribution's /tmp and
/// returns its Windows path; the repository is removed on drop.
struct WslRepo {
    distro: String,
    linux: String,
}

impl WslRepo {
    fn new(script: &str) -> Option<Self> {
        let distro = std::env::var("RSIT_TEST_WSL_DISTRO").ok().filter(|d| !d.is_empty())?;
        let linux = format!("/tmp/rsit-wsl-test-{}", std::process::id());
        let repo = Self { distro, linux };
        repo.sh(&format!(
            "rm -rf {0} && mkdir {0} && cd {0} && git init -q -b main && git config user.name Test && \
             git config user.email test@example.com && {script}",
            repo.linux
        ));
        Some(repo)
    }

    fn sh(&self, script: &str) -> String {
        let out = Command::new("wsl.exe").args(["-d", &self.distro, "--exec", "sh", "-c", script]).output().unwrap();
        assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn path(&self) -> PathBuf {
        PathBuf::from(format!(r"\\wsl.localhost\{}{}", self.distro, self.linux.replace('/', r"\")))
    }
}

impl Drop for WslRepo {
    fn drop(&mut self) {
        let _ = Command::new("wsl.exe").args(["-d", &self.distro, "--exec", "rm", "-rf", &self.linux]).status();
    }
}

#[test]
fn reads_and_writes_through_the_distributions_git() {
    let Some(wsl) = WslRepo::new(
        "printf 'a\\n' > f.txt && printf '#!/bin/sh\\n' > run.sh && chmod +x run.sh && git add . && \
         git commit -qm first && echo b >> f.txt && git commit -qam second",
    ) else {
        eprintln!("RSIT_TEST_WSL_DISTRO not set, skipping");
        return;
    };
    // Windows configuration with core.autocrlf=true, as Git for Windows' system
    // file has; it must not apply to files inside Linux.
    // SAFETY: the only test in this binary, set before anything reads it
    unsafe {
        std::env::set_var("GIT_CONFIG_COUNT", "1");
        std::env::set_var("GIT_CONFIG_KEY_0", "core.autocrlf");
        std::env::set_var("GIT_CONFIG_VALUE_0", "true");
    }
    let repo = Repo::discover(&wsl.path()).unwrap();
    let cwd = repo.cwd().to_path_buf();
    assert_eq!(repo.display_name(), wsl.linux.rsplit('/').next().unwrap());
    assert_eq!(to_worktree(&repo.local(), "f.txt", b"x\ny\n").unwrap(), b"x\ny\n");
    assert_eq!(file_at_revision(&repo, Revision::WorkTree, "f.txt").unwrap().unwrap(), b"a\nb\n");

    // Windows git would refuse ("dubious ownership") or report run.sh's mode
    let status = changes::status(&cwd).unwrap();
    assert!(status.entries.is_empty(), "{:?}", status.entries);

    // commit: message on stdin, environment through WSLENV
    std::fs::write(cwd.join("g.txt"), "new\n").unwrap();
    changes::stage(&cwd, &["g.txt".into()]).unwrap();
    changes::commit(&cwd, "third: it's «new»", false).unwrap();
    assert_eq!(wsl.sh(&format!("cd {} && git log -1 --format=%s", wsl.linux)).trim(), "third: it's «new»");

    // interactive rebase: the todo and messages are read by the distribution's shell
    let head = repo.local().head_id().unwrap().detach();
    let second = repo.local().find_commit(head).unwrap().parent_ids().next().unwrap().detach();
    let mut plan = RebasePlan::from_commit(&cwd, second).unwrap();
    plan.entries[0].action = Action::Reword;
    plan.entries[0].new_message = Some("second, reworded".into());
    run_interactive(&cwd, &plan).unwrap();
    let log = wsl.sh(&format!("cd {} && git log --format=%s", wsl.linux));
    assert_eq!(log.lines().collect::<Vec<_>>(), ["third: it's «new»", "second, reworded", "first"]);
    assert!(!cli::from_git_path(&cwd, ".git/rebase-merge").exists());
}
