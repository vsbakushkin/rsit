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

use crate::log_view::LogView;

/// What the menu acts on: the selected commit and its local branches.
pub struct MenuTarget {
    pub id: ObjectId,
    pub local_branches: Vec<String>,
    pub is_head: bool,
}

pub fn build(menu: PopupMenu, view: WeakEntity<LogView>, cx: &mut App) -> PopupMenu {
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
        MenuTarget { id, local_branches, is_head: data.refs.head == Some(id) }
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
fn prompt_name(
    title: &str,
    ok_label: &'static str,
    option: Option<&'static str>,
    window: &mut Window,
    cx: &mut App,
    on_ok: impl Fn(String, bool, &mut Window, &mut App) + 'static,
) {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("Name"));
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
