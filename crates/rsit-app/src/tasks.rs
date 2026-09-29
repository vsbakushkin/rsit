//! Background git tasks with progress in the status bar and a notification
//! when they finish (IntelliJ background tasks + VCS notifications).

use std::path::PathBuf;

use futures::{FutureExt as _, StreamExt as _};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::*;

/// What is running right now, shown in the status bar.
#[derive(Default)]
pub struct Activity {
    pub tasks: Vec<RunningTask>,
    next_id: usize,
}

pub struct RunningTask {
    id: usize,
    pub label: SharedString,
    /// The last progress line git printed.
    pub progress: SharedString,
}

impl Global for Activity {}

impl Activity {
    /// Status bar text: the newest task with its progress.
    pub fn summary(&self) -> Option<SharedString> {
        let task = self.tasks.last()?;
        Some(if task.progress.is_empty() {
            format!("{}…", task.label).into()
        } else {
            format!("{}: {}", task.label, task.progress).into()
        })
    }
}

/// Mutates the activity so that observers (the status bar) re-render.
fn update_activity<R>(cx: &mut App, f: impl FnOnce(&mut Activity) -> R) -> R {
    cx.default_global::<Activity>();
    cx.update_global::<Activity, _>(|activity, _| f(activity))
}

pub type Op = Box<dyn FnOnce(&std::path::Path, &mut dyn FnMut(&str)) -> anyhow::Result<String> + Send>;

/// Runs `op` in the background. `success` is the notification text on success
/// (`None` stays silent); errors always notify and stay until dismissed.
pub fn run_git_task(
    label: impl Into<SharedString>,
    cwd: PathBuf,
    success: Option<String>,
    op: Op,
    window: &mut Window,
    cx: &mut App,
) {
    let label = label.into();
    let id = update_activity(cx, |activity| {
        activity.next_id += 1;
        let id = activity.next_id;
        activity.tasks.push(RunningTask { id, label: label.clone(), progress: SharedString::default() });
        id
    });
    let (tx, mut lines) = futures::channel::mpsc::unbounded::<String>();
    let task = cx.background_spawn(async move {
        let mut report = |line: &str| {
            tx.unbounded_send(line.to_string()).ok();
        };
        op(&cwd, &mut report)
    });
    window
        .spawn(cx, async move |cx| {
            let mut task = task.fuse();
            let result = loop {
                futures::select! {
                    line = lines.next() => {
                        if let Some(line) = line {
                            cx.update(|_, cx| {
                                update_activity(cx, |activity| {
                                    if let Some(t) = activity.tasks.iter_mut().find(|t| t.id == id) {
                                        t.progress = line.into();
                                    }
                                })
                            })
                            .ok();
                        }
                    }
                    result = task => break result,
                }
            };
            cx.update(|window, cx| {
                update_activity(cx, |activity| activity.tasks.retain(|t| t.id != id));
                match result {
                    Ok(_) => {
                        if let Some(message) = success {
                            window.push_notification(Notification::success(message), cx);
                        }
                    }
                    Err(e) => window.push_notification(
                        Notification::error(format!("{label} failed: {e:#}")).autohide(false),
                        cx,
                    ),
                }
            })
            .ok();
        })
        .detach();
}
