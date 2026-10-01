//! Notices repository changes: refs (commits, fetches, checkouts) refresh the
//! log, the index (staging) refreshes local changes.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use notify::{PollWatcher, RecursiveMode, Watcher};
use rsit_git::Repo;

/// Keeps the file system watch alive; drop it to stop watching.
pub struct RefsWatcher {
    _watcher: Box<dyn Watcher + Send>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoEvent {
    /// HEAD, packed-refs or something under `refs/`.
    Refs,
    /// The index (`.git/index`).
    Index,
}

/// Sends an event whenever refs or the index change.
pub fn watch_repo(repo: &Repo) -> Result<(RefsWatcher, UnboundedReceiver<RepoEvent>)> {
    let (tx, rx) = unbounded();
    let absolute = |p: &Path| rsit_git::dirs::canonical(p).unwrap_or_else(|_| p.to_path_buf());
    let git_dir = absolute(repo.git_dir());
    let common_dir = absolute(repo.common_dir());
    let refs_dir = common_dir.join("refs");
    let classify = {
        let (git_dir, common_dir, refs_dir) = (git_dir.clone(), common_dir.clone(), refs_dir.clone());
        move |path: &Path| -> Option<RepoEvent> {
            if path.extension().is_some_and(|e| e == "lock") {
                return None;
            }
            if path.starts_with(&refs_dir) || path == git_dir.join("HEAD") || path == common_dir.join("packed-refs") {
                Some(RepoEvent::Refs)
            } else if path == git_dir.join("index") {
                Some(RepoEvent::Index)
            } else {
                None
            }
        }
    };
    let handler = move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        if event.kind.is_access() {
            return;
        }
        let mut sent = [false; 2];
        for kind in event.paths.iter().filter_map(|p| classify(p)) {
            if !std::mem::replace(&mut sent[kind as usize], true) {
                tx.unbounded_send(kind).ok();
            }
        }
    };
    // a WSL share delivers no change notifications for writes made inside Linux
    let mut watcher: Box<dyn Watcher + Send> = if rsit_git::wsl::WslPath::parse(&git_dir).is_some() {
        let config = notify::Config::default().with_poll_interval(Duration::from_secs(1));
        Box::new(PollWatcher::new(handler, config)?)
    } else {
        Box::new(notify::recommended_watcher(handler)?)
    };
    watcher.watch(&git_dir, RecursiveMode::NonRecursive)?;
    if common_dir != git_dir {
        watcher.watch(&common_dir, RecursiveMode::NonRecursive)?;
    }
    if refs_dir.exists() {
        watcher.watch(&refs_dir, RecursiveMode::Recursive)?;
    }
    Ok((RefsWatcher { _watcher: watcher }, rx))
}
