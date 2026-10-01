//! rsit UI: the log window and its dialogs. The binary in `main.rs` only parses
//! arguments and opens the window, so UI tests can drive the same views.

pub mod blame_view;
pub mod commit_menu;
pub mod commit_panel;
pub mod diff_model;
pub mod diff_view;
pub mod file_picker;
pub mod file_view;
pub mod git_actions;
pub mod graph_paint;
pub mod history_view;
pub mod log_view;
pub mod merge_view;
pub mod navigator;
pub mod rebase_view;
pub mod selection;
pub mod settings;
pub mod tasks;
pub mod text;
pub mod welcome;
pub mod workspace;

/// Initializes gpui-kit and rsit key bindings.
pub fn init(cx: &mut gpui_kit::App) {
    gpui_kit::init(cx);
    settings::init(cx);
    log_view::init(cx);
    diff_view::init(cx);
    commit_panel::init(cx);
    workspace::init(cx);
    history_view::init(cx);
    file_view::init(cx);
    rebase_view::init(cx);
    merge_view::init(cx);
    blame_view::init(cx);
    welcome::init(cx);
}

gpui_kit::assets::icon_assets!(ExtraIcons, [GitBranch, ArrowDownToLine, ArrowUpFromLine, Settings]);

/// gpui-kit's component icons plus the extra Lucide icons rsit uses.
pub struct AppAssets;

impl gpui_kit::AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        // the default bundle errors on unknown paths, so ask the extras first
        match ExtraIcons.load(path)? {
            Some(data) => Ok(Some(data)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<gpui_kit::SharedString>> {
        let mut all = gpui_kit::assets::Assets.list(path)?;
        all.extend(ExtraIcons.list(path)?);
        Ok(all)
    }
}
