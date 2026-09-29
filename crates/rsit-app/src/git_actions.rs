//! Branch and remote actions shared by the branches popup, the refs panel and
//! the log (IntelliJ Git Branches popup and Git menu).

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;
use rsit_git::ops;
use rsit_git::{RefKind, Repo};

use crate::commit_menu::{prompt_name, prompt_name_with};
use crate::tasks::run_git_task;

pub fn task(
    repo: &Repo,
    label: impl Into<SharedString>,
    success: Option<String>,
    op: impl FnOnce(&std::path::Path, &mut dyn FnMut(&str)) -> anyhow::Result<String> + Send + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    run_git_task(label, repo.cwd().to_path_buf(), success, Box::new(op), window, cx);
}

pub fn fetch(repo: &Repo, window: &mut Window, cx: &mut App) {
    task(repo, "Fetching", Some("Fetched all remotes".into()), |cwd, p| ops::fetch(cwd, p), window, cx);
}

/// IntelliJ "Update Project".
pub fn update(repo: &Repo, window: &mut Window, cx: &mut App) {
    task(repo, "Updating", Some("Project updated".into()), |cwd, p| ops::pull(cwd, p), window, cx);
}

/// Pushes the current branch to its upstream, or to the first remote with
/// `--set-upstream` on its first push.
pub fn push_current(repo: &Repo, force: bool, window: &mut Window, cx: &mut App) {
    let Ok(refs) = rsit_git::read_refs(&repo.local()) else { return };
    let Some(branch) = refs.current_branch.clone() else {
        window.push_notification(
            gpui_kit::component::notification::Notification::warning("HEAD is detached: nothing to push"),
            cx,
        );
        return;
    };
    let run = move |repo: &Repo, window: &mut Window, cx: &mut App| {
        let branch = branch.clone();
        let label = if force { format!("Force pushing {branch}") } else { format!("Pushing {branch}") };
        task(
            repo,
            label,
            Some(format!("Pushed {branch}")),
            move |cwd, p| {
                let upstream = ops::upstream(cwd, &branch);
                let remote = match &upstream {
                    Some(u) => u.split_once('/').map(|(r, _)| r.to_string()).unwrap_or_else(|| "origin".into()),
                    None => ops::remotes(cwd)?.into_iter().next().ok_or_else(|| anyhow::anyhow!("no remotes configured"))?,
                };
                ops::push(cwd, &remote, &branch, upstream.is_none(), force, p)
            },
            window,
            cx,
        );
    };
    if force {
        let repo = repo.clone();
        confirm(
            "Force Push",
            "Force push overwrites the remote branch (with --force-with-lease). Continue?",
            window,
            cx,
            move |window, cx| run(&repo, window, cx),
        );
    } else {
        run(repo, window, cx);
    }
}

pub fn checkout(repo: &Repo, name: String, remote: bool, window: &mut Window, cx: &mut App) {
    let label = format!("Checking out {name}");
    task(repo, label, None, move |cwd, _| ops::checkout_branch(cwd, &name, remote).map(|_| String::new()), window, cx);
}

pub fn new_branch_from(repo: &Repo, rev: String, window: &mut Window, cx: &mut App) {
    let repo = repo.clone();
    prompt_name(&format!("New Branch from {rev}"), "Create", Some("Checkout branch"), window, cx, move |name, checkout, window, cx| {
        let rev = rev.clone();
        let success = format!("Created branch {name}");
        task(
            &repo,
            format!("Creating {name}"),
            Some(success),
            move |cwd, _| rsit_git::cli::create_branch(cwd, &name, &rev, checkout),
            window,
            cx,
        );
    });
}

pub fn merge_into_current(repo: &Repo, name: String, window: &mut Window, cx: &mut App) {
    let success = format!("Merged {name}");
    task(repo, format!("Merging {name}"), Some(success), move |cwd, _| ops::merge(cwd, &name), window, cx);
}

pub fn rebase_current_onto(repo: &Repo, name: String, window: &mut Window, cx: &mut App) {
    let success = format!("Rebased onto {name}");
    task(repo, format!("Rebasing onto {name}"), Some(success), move |cwd, _| ops::rebase(cwd, &name), window, cx);
}

pub fn rename_branch(repo: &Repo, name: String, window: &mut Window, cx: &mut App) {
    let repo = repo.clone();
    let title = format!("Rename {name}");
    prompt_name_with(&title, "Rename", None, &name.clone(), window, cx, move |new, _, window, cx| {
        let old = name.clone();
        task(&repo, "Renaming branch", None, move |cwd, _| ops::rename_branch(cwd, &old, &new).map(|_| String::new()), window, cx);
    });
}

/// Deletes a local branch; if git refuses because it is not merged, asks to force.
pub fn delete_branch(repo: &Repo, name: String, window: &mut Window, cx: &mut App) {
    let cwd = repo.cwd().to_path_buf();
    let repo = repo.clone();
    window
        .spawn(cx, async move |cx| {
            let n = name.clone();
            let result = cx.background_spawn(async move { ops::delete_branch(&cwd, &n, false) }).await;
            cx.update(|window, cx| match result {
                Ok(()) => window.push_notification(format!("Deleted branch {name}"), cx),
                Err(_) => {
                    let detail = format!("Branch {name} is not fully merged. Delete it anyway?");
                    confirm("Delete Branch", &detail, window, cx, move |window, cx| {
                        let name = name.clone();
                        let success = format!("Deleted branch {name}");
                        task(&repo, "Deleting branch", Some(success), move |cwd, _| ops::delete_branch(cwd, &name, true).map(|_| String::new()), window, cx);
                    });
                }
            })
            .ok();
        })
        .detach();
}

pub fn delete_remote_branch(repo: &Repo, name: String, window: &mut Window, cx: &mut App) {
    let repo = repo.clone();
    let detail = format!("Delete {name} on the remote?");
    confirm("Delete Remote Branch", &detail, window, cx, move |window, cx| {
        let name = name.clone();
        let success = format!("Deleted {name}");
        task(&repo, format!("Deleting {name}"), Some(success), move |cwd, p| ops::delete_remote_branch(cwd, &name, p).map(|_| String::new()), window, cx);
    });
}

pub fn delete_tag(repo: &Repo, name: String, window: &mut Window, cx: &mut App) {
    let repo = repo.clone();
    confirm("Delete Tag", &format!("Delete tag {name}?"), window, cx, move |window, cx| {
        let name = name.clone();
        task(&repo, "Deleting tag", None, move |cwd, _| ops::delete_tag(cwd, &name).map(|_| String::new()), window, cx);
    });
}

pub fn confirm(
    title: &str,
    detail: &str,
    window: &mut Window,
    cx: &mut App,
    on_ok: impl Fn(&mut Window, &mut App) + 'static,
) {
    let (title, detail) = (SharedString::from(title.to_string()), SharedString::from(detail.to_string()));
    let on_ok = std::rc::Rc::new(on_ok);
    window.open_alert_dialog(cx, move |dialog, _, _| {
        let on_ok = on_ok.clone();
        dialog.title(title.clone()).description(detail.clone()).on_ok(move |_, window, cx| {
            on_ok(window, cx);
            true
        })
    });
}

/// Actions for one branch (IntelliJ branch popup submenu). `current` is the
/// checked out branch, which cannot be merged into itself or deleted.
pub fn branch_menu(menu: PopupMenu, repo: &Repo, name: &str, kind: RefKind, current: Option<&str>) -> PopupMenu {
    let is_current = kind == RefKind::LocalBranch && current == Some(name);
    let item = |label: String, f: fn(&Repo, String, &mut Window, &mut App)| {
        let (repo, name) = (repo.clone(), name.to_string());
        PopupMenuItem::new(label).on_click(move |_, window, cx| f(&repo, name.clone(), window, cx))
    };
    let mut menu = menu;
    match kind {
        RefKind::LocalBranch | RefKind::RemoteBranch => {
            let remote = kind == RefKind::RemoteBranch;
            if !is_current {
                let (repo, n) = (repo.clone(), name.to_string());
                menu = menu.item(PopupMenuItem::new("Checkout").on_click(move |_, window, cx| checkout(&repo, n.clone(), remote, window, cx)));
            }
            menu = menu.item(item(format!("New Branch from '{name}'…"), new_branch_from));
            if !is_current && current.is_some() {
                let cur = current.unwrap_or_default();
                menu = menu
                    .separator()
                    .item(item(format!("Merge '{name}' into '{cur}'"), merge_into_current))
                    .item(item(format!("Rebase '{cur}' onto '{name}'"), rebase_current_onto));
            }
            if kind == RefKind::LocalBranch {
                menu = menu.separator().item(item("Rename…".into(), rename_branch));
                if !is_current {
                    menu = menu.item(item("Delete".into(), delete_branch));
                }
            } else {
                menu = menu.separator().item(item("Delete on Remote…".into(), delete_remote_branch));
            }
        }
        RefKind::Tag => {
            menu = menu.item(item(format!("New Branch from '{name}'…"), new_branch_from)).separator().item(item("Delete Tag…".into(), delete_tag));
        }
        RefKind::Head | RefKind::Other => {}
    }
    menu
}
