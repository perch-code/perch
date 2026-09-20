//! The review view: a pull request, its files, and a read-only editor.

pub mod cards;
pub mod composer_styling;
pub mod conversation;
pub mod doc;
pub mod editor;
pub mod gutter;
pub mod header;
pub mod hover;
pub mod issue;
pub mod markdown;
pub mod merge;
pub mod model;
pub mod rail;
pub mod references;
pub mod review_launch;
pub mod scope;
pub mod strip;
pub mod styling;
pub mod tabs;
pub mod threads;
pub mod timeline;
pub mod tree;

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
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

/// The whole right-hand side while a pull request is open.
pub fn review_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_item: crate::ui::items::OpenItem,
) -> impl IntoView {
    let open_file: OpenFile = Rc::new({
        let backend = backend.clone();
        move |path: &str| {
            // Open the path to it in the tree, so a file reached from the
            // changed list or from a definition can be found there too.
            state
                .expanded
                .update(|expanded| tree::reveal(expanded, path));
            if state.open_tab(path) {
                backend.open_file(path);
            }
        }
    });

    let body_backend = backend.clone();
    let header_open_item = open_item;
    let on_back: Rc<dyn Fn()> = Rc::new({
        let backend = backend.clone();
        move || {
            let navigation_id = backend.close_review();
            app.begin_close(state, navigation_id);
        }
    });

    // The backend closes the review itself when the archive goes through,
    // and the Items event that follows closes the view. A refusal leaves
    // everything as it was, with the reason in the status line.
    let on_archive: Rc<dyn Fn(String)> = Rc::new({
        let backend = backend.clone();
        move |id: String| {
            backend.archive_item(&id);
        }
    });

    let header_backend = backend.clone();
    // A memo, so the header is rebuilt when the kind of thing open changes
    // and not on the way from Preparing to Ready.
    let is_issue = floem::reactive::Memo::new(move |_| state.phase.get() == Phase::Issue);
    Stack::vertical((
        dyn_container(
            move || is_issue.get(),
            move |is_issue| {
                if is_issue {
                    header::issue_header(
                        app,
                        state,
                        header_backend.clone(),
                        header_open_item.clone(),
                    )
                    .into_any()
                } else {
                    let (back, archive) = (on_back.clone(), on_archive.clone());
                    header::header(
                        app,
                        state,
                        header_backend.clone(),
                        header_open_item.clone(),
                        move || back(),
                        move |id| archive(id),
                    )
                    .into_any()
                }
            },
        )
        // Keep header-owned popovers above the item surface. Floem does not
        // let a descendant's z-index escape its parent's sibling context.
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
                Phase::Issue | Phase::Ready => {
                    item_view(app, state, open_file.clone(), body_backend.clone()).into_any()
                }
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

/// The tab strip, then the tab: Conversation for every item, Files for one
/// with a checkout. The strip is drawn for an issue too, with one tab, so
/// the view keeps its shape when Start work adds a checkout.
fn item_view(
    app: AppState,
    state: ReviewState,
    open_file: OpenFile,
    backend: Rc<Backend>,
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
    Stack::vertical((
        strip::strip(state),
        dyn_container(
            move || tab.get(),
            move |tab| match tab {
                ItemTab::Conversation => {
                    conversation::view(app, state, backend.clone(), open_file.clone()).into_any()
                }
                ItemTab::Files => body(app, state, open_file.clone(), backend.clone()).into_any(),
            },
        )
        .style(|s| s.flex_grow(1.0_f32).min_height(0.0).width_full()),
    ))
    .style(|s| s.size_full().flex_col().min_height(0.0))
}

/// The rail, the code and the thread column. The column stays where the
/// reader put it: a file's threads are drawn in the code as cards, so
/// nothing opens the column for them, and `open_composer` opens it only
/// for a composer it draws.
fn body(
    app: AppState,
    state: ReviewState,
    open_file: OpenFile,
    backend: Rc<Backend>,
) -> impl IntoView {
    let column_backend = backend.clone();
    let column_open = open_file.clone();
    Stack::horizontal((
        rail::rail(state, open_file.clone(), backend.clone()),
        Stack::vertical((
            tabs::tab_bar(app, state),
            editor::editor_pane(app, state, backend, open_file),
        ))
        .style(|s| s.flex_grow(1.0_f32).min_width(0.0).height_full().flex_col()),
        dyn_container(
            move || state.threads_open.get(),
            move |open| {
                if open {
                    floem::views::Container::new(Stack::horizontal((
                        threads::resize_handle(state),
                        threads::thread_column(
                            app,
                            state,
                            column_backend.clone(),
                            column_open.clone(),
                        ),
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
    if file.truncated {
        return whole(format!("{}{STATUS_SEPARATOR}not shown as text", file.path));
    }
    let language = file.language.map(|l| l.name()).unwrap_or("Plain text");
    let lines = counted(file.line_count(), "line");
    let changes = counted(file.change_count(), "change");
    let line = state.cursor_line.get() + 1;
    StatusLine {
        before: format!(
            "{}{STATUS_SEPARATOR}{language}{STATUS_SEPARATOR}{lines}{STATUS_SEPARATOR}{changes}",
            file.path
        ),
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
/// is Show whole for the item, and it is gone rather than blank when the
/// file has nothing folded, so the line reads as it did without folds.
pub fn review_status(app: AppState, state: ReviewState) -> impl IntoView {
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
                            .on_event_stop(el::Click, move |_, _| tabs::toggle_folded(app, state))
                            .style(move |s| {
                                crate::ui::widgets::clickable(muted(s))
                                    .hover(|s| s.color(theme::ACCENT))
                            }),
                        "Show this worktree's files whole".to_string(),
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
    use floem::headless::{HeadlessHarness, TestRoot};

    fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().split_whitespace().any(|part| part == name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
    }

    fn descends_from(mut id: floem::ViewId, ancestor: floem::ViewId) -> bool {
        loop {
            if id == ancestor {
                return true;
            }
            let Some(parent) = id.parent() else {
                return false;
            };
            id = parent;
        }
    }

    #[test]
    fn review_launcher_stays_above_metadata_and_item_content() {
        let app = AppState::new();
        let state = ReviewState::new();
        let now = "2026-09-20T12:00:00Z".parse().unwrap();

        let mut item = perch_github::Item::new("local/perch", now);
        item.id = "worktree".into();
        item.checkout = Some(perch_github::ItemCheckout {
            branch: "feature/review-layer".into(),
            base_ref: "main".into(),
            head_sha: "abcdef0123456789".into(),
            path: std::path::PathBuf::from("/tmp/perch-review-layer-test"),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
        });
        state.item.set(Some(std::sync::Arc::new(item)));
        state.ready.set(true);
        state.phase.set(Phase::Ready);
        state.review_launcher_open.set(true);

        // Force the metadata sibling to exist as well as the item content
        // sibling below the complete header.
        let mut child = perch_github::Item::new("local/perch", now);
        child.id = "child-worktree".into();
        child.name = Some("Child branch".into());
        child.from_item_id = Some("worktree".into());
        app.items.set(vec![child]);

        let root = TestRoot::new();
        let view = review_view(
            app,
            state,
            Rc::new(Backend::detached()),
            Rc::new(|_: &perch_github::Item| {}),
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 700.0);
        harness.rebuild();
        harness.paint();

        let popover = named(harness.root_id(), "review-launch-popover").unwrap();
        let bounds = popover.get_visual_rect_no_clip();
        let x = bounds.x0 + 2.0;
        let metadata_y = theme::REVIEW_HEADER_HEIGHT + theme::METADATA_HEIGHT / 2.0;
        let content_y = theme::REVIEW_HEADER_HEIGHT + theme::METADATA_HEIGHT + 10.0;
        assert!(bounds.y0 < metadata_y && bounds.y1 > content_y);

        for (layer, y) in [("metadata", metadata_y), ("content", content_y)] {
            let target = harness
                .element_id_at(x, y)
                .unwrap_or_else(|| panic!("no hit target over {layer}"))
                .owning_id();
            assert!(
                descends_from(target, popover),
                "{layer} painted over the review launcher"
            );
        }

        let close = named(harness.root_id(), "review-launch-close").unwrap();
        let close_bounds = close.get_visual_rect_no_clip();
        harness.click(
            (close_bounds.x0 + close_bounds.x1) / 2.0,
            (close_bounds.y0 + close_bounds.y1) / 2.0,
        );
        assert!(!state.review_launcher_open.get());
    }

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
