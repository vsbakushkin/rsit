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
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(1200.), px(800.)),
            })),
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
        let labels: Vec<String> = (0..20usize)
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

#[gpui_kit::test]
async fn diff_view_navigates_and_folds(cx: &mut TestAppContext) {
    isolate_cache();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    let long = "x".repeat(300);
    let before: String = (0..40).map(|i| if i == 1 { format!("{long}\n") } else { format!("line {i}\n") }).collect();
    std::fs::write(p.join("a.txt"), &before).unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "base"]);
    let after = before.replace("line 5\n", "line five\n").replace("line 30\n", "line thirty\n");
    std::fs::write(p.join("a.txt"), after).unwrap();
    git(p, &["commit", "-qam", "edit"]);

    cx.update(rsit_app::init);
    let repo = rsit_git::Repo::discover(p).unwrap();
    let local = repo.local();
    let commit = local.rev_parse_single("HEAD").unwrap().detach();
    let parent = rsit_git::first_parent(&local, commit).unwrap();
    let files: Vec<rsit_app::diff_model::DiffItem> = rsit_git::changed_files(&local, commit)
        .unwrap()
        .into_iter()
        .map(|f| rsit_app::diff_model::DiffItem::for_commit(f, parent, commit))
        .collect();
    let (window, view) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(1200.), px(800.)),
            })),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| rsit_app::diff_view::DiffView::new(repo, files, 0, window, cx))
        })
        .unwrap()
    });
    let window: gpui_kit::AnyWindowHandle = window;
    cx.run_until_parked();
    // two changes, opened at the first; unchanged lines folded around them
    assert_eq!(cx.update(|cx| view.read(cx).change_position()), (Some(0), 2));
    let folded_rows = cx.update(|cx| view.read(cx).row_count());
    assert!(folded_rows < 40, "folded to {folded_rows} rows");

    cx.update_window(window, |_, window, cx| window.press("f7", cx)).unwrap();
    assert_eq!(cx.update(|cx| view.read(cx).change_position()), (Some(1), 2));

    cx.update_window(window, |_, window, cx| window.click(("fold", 10usize), cx)).unwrap();
    cx.run_until_parked();
    assert!(cx.update(|cx| view.read(cx).row_count()) > folded_rows, "fold expanded");

    // a horizontal wheel scroll moves both sides, bounded by the widest line
    let scroll = |cx: &mut TestAppContext, dx: f32| {
        cx.update_window(window, |_, window, cx| {
            window.scroll("diff-scroll", gpui_kit::ScrollDelta::Pixels(point(px(dx), px(0.))), cx)
        })
        .unwrap();
    };
    scroll(cx, -50.);
    assert_eq!(cx.update(|cx| view.read(cx).horizontal_offset()), 50.0);
    scroll(cx, 500.);
    assert_eq!(cx.update(|cx| view.read(cx).horizontal_offset()), 0.0);
    scroll(cx, -100_000.);
    assert!(cx.update(|cx| view.read(cx).horizontal_offset()) < 300.0 * 8.0);

    cx.update_window(window, |_, window, cx| window.click("layout", cx)).unwrap();
    cx.run_until_parked();
    // unified: each modified line shows as a deletion plus an insertion
    assert!(cx.update(|cx| view.read(cx).row_count()) > folded_rows + 2);
}

#[gpui_kit::test]
async fn commit_panel_stages_and_commits(cx: &mut TestAppContext) {
    isolate_cache();
    let repo_dir = demo_repo();
    let p = repo_dir.path();
    git(p, &["config", "user.name", "Test"]);
    git(p, &["config", "user.email", "test@example.com"]);
    std::fs::write(p.join("new.txt"), "hello\n").unwrap();
    std::fs::write(p.join("f1"), "changed\n").unwrap();

    cx.update(rsit_app::init);
    let repo = rsit_git::Repo::discover(p).unwrap();
    let (window, panel) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(500.), px(800.)),
            })),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| rsit_app::commit_panel::CommitPanel::new(repo, false, window, cx))
        })
        .unwrap()
    });
    let window: gpui_kit::AnyWindowHandle = window;
    cx.run_until_parked();
    let counts = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let s = panel.read(cx).status().clone();
            (s.staged().count(), s.unstaged().count(), s.untracked().count())
        })
    };
    assert_eq!(counts(cx), (0, 1, 1));

    // "+" on the unversioned file stages it
    cx.update_window(window, |_, window, cx| window.click("act-unversioned:new.txt", cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(counts(cx), (1, 1, 0));

    // type a message and commit with Ctrl+Enter
    cx.update_window(window, |_, window, cx| {
        window.click("commit-message", cx);
        window.input("Add new.txt", cx);
        window.press("ctrl-enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(git(p, &["log", "-1", "--format=%s"]).trim(), "Add new.txt");
    assert_eq!(git(p, &["show", "--name-only", "--format="]).trim(), "new.txt", "only the staged file");
    assert_eq!(counts(cx), (0, 1, 0), "f1 stays modified");
}

#[gpui_kit::test]
async fn diff_stages_a_single_change(cx: &mut TestAppContext) {
    isolate_cache();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    let base: String = (1..=30).map(|i| format!("line {i}\n")).collect();
    std::fs::write(p.join("a.txt"), &base).unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "base"]);
    std::fs::write(p.join("a.txt"), base.replace("line 2\n", "line two\n").replace("line 25\n", "line 25!\n")).unwrap();

    cx.update(rsit_app::init);
    let repo = rsit_git::Repo::discover(p).unwrap();
    let item = rsit_app::diff_model::DiffItem {
        change: rsit_git::FileChange { kind: rsit_git::ChangeKind::Modified, path: "a.txt".into(), old_path: None },
        left: Some(rsit_git::Revision::Index),
        right: Some(rsit_git::Revision::WorkTree),
        left_label: "Staged".into(),
        right_label: "Local".into(),
        action: Some(rsit_app::diff_model::ChangeAction::Stage),
    };
    let (window, view) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(1200.), px(800.)),
            })),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| rsit_app::diff_view::DiffView::new(repo, vec![item], 0, window, cx))
        })
        .unwrap()
    });
    let window: gpui_kit::AnyWindowHandle = window;
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| view.read(cx).change_position()).1, 2);

    // "+" next to the second change
    cx.update_window(window, |_, window, cx| window.click(("change-action", 1usize), cx)).unwrap();
    cx.run_until_parked();
    let staged = git(p, &["diff", "--cached", "-U0"]);
    assert!(staged.contains("+line 25!") && !staged.contains("two"), "{staged}");
    assert_eq!(cx.update(|cx| view.read(cx).change_position()).1, 1, "the diff reloads without the staged change");
}

#[gpui_kit::test]
async fn merge_conflict_banner_aborts(cx: &mut TestAppContext) {
    isolate_cache();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    std::fs::write(p.join("f"), "base\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "base"]);
    git(p, &["checkout", "-qb", "other"]);
    std::fs::write(p.join("f"), "other\n").unwrap();
    git(p, &["commit", "-qam", "other"]);
    git(p, &["checkout", "-q", "main"]);
    std::fs::write(p.join("f"), "main\n").unwrap();
    git(p, &["commit", "-qam", "main"]);
    let merge = Command::new("git").current_dir(p).args(["merge", "other"]).output().unwrap();
    assert!(!merge.status.success(), "conflict expected");

    cx.update(rsit_app::init);
    let repo = rsit_git::Repo::discover(p).unwrap();
    let (window, panel) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(500.), px(700.)),
            })),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| rsit_app::commit_panel::CommitPanel::new(repo, false, window, cx))
        })
        .unwrap()
    });
    let window: gpui_kit::AnyWindowHandle = window;
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| panel.read(cx).status().conflicted().count()), 1);
    cx.update_window(window, |_, window, cx| {
        assert!(window.try_find("operation-banner").is_some());
        window.click("abort-operation", cx);
    })
    .unwrap();
    cx.run_until_parked();
    // the watcher is off in tests: refresh as a window activation would
    cx.update(|cx| panel.update(cx, |panel, cx| panel.refresh(cx)));
    cx.run_until_parked();
    assert!(!p.join(".git/MERGE_HEAD").exists(), "merge aborted");
    cx.update_window(window, |_, window, _| assert!(window.try_find("operation-banner").is_none())).unwrap();
    assert_eq!(std::fs::read_to_string(p.join("f")).unwrap(), "main\n");
}

#[gpui_kit::test]
async fn branches_popup_lists_branches(cx: &mut TestAppContext) {
    isolate_cache();
    let repo_dir = demo_repo();
    cx.update(rsit_app::init);
    let repo = rsit_git::Repo::discover(repo_dir.path()).unwrap();
    let (window, _) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(1400.), px(800.)),
            })),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| rsit_app::workspace::Workspace::new(repo, Default::default(), false, window, cx))
        })
        .unwrap()
    });
    let window: gpui_kit::AnyWindowHandle = window;
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.click("branches", cx)).unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, _| {
        let menu = window.within("popup-menu");
        let labels: Vec<String> =
            (0..20usize).filter_map(|i| menu.try_find(i).and_then(|item| item.label().map(str::to_string))).collect();
        assert!(labels.iter().any(|l| l == "Update Project"), "{labels:?}");
        assert!(labels.iter().any(|l| l == "★ main"), "{labels:?}");
        assert!(labels.iter().any(|l| l == "feature"), "{labels:?}");
    })
    .unwrap();
}

#[gpui_kit::test]
async fn file_view_annotates_and_shows_history(cx: &mut TestAppContext) {
    isolate_cache();
    let repo_dir = demo_repo();
    let p = repo_dir.path();
    std::fs::write(p.join("f1"), "one\n").unwrap();
    git(p, &["commit", "-qam", "set f1"]);
    std::fs::write(p.join("f1"), "one\nlocal\n").unwrap();
    cx.update(rsit_app::init);
    let repo = rsit_git::Repo::discover(p).unwrap();
    let (window, view) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(1200.), px(800.)),
            })),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| {
                rsit_app::file_view::FileView::new(
                    repo,
                    "f1".into(),
                    None,
                    rsit_app::file_view::FileTab::Annotate,
                    window,
                    cx,
                )
            })
        })
        .unwrap()
    });
    let window: gpui_kit::AnyWindowHandle = window;
    cx.run_until_parked();
    let blame = cx.update(|cx| view.read(cx).blame().cloned()).unwrap();
    assert_eq!(cx.update(|cx| blame.read(cx).line_count()), 2);
    assert_eq!(cx.update(|cx| blame.read(cx).commit_of_line(0)).as_deref(), Some("set f1"));
    assert_eq!(
        cx.update(|cx| blame.read(cx).commit_of_line(1)).as_deref(),
        Some("Version of f1 from f1"),
        "uncommitted"
    );

    cx.update_window(window, |_, window, cx| window.click("tab-history", cx)).unwrap();
    cx.run_until_parked();
    let history = cx.update(|cx| view.read(cx).history().cloned()).unwrap();
    let subjects: Vec<String> =
        cx.update(|cx| history.read(cx).revisions().iter().map(|r| r.subject.clone()).collect());
    assert_eq!(subjects, ["set f1", "c1"]);
    cx.update_window(window, |_, window, _| assert!(window.try_find(("revision", 0usize)).is_some())).unwrap();
}

#[gpui_kit::test]
async fn go_to_file_picks_with_keyboard(cx: &mut TestAppContext) {
    isolate_cache();
    let repo_dir = demo_repo();
    cx.update(rsit_app::init);
    let repo = rsit_git::Repo::discover(repo_dir.path()).unwrap();
    let (window, _) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(1400.), px(800.)),
            })),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| rsit_app::workspace::Workspace::new(repo, Default::default(), false, window, cx))
        })
        .unwrap()
    });
    let window: gpui_kit::AnyWindowHandle = window;
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.click("go-to-file", cx)).unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.input("f", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        assert!(window.try_find(("file-match", 1usize)).is_some(), "several matches for 'f'");
        window.press("down", cx);
        window.press("down", cx);
        window.press("up", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let windows_before = cx.update(|cx| cx.windows().len());
    cx.update_window(window, |_, window, cx| window.press("enter", cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), windows_before + 1, "a file window opened");
}

#[gpui_kit::test]
async fn rebase_dialog_squashes_and_rewords(cx: &mut TestAppContext) {
    isolate_cache();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.name", "T"]);
    git(p, &["config", "user.email", "t@e"]);
    for i in 0..4 {
        std::fs::write(p.join(format!("f{i}")), "x\n").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-qm", &format!("c{i}")]);
    }
    cx.update(rsit_app::init);
    let repo = rsit_git::Repo::discover(p).unwrap();
    let base = rsit_git::ObjectId::from_hex(git(p, &["rev-parse", "HEAD~3"]).trim().as_bytes()).unwrap();
    let plan = rsit_git::rebase::RebasePlan::after(p, Some(base)).unwrap(); // c1, c2, c3
    let (window, view) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(1100.), px(640.)),
            })),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| rsit_app::rebase_view::RebaseView::new(repo, plan, window, cx))
        })
        .unwrap()
    });
    let window: gpui_kit::AnyWindowHandle = window;
    cx.run_until_parked();
    // keyboard on the table: row 0 stays, "s" on row 1 squashes c2 into c1, "d" drops c3
    cx.update_window(window, |_, window, cx| {
        window.click(("rebase-row", 1usize), cx);
        window.press("s", cx);
        window.press("d", cx);
    })
    .unwrap();
    let actions: Vec<_> = cx.update(|cx| view.read(cx).plan().entries.iter().map(|e| e.action).collect());
    use rsit_git::rebase::Action;
    assert_eq!(actions, [Action::Pick, Action::Squash, Action::Drop]);

    // the squash target's combined message is editable
    cx.update_window(window, |_, window, cx| {
        window.click(("rebase-row", 0usize), cx);
        window.click("rebase-message", cx);
        window.press("ctrl-a", cx);
        window.input("c1 and c2", cx);
    })
    .unwrap();
    cx.update_window(window, |_, window, cx| window.click("start-rebase", cx)).unwrap();
    cx.run_until_parked();
    let log: Vec<String> = git(p, &["log", "--format=%s"]).lines().map(str::to_string).collect();
    assert_eq!(log, ["c1 and c2", "c0"]);
    assert!(!p.join("f3").exists(), "c3 dropped");
}

#[gpui_kit::test]
async fn merge_window_resolves_and_stages(cx: &mut TestAppContext) {
    isolate_cache();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.name", "T"]);
    git(p, &["config", "user.email", "t@e"]);
    let base: String = (1..=12).map(|i| format!("line {i}\n")).collect();
    std::fs::write(p.join("f"), &base).unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "base"]);
    git(p, &["checkout", "-qb", "feature"]);
    std::fs::write(p.join("f"), base.replace("line 2\n", "line 2 theirs\n").replace("line 10\n", "line 10 theirs\n"))
        .unwrap();
    git(p, &["commit", "-qam", "feature"]);
    git(p, &["checkout", "-q", "main"]);
    std::fs::write(p.join("f"), base.replace("line 6\n", "line 6 ours\n").replace("line 10\n", "line 10 ours\n"))
        .unwrap();
    git(p, &["commit", "-qam", "main"]);
    assert!(!Command::new("git").current_dir(p).args(["merge", "feature"]).output().unwrap().status.success());

    cx.update(rsit_app::init);
    let repo = rsit_git::Repo::discover(p).unwrap();
    let (window, view) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(1500.), px(900.)),
            })),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| rsit_app::merge_view::MergeView::new(repo, "f".into(), window, cx))
        })
        .unwrap()
    });
    let window: gpui_kit::AnyWindowHandle = window;
    cx.run_until_parked();
    let counts = |cx: &mut TestAppContext| {
        cx.update(|cx| view.read(cx).model().map(|m| (m.unresolved(), m.unresolved_conflicts()))).unwrap()
    };
    assert_eq!(counts(cx), (3, 1));

    cx.update_window(window, |_, window, cx| window.click("apply-non-conflicting", cx)).unwrap();
    assert_eq!(counts(cx), (1, 1));
    // the conflict is chunk 2: take ours, then append theirs
    cx.update_window(window, |_, window, cx| {
        window.click(("apply-left", 2usize), cx);
        window.click(("apply-right", 2usize), cx);
    })
    .unwrap();
    assert_eq!(counts(cx), (0, 0));
    cx.update_window(window, |_, window, cx| window.click("apply-merge", cx)).unwrap();
    cx.run_until_parked();

    let merged = std::fs::read_to_string(p.join("f")).unwrap();
    let expected = base
        .replace("line 2\n", "line 2 theirs\n")
        .replace("line 6\n", "line 6 ours\n")
        .replace("line 10\n", "line 10 ours\nline 10 theirs\n");
    assert_eq!(merged, expected);
    assert!(rsit_git::conflicts::conflicted_paths(p).unwrap().is_empty(), "staged as resolved");
}

#[gpui_kit::test]
async fn diff_text_can_be_selected_and_copied(cx: &mut TestAppContext) {
    isolate_cache();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    std::fs::write(p.join("a.txt"), "alpha one\nbeta two\ngamma three\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "base"]);
    std::fs::write(p.join("a.txt"), "alpha one\nbeta 2\ngamma three\n").unwrap();
    git(p, &["commit", "-qam", "edit"]);

    cx.update(rsit_app::init);
    let repo = rsit_git::Repo::discover(p).unwrap();
    let local = repo.local();
    let commit = local.rev_parse_single("HEAD").unwrap().detach();
    let parent = rsit_git::first_parent(&local, commit).unwrap();
    let files: Vec<rsit_app::diff_model::DiffItem> = rsit_git::changed_files(&local, commit)
        .unwrap()
        .into_iter()
        .map(|f| rsit_app::diff_model::DiffItem::for_commit(f, parent, commit))
        .collect();
    let (window, _view) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(1200.), px(800.)),
            })),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| rsit_app::diff_view::DiffView::new(repo, files, 0, window, cx))
        })
        .unwrap()
    });
    let window: gpui_kit::AnyWindowHandle = window;
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        // rows 0..3 are the three lines (nothing folded); drag in the left pane
        // from the start of row 0 to past the end of row 1
        let row0 = window.find(("line", 0usize)).bounds();
        let row1 = window.find(("line", 1usize)).bounds();
        let text_x = row0.origin.x + px(48. + 4. + 1.);
        let pane_end = row1.origin.x + row1.size.width / 2. - px(8.);
        window.drag(point(text_x, row0.center().y), point(pane_end, row1.center().y), cx);
        window.press("ctrl-c", cx);
    })
    .unwrap();
    let copied = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
    assert_eq!(copied, "alpha one\nbeta two");

    // select all copies the whole side that has the selection
    cx.update_window(window, |_, window, cx| {
        window.press("ctrl-a", cx);
        window.press("ctrl-c", cx);
    })
    .unwrap();
    let all = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
    assert_eq!(all, "alpha one\nbeta two\ngamma three\n");
}

#[gpui_kit::test]
async fn annotate_text_can_be_copied(cx: &mut TestAppContext) {
    isolate_cache();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    std::fs::write(p.join("a.txt"), "first line\n\tsecond\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "base"]);
    cx.update(rsit_app::init);
    let repo = rsit_git::Repo::discover(p).unwrap();
    let (window, _) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(1000.), px(600.)),
            })),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| {
                rsit_app::file_view::FileView::new(
                    repo,
                    "a.txt".into(),
                    None,
                    rsit_app::file_view::FileTab::Annotate,
                    window,
                    cx,
                )
            })
        })
        .unwrap()
    });
    let window: gpui_kit::AnyWindowHandle = window;
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        // drag across "second" (after the tab) in the code column, past the annotation (230) and number (48) columns
        let row = window.find(("blame-line", 1usize)).bounds();
        let x = row.origin.x + px(230. + 48. + 8. + 60.);
        window.drag(point(x, row.center().y), point(x + px(400.), row.center().y), cx);
        window.press("ctrl-c", cx);
    })
    .unwrap();
    let dragged = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
    assert!(!dragged.is_empty() && "second".ends_with(&dragged), "tabs map back to the source: {dragged:?}");
    cx.update_window(window, |_, window, cx| {
        window.press("ctrl-a", cx);
        window.press("ctrl-c", cx);
    })
    .unwrap();
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default(), "first line\n\tsecond\n");
}
