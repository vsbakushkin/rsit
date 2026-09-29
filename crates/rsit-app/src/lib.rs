//! rsit UI: the log window and its dialogs. The binary in `main.rs` only parses
//! arguments and opens the window, so UI tests can drive the same views.

pub mod commit_menu;
pub mod commit_panel;
pub mod diff_model;
pub mod diff_view;
pub mod graph_paint;
pub mod log_view;
pub mod workspace;

/// Initializes gpui-kit and rsit key bindings.
pub fn init(cx: &mut gpui_kit::App) {
    gpui_kit::init(cx);
    log_view::init(cx);
    diff_view::init(cx);
    commit_panel::init(cx);
    workspace::init(cx);
}
