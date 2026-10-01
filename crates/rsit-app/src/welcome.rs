//! The Welcome window shown when rsit starts outside a repository: recent
//! repositories and a folder picker (IntelliJ's Welcome screen).

use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rsit_git::Repo;
use rsit_log::LogFilter;

actions!(welcome, [SelectPrevRecent, SelectNextRecent, OpenSelectedRecent, ForgetSelectedRecent]);

const CONTEXT: &str = "Welcome";

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevRecent, Some(CONTEXT)),
        KeyBinding::new("down", SelectNextRecent, Some(CONTEXT)),
        KeyBinding::new("enter", OpenSelectedRecent, Some(CONTEXT)),
        KeyBinding::new("delete", ForgetSelectedRecent, Some(CONTEXT)),
    ]);
}

/// How many repositories the recent list keeps.
const MAX_RECENT: usize = 20;

/// Recently opened repositories, newest first, in `$XDG_STATE_HOME/rsit/recent.json`.
pub struct Recent;

impl Recent {
    pub fn load() -> Vec<PathBuf> {
        let Some(path) = recent_path() else { return Vec::new() };
        std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    /// Moves `dir` to the top of the list.
    pub fn add(dir: &Path) {
        let mut list = Self::load();
        list.retain(|d| d != dir);
        list.insert(0, dir.to_path_buf());
        list.truncate(MAX_RECENT);
        Self::save(&list);
    }

    pub fn remove(dir: &Path) {
        let mut list = Self::load();
        list.retain(|d| d != dir);
        Self::save(&list);
    }

    fn save(list: &[PathBuf]) {
        let Some(path) = recent_path() else { return };
        let text = serde_json::to_string_pretty(list).expect("recent serialize");
        let result = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, text));
        if let Err(e) = result {
            eprintln!("rsit: cannot save {}: {e}", path.display());
        }
    }
}

fn recent_path() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".local").join("state")))?;
    Some(state.join("rsit").join("recent.json"))
}

pub struct Welcome {
    filter: LogFilter,
    recent: Vec<PathBuf>,
    selected: usize,
    /// Whether the opened main window watches the file system (off in tests).
    watch: bool,
    focus: FocusHandle,
    error: Option<SharedString>,
    _pick: Option<Task<()>>,
}

/// Opens the Welcome window; the app quits when it closes without opening a repository.
pub fn open(filter: LogFilter, cx: &mut App) {
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some("Welcome to rsit".into()), ..Default::default() }),
        window_bounds: Some(WindowBounds::centered(size(px(720.), px(480.)), cx)),
        app_id: Some("rsit".into()),
        ..Default::default()
    };
    let result =
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Welcome::new(filter, Recent::load(), window, cx)));
    match result {
        Ok(_) => cx
            .on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach(),
        Err(e) => {
            eprintln!("rsit: cannot open window: {e:#}");
            cx.quit();
        }
    }
}

impl Welcome {
    pub fn new(filter: LogFilter, recent: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self { filter, recent, selected: 0, watch: true, focus, error: None, _pick: None }
    }

    /// Opened repositories are not watched for changes (for tests).
    pub fn without_watching(mut self) -> Self {
        self.watch = false;
        self
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn recent(&self) -> &[PathBuf] {
        &self.recent
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let last = self.recent.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        cx.notify();
    }

    /// Asks for a folder with the system picker and opens the repository in it.
    fn pick(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        self._pick = Some(cx.spawn(async move |this, cx| {
            let picked = match paths.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(e)) => {
                    this.update(cx, |this, cx| {
                        this.error = Some(format!("Cannot show the folder picker: {e:#}").into());
                        cx.notify();
                    })
                    .ok();
                    None
                }
            };
            let Some(dir) = picked else { return };
            this.update_in(cx, |this, window, cx| this.open_repo(&dir, window, cx)).ok();
        }));
    }

    /// Opens the repository at `dir` in the main window and closes this one.
    pub fn open_repo(&mut self, dir: &Path, window: &mut Window, cx: &mut Context<Self>) {
        match Repo::discover(dir) {
            Ok(repo) => {
                crate::workspace::open(repo, self.filter.clone(), self.watch, cx);
                window.remove_window();
            }
            Err(e) => {
                self.error = Some(format!("{e:#}").into());
                cx.notify();
            }
        }
    }

    fn forget(&mut self, dir: &Path, cx: &mut Context<Self>) {
        Recent::remove(dir);
        self.recent.retain(|d| d != dir);
        self.selected = self.selected.min(self.recent.len().saturating_sub(1));
        cx.notify();
    }
}

impl Render for Welcome {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, hover, active, border) =
            (theme.muted_foreground, theme.list_hover, theme.list_active, theme.border);
        let rows: Vec<AnyElement> = self
            .recent
            .iter()
            .enumerate()
            .map(|(i, dir)| {
                let name = dir.file_name().map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().into());
                let missing = !dir.exists();
                let (open_dir, forget_dir) = (dir.clone(), dir.clone());
                div()
                    .id(("recent", i))
                    .px_3()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(theme.radius)
                    .when(i == self.selected, |d| d.bg(active))
                    .when(i != self.selected, |d| d.hover(|s| s.bg(hover)))
                    .cursor_pointer()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().when(missing, |d| d.text_color(muted)).child(name))
                            .child(div().text_sm().text_color(muted).truncate().child(dir.display().to_string())),
                    )
                    .child(Button::new(("forget", i)).xsmall().ghost().label("×").tooltip("Remove from list").on_click(
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.forget(&forget_dir, cx);
                        }),
                    ))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_repo(&open_dir, window, cx)))
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        div()
            .id("welcome")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectPrevRecent, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNextRecent, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &OpenSelectedRecent, window, cx| {
                if let Some(dir) = this.recent.get(this.selected).cloned() {
                    this.open_repo(&dir, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ForgetSelectedRecent, _, cx| {
                if let Some(dir) = this.recent.get(this.selected).cloned() {
                    this.forget(&dir, cx);
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_6()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(div().text_xl().child("rsit"))
                            .child(div().text_sm().text_color(muted).child("Open a Git repository to start")),
                    )
                    .child(
                        Button::new("open-repo")
                            .primary()
                            .label("Open…")
                            .on_click(cx.listener(|this, _, _, cx| this.pick(cx))),
                    ),
            )
            .children(self.error.clone().map(|e| div().text_sm().text_color(theme.danger).child(e)))
            .child(div().text_sm().text_color(muted).child("Recent repositories"))
            .child(
                div()
                    .id("recent-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .border_t_1()
                    .border_color(border)
                    .pt_1()
                    .children(rows)
                    .when(empty, |d| d.child(div().p_3().text_color(muted).child("No recent repositories"))),
            )
    }
}
