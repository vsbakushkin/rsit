//! Annotate (IntelliJ "Annotate with Git Blame"): the file with author and
//! date per line, colored by age; hovering a line highlights its commit.

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rsit_git::history::{Blame, BlameOptions};
use rsit_git::{ObjectId, Repo};

use crate::diff_model::{DiffSide, language_for};
use crate::file_view::{self, FileTab};
use crate::selection::{SelectableText, SelectionState};
use crate::settings::{editor_scale, scaled};
use crate::text::{clip, max_columns};

const ROW_HEIGHT: f32 = 20.0;
const CONTEXT: &str = "BlameView";

actions!(blame, [CopyBlameText, SelectAllBlameText]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-c", CopyBlameText, Some(CONTEXT)),
        KeyBinding::new("ctrl-a", SelectAllBlameText, Some(CONTEXT)),
    ]);
}
const ANNOTATION_WIDTH: f32 = 230.0;
const CHAR_WIDTH: f32 = 8.0;

struct BlameData {
    blame: Blame,
    side: DiffSide,
    /// Per commit: 0 = oldest, 1 = newest (IntelliJ colors newer lines stronger).
    age: Vec<f32>,
    max_columns: usize,
}

pub struct BlameView {
    repo: Repo,
    path: String,
    rev: Option<ObjectId>,
    options: BlameOptions,
    data: Option<Arc<BlameData>>,
    hovered: Option<usize>,
    selected: Option<usize>,
    error: Option<SharedString>,
    h_offset: f32,
    scroll: UniformListScrollHandle,
    selection: SelectionState,
    focus: FocusHandle,
    _load: Option<Task<()>>,
    _theme: Subscription,
}

impl BlameView {
    pub fn new(repo: Repo, path: String, rev: Option<ObjectId>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            repo,
            path,
            rev,
            options: BlameOptions::default(),
            data: None,
            hovered: None,
            selected: None,
            error: None,
            h_offset: 0.0,
            scroll: UniformListScrollHandle::new(),
            selection: SelectionState::default(),
            focus: cx.focus_handle(),
            _load: None,
            _theme: crate::settings::observe_highlight_theme(cx, Self::reload),
        };
        this.reload(cx);
        this
    }

    /// Lines of the annotated text (0 while loading).
    pub fn line_count(&self) -> usize {
        self.data.as_ref().map_or(0, |d| d.blame.lines.len())
    }

    /// Summary of the commit that last changed line `line` (0-based).
    pub fn commit_of_line(&self, line: usize) -> Option<String> {
        let data = self.data.as_ref()?;
        Some(data.blame.commits[data.blame.lines.get(line)?.commit].summary.clone())
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let (repo, path, rev, options) = (self.repo.clone(), self.path.clone(), self.rev, self.options);
        let theme = cx.theme().highlight_theme.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            let data = cx
                .background_spawn(async move {
                    let rev = rev.map(|r| r.to_string());
                    let blame = rsit_git::history::blame(repo.cwd(), &path, rev.as_deref(), options)?;
                    let side = DiffSide::new(Some(blame.text.clone().into_bytes()), language_for(&path), &theme);
                    let mut times: Vec<i64> = blame.commits.iter().map(|c| c.time).collect();
                    times.sort_unstable();
                    times.dedup();
                    let age = blame
                        .commits
                        .iter()
                        .map(|c| {
                            if c.uncommitted || times.len() < 2 {
                                return 1.0;
                            }
                            times.binary_search(&c.time).unwrap_or(0) as f32 / (times.len() - 1) as f32
                        })
                        .collect();
                    let max_columns = max_columns(&blame.text);
                    anyhow::Ok(BlameData { blame, side, age, max_columns })
                })
                .await;
            this.update(cx, |this, cx| {
                match data {
                    Ok(data) => this.data = Some(Arc::new(data)),
                    Err(e) => this.error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn toggle(&mut self, f: impl FnOnce(&mut BlameOptions), cx: &mut Context<Self>) {
        f(&mut self.options);
        self.reload(cx);
    }

    fn scroll_horizontally(&mut self, delta: f32, cx: &mut Context<Self>) {
        let max = self.data.as_ref().map_or(0.0, |d| (d.max_columns as f32 * CHAR_WIDTH - 200.0).max(0.0));
        let offset = (self.h_offset - delta).clamp(0.0, max);
        if offset != self.h_offset {
            self.h_offset = offset;
            cx.notify();
        }
    }

    /// Commit id and path of commit `index`, if it is a real commit.
    fn commit_at(&self, index: usize) -> Option<(ObjectId, String)> {
        let c = self.data.as_ref()?.blame.commits.get(index)?;
        (!c.uncommitted).then(|| (c.id, c.filename.clone()))
    }

    fn show_diff(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((id, path)) = self.commit_at(index) else { return };
        let files = rsit_git::changed_files(&self.repo.local(), id).unwrap_or_default();
        let selected = files.iter().position(|f| f.path == path).unwrap_or(0);
        if !files.is_empty() {
            crate::diff_view::open(self.repo.clone(), id, files, selected, cx);
        }
    }

    /// Annotates the state before commit `index` changed these lines.
    fn annotate_previous(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(data) = &self.data else { return };
        let commit = &data.blame.commits[index];
        let previous = if commit.uncommitted {
            rsit_git::read_refs(&self.repo.local()).ok().and_then(|r| r.head).map(|h| (h, self.path.clone()))
        } else {
            commit.previous.clone()
        };
        if let Some((rev, path)) = previous {
            file_view::open(self.repo.clone(), path, Some(rev), FileTab::Annotate, cx);
        }
    }

    fn render_rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(data) = self.data.clone() else { return Vec::new() };
        let theme = cx.theme();
        let (muted, bg, mono) = (theme.muted_foreground, theme.background, theme.mono_font_family.clone());
        let s = editor_scale(cx);
        let age_color: Hsla = if theme.is_dark() { rgb(0x4a7a52).into() } else { rgb(0x8fce9a).into() };
        let uncommitted_color: Hsla = if theme.is_dark() { rgb(0x5a5a2a).into() } else { rgb(0xf2e6a0).into() };
        let hover_color = theme.list_active;
        let selection_bg = theme.selection;
        let h_offset = self.h_offset;
        range
            .map(|i| {
                let line = &data.blame.lines[i];
                let commit = &data.blame.commits[line.commit];
                let first_of_group = i == 0 || data.blame.lines[i - 1].commit != line.commit;
                let index = line.commit;
                let hovered = self.hovered == Some(index) || self.selected == Some(index);
                let gutter_bg = if hovered {
                    hover_color
                } else if commit.uncommitted {
                    uncommitted_color
                } else {
                    age_color.opacity(0.08 + 0.5 * data.age[index])
                };
                let label = if !first_of_group {
                    String::new()
                } else if commit.uncommitted {
                    "Not Committed Yet".to_string()
                } else {
                    format!("{}  {}", format_date(commit.time), short_name(&commit.author))
                };
                let annotation = div()
                    .id(("annotation", i))
                    .test_support()
                    .w(scaled(ANNOTATION_WIDTH, s))
                    .flex_none()
                    .h_full()
                    .px_2()
                    .flex()
                    .items_center()
                    .bg(gutter_bg)
                    .text_size(scaled(12., s))
                    .truncate()
                    .cursor_pointer()
                    .child(label)
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        let next = if *hovered { Some(index) } else { None };
                        if this.hovered != next {
                            this.hovered = next;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        this.selected = Some(index);
                        if event.click_count() >= 2
                            && let Some((id, _)) = this.commit_at(index)
                        {
                            crate::navigator::select_in_log(id, cx);
                        }
                        cx.notify();
                    }))
                    .context_menu({
                        let view = cx.entity().downgrade();
                        let (id, uncommitted) = (commit.id, commit.uncommitted);
                        move |menu, _, _| {
                            let item = |label: &str, f: fn(&mut BlameView, usize, &mut Context<BlameView>)| {
                                let view = view.clone();
                                PopupMenuItem::new(label.to_string()).on_click(move |_, _, cx| {
                                    view.update(cx, |this, cx| f(this, index, cx)).unwrap_or(())
                                })
                            };
                            let mut menu = menu.item(item("Annotate Previous Revision", Self::annotate_previous));
                            if !uncommitted {
                                menu = menu
                                    .item(item("Show Diff", Self::show_diff))
                                    .item(PopupMenuItem::new("Select in Log").on_click(move |_, _, cx| {
                                        crate::navigator::select_in_log(id, cx);
                                    }))
                                    .separator()
                                    .item(PopupMenuItem::new("Copy Revision Number").on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(id.to_string()))
                                    }));
                            }
                            menu
                        }
                    });
                let range = data.side.line_range(i as u32);
                let text = &data.side.text[range.clone()];
                let selected = self.selection.selection.and_then(|s| s.line_range(0, i as u32, text.len()));
                let rendered = crate::selection::render_line(
                    text,
                    clip(&data.side.syntax, &range).collect(),
                    selected,
                    selection_bg,
                );
                let code = crate::selection::attach(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .px_2()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .flex()
                        .items_center(),
                    0,
                    i as u32,
                    &rendered,
                    text.to_string().into(),
                    cx,
                )
                .child(crate::selection::line_content(rendered, selection_bg, h_offset));
                div()
                    .id(("blame-line", i))
                    .test_support()
                    .h(scaled(ROW_HEIGHT, s))
                    .w_full()
                    .flex()
                    .font_family(mono.clone())
                    .when(hovered, |d| d.bg(hover_color.opacity(0.35)))
                    .when(!hovered, |d| d.bg(bg))
                    .child(annotation)
                    .child(
                        div()
                            .w(scaled(48., s))
                            .flex_none()
                            .px_1()
                            .flex()
                            .justify_end()
                            .items_center()
                            .text_size(scaled(12., s))
                            .text_color(muted)
                            .child((i + 1).to_string()),
                    )
                    .child(code)
                    .into_any_element()
            })
            .collect()
    }

    fn render_status(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let data = self.data.as_ref()?;
        let index = self.hovered.or(self.selected)?;
        let c = &data.blame.commits[index];
        let text = if c.uncommitted {
            "Not committed yet".to_string()
        } else {
            format!("{}  {} <{}>  {}  {}", c.id.to_hex_with_len(8), c.author, c.email, format_date(c.time), c.summary)
        };
        Some(
            div()
                .px_2()
                .py_1()
                .border_t_1()
                .border_color(cx.theme().border)
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(text),
        )
    }
}

impl Render for BlameView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let options = self.options;
        let toggle = |id: &'static str, label: &'static str, on: bool| {
            Button::new(id).small().ghost().label(if on { format!("✓ {label}") } else { label.to_string() })
        };
        let body: AnyElement = if let Some(error) = &self.error {
            div().p_4().text_color(muted).child(error.clone()).into_any_element()
        } else if self.data.is_none() {
            div().p_4().text_color(muted).child("Annotating…").into_any_element()
        } else {
            div()
                .id("blame-scroll")
                .test_support()
                .flex_1()
                .min_h_0()
                .flex()
                .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                    let dx = f32::from(event.delta.pixel_delta(px(ROW_HEIGHT)).x);
                    if dx != 0.0 {
                        this.scroll_horizontally(dx, cx);
                    }
                }))
                .child(
                    uniform_list("blame-rows", self.line_count(), cx.processor(Self::render_rows))
                        .track_scroll(&self.scroll)
                        .size_full()
                        .text_size(theme.mono_font_size),
                )
                .into_any_element()
        };
        let title = match self.rev {
            Some(rev) => format!("{} @ {}", self.path, rev.to_hex_with_len(8)),
            None => self.path.clone(),
        };
        crate::selection::release(div().id("blame-view"), cx)
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &CopyBlameText, _, cx| this.copy_selection(cx)))
            .on_action(cx.listener(|this, _: &SelectAllBlameText, _, cx| this.select_all_text(0, cx)))
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(border)
                    .child(toggle("ignore-ws", "Ignore Whitespace", options.ignore_whitespace).on_click(
                        cx.listener(|this, _, _, cx| this.toggle(|o| o.ignore_whitespace = !o.ignore_whitespace, cx)),
                    ))
                    .child(
                        toggle("detect-moves", "Detect Movements Within File", options.detect_moves).on_click(
                            cx.listener(|this, _, _, cx| this.toggle(|o| o.detect_moves = !o.detect_moves, cx)),
                        ),
                    )
                    .child(toggle("detect-copies", "Detect Movements Across Files", options.detect_copies).on_click(
                        cx.listener(|this, _, _, cx| this.toggle(|o| o.detect_copies = !o.detect_copies, cx)),
                    ))
                    .child(div().flex_1())
                    .child(div().text_color(muted).truncate().child(title)),
            )
            .child(body)
            .children(self.render_status(cx))
    }
}

/// "Jane Doe" -> "Jane Doe"; long names keep the first name and initials.
fn short_name(name: &str) -> String {
    if name.chars().count() <= 18 {
        return name.to_string();
    }
    let mut parts = name.split_whitespace();
    let first = parts.next().unwrap_or(name);
    let initials: String = parts.filter_map(|p| p.chars().next()).map(|c| format!(" {c}.")).collect();
    format!("{first}{initials}")
}

fn format_date(secs: i64) -> String {
    use chrono::{Local, TimeZone};
    Local.timestamp_opt(secs, 0).single().map(|t| t.format("%d.%m.%Y").to_string()).unwrap_or_default()
}

impl SelectableText for BlameView {
    fn selection_state(&mut self) -> &mut SelectionState {
        &mut self.selection
    }

    fn line_text(&self, _: usize, line: u32) -> Option<&str> {
        let side = &self.data.as_ref()?.side;
        (line < side.line_count()).then(|| &side.text[side.line_range(line)])
    }

    fn line_count(&self, _: usize) -> u32 {
        self.data.as_ref().map_or(0, |d| d.side.line_count())
    }

    fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }
}
