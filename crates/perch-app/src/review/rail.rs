//! The file rail: the scope, then the changed files or the whole worktree.

use std::rc::Rc;

use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalWith};
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::{Decorators, Empty, Label, ScrollExt, Stack, dyn_container, dyn_stack};
use perch_git::{ChangeStatus, ChangedFile, TreeEntry};

use crate::backend::Backend;
use crate::review::model::{RailMode, ReviewState};
use crate::review::{scope, tree};
use crate::review::OpenFile;
use crate::icons::SidebarIcon;
use crate::state::Checked;
use crate::ui::widgets::{checkbox, sidebar_glyph};
use crate::theme;

pub fn rail(state: ReviewState, on_open: OpenFile, backend: Rc<Backend>) -> impl IntoView {
    Stack::vertical((
        scope::button(state, backend.clone()),
        mode_toggle(state),
        dyn_container(
            move || state.rail_mode.get(),
            move |mode| match mode {
                RailMode::Changed => changed_pane(state, on_open.clone(), backend.clone()).into_any(),
                RailMode::AllFiles => tree_list(state, on_open.clone()).into_any(),
            },
        )
        .style(|s| s.flex_grow(1.0_f32).min_height(0.0).width_full()),
    ))
    .style(|s| {
        s.width(theme::RAIL_WIDTH)
            .height_full()
            .flex_col()
            .flex_shrink(0.0_f32)
            .background(theme::RAIL_BG)
            .border_right(1.0)
            .border_color(theme::BORDER)
    })
}

fn mode_toggle(state: ReviewState) -> impl IntoView {
    let changed_count = move || state.changed_files.with(|f| f.len());

    Stack::horizontal((
        segment(state, RailMode::Changed, "Changed", Some(changed_count)),
        segment(state, RailMode::AllFiles, "All files", None::<fn() -> usize>),
    ))
    .style(|s| {
        s.height(42.0)
            .width_full()
            .flex_shrink(0.0_f32)
            .items_center()
            .padding_horiz(8.0)
            .gap(4.0)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

fn segment(
    state: ReviewState,
    mode: RailMode,
    text: &'static str,
    count: Option<impl Fn() -> usize + 'static + Copy>,
) -> impl IntoView {
    let active = move || state.rail_mode.get() == mode;

    Stack::horizontal((
        Label::derived(move || text).style(move |s| {
            s.font_size(theme::TEXT_META)
                .font_weight(if active() { FontWeight::SEMI_BOLD } else { FontWeight::NORMAL })
                .color(if active() { theme::FG } else { theme::FG_MUTED })
        }),
        dyn_container(
            move || count.map(|c| c()).unwrap_or(0),
            move |n| {
                if n == 0 {
                    Empty::new().into_any()
                } else {
                    Label::derived(move || n.to_string())
                        .style(|s| {
                            s.font_size(theme::TEXT_BADGE)
                                .color(theme::BADGE_FG)
                                .background(theme::BADGE_BG)
                                .border_radius(8.0)
                                .padding_horiz(5.0)
                                .margin_left(6.0)
                        })
                        .into_any()
                }
            },
        ),
    ))
    .on_event_stop(el::Click, move |_, _| state.rail_mode.set(mode))
    .style(move |s| {
        let base = crate::ui::widgets::clickable(s)
            .height(30.0)
            .items_center()
            .padding_horiz(11.0)
            .border_radius(theme::RADIUS)
            ;
        if active() {
            base.background(theme::CANVAS)
                .border(1.0)
                .border_color(theme::BORDER)
        } else {
            base.border(1.0).border_color(Color::TRANSPARENT)
        }
    })
}

fn changed_pane(state: ReviewState, on_open: OpenFile, backend: Rc<Backend>) -> impl IntoView {
    Stack::vertical((
        viewed_row(state, backend.clone()),
        changed_list(state, on_open, backend),
    ))
    .style(|s| s.flex_col().size_full().min_height(0.0))
}

/// How many of the changed files are viewed, and a button for the rest.
/// Absent when there is nothing to view.
fn viewed_row(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // A memo, so the row is only rebuilt when the list empties or fills,
    // not on every toggle and scope change.
    let empty = Memo::new(move |_| state.changed_files.with(|f| f.is_empty()));
    // A deletion has no content to view, so "all" is every file that has.
    let all_viewed = move || {
        state.changed_files.with(|files| {
            let mut viewable = files.iter().filter(|f| f.head_blob.is_some()).peekable();
            viewable.peek().is_some() && viewable.all(|f| state.is_viewed(f))
        })
    };

    dyn_container(
        move || empty.get(),
        move |empty| {
            if empty {
                return Empty::new().into_any();
            }
            let backend = backend.clone();
            Stack::horizontal((
                Label::derived(move || {
                    let (viewed, total) = state.viewed_count();
                    format!("{viewed} of {total} viewed")
                })
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::FG_MUTED)
                        .flex_grow(1.0_f32)
                        .selectable(false)
                }),
                Label::derived(move || if all_viewed() { "Clear all" } else { "Mark all" })
                    .on_event_stop(el::Click, move |_, _| set_all_viewed(state, &backend, !all_viewed()))
                    .style(|s| {
                        s.font_size(theme::TEXT_BADGE)
                            .color(theme::ACCENT)
                            .selectable(false)
                            .padding_horiz(6.0)
                            .padding_vert(2.0)
                            .border_radius(4.0)
                            .hover(|s| s.background(theme::BADGE_BG).cursor(CursorStyle::Pointer))
                    }),
            ))
            .style(|s| {
                s.height(theme::VIEWED_ROW_HEIGHT)
                    .width_full()
                    .flex_shrink(0.0_f32)
                    .items_center()
                    .padding_left(10.0)
                    .padding_right(4.0)
                    .border_bottom(1.0)
                    .border_color(theme::BORDER_MUTED)
            })
            .into_any()
        },
    )
}

/// Mark or clear every changed file that has content to mark. One write
/// per file that is not already there, so Mark all on a half-viewed list
/// does not rewrite the half that is.
fn set_all_viewed(state: ReviewState, backend: &Backend, viewed: bool) {
    let Some(item_id) = state.item.with(|i| i.as_ref().map(|i| i.id.clone())) else {
        return;
    };
    for file in state.changed_files.get().iter() {
        if let Some(blob) = &file.head_blob {
            if state.is_viewed(file) != viewed {
                backend.set_viewed(&item_id, &file.path, blob, viewed);
            }
        }
    }
}

fn changed_list(state: ReviewState, on_open: OpenFile, backend: Rc<Backend>) -> impl IntoView {
    dyn_stack(
        move || state.changed_files.get().as_ref().clone(),
        |f| f.path.clone(),
        move |f| changed_row(state, f, on_open.clone(), backend.clone()),
    )
    .style(|s| s.flex_col().width_full().padding_vert(4.0))
    .scroll()
    .style(|s| s.size_full().min_height(0.0))
}

fn changed_row(state: ReviewState, file: ChangedFile, on_open: OpenFile, backend: Rc<Backend>) -> impl IntoView {
    let click_path = file.path.clone();
    // One closure per use: each captures a String, so none of them is Copy.
    let (name_path, row_path) = (file.path.clone(), file.path.clone());
    let active_name = move || state.active.with(|a| a.as_deref() == Some(name_path.as_str()));
    let active_row = move || state.active.with(|a| a.as_deref() == Some(row_path.as_str()));
    // A viewed row drops to the subtle colour, name and counts alike, so
    // what is left to read stands out from what is done.
    let (name_file, counts_file) = (file.clone(), file.clone());
    let viewed_name = move || state.is_viewed(&name_file);
    let viewed_counts = move || state.is_viewed(&counts_file);

    let directory = file.directory().to_string();
    let name = file.file_name().to_string();
    let status = file.status;
    let additions = file.additions;
    let deletions = file.deletions;
    let binary = file.binary;

    Stack::horizontal((
        status_letter(status),
        // The directory is muted so the eye lands on the file name.
        Stack::horizontal((
            Label::derived(move || directory.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG_SUBTLE)
                    .text_ellipsis()
                    .min_width(0.0)
            }),
            Label::derived(move || name.clone()).style(move |s| {
                s.font_size(theme::TEXT_META)
                    .color(if viewed_name() { theme::FG_SUBTLE } else { theme::FG })
                    .font_weight(if active_name() { FontWeight::SEMI_BOLD } else { FontWeight::NORMAL })
                    .flex_shrink(0.0_f32)
            }),
        ))
        .style(|s| s.flex_grow(1.0_f32).min_width(0.0).items_center()),
        counts(additions, deletions, binary, viewed_counts),
        viewed_box(state, file, backend),
    ))
    .on_event_stop(el::Click, move |_, _| on_open(&click_path))
    .style(move |s| {
        let base = crate::ui::widgets::clickable(s)
            .height(32.0)
            .width_full()
            .items_center()
            .padding_left(10.0)
            .padding_right(4.0)
            .hover(|s| s.background(theme::CANVAS_HOVER));
        if active_row() {
            base.background(theme::ACCENT_SUBTLE)
        } else {
            base
        }
    })
}

/// The Viewed check at the row's right edge. A deletion has no head
/// content to have viewed, so it gets the space and no box, and the
/// counts stay in their column.
fn viewed_box(state: ReviewState, file: ChangedFile, backend: Rc<Backend>) -> impl IntoView {
    let Some(blob) = file.head_blob.clone() else {
        return Empty::new().style(|s| s.size(26.0, 26.0).flex_shrink(0.0_f32)).into_any();
    };
    let path = file.path.clone();
    let toggle_file = file.clone();
    checkbox(
        move || if state.is_viewed(&file) { Checked::Full } else { Checked::Empty },
        move |_| {
            let Some(item_id) = state.item.with(|i| i.as_ref().map(|i| i.id.clone())) else {
                return;
            };
            backend.set_viewed(&item_id, &path, &blob, !state.is_viewed(&toggle_file));
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
    .into_any()
}

fn status_letter(status: ChangeStatus) -> impl IntoView {
    let color = match status {
        ChangeStatus::Added => theme::STATUS_ADDED,
        ChangeStatus::Modified | ChangeStatus::TypeChanged => theme::STATUS_MODIFIED,
        ChangeStatus::Deleted => theme::STATUS_DELETED,
        ChangeStatus::Renamed | ChangeStatus::Copied => theme::STATUS_RENAMED,
    };
    Label::derived(move || status.letter()).style(move |s| {
        s.font_size(theme::TEXT_BADGE)
            .font_weight(FontWeight::SEMI_BOLD)
            .color(color)
            .width(14.0)
            .flex_shrink(0.0_f32)
            .text_align(floem::text::Alignment::Center)
            .margin_right(6.0)
    })
}

fn counts(additions: u32, deletions: u32, binary: bool, muted: impl Fn() -> bool + 'static) -> impl IntoView {
    // Two readers of one closure that captures the file, so it is not Copy.
    let muted = Rc::new(muted);
    let muted_deletions = muted.clone();
    Stack::horizontal((
        Label::derived(move || if binary { "bin".to_string() } else { format!("+{additions}") })
            .style(move |s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(if binary || muted() { theme::FG_SUBTLE } else { theme::STATUS_ADDED })
            }),
        Label::derived(move || if binary { String::new() } else { format!("-{deletions}") }).style(
            move |s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(if muted_deletions() { theme::FG_SUBTLE } else { theme::STATUS_DELETED })
                    .margin_left(5.0)
            },
        ),
    ))
    .style(|s| s.items_center().flex_shrink(0.0_f32).margin_left(8.0))
}

/// Whether the changed file at a path is viewed, for the tree, which
/// lists every file and knows the changed ones only by path.
fn viewed_at(state: ReviewState, path: &str) -> bool {
    state
        .changed_files
        .with(|files| files.iter().find(|f| f.path == path).is_some_and(|f| state.is_viewed(f)))
}

fn tree_list(state: ReviewState, on_open: OpenFile) -> impl IntoView {
    dyn_stack(
        move || {
            state
                .expanded
                .with(|expanded| tree::visible_entries(state.tree.get().as_ref(), expanded))
        },
        |e| e.path.clone(),
        move |e| tree_row(state, e, on_open.clone()),
    )
    .style(|s| s.flex_col().width_full().padding_vert(4.0))
    .scroll()
    .style(|s| s.size_full().min_height(0.0))
}

fn tree_row(state: ReviewState, entry: TreeEntry, on_open: OpenFile) -> impl IntoView {
    let click_path = entry.path.clone();
    let status_path = entry.path.clone();
    let (name_path, row_path) = (entry.path.clone(), entry.path.clone());
    let chevron_path = entry.path.clone();
    let is_expanded = move || state.expanded.with(|e| e.contains(&chevron_path));
    let is_dir = entry.is_dir;
    let depth = entry.depth;
    let name = entry
        .path
        .rsplit('/')
        .next()
        .unwrap_or(&entry.path)
        .to_string();

    let active_name = move || state.active.with(|a| a.as_deref() == Some(name_path.as_str()));
    let active_row = move || state.active.with(|a| a.as_deref() == Some(row_path.as_str()));
    // A file the pull request touched carries the same letter here as in the
    // changed list, so the diff is visible while browsing the whole tree.
    let status = move || state.status_of(&status_path);
    // And the same grey once it is viewed.
    let viewed_path = entry.path.clone();
    let viewed = move || !is_dir && viewed_at(state, &viewed_path);

    Stack::horizontal((
        Empty::new().style(move |s| s.width(depth as f64 * 12.0).flex_shrink(0.0_f32)),
        // A chevron drawn rather than typed: the arrow characters are not in
        // the UI font and rendered as missing-glyph boxes.
        Container::new(dyn_container(
            move || is_dir.then(&is_expanded),
            move |open| match open {
                Some(true) => sidebar_glyph(SidebarIcon::ChevronDown, theme::FG_SUBTLE).into_any(),
                Some(false) => sidebar_glyph(SidebarIcon::ChevronRight, theme::FG_SUBTLE).into_any(),
                None => Empty::new().into_any(),
            },
        ))
        .style(|s| {
            s.size(14.0, 14.0)
                .flex_shrink(0.0_f32)
                .items_center()
                .justify_center()
                .margin_right(3.0)
        }),
        Label::derived(move || name.clone()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(if viewed() {
                    theme::FG_SUBTLE
                } else if is_dir {
                    theme::FG_MUTED
                } else {
                    theme::FG
                })
                .font_weight(if active_name() { FontWeight::SEMI_BOLD } else { FontWeight::NORMAL })
                .flex_grow(1.0_f32)
                .min_width(0.0)
                .text_ellipsis()
        }),
        dyn_container(status, move |s| match s {
            Some(status) => status_letter(status).into_any(),
            None => Empty::new().into_any(),
        }),
    ))
    .on_event_stop(el::Click, move |_, _| {
        if is_dir {
            state.expanded.update(|expanded| tree::toggle(expanded, &click_path));
        } else {
            on_open(&click_path);
        }
    })
    .style(move |s| {
        let base = crate::ui::widgets::clickable(s)
            .height(28.0)
            .width_full()
            .items_center()
            .padding_horiz(10.0);
        let base = base.hover(|s| s.background(theme::CANVAS_HOVER));
        if active_row() {
            base.background(theme::ACCENT_SUBTLE)
        } else {
            base
        }
    })
}

/// The rail in a headless window, driven by clicks.
#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::reactive::{SignalGet, SignalUpdate};
    use perch_git::{ChangeStatus, ChangedFile};
    use perch_github::{Item, ItemCheckout};

    use crate::review::model::{ItemTab, Phase, ReviewState};
    use crate::state::AppState;
    use crate::theme;

    fn changed(path: &str, blob: Option<&str>) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            old_path: None,
            status: if blob.is_some() { ChangeStatus::Modified } else { ChangeStatus::Deleted },
            additions: 3,
            deletions: 1,
            binary: false,
            head_blob: blob.map(String::from),
        }
    }

    /// The whole window on Files with a branch of three changed files,
    /// the second of them viewed, and the drawer shut so the rail starts
    /// at the activity rail's edge.
    fn app_harness() -> (HeadlessHarness, ReviewState) {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let now = "2026-09-18T09:59:00Z".parse().unwrap();
        let mut item = Item::new("slatedb", "slatedb/slatedb", now);
        item.id = "wt".into();
        item.checkout = Some(ItemCheckout {
            branch: "perch/pr-812".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: std::path::PathBuf::from("/tmp/wt"),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
        });
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state.set_phase(Phase::Ready);
        state.tab.set(ItemTab::Files);
        state.changed_files.set(Arc::new(vec![
            changed("src/a.rs", Some("blob-a")),
            changed("src/b.rs", Some("blob-b")),
            changed("src/c.rs", Some("blob-c")),
        ]));
        state.viewed.set(Arc::new([("src/b.rs".to_string(), "blob-b".to_string())].into_iter().collect()));
        state.composer_autofocus.set(false);
        app.sidebar_open.set(false);
        let backend = crate::backend::Backend::detached();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        (harness, state)
    }

    /// The top of the changed list: header, tab strip, scope button, mode
    /// toggle, viewed row, then the list's 4px of padding.
    fn list_top() -> f64 {
        theme::REVIEW_HEADER_HEIGHT + theme::TAB_HEIGHT + theme::SCOPE_BUTTON_HEIGHT + 42.0 + theme::VIEWED_ROW_HEIGHT + 4.0
    }

    /// The centre of a row's checkbox: 26 square against the row's 4px
    /// right padding, in a rail `RAIL_WIDTH` wide from the activity rail.
    fn checkbox_center(row: usize) -> (f64, f64) {
        let x = theme::ACTIVITY_RAIL_WIDTH + theme::RAIL_WIDTH - 4.0 - 13.0;
        let y = list_top() + row as f64 * 32.0 + 16.0;
        (x, y)
    }

    /// The check is inside a row that is itself a button, and a click on
    /// it must check and not open. The keys are not driven here: a bare
    /// `}` only reaches a focused view, and the one focusable view on
    /// Files is the editor, which the headless window cannot focus.
    #[test]
    fn the_viewed_check_does_not_open_the_file() {
        let (mut harness, state) = app_harness();
        let (x, y) = checkbox_center(0);
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.active.get(), None, "click at ({x}, {y}) over {under:?} opened the file");

        // The same row, on its name, does open it.
        let x = theme::ACTIVITY_RAIL_WIDTH + 60.0;
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.active.get().as_deref(), Some("src/a.rs"));
    }

    /// A deletion has nothing to view; its row keeps the check's space so
    /// the counts line up, and a click there opens the file as the rest
    /// of the row does.
    #[test]
    fn a_deletion_has_no_check() {
        let (mut harness, state) = app_harness();
        state.changed_files.set(Arc::new(vec![changed("src/gone.rs", None), changed("src/a.rs", Some("blob-a"))]));
        harness.rebuild();
        harness.paint();
        let (x, y) = checkbox_center(0);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.active.get().as_deref(), Some("src/gone.rs"));
    }
}
