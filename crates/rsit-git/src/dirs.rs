//! Where rsit keeps its files, and absolute paths that work on every platform.
//!
//! The `XDG_*` variables win everywhere (tests point them at temporary
//! directories); otherwise Linux uses the XDG defaults and Windows uses
//! `%APPDATA%` for settings and `%LOCALAPPDATA%` for state and caches.

use std::io;
use std::path::{Path, PathBuf};

/// `path` made absolute with symlinks resolved. Unlike `Path::canonicalize`,
/// on Windows it returns `C:\repo` rather than `\\?\C:\repo`, which `git`, file
/// dialogs and `strip_prefix` against gix paths all expect.
pub fn canonical(path: &Path) -> io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// Settings: `$XDG_CONFIG_HOME/rsit`, `~/.config/rsit` or `%APPDATA%\rsit`.
pub fn config_dir() -> Option<PathBuf> {
    base("XDG_CONFIG_HOME", ".config", "APPDATA")
}

/// The recent repositories list: `$XDG_STATE_HOME/rsit`, `~/.local/state/rsit`
/// or `%LOCALAPPDATA%\rsit`.
pub fn state_dir() -> Option<PathBuf> {
    base("XDG_STATE_HOME", ".local/state", "LOCALAPPDATA")
}

/// Caches: `$XDG_CACHE_HOME/rsit`, `~/.cache/rsit` or `%LOCALAPPDATA%\rsit\cache`.
pub fn cache_dir() -> Option<PathBuf> {
    let windows = cfg!(windows) && std::env::var_os("XDG_CACHE_HOME").is_none_or(|d| d.is_empty());
    let dir = base("XDG_CACHE_HOME", ".cache", "LOCALAPPDATA")?;
    Some(if windows { dir.join("cache") } else { dir })
}

fn base(xdg: &str, home_relative: &str, windows: &str) -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|d| !d.is_empty()).map(PathBuf::from);
    let dir = var(xdg)
        .or_else(|| if cfg!(windows) { var(windows) } else { std::env::home_dir().map(|h| h.join(home_relative)) })?;
    Some(dir.join("rsit"))
}
