//! Cross-window navigation: other windows (annotate, history, diff) can select
//! a commit in the main window's log.

use gpui_kit::*;
use rsit_git::ObjectId;

use crate::log_view::LogView;

pub struct Navigator {
    window: AnyWindowHandle,
    log: WeakEntity<LogView>,
}

impl Global for Navigator {}

impl Navigator {
    /// The main window.
    pub fn window(&self) -> AnyWindowHandle {
        self.window
    }
}

pub fn register(window: AnyWindowHandle, log: WeakEntity<LogView>, cx: &mut App) {
    cx.set_global(Navigator { window, log });
}

/// Selects `id` in the log of the main window and brings it to front.
/// Returns `false` when there is no main window (e.g. `rsit --blame`).
pub fn select_in_log(id: ObjectId, cx: &mut App) -> bool {
    let Some(nav) = cx.try_global::<Navigator>() else { return false };
    let (handle, log) = (nav.window, nav.log.clone());
    handle
        .update(cx, |_, window, cx| {
            window.activate_window();
            log.update(cx, |log, cx| log.navigate_to(id, window, cx)).is_ok()
        })
        .unwrap_or(false)
}
