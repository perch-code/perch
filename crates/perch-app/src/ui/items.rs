//! Item rows: a pull request, an issue, or a branch of the reader's own,
//! under its repository in the sidebar.
//!
//! One row draws every shape an item can have. The glyph and the reference
//! line come from which parts the item has, never from a stored kind. The
//! row's menu has every entry on every row, with the ones that cannot
//! apply greyed rather than hidden, so it is the same size and order
//! everywhere.

// TextInput::on_enter is deprecated in favour of a listener; the name says
// what it does, so it stays until the listener form is the only one.
#![allow(deprecated)]

use std::rc::Rc;

use floem::menu::Menu;
use floem::muda::accelerator::{Accelerator, Code as KeyCode};
use floem::prelude::*;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::{Container, Decorators, Empty, Label, Stack, TextInput, dyn_container};
use perch_github::{CheckStatus, Item, ItemSubject, SubjectState};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::items::{attention_label, full_reference, reference, ItemGroup};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{
    clickable, copy_to_clipboard, icon_button, open_in_browser, open_in_terminal, reveal_in_finder,
    sidebar_glyph, text_button,
};

/// Opening an item. Shared by every row.
pub type OpenItem = Rc<dyn Fn(&Item)>;

/// Where a row sits, which changes what it says beside the reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Under Priority: the attention reason is shown.
    Priority,
    /// Under Pinned: a pin glyph.
    Pinned,
    /// Under a repository, an owner or a date, which decides whether the
    /// second line names the repository.
    Section(ItemGroup),
}

/// The state glyph an item shows: the pull request or issue state icon,
/// or a branch for a checkout with no subject.
pub fn state_of(item: &Item) -> Option<SubjectState> {
    match &item.subject {
        Some(ItemSubject::PullRequest(pr)) => Some(if pr.merged {
            SubjectState::PrMerged
        } else if !pr.is_open() {
            SubjectState::PrClosed
        } else if pr.draft {
            SubjectState::PrDraft
        } else {
            SubjectState::PrOpen
        }),
        Some(ItemSubject::Issue(issue)) => Some(if issue.is_open() {
            SubjectState::IssueOpen
        } else if issue.state_reason.as_deref() == Some("not_planned") {
            SubjectState::IssueNotPlanned
        } else {
            SubjectState::IssueClosed
        }),
        None => None,
    }
}

/// One item as the sidebar draws it.
pub fn item_row(
    state: AppState,
    backend: Rc<Backend>,
    item: Item,
    placement: Placement,
    on_open: OpenItem,
) -> impl IntoView {
    let hover_id = item.id.clone();
    let leave_id = item.id.clone();
    let open_target = item.clone();
    let selected = {
        let id = item.id.clone();
        move || state.open_item.with(|o| o.as_deref() == Some(id.as_str()))
    };
    let archived = item.is_archived();
    let attention = item.needs_attention();
    let mut reference_text = match placement {
        Placement::Section(group) => reference(&item, group),
        // No heading above these names a repository, so the row does.
        Placement::Priority | Placement::Pinned => full_reference(&item),
    };
    // A branch of the reader's own says how far it has gone from its base.
    if item.subject.is_none() {
        if let Some((a, d)) = item.checkout.as_ref().and_then(|c| c.churn) {
            reference_text = format!("{reference_text}  +{a} \u{2212}{d}");
        }
    }
    if placement == Placement::Priority {
        if let Some(reason) = item.attention_reason.as_deref() {
            reference_text = format!("{reference_text} \u{00b7} {}", attention_label(reason));
        }
    }

    let menu_item = item.clone();
    let menu_backend = backend.clone();
    let actions_backend = backend.clone();
    let under_backend = backend.clone();
    let row_item = item.clone();
    let title_item = item.clone();

    // The click and the context menu are on the row, and both land on the
    // deepest view under the pointer, so everything drawn in the row lets
    // the pointer through to it: the content stack is pointer_events_none
    // and every view in it inherits that, except the controls, which set
    // it back: the rename box and the hover buttons.
    let content = Stack::horizontal((
        glyph(&item),
        Stack::vertical((
            // The title, or a text box in its place while it is renamed.
            dyn_container(
                {
                    let id = item.id.clone();
                    move || state.renaming.with(|r| r.as_deref() == Some(id.as_str()))
                },
                move |renaming| {
                    if renaming {
                        rename_box(state, backend.clone(), title_item.clone()).into_any()
                    } else {
                        let title = title_item.title().to_string();
                        Label::derived(move || title.clone())
                            .style(move |s| {
                                s.font_size(theme::TEXT_BODY)
                                    .font_weight(if attention { FontWeight::SEMI_BOLD } else { FontWeight::NORMAL })
                                    .color(if archived { theme::FG_SUBTLE } else { theme::FG })
                                    .text_ellipsis()
                                    .max_width_full()
                            })
                            .into_any()
                    }
                },
            )
            .style(|s| s.width_full().min_width(0.0)),
            Stack::horizontal((
                Label::derived(move || reference_text.clone()).style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::FG_SUBTLE)
                        .text_ellipsis()
                        .min_width(0.0)
                        .flex_shrink(1.0_f32)
                        .margin_top(1.0)
                }),
                chips(state, &item),
            ))
            .style(|s| s.items_center().width_full().min_width(0.0)),
        ))
        .style(|s| s.flex_col().flex_grow(1.0_f32).min_width(0.0).justify_center()),
        row_actions(state, actions_backend, row_item, placement).style(|s| s.pointer_events_auto()),
        trailing(attention, placement),
    ))
    .style(|s| s.items_center().width_full().min_width(0.0).pointer_events_none());

    let row = Container::new(content)
        .on_event_stop(el::Click, move |_, _| on_open(&open_target))
        .on_event_cont(el::PointerEnter, move |_, _| state.hovered_item.set(Some(hover_id.clone())))
        .on_event_cont(el::PointerLeave, move |_, _| {
            state.hovered_item.update(|h| {
                if h.as_deref() == Some(leave_id.as_str()) {
                    *h = None;
                }
            })
        })
        .context_menu(move || item_menu(state, menu_backend.clone(), menu_item.clone()))
        .style(move |s| {
            let base = clickable(s)
                .width_full()
                .height(46.0)
                .items_center()
                .padding_left(10.0)
                .padding_right(6.0)
                .margin_horiz(6.0)
                .border_radius(theme::RADIUS);
            if selected() {
                base.background(theme::ACCENT_SUBTLE)
            } else {
                base.hover(|s| s.background(theme::CANVAS_HOVER))
            }
        });

    // What opens under the row: a Delete confirmation.
    let under_item = item.clone();
    Stack::vertical((
        row,
        dyn_container(
            {
                let id = item.id.clone();
                move || state.confirm_delete.with(|d| d.as_deref() == Some(id.as_str()))
            },
            move |deleting| {
                if deleting {
                    delete_confirmation(state, under_backend.clone(), under_item.clone()).into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        )
        .style(|s| s.width_full()),
    ))
    .style(|s| s.flex_col().width_full())
}

fn glyph(item: &Item) -> impl IntoView {
    let state = state_of(item);
    let muted = item.is_archived();
    Container::new(match state {
        Some(state) => floem::views::svg(move || icons::state_icon(state))
            .style(move |s| {
                s.size(16.0, 16.0)
                    .color(if muted { theme::FG_SUBTLE } else { icons::state_color(state) })
            })
            .into_any(),
        None => sidebar_glyph(SidebarIcon::Branch, if muted { theme::FG_SUBTLE } else { theme::FG_MUTED })
            .into_any(),
    })
    .style(|s| s.size(16.0, 16.0).margin_right(10.0).flex_shrink(0.0_f32))
}

/// The CI chip and the review chip after a pull request's reference, from
/// the snapshot alone.
fn chips(state: AppState, item: &Item) -> impl IntoView {
    let (checks, review) = match item.pull_request() {
        Some(pr) => (
            pr.checks,
            pr.review_decision.or(if pr.review_requested {
                Some(perch_github::ReviewDecision::ReviewRequested)
            } else {
                None
            }),
        ),
        None => (None, None),
    };
    dyn_container(
        move || state.item_filters.with(|f| f.show_pr_status) && (checks.is_some() || review.is_some()),
        move |show| {
            if !show {
                return Empty::new().into_any();
            }
            Stack::horizontal((
                match checks {
                    Some(CheckStatus::Success) => {
                        sidebar_glyph(SidebarIcon::Check, theme::OPEN).style(|s| s.size(11.0, 11.0)).into_any()
                    }
                    Some(CheckStatus::Failure) => {
                        sidebar_glyph(SidebarIcon::Close, theme::CLOSED).style(|s| s.size(11.0, 11.0)).into_any()
                    }
                    Some(CheckStatus::Pending) => Empty::new()
                        .style(|s| s.size(7.0, 7.0).border_radius(4.0).background(theme::DRAFT_YELLOW))
                        .into_any(),
                    None => Empty::new().into_any(),
                },
                match review {
                    Some(r) => Label::derived(move || r.label().to_string())
                        .style(move |s| {
                            let color = match r {
                                perch_github::ReviewDecision::Approved => theme::OPEN,
                                perch_github::ReviewDecision::ChangesRequested => theme::CLOSED,
                                perch_github::ReviewDecision::ReviewRequested => theme::FG_MUTED,
                            };
                            s.font_size(theme::TEXT_SECTION)
                                .color(color)
                                .border(1.0)
                                .border_color(color.with_alpha(0.4))
                                .border_radius(8.0)
                                .padding_horiz(5.0)
                                .margin_left(5.0)
                                .selectable(false)
                        })
                        .into_any(),
                    None => Empty::new().into_any(),
                },
            ))
            .style(|s| s.items_center().margin_left(6.0).flex_shrink(0.0_f32))
            .into_any()
        },
    )
}

/// The accent dot at the right edge of a row that needs looking at, or the
/// pin under Pinned.
fn trailing(attention: bool, placement: Placement) -> impl IntoView {
    Container::new(if placement == Placement::Pinned {
        sidebar_glyph(SidebarIcon::Pin, theme::FG_SUBTLE).style(|s| s.size(11.0, 11.0)).into_any()
    } else if attention {
        Empty::new()
            .style(|s| s.size(7.0, 7.0).border_radius(4.0).background(theme::ACCENT))
            .into_any()
    } else {
        Empty::new().into_any()
    })
    .style(|s| s.width(14.0).items_center().justify_center().flex_shrink(0.0_f32))
}

/// Save, Done and the menu, on hover, as the inbox rows have.
fn row_actions(state: AppState, backend: Rc<Backend>, item: Item, placement: Placement) -> impl IntoView {
    let id = item.id.clone();
    let hovering = move || state.hovered_item.with(|h| h.as_deref() == Some(id.as_str()));
    let saved = item.saved;
    let done = item.is_done();
    let _ = placement;

    dyn_container(
        move || (hovering(), saved),
        move |(hovered, saved)| {
            if !hovered && !saved {
                return Empty::new().into_any();
            }
            let save_id = item.id.clone();
            let save_backend = backend.clone();
            let done_id = item.id.clone();
            let done_backend = backend.clone();
            let menu_item = item.clone();
            let menu_backend = backend.clone();
            Stack::horizontal((
                icon_button(
                    move || if saved { SidebarIcon::BookmarkFilled } else { SidebarIcon::Bookmark },
                    move || if saved { theme::ACCENT } else { theme::FG_SUBTLE },
                    move || save_backend.mark_item(&save_id, Some(!saved), None),
                ),
                if hovered {
                    Stack::horizontal((
                        icon_button(
                            move || if done { SidebarIcon::Undo } else { SidebarIcon::Check },
                            || theme::FG_SUBTLE,
                            move || done_backend.mark_item(&done_id, None, Some(!done)),
                        ),
                        more_button(move || item_menu(state, menu_backend.clone(), menu_item.clone())),
                    ))
                    .style(|s| s.items_center())
                    .into_any()
                } else {
                    Empty::new().into_any()
                },
            ))
            .style(|s| s.items_center().flex_shrink(0.0_f32))
            .into_any()
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

/// The `...` button: the item menu.
fn more_button(menu: impl Fn() -> Menu + 'static) -> impl IntoView {
    Container::new(
        sidebar_glyph(SidebarIcon::Ellipsis, theme::FG_SUBTLE)
            // The menu looks only at the deepest view under the pointer, so
            // the glyph must not be it.
            .style(|s| s.pointer_events_none()),
    )
    .popout_menu(menu)
    // The row underneath is a button too.
    .on_event_stop(el::Click, |_, _| {})
    .style(|s| {
        clickable(s)
            .size(26.0, 26.0)
            .items_center()
            .justify_center()
            .border_radius(13.0)
            .hover(|s| s.background(theme::BADGE_BG).cursor(CursorStyle::Pointer))
    })
}

fn key(code: KeyCode) -> Accelerator {
    Accelerator::new(None, code)
}

/// The menu every row has, in one shape.
///
/// Every entry is present on every row; those that cannot apply are greyed.
/// The letters work while the menu is open, the way the accelerators say.
pub fn item_menu(state: AppState, backend: Rc<Backend>, item: Item) -> Menu {
    let id = item.id.clone();
    let has_subject = item.subject.is_some();
    let on_disk = item.checkout.as_ref().is_some_and(|c| c.on_disk);
    let path = item.checkout.as_ref().map(|c| c.path.clone());
    let github_url = match &item.subject {
        Some(ItemSubject::PullRequest(pr)) => {
            Some(format!("https://github.com/{}/pull/{}", pr.repo_full_name, pr.number))
        }
        Some(ItemSubject::Issue(i)) => Some(format!("https://github.com/{}/issues/{}", i.repo_full_name, i.number)),
        None => None,
    };
    let pinned = item.is_pinned();
    let archived = item.is_archived();
    let is_issue = item.issue().is_some();

    let mut menu = Menu::new().submenu("Open in", {
        let path = path.clone();
        let github_url = github_url.clone();
        move |sub| {
            let (reveal, terminal, copy) = (path.clone(), path.clone(), path.clone());
            sub.item("GitHub", {
                let url = github_url.clone();
                move |i| {
                    let url = url.clone();
                    i.enabled(has_subject).action(move || {
                        if let Some(url) = &url {
                            open_in_browser(url)
                        }
                    })
                }
            })
            .item("Finder", move |i| {
                let p = reveal.clone();
                i.enabled(on_disk).action(move || {
                    if let Some(p) = &p {
                        reveal_in_finder(p)
                    }
                })
            })
            .item("Terminal", move |i| {
                let p = terminal.clone();
                i.enabled(on_disk).action(move || {
                    if let Some(p) = &p {
                        open_in_terminal(p)
                    }
                })
            })
            .item("Copy Path", move |i| {
                let p = copy.clone();
                i.enabled(on_disk).action(move || {
                    if let Some(p) = &p {
                        copy_to_clipboard(p.to_string_lossy().into_owned())
                    }
                })
            })
        }
    });
    menu = menu.separator();

    {
        let (b, i) = (backend.clone(), id.clone());
        menu = menu.item(if pinned { "Unpin" } else { "Pin" }, move |m| {
            let (b, i) = (b.clone(), i.clone());
            m.accelerator(key(KeyCode::KeyP)).action(move || b.pin_item(&i, !pinned))
        });
    }
    {
        let (b, i) = (backend.clone(), id.clone());
        menu = menu.item("Mark Unread", move |m| {
            let (b, i) = (b.clone(), i.clone());
            m.accelerator(key(KeyCode::KeyU)).action(move || b.mark_unread(&i))
        });
    }
    {
        let i = id.clone();
        menu = menu.item("Rename", move |m| {
            let i = i.clone();
            m.accelerator(key(KeyCode::KeyR)).action(move || {
                state.confirm_delete.set(None);
                state.renaming.set(Some(i.clone()));
            })
        });
    }
    {
        // Start work cuts from the default branch; Fork from the item's
        // head. Neither applies to an archived item or one with nothing
        // to cut from.
        let label = if is_issue { "Start Work" } else { "Fork" };
        let can = !archived && (is_issue || item.checkout.is_some());
        let (b, i) = (backend.clone(), id.clone());
        menu = menu.item(label, move |m| {
            let (b, i) = (b.clone(), i.clone());
            m.enabled(can).accelerator(key(KeyCode::KeyF)).action(move || {
                state.begin_open("Cutting a branch");
                if is_issue {
                    b.start_work(&i)
                } else {
                    b.fork_item(&i)
                }
            })
        });
    }
    menu = menu.separator();

    {
        let (b, i) = (backend.clone(), id.clone());
        // An unowned checkout that has gone from disk cannot come back.
        let restorable = !archived
            || item.checkout.as_ref().is_none_or(|c| c.owned || c.on_disk);
        menu = menu.item(if archived { "Restore" } else { "Archive" }, move |m| {
            let (b, i) = (b.clone(), i.clone());
            m.enabled(restorable).accelerator(key(KeyCode::KeyA)).action(move || {
                if archived {
                    b.restore_item(&i)
                } else {
                    b.archive_item(&i)
                }
            })
        });
    }
    {
        let i = id.clone();
        menu = menu.item("Delete", move |m| {
            let i = i.clone();
            m.accelerator(key(KeyCode::KeyD)).action(move || {
                state.renaming.set(None);
                state.confirm_delete.set(Some(i.clone()));
            })
        });
    }
    menu
}

/// The title as a text box. Enter commits, Escape cancels, empty restores
/// the given title.
fn rename_box(state: AppState, backend: Rc<Backend>, item: Item) -> impl IntoView {
    let text = RwSignal::new(item.title().to_string());
    let id = item.id.clone();
    let commit_backend = backend.clone();
    let input = TextInput::new(text)
        .placeholder(item.given_title())
        .on_enter(move || {
            let name = text.get();
            let name = name.trim();
            commit_backend.rename_item(&id, (!name.is_empty()).then(|| name.to_string()));
            state.renaming.set(None);
        })
        .on_event_cont(el::FocusGained, move |_, _| state.search_focused.set(true))
        .on_event_cont(el::FocusLost, move |_, _| state.search_focused.set(false))
        // A click in the box must not open the item.
        .on_event_stop(el::Click, |_, _| {})
        .style(|s| {
            // The row lets the pointer through; the box takes it back.
            s.pointer_events_auto()
                .width_full()
                .height(24.0)
                .font_size(theme::TEXT_BODY)
                .padding_horiz(6.0)
                .background(theme::CANVAS)
                .border(1.0)
                .border_color(theme::ACCENT)
                .border_radius(4.0)
        });
    let id = input.view_id();
    // Take the keyboard once the box is in the tree.
    let asked = std::rc::Rc::new(std::cell::Cell::new(false));
    input.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if !asked.replace(true) {
            id.request_focus();
        }
    })
}

/// "Delete <title>?" with what goes on disk, under the row.
fn delete_confirmation(state: AppState, backend: Rc<Backend>, item: Item) -> impl IntoView {
    let what = match item.checkout.as_ref() {
        Some(c) if c.owned && c.on_disk => format!(
            "Removes the row and its threads, the directory {}, and the branch {}.",
            c.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            c.branch
        ),
        Some(c) if c.owned => format!("Removes the row and its threads, and the branch {}.", c.branch),
        Some(_) => "Removes the row and its threads. Nothing on disk is touched.".to_string(),
        None => "Removes the row and its threads.".to_string(),
    };
    let title = item.title().to_string();
    let id = item.id.clone();
    Stack::vertical((
        Label::derived(move || format!("Delete {title}?")).style(|s| {
            s.font_size(theme::TEXT_META).font_weight(FontWeight::SEMI_BOLD).color(theme::FG).selectable(false)
        }),
        Label::derived(move || what.clone()).style(|s| {
            s.font_size(theme::TEXT_BADGE).color(theme::FG_MUTED).text_wrap().width_full().margin_top(2.0)
        }),
        Stack::horizontal((
            text_button(|| "Delete".to_string(), SidebarIcon::Trash, move || {
                backend.delete_item(&id);
                state.confirm_delete.set(None);
            }),
            text_button(|| "Cancel".to_string(), SidebarIcon::Close, move || state.confirm_delete.set(None)),
        ))
        .style(|s| s.gap(6.0).margin_top(8.0)),
    ))
    .on_event_stop(el::Click, |_, _| {})
    .style(|s| {
        s.flex_col()
            .width_full()
            .padding(10.0)
            .margin_horiz(6.0)
            .margin_bottom(4.0)
            .background(theme::NOTICE_BG)
            .border(1.0)
            .border_color(theme::NOTICE_BORDER)
            .border_radius(theme::RADIUS)
    })
}

/// A section heading in the sidebar: a name, a count in the muted colour,
/// and a chevron when it can fold.
pub fn section_header(title: String, count: usize, collapsed: Option<bool>) -> impl IntoView {
    Stack::horizontal((
        match collapsed {
            Some(c) => sidebar_glyph(
                if c { SidebarIcon::ChevronRight } else { SidebarIcon::ChevronDown },
                theme::FG_SUBTLE,
            )
            .style(|s| s.size(10.0, 10.0).margin_right(5.0))
            .into_any(),
            None => Empty::new().into_any(),
        },
        Label::derived(move || title.clone()).style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(theme::FG_SUBTLE)
                .text_ellipsis()
                .min_width(0.0)
                .selectable(false)
        }),
        Label::derived(move || if count == 0 { String::new() } else { count.to_string() }).style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .color(theme::FG_SUBTLE)
                .margin_left(6.0)
                .selectable(false)
        }),
    ))
    .style(move |s| {
        let base = s
            .items_center()
            .width_full()
            .padding_horiz(16.0)
            .padding_top(14.0)
            .padding_bottom(4.0)
            .selectable(false);
        if collapsed.is_some() {
            base.hover(|s| s.cursor(CursorStyle::Pointer))
        } else {
            base
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_has_no_state_glyph_and_a_closed_issue_has_one() {
        let mut i = Item::new("criccomini/perch", "2026-09-18T10:00:00Z".parse().unwrap());
        assert_eq!(state_of(&i), None);
        i.subject = Some(ItemSubject::Issue(perch_github::Issue {
            number: 1,
            title: "t".into(),
            body: String::new(),
            state: "closed".into(),
            state_reason: Some("not_planned".into()),
            author_login: String::new(),
            author_avatar_url: String::new(),
            labels: vec![],
            assignees: vec![],
            comments: 0,
            created_at: None,
            updated_at: "2026-09-18T10:00:00Z".parse().unwrap(),
            repo_full_name: "r".into(),
        }));
        assert_eq!(state_of(&i), Some(SubjectState::IssueNotPlanned));
    }
}
