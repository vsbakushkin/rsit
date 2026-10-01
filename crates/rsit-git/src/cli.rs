//! Runs the system `git`, the way IntelliJ does for everything that writes:
//! hooks, credential helpers, signing and ssh work as configured by the user.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use anyhow::{Result, bail};

pub fn git(cwd: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        // never open an interactive editor; callers that need one set their own
        .env("GIT_EDITOR", "true")
        .arg("-c")
        .arg("color.ui=false")
        .arg("-c")
        .arg("core.quotepath=false")
        .stdin(Stdio::null());
    // a GUI process has no console, so Windows would open one for every git call
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, CREATE_NO_WINDOW);
    cmd
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Runs git and returns stdout, failing with stderr on a non-zero exit.
pub fn run(cwd: &Path, args: &[&str]) -> Result<String> {
    let Output { status, stdout, stderr } = git(cwd).args(args).output()?;
    if !status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&stderr).trim());
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

/// Unified diff of `path` in `commit` against its first parent.
pub fn show_file_diff(cwd: &Path, commit: &str, path: &str) -> Result<String> {
    run(cwd, &["show", "--format=", "--first-parent", "-M", commit, "--", path])
}

/// Checks out `rev` (a branch name or a commit for a detached HEAD).
pub fn checkout(cwd: &Path, rev: &str) -> Result<String> {
    run(cwd, &["checkout", rev])
}

/// Creates branch `name` at `rev`, optionally checking it out.
pub fn create_branch(cwd: &Path, name: &str, rev: &str, checkout: bool) -> Result<String> {
    if checkout { run(cwd, &["checkout", "-b", name, rev]) } else { run(cwd, &["branch", name, rev]) }
}

/// Creates a lightweight tag `name` at `rev`.
pub fn create_tag(cwd: &Path, name: &str, rev: &str) -> Result<String> {
    run(cwd, &["tag", name, rev])
}
