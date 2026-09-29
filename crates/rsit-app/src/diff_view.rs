//! Read-only unified diff of one file in a commit (MVP; the side-by-side viewer
//! comes with the diff stage of the plan).

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::*;
use rsit_git::{FileChange, ObjectId, Repo};

pub struct DiffView {
    editor: Entity<EditorState>,
    title: SharedString,
    _load: Task<()>,
}

pub fn open(repo: Repo, commit: ObjectId, change: FileChange, cx: &mut App) {
    let title: SharedString = format!("{} @ {}", change.path, commit.to_hex_with_len(8)).into();
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some(title.clone()), ..Default::default() }),
        window_bounds: Some(WindowBounds::centered(size(px(1000.), px(760.)), cx)),
        app_id: Some("rsit".into()),
        ..Default::default()
    };
    let result = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| {
            let editor = cx.new(|cx| EditorState::new(window, cx).language("diff").line_number(false));
            let load = cx.spawn_in(window, async move |this: WeakEntity<DiffView>, cx| {
                let path = change.path.clone();
                let text = cx
                    .background_spawn(async move {
                        rsit_git::cli::show_file_diff(repo.cwd(), &commit.to_string(), &path)
                            .unwrap_or_else(|e| format!("{e:#}"))
                    })
                    .await;
                this.update_in(cx, |this, window, cx| {
                    this.editor.update(cx, |editor, cx| editor.set_value(text, window, cx));
                })
                .ok();
            });
            DiffView { editor, title, _load: load }
        })
    });
    if let Err(e) = result {
        eprintln!("cannot open diff window: {e:#}");
    }
}

impl Render for DiffView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .text_sm()
            .child(div().px_2().py_1().border_b_1().border_color(theme.border).child(self.title.clone()))
            .child(div().flex_1().min_h_0().child(Editor::new(&self.editor).size_full()))
    }
}
