//! Notices ref changes (commits, fetches, checkouts) to refresh the log.

use std::path::Path;

use anyhow::Result;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use rsit_git::Repo;

/// Keeps the file system watch alive; drop it to stop watching.
pub struct RefsWatcher {
    _watcher: RecommendedWatcher,
}

/// Sends `()` whenever HEAD, packed-refs or anything under `refs/` changes.
pub fn watch_refs(repo: &Repo) -> Result<(RefsWatcher, UnboundedReceiver<()>)> {
    let (tx, rx) = unbounded();
    let absolute = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let git_dir = absolute(repo.git_dir());
    let common_dir = absolute(repo.common_dir());
    let refs_dir = common_dir.join("refs");
    let relevant = {
        let (git_dir, common_dir, refs_dir) = (git_dir.clone(), common_dir.clone(), refs_dir.clone());
        move |path: &Path| {
            if path.extension().is_some_and(|e| e == "lock") {
                return false;
            }
            path.starts_with(&refs_dir)
                || path == git_dir.join("HEAD")
                || path == common_dir.join("packed-refs")
        }
    };
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        if event.kind.is_access() {
            return;
        }
        if event.paths.iter().any(|p| relevant(p)) {
            tx.unbounded_send(()).ok();
        }
    })?;
    watcher.watch(&git_dir, RecursiveMode::NonRecursive)?;
    if common_dir != git_dir {
        watcher.watch(&common_dir, RecursiveMode::NonRecursive)?;
    }
    if refs_dir.exists() {
        watcher.watch(&refs_dir, RecursiveMode::Recursive)?;
    }
    Ok((RefsWatcher { _watcher: watcher }, rx))
}
