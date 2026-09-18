//! The review view: a pull request, its files, and a read-only editor.

pub mod editor;
pub mod gutter;
pub mod hover;
pub mod header;
pub mod model;
pub mod rail;
pub mod styling;
pub mod tabs;
pub mod threads;
pub mod tree;

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Effect, SignalGet, SignalUpdate, SignalWith};
use floem::views::{Decorators, Stack, dyn_container};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::{Preparing, ReviewState};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::centered_message;

/// Opening a file, shared by every list that can request one.
///
/// An `Rc` rather than a plain closure because it threads through several
/// views that each need their own copy, and it captures the backend handle.
pub type OpenFile = Rc<dyn Fn(&str)>;

/// The whole right-hand side while a pull request is open.
pub fn review_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    on_mark: crate::ui::list::OnMark,
) -> impl IntoView {
    let open_file: OpenFile = Rc::new({
        let backend = backend.clone();
        move |path: &str| {
            // Open the path to it in the tree, so a file reached from the
            // changed list or from a definition can be found there too.
            state.expanded.update(|expanded| tree::reveal(expanded, path));
            if state.open_tab(path) {
                backend.open_file(path);
            }
        }
    });

    let body_backend = backend.clone();
    let on_back = {
        let backend = backend.clone();
        move || {
            app.leave_review(state);
            backend.close_review();
        }
    };

    let on_done = {
        let backend = backend.clone();
        move |thread_id: String| {
            on_mark(vec![thread_id], crate::ui::list::MarkAction::Done(true));
            // Dismissing is also leaving: the next notification is the point.
            app.leave_review(state);
            backend.close_review();
        }
    };

    // The backend closes the review itself when the archive goes through,
    // and the Worktrees event that follows closes the view. A refusal
    // leaves everything as it was, with the reason in the status line.
    let on_archive = {
        let backend = backend.clone();
        move |id: String| backend.archive_worktree(&id)
    };

    Stack::vertical((
        header::header(app, state, on_back, on_done, on_archive),
        dyn_container(
            move || match state.preparing.get() {
                Preparing::Working { .. } => Phase::Preparing,
                Preparing::Failed { .. } => Phase::Failed,
                // Idle before the pull request lands still means preparing:
                // the first event has simply not arrived yet.
                Preparing::Idle if !state.is_open() => Phase::Preparing,
                Preparing::Idle => Phase::Ready,
            },
            move |phase| match phase {
                Phase::Preparing => header::preparing_card(
                    move || match state.preparing.get() {
                        Preparing::Working { message, .. } => message,
                        _ => String::new(),
                    },
                    move || match state.preparing.get() {
                        Preparing::Working { percent, .. } => percent,
                        _ => None,
                    },
                )
                .into_any(),
                Phase::Failed => centered_message(
                    SidebarIcon::Warning,
                    || "Could not open this pull request".to_string(),
                    move || match state.preparing.get() {
                        Preparing::Failed { message } => message,
                        _ => String::new(),
                    },
                )
                .into_any(),
                Phase::Ready => {
                    body(app, state, open_file.clone(), body_backend.clone()).into_any()
                }
            },
        )
        .style(|s| s.flex_grow(1.0_f32).min_height(0.0).width_full()),
    ))
    .style(|s| s.size_full().flex_col().min_height(0.0).background(theme::CANVAS))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Preparing,
    Failed,
    Ready,
}

fn body(app: AppState, state: ReviewState, open_file: OpenFile, backend: Rc<Backend>) -> impl IntoView {
    // The column opens itself when a file with threads is opened, and
    // stays where it was put otherwise.
    Effect::new(move |_| {
        if let Some(path) = state.active.get() {
            if !state.threads_in(&path).is_empty() {
                state.threads_open.set(true);
            }
        }
    });

    let column_backend = backend.clone();
    let column_open = open_file.clone();
    Stack::horizontal((
        rail::rail(state, open_file.clone()),
        Stack::vertical((
            tabs::tab_bar(state),
            editor::editor_pane(app, state, backend, open_file),
        ))
            .style(|s| s.flex_grow(1.0_f32).min_width(0.0).height_full().flex_col()),
        dyn_container(
            move || state.threads_open.get(),
            move |open| {
                if open {
                    floem::views::Container::new(Stack::horizontal((
                        threads::resize_handle(state),
                        threads::thread_column(app, state, column_backend.clone(), column_open.clone()),
                    )))
                    .style(|s| {
                        s.height_full()
                            .flex_shrink(0.0_f32)
                            .position(floem::style::Position::Relative)
                    })
                    .into_any()
                } else {
                    floem::views::Empty::new().into_any()
                }
            },
        ),
    ))
    .style(|s| s.size_full().min_height(0.0))
}

/// One line under the review view, replacing the inbox's status bar content.
pub fn review_status(state: ReviewState) -> impl IntoView {
    use floem::views::Label;

    Label::derived(move || {
        let server = state.lsp_status.get();
        let server = if matches!(server, perch_lsp::Status::Absent) {
            String::new()
        } else {
            format!("  \u{00b7}  {}", server.describe())
        };
        let archived = match state.archived.get() {
            Some(0) => "  \u{00b7}  discussion up to date".to_string(),
            Some(1) => "  \u{00b7}  1 message archived".to_string(),
            Some(n) => format!("  \u{00b7}  {n} messages archived"),
            None => String::new(),
        };
        let server = format!("{server}{archived}");

        let Some(file) = state.active_file() else {
            let n = state.changed_files.with(|f| f.len());
            return format!("{n} changed files{server}");
        };
        if file.truncated {
            return format!("{}  \u{00b7}  not shown as text{server}", file.path);
        }
        let language = file.language.map(|l| l.name()).unwrap_or("Plain text");
        let changes = file.change_count();
        let line = state.cursor_line.get() + 1;
        format!(
            "{}  \u{00b7}  {language}  \u{00b7}  {} lines  \u{00b7}  {changes} changes  \u{00b7}  ln {line}{server}",
            file.path,
            file.line_count()
        )
    })
    .style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::FG_MUTED)
            .text_ellipsis()
            .min_width(0.0)
    })
}
