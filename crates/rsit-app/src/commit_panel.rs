//! The Commit tool window (IntelliJ with the staging area enabled): staged,
//! unstaged, unversioned and conflicted files, the commit message, Amend and Commit.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rsit_git::changes::{Status, StatusEntry};
use rsit_git::ops::Operation;
use rsit_git::{ChangeKind, FileChange, Repo, Revision};

use crate::diff_model::{ChangeAction, DiffItem};

const CONTEXT: &str = "CommitPanel";
const ROW_HEIGHT: f32 = 22.0;

actions!(commit, [StageSelected, UnstageSelected, RollbackSelected, ShowSelectedDiff, RefreshChanges, CommitAndPush]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-alt-a", StageSelected, Some(CONTEXT)),
        KeyBinding::new("ctrl-alt-z", RollbackSelected, Some(CONTEXT)),
        KeyBinding::new("ctrl-d", ShowSelectedDiff, Some(CONTEXT)),
        KeyBinding::new("enter", ShowSelectedDiff, Some(CONTEXT)),
        KeyBinding::new("f5", RefreshChanges, Some(CONTEXT)),
        KeyBinding::new("ctrl-alt-k", CommitAndPush, None),
    ]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Conflicts,
    Staged,
    Unstaged,
    Unversioned,
}

impl Group {
    const ALL: [Group; 4] = [Group::Conflicts, Group::Staged, Group::Unstaged, Group::Unversioned];

    fn title(self) -> &'static str {
        match self {
            Group::Conflicts => "Merge Conflicts",
            Group::Staged => "Staged",
            Group::Unstaged => "Changes",
            Group::Unversioned => "Unversioned Files",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Group::Conflicts => "conflicts",
            Group::Staged => "staged",
            Group::Unstaged => "unstaged",
            Group::Unversioned => "unversioned",
        }
    }
}

pub struct CommitPanel {
    repo: Repo,
    status: Status,
    /// A merge/rebase/cherry-pick/revert waiting to be continued or aborted.
    operation: Option<Operation>,
    loaded: bool,
    selected: Option<(Group, String)>,
    message: Entity<TextareaState>,
    amend: bool,
    /// Message typed before Amend replaced it with the last commit's.
    message_before_amend: Option<String>,
    busy: bool,
    focus: FocusHandle,
    _refresh: Option<Task<()>>,
    _watch: Option<(rsit_log::watch::RefsWatcher, Task<()>)>,
    _subscriptions: Vec<Subscription>,
}

impl CommitPanel {
    pub fn new(repo: Repo, watch: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| TextareaState::new(window, cx).placeholder("Commit Message"));
        let mut subscriptions = vec![cx.subscribe_in(&message, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { secondary: true, .. } = event {
                this.commit(window, cx);
            }
        })];
        // IntelliJ refreshes local changes when the IDE window gets focus
        subscriptions.push(cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.refresh(cx);
            }
        }));
        let mut this = Self {
            repo,
            status: Status::default(),
            operation: None,
            loaded: false,
            selected: None,
            message,
            amend: false,
            message_before_amend: None,
            busy: false,
            focus: cx.focus_handle(),
            _refresh: None,
            _watch: None,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        if watch {
            this.watch(cx);
        }
        this
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    fn watch(&mut self, cx: &mut Context<Self>) {
        let Ok((watcher, mut events)) = rsit_log::watch::watch_repo(&self.repo) else {
            return;
        };
        let task = cx.spawn(async move |this, cx| {
            use futures::StreamExt as _;
            while events.next().await.is_some() {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                while events.try_recv().is_ok() {}
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        self._watch = Some((watcher, task));
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        self._refresh = Some(cx.spawn(async move |this, cx| {
            let (status, operation) = cx
                .background_spawn(async move {
                    (rsit_git::changes::status(repo.cwd()), rsit_git::ops::operation_in_progress(&repo))
                })
                .await;
            this.update(cx, |this, cx| {
                this.operation = operation;
                match status {
                    Ok(status) => {
                        this.status = status;
                        this.loaded = true;
                        if let Some((group, path)) = &this.selected
                            && !this.entries(*group).any(|e| &e.path == path)
                        {
                            this.selected = None;
                        }
                    }
                    Err(e) => eprintln!("rsit: {e:#}"),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn entries(&self, group: Group) -> Box<dyn Iterator<Item = &StatusEntry> + '_> {
        match group {
            Group::Conflicts => Box::new(self.status.conflicted()),
            Group::Staged => Box::new(self.status.staged()),
            Group::Unstaged => Box::new(self.status.unstaged()),
            Group::Unversioned => Box::new(self.status.untracked()),
        }
    }

    /// Runs a git operation in the background, then refreshes; failures become notifications.
    fn run(
        &mut self,
        op: impl FnOnce(&std::path::Path) -> anyhow::Result<Option<String>> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cwd = self.repo.cwd().to_path_buf();
        self.busy = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { op(&cwd) }).await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(Some(message)) => window.push_notification(Notification::success(message), cx),
                    Ok(None) => {}
                    Err(e) => window.push_notification(Notification::error(format!("{e:#}")).autohide(false), cx),
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    fn stage(&mut self, paths: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        if !paths.is_empty() {
            self.run(move |cwd| rsit_git::changes::stage(cwd, &paths).map(|_| None), window, cx);
        }
    }

    fn unstage(&mut self, paths: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let has_head = self.status.has_head;
        if !paths.is_empty() {
            self.run(move |cwd| rsit_git::changes::unstage(cwd, &paths, has_head).map(|_| None), window, cx);
        }
    }

    /// IntelliJ "Rollback": discard local changes (asks first; this loses work).
    fn rollback(&mut self, group: Group, paths: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() || group == Group::Conflicts {
            return;
        }
        let view = cx.entity().downgrade();
        let what = if paths.len() == 1 { paths[0].clone() } else { format!("{} files", paths.len()) };
        let (title, detail) = match group {
            Group::Unversioned => ("Delete Files", format!("Delete {what} from disk?")),
            Group::Staged => ("Rollback Changes", format!("Discard staged and local changes in {what}?")),
            _ => ("Rollback Changes", format!("Discard local changes in {what}?")),
        };
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let (view, paths) = (view.clone(), paths.clone());
            dialog.title(title).description(detail.clone()).on_ok(move |_, window, cx| {
                let paths = paths.clone();
                view.update(cx, |this, cx| {
                    this.run(
                        move |cwd| {
                            match group {
                                Group::Unversioned => rsit_git::changes::delete_untracked(cwd, &paths)?,
                                Group::Staged => rsit_git::changes::rollback(cwd, &paths, true)?,
                                _ => rsit_git::changes::rollback(cwd, &paths, false)?,
                            }
                            Ok(None)
                        },
                        window,
                        cx,
                    )
                })
                .ok();
                true
            })
        });
    }

    fn accept_side(&mut self, path: String, ours: bool, window: &mut Window, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        self.run(
            move |_| rsit_git::conflicts::accept_side(&repo, &path, ours).map(|_| Some(format!("Resolved {path}"))),
            window,
            cx,
        );
    }

    fn finish_operation(&mut self, abort: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(op) = self.operation else { return };
        let (verb, done) = if abort { ("Aborting", "aborted") } else { ("Continuing", "completed") };
        let success = format!("{} {done}", op.name());
        crate::tasks::run_git_task(
            format!("{verb} {}", op.name().to_lowercase()),
            self.repo.cwd().to_path_buf(),
            Some(success),
            Box::new(move |cwd, _| {
                if abort {
                    rsit_git::ops::abort_operation(cwd, op).map(|_| String::new())
                } else {
                    rsit_git::ops::continue_operation(cwd, op)
                }
            }),
            window,
            cx,
        );
    }

    fn render_operation(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let op = self.operation?;
        let theme = cx.theme();
        let conflicts = self.status.conflicted().count();
        let text = if conflicts > 0 {
            format!("{} in progress: {conflicts} conflict{}", op.name(), if conflicts == 1 { "" } else { "s" })
        } else {
            format!("{} in progress", op.name())
        };
        Some(
            div()
                .id("operation-banner")
                .test_support()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .bg(theme.warning.opacity(0.15))
                .border_b_1()
                .border_color(theme.border)
                .child(div().flex_1().min_w(rems(10.)).child(text))
                .when(conflicts > 0, |d| {
                    d.child(Button::new("resolve-conflicts").xsmall().primary().label("Resolve…").on_click(
                        cx.listener(|this, _, _, cx| {
                            if let Some(path) = this.status.conflicted().next().map(|e| e.path.clone()) {
                                crate::merge_view::open(this.repo.clone(), path, cx);
                            }
                        }),
                    ))
                })
                .when(op != Operation::Other, |d| {
                    d.child(
                        Button::new("continue-operation")
                            .xsmall()
                            .outline()
                            .label("Continue")
                            .disabled(conflicts > 0)
                            .on_click(cx.listener(|this, _, window, cx| this.finish_operation(false, window, cx))),
                    )
                    .child(
                        Button::new("abort-operation")
                            .xsmall()
                            .ghost()
                            .label("Abort")
                            .on_click(cx.listener(|this, _, window, cx| this.finish_operation(true, window, cx))),
                    )
                }),
        )
    }

    fn set_amend(&mut self, amend: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.amend = amend;
        let current = self.message.read(cx).value().to_string();
        if amend {
            if current.trim().is_empty()
                && let Ok(last) = rsit_git::changes::last_commit_message(self.repo.cwd())
            {
                self.message_before_amend = Some(current);
                self.message.update(cx, |m, cx| m.set_value(last, window, cx));
            }
        } else if let Some(previous) = self.message_before_amend.take() {
            self.message.update(cx, |m, cx| m.set_value(previous, window, cx));
        }
        cx.notify();
    }

    fn can_commit(&self, cx: &App) -> bool {
        !self.busy
            && self.status.conflicted().next().is_none()
            && (self.amend || self.status.staged().next().is_some())
            && !self.message.read(cx).value().trim().is_empty()
    }

    pub fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_then(false, window, cx)
    }

    /// Commits, then optionally pushes the current branch (IntelliJ "Commit and Push").
    pub fn commit_then(&mut self, push: bool, window: &mut Window, cx: &mut Context<Self>) {
        let message = self.message.read(cx).value().to_string();
        if !self.can_commit(cx) {
            let reason = if message.trim().is_empty() {
                "Enter a commit message"
            } else if self.status.conflicted().next().is_some() {
                "Resolve merge conflicts first"
            } else {
                "No staged changes to commit"
            };
            window.push_notification(Notification::warning(reason), cx);
            return;
        }
        let amend = self.amend;
        let count = self.status.staged().count();
        let subject = message.lines().next().unwrap_or("").to_string();
        let view = cx.entity().downgrade();
        let cwd = self.repo.cwd().to_path_buf();
        self.busy = true;
        cx.notify();
        cx.spawn_in(window, async move |_, cx| {
            let result = cx.background_spawn(async move { rsit_git::changes::commit(&cwd, &message, amend) }).await;
            view.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(_) => {
                        let files = if count == 1 { "1 file".to_string() } else { format!("{count} files") };
                        let verb = if amend { "Amended" } else { "Committed" };
                        window.push_notification(Notification::success(format!("{verb} {files}: {subject}")), cx);
                        this.message.update(cx, |m, cx| m.set_value("", window, cx));
                        this.amend = false;
                        this.message_before_amend = None;
                        if push {
                            crate::git_actions::push_current(&this.repo, false, window, cx);
                        }
                    }
                    Err(e) => window.push_notification(Notification::error(format!("{e:#}")).autohide(false), cx),
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Diff items of a group, IntelliJ-style: staged = HEAD vs index, local = index vs disk.
    fn diff_items(&self, group: Group) -> Vec<DiffItem> {
        let head = rsit_git::read_refs(&self.repo.local()).ok().and_then(|r| r.head);
        self.entries(group)
            .map(|e| {
                let (kind, left, right, labels, action) = match group {
                    Group::Staged => {
                        let kind = e.staged.unwrap_or(ChangeKind::Modified);
                        let left = head.filter(|_| kind != ChangeKind::Added).map(Revision::Commit);
                        let right = (kind != ChangeKind::Deleted).then_some(Revision::Index);
                        (kind, left, right, ("HEAD", "Staged"), Some(ChangeAction::Unstage))
                    }
                    Group::Unstaged => {
                        let kind = e.unstaged.unwrap_or(ChangeKind::Modified);
                        let right = (kind != ChangeKind::Deleted).then_some(Revision::WorkTree);
                        (kind, Some(Revision::Index), right, ("Staged", "Local"), Some(ChangeAction::Stage))
                    }
                    Group::Unversioned => (ChangeKind::Added, None, Some(Revision::WorkTree), ("", "Local"), None),
                    Group::Conflicts => (
                        ChangeKind::Modified,
                        head.map(Revision::Commit),
                        Some(Revision::WorkTree),
                        ("HEAD", "Local"),
                        None,
                    ),
                };
                // renames only exist in the index; the working tree side uses the new path
                let old_path = if group == Group::Staged { e.orig_path.clone() } else { None };
                DiffItem {
                    change: FileChange { kind, path: e.path.clone(), old_path },
                    left,
                    right,
                    left_label: labels.0.into(),
                    right_label: labels.1.into(),
                    action,
                }
            })
            .collect()
    }

    fn show_diff(&mut self, group: Group, path: &str, cx: &mut Context<Self>) {
        let items = self.diff_items(group);
        let index = items.iter().position(|i| i.change.path == path).unwrap_or(0);
        if !items.is_empty() {
            crate::diff_view::open_items(self.repo.clone(), format!("{} — {}", group.title(), path), items, index, cx);
        }
    }

    fn with_selection(&mut self, f: impl FnOnce(&mut Self, Group, String)) {
        if let Some((group, path)) = self.selected.clone() {
            f(self, group, path);
        }
    }

    // ---- rendering ----

    fn render_group(&mut self, group: Group, cx: &mut Context<Self>) -> Option<AnyElement> {
        let entries: Vec<StatusEntry> = self.entries(group).cloned().collect();
        if entries.is_empty() && group != Group::Staged && group != Group::Unstaged {
            return None;
        }
        let theme = cx.theme();
        let (muted, hover, active) = (theme.muted_foreground, theme.list_hover, theme.list_active);
        let paths: Vec<String> = entries.iter().map(|e| e.path.clone()).collect();
        let header_action = match group {
            Group::Staged => Some((IconName::Minus, "Unstage All")),
            Group::Unstaged => Some((IconName::Plus, "Stage All")),
            Group::Unversioned => Some((IconName::Plus, "Add All")),
            Group::Conflicts => None,
        };
        let header = div()
            .id(SharedString::from(format!("group-{}", group.id())))
            .test_support()
            .h(rems(ROW_HEIGHT / 16.))
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(group.title()))
            .child(div().text_color(muted).child(format!("{}", entries.len())))
            .child(div().flex_1())
            .children(header_action.filter(|_| !entries.is_empty()).map(|(icon, tooltip)| {
                let paths = paths.clone();
                Button::new(SharedString::from(format!("all-{}", group.id())))
                    .xsmall()
                    .ghost()
                    .icon(icon)
                    .tooltip(tooltip)
                    .on_click(cx.listener(move |this, _, window, cx| match group {
                        Group::Staged => this.unstage(paths.clone(), window, cx),
                        _ => this.stage(paths.clone(), window, cx),
                    }))
            }));
        let rows = entries.into_iter().map(|e| {
            let kind = match group {
                Group::Staged => e.staged,
                Group::Unstaged => e.unstaged,
                Group::Unversioned => Some(ChangeKind::Added),
                Group::Conflicts => Some(ChangeKind::Modified),
            }
            .unwrap_or(ChangeKind::Modified);
            let (letter, mut color) = crate::log_view::change_style(kind);
            if group == Group::Unversioned {
                color = rgb(0xC7222D).into(); // IntelliJ unversioned red
            } else if group == Group::Conflicts {
                color = rgb(0xD5756C).into();
            }
            let (dir, name) = match e.path.rsplit_once('/') {
                Some((d, n)) => (d.to_string(), n.to_string()),
                None => (String::new(), e.path.clone()),
            };
            let name = match &e.orig_path {
                Some(orig) if group == Group::Staged => format!("{name} ← {orig}"),
                _ => name,
            };
            let path = e.path.clone();
            let selected = self.selected.as_ref().is_some_and(|(g, p)| *g == group && *p == path);
            let row_action = match group {
                Group::Staged => Some(IconName::Minus),
                Group::Unstaged | Group::Unversioned => Some(IconName::Plus),
                Group::Conflicts => None,
            };
            let (p1, p2, p3) = (path.clone(), path.clone(), path.clone());
            div()
                .id(SharedString::from(format!("{}:{}", group.id(), path)))
                .test_support()
                .group("change-row")
                .h(rems(ROW_HEIGHT / 16.))
                .pl_4()
                .pr_1()
                .flex()
                .items_center()
                .gap_2()
                .when(selected, |d| d.bg(active))
                .when(!selected, |d| d.hover(|s| s.bg(hover)))
                .child(div().w(rems(0.75)).text_color(color).child(letter.to_string()))
                .child(div().text_color(color).whitespace_nowrap().child(name))
                .child(div().flex_1().min_w_0().truncate().text_color(muted).child(dir))
                .children(row_action.map(|icon| {
                    Button::new(SharedString::from(format!("act-{}:{}", group.id(), p3)))
                        .xsmall()
                        .ghost()
                        .icon(icon)
                        .on_click(cx.listener(move |this, _, window, cx| match group {
                            Group::Staged => this.unstage(vec![p1.clone()], window, cx),
                            _ => this.stage(vec![p1.clone()], window, cx),
                        }))
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, window, cx| {
                        window.focus(&this.focus, cx);
                        this.selected = Some((group, p2.clone()));
                        cx.notify();
                    }),
                )
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    this.selected = Some((group, path.clone()));
                    if event.click_count() >= 2 {
                        if group == Group::Conflicts {
                            crate::merge_view::open(this.repo.clone(), path.clone(), cx);
                        } else {
                            this.show_diff(group, &path, cx);
                        }
                    }
                    cx.notify();
                }))
                .into_any_element()
        });
        Some(div().flex().flex_col().child(header).children(rows).into_any_element())
    }

    fn render_changes(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let groups: Vec<AnyElement> = Group::ALL.iter().filter_map(|&g| self.render_group(g, cx)).collect();
        let empty = self.loaded && self.status.entries.is_empty();
        let view = cx.entity().downgrade();
        div()
            .id("changes-tree")
            .test_support()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &StageSelected, window, cx| {
                this.with_selection(|this, group, path| match group {
                    Group::Staged => this.unstage(vec![path], window, cx),
                    Group::Conflicts => {}
                    _ => this.stage(vec![path], window, cx),
                })
            }))
            .on_action(cx.listener(|this, _: &RollbackSelected, window, cx| {
                this.with_selection(|this, group, path| this.rollback(group, vec![path], window, cx))
            }))
            .on_action(cx.listener(|this, _: &ShowSelectedDiff, _, cx| {
                this.with_selection(|this, group, path| this.show_diff(group, &path, cx))
            }))
            .on_action(cx.listener(|this, _: &RefreshChanges, _, cx| this.refresh(cx)))
            .on_action(cx.listener(|this, _: &CommitAndPush, window, cx| this.commit_then(true, window, cx)))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py_1()
            .children(groups)
            .when(empty, |d| d.child(div().p_4().text_color(muted).child("No local changes")))
            .context_menu(move |menu, _, cx| {
                let Some(this) = view.upgrade() else {
                    return menu;
                };
                let Some((group, path)) = this.read(cx).selected.clone() else {
                    return menu;
                };
                let entity = this.downgrade();
                let item =
                    |label: &str, f: fn(&mut CommitPanel, Group, String, &mut Window, &mut Context<CommitPanel>)| {
                        let (entity, path) = (entity.clone(), path.clone());
                        PopupMenuItem::new(label.to_string()).on_click(move |_, window, cx| {
                            let path = path.clone();
                            entity.update(cx, |this, cx| f(this, group, path, window, cx)).ok();
                        })
                    };
                let mut menu = menu;
                match group {
                    Group::Staged => menu = menu.item(item("Unstage", |t, _, p, w, cx| t.unstage(vec![p], w, cx))),
                    Group::Unstaged | Group::Unversioned => {
                        menu = menu.item(item("Stage", |t, _, p, w, cx| t.stage(vec![p], w, cx)))
                    }
                    Group::Conflicts => {
                        menu = menu
                            .item(item("Merge…", |t, _, p, _, cx| crate::merge_view::open(t.repo.clone(), p, cx)))
                            .item(item("Accept Yours", |t, _, p, w, cx| t.accept_side(p, true, w, cx)))
                            .item(item("Accept Theirs", |t, _, p, w, cx| t.accept_side(p, false, w, cx)))
                            .separator();
                    }
                }
                menu = menu.item(item("Show Diff", |t, g, p, _, cx| t.show_diff(g, &p, cx)));
                if group != Group::Unversioned {
                    menu = menu
                        .item(item("Show History", |t, _, p, _, cx| {
                            crate::file_view::open(t.repo.clone(), p, None, crate::file_view::FileTab::History, cx)
                        }))
                        .item(item("Annotate", |t, _, p, _, cx| {
                            crate::file_view::open(t.repo.clone(), p, None, crate::file_view::FileTab::Annotate, cx)
                        }));
                }
                match group {
                    Group::Unversioned => {
                        menu = menu.separator().item(item("Delete…", |t, g, p, w, cx| t.rollback(g, vec![p], w, cx)))
                    }
                    Group::Staged | Group::Unstaged => {
                        menu =
                            menu.separator().item(item("Rollback…", |t, g, p, w, cx| t.rollback(g, vec![p], w, cx)))
                    }
                    Group::Conflicts => {}
                }
                let copy = path.clone();
                menu.separator().item(
                    PopupMenuItem::new("Copy Path")
                        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))),
                )
            })
    }
}

impl Render for CommitPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.border;
        let can_commit = self.can_commit(cx);
        let staged = self.status.staged().count();
        let commit_label = match (self.amend, staged) {
            (true, _) => "Amend Commit".to_string(),
            (false, 0) => "Commit".to_string(),
            (false, n) => format!("Commit {n} file{}", if n == 1 { "" } else { "s" }),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_2()
                    .h(rems(1.875))
                    .border_b_1()
                    .border_color(border)
                    .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child("Commit"))
                    .child(
                        Button::new("refresh-changes")
                            .xsmall()
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .tooltip("Refresh (F5)")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .children(self.render_operation(cx))
            .child(self.render_changes(cx))
            .child(
                div()
                    .border_t_1()
                    .border_color(border)
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        Checkbox::new("amend").label("Amend").checked(self.amend).on_click(
                            cx.listener(|this, checked: &bool, window, cx| this.set_amend(*checked, window, cx)),
                        ),
                    )
                    .child(div().id("commit-message").test_support().child(Textarea::new(&self.message).h(rems(6.875))))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("commit")
                                    .primary()
                                    .small()
                                    .label(commit_label)
                                    .tooltip("Ctrl+Enter")
                                    .loading(self.busy)
                                    .disabled(!can_commit)
                                    .on_click(cx.listener(|this, _, window, cx| this.commit(window, cx))),
                            )
                            .child(
                                Button::new("commit-and-push")
                                    .small()
                                    .outline()
                                    .label("Commit and Push…")
                                    .tooltip("Ctrl+Alt+K")
                                    .disabled(!can_commit)
                                    .on_click(cx.listener(|this, _, window, cx| this.commit_then(true, window, cx))),
                            ),
                    ),
            )
    }
}
