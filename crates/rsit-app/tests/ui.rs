//! UI workflows on a real repository in a headless window.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, Bounds, TestAppContext, WindowBounds, WindowOptions, point, px, size};
use rsit_app::log_view::LogView;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// main: c1 - c2 - c3, feature: c2 - f1
fn demo_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    for i in 1..=3 {
        std::fs::write(p.join(format!("f{i}")), i.to_string()).unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-qm", &format!("c{i}")]);
        if i == 2 {
            git(p, &["branch", "feature"]);
        }
    }
    git(p, &["checkout", "-q", "feature"]);
    std::fs::write(p.join("fx"), "x").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "f1"]);
    git(p, &["checkout", "-q", "main"]);
    dir
}

fn open(cx: &mut TestAppContext, repo: &Path) -> gpui_kit::AnyWindowHandle {
    open_view(cx, repo).0
}

/// Keeps graph caches of the temporary test repositories out of the user's cache.
fn isolate_cache() {
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let dir = DIR.get_or_init(|| tempfile::tempdir().unwrap());
    // SAFETY: every test sets the same value before any repository is loaded
    unsafe { std::env::set_var("XDG_CACHE_HOME", dir.path()) };
}

fn open_view(cx: &mut TestAppContext, repo: &Path) -> (gpui_kit::AnyWindowHandle, gpui_kit::Entity<LogView>) {
    isolate_cache();
    cx.update(rsit_app::init);
    let repo = rsit_git::Repo::discover(repo).unwrap();
    cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.), px(0.)), size: size(px(1200.), px(800.)) })),
            ..Default::default()
        };
        let (window, view) = gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| LogView::with_options(repo, Default::default(), false, window, cx))
        })
        .unwrap();
        (window, view)
    })
}

async fn wait_rows(cx: &mut TestAppContext, window: gpui_kit::AnyWindowHandle, rows: usize) {
    cx.wait_for(window, Duration::from_secs(10), move |window, _| {
        window.try_find(("row", rows - 1)).is_some() && window.try_find(("row", rows)).is_none()
    })
    .await;
}

#[gpui_kit::test]
async fn shows_all_commits(cx: &mut TestAppContext) {
    let repo = demo_repo();
    let window = open(cx, repo.path());
    wait_rows(cx, window, 4).await;
}

#[gpui_kit::test]
async fn context_menu_creates_tag(cx: &mut TestAppContext) {
    let repo = demo_repo();
    let window = open(cx, repo.path());
    wait_rows(cx, window, 4).await;
    cx.update_window(window, |_, window, cx| {
        window.right_click(("row", 1usize), cx);
    })
    .unwrap();
    cx.wait_for(window, Duration::from_secs(2), |window, _| window.try_find("popup-menu").is_some()).await;
    cx.update_window(window, |_, window, cx| {
        let menu = window.within("popup-menu");
        let labels: Vec<String> = (0..8usize)
            .map(|i| menu.try_find(i).and_then(|item| item.label().map(str::to_string)).unwrap_or_default())
            .collect();
        let tag = labels.iter().position(|l| l == "New Tag…").unwrap_or_else(|| panic!("menu: {labels:?}"));
        window.within("popup-menu").click(tag, cx);
    })
    .unwrap();
    cx.wait_for(window, Duration::from_secs(2), |window, _| window.try_find("popup-menu").is_none()).await;
    cx.update_window(window, |_, window, cx| {
        use gpui_kit::component::WindowExt as _;
        assert!(window.has_active_dialog(cx), "the New Tag dialog is open");
        assert_eq!(window.find("ref-name").focused(), Some(true), "name field is focused");
        window.input("v-test", cx);
        window.press("enter", cx);
        assert!(!window.has_active_dialog(cx), "Enter confirms the dialog");
    })
    .unwrap();
    let dir = repo.path().to_path_buf();
    cx.run_until_parked();
    let tagged = git(&dir, &["rev-list", "-n1", "v-test"]);
    let data = rsit_log::LogData::load(rsit_git::Repo::discover(&dir).unwrap(), None).unwrap();
    assert_eq!(tagged.trim(), data.id(1).to_string(), "tag is on the right-clicked row");
}

/// main: c0 ... c40; feature: c0 - f1 (newest), so f1 -> c0 is a long edge
/// drawn with an arrow right below f1.
#[gpui_kit::test]
async fn arrow_click_jumps_to_far_end(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let commit = |msg: &str, t: u32| {
        let date = format!("@{} +0000", 1_700_000_000 + t * 60);
        let out = Command::new("git")
            .current_dir(p)
            .args(["commit", "-q", "--allow-empty", "-m", msg])
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@e")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@e")
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .output()
            .unwrap();
        assert!(out.status.success());
    };
    git(p, &["init", "-q", "-b", "main"]);
    commit("c0", 0);
    git(p, &["branch", "feature"]);
    for i in 1..=40 {
        commit(&format!("c{i}"), i);
    }
    git(p, &["checkout", "-q", "feature"]);
    commit("f1", 100);
    git(p, &["checkout", "-q", "main"]);
    let c0 = git(p, &["rev-parse", "main~40"]).trim().to_string();

    let (window, view) = open_view(cx, p);
    cx.wait_for(window, Duration::from_secs(10), |window, _| window.try_find(("graph", 1usize)).is_some()).await;
    // row 1 is c40; the f1 -> c0 edge passes it in lane 1 with a down arrow
    cx.update_window(window, |_, window, cx| {
        window.click_at(("graph", 1usize), point(px(24.), px(17.)), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let selected = cx.update(|cx| view.read(cx).selected_id()).unwrap();
    assert_eq!(selected.to_string(), c0);
}
