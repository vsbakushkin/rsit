//! The Log: refs on the left, commit table with the graph in the middle,
//! changed files and commit details on the right (IntelliJ Git → Log).

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rsit_git::{FileChange, ObjectId, Ref, RefKind, Repo};
use rsit_log::{LogData, LogFilter, MetaCache, VisibleGraph};

use crate::graph_paint::{self, ROW_HEIGHT, RowGraph};

const CONTEXT: &str = "LogTable";
const FILTER_DELAY: Duration = Duration::from_millis(350);

actions!(
    log,
    [SelectPrev, SelectNext, SelectPageUp, SelectPageDown, SelectFirst, SelectLast, CopyHash, FocusFilter, Refresh, ShowDiff]
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
        KeyBinding::new("ctrl-d", ShowDiff, Some(CONTEXT)),
        KeyBinding::new("enter", ShowDiff, Some(CONTEXT)),
        KeyBinding::new("ctrl-f", FocusFilter, None),
        KeyBinding::new("f5", Refresh, None),
        KeyBinding::new("ctrl-r", Refresh, None),
    ]);
}

pub struct LogView {
    repo: Repo,
    /// Latest loaded history.
    data: Option<Arc<LogData>>,
    /// What the table shows (the history, possibly filtered).
    graph: Option<VisibleGraph>,
    filter: LogFilter,
    meta: MetaCache,
    selected: Option<u32>,
    changes: Vec<FileChange>,
    /// Branches containing the selected commit; `None` while computing.
    containing: Option<Vec<String>>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    text_input: Entity<InputState>,
    user_input: Entity<InputState>,
    paths_input: Entity<InputState>,
    status: SharedString,
    loading: bool,
    filtering: bool,
    _load: Option<Task<()>>,
    _filter: Option<Task<()>>,
    _filter_delay: Option<Task<()>>,
    _changes: Option<Task<()>>,
    _watch: Option<(rsit_log::watch::RefsWatcher, Task<()>)>,
    _subscriptions: Vec<Subscription>,
}

impl LogView {
    pub fn new(repo: Repo, filter: LogFilter, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_options(repo, filter, true, window, cx)
    }

    /// `watch_refs: false` skips the file system watcher (UI tests run on a
    /// deterministic scheduler that forbids wakeups from foreign threads).
    pub fn with_options(
        repo: Repo,
        filter: LogFilter,
        watch_refs: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = |placeholder: &'static str, value: String, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                state.set_value(value, window, cx);
                state
            })
        };
        let text_input = input("Text or hash", filter.text.clone(), window, cx);
        let user_input = input("User", filter.user.clone(), window, cx);
        let paths_input = input("Paths", filter.paths.join(", "), window, cx);
        let subscriptions = [&text_input, &user_input, &paths_input]
            .into_iter()
            .map(|input| {
                cx.subscribe_in(input, window, |this, _, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => this.schedule_filter(cx),
                    InputEvent::PressEnter { .. } => this.update_filter_from_inputs(cx),
                    _ => {}
                })
            })
            .collect();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut this = Self {
            meta: MetaCache::new(&repo),
            repo,
            data: None,
            graph: None,
            filter,
            selected: None,
            changes: Vec::new(),
            containing: None,
            scroll: UniformListScrollHandle::new(),
            focus,
            text_input,
            user_input,
            paths_input,
            status: SharedString::default(),
            loading: true,
            filtering: false,
            _load: None,
            _filter: None,
            _filter_delay: None,
            _changes: None,
            _watch: None,
            _subscriptions: subscriptions,
        };
        this.reload(true, cx);
        if watch_refs {
            this.watch_refs(cx);
        }
        this
    }

    /// Refreshes the log when refs change on disk (commit, fetch, checkout…).
    fn watch_refs(&mut self, cx: &mut Context<Self>) {
        let (watcher, mut events) = match rsit_log::watch::watch_repo(&self.repo) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("rsit: cannot watch refs: {e:#}");
                return;
            }
        };
        let task = cx.spawn(async move |this, cx| {
            use futures::StreamExt as _;
            while let Some(event) = events.next().await {
                if event != rsit_log::watch::RepoEvent::Refs {
                    continue;
                }
                // git writes several files per operation; wait for it to settle
                cx.background_executor().timer(Duration::from_millis(300)).await;
                while events.try_recv().is_ok() {}
                if this.update(cx, |this, cx| this.reload(false, cx)).is_err() {
                    break;
                }
            }
        });
        self._watch = Some((watcher, task));
    }

    // ---- loading ----

    /// Loads the history. With `first_screen`, shows the newest commits first
    /// and then the whole history (startup); otherwise swaps in the new data once.
    fn reload(&mut self, first_screen: bool, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        self.loading = true;
        self._load = Some(cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            if !first_screen {
                let full = cx.background_spawn(async move { LogData::load(repo, None) }).await;
                this.update(cx, |this, cx| this.set_data(full, started, cx)).ok();
                return;
            }
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
        cx.notify();
    }

    fn set_data(&mut self, data: anyhow::Result<LogData>, started: std::time::Instant, cx: &mut Context<Self>) {
        let data = match data {
            Ok(data) => data,
            Err(e) => {
                self.loading = false;
                self.status = format!("Error: {e:#}").into();
                cx.notify();
                return;
            }
        };
        self.loading = data.partial;
        self.status = if data.partial {
            format!("{}+ commits, loading…", data.len())
        } else {
            format!("{} commits · {} refs · {:.0?}", data.len(), data.refs.refs.len(), started.elapsed())
        }
        .into();
        self.data = Some(Arc::new(data));
        self.apply_filter(cx);
    }

    // ---- filtering ----

    fn schedule_filter(&mut self, cx: &mut Context<Self>) {
        self._filter_delay = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FILTER_DELAY).await;
            this.update(cx, |this, cx| this.update_filter_from_inputs(cx)).ok();
        }));
    }

    fn update_filter_from_inputs(&mut self, cx: &mut Context<Self>) {
        self._filter_delay = None;
        let mut filter = self.filter.clone();
        filter.text = self.text_input.read(cx).value().trim().to_string();
        filter.user = self.user_input.read(cx).value().trim().to_string();
        filter.paths = self
            .paths_input
            .read(cx)
            .value()
            .split([',', ' '])
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(String::from)
            .collect();
        self.set_filter(filter, cx);
    }

    fn set_filter(&mut self, filter: LogFilter, cx: &mut Context<Self>) {
        if filter == self.filter {
            return;
        }
        self.filter = filter;
        self.apply_filter(cx);
    }

    /// Rebuilds the visible graph for the current data and filter.
    fn apply_filter(&mut self, cx: &mut Context<Self>) {
        let Some(data) = self.data.clone() else { return };
        if self.filter.is_empty() {
            self._filter = None;
            self.filtering = false;
            self.set_graph(VisibleGraph::new(data, None), cx);
            return;
        }
        self.filtering = true;
        let filter = self.filter.clone();
        self._filter = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let filtered = rsit_log::filter::filtered_graph(&data, &filter)?;
                    anyhow::Ok(VisibleGraph::new(data, filtered))
                })
                .await;
            this.update(cx, |this, cx| {
                this.filtering = false;
                match result {
                    Ok(graph) => this.set_graph(graph, cx),
                    Err(e) => {
                        this.status = format!("Filter failed: {e:#}").into();
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn set_graph(&mut self, graph: VisibleGraph, cx: &mut Context<Self>) {
        // keep the selected commit selected if it is still visible
        let selected_id = self.selected_id();
        let row = selected_id.and_then(|id| graph.row_of(&id)).or_else(|| (!graph.is_empty()).then_some(0));
        self.graph = Some(graph);
        self.selected = None;
        match row {
            Some(row) => self.select(row, cx),
            None => self.changes.clear(),
        }
        cx.notify();
    }

    fn toggle_branch_filter(&mut self, name: String, cx: &mut Context<Self>) {
        let mut filter = self.filter.clone();
        if filter.branches == [name.clone()] {
            filter.branches.clear();
        } else {
            filter.branches = vec![name];
        }
        self.set_filter(filter, cx);
    }

    fn clear_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [&self.text_input, &self.user_input, &self.paths_input] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.set_filter(LogFilter::default(), cx);
    }

    pub fn repo(&self) -> &Repo {
        &self.repo
    }

    pub fn menu_target(&self) -> Option<crate::commit_menu::MenuTarget> {
        let graph = self.graph.as_ref()?;
        Some(self.menu_target_for(self.selected_id()?, &graph.data))
    }

    // ---- selection ----

    pub fn selected_id(&self) -> Option<ObjectId> {
        Some(self.graph.as_ref()?.id(self.selected?))
    }

    fn select(&mut self, row: u32, cx: &mut Context<Self>) {
        let Some(graph) = &self.graph else { return };
        if graph.is_empty() {
            return;
        }
        let row = row.min(graph.len() as u32 - 1);
        self.scroll.scroll_to_item(row as usize, ScrollStrategy::Nearest);
        if self.selected == Some(row) {
            return;
        }
        self.selected = Some(row);
        self.changes.clear();
        self.containing = None;
        let repo = self.repo.clone();
        let id = graph.id(row);
        let data = graph.data.clone();
        let permanent = graph.permanent_row(row);
        self._changes = Some(cx.spawn(async move |this, cx| {
            let changes = cx.background_spawn(async move { rsit_git::changed_files(&repo.local(), id) }).await;
            this.update(cx, |this, cx| {
                if this.selected_id() == Some(id) {
                    this.changes = changes.unwrap_or_default();
                    cx.notify();
                }
            })
            .ok();
            let containing = cx.background_spawn(async move { data.containing_branches(permanent) }).await;
            this.update(cx, |this, cx| {
                if this.selected_id() == Some(id) {
                    this.containing = Some(containing);
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    /// Selects `id`, dropping filters that hide it.
    fn navigate_to(&mut self, id: ObjectId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(graph) = &self.graph else { return };
        match graph.row_of(&id) {
            Some(row) => {
                self.scroll.scroll_to_item(row as usize, ScrollStrategy::Center);
                self.select(row, cx);
            }
            None if graph.data.row_of(&id).is_some() => {
                // hidden by the filter: show everything, then select it
                self.clear_filters(window, cx);
                if let Some(row) = self.graph.as_ref().and_then(|g| g.row_of(&id)) {
                    self.scroll.scroll_to_item(row as usize, ScrollStrategy::Center);
                    self.select(row, cx);
                }
            }
            None => {}
        }
    }

    /// Follows the arrow of a long edge under the given point of `row`'s graph.
    fn jump_by_arrow(&mut self, row: u32, x: f32, y: f32, cx: &mut Context<Self>) -> bool {
        let Some(graph) = self.graph.as_mut() else { return false };
        let elements = graph.print_row(row);
        let Some(target) = graph_paint::element_at(&elements, x, y).and_then(graph_paint::arrow_target) else {
            return false;
        };
        self.scroll.scroll_to_item(target as usize, ScrollStrategy::Center);
        self.select(target, cx);
        true
    }

    fn move_selection(&mut self, delta: i64, cx: &mut Context<Self>) {
        let Some(graph) = &self.graph else { return };
        let last = graph.len() as i64 - 1;
        let row = self.selected.map_or(0, |r| r as i64 + delta).clamp(0, last.max(0));
        self.select(row as u32, cx);
    }

    fn page_rows(&self) -> i64 {
        let height = self.scroll.0.borrow().base_handle.bounds().size.height;
        ((f32::from(height) / ROW_HEIGHT) as i64 - 1).max(1)
    }

    fn copy_hash(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.selected_id() {
            cx.write_to_clipboard(ClipboardItem::new_string(id.to_string()));
        }
    }

    /// Opens the diff of the selected commit, reading its files if they are not loaded yet.
    fn show_commit_diff(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id() else { return };
        let files = if self.changes.is_empty() {
            rsit_git::changed_files(&self.repo.local(), id).unwrap_or_default()
        } else {
            self.changes.clone()
        };
        if !files.is_empty() {
            crate::diff_view::open(self.repo.clone(), id, files, 0, cx);
        }
    }

    fn open_diff(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id() else { return };
        crate::diff_view::open(self.repo.clone(), id, self.changes.clone(), index, cx);
    }

    // ---- rendering ----

    fn render_rows(&mut self, range: Range<usize>, _window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(graph) = self.graph.as_mut() else { return Vec::new() };
        let data = graph.data.clone();
        let theme = cx.theme();
        let (list_active, background, muted, hover) =
            (theme.list_active, theme.background, theme.muted_foreground, theme.list_hover);
        let min_graph = graph.recommended_width().min(6) as f32 * graph_paint::LANE_WIDTH;
        let mono = theme.mono_font_family.clone();
        let mut rows = Vec::with_capacity(range.len());
        for row in range {
            let row = row as u32;
            let permanent = graph.permanent_row(row);
            let elements = graph.print_row(row);
            let graph_width = graph_paint::graph_width(&elements).max(min_graph);
            let selected = self.selected == Some(row);
            let bg = if selected { list_active } else { background };
            let id = data.id(permanent);
            let meta = self.meta.get(id);
            let (subject, author, date) = match &meta {
                Some(m) => (m.subject().to_string(), m.author.name.clone(), format_time(m.author.time)),
                None => (String::new(), String::new(), String::new()),
            };
            let row_graph = RowGraph { elements, is_head: data.is_head(permanent), background: bg };
            let hash = id.to_hex_with_len(8).to_string();
            rows.push(
                div()
                    .id(("row", row as usize))
                    .test_support()
                    .h(px(ROW_HEIGHT))
                    .w_full()
                    .flex()
                    .items_center()
                    .bg(bg)
                    .when(!selected, |d| d.hover(|s| s.bg(hover)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        window.focus(&this.focus, cx);
                        this.select(row, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus, cx);
                            this.select(row, cx);
                        }),
                    )
                    .child({
                        // bounds from the last paint, to hit-test clicks on arrows
                        let painted = Rc::new(Cell::new(None::<Bounds<Pixels>>));
                        let painted_for_click = painted.clone();
                        div()
                            .id(("graph", row as usize))
                            .test_support()
                            .flex_none()
                            .child(
                                canvas(|_, _, _| (), move |bounds, _, window, _| {
                                    painted.set(Some(bounds));
                                    graph_paint::paint_row(bounds, row_graph, window)
                                })
                                .w(px(graph_width))
                                .h(px(ROW_HEIGHT)),
                            )
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                let Some(bounds) = painted_for_click.get() else { return };
                                let local = event.position() - bounds.origin;
                                if this.jump_by_arrow(row, f32::from(local.x), f32::from(local.y), cx) {
                                    window.focus(&this.focus, cx);
                                    cx.stop_propagation();
                                }
                            }))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.))
                            .flex()
                            .items_center()
                            .gap_1()
                            .overflow_hidden()
                            .child(div().flex_shrink(1.).min_w(px(60.)).truncate().child(subject))
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .flex_shrink(1.)
                                    .min_w_0()
                                    .overflow_hidden()
                                    .children(data.refs_at(permanent).iter().take(4).map(ref_label)),
                            ),
                    )
                    .child(div().w(px(160.)).flex_shrink(1.).min_w(px(40.)).px_2().truncate().text_color(muted).child(author))
                    .child(div().w(px(130.)).flex_shrink(1.).min_w(px(40.)).px_2().truncate().text_color(muted).child(date))
                    .child(
                        div()
                            .w(px(90.))
                            .flex_none()
                            .px_2()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_color(muted)
                            .font_family(mono.clone())
                            .child(hash),
                    )
                    .into_any_element(),
            );
        }
        rows
    }

    fn render_table(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.graph.as_ref().map_or(0, |g| g.len());
        let theme = cx.theme();
        let header = |label: &'static str, w: Option<f32>| {
            let d = div().px_2().truncate().child(label);
            match w {
                // the hash keeps its width; author and date give way to the subject
                Some(w) if label == "Hash" => d.w(px(w)).flex_none(),
                Some(w) => d.w(px(w)).flex_shrink(1.).min_w(px(40.)),
                None => d.flex_1().min_w(px(160.)),
            }
        };
        let empty_message = match &self.graph {
            Some(g) if g.is_empty() && g.is_filtered() => Some("No commits matching filters"),
            Some(g) if g.is_empty() => Some("No commits"),
            None if self.loading => Some("Loading…"),
            _ => None,
        };
        div()
            .id("log-table")
            .test_support()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPageUp, _, cx| this.move_selection(-this.page_rows(), cx)))
            .on_action(cx.listener(|this, _: &SelectPageDown, _, cx| this.move_selection(this.page_rows(), cx)))
            .on_action(cx.listener(|this, _: &SelectFirst, _, cx| this.select(0, cx)))
            .on_action(cx.listener(|this, _: &SelectLast, _, cx| this.move_selection(i64::MAX / 2, cx)))
            .on_action(cx.listener(|this, _: &CopyHash, _, cx| this.copy_hash(cx)))
            .on_action(cx.listener(|this, _: &ShowDiff, _, cx| this.show_commit_diff(cx)))
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
            .when_some(empty_message, |d, msg| {
                d.child(div().p_4().flex().justify_center().text_color(theme.muted_foreground).child(msg))
            })
            .child({
                let view = cx.entity().downgrade();
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .child(
                        uniform_list("log", count, cx.processor(Self::render_rows))
                            .track_scroll(&self.scroll)
                            .size_full(),
                    )
                    .context_menu(move |menu, _, cx| crate::commit_menu::build(menu, view.clone(), cx))
            })
    }

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let status: SharedString = if self.filtering {
            "Filtering…".into()
        } else {
            match &self.graph {
                Some(g) if g.is_filtered() => format!("{} of {}", g.len(), g.data.len()).into(),
                _ => self.status.clone(),
            }
        };
        let branch_chip = self.filter.branches.first().cloned().map(|name| {
            Button::new("branch-filter")
                .small()
                .outline()
                .label(format!("Branch: {name} ✕"))
                .tooltip("Remove branch filter")
                .on_click(cx.listener(|this, _, _, cx| {
                    let mut filter = this.filter.clone();
                    filter.branches.clear();
                    this.set_filter(filter, cx);
                }))
        });
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(border)
            .child(div().w(px(280.)).child(Input::new(&self.text_input).cleanable(true)))
            .child(
                Button::new("regex")
                    .small()
                    .ghost()
                    .label(".*")
                    .toggled(self.filter.regex)
                    .tooltip("Regex")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let mut filter = this.filter.clone();
                        filter.regex = !filter.regex;
                        this.set_filter(filter, cx);
                    })),
            )
            .child(
                Button::new("match-case")
                    .small()
                    .ghost()
                    .label("Aa")
                    .toggled(self.filter.match_case)
                    .tooltip("Match case")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let mut filter = this.filter.clone();
                        filter.match_case = !filter.match_case;
                        this.set_filter(filter, cx);
                    })),
            )
            .child(div().w(px(150.)).child(Input::new(&self.user_input).cleanable(true)))
            .child(div().w(px(200.)).child(Input::new(&self.paths_input).cleanable(true)))
            .children(branch_chip)
            .when(!self.filter.is_empty(), |d| {
                d.child(
                    Button::new("clear-filters")
                        .small()
                        .ghost()
                        .label("Clear")
                        .on_click(cx.listener(|this, _, window, cx| this.clear_filters(window, cx))),
                )
            })
            .child(div().flex_1())
            .child(div().text_color(muted).whitespace_nowrap().child(status))
    }

    fn render_refs(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mut entries: Vec<AnyElement> = Vec::new();
        if let Some(graph) = &self.graph {
            let data = graph.data.clone();
            let sections = [("Local", RefKind::LocalBranch), ("Remote", RefKind::RemoteBranch), ("Tags", RefKind::Tag)];
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
                    let name = r.name.clone();
                    let current = kind == RefKind::LocalBranch && data.refs.current_branch.as_deref() == Some(r.name.as_str());
                    let visible = graph.row_of(&target).is_some();
                    let filtered_on = self.filter.branches.contains(&r.name);
                    entries.push(
                        div()
                            .id(SharedString::from(format!("ref-{}", r.full_name)))
                            .px_3()
                            .h(px(ROW_HEIGHT))
                            .flex()
                            .items_center()
                            .gap_1()
                            .truncate()
                            .when(!visible, |d| d.text_color(theme.muted_foreground))
                            .when(filtered_on, |d| d.bg(theme.list_active))
                            .hover(|s| s.bg(theme.list_hover))
                            .when(current, |d| d.child(div().text_color(ref_color(RefKind::Head)).child("★")))
                            .child(r.name.clone())
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                if event.click_count() >= 2 {
                                    this.toggle_branch_filter(name.clone(), cx);
                                } else {
                                    window.focus(&this.focus, cx);
                                    this.navigate_to(target, window, cx);
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
        let selected = self.selected_id().zip(self.graph.as_ref().map(|g| g.data.clone()));
        let meta = selected.and_then(|(id, data)| Some((self.meta.get(id)?, data)));
        let files = self.changes.iter().enumerate().map(|(i, change)| {
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
                        this.open_diff(i, cx);
                    }
                }))
        });
        let containing = self.containing.clone();
        let details = meta.map(|(meta, data)| {
            let parents = meta.parents.clone();
            let refs: Vec<String> =
                data.row_of(&meta.id).map(|row| data.refs_at(row).iter().map(|r| r.name.clone()).collect()).unwrap_or_default();
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
                            d.child(format!("committed by {} {}", meta.committer.name, format_time_full(meta.committer.time)))
                        }),
                )
                .child(div().font_family(cx.theme().mono_font_family.clone()).text_color(muted).child(meta.id.to_string()))
                .when(!parents.is_empty(), |d| {
                    d.child(div().flex().flex_wrap().gap_1().text_color(muted).child("parents:").children(
                        parents.into_iter().map(|p| {
                            div()
                                .id(SharedString::from(format!("parent-{p}")))
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_color(cx.theme().link)
                                .cursor_pointer()
                                .child(p.to_hex_with_len(10).to_string())
                                .on_click(cx.listener(move |this, _, window, cx| this.navigate_to(p, window, cx)))
                        }),
                    ))
                })
                .when(!refs.is_empty(), |d| d.child(div().text_color(muted).child(format!("refs: {}", refs.join(", ")))))
                .child(div().text_color(muted).whitespace_normal().child(match &containing {
                    None => "In branches: computing…".to_string(),
                    Some(b) if b.is_empty() => "In no branches".to_string(),
                    Some(b) => {
                        const SHOWN: usize = 12;
                        let mut text = format!("In {} branch{}: {}", b.len(), if b.len() == 1 { "" } else { "es" }, b[..b.len().min(SHOWN)].join(", "));
                        if b.len() > SHOWN {
                            text.push_str(&format!(" and {} more", b.len() - SHOWN));
                        }
                        text
                    }
                }))
        });
        v_resizable("details")
            .child(
                resizable_panel().child(
                    div().id("changes").size_full().overflow_y_scroll().border_b_1().border_color(border).children(files),
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, fg) = (theme.background, theme.foreground);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
                let focus = this.text_input.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            }))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.reload(false, cx)))
            .child(self.render_toolbar(cx))
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

pub fn change_style(kind: rsit_git::ChangeKind) -> (char, Hsla) {
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
