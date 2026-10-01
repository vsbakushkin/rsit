//! rsit — a standalone git client modelled on IntelliJ's Git tool window.

use std::path::PathBuf;

use clap::Parser;
use gpui_kit::*;

#[derive(Parser)]
// release builds get their version (`0.1.N`) through RSIT_VERSION
#[command(
    name = "rsit",
    version = option_env!("RSIT_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
    about = "Git log with a commit graph, IntelliJ style"
)]
struct Args {
    /// Path inside the repository. Without it rsit opens the last repository
    /// (or asks for one); --diff, --blame and the like use the current directory.
    path: Option<PathBuf>,
    /// Initial text filter (commit message or hash).
    #[arg(long)]
    text: Option<String>,
    /// Treat --text as an extended regular expression.
    #[arg(long)]
    regex: bool,
    /// Initial user filter (author name or email).
    #[arg(long)]
    user: Option<String>,
    /// Initial branch filter (short ref name, e.g. `main` or `origin/main`).
    #[arg(long)]
    branch: Option<String>,
    /// Initial path filter; may be repeated.
    #[arg(long = "path")]
    paths: Vec<String>,
    /// Open only the diff of this revision (e.g. `HEAD`, a hash or a branch).
    #[arg(long)]
    diff: Option<String>,
    /// Annotate this file (IntelliJ "Annotate with Git Blame").
    #[arg(long, value_name = "FILE")]
    blame: Option<PathBuf>,
    /// Show the history of this file.
    #[arg(long, value_name = "FILE")]
    history: Option<PathBuf>,
    /// Resolve the merge conflict in this file.
    #[arg(long, value_name = "FILE")]
    merge: Option<PathBuf>,
    /// Interactively rebase the commits after BASE (like `git rebase -i BASE`).
    #[arg(long, value_name = "BASE")]
    rebase: Option<String>,
}

actions!(rsit, [Quit]);

fn main() {
    let args = Args::parse();
    let filter = rsit_log::LogFilter {
        text: args.text.unwrap_or_default(),
        regex: args.regex,
        match_case: false,
        user: args.user.unwrap_or_default(),
        branches: args.branch.into_iter().collect(),
        paths: args.paths,
    };
    // without a path (e.g. from the app launcher) open the last repository,
    // or let the user choose one; the views below always need a path
    let views = [&args.diff, &args.rebase].iter().any(|a| a.is_some())
        || [&args.blame, &args.history, &args.merge].iter().any(|a| a.is_some());
    if args.path.is_none() && !views {
        gpui_kit::application().with_assets(rsit_app::AppAssets).run(move |cx| {
            init(cx);
            match rsit_app::welcome::last_repo() {
                Some(repo) => rsit_app::workspace::open(repo, filter, true, cx),
                None => rsit_app::welcome::open(filter, cx),
            }
            cx.activate(true);
        });
        return;
    }
    let path = args.path.unwrap_or_else(|| PathBuf::from("."));
    let repo = match rsit_git::Repo::discover(&path) {
        Ok(repo) => repo,
        Err(e) => {
            eprintln!("rsit: {e:#}");
            std::process::exit(1);
        }
    };

    let diff = match args.diff.as_deref().map(|rev| resolve_diff(&repo, rev)).transpose() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("rsit: {e:#}");
            std::process::exit(1);
        }
    };

    let file = match (&args.blame, &args.history) {
        (Some(f), _) => Some((f.clone(), rsit_app::file_view::FileTab::Annotate)),
        (None, Some(f)) => Some((f.clone(), rsit_app::file_view::FileTab::History)),
        _ => None,
    };
    let file = match file.map(|(f, tab)| repo_relative(&repo, &f).map(|p| (p, tab))).transpose() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("rsit: {e:#}");
            std::process::exit(1);
        }
    };

    let merge = match args.merge.as_deref().map(|f| repo_relative(&repo, f)).transpose() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("rsit: {e:#}");
            std::process::exit(1);
        }
    };
    let rebase = match args.rebase.as_deref().map(|base| resolve_rebase(&repo, base)).transpose() {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!("rsit: {e:#}");
            std::process::exit(1);
        }
    };

    gpui_kit::application().with_assets(rsit_app::AppAssets).run(move |cx| {
        init(cx);
        if let Some(path) = merge {
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            rsit_app::merge_view::open(repo, path, cx);
            cx.activate(true);
            return;
        }
        if let Some(plan) = rebase {
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            rsit_app::rebase_view::open(repo, plan, cx);
            cx.activate(true);
            return;
        }
        if let Some((path, tab)) = file {
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            rsit_app::file_view::open(repo, path, None, tab, cx);
            cx.activate(true);
            return;
        }
        if let Some((commit, files)) = diff {
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            // `--path` picks the file to show first
            let first = filter
                .paths
                .first()
                .and_then(|p| files.iter().position(|f| f.path == *p || f.path.ends_with(p.as_str())))
                .unwrap_or(0);
            rsit_app::diff_view::open(repo, commit, files, first, cx);
            cx.activate(true);
            return;
        }
        rsit_app::workspace::open(repo, filter, true, cx);
        cx.activate(true);
    });
}

fn init(cx: &mut App) {
    rsit_app::init(cx);
    rsit_app::settings::load(cx);
    cx.bind_keys([KeyBinding::new("ctrl-q", Quit, None)]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}

fn resolve_diff(repo: &rsit_git::Repo, rev: &str) -> anyhow::Result<(rsit_git::ObjectId, Vec<rsit_git::FileChange>)> {
    let local = repo.local();
    let commit = local.rev_parse_single(rev)?.object()?.peel_to_commit()?.id;
    let files = rsit_git::changed_files(&local, commit)?;
    Ok((commit, files))
}

/// Path of `file` relative to the repository's working tree.
fn repo_relative(repo: &rsit_git::Repo, file: &std::path::Path) -> anyhow::Result<String> {
    let workdir = repo.workdir().ok_or_else(|| anyhow::anyhow!("bare repository"))?;
    let absolute = std::path::absolute(file)?;
    // the file may be deleted; canonicalize its directory instead
    let absolute = match absolute.canonicalize() {
        Ok(p) => p,
        Err(_) => absolute
            .parent()
            .and_then(|d| d.canonicalize().ok())
            .map(|d| d.join(absolute.file_name().unwrap_or_default()))
            .unwrap_or(absolute),
    };
    let relative =
        absolute.strip_prefix(workdir).map_err(|_| anyhow::anyhow!("{} is outside the repository", file.display()))?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn resolve_rebase(repo: &rsit_git::Repo, base: &str) -> anyhow::Result<rsit_git::rebase::RebasePlan> {
    let base = repo.local().rev_parse_single(base)?.object()?.peel_to_commit()?.id;
    rsit_git::rebase::RebasePlan::after(repo.cwd(), Some(base))
}
