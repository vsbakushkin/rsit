//! The Welcome window shown when rsit starts outside a repository: recent
//! repositories and a folder picker (IntelliJ's Welcome screen).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rsit_git::Repo;
use rsit_git::wsl::WslPath;
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

/// Recently opened repositories, newest first, in `recent.json` in [`rsit_git::dirs::state_dir`].
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

/// Opens the most recently opened repository, or the Welcome window when there
/// is none (see [`open_dir`]).
pub fn open_last(filter: LogFilter, cx: &mut App) {
    match Recent::load().into_iter().next() {
        Some(dir) => open_dir(dir, filter, cx),
        None => open(filter, cx),
    }
}

/// Opens the repository at `dir`, or the Welcome window when it is gone. One
/// inside a stopped WSL distribution opens from the Welcome window once the
/// distribution has started, so a window shows at once.
pub fn open_dir(dir: PathBuf, filter: LogFilter, cx: &mut App) {
    if let Some(wsl) = stopped_wsl(&dir) {
        return open_with(filter, cx, move |welcome, cx| welcome.open_in_background(dir, &wsl.distro, cx));
    }
    match Repo::discover(&dir) {
        Ok(repo) => crate::workspace::open(repo, filter, true, cx),
        Err(_) => open(filter, cx),
    }
}

/// Whether opening `dir` first has to start its WSL distribution.
pub fn waits_for_wsl(dir: &Path) -> bool {
    stopped_wsl(dir).is_some()
}

/// The WSL distribution `dir` is in, if it is not running.
fn stopped_wsl(dir: &Path) -> Option<WslPath> {
    WslPath::parse(dir).filter(|wsl| !wsl.is_running())
}

/// Recent directories that no longer exist. Ones inside a stopped WSL
/// distribution are not checked: that would start it and take ~20 s.
fn missing_dirs(dirs: &[PathBuf]) -> HashSet<PathBuf> {
    let mut running = HashMap::new();
    dirs.iter()
        .filter(|dir| match WslPath::parse(dir) {
            Some(wsl) => *running.entry(wsl.distro.clone()).or_insert_with(|| wsl.is_running()),
            None => true,
        })
        .filter(|dir| !dir.is_dir())
        .cloned()
        .collect()
}

/// Shows the system folder picker; resolves to the chosen folder, or `None` if cancelled.
pub fn pick_folder(cx: &mut App) -> Task<anyhow::Result<Option<PathBuf>>> {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Open".into()),
    });
    cx.spawn(async move |_| match paths.await {
        Ok(Ok(Some(paths))) => Ok(paths.into_iter().next()),
        Ok(Ok(None)) | Err(_) => Ok(None),
        Ok(Err(e)) => Err(e.context("cannot show the folder picker")),
    })
}

fn recent_path() -> Option<PathBuf> {
    Some(rsit_git::dirs::state_dir()?.join("recent.json"))
}

pub struct Welcome {
    filter: LogFilter,
    recent: Vec<PathBuf>,
    selected: usize,
    /// Whether the opened main window watches the file system (off in tests).
    watch: bool,
    focus: FocusHandle,
    error: Option<SharedString>,
    /// What a repository being opened in the background waits for.
    status: Option<SharedString>,
    /// Recent directories found gone, checked off the UI thread.
    missing: HashSet<PathBuf>,
    _pick: Option<Task<()>>,
    _open: Option<Task<()>>,
    _missing: Option<Task<()>>,
}

/// Opens the Welcome window; the app quits when it closes without opening a repository.
pub fn open(filter: LogFilter, cx: &mut App) {
    open_with(filter, cx, |_, _| {});
}

fn open_with(filter: LogFilter, cx: &mut App, init: impl FnOnce(&mut Welcome, &mut Context<Welcome>) + 'static) {
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some("Welcome to rsit".into()), ..Default::default() }),
        window_bounds: Some(WindowBounds::centered(size(px(720.), px(480.)), cx)),
        app_id: Some("rsit".into()),
        ..Default::default()
    };
    let result = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| {
            let mut welcome = Welcome::new(filter, Recent::load(), window, cx);
            init(&mut welcome, cx);
            welcome
        })
    });
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
        let dirs = recent.clone();
        let check = cx.background_executor().spawn(async move { missing_dirs(&dirs) });
        let missing = cx.spawn(async move |this, cx| {
            let missing = check.await;
            this.update(cx, |this, cx| {
                this.missing = missing;
                cx.notify();
            })
            .ok();
        });
        Self {
            filter,
            recent,
            selected: 0,
            watch: true,
            focus,
            error: None,
            status: None,
            missing: HashSet::new(),
            _pick: None,
            _open: None,
            _missing: Some(missing),
        }
    }

    /// Opened repositories are not watched for changes (for tests).
    pub fn without_watching(mut self) -> Self {
        self.watch = false;
        self
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
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
        let picked = pick_folder(cx);
        self._pick = Some(cx.spawn(async move |this, cx| {
            let picked = picked.await;
            this.update_in(cx, |this, window, cx| match picked {
                Ok(Some(dir)) => this.open_repo(&dir, window, cx),
                Ok(None) => {}
                Err(e) => {
                    this.error = Some(format!("{e:#}").into());
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Opens the repository at `dir` in the main window and closes this one.
    pub fn open_repo(&mut self, dir: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(wsl) = stopped_wsl(dir) {
            return self.open_in_background(dir.to_path_buf(), &wsl.distro, cx);
        }
        self.opened(Repo::discover(dir), window, cx);
    }

    /// Opens a repository inside a stopped WSL distribution, showing a status
    /// while the distribution starts.
    fn open_in_background(&mut self, dir: PathBuf, distro: &str, cx: &mut Context<Self>) {
        let name = dir.file_name().map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().into());
        self.error = None;
        self.status = Some(format!("Starting WSL ({distro}) to open {name}…").into());
        cx.notify();
        let discover = cx.background_executor().spawn(async move { Repo::discover(&dir) });
        self._open = Some(cx.spawn(async move |this, cx| {
            let result = discover.await;
            this.update_in(cx, |this, window, cx| {
                this.status = None;
                this.opened(result, window, cx);
            })
            .ok();
        }));
    }

    fn opened(&mut self, result: anyhow::Result<Repo>, window: &mut Window, cx: &mut Context<Self>) {
        match result {
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
                let missing = self.missing.contains(dir);
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
            .children(self.status.clone().map(|s| div().id("welcome-status").text_sm().text_color(muted).child(s)))
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

#[cfg(test)]
mod tests {
    // not `super::*`: gpui's `test` attribute would shadow the standard one
    use std::collections::HashSet;

    use super::missing_dirs;

    #[test]
    fn missing_dirs_lists_gone_directories() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone");
        let missing = missing_dirs(&[dir.path().to_path_buf(), gone.clone()]);
        assert_eq!(missing, HashSet::from([gone]));
    }
}
