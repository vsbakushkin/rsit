//! Runs the system `git`, the way IntelliJ does for everything that writes:
//! hooks, credential helpers, signing and ssh work as configured by the user.
//! For a repository inside WSL that is the distribution's `git`, see [`crate::wsl`].

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{Result, bail};

use crate::wsl::WslPath;

/// Variables rsit sets for git; WSL passes only the ones listed in `WSLENV`.
const GIT_VARS: &[&str] = &["GIT_TERMINAL_PROMPT", "LC_ALL", "GIT_EDITOR", "GIT_SEQUENCE_EDITOR", "GIT_OPTIONAL_LOCKS"];

pub fn git(cwd: &Path) -> Command {
    let mut cmd = match WslPath::parse(cwd) {
        Some(wsl) => {
            let mut cmd = Command::new("wsl.exe");
            cmd.args(["--distribution", &wsl.distro, "--cd", &wsl.linux, "--exec", "git"]);
            let user = std::env::var("WSLENV").unwrap_or_default();
            let vars = GIT_VARS.join(":");
            cmd.env("WSLENV", if user.is_empty() { vars } else { format!("{user}:{vars}") });
            cmd
        }
        None => {
            let mut cmd = Command::new("git");
            cmd.current_dir(cwd);
            cmd
        }
    };
    cmd.env("GIT_TERMINAL_PROMPT", "0")
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

/// A path printed by git running in `cwd` (relative to it, or absolute on the
/// side git runs on) as a path rsit can open.
pub fn from_git_path(cwd: &Path, path: &str) -> PathBuf {
    match WslPath::parse(cwd) {
        Some(wsl) if path.starts_with('/') => wsl.host(path),
        _ => cwd.join(path),
    }
}

/// `path` as git running in `cwd` and the commands it starts (editors, `exec`
/// lines) see it; `path` must come from [`temp_dir`] or lie in the repository.
pub fn to_git_path(cwd: &Path, path: &Path) -> String {
    if WslPath::parse(cwd).is_some() {
        return WslPath::parse(path).map(|p| p.linux).unwrap_or_else(|| path.display().to_string());
    }
    let path = path.display().to_string();
    // Git for Windows' shell takes `C:/dir` more reliably than `C:\dir`
    if cfg!(windows) { path.replace('\\', "/") } else { path }
}

/// A directory for files git running in `cwd` must read: the system one, or
/// `/tmp` of the distribution for a repository inside WSL.
pub fn temp_dir(cwd: &Path) -> PathBuf {
    match WslPath::parse(cwd) {
        Some(wsl) => wsl.host("/tmp"),
        None => std::env::temp_dir(),
    }
}

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
