//! File history (IntelliJ "Show History"): commits that changed the file,
//! following renames, with the file's diff in the selected revision.

use std::ops::Range;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rsit_git::history::FileRevision;
use rsit_git::{ChangeKind, FileChange, ObjectId, Repo};

use crate::diff_model::DiffItem;
use crate::diff_view::DiffView;
use crate::file_view::{self, FileTab};

const CONTEXT: &str = "FileHistory";
const ROW_HEIGHT: f32 = 22.0;

actions!(history, [HistoryPrev, HistoryNext]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", HistoryPrev, Some(CONTEXT)),
        KeyBinding::new("down", HistoryNext, Some(CONTEXT)),
    ]);
}

pub struct HistoryView {
    repo: Repo,
    path: String,
    rev: Option<ObjectId>,
    revisions: Vec<FileRevision>,
    selected: Option<usize>,
    diff: Entity<DiffView>,
    error: Option<SharedString>,
    loaded: bool,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    _load: Option<Task<()>>,
}

impl HistoryView {
    pub fn new(repo: Repo, path: String, rev: Option<ObjectId>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let diff = cx.new(|cx| DiffView::embedded(repo.clone(), window, cx));
        let mut this = Self {
            repo,
            path,
            rev,
            revisions: Vec::new(),
            selected: None,
            diff,
            error: None,
            loaded: false,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            _load: None,
        };
        this.load(cx);
        this
    }

    pub fn revisions(&self) -> &[FileRevision] {
        &self.revisions
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let (cwd, path, rev) = (self.repo.cwd().to_path_buf(), self.path.clone(), self.rev.map(|r| r.to_string()));
        self._load = Some(cx.spawn(async move |this, cx| {
            let history =
                cx.background_spawn(async move { rsit_git::history::file_history(&cwd, &path, rev.as_deref()) }).await;
            this.update(cx, |this, cx| {
                this.loaded = true;
                match history {
                    Ok(revisions) => {
                        this.revisions = revisions;
                        if !this.revisions.is_empty() {
                            this.select(0, cx);
                        }
                    }
                    Err(e) => this.error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(r) = self.revisions.get(index) else { return };
        self.selected = Some(index);
        self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        let change = FileChange { kind: r.kind, path: r.path.clone(), old_path: r.old_path.clone() };
        let item = DiffItem::for_commit(change, r.parents.first().copied(), r.commit);
        self.diff.update(cx, |diff, cx| diff.set_items(vec![item], 0, cx));
        cx.notify();
    }

    fn move_selection(&mut self, delta: i64, cx: &mut Context<Self>) {
        if self.revisions.is_empty() {
            return;
        }
        let next = self.selected.map_or(0, |s| s as i64 + delta).clamp(0, self.revisions.len() as i64 - 1);
        self.select(next as usize, cx);
    }

    fn render_rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (muted, active, hover, mono) =
            (theme.muted_foreground, theme.list_active, theme.list_hover, theme.mono_font_family.clone());
        range
            .map(|i| {
                let r = &self.revisions[i];
                let selected = self.selected == Some(i);
                let note = match (&r.kind, &r.old_path) {
                    (ChangeKind::Renamed, Some(old)) => Some(format!("renamed from {old}")),
                    (ChangeKind::Added, _) => Some("added".to_string()),
                    (ChangeKind::Deleted, _) => Some("deleted".to_string()),
                    _ => None,
                };
                let (commit, path) = (r.commit, r.path.clone());
                div()
                    .id(("revision", i))
                    .test_support()
                    .h(rems(ROW_HEIGHT / 16.))
                    .w_full()
                    .flex()
                    .items_center()
                    .when(selected, |d| d.bg(active))
                    .when(!selected, |d| d.hover(|s| s.bg(hover)))
                    .child(
                        div()
                            .w(rems(5.25))
                            .flex_none()
                            .px_2()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .font_family(mono.clone())
                            .text_color(muted)
                            .child(r.commit.to_hex_with_len(8).to_string()),
                    )
                    .child(
                        div()
                            .w(rems(5.625))
                            .flex_none()
                            .px_1()
                            .whitespace_nowrap()
                            .text_color(muted)
                            .child(format_date(r.time)),
                    )
                    .child(
                        div()
                            .w(rems(8.125))
                            .flex_shrink(1.)
                            .min_w(rems(2.5))
                            .px_1()
                            .truncate()
                            .text_color(muted)
                            .child(r.author.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(rems(6.25))
                            .px_1()
                            .flex()
                            .gap_2()
                            .overflow_hidden()
                            .child(div().truncate().child(r.subject.clone()))
                            .children(note.map(|n| div().flex_none().text_xs().text_color(muted).child(n))),
                    )
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        window.focus(&this.focus, cx);
                        this.select(i, cx);
                        if event.click_count() >= 2 {
                            crate::navigator::select_in_log(commit, cx);
                        }
                    }))
                    .context_menu({
                        let repo = self.repo.clone();
                        move |menu, _, _| {
                            let (repo_a, path_a) = (repo.clone(), path.clone());
                            let repo_b = repo.clone();
                            menu.item(PopupMenuItem::new("Annotate Revision").on_click(move |_, _, cx| {
                                file_view::open(repo_a.clone(), path_a.clone(), Some(commit), FileTab::Annotate, cx)
                            }))
                            .item(PopupMenuItem::new("Show All Affected Files").on_click(move |_, _, cx| {
                                let files = rsit_git::changed_files(&repo_b.local(), commit).unwrap_or_default();
                                if !files.is_empty() {
                                    crate::diff_view::open(repo_b.clone(), commit, files, 0, cx);
                                }
                            }))
                            .item(PopupMenuItem::new("Select in Log").on_click(move |_, _, cx| {
                                crate::navigator::select_in_log(commit, cx);
                            }))
                            .separator()
                            .item(PopupMenuItem::new("Copy Revision Number").on_click(
                                move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(commit.to_string())),
                            ))
                        }
                    })
                    .into_any_element()
            })
            .collect()
    }
}

impl Render for HistoryView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let list: AnyElement = if let Some(error) = &self.error {
            div().p_4().text_color(muted).child(error.clone()).into_any_element()
        } else if !self.loaded {
            div().p_4().text_color(muted).child("Loading history…").into_any_element()
        } else if self.revisions.is_empty() {
            div().p_4().text_color(muted).child("No commits changed this file").into_any_element()
        } else {
            uniform_list("history-rows", self.revisions.len(), cx.processor(Self::render_rows))
                .track_scroll(&self.scroll)
                .size_full()
                .into_any_element()
        };
        div()
            .id("file-history")
            .test_support()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &HistoryPrev, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &HistoryNext, _, cx| this.move_selection(1, cx)))
            .size_full()
            .child(
                h_resizable("history-split")
                    .child(resizable_panel().size(px(560.)).child(div().size_full().text_sm().child(list)))
                    .child(resizable_panel().child(self.diff.clone())),
            )
    }
}

fn format_date(secs: i64) -> String {
    use chrono::{Local, TimeZone};
    Local.timestamp_opt(secs, 0).single().map(|t| t.format("%d.%m.%Y").to_string()).unwrap_or_default()
}
