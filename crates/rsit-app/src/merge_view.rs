//! Three-way merge window (IntelliJ `TextMergeViewer`): Yours | Result | Theirs,
//! aligned row by row, with per-chunk apply/ignore buttons, "Apply
//! Non-Conflicting Changes", "Resolve Simple Conflicts" and manual editing.

use std::ops::Range;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rsit_diff::merge::{ChunkKind, MergeModel};
use rsit_diff::{Lines, WhitespacePolicy};
use rsit_git::Repo;
use rsit_git::conflicts::{self, ConflictVersions};

use crate::diff_model::{DiffSide, language_for};
use crate::selection::{SelectableText, SelectionState};
use crate::text::clip;

const CONTEXT: &str = "MergeView";
const ROW_HEIGHT: f32 = 20.0;
const GUTTER: f32 = 44.0;
const CONTROLS: f32 = 44.0;

actions!(merge, [NextConflict, PrevConflict, CopyMergeText, SelectAllMergeText]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("f7", NextConflict, Some(CONTEXT)),
        KeyBinding::new("shift-f7", PrevConflict, Some(CONTEXT)),
        KeyBinding::new("ctrl-c", CopyMergeText, Some(CONTEXT)),
        KeyBinding::new("ctrl-a", SelectAllMergeText, Some(CONTEXT)),
    ]);
}

#[derive(Clone, Copy, Debug)]
struct MergeRow {
    left: Option<u32>,
    result: Option<u32>,
    right: Option<u32>,
    chunk: Option<usize>,
    /// First row of its chunk: shows the chunk's buttons.
    chunk_start: bool,
}

pub struct MergeView {
    repo: Repo,
    path: String,
    labels: (String, String),
    versions: Option<ConflictVersions>,
    model: Option<MergeModel>,
    left: Option<DiffSide>,
    right: Option<DiffSide>,
    result: Option<DiffSide>,
    rows: Vec<MergeRow>,
    /// First row of every chunk.
    chunk_rows: Vec<usize>,
    current: Option<usize>,
    /// Manual editing of the whole result replaces the chunk model.
    manual: Option<Entity<EditorState>>,
    error: Option<SharedString>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    selection: SelectionState,
    _load: Option<Task<()>>,
}

/// Opens the merge window for a conflicted `path`.
pub fn open(repo: Repo, path: String, cx: &mut App) {
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some(format!("Merge {path}").into()), ..Default::default() }),
        window_bounds: Some(WindowBounds::centered(size(px(1600.), px(950.)), cx)),
        app_id: Some("rsit".into()),
        ..Default::default()
    };
    if let Err(e) = gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| MergeView::new(repo, path, window, cx)))
    {
        eprintln!("rsit: cannot open merge window: {e:#}");
    }
}

impl MergeView {
    pub fn new(repo: Repo, path: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut this = Self {
            repo,
            path,
            labels: ("Yours".into(), "Theirs".into()),
            versions: None,
            model: None,
            left: None,
            right: None,
            result: None,
            rows: Vec::new(),
            chunk_rows: Vec::new(),
            current: None,
            manual: None,
            error: None,
            scroll: UniformListScrollHandle::new(),
            focus,
            selection: SelectionState::default(),
            _load: None,
        };
        this.load(cx);
        this
    }

    pub fn model(&self) -> Option<&MergeModel> {
        self.model.as_ref()
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let (repo, path) = (self.repo.clone(), self.path.clone());
        let theme = cx.theme().highlight_theme.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let versions = conflicts::conflict_versions(&repo, &path)?;
                    let labels = conflicts::side_labels(&repo);
                    let model = versions.mergeable().then(|| {
                        let text = |b: &Option<Vec<u8>>| {
                            String::from_utf8_lossy(b.as_deref().unwrap_or_default()).into_owned()
                        };
                        MergeModel::new(
                            text(&versions.base),
                            text(&versions.ours),
                            text(&versions.theirs),
                            WhitespacePolicy::Default,
                        )
                    });
                    let lang = language_for(&path);
                    let sides = model.as_ref().map(|m| {
                        (
                            DiffSide::new(Some(m.left.clone().into_bytes()), lang, &theme),
                            DiffSide::new(Some(m.right.clone().into_bytes()), lang, &theme),
                        )
                    });
                    anyhow::Ok((versions, labels, model, sides))
                })
                .await;
            this.update(cx, |this, cx| {
                match loaded {
                    Ok((versions, labels, model, sides)) => {
                        this.versions = Some(versions);
                        this.labels = labels;
                        this.model = model;
                        if let Some((l, r)) = sides {
                            this.left = Some(l);
                            this.right = Some(r);
                        }
                        this.rebuild(cx);
                        this.next_conflict(1, cx);
                    }
                    Err(e) => this.error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Recomputes the result text and the aligned rows after any change.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let Some(model) = &self.model else { return };
        // result lines move when a chunk changes
        if self.selection.pane() == Some(1) {
            self.selection.clear();
        }
        let result = model.result();
        let theme = cx.theme().highlight_theme.clone();
        self.result = Some(DiffSide::new(Some(result.into_bytes()), language_for(&self.path), &theme));
        let base_len = Lines::new(&model.base).len() as u32;
        let (mut rows, mut chunk_rows) = (Vec::new(), Vec::new());
        let (mut base_pos, mut left_pos, mut right_pos, mut result_pos) = (0u32, 0u32, 0u32, 0u32);
        let unchanged = |rows: &mut Vec<MergeRow>, n: u32, l: &mut u32, r: &mut u32, res: &mut u32| {
            for _ in 0..n {
                rows.push(MergeRow {
                    left: Some(*l),
                    result: Some(*res),
                    right: Some(*r),
                    chunk: None,
                    chunk_start: false,
                });
                *l += 1;
                *r += 1;
                *res += 1;
            }
        };
        for (i, c) in model.chunks.iter().enumerate() {
            unchanged(&mut rows, c.base.start - base_pos, &mut left_pos, &mut right_pos, &mut result_pos);
            let result_lines = Lines::new(&model.result_chunk(i)).len() as u32;
            let height = (c.left.len() as u32).max(c.right.len() as u32).max(result_lines).max(1);
            chunk_rows.push(rows.len());
            for k in 0..height {
                rows.push(MergeRow {
                    left: (k < c.left.len() as u32).then_some(c.left.start + k),
                    result: (k < result_lines).then_some(result_pos + k),
                    right: (k < c.right.len() as u32).then_some(c.right.start + k),
                    chunk: Some(i),
                    chunk_start: k == 0,
                });
            }
            base_pos = c.base.end;
            left_pos = c.left.end;
            right_pos = c.right.end;
            result_pos += result_lines;
        }
        unchanged(&mut rows, base_len - base_pos, &mut left_pos, &mut right_pos, &mut result_pos);
        self.rows = rows;
        self.chunk_rows = chunk_rows;
        cx.notify();
    }

    fn update_model(&mut self, f: impl FnOnce(&mut MergeModel), cx: &mut Context<Self>) {
        if let Some(model) = &mut self.model {
            f(model);
            self.rebuild(cx);
        }
    }

    pub fn apply_side(&mut self, chunk: usize, left: bool, cx: &mut Context<Self>) {
        self.update_model(
            |m| {
                let r = &mut m.resolutions[chunk];
                r.merged = None;
                r.ignored = false;
                if left { r.left_applied = true } else { r.right_applied = true }
            },
            cx,
        );
    }

    pub fn ignore(&mut self, chunk: usize, cx: &mut Context<Self>) {
        self.update_model(|m| m.resolutions[chunk].ignored = true, cx);
    }

    pub fn apply_non_conflicting(&mut self, cx: &mut Context<Self>) {
        self.update_model(MergeModel::apply_non_conflicting, cx);
    }

    pub fn resolve_simple(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut count = 0;
        self.update_model(|m| count = m.resolve_simple_conflicts(), cx);
        let message = match count {
            0 => "No conflicts could be resolved automatically".to_string(),
            1 => "Resolved 1 conflict".to_string(),
            n => format!("Resolved {n} conflicts"),
        };
        window.push_notification(message, cx);
    }

    /// Moves to the next unresolved chunk (conflicts first, like IntelliJ's F7).
    fn next_conflict(&mut self, delta: i64, cx: &mut Context<Self>) {
        let Some(model) = &self.model else { return };
        let unresolved: Vec<usize> = (0..model.chunks.len()).filter(|&i| !model.resolutions[i].is_resolved()).collect();
        if unresolved.is_empty() {
            return;
        }
        let next = match self.current {
            None => unresolved[0],
            Some(c) if delta > 0 => unresolved.iter().copied().find(|&i| i > c).unwrap_or(unresolved[0]),
            Some(c) => unresolved.iter().rev().copied().find(|&i| i < c).unwrap_or(*unresolved.last().unwrap()),
        };
        self.current = Some(next);
        if let Some(&row) = self.chunk_rows.get(next) {
            self.scroll.scroll_to_item(row, ScrollStrategy::Center);
        }
        cx.notify();
    }

    fn edit_manually(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = &self.model else { return };
        let text = model.result();
        let lang = language_for(&self.path);
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx).language(lang);
            state.set_value(text, window, cx);
            state
        });
        self.manual = Some(editor);
        cx.notify();
    }

    /// The text to save: the manual edit or the chunk model's result.
    pub fn result_text(&self, cx: &App) -> Option<String> {
        match &self.manual {
            Some(editor) => Some(editor.read(cx).value().to_string()),
            None => self.model.as_ref().map(MergeModel::result),
        }
    }

    /// Saves the result, stages the file and closes the window.
    pub fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let unresolved = if self.manual.is_some() { 0 } else { self.model.as_ref().map_or(0, MergeModel::unresolved) };
        let Some(text) = self.result_text(cx) else { return };
        let (repo, path) = (self.repo.clone(), self.path.clone());
        let save = move |window: &mut Window, cx: &mut App| match conflicts::save_resolved(&repo, &path, &text) {
            Ok(()) => {
                window.push_notification(format!("Resolved {path}"), cx);
                window.remove_window();
            }
            Err(e) => window.push_notification(
                gpui_kit::component::notification::Notification::error(format!("{e:#}")).autohide(false),
                cx,
            ),
        };
        if unresolved > 0 {
            let detail =
                format!("{unresolved} change(s) are not resolved; their base text will be saved. Apply anyway?");
            crate::git_actions::confirm("Unresolved Changes", &detail, window, cx, save);
        } else {
            save(window, cx);
        }
    }

    /// Takes one side for the whole file (IntelliJ "Accept Left/Right").
    pub fn accept(&mut self, ours: bool, window: &mut Window, cx: &mut Context<Self>) {
        match conflicts::accept_side(&self.repo, &self.path, ours) {
            Ok(()) => {
                window.push_notification(format!("Resolved {}", self.path), cx);
                window.remove_window();
            }
            Err(e) => window.push_notification(
                gpui_kit::component::notification::Notification::error(format!("{e:#}")).autohide(false),
                cx,
            ),
        }
    }

    /// A selectable line of pane `pane` (0 yours, 1 result, 2 theirs).
    fn text_cell(
        &self,
        pane: usize,
        side: &DiffSide,
        line: Option<u32>,
        bg: Option<Hsla>,
        cx: &mut Context<Self>,
    ) -> Div {
        let cell = div().flex_1().min_w_0().h_full().px_1().overflow_hidden().whitespace_nowrap().flex().items_center();
        let cell = match bg {
            Some(bg) => cell.bg(bg),
            None => cell,
        };
        let Some(line) = line else { return cell };
        let range = side.line_range(line);
        let text = &side.text[range.clone()];
        let selection_bg = cx.theme().selection;
        let selected = self.selection.selection.and_then(|s| s.line_range(pane, line, text.len()));
        let rendered =
            crate::selection::render_line(text, clip(&side.syntax, &range).collect(), selected, selection_bg);
        crate::selection::attach(cell, pane, line, &rendered, text.to_string().into(), cx)
            .child(crate::selection::line_content(rendered, selection_bg, 0.))
    }

    fn render_rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let (Some(model), Some(left), Some(right), Some(result)) = (&self.model, &self.left, &self.right, &self.result)
        else {
            return Vec::new();
        };
        let theme = cx.theme();
        let dark = theme.is_dark();
        let (muted, border, mono) = (theme.muted_foreground, theme.border, theme.mono_font_family.clone());
        let color = |hex_light: u32, hex_dark: u32| -> Hsla { rgb(if dark { hex_dark } else { hex_light }).into() };
        let (changed, inserted, conflict, resolved_bg) = (
            color(0xe6eefa, 0x385570),
            color(0xe9f5e6, 0x294436),
            color(0xffe0e0, 0x5a3434),
            color(0xf5f5f5, 0x2f3133),
        );
        let accent = color(0x3574f0, 0x7a9ec2);
        let current = self.current;
        let mut out = Vec::with_capacity(range.len());
        for ix in range {
            let row = self.rows[ix];
            let chunk = row.chunk.map(|c| (c, &model.chunks[c], &model.resolutions[c]));
            let side_bg = |is_left: bool| -> Option<Hsla> {
                let (_, c, r) = chunk?;
                if r.is_resolved() {
                    return Some(resolved_bg);
                }
                match c.kind {
                    ChunkKind::Conflict => Some(conflict),
                    ChunkKind::Left if is_left => Some(if c.base.is_empty() { inserted } else { changed }),
                    ChunkKind::Right if !is_left => Some(if c.base.is_empty() { inserted } else { changed }),
                    ChunkKind::Both => Some(changed),
                    _ => None,
                }
            };
            let result_bg = chunk.map(|(_, c, r)| {
                if r.is_resolved() {
                    inserted.opacity(0.6)
                } else if c.kind == ChunkKind::Conflict {
                    conflict
                } else {
                    changed
                }
            });
            let is_current = chunk.is_some_and(|(i, _, _)| Some(i) == current);
            // per-chunk buttons: >> x on the left, x << on the right
            let controls = |is_left: bool, cx: &mut Context<Self>| -> AnyElement {
                let base = div().w(px(CONTROLS)).flex_none().h_full().flex().items_center().justify_center().gap_1();
                let Some((i, c, r)) = chunk.filter(|_| row.chunk_start) else { return base.into_any_element() };
                let side_changed = match c.kind {
                    ChunkKind::Left => is_left,
                    ChunkKind::Right => !is_left,
                    ChunkKind::Both | ChunkKind::Conflict => true,
                };
                let applied = if is_left { r.left_applied } else { r.right_applied };
                if !side_changed || applied || r.merged.is_some() || r.ignored {
                    return base.into_any_element();
                }
                let arrow = div()
                    .id((if is_left { "apply-left" } else { "apply-right" }, i))
                    .test_support()
                    .cursor_pointer()
                    .text_color(accent)
                    .child(if is_left { "»" } else { "«" })
                    .on_click(cx.listener(move |this, _, _, cx| this.apply_side(i, is_left, cx)));
                let cross = div()
                    .id((if is_left { "ignore-left" } else { "ignore-right" }, i))
                    .cursor_pointer()
                    .text_color(muted)
                    .child("×")
                    .on_click(cx.listener(move |this, _, _, cx| this.ignore(i, cx)));
                if is_left { base.child(arrow).child(cross) } else { base.child(cross).child(arrow) }.into_any_element()
            };
            let number = |line: Option<u32>, bg: Option<Hsla>, mark: bool| {
                div()
                    .w(px(GUTTER))
                    .flex_none()
                    .h_full()
                    .px_1()
                    .flex()
                    .justify_end()
                    .items_center()
                    .text_xs()
                    .text_color(muted)
                    .when_some(bg, |d, bg| d.bg(bg))
                    .when(mark, |d| d.border_l_2().border_color(accent))
                    .children(line.map(|l| (l + 1).to_string()))
            };
            let left_controls = controls(true, cx);
            let right_controls = controls(false, cx);
            out.push(
                div()
                    .id(("merge-row", ix))
                    .h(px(ROW_HEIGHT))
                    .w_full()
                    .flex()
                    .font_family(mono.clone())
                    .child(number(row.left, side_bg(true), is_current))
                    .child(self.text_cell(0, left, row.left, side_bg(true), cx))
                    .child(left_controls)
                    .child(div().w(px(1.)).h_full().bg(border))
                    .child(number(row.result, result_bg, false))
                    .child(self.text_cell(1, result, row.result, result_bg, cx))
                    .child(div().w(px(1.)).h_full().bg(border))
                    .child(right_controls)
                    .child(number(row.right, side_bg(false), false))
                    .child(self.text_cell(2, right, row.right, side_bg(false), cx))
                    .into_any_element(),
            );
        }
        out
    }
}

impl Render for MergeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, fg, border, muted) = (theme.background, theme.foreground, theme.border, theme.muted_foreground);
        let (left_label, right_label) = self.labels.clone();
        let status = match (&self.model, &self.manual) {
            (_, Some(_)) => "Editing the result manually".to_string(),
            (Some(m), None) => match (m.unresolved(), m.unresolved_conflicts()) {
                (0, _) => "All changes have been processed".to_string(),
                (n, 0) => format!("{n} change(s) left"),
                (n, c) => format!("{n} change(s) left, {c} conflict(s)"),
            },
            (None, None) => String::new(),
        };
        let mergeable = self.model.is_some();
        let manual = self.manual.is_some();
        let toolbar = div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(border)
            .child(
                Button::new("prev-conflict")
                    .small()
                    .ghost()
                    .icon(gpui_kit::assets::IconName::ArrowUp)
                    .tooltip("Previous (Shift+F7)")
                    .disabled(!mergeable || manual)
                    .on_click(cx.listener(|t, _, _, cx| t.next_conflict(-1, cx))),
            )
            .child(
                Button::new("next-conflict")
                    .small()
                    .ghost()
                    .icon(gpui_kit::assets::IconName::ArrowDown)
                    .tooltip("Next (F7)")
                    .disabled(!mergeable || manual)
                    .on_click(cx.listener(|t, _, _, cx| t.next_conflict(1, cx))),
            )
            .child(
                Button::new("apply-non-conflicting")
                    .small()
                    .outline()
                    .label("Apply Non-Conflicting Changes")
                    .disabled(!mergeable || manual)
                    .on_click(cx.listener(|t, _, _, cx| t.apply_non_conflicting(cx))),
            )
            .child(
                Button::new("resolve-simple")
                    .small()
                    .ghost()
                    .label("Resolve Simple Conflicts")
                    .disabled(!mergeable || manual)
                    .on_click(cx.listener(|t, _, w, cx| t.resolve_simple(w, cx))),
            )
            .child(
                Button::new("edit-result")
                    .small()
                    .ghost()
                    .label("Edit Result…")
                    .disabled(!mergeable || manual)
                    .on_click(cx.listener(|t, _, w, cx| t.edit_manually(w, cx))),
            )
            .child(div().flex_1())
            .child(div().text_color(muted).child(status));
        let headers = div()
            .flex()
            .h(px(24.))
            .items_center()
            .border_b_1()
            .border_color(border)
            .text_color(muted)
            .child(div().flex_1().px_2().truncate().child(left_label.clone()))
            .child(div().flex_1().px_2().truncate().child("Result"))
            .child(div().flex_1().px_2().truncate().child(right_label.clone()));
        let body: AnyElement = if let Some(error) = &self.error {
            div().p_4().text_color(muted).child(error.clone()).into_any_element()
        } else if let Some(editor) = &self.manual {
            div().flex_1().min_h_0().child(Editor::new(editor).size_full()).into_any_element()
        } else if let Some(versions) = &self.versions.as_ref().filter(|_| !mergeable) {
            let what = if versions.is_binary() {
                "Binary file: choose one version".to_string()
            } else {
                let side = |v: &Option<Vec<u8>>| if v.is_some() { "modified" } else { "deleted" };
                format!("{left_label}: {}, {right_label}: {}", side(&versions.ours), side(&versions.theirs))
            };
            div().p_4().text_color(muted).child(what).into_any_element()
        } else if self.model.is_none() {
            div().p_4().text_color(muted).child("Loading…").into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(headers)
                .child(
                    uniform_list("merge-rows", self.rows.len(), cx.processor(Self::render_rows))
                        .track_scroll(&self.scroll)
                        .flex_1()
                        .w_full()
                        .text_size(px(13.)),
                )
                .into_any_element()
        };
        crate::selection::release(div().id("merge-view"), cx)
            .test_support()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &NextConflict, _, cx| this.next_conflict(1, cx)))
            .on_action(cx.listener(|this, _: &PrevConflict, _, cx| this.next_conflict(-1, cx)))
            .on_action(cx.listener(|this, _: &CopyMergeText, _, cx| this.copy_selection(cx)))
            .on_action(cx.listener(|this, _: &SelectAllMergeText, _, cx| this.select_all_text(1, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .child(toolbar)
            .child(body)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_2()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        Button::new("accept-left")
                            .small()
                            .outline()
                            .label(format!("Accept {}", short(&left_label)))
                            .on_click(cx.listener(|t, _, w, cx| t.accept(true, w, cx))),
                    )
                    .child(
                        Button::new("accept-right")
                            .small()
                            .outline()
                            .label(format!("Accept {}", short(&right_label)))
                            .on_click(cx.listener(|t, _, w, cx| t.accept(false, w, cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("cancel-merge")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, _| window.remove_window()),
                    )
                    .child(
                        Button::new("apply-merge")
                            .small()
                            .primary()
                            .label("Apply")
                            .disabled(!mergeable)
                            .on_click(cx.listener(|t, _, w, cx| t.apply(w, cx))),
                    ),
            )
    }
}

/// "Yours (main)" -> "Yours".
fn short(label: &str) -> &str {
    label.split(" (").next().unwrap_or(label)
}

impl SelectableText for MergeView {
    fn selection_state(&mut self) -> &mut SelectionState {
        &mut self.selection
    }

    fn line_text(&self, pane: usize, line: u32) -> Option<&str> {
        let side = [&self.left, &self.result, &self.right][pane.min(2)].as_ref()?;
        (line < side.line_count()).then(|| &side.text[side.line_range(line)])
    }

    fn line_count(&self, pane: usize) -> u32 {
        [&self.left, &self.result, &self.right][pane.min(2)].as_ref().map_or(0, |s| s.line_count())
    }

    fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }
}
