//! The Welcome window and the recent repositories list.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Bounds, TestAppContext, WindowBounds, WindowOptions, point, px, size};
use rsit_app::welcome::{Recent, Welcome};

/// Points the recent list and graph caches at a temporary directory; the
/// guard serializes tests, which share that list.
fn isolate() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = DIR.get_or_init(|| tempfile::tempdir().unwrap());
    // SAFETY: every test sets the same values, under the lock
    unsafe {
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        std::env::set_var("XDG_CACHE_HOME", dir.path().join("cache"));
    }
    let _ = std::fs::remove_dir_all(dir.path().join("state"));
    guard
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(dir.path())
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("f"), "1").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "c1"]);
    dir
}

#[test]
fn recent_list_keeps_newest_first() {
    let _guard = isolate();
    let dir = |i: usize| PathBuf::from(format!("/repos/r{i}"));
    for i in 0..25 {
        Recent::add(&dir(i));
    }
    let list = Recent::load();
    assert_eq!(list.len(), 20);
    assert_eq!(list[0], dir(24));
    assert_eq!(list[19], dir(5));

    // reopening moves an entry to the top without duplicating it
    Recent::add(&dir(10));
    let list = Recent::load();
    assert_eq!(list.len(), 20);
    assert_eq!(list[0], dir(10));
    assert_eq!(list.iter().filter(|d| **d == dir(10)).count(), 1);

    Recent::remove(&dir(10));
    assert!(!Recent::load().contains(&dir(10)));
}

fn open_welcome(
    cx: &mut TestAppContext,
    recent: Vec<PathBuf>,
) -> (gpui_kit::AnyWindowHandle, gpui_kit::Entity<Welcome>) {
    cx.update(rsit_app::init);
    cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(720.), px(480.)),
            })),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Welcome::new(Default::default(), recent, window, cx).without_watching())
        })
        .unwrap()
    })
}

#[gpui_kit::test]
fn enter_on_a_folder_outside_git_shows_an_error(cx: &mut TestAppContext) {
    let _guard = isolate();
    let plain = tempfile::tempdir().unwrap();
    let (window, welcome) = open_welcome(cx, vec![plain.path().to_path_buf()]);
    press(cx, window, "enter");
    let error = cx.update(|cx| welcome.read(cx).error().map(str::to_owned));
    assert!(error.is_some_and(|e| e.contains("not a git repository")));
    assert!(cx.update(|cx| cx.windows().len()) == 1, "the welcome window stays open");
}

#[gpui_kit::test]
fn enter_opens_the_selected_repository(cx: &mut TestAppContext) {
    let _guard = isolate();
    let (first, second) = (repo(), repo());
    let recent = vec![first.path().to_path_buf(), second.path().to_path_buf()];
    let (window, welcome) = open_welcome(cx, recent);

    // Delete forgets the first entry, Enter opens the one now selected
    press(cx, window, "delete");
    assert_eq!(cx.update(|cx| welcome.read(cx).recent().len()), 1);
    press(cx, window, "enter");

    let windows = cx.update(|cx| cx.windows());
    assert_eq!(windows.len(), 1, "the welcome window is replaced by the main window");
    assert_ne!(windows[0].window_id(), window.window_id());
    let opened = Recent::load();
    assert_eq!(opened.first().map(PathBuf::as_path), Some(canonical(second.path()).as_path()));
}

fn press(cx: &mut TestAppContext, window: gpui_kit::AnyWindowHandle, key: &str) {
    cx.update_window(window, |_, window, cx| window.press(key, cx)).unwrap();
    cx.run_until_parked();
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}
