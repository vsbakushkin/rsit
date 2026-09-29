//! A window for one file with two tabs: Annotate and History.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::*;
use rsit_git::{ObjectId, Repo};

use crate::blame_view::BlameView;
use crate::history_view::HistoryView;

actions!(file_view, [CloseFileView]);

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", CloseFileView, Some("FileView"))]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileTab {
    Annotate,
    History,
}

pub struct FileView {
    repo: Repo,
    path: String,
    rev: Option<ObjectId>,
    tab: FileTab,
    blame: Option<Entity<BlameView>>,
    history: Option<Entity<HistoryView>>,
    focus: FocusHandle,
}

/// Opens `path` (at `rev`, or the working tree) in a new window on `tab`.
pub fn open(repo: Repo, path: String, rev: Option<ObjectId>, tab: FileTab, cx: &mut App) {
    let title = match rev {
        Some(rev) => format!("{path} @ {}", rev.to_hex_with_len(8)),
        None => path.clone(),
    };
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some(title.into()), ..Default::default() }),
        window_bounds: Some(WindowBounds::centered(size(px(1400.), px(900.)), cx)),
        app_id: Some("rsit".into()),
        ..Default::default()
    };
    let result =
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| FileView::new(repo, path, rev, tab, window, cx)));
    if let Err(e) = result {
        eprintln!("rsit: cannot open file window: {e:#}");
    }
}

impl FileView {
    pub fn new(
        repo: Repo,
        path: String,
        rev: Option<ObjectId>,
        tab: FileTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut this = Self { repo, path, rev, tab, blame: None, history: None, focus };
        this.show(tab, window, cx);
        this
    }

    pub fn blame(&self) -> Option<&Entity<BlameView>> {
        self.blame.as_ref()
    }

    pub fn history(&self) -> Option<&Entity<HistoryView>> {
        self.history.as_ref()
    }

    /// Switches tabs; each tab loads the first time it is shown.
    pub fn show(&mut self, tab: FileTab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        match tab {
            FileTab::Annotate if self.blame.is_none() => {
                let (repo, path, rev) = (self.repo.clone(), self.path.clone(), self.rev);
                self.blame = Some(cx.new(|cx| BlameView::new(repo, path, rev, cx)));
            }
            FileTab::History if self.history.is_none() => {
                let (repo, path, rev) = (self.repo.clone(), self.path.clone(), self.rev);
                let history = cx.new(|cx| HistoryView::new(repo, path, rev, window, cx));
                window.focus(&history.read(cx).focus_handle().clone(), cx);
                self.history = Some(history);
            }
            _ => {}
        }
        cx.notify();
    }
}

impl Render for FileView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, fg, border) = (theme.background, theme.foreground, theme.border);
        let tab = |id: &'static str, label: &'static str, this_tab: FileTab, current: FileTab| {
            let button = Button::new(id).small().label(label).toggled(this_tab == current);
            if this_tab == current { button.outline() } else { button.ghost() }
        };
        let body: AnyElement = match self.tab {
            FileTab::Annotate => self.blame.clone().map(|b| b.into_any_element()),
            FileTab::History => self.history.clone().map(|h| h.into_any_element()),
        }
        .unwrap_or_else(|| div().into_any_element());
        div()
            .id("file-view")
            .test_support()
            .key_context("FileView")
            .track_focus(&self.focus)
            .on_action(|_: &CloseFileView, window, _| window.remove_window())
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        tab("tab-annotate", "Annotate", FileTab::Annotate, self.tab)
                            .on_click(cx.listener(|this, _, window, cx| this.show(FileTab::Annotate, window, cx))),
                    )
                    .child(
                        tab("tab-history", "History", FileTab::History, self.tab)
                            .on_click(cx.listener(|this, _, window, cx| this.show(FileTab::History, window, cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}
