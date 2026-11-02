//! The file rail: the scope, then the worktree as a tree, cut down to the
//! changed files until the filter is turned off. On an issue, the default
//! branch it reads in place of the scope, and its tree whole. It is what
//! the drawer on the right holds while it holds the files.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::views::{dyn_container, dyn_stack, Decorators, Empty, Label, ScrollExt, Stack};
use perch_git::{ChangeStatus, ChangedFile, TreeEntry};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::{ItemTab, ReviewState};
use crate::review::tree::{self, Row};
use crate::review::OpenFile;
use crate::review::{cards, scope};
use crate::state::{AppState, Checked};
use crate::theme;
use crate::ui::drawer;
use crate::ui::widgets::{checkbox, icon_button, sidebar_glyph};

/// The changed files by path, for the tree, which lists every file and
/// knows the changed ones only by path.
type ChangedByPath = Memo<Arc<HashMap<String, ChangedFile>>>;

/// The drawer holding the files: its bar, then the rail. A file opened
/// from the tree opens on Code, whichever tab the reader was on.
pub fn drawer_view(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let open_file = crate::review::open_file(state, backend.clone());
    let on_open: OpenFile = Rc::new(move |path: &str| {
        state.one_move(|| {
            if state.tab.get_untracked() != ItemTab::Code {
                state.show_tab(ItemTab::Code);
            }
            open_file(path);
        });
    });
    Stack::vertical((
        drawer::header_bar(
            Empty::new(),
            || "Files".to_string(),
            Empty::new(),
            drawer::close_button(app, backend.clone(), "files-close Close files"),
        ),
        rail(app, state, on_open, backend),
    ))
    .style(|s| s.size_full().flex_col().min_height(0.0))
    .debug_name("files-panel")
}

pub fn rail(
    app: AppState,
    state: ReviewState,
    on_open: OpenFile,
    backend: Rc<Backend>,
) -> impl IntoView {
    let changed: ChangedByPath = Memo::new(move |_| {
        state
            .changed_files
            .with(|files| Arc::new(files.iter().map(|f| (f.path.clone(), f.clone())).collect()))
    });

    let controls_backend = backend.clone();
    Stack::vertical((
        dyn_container(
            move || {
                (
                    state.historical_review.with(Option::is_some),
                    state.home.with(Option::is_some),
                )
            },
            // A retained snapshot has no scope to pick and nothing to mark
            // viewed, so neither row is there; the tab bar says what the
            // files are, and holds the way back. An issue has no change
            // either: its row is the branch it reads.
            move |(historical, home)| {
                if historical {
                    Empty::new().into_any()
                } else if home {
                    home_row(app, state, controls_backend.clone()).into_any()
                } else {
                    Stack::vertical((
                        scope::button(state, controls_backend.clone()),
                        viewed_row(state, controls_backend.clone()),
                    ))
                    .style(|s| s.flex_col().width_full())
                    .into_any()
                }
            },
        ),
        tree_list(state, changed, on_open, backend),
    ))
    .style(|s| {
        s.width_full()
            .flex_grow(1.0_f32)
            .min_height(0.0)
            .flex_col()
            .background(theme::rail_bg())
    })
}

/// The branch an issue's Code reads, its commit, how long ago it was
/// fetched, and a button to fetch it again.
fn home_row(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let home = move || state.home.get();
    let fetched = move || {
        if state.home_refreshing.get() {
            return "Fetching…".to_string();
        }
        match home().and_then(|h| h.fetched_at) {
            Some(at) => format!("fetched {}", crate::timefmt::relative(at, app.now.get())),
            None => String::new(),
        }
    };
    let refresh = icon_button(
        || SidebarIcon::Refresh,
        theme::fg_muted,
        move || {
            if state.home_refreshing.get_untracked() {
                return;
            }
            if let Some(id) = state
                .item
                .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
            {
                state.home_refreshing.set(true);
                backend.refresh_issue_home(&id);
            }
        },
    )
    .debug_name("home-refresh");
    let branch = move || home().map(|h| h.branch.clone()).unwrap_or_default();
    Stack::horizontal((
        Label::derived(branch).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg())
                .text_ellipsis()
                .min_width(0.0)
                .flex_shrink(1.0_f32)
        }),
        Label::derived(move || {
            home()
                .map(|h| h.head_sha.chars().take(7).collect::<String>())
                .unwrap_or_default()
        })
        .style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .font_family(theme::CODE_FONT_STACK.to_string())
                .color(theme::fg_muted())
                .margin_left(7.0)
                .flex_shrink(0.0_f32)
        }),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        Label::derived(fetched).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg_subtle())
                .text_ellipsis()
                .min_width(0.0)
                .margin_right(2.0)
        }),
        cards::tip_with(refresh, move || format!("Fetch {} again", branch())),
    ))
    .debug_name("home-row")
    .style(|s| {
        s.height(theme::SCOPE_BUTTON_HEIGHT)
            .width_full()
            .flex_shrink(0.0_f32)
            .items_center()
            .padding_left(10.0)
            .padding_right(4.0)
            .border_bottom(1.0)
            .border_color(theme::border())
            .selectable(false)
    })
}

/// How many of the changed files are viewed, a button for the rest, and
/// the filter. With nothing changed there is nothing to count or to
/// filter, so the row says so and the tree below is the whole worktree.
fn viewed_row(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // A memo, so the row is only rebuilt when the list empties or fills,
    // not on every toggle and scope change.
    let empty = Memo::new(move |_| state.changed_files.with(|f| f.is_empty()));
    // "All" is every file that can be marked.
    let all_viewed = move || {
        state.changed_files.with(|files| {
            let mut viewable = files.iter().filter(|f| f.viewed_key().is_some()).peekable();
            viewable.peek().is_some() && viewable.all(|f| state.is_viewed(f))
        })
    };

    dyn_container(
        move || empty.get(),
        move |empty| {
            if empty {
                return Container::new(Label::derived(move || "No changes").style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::fg_muted())
                        .selectable(false)
                }))
                .style(|s| {
                    s.height(theme::VIEWED_ROW_HEIGHT)
                        .width_full()
                        .flex_shrink(0.0_f32)
                        .items_center()
                        .padding_left(10.0)
                        .border_bottom(1.0)
                        .border_color(theme::border())
                })
                .into_any();
            }
            let backend = backend.clone();
            Stack::horizontal((
                Label::derived(move || {
                    let (viewed, total) = state.viewed_count();
                    format!("{viewed} of {total} viewed")
                })
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::fg_muted())
                        .flex_grow(1.0_f32)
                        .selectable(false)
                }),
                Label::derived(move || {
                    if all_viewed() {
                        "Clear all"
                    } else {
                        "Mark all"
                    }
                })
                .on_event_stop(el::Click, move |_, _| {
                    set_all_viewed(state, &backend, !all_viewed())
                })
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::accent())
                        .selectable(false)
                        .padding_horiz(6.0)
                        .padding_vert(2.0)
                        .margin_right(2.0)
                        .border_radius(4.0)
                        .hover(|s| s.background(theme::badge_bg()).cursor(CursorStyle::Pointer))
                }),
                filter_button(state),
            ))
            .style(|s| {
                s.height(theme::VIEWED_ROW_HEIGHT)
                    .width_full()
                    .flex_shrink(0.0_f32)
                    .items_center()
                    .padding_left(10.0)
                    .padding_right(4.0)
                    .border_bottom(1.0)
                    .border_color(theme::border())
            })
            .into_any()
        },
    )
}

/// The filter: lit in the accent while the tree is cut down to the
/// changed files, grey while it is the whole worktree. The same glyph the
/// sidebar's filter uses, so it reads as one.
fn filter_button(state: ReviewState) -> impl IntoView {
    dyn_container(
        move || state.changed_only.get(),
        move |on| {
            let button = icon_button(
                || SidebarIcon::Filter,
                move || {
                    if on {
                        theme::accent()
                    } else {
                        theme::fg_muted()
                    }
                },
                move || state.changed_only.set(!on),
            );
            let text = if on {
                "Showing the changed files. Click for the whole worktree."
            } else {
                "Showing the whole worktree. Click for the changed files."
            };
            cards::tip(button, text.into()).into_any()
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

/// Mark or clear every changed file that has content to mark. One write
/// per file that is not already there, so Mark all on a half-viewed list
/// does not rewrite the half that is.
fn set_all_viewed(state: ReviewState, backend: &Backend, viewed: bool) {
    let Some(item_id) = state.item.with(|i| i.as_ref().map(|i| i.id.clone())) else {
        return;
    };
    for file in state.changed_files.get().iter() {
        if let Some(key) = file.viewed_key() {
            if state.is_viewed(file) != viewed {
                backend.set_viewed(&item_id, &file.path, key, viewed);
            }
        }
    }
}

/// The tree. Three steps, each remembered so a click on a chevron redoes
/// only the last: the worktree with the deleted files put back, the rows
/// for it whole or cut down to the changes, then the rows that are open.
fn tree_list(
    state: ReviewState,
    changed: ChangedByPath,
    on_open: OpenFile,
    backend: Rc<Backend>,
) -> impl IntoView {
    // A deleted file is gone from the worktree but is in the changed list,
    // and needs a row to be opened and marked from.
    let full: Memo<Arc<Vec<TreeEntry>>> = Memo::new(move |_| {
        let tree = state.tree.get();
        let missing = changed.with(|c| tree::with_paths(&tree, c.keys().map(String::as_str)));
        match missing {
            Some(merged) => Arc::new(merged),
            None => tree,
        }
    });
    let rows: Memo<Arc<Vec<Row>>> = Memo::new(move |_| {
        let full = full.get();
        let filtered = state.changed_only.get() && changed.with(|c| !c.is_empty());
        if filtered {
            let pruned = changed.with(|c| tree::changed_only(&full, c.keys().map(String::as_str)));
            Arc::new(tree::rows(&pruned, true))
        } else {
            Arc::new(tree::rows(&full, false))
        }
    });

    // Keyed on the whole row, not the path: the same directory is one
    // row folded with the filter on and another on its own with it off,
    // and a row kept by its path would show the old label at the old
    // depth.
    dyn_stack(
        move || {
            state
                .expanded
                .with(|expanded| tree::visible(rows.get().as_ref(), expanded))
        },
        |row| row.clone(),
        move |row| tree_row(state, row, changed, on_open.clone(), backend.clone()),
    )
    .style(|s| s.flex_col().width_full().padding_vert(4.0))
    .scroll()
    // Stretched across the rail rather than its full width, so a selected
    // row can reach the window's edge under the bar.
    .class(crate::scrollers::ToEdge)
    .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0))
}

fn tree_row(
    state: ReviewState,
    row: Row,
    changed: ChangedByPath,
    on_open: OpenFile,
    backend: Rc<Backend>,
) -> impl IntoView {
    let is_dir = row.is_dir;
    let depth = row.depth;
    let label = row.label.clone();
    let click_row = row.clone();
    let chevron_row = row.clone();
    let chevron_click_row = row.clone();
    let is_expanded = move || state.expanded.with(|e| tree::is_expanded(&chevron_row, e));
    // One closure per use: each captures a String, so none of them is Copy.
    let (name_path, row_path, file_path, viewed_path, scroll_path) = (
        row.path.clone(),
        row.path.clone(),
        row.path.clone(),
        row.path.clone(),
        row.path,
    );

    let active_name = move || {
        state
            .active
            .with(|a| a.as_deref() == Some(name_path.as_str()))
    };
    let active_row = move || {
        state
            .active
            .with(|a| a.as_deref() == Some(row_path.as_str()))
    };
    // A file the pull request touched carries its status, churn and Viewed
    // check, so the changes are marked while browsing the whole tree.
    let file = move || changed.with(|c| c.get(&file_path).cloned());
    // A viewed file drops to the subtle colour, name and counts alike, so
    // what is left to read stands out from what is done.
    let viewed = move || changed.with(|c| c.get(&viewed_path).is_some_and(|f| state.is_viewed(f)));

    let row = Stack::horizontal((
        Empty::new().style(move |s| s.width(depth as f64 * 12.0).flex_shrink(0.0_f32)),
        // A chevron drawn rather than typed: the arrow characters are not in
        // the UI font and rendered as missing-glyph boxes. On a directory
        // it is a button of its own, so a directory that is itself a
        // change, a submodule, can be opened by its row and still shut.
        Container::new(dyn_container(
            move || is_dir.then(&is_expanded),
            move |open| match open {
                Some(true) => sidebar_glyph(SidebarIcon::ChevronDown, theme::fg_subtle).into_any(),
                Some(false) => {
                    sidebar_glyph(SidebarIcon::ChevronRight, theme::fg_subtle).into_any()
                }
                None => Empty::new().into_any(),
            },
        ))
        .on_event(el::Click, move |_, _| {
            if is_dir {
                state
                    .expanded
                    .update(|expanded| tree::toggle_row(expanded, &chevron_click_row));
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        .style(|s| {
            s.size(14.0, 14.0)
                .flex_shrink(0.0_f32)
                .items_center()
                .justify_center()
                .margin_right(3.0)
        }),
        Label::derived(move || label.clone()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(if viewed() {
                    theme::fg_subtle()
                } else if is_dir {
                    theme::fg_muted()
                } else {
                    theme::fg()
                })
                .font_weight(if active_name() {
                    theme::weight_emphasis()
                } else {
                    theme::weight_body()
                })
                .flex_grow(1.0_f32)
                .min_width(0.0)
                .text_ellipsis()
        }),
        dyn_container(file, move |file| match file {
            Some(file) => change_marks(state, file, backend.clone()).into_any(),
            None => Empty::new().into_any(),
        })
        .style(|s| s.flex_shrink(0.0_f32)),
    ))
    .on_event_stop(el::Click, move |_, _| {
        // A directory that is itself a change, a submodule, opens like a
        // file: the diff is what the reader came for. The chevron shuts it.
        if is_dir && !changed.with(|c| c.contains_key(&click_row.path)) {
            state
                .expanded
                .update(|expanded| tree::toggle_row(expanded, &click_row));
        } else {
            on_open(&click_row.path);
        }
    })
    .style(move |s| {
        let base = crate::ui::widgets::clickable(s)
            .height(theme::TREE_ROW_HEIGHT)
            .width_full()
            .items_center()
            .padding_left(10.0)
            .padding_right(4.0);
        let base = base.hover(|s| s.background(theme::canvas_hover()));
        if active_row() {
            base.background(theme::accent_subtle())
        } else {
            base
        }
    })
    .debug_name(format!("tree-row:{scroll_path}"));
    // The row of the file just opened scrolls into view. From the first
    // layout after the ask rather than at once: a row the open brought
    // into the tree, by opening its directory or turning the filter off,
    // has no place to scroll to until it is laid out.
    let row_id = row.view_id();
    row.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        let mine = state
            .scroll_to_row
            .with_untracked(|p| p.as_deref() == Some(scroll_path.as_str()));
        if mine {
            state.scroll_to_row.set(None);
            row_id.scroll_to(None);
        }
    })
}

/// The right end of a changed file's row: its status, its churn, and the
/// Viewed check.
fn change_marks(state: ReviewState, file: ChangedFile, backend: Rc<Backend>) -> impl IntoView {
    let counts_file = file.clone();
    let viewed_counts = move || state.is_viewed(&counts_file);
    let viewed_file = file.clone();
    Stack::horizontal((
        status_letter(file.status),
        counts(file.additions, file.deletions, file.binary, viewed_counts),
        dyn_container(
            move || state.historical_review.with(Option::is_some),
            move |read_only| {
                if read_only {
                    Empty::new()
                        .style(|s| s.size(26.0, 26.0).flex_shrink(0.0_f32))
                        .into_any()
                } else {
                    viewed_box(state, viewed_file.clone(), backend.clone()).into_any()
                }
            },
        ),
    ))
    .style(|s| s.items_center().flex_shrink(0.0_f32).margin_left(8.0))
}

/// The Viewed check at the row's right edge. A file with nothing to key
/// Viewed on gets the space and no box, so the counts stay in their
/// column. A deletion has a box. Checking a file is being done with it,
/// so its tab shuts as well; unchecking leaves the tab, since the reader
/// is going back to the file.
fn viewed_box(state: ReviewState, file: ChangedFile, backend: Rc<Backend>) -> impl IntoView {
    let Some(blob) = file.viewed_key().map(str::to_string) else {
        return Empty::new()
            .style(|s| s.size(26.0, 26.0).flex_shrink(0.0_f32))
            .into_any();
    };
    let path = file.path.clone();
    let toggle_file = file.clone();
    checkbox(
        move || {
            if state.is_viewed(&file) {
                Checked::Full
            } else {
                Checked::Empty
            }
        },
        move || {
            let Some(item_id) = state.item.with(|i| i.as_ref().map(|i| i.id.clone())) else {
                return;
            };
            let viewed = !state.is_viewed(&toggle_file);
            backend.set_viewed(&item_id, &path, &blob, viewed);
            if viewed && state.tabs.with(|t| t.iter().any(|x| x.path == path)) {
                state.close_tab(&path);
            }
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
    .into_any()
}

/// A changed file's status as git's letter in its colour: the rail's, and
/// Cmd-P's list's.
pub(crate) fn status_letter(status: ChangeStatus) -> impl IntoView {
    Label::derived(move || status.letter()).style(move |s| {
        let color = match status {
            ChangeStatus::Added => theme::status_added(),
            ChangeStatus::Modified | ChangeStatus::TypeChanged => theme::status_modified(),
            ChangeStatus::Deleted => theme::status_deleted(),
            ChangeStatus::Renamed | ChangeStatus::Copied => theme::status_renamed(),
        };
        s.font_size(theme::TEXT_BADGE)
            .font_weight(theme::weight_emphasis())
            .color(color)
            .width(14.0)
            .flex_shrink(0.0_f32)
            .text_align(floem::text::Alignment::Center)
    })
}

fn counts(
    additions: u32,
    deletions: u32,
    binary: bool,
    muted: impl Fn() -> bool + 'static,
) -> impl IntoView {
    // Two readers of one closure that captures the file, so it is not Copy.
    let muted = Rc::new(muted);
    let muted_deletions = muted.clone();
    Stack::horizontal((
        Label::derived(move || {
            if binary {
                "bin".to_string()
            } else {
                format!("+{additions}")
            }
        })
        .style(move |s| {
            s.font_size(theme::TEXT_BADGE).color(if binary || muted() {
                theme::fg_subtle()
            } else {
                theme::status_added()
            })
        }),
        Label::derived(move || {
            if binary {
                String::new()
            } else {
                format!("-{deletions}")
            }
        })
        .style(move |s| {
            s.font_size(theme::TEXT_BADGE)
                .color(if muted_deletions() {
                    theme::fg_subtle()
                } else {
                    theme::status_deleted()
                })
                .margin_left(5.0)
        }),
    ))
    .style(|s| s.items_center().flex_shrink(0.0_f32).margin_left(6.0))
}

/// The rail in a headless window, driven by clicks.
#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::reactive::{SignalGet, SignalUpdate};
    use perch_git::{ChangeStatus, ChangedFile, TreeEntry};
    use perch_github::{Item, ItemCheckout};

    use crate::review::model::{ItemTab, Phase, ReviewState};
    use crate::state::AppState;
    use crate::theme;

    fn changed(path: &str, blob: Option<&str>) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            old_path: None,
            status: if blob.is_some() {
                ChangeStatus::Modified
            } else {
                ChangeStatus::Deleted
            },
            additions: 3,
            deletions: 1,
            binary: false,
            head_blob: blob.map(String::from),
        }
    }

    /// The left edge of the drawer on the right, docked at its default
    /// width against the window's right edge.
    const DRAWER_LEFT: f64 = 1400.0 - theme::DRAWER_WIDTH;

    /// Code, its rail and the drawer, all of it dark once the system is.
    #[test]
    fn goes_dark_with_the_system() {
        let mut harness = app_harness().0;
        let left = crate::theme::dark_check::light_left(&mut harness);
        assert!(
            left.is_empty(),
            "light colours left in dark:\n{}",
            left.join("\n")
        );
    }

    /// The whole window on Code with a worktree of `src` and a README,
    /// a branch that changed three of the files in `src`, the second of
    /// them viewed, and the sidebar shut so the drawer on the right, on
    /// the files, is docked. The filter is on, so the rows are `src` and
    /// the three changed files.
    fn app_harness() -> (HeadlessHarness, ReviewState) {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let now = "2026-09-18T09:59:00Z".parse().unwrap();
        let mut item = Item::new("slatedb/slatedb", now);
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
            divergence: None,
            source: None,
        });
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state.set_phase(Phase::Ready);
        state.tab.set(ItemTab::Code);
        state.tree.set(Arc::new(
            [
                "src",
                "src/a.rs",
                "src/b.rs",
                "src/c.rs",
                "src/untouched.rs",
                "README.md",
            ]
            .into_iter()
            .map(|path| TreeEntry::new(path, path == "src"))
            .collect(),
        ));
        state.set_changed_files(Arc::new(vec![
            changed("src/a.rs", Some("blob-a")),
            changed("src/b.rs", Some("blob-b")),
            changed("src/c.rs", Some("blob-c")),
        ]));
        state.viewed.set(Arc::new(
            [("src/b.rs".to_string(), "blob-b".to_string())]
                .into_iter()
                .collect(),
        ));
        state.composer_autofocus.set(false);
        app.sidebar_open.set(false);
        app.tree_open.set(true);
        let backend = crate::backend::Backend::detached();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        (harness, state)
    }

    /// The top of the drawer's rail: under the item bar and the tabs, and
    /// the drawer's own bar.
    fn rail_top() -> f64 {
        theme::REVIEW_HEADER_HEIGHT + theme::TAB_HEIGHT + theme::DRAWER_HEADER_HEIGHT
    }

    /// The top of the tree: the scope button, viewed row, then the list's
    /// 4px of padding.
    fn list_top() -> f64 {
        rail_top() + theme::SCOPE_BUTTON_HEIGHT + theme::VIEWED_ROW_HEIGHT + 4.0
    }

    /// The centre of a row's name, `row` rows down the tree.
    fn name_center(row: usize) -> (f64, f64) {
        let x = DRAWER_LEFT + 60.0;
        let y = list_top() + row as f64 * theme::TREE_ROW_HEIGHT + theme::TREE_ROW_HEIGHT / 2.0;
        (x, y)
    }

    /// The centre of a row's checkbox: 26 square against the row's 4px
    /// right padding, at the window's right edge.
    fn checkbox_center(row: usize) -> (f64, f64) {
        let x = 1400.0 - 4.0 - 13.0;
        (x, name_center(row).1)
    }

    /// The centre of the filter button, at the right end of the viewed
    /// row, the same 26 square against the same 4px.
    fn filter_center() -> (f64, f64) {
        let x = 1400.0 - 4.0 - 13.0;
        let y = rail_top() + theme::SCOPE_BUTTON_HEIGHT + theme::VIEWED_ROW_HEIGHT / 2.0;
        (x, y)
    }

    fn click(harness: &mut HeadlessHarness, (x, y): (f64, f64)) {
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
    }

    /// The view given `name` as its debug name, which floem follows with
    /// the view's own: `tree-row:src - Horizontal Stack`.
    fn find(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        let debug = id.debug_name();
        if debug == name || debug.starts_with(&format!("{name} - ")) {
            return Some(id);
        }
        id.children().into_iter().find_map(|c| find(c, name))
    }

    /// The one scroll in the rail, the tree's.
    fn rail_scroll(harness: &HeadlessHarness) -> floem::ViewId {
        fn scrolls(id: floem::ViewId, out: &mut Vec<floem::ViewId>) {
            if id.debug_name() == "Scroll" {
                out.push(id);
            }
            for child in id.children() {
                scrolls(child, out);
            }
        }
        let mut found = Vec::new();
        scrolls(harness.root_id(), &mut found);
        let rail: Vec<_> = found
            .into_iter()
            .filter(|id| id.get_visual_rect_no_clip().x0 >= DRAWER_LEFT)
            .collect();
        let [scroll] = rail[..] else {
            panic!("one scroll in the rail, not {}", rail.len())
        };
        scroll
    }

    /// The check is inside a row that is itself a button, and a click on
    /// it must check and not open. The keys are not driven here: a bare
    /// `}` only reaches a focused view, and the one focusable view on
    /// Files is the editor, which the headless window cannot focus.
    #[test]
    fn the_viewed_check_does_not_open_the_file() {
        let (mut harness, state) = app_harness();
        // Row 0 is `src`; a.rs is under it.
        let (x, y) = checkbox_center(1);
        let under = harness.element_id_at(x, y);
        click(&mut harness, (x, y));
        assert_eq!(
            state.active.get(),
            None,
            "click at ({x}, {y}) over {under:?} opened the file"
        );

        // The same row, on its name, does open it.
        click(&mut harness, name_center(1));
        assert_eq!(state.active.get().as_deref(), Some("src/a.rs"));
    }

    /// Checking an open file shuts its tab, and the row opens it again.
    /// Unchecking leaves the tab: b.rs starts viewed, and the detached
    /// backend never answers, so its check unchecks every time.
    #[test]
    fn checking_an_open_file_closes_its_tab() {
        let (mut harness, state) = app_harness();
        click(&mut harness, name_center(1));
        click(&mut harness, name_center(2));
        assert_eq!(state.active.get().as_deref(), Some("src/b.rs"));
        assert_eq!(state.tabs.get().len(), 2);

        click(&mut harness, checkbox_center(1));
        assert_eq!(
            state
                .tabs
                .get()
                .iter()
                .map(|t| t.path.as_str())
                .collect::<Vec<_>>(),
            ["src/b.rs"],
            "the check shut a.rs"
        );
        assert_eq!(state.active.get().as_deref(), Some("src/b.rs"));

        click(&mut harness, checkbox_center(2));
        assert_eq!(state.tabs.get().len(), 1, "unchecking b.rs left its tab");
        assert_eq!(state.active.get().as_deref(), Some("src/b.rs"));

        // Checking the active file shows its neighbour, as its close button does.
        click(&mut harness, name_center(1));
        assert_eq!(state.active.get().as_deref(), Some("src/a.rs"));
        assert_eq!(state.tabs.get().len(), 2);
        click(&mut harness, checkbox_center(1));
        assert_eq!(state.tabs.get().len(), 1);
        assert_eq!(state.active.get().as_deref(), Some("src/b.rs"));

        // A check on a file with no tab changes nothing.
        click(&mut harness, checkbox_center(3));
        assert_eq!(state.tabs.get().len(), 1);
        assert_eq!(state.active.get().as_deref(), Some("src/b.rs"));
    }

    /// A deletion has a check like any other file, so a click on it marks
    /// the file rather than opening it. The deleted file is not in the
    /// worktree, and has a row all the same.
    #[test]
    fn a_deletion_has_a_check() {
        let (mut harness, state) = app_harness();
        state.set_changed_files(Arc::new(vec![
            changed("src/gone.rs", None),
            changed("src/a.rs", Some("blob-a")),
        ]));
        harness.rebuild();
        harness.paint();
        // src, a.rs, gone.rs: the tree's order, not the list's.
        click(&mut harness, checkbox_center(2));
        assert_eq!(state.active.get(), None, "the check opened gone.rs");
        click(&mut harness, name_center(2));
        assert_eq!(state.active.get().as_deref(), Some("src/gone.rs"));
    }

    /// The filter cuts the tree to the changed files; off, the whole
    /// worktree is there, with the changed files still marked.
    #[test]
    fn the_filter_button_shows_the_whole_worktree_and_back() {
        let (mut harness, state) = app_harness();
        assert!(state.changed_only.get(), "the filter starts on");
        // Row 4 would be untouched.rs in the whole tree. Filtered, there
        // is nothing there.
        click(&mut harness, name_center(4));
        assert_eq!(state.active.get(), None, "the filtered tree has four rows");

        click(&mut harness, filter_center());
        assert!(!state.changed_only.get(), "the filter button turns it off");
        click(&mut harness, name_center(4));
        assert_eq!(state.active.get().as_deref(), Some("src/untouched.rs"));
        // The changed files keep their check in the whole tree.
        click(&mut harness, checkbox_center(3));
        assert_eq!(
            state.active.get().as_deref(),
            Some("src/untouched.rs"),
            "the check did not open c.rs"
        );

        click(&mut harness, filter_center());
        assert!(state.changed_only.get());
        click(&mut harness, name_center(1));
        assert_eq!(state.active.get().as_deref(), Some("src/a.rs"));
        click(&mut harness, name_center(4));
        assert_eq!(
            state.active.get().as_deref(),
            Some("src/a.rs"),
            "row 4 is gone again"
        );
    }

    /// A file opened from outside the rail, a definition Cmd-click landed
    /// on say, that the filter hides: the tree turns to the whole
    /// worktree, the way to the file opens, and its row is selected.
    #[test]
    fn opening_a_file_the_filter_hides_shows_the_whole_worktree_at_it() {
        use floem::peniko::Brush;
        use floem::style::Background;
        let (mut harness, state) = app_harness();
        // The reader shut src; the jump opens it again.
        click(&mut harness, name_center(0));
        assert!(!state.expanded.get().contains("src"));

        state.navigate_to("src/untouched.rs", 0, 0);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.active.get().as_deref(), Some("src/untouched.rs"));
        assert!(!state.changed_only.get(), "the filter came off");
        assert!(state.expanded.get().contains("src"));
        // Row 4 of the whole tree, drawn as the active row.
        let row =
            find(harness.root_id(), "tree-row:src/untouched.rs").expect("the row is in the tree");
        let rect = row.get_visual_rect_no_clip();
        assert_eq!(rect.center().y, name_center(4).1);
        assert_eq!(
            harness.get_computed_style(row).get(Background),
            Some(Brush::Solid(theme::accent_subtle())),
            "the row is selected"
        );
        assert_eq!(state.scroll_to_row.get(), None, "the row took the scroll");

        // A jump to a changed file leaves the filter as the reader set it.
        click(&mut harness, filter_center());
        assert!(state.changed_only.get());
        state.navigate_to("src/b.rs", 0, 0);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.active.get().as_deref(), Some("src/b.rs"));
        assert!(state.changed_only.get(), "b.rs is in the filtered tree");
    }

    /// The rail scrolls to the opened file's row: a definition can be far
    /// down a tree the filter had cut to a handful of rows.
    #[test]
    fn the_rail_scrolls_to_the_opened_file() {
        let (mut harness, state) = app_harness();
        let files: Vec<String> = (0..60).map(|i| format!("src/f{i:02}.rs")).collect();
        state.tree.set(Arc::new(
            std::iter::once(TreeEntry::new("src", true))
                .chain(
                    files
                        .iter()
                        .map(|path| TreeEntry::new(path.as_str(), false)),
                )
                .chain(std::iter::once(TreeEntry::new("README.md", false)))
                .collect(),
        ));
        state.set_changed_files(Arc::new(vec![changed("src/f00.rs", Some("blob-0"))]));
        harness.rebuild();
        harness.paint();

        state.navigate_to("src/f59.rs", 0, 0);
        harness.rebuild();
        harness.paint();
        assert!(!state.changed_only.get());
        let scroll = rail_scroll(&harness);
        let viewport = scroll.get_visual_rect();
        let row = find(harness.root_id(), "tree-row:src/f59.rs").expect("the row is in the tree");
        let rect = row.get_visual_rect_no_clip();
        assert!(
            viewport.contains_rect(rect),
            "row 60 at {rect:?} is in the rail's {viewport:?}"
        );
        // The list scrolled: the top row is off the top.
        let top = find(harness.root_id(), "tree-row:src").unwrap();
        assert!(top.get_visual_rect_no_clip().y1 < viewport.y0);
        assert_eq!(state.scroll_to_row.get(), None);
    }

    /// The open file's highlight runs to the window's edge while macOS
    /// overlays the bars, with no gutter kept for a bar that is not there;
    /// kept bars keep theirs.
    #[test]
    fn the_open_files_row_reaches_the_window_edge() {
        use floem::style::{ScrollbarWidth, StyleProp};

        let (mut harness, state) = app_harness();
        state.navigate_to("src/a.rs", 0, 0);
        harness.rebuild();
        harness.paint();
        let row = find(harness.root_id(), "tree-row:src/a.rs").expect("the row is in the tree");
        let right = row.get_visual_rect().x1;
        let bar = ScrollbarWidth::default_value().0;
        let edge = if crate::scrollers::overlaid() {
            1400.0
        } else {
            1400.0 - bar
        };
        assert_eq!(right, edge);
    }

    /// A run of lone directories is one row, and that row opens and
    /// shuts the run. Here the branch changed one file deep in `lib`,
    /// so the filtered tree is `lib/inner/deep` and the file under it.
    #[test]
    fn a_folded_run_is_one_row_that_toggles() {
        let (mut harness, state) = app_harness();
        state.tree.set(Arc::new(
            [
                "lib",
                "lib/inner",
                "lib/inner/deep",
                "lib/inner/deep/x.rs",
                "lib/inner/other.rs",
                "README.md",
            ]
            .into_iter()
            .map(|path| TreeEntry::new(path, !path.ends_with(".rs") && !path.ends_with(".md")))
            .collect(),
        ));
        state.set_changed_files(Arc::new(vec![changed(
            "lib/inner/deep/x.rs",
            Some("blob-x"),
        )]));
        harness.rebuild();
        harness.paint();
        // Row 0 is the run, row 1 the file.
        click(&mut harness, name_center(1));
        assert_eq!(state.active.get().as_deref(), Some("lib/inner/deep/x.rs"));

        state.active.set(None);
        click(&mut harness, name_center(0));
        assert!(
            !state.expanded.get().contains("lib/inner/deep"),
            "the run shut at its innermost"
        );
        click(&mut harness, name_center(1));
        assert_eq!(state.active.get(), None, "the file is hidden");

        click(&mut harness, name_center(0));
        let expanded = state.expanded.get();
        assert!(
            expanded.contains("lib")
                && expanded.contains("lib/inner")
                && expanded.contains("lib/inner/deep")
        );
        click(&mut harness, name_center(1));
        assert_eq!(state.active.get().as_deref(), Some("lib/inner/deep/x.rs"));

        // Off, the run comes apart into its directories, all still open:
        // lib, inner, deep, x.rs, other.rs, README.md.
        click(&mut harness, filter_center());
        state.active.set(None);
        click(&mut harness, name_center(4));
        assert_eq!(state.active.get().as_deref(), Some("lib/inner/other.rs"));
    }

    /// A directory that is itself a change, a submodule, has the marks
    /// and opens on a click, as the flat list opened it; its chevron is
    /// what shuts it.
    #[test]
    fn a_changed_directory_opens_and_its_chevron_toggles() {
        let (mut harness, state) = app_harness();
        state.tree.set(Arc::new(
            ["vendor", "vendor/lib", "vendor/lib/lib.c", "README.md"]
                .into_iter()
                .map(|path| TreeEntry::new(path, !path.contains('.')))
                .collect(),
        ));
        state.set_changed_files(Arc::new(vec![changed("vendor/lib", Some("commit-sha"))]));
        harness.rebuild();
        harness.paint();
        // One folded row, `vendor/lib`.
        click(&mut harness, name_center(0));
        assert_eq!(state.active.get().as_deref(), Some("vendor/lib"));
        // Its check checks rather than opens.
        state.active.set(None);
        click(&mut harness, checkbox_center(0));
        assert_eq!(state.active.get(), None);
        // The chevron, at the row's left edge after the 10px padding,
        // opens the run: the change is the directory itself, so only the
        // way to it started open.
        let x = DRAWER_LEFT + 10.0 + 7.0;
        let y = name_center(0).1;
        assert!(state.expanded.get().contains("vendor"));
        assert!(!state.expanded.get().contains("vendor/lib"));
        click(&mut harness, (x, y));
        assert_eq!(state.active.get(), None, "the chevron did not open it");
        assert!(state.expanded.get().contains("vendor/lib"));
        click(&mut harness, (x, y));
        assert!(
            !state.expanded.get().contains("vendor/lib"),
            "and shuts it again"
        );
        assert_eq!(state.active.get(), None);
    }

    /// A directory row shuts and opens on a click, in either mode.
    #[test]
    fn a_directory_row_toggles() {
        let (mut harness, state) = app_harness();
        assert!(
            state.expanded.get().contains("src"),
            "the changed files' directory starts open"
        );
        click(&mut harness, name_center(0));
        assert!(!state.expanded.get().contains("src"));
        // Shut, its files are gone: row 1 is empty rail.
        click(&mut harness, name_center(1));
        assert_eq!(state.active.get(), None);
        click(&mut harness, name_center(0));
        assert!(state.expanded.get().contains("src"));
        click(&mut harness, name_center(1));
        assert_eq!(state.active.get().as_deref(), Some("src/a.rs"));
    }

    /// With nothing changed there is no filter to speak of: the row says
    /// so and the tree is the whole worktree.
    #[test]
    fn nothing_changed_shows_the_whole_worktree() {
        let (mut harness, state) = app_harness();
        state.set_changed_files(Arc::new(Vec::new()));
        harness.rebuild();
        harness.paint();
        assert!(state.changed_only.get(), "the flag is untouched");
        click(&mut harness, name_center(4));
        assert_eq!(state.active.get().as_deref(), Some("src/untouched.rs"));
        // No filter button either: a click where it was is nothing.
        click(&mut harness, filter_center());
        assert!(state.changed_only.get());

        // Changes back, the row and its button come back with them.
        state.set_changed_files(Arc::new(vec![changed("src/a.rs", Some("blob-a"))]));
        harness.rebuild();
        harness.paint();
        state.active.set(None);
        click(&mut harness, name_center(4));
        assert_eq!(
            state.active.get(),
            None,
            "filtered again: src and a.rs only"
        );
        click(&mut harness, filter_center());
        assert!(!state.changed_only.get());
    }
}
