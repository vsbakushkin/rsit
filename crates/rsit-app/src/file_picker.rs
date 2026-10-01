//! "Go to File" (IntelliJ Ctrl+Shift+N): fuzzy search over the repository's
//! files; Enter annotates the file, Ctrl+Enter shows its history.

use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rsit_git::Repo;

use crate::file_view::{self, FileTab};

const MAX_RESULTS: usize = 100;

pub struct FilePicker {
    repo: Repo,
    input: Entity<InputState>,
    files: Arc<Vec<String>>,
    matches: Vec<usize>,
    selected: usize,
    scroll: UniformListScrollHandle,
    _load: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// Opens the picker as a dialog in `window`.
pub fn open(repo: Repo, window: &mut Window, cx: &mut App) {
    let picker = cx.new(|cx| FilePicker::new(repo, window, cx));
    let focus = picker.read(cx).input.read(cx).focus_handle(cx);
    window.open_dialog(cx, move |dialog, _, _| {
        let picker_for_ok = picker.clone();
        // the dialog handles Enter itself (before its content sees it): confirm = open
        dialog.title("Go to File").w(px(680.)).child(picker.clone()).on_ok(move |_, _, cx| {
            picker_for_ok.update(cx, |p, cx| p.open_selected(FileTab::Annotate, cx));
            true
        })
    });
    window.defer(cx, move |window, cx| window.focus(&focus, cx));
}

impl FilePicker {
    pub fn new(repo: Repo, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("File name or path"));
        let subscriptions = vec![cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                this.update_matches(cx);
            }
        })];
        let mut this = Self {
            repo,
            input,
            files: Arc::new(Vec::new()),
            matches: Vec::new(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            _load: None,
            _subscriptions: subscriptions,
        };
        let cwd = this.repo.cwd().to_path_buf();
        this._load = Some(cx.spawn(async move |this, cx| {
            let files = cx
                .background_spawn(async move {
                    let out =
                        rsit_git::cli::run(&cwd, &["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
                            .unwrap_or_default();
                    let mut files: Vec<String> =
                        out.split('\0').filter(|f| !f.is_empty()).map(str::to_string).collect();
                    files.sort();
                    files.dedup();
                    files
                })
                .await;
            this.update(cx, |this, cx| {
                this.files = Arc::new(files);
                this.update_matches(cx);
            })
            .ok();
        }));
        this
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn matches(&self) -> Vec<&str> {
        self.matches.iter().map(|&i| self.files[i].as_str()).collect()
    }

    fn update_matches(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().to_string();
        self.matches = rank(&self.files, &query, MAX_RESULTS);
        self.selected = 0;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn move_selection(&mut self, delta: i64, cx: &mut Context<Self>) {
        if self.matches.is_empty() {
            return;
        }
        self.selected = (self.selected as i64 + delta).clamp(0, self.matches.len() as i64 - 1) as usize;
        self.scroll.scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn choose(&mut self, tab: FileTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_selected(tab, cx) {
            window.close_dialog(cx);
        }
    }

    /// Opens the selected file; `false` if nothing matches.
    fn open_selected(&mut self, tab: FileTab, cx: &mut App) -> bool {
        let Some(&index) = self.matches.get(self.selected) else { return false };
        file_view::open(self.repo.clone(), self.files[index].clone(), None, tab, cx);
        true
    }
}

impl Render for FilePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, active) = (theme.muted_foreground, theme.list_active);
        let items: Vec<AnyElement> = self
            .matches
            .iter()
            .enumerate()
            .map(|(row, &i)| {
                let path = &self.files[i];
                let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
                div()
                    .id(("file-match", row))
                    .test_support()
                    .h(rems(1.5))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(row == self.selected, |d| d.bg(active))
                    .child(div().whitespace_nowrap().child(name.to_string()))
                    .child(div().flex_1().min_w_0().truncate().text_color(muted).child(dir.to_string()))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        this.selected = row;
                        if event.click_count() >= 2 {
                            this.choose(FileTab::Annotate, window, cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        div()
            .id("file-picker")
            .test_support()
            // the input would take the arrows for itself; the list gets them first
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "up" => this.move_selection(-1, cx),
                    "down" => this.move_selection(1, cx),
                    "enter" if event.keystroke.modifiers.control => this.choose(FileTab::History, window, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .flex()
            .flex_col()
            .gap_2()
            .child(Input::new(&self.input).id("file-query"))
            .child(div().id("file-matches").h(rems(23.75)).overflow_y_scroll().children(items))
            .child(div().text_xs().text_color(muted).child("Enter: Annotate · Ctrl+Enter: History"))
    }
}

/// Indices of the best `limit` matches of `query` in `files`, best first.
pub fn rank(files: &[String], query: &str, limit: usize) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..files.len().min(limit)).collect();
    }
    let mut scored: Vec<(i64, usize)> =
        files.iter().enumerate().filter_map(|(i, f)| score(f, &query).map(|s| (s, i))).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(files[a.1].len().cmp(&files[b.1].len())).then(a.1.cmp(&b.1)));
    scored.truncate(limit);
    scored.into_iter().map(|(_, i)| i).collect()
}

/// Fuzzy score: the query's characters in order, preferring matches in the
/// file name, at word starts and in runs; `None` if it does not match.
fn score(path: &str, query: &str) -> Option<i64> {
    let lower = path.to_lowercase();
    let name_start = lower.rfind('/').map_or(0, |i| i + 1);
    // with a slash the query addresses the path, otherwise try the name first
    let target_start = if query.contains('/') { 0 } else { name_start };
    subsequence_score(&lower, target_start, query)
        .map(|s| s + 1000)
        .or_else(|| (target_start > 0).then(|| subsequence_score(&lower, 0, query)).flatten())
}

fn subsequence_score(text: &str, start: usize, query: &str) -> Option<i64> {
    let chars: Vec<char> = text[start..].chars().collect();
    let mut score = 0i64;
    let mut pos = 0usize;
    let mut previous: Option<usize> = None;
    for q in query.chars() {
        let found = (pos..chars.len()).find(|&i| chars[i] == q)?;
        let word_start = found == 0 || matches!(chars[found - 1], '/' | '_' | '-' | '.' | ' ');
        score += 10;
        if word_start {
            score += 15;
        }
        if previous.is_some_and(|p| p + 1 == found) {
            score += 20;
        }
        score -= (found - pos) as i64;
        previous = Some(found);
        pos = found + 1;
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    // not `super::*`: gpui's `test` attribute would shadow the standard one
    use super::rank;

    #[test]
    fn ranks_file_names_first() {
        let files: Vec<String> = ["src/log_view.rs", "src/lib.rs", "crates/log/mod.rs", "README.md", "src/view/log.rs"]
            .into_iter()
            .map(String::from)
            .collect();
        let names = |q: &str| rank(&files, q, 10).into_iter().map(|i| files[i].as_str()).collect::<Vec<_>>();
        assert_eq!(names("logv")[0], "src/log_view.rs");
        assert_eq!(names("lib")[0], "src/lib.rs");
        assert_eq!(names("view/log")[0], "src/view/log.rs");
        assert!(names("zzz").is_empty());
        assert_eq!(names("").len(), 5);
    }
}
