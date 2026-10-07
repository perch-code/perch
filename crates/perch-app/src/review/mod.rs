//! The review view: a pull request, its files, and a read-only editor.

pub mod apply_suggestion;
pub mod cards;
pub mod changescape;
pub mod close_as_issue;
pub mod close_menu;
pub mod code_refs;
pub mod composer_styling;
pub mod conversation;
pub mod conversation_find;
pub mod doc;
pub mod editor;
pub mod emoji;
pub mod find;
pub mod go_to;
pub mod gutter;
pub mod header;
pub mod hierarchy;
pub mod hover;
pub mod images;
pub mod linked_text;
pub mod markdown;
pub mod markdown_toolbar;
pub mod math;
pub mod mention;
pub mod merge;
pub mod model;
pub mod navigation;
pub mod open_pull;
pub mod picture;
pub mod rail;
pub mod references;
pub mod review_launch;
pub mod review_menu;
pub mod run_view;
pub mod scope;
pub mod start_work;
pub mod sticky;
pub mod styling;
pub mod submit_review;
pub mod suggestion;
pub mod summary;
pub mod switch;
pub mod tabs;
pub mod threads;
pub mod timeline;
pub mod tree;

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{SignalGet, SignalWith};
use floem::views::{dyn_container, Decorators, Empty, Stack};

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
        if state.open_tab(path) {
            load_file(&backend, path);
        }
        // Show it in the tree, so a file reached from a card or from
        // a definition can be found there too: the way to it opens,
        // the filter comes off when the file is not a change, and the
        // rail scrolls to its row.
        state.show_in_tree(path);
    })
}

/// Ask the backend for a file's content. A file outside the worktree has
/// no place in the tree, and is read from where it is; the loader's
/// limits still apply.
pub fn load_file(backend: &Backend, path: &str) {
    if std::path::Path::new(path).is_absolute() {
        backend.open_outside_file(path);
    } else {
        backend.open_file(path);
    }
}

/// The main pane while an item is open.
pub fn review_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_item: crate::ui::items::OpenItem,
) -> impl IntoView {
    let open_file = open_file(state, backend.clone());
    changescape::poll(state, backend.clone());
    let body_backend = backend.clone();
    let tabs_backend = backend.clone();

    Stack::vertical((
        header::header(app, state, backend.clone())
            // Keep header-owned popovers above the item surface. Floem does
            // not let a descendant's z-index escape its parent's sibling
            // context.
            .style(|s| s.width_full().flex_shrink(0.0_f32).z_index(1)),
        // Code's tabs, once there is an item to have them. They run the
        // pane's width, over the drawer too, so its tiles sit over what
        // they open. Conversation has none.
        dyn_container(
            {
                // A memo, so the strip is built again when it comes or
                // goes and not on every write to the tab.
                let code = floem::reactive::Memo::new(move |_| showing_code(state));
                move || code.get()
            },
            move |code| {
                if code {
                    tabs::tab_bar(app, state, tabs_backend.clone()).into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        )
        .style(|s| s.width_full().flex_shrink(0.0_f32)),
        Stack::horizontal((
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
            // A zero basis, so the body takes what the drawer leaves.
            .debug_name("pane-body")
            .style(|s| {
                s.flex_grow(1.0_f32)
                    .flex_basis(0.0)
                    .min_width(0.0)
                    .height_full()
            }),
            // The drawer on the right, under the tabs. Outside the keyed
            // body, so the open file, its scroll and a composer survive it
            // opening and shutting; over the body's edge when the window
            // is too narrow to dock it, which is why the row is its
            // positioning box.
            crate::ui::drawer::panel(app, state, backend),
        ))
        .style(|s| {
            s.flex_grow(1.0_f32)
                .min_height(0.0)
                .width_full()
                .position(floem::style::Position::Relative)
        }),
    ))
    .style(|s| {
        s.size_full()
            .flex_col()
            .min_height(0.0)
            .background(theme::canvas())
    })
}

/// Whether the pane shows Code: its tabs and the editor. An item with no
/// files shows Conversation whatever its tab says. Tracked.
fn showing_code(state: ReviewState) -> bool {
    matches!(state.phase.get(), Phase::Issue | Phase::Ready)
        && state.tab.get() == ItemTab::Code
        && state.has_files()
}

/// Whether the pane shows the Changescape tab: picked, and a changescape
/// to show. A tab kept from when there was one shows Conversation.
/// Tracked.
fn showing_changescape(state: ReviewState) -> bool {
    matches!(state.phase.get(), Phase::Issue | Phase::Ready)
        && state.tab.get() == ItemTab::Changescape
        && changescape::available(state)
}

/// The head the pull request moved to that the files on screen are not
/// at, unless it is the one a dirty worktree was already left behind by,
/// which opening again would not change. None while the files are still
/// on their way. Tracked.
fn moved_head(state: ReviewState) -> Option<String> {
    let head = state.head_moved.get()?;
    let behind = state.item.with(|item| {
        item.as_ref()
            .and_then(|item| item.checkout.as_ref())
            .is_some_and(|checkout| checkout.head_sha != head)
    });
    let left_behind = state
        .stale_head
        .with(|stale| stale.as_deref() == Some(head.as_str()));
    (behind && !left_behind).then_some(head)
}

/// What the switch has picked. Threads anchored in the open file are
/// drawn inline in the code; the whole conversation is Conversation's,
/// and the files are a tree in the drawer on the right.
fn item_view(
    app: AppState,
    state: ReviewState,
    open_file: OpenFile,
    backend: Rc<Backend>,
    open_item: crate::ui::items::OpenItem,
) -> impl IntoView {
    // A memo, so the pane is built again when what it shows changes and
    // not on every write to the tab.
    let showing = floem::reactive::Memo::new(move |_| showing_code(state));
    let changescape_shown = floem::reactive::Memo::new(move |_| showing_changescape(state));
    // Whether Code is waiting on the checkout, and whether that failed.
    let pending = floem::reactive::Memo::new(move |_| {
        state.files_pending.with(|pending| match pending {
            None => None,
            Some(Preparing::Failed { .. }) => Some(true),
            Some(_) => Some(false),
        })
    });
    let refresh_backend = backend.clone();
    Stack::vertical((
        dyn_container(
            move || (showing.get(), pending.get(), changescape_shown.get()),
            move |(code, pending, changescape)| {
                if code && pending == Some(false) {
                    header::preparing_card(
                        move || match state.files_pending.get() {
                            Some(Preparing::Working { message, .. }) => message,
                            _ => String::new(),
                        },
                        move || match state.files_pending.get() {
                            Some(Preparing::Working { percent, .. }) => percent,
                            _ => None,
                        },
                    )
                    .debug_name("files preparing")
                    .into_any()
                } else if code && pending == Some(true) {
                    centered_message(
                        SidebarIcon::Warning,
                        || "Could not check out the files".to_string(),
                        move || match state.files_pending.get() {
                            Some(Preparing::Failed { message }) => message,
                            _ => String::new(),
                        },
                    )
                    .debug_name("files failed")
                    .into_any()
                } else if code {
                    Stack::vertical((
                        changescape::strip::code_strip(state, backend.clone()),
                        editor::editor_pane(app, state, backend.clone(), open_file.clone())
                            .style(|s| s.flex_grow(1.0_f32).min_height(0.0)),
                    ))
                    .style(|s| s.size_full().flex_col().min_height(0.0))
                    .into_any()
                } else if changescape {
                    changescape::view(app, state, backend.clone()).into_any()
                } else {
                    conversation::view(
                        app,
                        state,
                        backend.clone(),
                        open_file.clone(),
                        open_item.clone(),
                    )
                    .into_any()
                }
            },
        )
        .style(|s| s.size_full().min_height(0.0)),
        // GitHub has news about the item: offered, not applied, so what is
        // being read stays put. A moved head is opened again, which checks
        // the new head out; other news is only fetched.
        crate::ui::widgets::news_pill(
            move || match moved_head(state) {
                Some(head) => format!("Pull request moved to {}", timeline::short_sha(&head)),
                None => "New activity on GitHub".to_string(),
            },
            move || moved_head(state).is_some() || state.upstream_changed.get(),
            move || {
                let Some(item_id) = state
                    .item
                    .with_untracked(|item| item.as_ref().map(|item| item.id.clone()))
                else {
                    return;
                };
                let moved = moved_head(state).is_some();
                state.upstream_changed.set(false);
                state.head_moved.set(None);
                if moved {
                    let navigation_id = refresh_backend.open_item(&item_id);
                    app.begin_item_review(
                        state,
                        "Reading the pull request",
                        &item_id,
                        navigation_id,
                    );
                } else {
                    refresh_backend.refresh_open_item(&item_id);
                }
            },
        ),
    ))
    .style(|s| {
        s.size_full()
            .min_height(0.0)
            .position(floem::style::Position::Relative)
    })
}
