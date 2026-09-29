//! rsit — a standalone git client modelled on IntelliJ's Git tool window.

mod diff_view;
mod graph_paint;
mod log_view;

use std::path::PathBuf;

use clap::Parser;
use gpui_kit::*;

#[derive(Parser)]
#[command(version, about = "Git log with a commit graph, IntelliJ style")]
struct Args {
    /// Path inside the repository (defaults to the current directory).
    path: Option<PathBuf>,
}

actions!(rsit, [Quit]);

fn main() {
    let args = Args::parse();
    let path = args.path.unwrap_or_else(|| PathBuf::from("."));
    let repo = match rsit_git::Repo::discover(&path) {
        Ok(repo) => repo,
        Err(e) => {
            eprintln!("rsit: {e:#}");
            std::process::exit(1);
        }
    };

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        log_view::init(cx);
        cx.bind_keys([KeyBinding::new("ctrl-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());

        let title = format!("{} — rsit", repo.display_name());
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions { title: Some(title.into()), ..Default::default() }),
            window_bounds: Some(WindowBounds::centered(size(px(1500.), px(900.)), cx)),
            app_id: Some("rsit".into()),
            ..Default::default()
        };
        let (main_window, _) =
            gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| log_view::LogView::new(repo, window, cx)))
                .expect("failed to open window");
        // closing the log window quits, diff windows are secondary
        cx.on_window_closed(move |cx, closed| {
            if closed == main_window.window_id() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
