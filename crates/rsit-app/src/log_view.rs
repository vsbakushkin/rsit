//! The Log: refs on the left, commit table with the graph in the middle,
//! changed files and commit details on the right (IntelliJ Git → Log).

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rsit_git::{FileChange, Ref, RefKind, Repo};
use rsit_log::{GraphPrinter, LogData, MetaCache};

use crate::graph_paint::{self, ROW_HEIGHT, RowGraph};

const CONTEXT: &str = "LogTable";

actions!(
    log,
    [SelectPrev, SelectNext, SelectPageUp, SelectPageDown, SelectFirst, SelectLast, CopyHash, FocusFilter, Refresh]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPrev, Some(CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("pageup", SelectPageUp, Some(CONTEXT)),
        KeyBinding::new("pagedown", SelectPageDown, Some(CONTEXT)),
        KeyBinding::new("home", SelectFirst, Some(CONTEXT)),
        KeyBinding::new("end", SelectLast, Some(CONTEXT)),
        KeyBinding::new("ctrl-c", CopyHash, Some(CONTEXT)),
        KeyBinding::new("ctrl-f", FocusFilter, None),
        KeyBinding::new("f5", Refresh, None),
        KeyBinding::new("ctrl-r", Refresh, None),
    ]);
}

pub struct LogView {
    repo: Repo,
    data: Option<Arc<LogData>>,
    printer: Option<GraphPrinter>,
    meta: MetaCache,
    selected: Option<u32>,
    changes: Vec<FileChange>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    filter: Entity<InputState>,
    status: SharedString,
    _load: Option<Task<()>>,
    _changes: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl LogView {
    pub fn new(repo: Repo, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Text or hash"));
        let subscriptions = vec![cx.subscribe_in(&filter, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.find_next(window, cx);
            }
        })];
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut this = Self {
            meta: MetaCache::new(&repo),
            repo,
            data: None,
            printer: None,
            selected: None,
            changes: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            focus,
            filter,
            status: "Loading…".into(),
            _load: None,
            _changes: None,
            _subscriptions: subscriptions,
        };
        this.reload(cx);
        this
    }

    /// Loads the first screen quickly, then the whole history.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            let first = {
                let repo = repo.clone();
                cx.background_spawn(async move { LogData::load(repo, Some(rsit_log::FIRST_SCREEN_COMMITS)) }).await
            };
            let partial = matches!(&first, Ok(d) if d.partial);
            if this.update(cx, |this, cx| this.set_data(first, started, cx)).is_err() || !partial {
                return;
            }
            let full = cx.background_spawn(async move { LogData::load(repo, None) }).await;
            this.update(cx, |this, cx| this.set_data(full, started, cx)).ok();
        }));
    }

    fn set_data(&mut self, data: anyhow::Result<LogData>, started: std::time::Instant, cx: &mut Context<Self>) {
        let data = match data {
            Ok(data) => data,
            Err(e) => {
                self.status = format!("Error: {e:#}").into();
                cx.notify();
                return;
            }
        };
        // keep the selected commit selected across reloads
        let selected_id = self.selected.and_then(|row| self.data.as_ref().map(|d| d.id(row)));
        self.printer = Some(GraphPrinter::new(&data));
        self.status = if data.partial {
            format!("{}+ commits, loading…", data.len())
        } else {
            format!("{} commits · {} refs · {:.0?}", data.len(), data.refs.refs.len(), started.elapsed())
        }
        .into();
        let data = Arc::new(data);
        let row = selected_id.and_then(|id| data.row_of(&id)).or_else(|| (!data.is_empty()).then_some(0));
        self.data = Some(data);
        self.selected = None;
        if let Some(row) = row {
            self.select(row, cx);
        }
        cx.notify();
    }

    fn select(&mut self, row: u32, cx: &mut Context<Self>) {
        let Some(data) = self.data.clone() else { return };
        if data.is_empty() {
            return;
        }
        let row = row.min(data.len() as u32 - 1);
        self.scroll.scroll_to_item(row as usize, ScrollStrategy::Nearest);
        if self.selected == Some(row) {
            return;
        }
        self.selected = Some(row);
        self.changes.clear();
        let repo = self.repo.clone();
        let id = data.id(row);
        self._changes = Some(cx.spawn(async move |this, cx| {
            let changes = cx.background_spawn(async move { rsit_git::changed_files(&repo.local(), id) }).await;
            this.update(cx, |this, cx| {
                if this.selected.map(|r| data.id(r)) == Some(id) {
                    this.changes = changes.unwrap_or_default();
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn move_selection(&mut self, delta: i64, cx: &mut Context<Self>) {
        let Some(data) = &self.data else { return };
        let last = data.len() as i64 - 1;
        let row = self.selected.map_or(0, |r| r as i64 + delta).clamp(0, last.max(0));
        self.select(row as u32, cx);
    }

    fn page_rows(&self) -> i64 {
        let height = self.scroll.0.borrow().base_handle.bounds().size.height;
        ((f32::from(height) / ROW_HEIGHT) as i64 - 1).max(1)
    }

    /// Selects the next commit (after the selection) whose hash, subject or author matches the filter.
    fn find_next(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let query = self.filter.read(cx).value().trim().to_lowercase();
        let Some(data) = self.data.clone() else { return };
        if query.is_empty() || data.is_empty() {
            return;
        }
        let start = self.selected.map_or(0, |r| r + 1);
        let n = data.len() as u32;
        for offset in 0..n {
            let row = (start + offset) % n;
            let id = data.id(row);
            if id.to_hex().to_string().starts_with(&query) {
                return self.select(row, cx);
            }
            let Some(meta) = self.meta.get(id) else { continue };
            if meta.message.to_lowercase().contains(&query) || meta.author.name.to_lowercase().contains(&query) {
                return self.select(row, cx);
            }
        }
        self.status = format!("No commits match “{query}”").into();
        cx.notify();
    }

    fn copy_hash(&mut self, cx: &mut Context<Self>) {
        if let (Some(data), Some(row)) = (&self.data, self.selected) {
            cx.write_to_clipboard(ClipboardItem::new_string(data.id(row).to_string()));
        }
    }

    fn open_diff(&mut self, change: FileChange, cx: &mut Context<Self>) {
        let (Some(data), Some(row)) = (&self.data, self.selected) else { return };
        crate::diff_view::open(self.repo.clone(), data.id(row), change, cx);
    }

    fn render_rows(&mut self, range: Range<usize>, _window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let (Some(data), Some(printer)) = (self.data.clone(), self.printer.as_mut()) else { return Vec::new() };
        let theme = cx.theme();
        let (list_active, background, muted) = (theme.list_active, theme.background, theme.muted_foreground);
        let min_graph = printer.recommended_width().min(6) as f32 * graph_paint::LANE_WIDTH;
        let mut rows = Vec::with_capacity(range.len());
        for row in range {
            let row = row as u32;
            let elements = printer.row(&data, row);
            let graph_width = graph_paint::graph_width(&elements).max(min_graph);
            let selected = self.selected == Some(row);
            let bg = if selected { list_active } else { background };
            let meta = self.meta.get(data.id(row));
            let (subject, author, date) = match &meta {
                Some(m) => (m.subject().to_string(), m.author.name.clone(), format_time(m.author.time)),
                None => (String::new(), String::new(), String::new()),
            };
            let row_graph = RowGraph { elements, is_head: data.is_head(row), background: bg };
            let hash = data.id(row).to_hex_with_len(8).to_string();
            rows.push(
                div()
                    .id(("row", row as usize))
                    .h(px(ROW_HEIGHT))
                    .w_full()
                    .flex()
                    .items_center()
                    .bg(bg)
                    .when(!selected, |d| d.hover(|s| s.bg(cx.theme().list_hover)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        window.focus(&this.focus, cx);
                        this.select(row, cx);
                    }))
                    .child(
                        canvas(|_, _, _| (), move |bounds, _, window, _| graph_paint::paint_row(bounds, row_graph, window))
                            .w(px(graph_width))
                            .h(px(ROW_HEIGHT))
                            .flex_none(),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .overflow_hidden()
                            .child(div().flex_shrink(1.).min_w_0().truncate().child(subject))
                            .children(data.refs_at(row).iter().take(4).map(ref_label)),
                    )
                    .child(div().w(px(160.)).flex_none().px_2().truncate().text_color(muted).child(author))
                    .child(div().w(px(130.)).flex_none().px_2().truncate().text_color(muted).child(date))
                    .child(div().w(px(90.)).flex_none().px_2().whitespace_nowrap().overflow_hidden().text_color(muted).font_family("monospace").child(hash))
                    .into_any_element(),
            );
        }
        rows
    }

    fn render_table(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.data.as_ref().map_or(0, |d| d.len());
        let theme = cx.theme();
        let header = |label: &'static str, w: Option<f32>| {
            let d = div().px_2().truncate().child(label);
            match w {
                Some(w) => d.w(px(w)).flex_none(),
                None => d.flex_1(),
            }
        };
        div()
            .id("log-table")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPageUp, _, cx| this.move_selection(-this.page_rows(), cx)))
            .on_action(cx.listener(|this, _: &SelectPageDown, _, cx| this.move_selection(this.page_rows(), cx)))
            .on_action(cx.listener(|this, _: &SelectFirst, _, cx| this.select(0, cx)))
            .on_action(cx.listener(|this, _: &SelectLast, _, cx| this.move_selection(i64::MAX / 2, cx)))
            .on_action(cx.listener(|this, _: &CopyHash, _, cx| this.copy_hash(cx)))
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .h(px(ROW_HEIGHT + 2.0))
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_color(theme.muted_foreground)
                    .child(header("Subject", None))
                    .child(header("Author", Some(160.)))
                    .child(header("Date", Some(130.)))
                    .child(header("Hash", Some(90.))),
            )
            .child(
                uniform_list("log", count, cx.processor(Self::render_rows))
                    .track_scroll(&self.scroll)
                    .flex_1()
                    .w_full(),
            )
    }

    fn render_refs(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mut entries: Vec<AnyElement> = Vec::new();
        if let Some(data) = self.data.clone() {
            let sections = [
                ("Local", RefKind::LocalBranch),
                ("Remote", RefKind::RemoteBranch),
                ("Tags", RefKind::Tag),
            ];
            for (title, kind) in sections {
                let mut refs: Vec<&Ref> = data.refs.refs.iter().filter(|r| r.kind == kind).collect();
                if refs.is_empty() {
                    continue;
                }
                refs.sort_by(|a, b| data.refs.label_cmp(a, b));
                entries.push(
                    div().px_2().pt_2().pb_1().text_xs().text_color(theme.muted_foreground).child(title).into_any_element(),
                );
                for r in refs {
                    let target = r.target;
                    let current = data.refs.current_branch.as_deref() == Some(r.name.as_str()) && kind == RefKind::LocalBranch;
                    let loaded = data.row_of(&target).is_some();
                    entries.push(
                        div()
                            .id(SharedString::from(format!("ref-{}", r.full_name)))
                            .px_3()
                            .h(px(ROW_HEIGHT))
                            .flex()
                            .items_center()
                            .gap_1()
                            .truncate()
                            .when(!loaded, |d| d.text_color(theme.muted_foreground))
                            .hover(|s| s.bg(theme.list_hover))
                            .when(current, |d| d.child(div().text_color(ref_color(RefKind::Head)).child("★")))
                            .child(r.name.clone())
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                if let Some(row) = this.data.as_ref().and_then(|d| d.row_of(&target)) {
                                    window.focus(&this.focus, cx);
                                    this.scroll.scroll_to_item(row as usize, ScrollStrategy::Center);
                                    this.select(row, cx);
                                }
                            }))
                            .into_any_element(),
                    );
                }
            }
        }
        div().id("refs").size_full().overflow_y_scroll().children(entries)
    }

    fn render_details(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, hover) = (theme.muted_foreground, theme.border, theme.list_hover);
        let meta = match (&self.data, self.selected) {
            (Some(data), Some(row)) => self.meta.get(data.id(row)).map(|m| (m, data.clone(), row)),
            _ => None,
        };
        let files = self.changes.iter().enumerate().map(|(i, change)| {
            let change_for_click = change.clone();
            let (letter, color) = change_style(change.kind);
            let (dir, name) = match change.path.rsplit_once('/') {
                Some((dir, name)) => (dir.to_string(), name.to_string()),
                None => (String::new(), change.path.clone()),
            };
            div()
                .id(("file", i))
                .h(px(ROW_HEIGHT))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .hover(|s| s.bg(hover))
                .child(div().w(px(12.)).text_color(color).child(letter.to_string()))
                .child(div().text_color(color).child(name))
                .child(div().flex_1().min_w_0().truncate().text_color(muted).child(dir))
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    if event.click_count() >= 2 {
                        this.open_diff(change_for_click.clone(), cx);
                    }
                }))
        });
        let details = meta.map(|(meta, data, row)| {
            let parents: Vec<String> =
                meta.parents.iter().map(|p| p.to_hex_with_len(10).to_string()).collect();
            let refs: Vec<String> = data.refs_at(row).iter().map(|r| r.name.clone()).collect();
            div()
                .p_2()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().whitespace_normal().child(meta.message.trim_end().to_string()))
                .child(
                    div()
                        .text_color(muted)
                        .flex()
                        .flex_col()
                        .child(format!("{} <{}>", meta.author.name, meta.author.email))
                        .child(format!("authored {}", format_time_full(meta.author.time)))
                        .when(meta.committer.email != meta.author.email || meta.committer.time != meta.author.time, |d| {
                            d.child(format!(
                                "committed by {} {}",
                                meta.committer.name,
                                format_time_full(meta.committer.time)
                            ))
                        }),
                )
                .child(div().font_family("monospace").text_color(muted).child(meta.id.to_string()))
                .when(!parents.is_empty(), |d| d.child(div().text_color(muted).child(format!("parents: {}", parents.join(", ")))))
                .when(!refs.is_empty(), |d| d.child(div().text_color(muted).child(format!("refs: {}", refs.join(", ")))))
        });
        v_resizable("details")
            .child(
                resizable_panel().child(
                    div()
                        .id("changes")
                        .size_full()
                        .overflow_y_scroll()
                        .border_b_1()
                        .border_color(border)
                        .children(files),
                ),
            )
            .child(
                resizable_panel()
                    .size(px(260.))
                    .child(div().id("commit-details").size_full().overflow_y_scroll().children(details)),
            )
    }
}

impl Render for LogView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        let theme = cx.theme();
        let (bg, fg, border, muted) = (theme.background, theme.foreground, theme.border, theme.muted_foreground);
        let toolbar = div()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(border)
            .child(div().w(px(320.)).child(Input::new(&self.filter).cleanable(true)))
            .child(div().flex_1())
            .child(div().text_color(muted).child(self.status.clone()));
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
                let focus = this.filter.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            }))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.reload(cx)))
            .child(toolbar)
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("main")
                        .child(resizable_panel().size(px(220.)).child(self.render_refs(cx)))
                        .child(resizable_panel().child(self.render_table(cx)))
                        .child(resizable_panel().size(px(420.)).child(self.render_details(cx))),
                ),
            )
    }
}

fn ref_color(kind: RefKind) -> Hsla {
    // VcsLogStandardColors.Refs: TIP, BRANCH, BRANCH_REF, TAG
    match kind {
        RefKind::Head => rgb(0xE8B74A).into(),
        RefKind::LocalBranch => rgb(0x5FAE5F).into(),
        RefKind::RemoteBranch => rgb(0xA57FD6).into(),
        RefKind::Tag | RefKind::Other => rgb(0x8C8C8C).into(),
    }
}

fn ref_label(r: &Ref) -> AnyElement {
    let color = ref_color(r.kind);
    div()
        .flex_none()
        .px_1()
        .rounded_sm()
        .border_1()
        .border_color(color)
        .text_xs()
        .text_color(color)
        .child(r.name.clone())
        .into_any_element()
}

fn change_style(kind: rsit_git::ChangeKind) -> (char, Hsla) {
    use rsit_git::ChangeKind::*;
    let color: Hsla = match kind {
        Added => rgb(0x62B543).into(),
        Deleted => rgb(0x9A9A9A).into(),
        Modified => rgb(0x5E9EE8).into(),
        Renamed | Copied => rgb(0x3C9E9E).into(),
    };
    (kind.letter(), color)
}

fn format_time(secs: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(t) = Local.timestamp_opt(secs, 0).single() else { return String::new() };
    let today = Local::now().date_naive();
    let date = t.date_naive();
    if date == today {
        format!("Today {}", t.format("%H:%M"))
    } else if today.pred_opt() == Some(date) {
        format!("Yesterday {}", t.format("%H:%M"))
    } else {
        t.format("%d.%m.%Y, %H:%M").to_string()
    }
}

fn format_time_full(secs: i64) -> String {
    use chrono::{Local, TimeZone};
    Local.timestamp_opt(secs, 0).single().map(|t| t.format("%d.%m.%Y %H:%M:%S").to_string()).unwrap_or_default()
}
