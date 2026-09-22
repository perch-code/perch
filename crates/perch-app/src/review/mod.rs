//! The review view: a pull request, its files, and a read-only editor.

pub mod cards;
pub mod code_refs;
pub mod composer_styling;
pub mod conversation;
pub mod doc;
pub mod editor;
pub mod find;
pub mod go_to;
pub mod gutter;
pub mod header;
pub mod hierarchy;
pub mod hover;
pub mod issue;
pub mod linked_text;
pub mod markdown;
pub mod merge;
pub mod model;
pub mod navigation;
pub mod picture;
pub mod rail;
pub mod references;
pub mod review_launch;
pub mod scope;
pub mod styling;
pub mod submit_review;
pub mod summary;
pub mod switch;
pub mod tabs;
pub mod threads;
pub mod timeline;
pub mod tree;

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{SignalGet, SignalWith};
use floem::views::{dyn_container, Decorators, Stack};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::{ItemTab, Phase, Preparing, ReviewState};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::centered_message;

/// Opening a file, shared by every list that can request one.
///
/// An `Rc` rather than a plain closure because it threads through several
/// views that each need their own copy, and it captures the backend handle.
pub type OpenFile = Rc<dyn Fn(&str)>;

/// Opening a file: its tab, its content from the backend, and its row in
/// the rail's tree. One closure for the item view and the drawer alike.
pub fn open_file(state: ReviewState, backend: Rc<Backend>) -> OpenFile {
    Rc::new(move |path: &str| {
        // A file outside the worktree has no place in the tree, and is
        // read from where it is; the loader's limits still apply.
        if state.open_tab(path) {
            if std::path::Path::new(path).is_absolute() {
                backend.open_outside_file(path);
            } else {
                backend.open_file(path);
            }
        }
        // Show it in the tree, so a file reached from a card or from
        // a definition can be found there too: the way to it opens,
        // the filter comes off when the file is not a change, and the
        // rail scrolls to its row.
        state.show_in_tree(path);
    })
}

/// The main pane while an item is open.
pub fn review_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_item: crate::ui::items::OpenItem,
) -> impl IntoView {
    let open_file = open_file(state, backend.clone());
    let body_backend = backend.clone();

    Stack::vertical((
        header::header(app, state, backend.clone())
            // Keep header-owned popovers above the item surface. Floem does
            // not let a descendant's z-index escape its parent's sibling
            // context.
            .style(|s| s.width_full().flex_shrink(0.0_f32).z_index(1)),
        dyn_container(
            move || state.phase.get(),
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
                Phase::Issue | Phase::Ready => item_view(
                    app,
                    state,
                    open_file.clone(),
                    body_backend.clone(),
                    open_item.clone(),
                )
                .into_any(),
            },
        )
        .style(|s| s.flex_grow(1.0_f32).min_height(0.0).width_full()),
    ))
    .style(|s| {
        s.size_full()
            .flex_col()
            .min_height(0.0)
            .background(theme::CANVAS)
    })
}

/// The tab the header's switch has picked: Conversation for every item,
/// Files for one with a checkout.
fn item_view(
    app: AppState,
    state: ReviewState,
    open_file: OpenFile,
    backend: Rc<Backend>,
    open_item: crate::ui::items::OpenItem,
) -> impl IntoView {
    // An item without a checkout has no Files to show, whatever the tab
    // remembered for it says.
    let tab = floem::reactive::Memo::new(move |_| {
        let has_files = state
            .item
            .with(|i| i.as_ref().is_some_and(|i| i.checkout.is_some()));
        match state.tab.get() {
            ItemTab::Files if has_files => ItemTab::Files,
            _ => ItemTab::Conversation,
        }
    });
    dyn_container(
        move || tab.get(),
        move |tab| match tab {
            ItemTab::Conversation => conversation::view(
                app,
                state,
                backend.clone(),
                open_file.clone(),
                open_item.clone(),
            )
            .into_any(),
            ItemTab::Files => body(app, state, open_file.clone(), backend.clone()).into_any(),
        },
    )
    .style(|s| s.size_full().min_height(0.0))
}

/// The file tabs and the code. Threads anchored in the open file are
/// drawn inline; the complete item conversation lives on the Conversation
/// tab, and the files as a tree in the drawer on the right.
fn body(
    app: AppState,
    state: ReviewState,
    open_file: OpenFile,
    backend: Rc<Backend>,
) -> impl IntoView {
    Stack::vertical((
        tabs::tab_bar(app, state, backend.clone()),
        editor::editor_pane(app, state, backend, open_file),
    ))
    .style(|s| s.size_full().min_height(0.0).flex_col())
}

/// The separator between the status line's parts.
const STATUS_SEPARATOR: &str = "  \u{00b7}  ";

/// The status line's text in three parts, so that the hidden count can be
/// a view of its own: up to the change count; how many lines the open
/// file's folds hide, when they hide any; and the cursor's line, then the
/// language server's state. A line that is not about a file is all
/// `before`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusLine {
    before: String,
    hidden: Option<String>,
    after: String,
}

fn status_line(state: ReviewState, hidden: usize) -> StatusLine {
    let server = state.lsp_status.get();
    let server = if matches!(server, perch_lsp::Status::Absent) {
        String::new()
    } else {
        format!("{STATUS_SEPARATOR}{}", server.describe())
    };
    let whole = |before: String| StatusLine {
        before: format!("{before}{server}"),
        hidden: None,
        after: String::new(),
    };

    if let Some(issue) = state.issue.get() {
        return whole(issue::describe(&issue));
    }
    let Some(file) = state.active_file() else {
        let n = state.changed_files.with(|f| f.len());
        return whole(format!("{n} changed files"));
    };
    // A file outside the worktree is named by its file name and where it
    // is from; its absolute path is nothing the reader wants to read.
    let name = match file.origin.label() {
        Some(label) => {
            let name = file.path.rsplit('/').next().unwrap_or(&file.path);
            format!("{name}{STATUS_SEPARATOR}{label}")
        }
        None => file.path.clone(),
    };
    if file.truncated {
        return whole(format!("{name}{STATUS_SEPARATOR}not shown as text"));
    }
    let changes = if file.origin.is_outside() {
        String::new()
    } else {
        format!("{STATUS_SEPARATOR}{}", counted(file.change_count(), "change"))
    };
    // A picture is described as one: what it is and how big, then its
    // changes, which are the markup's and are counted all the same. No
    // line count and no cursor, since neither is on the screen.
    if let Some(picture) = state.drawn_picture() {
        let size = match picture.current() {
            Some(Ok(drawing)) => picture::dims(drawing.size()),
            _ => "not drawn".to_string(),
        };
        return whole(format!("{name}{STATUS_SEPARATOR}SVG{STATUS_SEPARATOR}{size}{changes}"));
    }
    let language = file.language.map(|l| l.name()).unwrap_or("Plain text");
    let lines = counted(file.line_count(), "line");
    let line = state.cursor_line.get() + 1;
    StatusLine {
        before: format!("{name}{STATUS_SEPARATOR}{language}{STATUS_SEPARATOR}{lines}{changes}"),
        hidden: (hidden > 0).then(|| hidden_lines_label(hidden)),
        after: format!("{STATUS_SEPARATOR}ln {line}{server}"),
    }
}

/// `1 change`, `4 changes`: a count with its noun, singular for one, as
/// the tab strip and the hidden count say it.
fn counted(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// `312 lines hidden`, with thousands separated.
fn hidden_lines_label(hidden: usize) -> String {
    if hidden == 1 {
        "1 line hidden".to_string()
    } else {
        format!("{} lines hidden", crate::ui::status::with_thousands(hidden))
    }
}

/// One line under the review view, in the status bar's right-hand slot.
/// The hidden count is a label of its own between the others: clicking it
/// is Show whole for the open file, and it is gone rather than blank when
/// the file has nothing folded, so the line reads as it did without folds.
pub fn review_status(state: ReviewState) -> impl IntoView {
    use floem::views::{Empty, Label};

    let muted = |s: floem::style::Style| s.font_size(theme::TEXT_META).color(theme::FG_MUTED);
    let line = move || status_line(state, state.hidden_lines());
    Stack::horizontal((
        Label::derived(move || line().before)
            .style(move |s| muted(s).text_ellipsis().min_width(0.0)),
        dyn_container(
            move || line().hidden,
            move |hidden| match hidden {
                None => Empty::new().into_any(),
                Some(text) => Stack::horizontal((
                    Label::derived(|| STATUS_SEPARATOR.to_string()).style(muted),
                    cards::tip(
                        Label::derived(move || text.clone())
                            .on_event_stop(el::Click, move |_, _| state.toggle_whole())
                            .style(move |s| {
                                crate::ui::widgets::clickable(muted(s))
                                    .hover(|s| s.color(theme::ACCENT))
                            }),
                        "Show this file whole".to_string(),
                    ),
                ))
                .style(|s| s.items_center().flex_shrink(0.0_f32))
                .into_any(),
            },
        ),
        Label::derived(move || line().after).style(move |s| muted(s).flex_shrink(0.0_f32)),
    ))
    .style(|s| s.items_center().min_width(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_line_names_the_hidden_lines_only_when_the_file_has_some() {
        let state = ReviewState::new();
        let file = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "fn a() {}\nfn b() {}\n".into(),
            "fn a() {}\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            "b1".into(),
        );
        state.open_tab("src/x.rs");
        state.finish_load("src/x.rs", std::sync::Arc::new(file));

        let before = format!(
            "src/x.rs{STATUS_SEPARATOR}Rust{STATUS_SEPARATOR}2 lines{STATUS_SEPARATOR}1 change"
        );
        let after = format!("{STATUS_SEPARATOR}ln 1");
        assert_eq!(
            status_line(state, 0),
            StatusLine {
                before: before.clone(),
                hidden: None,
                after: after.clone()
            }
        );
        assert_eq!(
            status_line(state, 312),
            StatusLine {
                before,
                hidden: Some("312 lines hidden".into()),
                after
            }
        );
        assert_eq!(
            status_line(state, 12_000).hidden.as_deref(),
            Some("12,000 lines hidden")
        );
        assert_eq!(
            status_line(state, 1).hidden.as_deref(),
            Some("1 line hidden")
        );

        // A file outside the worktree is named by its name and where it is
        // from, and has no changes to count.
        let outside = crate::backend::build_loaded_file(
            "/Users/me/.cargo/registry/src/index/tokio-1.40.0/src/lib.rs".into(),
            "fn a() {}\n".into(),
            String::new(),
            None,
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        state.open_tab("/Users/me/.cargo/registry/src/index/tokio-1.40.0/src/lib.rs");
        state.finish_load(
            "/Users/me/.cargo/registry/src/index/tokio-1.40.0/src/lib.rs",
            std::sync::Arc::new(outside),
        );
        assert_eq!(
            status_line(state, 0).before,
            format!("lib.rs{STATUS_SEPARATOR}tokio 1.40.0{STATUS_SEPARATOR}Rust{STATUS_SEPARATOR}1 line")
        );
        state.close_tab("/Users/me/.cargo/registry/src/index/tokio-1.40.0/src/lib.rs");

        // A line that is not about a file has no count, whatever it is
        // given: there is no document to have folds.
        state.close_tab("src/x.rs");
        assert_eq!(
            status_line(state, 312),
            StatusLine {
                before: "0 changed files".into(),
                hidden: None,
                after: String::new()
            }
        );
    }
}
