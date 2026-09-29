//! Diff viewer for the files of a commit (IntelliJ "Show Diff"): the changed
//! files on the left, the selected file's diff on the right, side-by-side
//! (aligned) or unified, with syntax and changed-word highlighting.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rsit_diff::{Change, ChangeKind, DiffRows, Folding, Layout, Row, WhitespacePolicy};
use rsit_git::{FileChange, ObjectId, Repo};

use crate::diff_model::{ChangeAction, DiffItem, DiffSide, FileDiff};

const CONTEXT: &str = "DiffView";
const ROW_HEIGHT: f32 = 20.0;
const GUTTER: f32 = 48.0;
const FOLD_CONTEXT: u32 = 4;
const TAB_WIDTH: usize = 4;
/// Approximate advance of a monospace glyph at the diff font size, used to
/// bound horizontal scrolling.
const CHAR_WIDTH: f32 = 8.0;

actions!(diff, [NextChange, PrevChange, NextFile, PrevFile, CloseDiff, ApplyChangeAction]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("f7", NextChange, Some(CONTEXT)),
        KeyBinding::new("shift-f7", PrevChange, Some(CONTEXT)),
        KeyBinding::new("alt-down", NextFile, Some(CONTEXT)),
        KeyBinding::new("alt-up", PrevFile, Some(CONTEXT)),
        KeyBinding::new("escape", CloseDiff, Some(CONTEXT)),
        KeyBinding::new("ctrl-alt-a", ApplyChangeAction, Some(CONTEXT)),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Settings {
    layout: Layout,
    policy: WhitespacePolicy,
    collapse: bool,
    words: bool,
}

pub struct DiffView {
    repo: Repo,
    files: Vec<DiffItem>,
    selected: usize,
    settings: Settings,
    diff: Option<Arc<FileDiff>>,
    rows: DiffRows,
    expanded: HashSet<u32>,
    current_change: Option<usize>,
    /// Horizontal scroll of both sides, in pixels.
    h_offset: f32,
    /// Widest line in characters (tabs expanded), for the scroll limit.
    max_columns: usize,
    error: Option<SharedString>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    _load: Option<Task<()>>,
}

/// Opens a diff window for `files` of `commit` (against its first parent), starting at `selected`.
pub fn open(repo: Repo, commit: ObjectId, files: Vec<FileChange>, selected: usize, cx: &mut App) {
    let parent = rsit_git::first_parent(&repo.local(), commit).ok().flatten();
    let items = files.into_iter().map(|f| DiffItem::for_commit(f, parent, commit)).collect();
    open_items(repo, format!("Changes in {}", commit.to_hex_with_len(8)), items, selected, cx);
}

/// Opens a diff window over arbitrary items (e.g. local changes).
pub fn open_items(repo: Repo, title: String, items: Vec<DiffItem>, selected: usize, cx: &mut App) {
    let title: SharedString = title.into();
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some(title), ..Default::default() }),
        window_bounds: Some(WindowBounds::centered(size(px(1500.), px(900.)), cx)),
        app_id: Some("rsit".into()),
        ..Default::default()
    };
    let result =
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| DiffView::new(repo, items, selected, window, cx)));
    if let Err(e) = result {
        eprintln!("rsit: cannot open diff window: {e:#}");
    }
}

impl DiffView {
    pub fn new(repo: Repo, files: Vec<DiffItem>, selected: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut this = Self {
            repo,
            files,
            selected: 0,
            settings: Settings {
                layout: Layout::SideBySide,
                policy: WhitespacePolicy::Default,
                collapse: true,
                words: true,
            },
            diff: None,
            rows: DiffRows { rows: Vec::new(), change_starts: Vec::new() },
            expanded: HashSet::new(),
            current_change: None,
            h_offset: 0.0,
            max_columns: 0,
            error: None,
            scroll: UniformListScrollHandle::new(),
            focus,
            _load: None,
        };
        this.select_file(selected, cx);
        this
    }

    fn select_file(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.files.len() {
            return;
        }
        self.selected = index;
        self.load_selected(cx);
    }

    /// Loads the selected file. With `keep_position` (after staging a change) the
    /// old diff stays visible until the new one is ready and the position is kept.
    fn load_selected(&mut self, cx: &mut Context<Self>) {
        self.load(false, cx)
    }

    fn load(&mut self, keep_position: bool, cx: &mut Context<Self>) {
        let Some(item) = self.files.get(self.selected).cloned() else {
            return;
        };
        if !keep_position {
            self.diff = None;
            self.expanded.clear();
            self.current_change = None;
            self.rows = DiffRows { rows: Vec::new(), change_starts: Vec::new() };
        }
        self.error = None;
        let (repo, policy) = (self.repo.clone(), self.settings.policy);
        let theme = cx.theme().highlight_theme.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            let diff = cx.background_spawn(async move { FileDiff::load(&repo, &item, policy, &theme) }).await;
            this.update(cx, |this, cx| {
                match diff {
                    Ok(diff) => {
                        this.max_columns = max_columns(&diff.left.text).max(max_columns(&diff.right.text));
                        this.diff = Some(Arc::new(diff));
                        if !keep_position {
                            this.h_offset = 0.0;
                        }
                        this.expanded.clear();
                        this.rebuild_rows();
                        let count = this.rows.change_starts.len();
                        match (keep_position, this.current_change) {
                            (_, _) if count == 0 => this.current_change = None,
                            (true, Some(i)) => this.go_to_change(i.min(count - 1)),
                            // open at the first change, like IntelliJ
                            _ => this.go_to_change(0),
                        }
                    }
                    Err(e) => this.error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// The action available for changes of the selected file, if any.
    fn action(&self) -> Option<ChangeAction> {
        self.files.get(self.selected)?.action
    }

    /// Stages or unstages one change (IntelliJ "Stage/Unstage Selected Changes").
    fn apply_action(&mut self, fragment: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(action), Some(diff), Some(item)) = (self.action(), self.diff.clone(), self.files.get(self.selected))
        else {
            return;
        };
        let Some(f) = diff.fragments.get(fragment) else {
            return;
        };
        let patch = rsit_diff::fragment_patch(&item.change.path, &diff.left.text, &diff.right.text, f);
        let cwd = self.repo.cwd().to_path_buf();
        self.current_change = Some(fragment);
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    rsit_git::changes::apply_to_index(&cwd, &patch, action == ChangeAction::Unstage)
                })
                .await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => this.load(true, cx),
                Err(e) => {
                    use gpui_kit::component::WindowExt as _;
                    window.push_notification(
                        gpui_kit::component::notification::Notification::error(format!("{e:#}")).autohide(false),
                        cx,
                    );
                }
            })
            .ok();
        })
        .detach();
    }

    fn apply_action_to_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.current_change {
            self.apply_action(i, window, cx);
        }
    }

    fn rebuild_rows(&mut self) {
        let Some(diff) = &self.diff else { return };
        let folding = Folding { context: FOLD_CONTEXT, expanded: self.expanded.clone() };
        self.rows = rsit_diff::build_rows(
            &diff.fragments,
            diff.left.line_count(),
            diff.right.line_count(),
            self.settings.layout,
            self.settings.collapse.then_some(&folding),
        );
    }

    fn update_settings(&mut self, update: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        let before = self.settings;
        update(&mut self.settings);
        if before.policy != self.settings.policy {
            if let Some(diff) = self.diff.take() {
                // the only other owner is a finished render, so this rarely clones
                let mut diff = Arc::try_unwrap(diff).unwrap_or_else(|arc| clone_diff(&arc));
                diff.recompare(self.settings.policy);
                self.diff = Some(Arc::new(diff));
            }
        }
        self.rebuild_rows();
        self.current_change = None;
        cx.notify();
    }

    /// Current difference (0-based) and the number of differences.
    pub fn change_position(&self) -> (Option<usize>, usize) {
        (self.current_change, self.rows.change_starts.len())
    }

    pub fn horizontal_offset(&self) -> f32 {
        self.h_offset
    }

    fn scroll_horizontally(&mut self, delta: f32, cx: &mut Context<Self>) {
        let max = (self.max_columns as f32 * CHAR_WIDTH - 200.0).max(0.0);
        let offset = (self.h_offset - delta).clamp(0.0, max);
        if offset != self.h_offset {
            self.h_offset = offset;
            cx.notify();
        }
    }

    pub fn row_count(&self) -> usize {
        self.rows.rows.len()
    }

    fn go_to_change(&mut self, index: usize) {
        if let Some(&row) = self.rows.change_starts.get(index) {
            self.current_change = Some(index);
            self.scroll.scroll_to_item(row, ScrollStrategy::Center);
        }
    }

    fn next_change(&mut self, delta: i64, cx: &mut Context<Self>) {
        let count = self.rows.change_starts.len() as i64;
        if count == 0 {
            return;
        }
        let next = match self.current_change {
            None if delta > 0 => 0,
            None => count - 1,
            Some(i) => i as i64 + delta,
        };
        if (0..count).contains(&next) {
            self.go_to_change(next as usize);
        } else if delta > 0 && self.selected + 1 < self.files.len() {
            // past the last change: continue in the next file, as IntelliJ does
            self.select_file(self.selected + 1, cx);
        } else if delta < 0 && self.selected > 0 {
            self.select_file(self.selected - 1, cx);
        }
        cx.notify();
    }

    fn expand_fold(&mut self, first_hidden: u32, cx: &mut Context<Self>) {
        self.expanded.insert(first_hidden);
        self.rebuild_rows();
        cx.notify();
    }

    // ---- rendering ----

    fn render_rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(diff) = self.diff.clone() else {
            return Vec::new();
        };
        let colors = DiffColors::new(cx);
        let words = self.settings.words;
        let current = self.current_change;
        let mono = cx.theme().mono_font_family.clone();
        let offset = self.h_offset;
        let action = self.action();
        range
            .map(|ix| {
                let row = self.rows.rows[ix].clone();
                match row {
                    Row::Fold { left, .. } => {
                        let start = left.start;
                        div()
                            .id(("fold", ix))
                            .test_support()
                            .h(px(ROW_HEIGHT))
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(colors.fold_bg)
                            .text_color(colors.muted)
                            .text_xs()
                            .cursor_pointer()
                            .child(format!("⋯ {} unchanged lines", left.len()))
                            .on_click(cx.listener(move |this, _, _, cx| this.expand_fold(start, cx)))
                            .into_any_element()
                    }
                    Row::Line { left, right, change } => {
                        let is_current = change.is_some_and(|c| Some(c.fragment) == current);
                        let marker = match (action, change) {
                            (Some(action), Some(c)) if self.rows.change_starts.binary_search(&ix).is_ok() => {
                                let fragment = c.fragment;
                                Some(
                                    div()
                                        .id(("change-action", fragment))
                                        .test_support()
                                        .w(px(14.))
                                        .flex_none()
                                        .h_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_color(colors.current)
                                        .cursor_pointer()
                                        .child(if action == ChangeAction::Stage { "+" } else { "−" })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.apply_action(fragment, window, cx)
                                        }))
                                        .into_any_element(),
                                )
                            }
                            (Some(_), _) => Some(div().w(px(14.)).flex_none().into_any_element()),
                            _ => None,
                        };
                        let row_el = div().id(("line", ix)).h(px(ROW_HEIGHT)).w_full().flex().font_family(mono.clone());
                        match self.settings.layout {
                            Layout::SideBySide => row_el
                                .children(marker)
                                .child(gutter(left, change, is_current, &colors))
                                .child(cell(&diff.left, left, change, Side::Left, words, offset, &colors))
                                .child(div().w(px(1.)).h_full().bg(colors.divider))
                                .child(gutter(right, change, false, &colors))
                                .child(cell(&diff.right, right, change, Side::Right, words, offset, &colors))
                                .into_any_element(),
                            Layout::Unified => {
                                let (side_diff, line, side) = match right {
                                    Some(_) if left.is_none() || change.is_none() => (&diff.right, right, Side::Right),
                                    _ => (&diff.left, left, Side::Left),
                                };
                                row_el
                                    .children(marker)
                                    .child(gutter(left, change, is_current, &colors))
                                    .child(gutter(right, change, false, &colors))
                                    .child(cell(side_diff, line, change, side, words, offset, &colors))
                                    .into_any_element()
                            }
                        }
                    }
                }
            })
            .collect()
    }

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let count = self.rows.change_starts.len();
        let position = match (self.current_change, count) {
            (_, 0) => "No differences".to_string(),
            (Some(i), n) => format!("Difference {} of {n}", i + 1),
            (None, n) => format!("{n} difference{}", if n == 1 { "" } else { "s" }),
        };
        let settings = self.settings;
        let policy_label = match settings.policy {
            WhitespacePolicy::Default => "Do not ignore whitespace",
            WhitespacePolicy::TrimWhitespaces => "Trim whitespace",
            WhitespacePolicy::IgnoreWhitespaces => "Ignore whitespace",
        };
        let path = self.files.get(self.selected).map(|item| match &item.change.old_path {
            Some(old) => format!("{old} → {}", item.change.path),
            None => item.change.path.clone(),
        });
        div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(border)
            .child(
                Button::new("prev-change")
                    .small()
                    .ghost()
                    .icon(gpui_kit::assets::IconName::ArrowUp)
                    .tooltip("Previous Difference (Shift+F7)")
                    .on_click(cx.listener(|this, _, _, cx| this.next_change(-1, cx))),
            )
            .child(
                Button::new("next-change")
                    .small()
                    .ghost()
                    .icon(gpui_kit::assets::IconName::ArrowDown)
                    .tooltip("Next Difference (F7)")
                    .on_click(cx.listener(|this, _, _, cx| this.next_change(1, cx))),
            )
            .child(div().w(px(150.)).text_color(muted).child(position))
            .children(self.action().map(|action| {
                let label = match action {
                    ChangeAction::Stage => "Stage Change",
                    ChangeAction::Unstage => "Unstage Change",
                };
                Button::new("change-action")
                    .small()
                    .outline()
                    .label(label)
                    .tooltip("Ctrl+Alt+A")
                    .disabled(self.current_change.is_none())
                    .on_click(cx.listener(|this, _, window, cx| this.apply_action_to_current(window, cx)))
            }))
            .child(
                Button::new("layout")
                    .small()
                    .outline()
                    .label(match settings.layout {
                        Layout::SideBySide => "Side-by-side",
                        Layout::Unified => "Unified",
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.update_settings(
                            |s| {
                                s.layout = match s.layout {
                                    Layout::SideBySide => Layout::Unified,
                                    Layout::Unified => Layout::SideBySide,
                                }
                            },
                            cx,
                        )
                    })),
            )
            .child(Button::new("whitespace").small().outline().label(policy_label).on_click(cx.listener(
                |this, _, _, cx| {
                    this.update_settings(
                        |s| {
                            s.policy = match s.policy {
                                WhitespacePolicy::Default => WhitespacePolicy::TrimWhitespaces,
                                WhitespacePolicy::TrimWhitespaces => WhitespacePolicy::IgnoreWhitespaces,
                                WhitespacePolicy::IgnoreWhitespaces => WhitespacePolicy::Default,
                            }
                        },
                        cx,
                    )
                },
            )))
            .child(
                Button::new("collapse")
                    .small()
                    .ghost()
                    .label(if settings.collapse { "✓ Collapse unchanged" } else { "Collapse unchanged" })
                    .toggled(settings.collapse)
                    .on_click(cx.listener(|this, _, _, cx| this.update_settings(|s| s.collapse = !s.collapse, cx))),
            )
            .child(
                Button::new("words")
                    .small()
                    .ghost()
                    .label(if settings.words { "✓ Highlight words" } else { "Highlight words" })
                    .toggled(settings.words)
                    .on_click(cx.listener(|this, _, _, cx| this.update_settings(|s| s.words = !s.words, cx))),
            )
            .child(div().flex_1())
            .children(path.map(|p| div().text_color(muted).truncate().child(p)))
    }

    fn render_files(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (active, hover, muted) = (theme.list_active, theme.list_hover, theme.muted_foreground);
        let items = self.files.iter().enumerate().map(|(i, item)| {
            let f = &item.change;
            let (letter, color) = crate::log_view::change_style(f.kind);
            let (dir, name) = match f.path.rsplit_once('/') {
                Some((d, n)) => (d.to_string(), n.to_string()),
                None => (String::new(), f.path.clone()),
            };
            div()
                .id(("diff-file", i))
                .h(px(22.))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .when(i == self.selected, |d| d.bg(active))
                .when(i != self.selected, |d| d.hover(|s| s.bg(hover)))
                .child(div().w(px(12.)).text_color(color).child(letter.to_string()))
                .child(div().text_color(color).whitespace_nowrap().child(name))
                .child(div().flex_1().min_w_0().truncate().text_color(muted).child(dir))
                .on_click(cx.listener(move |this, _, window, cx| {
                    window.focus(&this.focus, cx);
                    this.select_file(i, cx);
                }))
        });
        div().id("diff-files").size_full().overflow_y_scroll().children(items)
    }

    fn render_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let message = |text: SharedString| {
            div().size_full().flex().items_center().justify_center().text_color(muted).child(text).into_any_element()
        };
        if let Some(error) = &self.error {
            return message(error.clone());
        }
        let Some(diff) = &self.diff else {
            return message("Loading…".into());
        };
        let labels = self
            .files
            .get(self.selected)
            .map(|item| (item.left_label.clone(), item.right_label.clone()))
            .unwrap_or_default();
        if diff.binary {
            return message("Binary file has changed".into());
        }
        if diff.identical() {
            return message(match self.settings.policy {
                WhitespacePolicy::Default => "Contents are identical".into(),
                _ => "Contents are identical up to whitespace".into(),
            });
        }
        let header = |label: String| div().flex_1().px_2().truncate().text_color(muted).child(label);
        let theme = cx.theme();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .h(px(24.))
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(header(format!("{} {}", labels.0, if diff.left.exists { "" } else { "(file absent)" })))
                    .child(header(format!("{} {}", labels.1, if diff.right.exists { "" } else { "(file deleted)" }))),
            )
            .child(
                div()
                    .id("diff-scroll")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    // horizontal wheel/touchpad (shift+wheel arrives as horizontal)
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                        let delta = event.delta.pixel_delta(px(ROW_HEIGHT));
                        if f32::from(delta.x) != 0.0 {
                            this.scroll_horizontally(f32::from(delta.x), cx);
                        }
                    }))
                    .child(
                        uniform_list("diff-rows", self.rows.rows.len(), cx.processor(Self::render_rows))
                            .track_scroll(&self.scroll)
                            .size_full()
                            .text_size(px(13.)),
                    ),
            )
            .into_any_element()
    }
}

impl Render for DiffView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, fg) = (theme.background, theme.foreground);
        div()
            .id("diff-view")
            .test_support()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &NextChange, _, cx| this.next_change(1, cx)))
            .on_action(cx.listener(|this, _: &PrevChange, _, cx| this.next_change(-1, cx)))
            .on_action(cx.listener(|this, _: &NextFile, _, cx| {
                if this.selected + 1 < this.files.len() {
                    this.select_file(this.selected + 1, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &PrevFile, _, cx| {
                if this.selected > 0 {
                    this.select_file(this.selected - 1, cx)
                }
            }))
            .on_action(|_: &CloseDiff, window, _| window.remove_window())
            .on_action(cx.listener(|this, _: &ApplyChangeAction, window, cx| this.apply_action_to_current(window, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .child(self.render_toolbar(cx))
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("diff-main")
                        .child(resizable_panel().size(px(260.)).child(self.render_files(cx)))
                        .child(resizable_panel().child(self.render_body(cx))),
                ),
            )
    }
}

#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
}

struct DiffColors {
    inserted: Hsla,
    inserted_word: Hsla,
    deleted: Hsla,
    deleted_word: Hsla,
    modified: Hsla,
    modified_word: Hsla,
    empty: Hsla,
    fold_bg: Hsla,
    divider: Hsla,
    muted: Hsla,
    current: Hsla,
}

impl DiffColors {
    /// IntelliJ's diff palette (light and Darcula).
    fn new(cx: &App) -> Self {
        let theme = cx.theme();
        let c = |hex: u32| -> Hsla { rgb(hex).into() };
        if theme.is_dark() {
            Self {
                inserted: c(0x294436),
                inserted_word: c(0x3d6b4c),
                deleted: c(0x484a4a),
                deleted_word: c(0x656869),
                modified: c(0x385570),
                modified_word: c(0x4a6f93),
                empty: c(0x2b2d30),
                fold_bg: theme.muted,
                divider: theme.border,
                muted: theme.muted_foreground,
                current: c(0x7a9ec2),
            }
        } else {
            Self {
                inserted: c(0xe9f5e6),
                inserted_word: c(0xc6e6bd),
                deleted: c(0xeeeeee),
                deleted_word: c(0xd6d6d6),
                modified: c(0xe6eefa),
                modified_word: c(0xc2d6f2),
                empty: c(0xf7f8fa),
                fold_bg: theme.muted,
                divider: theme.border,
                muted: theme.muted_foreground,
                current: c(0x3574f0),
            }
        }
    }

    fn line_bg(&self, kind: ChangeKind) -> Hsla {
        match kind {
            ChangeKind::Inserted => self.inserted,
            ChangeKind::Deleted => self.deleted,
            ChangeKind::Modified => self.modified,
        }
    }

    fn word_bg(&self, kind: ChangeKind, side: Side) -> Hsla {
        match (kind, side) {
            (ChangeKind::Modified, _) => self.modified_word,
            (ChangeKind::Inserted, _) | (_, Side::Right) => self.inserted_word,
            (ChangeKind::Deleted, Side::Left) => self.deleted_word,
        }
    }
}

fn gutter(line: Option<u32>, change: Option<Change>, current: bool, colors: &DiffColors) -> impl IntoElement {
    div()
        .w(px(GUTTER))
        .flex_none()
        .h_full()
        .px_1()
        .flex()
        .justify_end()
        .items_center()
        .text_color(colors.muted)
        .text_xs()
        .when_some(change, |d, c| d.bg(colors.line_bg(c.kind)))
        .when(current, |d| d.border_l_2().border_color(colors.current))
        .children(line.map(|l| (l + 1).to_string()))
}

fn cell(
    side_diff: &DiffSide,
    line: Option<u32>,
    change: Option<Change>,
    side: Side,
    words: bool,
    h_offset: f32,
    colors: &DiffColors,
) -> impl IntoElement {
    let base = div().flex_1().min_w_0().h_full().px_1().overflow_hidden().whitespace_nowrap().flex().items_center();
    let Some(line) = line else {
        // aligned filler for lines that exist only on the other side
        return base.bg(if change.is_some() { colors.empty } else { gpui_kit::transparent_black() });
    };
    let range = side_diff.line_range(line);
    let text = &side_diff.text[range.clone()];
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = clip(&side_diff.syntax, &range).collect();
    let base = match change {
        Some(c) => {
            // an insertion/deletion shown on its own side reads as that kind
            let kind = match (c.kind, side) {
                (ChangeKind::Modified, _) => ChangeKind::Modified,
                (_, Side::Left) => ChangeKind::Deleted,
                (_, Side::Right) => ChangeKind::Inserted,
            };
            if words && c.kind == ChangeKind::Modified {
                let bg = colors.word_bg(c.kind, side);
                let word_ranges: Vec<(Range<usize>, HighlightStyle)> = side_diff
                    .words
                    .iter()
                    .filter(|w| w.start < range.end && w.end > range.start)
                    .map(|w| {
                        let r = w.start.max(range.start) - range.start..w.end.min(range.end) - range.start;
                        (r, HighlightStyle { background_color: Some(bg), ..Default::default() })
                    })
                    .collect();
                highlights = gpui_kit::combine_highlights(highlights, word_ranges).collect();
            }
            base.bg(colors.line_bg(kind))
        }
        None => base,
    };
    let (display, highlights) = expand_tabs(text, highlights);
    base.child(div().flex_none().ml(px(-h_offset)).child(StyledText::new(display).with_highlights(highlights)))
}

/// Widest line in display columns (tabs expanded to the next stop).
fn max_columns(text: &str) -> usize {
    text.lines()
        .map(|line| line.chars().fold(0, |col, c| if c == '\t' { col + TAB_WIDTH - col % TAB_WIDTH } else { col + 1 }))
        .max()
        .unwrap_or(0)
}

/// Highlights overlapping `range`, shifted to be relative to its start.
fn clip<'a>(
    styles: &'a [(Range<usize>, HighlightStyle)],
    range: &'a Range<usize>,
) -> impl Iterator<Item = (Range<usize>, HighlightStyle)> + 'a {
    let first = styles.partition_point(|(r, _)| r.end <= range.start);
    styles[first..]
        .iter()
        .take_while(|(r, _)| r.start < range.end)
        .filter(|(r, _)| r.end > range.start)
        .map(|(r, s)| (r.start.max(range.start) - range.start..r.end.min(range.end) - range.start, *s))
}

/// Replaces tabs with spaces up to the next tab stop, remapping highlight ranges.
fn expand_tabs(
    text: &str,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
    if !text.contains('\t') {
        return (text.to_string(), highlights);
    }
    let mut out = String::with_capacity(text.len() + 16);
    let mut map = vec![0usize; text.len() + 1];
    let mut column = 0;
    for (i, ch) in text.char_indices() {
        for b in 0..ch.len_utf8() {
            map[i + b] = out.len();
        }
        if ch == '\t' {
            let n = TAB_WIDTH - column % TAB_WIDTH;
            out.extend(std::iter::repeat_n(' ', n));
            column += n;
        } else {
            out.push(ch);
            column += 1;
        }
    }
    map[text.len()] = out.len();
    let highlights = highlights.into_iter().map(|(r, s)| (map[r.start]..map[r.end], s)).collect();
    (out, highlights)
}

fn clone_diff(diff: &FileDiff) -> FileDiff {
    let side = |s: &DiffSide| DiffSide {
        text: s.text.clone(),
        line_starts: s.line_starts.clone(),
        syntax: s.syntax.clone(),
        words: s.words.clone(),
        exists: s.exists,
    };
    FileDiff {
        left: side(&diff.left),
        right: side(&diff.right),
        fragments: diff.fragments.clone(),
        binary: diff.binary,
        language: diff.language,
    }
}
