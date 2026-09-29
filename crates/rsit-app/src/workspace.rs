//! Main window: the Commit tool window on the left (Alt+0) and the Log.

use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::*;
use rsit_git::Repo;
use rsit_log::LogFilter;

use crate::commit_panel::CommitPanel;
use crate::log_view::LogView;

actions!(workspace, [ToggleCommitPanel]);

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("alt-0", ToggleCommitPanel, None)]);
}

pub struct Workspace {
    pub log: Entity<LogView>,
    pub commit: Entity<CommitPanel>,
    show_commit: bool,
}

impl Workspace {
    /// `watch: false` disables file system watching (for tests).
    pub fn new(repo: Repo, filter: LogFilter, watch: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let commit = cx.new(|cx| CommitPanel::new(repo.clone(), watch, window, cx));
        let log = cx.new(|cx| LogView::with_options(repo, filter, watch, window, cx));
        Self { log, commit, show_commit: true }
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .on_action(cx.listener(|this, _: &ToggleCommitPanel, _, cx| {
                this.show_commit = !this.show_commit;
                cx.notify();
            }))
            .child(
                h_resizable("workspace")
                    .child(resizable_panel().size(px(360.)).visible(self.show_commit).child(self.commit.clone()))
                    .child(resizable_panel().child(self.log.clone())),
            )
    }
}
