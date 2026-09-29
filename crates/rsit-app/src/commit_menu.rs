//! Context menu of a commit in the log (IntelliJ `Vcs.Log.ContextMenu`) and the
//! small dialogs its actions need.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::WindowExt as _;
use gpui_kit::*;
use rsit_git::{ObjectId, RefKind};

use crate::git_actions;
use crate::log_view::LogView;
use rsit_git::ops::{self, ResetMode};

/// What the menu acts on: the selected commit and its local branches.
pub struct MenuTarget {
    pub id: ObjectId,
    pub local_branches: Vec<String>,
    pub is_head: bool,
    pub has_parent: bool,
}

pub fn build(menu: PopupMenu, view: WeakEntity<LogView>, window: &mut Window, cx: &mut Context<PopupMenu>) -> PopupMenu {
    let Some(entity) = view.upgrade() else { return menu };
    let Some(target) = entity.read(cx).menu_target() else { return menu };
    let id = target.id;
    let short = id.to_hex_with_len(8).to_string();

    let mut menu = menu.item(PopupMenuItem::new("Copy Revision Number").on_click(move |_, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(id.to_string()));
    }));
    menu = menu.separator();

    for branch in &target.local_branches {
        let branch = branch.clone();
        let view = view.clone();
        menu = menu.item(PopupMenuItem::new(format!("Checkout '{branch}'")).on_click(move |_, window, cx| {
            let branch = branch.clone();
            let label = format!("Checked out {branch}");
            run(&view, label, move |repo| rsit_git::cli::checkout(repo, &branch), window, cx);
        }));
    }
    {
        let view = view.clone();
        let rev = id.to_string();
        menu = menu.item(
            PopupMenuItem::new("Checkout Revision").disabled(target.is_head).on_click(move |_, window, cx| {
                let rev = rev.clone();
                let label = format!("Checked out {}", &rev[..8]);
                run(&view, label, move |repo| rsit_git::cli::checkout(repo, &rev), window, cx);
            }),
        );
    }
    menu = menu.separator();
    let repo = entity.read(cx).repo().clone();
    {
        let (repo, rev) = (repo.clone(), id.to_string());
        menu = menu.item(PopupMenuItem::new("Cherry-Pick").disabled(target.is_head).on_click(move |_, window, cx| {
            let rev = rev.clone();
            git_actions::task(&repo, "Cherry-picking", Some("Cherry-picked".into()), move |cwd, _| ops::cherry_pick(cwd, &[rev]), window, cx);
        }));
    }
    {
        let (repo, rev) = (repo.clone(), id.to_string());
        menu = menu.item(PopupMenuItem::new("Revert Commit").on_click(move |_, window, cx| {
            let rev = rev.clone();
            git_actions::task(&repo, "Reverting", Some("Reverted".into()), move |cwd, _| ops::revert(cwd, &[rev]), window, cx);
        }));
    }
    if target.is_head && target.has_parent {
        let repo = repo.clone();
        menu = menu.item(PopupMenuItem::new("Undo Commit").on_click(move |_, window, cx| {
            let repo = repo.clone();
            git_actions::confirm(
                "Undo Commit",
                "Move HEAD to the parent commit and keep its changes staged?",
                window,
                cx,
                move |window, cx| {
                    git_actions::task(&repo, "Undoing commit", None, |cwd, _| ops::undo_last_commit(cwd).map(|_| String::new()), window, cx)
                },
            );
        }));
    }
    {
        let (repo, rev) = (repo.clone(), id.to_string());
        let short = short.clone();
        menu = menu.submenu("Reset Current Branch to Here", window, cx, move |menu, _, _| {
            let mut menu = menu;
            for (label, mode) in [
                ("Soft: keep changes staged", ResetMode::Soft),
                ("Mixed: keep changes in the working tree", ResetMode::Mixed),
                ("Keep: keep local changes, drop committed", ResetMode::Keep),
                ("Hard: discard all changes…", ResetMode::Hard),
            ] {
                let (repo, rev, short) = (repo.clone(), rev.clone(), short.clone());
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                    let (repo, rev) = (repo.clone(), rev.clone());
                    let run = move |window: &mut Window, cx: &mut App| {
                        let rev = rev.clone();
                        git_actions::task(&repo, "Resetting", None, move |cwd, _| ops::reset(cwd, &rev, mode).map(|_| String::new()), window, cx);
                    };
                    if mode == ResetMode::Hard {
                        let detail = format!("Reset the current branch to {short} and discard all local changes?");
                        git_actions::confirm("Hard Reset", &detail, window, cx, run);
                    } else {
                        run(window, cx);
                    }
                }));
            }
            menu
        });
    }
    menu = menu.separator();
    {
        let view = view.clone();
        let rev = id.to_string();
        let title = format!("New Branch from {short}");
        menu = menu.item(PopupMenuItem::new("New Branch…").on_click(move |_, window, cx| {
            let (view, rev) = (view.clone(), rev.clone());
            prompt_name(&title, "Create", Some("Checkout branch"), window, cx, move |name, checkout, window, cx| {
                let rev = rev.clone();
                let label = format!("Created branch {name}");
                run(&view, label, move |repo| rsit_git::cli::create_branch(repo, &name, &rev, checkout), window, cx);
            });
        }));
    }
    {
        let view = view.clone();
        let rev = id.to_string();
        let title = format!("New Tag on {short}");
        menu = menu.item(PopupMenuItem::new("New Tag…").on_click(move |_, window, cx| {
            let (view, rev) = (view.clone(), rev.clone());
            prompt_name(&title, "Create", None, window, cx, move |name, _, window, cx| {
                let rev = rev.clone();
                let label = format!("Created tag {name}");
                run(&view, label, move |repo| rsit_git::cli::create_tag(repo, &name, &rev), window, cx);
            });
        }));
    }
    menu
}

impl LogView {
    pub fn menu_target_for(&self, id: ObjectId, data: &rsit_log::LogData) -> MenuTarget {
        let local_branches = data
            .row_of(&id)
            .map(|row| {
                data.refs_at(row)
                    .iter()
                    .filter(|r| r.kind == RefKind::LocalBranch && data.refs.current_branch.as_deref() != Some(&r.name))
                    .map(|r| r.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        let has_parent = data.row_of(&id).is_some_and(|row| data.parent_rows(row).next().is_some());
        MenuTarget { id, local_branches, is_head: data.refs.head == Some(id), has_parent }
    }
}

/// Runs a git operation in the background and reports its outcome. The log
/// refreshes by itself through the refs watcher.
fn run(
    view: &WeakEntity<LogView>,
    success: String,
    op: impl FnOnce(&std::path::Path) -> anyhow::Result<String> + Send + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(entity) = view.upgrade() else { return };
    let cwd = entity.read(cx).repo().cwd().to_path_buf();
    window
        .spawn(cx, async move |cx| {
            let result = cx.background_spawn(async move { op(&cwd) }).await;
            cx.update(|window, cx| match result {
                Ok(_) => window.push_notification(success, cx),
                Err(e) => window.push_notification(
                    gpui_kit::component::notification::Notification::error(format!("{e:#}")).autohide(false),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
}

/// Asks for a ref name; `option` adds a checkbox (checked by default) whose value
/// is passed to `on_ok`.
pub(crate) fn prompt_name(
    title: &str,
    ok_label: &'static str,
    option: Option<&'static str>,
    window: &mut Window,
    cx: &mut App,
    on_ok: impl Fn(String, bool, &mut Window, &mut App) + 'static,
) {
    prompt_name_with(title, ok_label, option, "", window, cx, on_ok)
}

pub(crate) fn prompt_name_with(
    title: &str,
    ok_label: &'static str,
    option: Option<&'static str>,
    initial: &str,
    window: &mut Window,
    cx: &mut App,
    on_ok: impl Fn(String, bool, &mut Window, &mut App) + 'static,
) {
    let initial = initial.to_string();
    let input = cx.new(|cx| {
        let mut state = InputState::new(window, cx).placeholder("Name");
        state.set_value(initial, window, cx);
        state
    });
    let checked = Rc::new(Cell::new(true));
    let on_ok = Rc::new(on_ok);
    let title: SharedString = title.to_string().into();
    let focus = input.read(cx).focus_handle(cx);
    window.open_dialog(cx, move |dialog, _, _| {
        let (input, checked, on_ok) = (input.clone(), checked.clone(), on_ok.clone());
        let checkbox = option.map(|label| {
            let checked_for_click = checked.clone();
            Checkbox::new("prompt-option").label(label).checked(checked.get()).on_click(move |value, window, _| {
                checked_for_click.set(*value);
                window.refresh();
            })
        });
        dialog
            .title(title.clone())
            .w(px(420.))
            .child(div().flex().flex_col().gap_3().child(Input::new(&input).id("ref-name")).children(checkbox))
            .footer(
                DialogFooter::new()
                    .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                    .child(DialogAction::new().child(Button::new("ok").primary().label(ok_label))),
            )
            .on_ok(move |_, window, cx| {
                let name = input.read(cx).value().trim().to_string();
                if name.is_empty() {
                    return false;
                }
                on_ok(name, checked.get(), window, cx);
                true
            })
    });
    // the dialog takes focus when it first renders; move it to the name field after that
    window.defer(cx, move |window, cx| window.focus(&focus, cx));
}
