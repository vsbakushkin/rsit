//! Interactive rebase dialog (IntelliJ `GitInteractiveRebaseDialog`): commits
//! in todo order (oldest first) with an action each, reordering, and a message
//! editor for reworded or squashed commits.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rsit_git::Repo;
use rsit_git::rebase::{Action, RebasePlan};

const TABLE: &str = "RebaseTable";
const ROW_HEIGHT: f32 = 26.0;

actions!(
    rebase,
    [SetPick, SetEdit, SetReword, SetSquash, SetFixup, SetDrop, MoveUp, MoveDown, SelectUp, SelectDown, StartRebase]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("p", SetPick, Some(TABLE)),
        KeyBinding::new("e", SetEdit, Some(TABLE)),
        KeyBinding::new("r", SetReword, Some(TABLE)),
        KeyBinding::new("s", SetSquash, Some(TABLE)),
        KeyBinding::new("f", SetFixup, Some(TABLE)),
        KeyBinding::new("d", SetDrop, Some(TABLE)),
        KeyBinding::new("delete", SetDrop, Some(TABLE)),
        KeyBinding::new("alt-up", MoveUp, Some(TABLE)),
        KeyBinding::new("alt-down", MoveDown, Some(TABLE)),
        KeyBinding::new("up", SelectUp, Some(TABLE)),
        KeyBinding::new("down", SelectDown, Some(TABLE)),
        KeyBinding::new("ctrl-enter", StartRebase, Some("RebaseView")),
    ]);
}

pub struct RebaseView {
    repo: Repo,
    plan: RebasePlan,
    original: RebasePlan,
    selected: usize,
    message: Entity<TextareaState>,
    /// Entry whose message is in the editor.
    editing: Option<usize>,
    table_focus: FocusHandle,
    running: bool,
    _subscriptions: Vec<Subscription>,
}

/// Opens the dialog for commits from `plan`.
pub fn open(repo: Repo, plan: RebasePlan, cx: &mut App) {
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some("Interactive Rebase".into()), ..Default::default() }),
        window_bounds: Some(WindowBounds::centered(size(px(1100.), px(640.)), cx)),
        app_id: Some("rsit".into()),
        ..Default::default()
    };
    if let Err(e) =
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| RebaseView::new(repo, plan, window, cx)))
    {
        eprintln!("rsit: cannot open rebase window: {e:#}");
    }
}

impl RebaseView {
    pub fn new(repo: Repo, plan: RebasePlan, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| TextareaState::new(window, cx));
        let subscriptions = vec![cx.subscribe(&message, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.message_edited(cx);
            }
        })];
        let table_focus = cx.focus_handle();
        window.focus(&table_focus, cx);
        let mut this = Self {
            repo,
            original: plan.clone(),
            plan,
            selected: 0,
            message,
            editing: None,
            table_focus,
            running: false,
            _subscriptions: subscriptions,
        };
        this.sync_editor(window, cx);
        this
    }

    pub fn plan(&self) -> &RebasePlan {
        &self.plan
    }

    /// Whether the message of entry `i` is edited here: rewording, or melding others into it.
    fn message_editable(&self, i: usize) -> bool {
        let e = &self.plan.entries[i];
        match e.action {
            Action::Reword => true,
            Action::Drop | Action::Squash | Action::Fixup => false,
            _ => self.plan.entries[i + 1..]
                .iter()
                .take_while(|n| matches!(n.action, Action::Squash | Action::Fixup | Action::Drop))
                .any(|n| n.action == Action::Squash),
        }
    }

    fn editor_text(&self, i: usize) -> String {
        let e = &self.plan.entries[i];
        match &e.new_message {
            Some(m) => m.clone(),
            None if e.action == Action::Reword => e.message.clone(),
            None => self.plan.combined_message(i),
        }
    }

    /// Loads the selected entry's message into the editor.
    fn sync_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.message_editable(self.selected).then_some(self.selected);
        self.editing = None; // ignore the change event of set_value
        let text = match target {
            Some(i) => self.editor_text(i),
            None => self.plan.entries[self.selected].message.clone(),
        };
        self.message.update(cx, |m, cx| m.set_value(text, window, cx));
        self.editing = target;
        cx.notify();
    }

    fn message_edited(&mut self, cx: &mut Context<Self>) {
        let Some(i) = self.editing else { return };
        let text = self.message.read(cx).value().to_string();
        let entry = &mut self.plan.entries[i];
        // an unchanged default stays unset so git keeps its own message
        let default =
            if entry.action == Action::Reword { entry.message.clone() } else { self.plan.combined_message(i) };
        let entry = &mut self.plan.entries[i];
        entry.new_message = (text.trim_end() != default.trim_end()).then_some(text);
        cx.notify();
    }

    pub fn set_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        let i = self.selected;
        let entry = &mut self.plan.entries[i];
        entry.action = action;
        if matches!(action, Action::Pick | Action::Drop | Action::Squash | Action::Fixup | Action::Edit) {
            entry.new_message = None;
        }
        // IntelliJ moves the selection on, so a run of commits is marked quickly
        if matches!(action, Action::Squash | Action::Fixup | Action::Drop | Action::Pick)
            && i + 1 < self.plan.entries.len()
        {
            self.selected = i + 1;
        }
        self.sync_editor(window, cx);
    }

    fn move_entry(&mut self, delta: i64, window: &mut Window, cx: &mut Context<Self>) {
        let i = self.selected as i64;
        let j = i + delta;
        if j < 0 || j >= self.plan.entries.len() as i64 {
            return;
        }
        self.plan.entries.swap(i as usize, j as usize);
        self.selected = j as usize;
        self.sync_editor(window, cx);
    }

    fn select(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if i < self.plan.entries.len() {
            self.selected = i;
            self.sync_editor(window, cx);
        }
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.plan = self.original.clone();
        self.selected = 0;
        self.sync_editor(window, cx);
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running || self.plan.validate().is_some() {
            return;
        }
        let plan = self.plan.clone();
        let cwd = self.repo.cwd().to_path_buf();
        let count = plan.entries.len();
        let op: crate::tasks::Op = Box::new(move |cwd, _| {
            rsit_git::rebase::run_interactive(cwd, &plan)?;
            let repo = rsit_git::Repo::discover(cwd)?;
            Ok(match rsit_git::ops::operation_in_progress(&repo) {
                Some(_) => "Rebase stopped: continue or abort it in the Commit panel".to_string(),
                None => format!("Rebased {count} commit{}", if count == 1 { "" } else { "s" }),
            })
        });
        // an empty success message shows what the operation returned
        let success = Some(String::new());
        match cx.try_global::<crate::navigator::Navigator>().map(|n| n.window()) {
            // report in the main window; this dialog is done
            Some(main) => {
                main.update(cx, |_, window, cx| crate::tasks::run_git_task("Rebasing", cwd, success, op, window, cx))
                    .ok();
                window.remove_window();
            }
            // standalone (`rsit --rebase`): stay open until git is done, or the app would quit
            None => {
                self.running = true;
                cx.notify();
                crate::tasks::run_git_task_then(
                    "Rebasing",
                    cwd,
                    success,
                    op,
                    |window, _| window.remove_window(),
                    window,
                    cx,
                );
            }
        }
    }

    fn render_row(&self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, active, hover, mono) =
            (theme.muted_foreground, theme.list_active, theme.list_hover, theme.mono_font_family.clone());
        let e = &self.plan.entries[i];
        let color: Hsla = match e.action {
            Action::Pick => muted,
            Action::Reword => rgb(0x3574f0).into(),
            Action::Edit => rgb(0xd9822b).into(),
            Action::Squash | Action::Fixup => rgb(0x9b59b6).into(),
            Action::Drop => rgb(0x9a9a9a).into(),
        };
        let melded = matches!(e.action, Action::Squash | Action::Fixup);
        let dropped = e.action == Action::Drop;
        let changed_message = e.new_message.is_some();
        div()
            .id(("rebase-row", i))
            .test_support()
            .h(rems(ROW_HEIGHT / 16.))
            .px_2()
            .flex()
            .items_center()
            .gap_3()
            .when(i == self.selected, |d| d.bg(active))
            .when(i != self.selected, |d| d.hover(|s| s.bg(hover)))
            .child(div().w(rems(4.)).flex_none().text_color(color).child(e.action.label()))
            .child(
                div()
                    .w(rems(4.75))
                    .flex_none()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .font_family(mono)
                    .text_color(muted)
                    .child(e.commit.to_hex_with_len(8).to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(melded, |d| d.pl_4())
                    .when(dropped, |d| d.line_through().text_color(muted))
                    .when(changed_message, |d| d.italic())
                    .child(if melded { format!("↳ {}", e.subject()) } else { e.subject().to_string() }),
            )
            .child(div().w(rems(8.125)).flex_none().truncate().text_color(muted).child(e.author.clone()))
            .on_click(cx.listener(move |this, _, window, cx| {
                window.focus(&this.table_focus, cx);
                this.select(i, window, cx);
            }))
            .into_any_element()
    }
}

impl Render for RebaseView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        let theme = cx.theme();
        let (bg, fg, border, muted) = (theme.background, theme.foreground, theme.border, theme.muted_foreground);
        let problem = self.plan.validate();
        let rows: Vec<AnyElement> = (0..self.plan.entries.len()).map(|i| self.render_row(i, cx)).collect();
        let action_button = |id: &'static str, label: &'static str, key: &'static str| {
            Button::new(id).small().ghost().label(label).tooltip(key)
        };
        let editing = self.editing.is_some();
        div()
            .key_context("RebaseView")
            .on_action(cx.listener(|this, _: &StartRebase, window, cx| this.start(window, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        action_button("act-pick", "Pick", "P")
                            .on_click(cx.listener(|t, _, w, cx| t.set_action(Action::Pick, w, cx))),
                    )
                    .child(
                        action_button("act-edit", "Edit", "E")
                            .on_click(cx.listener(|t, _, w, cx| t.set_action(Action::Edit, w, cx))),
                    )
                    .child(
                        action_button("act-reword", "Reword", "R")
                            .on_click(cx.listener(|t, _, w, cx| t.set_action(Action::Reword, w, cx))),
                    )
                    .child(
                        action_button("act-squash", "Squash", "S")
                            .on_click(cx.listener(|t, _, w, cx| t.set_action(Action::Squash, w, cx))),
                    )
                    .child(
                        action_button("act-fixup", "Fixup", "F")
                            .on_click(cx.listener(|t, _, w, cx| t.set_action(Action::Fixup, w, cx))),
                    )
                    .child(
                        action_button("act-drop", "Drop", "D / Delete")
                            .on_click(cx.listener(|t, _, w, cx| t.set_action(Action::Drop, w, cx))),
                    )
                    .child(div().w(rems(0.75)))
                    .child(
                        action_button("move-up", "Move Up", "Alt+↑")
                            .on_click(cx.listener(|t, _, w, cx| t.move_entry(-1, w, cx))),
                    )
                    .child(
                        action_button("move-down", "Move Down", "Alt+↓")
                            .on_click(cx.listener(|t, _, w, cx| t.move_entry(1, w, cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        action_button("reset", "Reset", "Restore the original list")
                            .on_click(cx.listener(|t, _, w, cx| t.reset(w, cx))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .id("rebase-table")
                            .test_support()
                            .key_context(TABLE)
                            .track_focus(&self.table_focus)
                            .on_action(cx.listener(|t, _: &SetPick, w, cx| t.set_action(Action::Pick, w, cx)))
                            .on_action(cx.listener(|t, _: &SetEdit, w, cx| t.set_action(Action::Edit, w, cx)))
                            .on_action(cx.listener(|t, _: &SetReword, w, cx| t.set_action(Action::Reword, w, cx)))
                            .on_action(cx.listener(|t, _: &SetSquash, w, cx| t.set_action(Action::Squash, w, cx)))
                            .on_action(cx.listener(|t, _: &SetFixup, w, cx| t.set_action(Action::Fixup, w, cx)))
                            .on_action(cx.listener(|t, _: &SetDrop, w, cx| t.set_action(Action::Drop, w, cx)))
                            .on_action(cx.listener(|t, _: &MoveUp, w, cx| t.move_entry(-1, w, cx)))
                            .on_action(cx.listener(|t, _: &MoveDown, w, cx| t.move_entry(1, w, cx)))
                            .on_action(cx.listener(|t, _: &SelectUp, w, cx| {
                                let i = t.selected.saturating_sub(1);
                                t.select(i, w, cx)
                            }))
                            .on_action(cx.listener(|t, _: &SelectDown, w, cx| {
                                let i = t.selected + 1;
                                t.select(i, w, cx)
                            }))
                            .flex_1()
                            .min_w_0()
                            .overflow_y_scroll()
                            .border_r_1()
                            .border_color(border)
                            .children(rows),
                    )
                    .child(
                        div()
                            .w(rems(26.25))
                            .flex_none()
                            .p_2()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_color(muted).child(if editing {
                                "Commit message (edited here)"
                            } else {
                                "Commit message (Reword or Squash to edit)"
                            }))
                            .child(
                                div()
                                    .id("rebase-message")
                                    .test_support()
                                    .flex_1()
                                    .child(Textarea::new(&self.message).h_full().disabled(!editing)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_2()
                    .border_t_1()
                    .border_color(border)
                    .child(div().flex_1().text_color(rgb(0xc7222d)).children(problem.clone()))
                    .child(
                        Button::new("cancel-rebase")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, _| window.remove_window()),
                    )
                    .child(
                        Button::new("start-rebase")
                            .small()
                            .primary()
                            .label("Start Rebasing")
                            .tooltip("Ctrl+Enter")
                            .loading(self.running)
                            .disabled(problem.is_some() || self.running)
                            .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
                    ),
            )
    }
}
