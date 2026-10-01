//! Main window: a toolbar with the branch widget and remote actions, the
//! Commit tool window on the left (Alt+0) and the Log.

use std::path::Path;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::*;
use rsit_git::{RefKind, Repo};
use rsit_log::LogFilter;

use crate::commit_panel::CommitPanel;
use crate::git_actions;
use crate::log_view::{LogLoaded, LogView};
use crate::tasks::Activity;

actions!(workspace, [ToggleCommitPanel, Fetch, Update, Push, GoToFile]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("alt-0", ToggleCommitPanel, None),
        // IntelliJ: Update Project, Push
        KeyBinding::new("ctrl-t", Update, None),
        KeyBinding::new("ctrl-shift-k", Push, None),
        KeyBinding::new("ctrl-shift-n", GoToFile, None),
    ]);
}

/// Opens the main window for `repo`; closing it quits the app, other windows are secondary.
/// `watch: false` disables file system watching (for tests).
pub fn open(repo: Repo, filter: LogFilter, watch: bool, cx: &mut App) {
    if let Some(dir) = repo.workdir().map(Path::to_path_buf) {
        crate::welcome::Recent::add(&dir);
    }
    let title = format!("{} — rsit", repo.display_name());
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some(title.into()), ..Default::default() }),
        window_bounds: Some(WindowBounds::centered(size(px(1500.), px(900.)), cx)),
        app_id: Some("rsit".into()),
        ..Default::default()
    };
    let (main_window, _) =
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Workspace::new(repo, filter, watch, window, cx)))
            .expect("failed to open window");
    cx.on_window_closed(move |cx, closed| {
        if closed == main_window.window_id() {
            cx.quit();
        }
    })
    .detach();
}

#[derive(Clone, Default)]
struct BranchInfo {
    current: Option<String>,
    /// Commits ahead/behind the upstream.
    ahead_behind: Option<(u32, u32)>,
}

pub struct Workspace {
    repo: Repo,
    pub log: Entity<LogView>,
    pub commit: Entity<CommitPanel>,
    show_commit: bool,
    branch: BranchInfo,
    _branch_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    /// `watch: false` disables file system watching (for tests).
    pub fn new(repo: Repo, filter: LogFilter, watch: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let commit = cx.new(|cx| CommitPanel::new(repo.clone(), watch, window, cx));
        let log = cx.new(|cx| LogView::with_options(repo.clone(), filter, watch, window, cx));
        crate::navigator::register(window.window_handle(), log.downgrade(), cx);
        let subscriptions = vec![
            cx.subscribe(&log, |this, _, _: &LogLoaded, cx| this.refresh_branch(cx)),
            cx.observe_global::<Activity>(|_, cx| cx.notify()),
        ];
        let mut this = Self {
            repo,
            log,
            commit,
            show_commit: true,
            branch: BranchInfo::default(),
            _branch_task: None,
            _subscriptions: subscriptions,
        };
        this.refresh_branch(cx);
        this
    }

    fn refresh_branch(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        self._branch_task = Some(cx.spawn(async move |this, cx| {
            let info = cx
                .background_spawn(async move {
                    let current = rsit_git::read_refs(&repo.local()).ok().and_then(|r| r.current_branch);
                    let ahead_behind = current.as_deref().and_then(|b| rsit_git::ops::ahead_behind(repo.cwd(), b));
                    BranchInfo { current, ahead_behind }
                })
                .await;
            this.update(cx, |this, cx| {
                this.branch = info;
                cx.notify();
            })
            .ok();
        }));
    }

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let mut label = self.branch.current.clone().unwrap_or_else(|| "Detached HEAD".into());
        if let Some((ahead, behind)) = self.branch.ahead_behind {
            if ahead > 0 {
                label.push_str(&format!(" ↑{ahead}"));
            }
            if behind > 0 {
                label.push_str(&format!(" ↓{behind}"));
            }
        }
        let repo = self.repo.clone();
        let branches = Button::new("branches")
            .small()
            .ghost()
            .icon(IconName::GitBranch)
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, window, cx| branches_popup(menu, &repo, window, cx));
        let activity = cx.try_global::<Activity>().and_then(Activity::summary);
        let action = |id: &'static str, icon: IconName, tooltip: &'static str| {
            Button::new(id).small().ghost().icon(icon).tooltip(tooltip)
        };
        div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .h(rems(2.125))
            .border_b_1()
            .border_color(border)
            .child(branches)
            .child(
                action("fetch", IconName::RefreshCw, "Fetch")
                    .on_click(cx.listener(|this, _, window, cx| git_actions::fetch(&this.repo, window, cx))),
            )
            .child(
                action("update", IconName::ArrowDownToLine, "Update Project (Ctrl+T)")
                    .on_click(cx.listener(|this, _, window, cx| git_actions::update(&this.repo, window, cx))),
            )
            .child(
                action("push", IconName::ArrowUpFromLine, "Push (Ctrl+Shift+K)").on_click(
                    cx.listener(|this, _, window, cx| git_actions::push_current(&this.repo, false, window, cx)),
                ),
            )
            .child(
                Button::new("go-to-file")
                    .small()
                    .ghost()
                    .icon(IconName::Search)
                    .tooltip("Go to File (Ctrl+Shift+N)")
                    .on_click(
                        cx.listener(|this, _, window, cx| crate::file_picker::open(this.repo.clone(), window, cx)),
                    ),
            )
            .child(div().flex_1())
            .children(activity.map(|a| div().text_color(muted).text_sm().truncate().child(a)))
            .child(
                Button::new("settings")
                    .small()
                    .ghost()
                    .icon(IconName::Settings)
                    .tooltip("Settings (Ctrl+Alt+S)")
                    .on_click(|_, _, cx| crate::settings::open(cx)),
            )
    }
}

/// IntelliJ's Git Branches popup: global actions, then local and remote branches.
fn branches_popup(
    menu: gpui_kit::component::menu::PopupMenu,
    repo: &Repo,
    window: &mut Window,
    cx: &mut Context<gpui_kit::component::menu::PopupMenu>,
) -> gpui_kit::component::menu::PopupMenu {
    let refs = rsit_git::read_refs(&repo.local()).unwrap_or_default();
    let current = refs.current_branch.clone();
    let action = |label: &str, f: fn(&Repo, &mut Window, &mut App)| {
        let repo = repo.clone();
        PopupMenuItem::new(label.to_string()).on_click(move |_, window, cx| f(&repo, window, cx))
    };
    let mut menu = menu
        .max_h(px(600.))
        .scrollable(true)
        .item({
            let repo = repo.clone();
            PopupMenuItem::new("New Branch…")
                .on_click(move |_, window, cx| git_actions::new_branch_from(&repo, "HEAD".into(), window, cx))
        })
        .item(action("Update Project", git_actions::update))
        .item(action("Fetch", git_actions::fetch))
        .item(action("Push", |r, w, cx| git_actions::push_current(r, false, w, cx)))
        .item(action("Force Push…", |r, w, cx| git_actions::push_current(r, true, w, cx)));
    for (title, kind) in [("Local", RefKind::LocalBranch), ("Remote", RefKind::RemoteBranch)] {
        let mut branches: Vec<_> = refs.refs.iter().filter(|r| r.kind == kind).collect();
        if branches.is_empty() {
            continue;
        }
        branches.sort_by(|a, b| refs.label_cmp(a, b));
        menu = menu.separator().label(title);
        for r in branches {
            let (repo, name, current) = (repo.clone(), r.name.clone(), current.clone());
            let label = if current.as_deref() == Some(r.name.as_str()) && kind == RefKind::LocalBranch {
                format!("★ {}", r.name)
            } else {
                r.name.clone()
            };
            menu = menu.submenu(label, window, cx, move |menu, _, _| {
                git_actions::branch_menu(menu, &repo, &name, kind, current.as_deref())
            });
        }
    }
    menu
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, fg) = (theme.background, theme.foreground);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .on_action(cx.listener(|this, _: &ToggleCommitPanel, _, cx| {
                this.show_commit = !this.show_commit;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Fetch, window, cx| git_actions::fetch(&this.repo, window, cx)))
            .on_action(cx.listener(|this, _: &Update, window, cx| git_actions::update(&this.repo, window, cx)))
            .on_action(
                cx.listener(|this, _: &Push, window, cx| git_actions::push_current(&this.repo, false, window, cx)),
            )
            .child(self.render_toolbar(cx))
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("workspace")
                        .child(
                            resizable_panel()
                                .size(px(360.))
                                .visible(self.show_commit)
                                .child(self.commit.clone().cached(StyleRefinement::default().size_full())),
                        )
                        .child(resizable_panel().child(self.log.clone())),
                ),
            )
    }
}
