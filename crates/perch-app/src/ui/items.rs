//! Item rows: a pull request, an issue, or a branch of the reader's own,
//! in the sidebar's flat list.
//!
//! One row draws every shape an item can have, in the shape `nav_row`
//! gives every row of the drawer. The glyph and the byline come from which
//! parts the item has, never from a stored kind. The row's menu has every
//! entry on every row, with the ones that cannot apply greyed rather than
//! hidden, so it is the same size and order everywhere.

// TextInput::on_enter is deprecated in favour of a listener; the name says
// what it does, so it stays until the listener form is the only one.
#![allow(deprecated)]

use std::rc::Rc;

use floem::menu::Menu;
use floem::muda::accelerator::{Accelerator, Code as KeyCode};
use floem::prelude::*;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{Container, Decorators, Empty, Label, Stack, TextInput, dyn_container};
use perch_github::{Item, ItemSubject, SubjectState};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::state::AppState;
use crate::theme;
use crate::ui::nav_row;
use crate::ui::widgets::{
    copy_to_clipboard, icon_button, open_in_browser, open_in_terminal, reveal_in_finder, sidebar_glyph,
    state_glyph, text_button,
};

/// Opening an item. Shared by every row.
pub type OpenItem = Rc<dyn Fn(&Item)>;

/// The state glyph an item shows: the pull request or issue state icon,
/// or a branch for a checkout with no subject.
pub fn state_of(item: &Item) -> Option<SubjectState> {
    item.subject.as_ref().map(|subject| subject.subject_state())
}

/// One item as the sidebar draws it.
pub fn item_row(
    state: AppState,
    backend: Rc<Backend>,
    item: Item,
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

    let menu_item = item.clone();
    let menu_backend = backend.clone();
    let actions_backend = backend.clone();
    let under_backend = backend.clone();
    let row_item = item.clone();
    let title_item = item.clone();
    let row_debug_name = format!("item row:{}", item.id);

    // The click and the context menu are on the row, and both land on the
    // deepest view under the pointer, so everything drawn in the row lets
    // the pointer through to it: the content stack is pointer_events_none
    // and every view in it inherits that, except the controls, which set
    // it back: the rename box and the hover buttons.
    let title = dyn_container(
        {
            let id = item.id.clone();
            move || state.renaming.with(|r| r.as_deref() == Some(id.as_str()))
        },
        move |renaming| {
            if renaming {
                rename_box(state, backend.clone(), title_item.clone()).into_any()
            } else {
                nav_row::title_label(title_item.title().to_string(), archived, false).into_any()
            }
        },
    )
    .style(|s| s.width_full().min_width(0.0));
    let content = Stack::horizontal((
        nav_row::glyph_box(glyph(&item)),
        nav_row::lines(
            title,
            row_trailing(state, actions_backend, row_item).style(|s| s.pointer_events_auto()),
            byline(state, &item),
        ),
    ))
    .style(|s| s.items_center().width_full().min_width(0.0).pointer_events_none());

    let row = Container::new(content)
        .debug_name(row_debug_name)
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
        .style(move |s| nav_row::row_style(s, selected()));
    let row = Container::new(row).style(nav_row::row_gutter);

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
    match state {
        Some(state) => {
            state_glyph(state, if muted { theme::NAV_SUBTLE } else { icons::state_color(state) })
                .into_any()
        }
        None => sidebar_glyph(SidebarIcon::Branch, if muted { theme::NAV_SUBTLE } else { theme::NAV_MUTED })
            .into_any(),
    }
}

/// Whose face the byline carries: the author of the pull request or the
/// issue, or the reader's own on a branch of their own.
fn author(state: AppState, item: &Item) -> (impl Fn() -> String + 'static, String) {
    let own = match &item.subject {
        Some(ItemSubject::PullRequest(pr)) => Some((pr.author_login.clone(), pr.author_avatar_url.clone())),
        Some(ItemSubject::Issue(issue)) => Some((issue.author_login.clone(), issue.author_avatar_url.clone())),
        None => None,
    };
    match own {
        Some((login, url)) => (Box::new(move || url.clone()) as Box<dyn Fn() -> String>, login),
        // The viewer arrives after the rows do, so the face follows it.
        None => (
            Box::new(move || state.viewer.with(|v| v.as_ref().map(|(_, url)| url.clone()).unwrap_or_default()))
                as Box<dyn Fn() -> String>,
            state.viewer.with_untracked(|v| v.as_ref().map(|(login, _)| login.clone()).unwrap_or_default()),
        ),
    }
}

/// The repository, then the number, then the churn: what Codex's byline
/// says of a branch. A branch with no pull request or issue has no
/// number, and its name is already the title.
fn byline(state: AppState, item: &Item) -> impl IntoView {
    let (avatar_url, login) = author(state, item);
    // A checkout says how far it has gone from its base once the scan has
    // measured it; a pull request says what GitHub counts until then.
    let churn = item
        .checkout
        .as_ref()
        .and_then(|c| c.churn)
        .or_else(|| item.pull_request().map(|pr| (pr.additions, pr.deletions)));
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
    nav_row::byline(
        state,
        avatar_url,
        login,
        item.repo_full_name.clone(),
        nav_row::reference(item.subject.as_ref().map(|subject| subject.number())),
        nav_row::chips(state, checks, review),
        nav_row::churn(churn),
    )
}

/// The title's right end: the pin, the bookmark and the age, or, while
/// the pointer is over the row, Save and the menu in their place.
fn row_trailing(state: AppState, backend: Rc<Backend>, item: Item) -> impl IntoView {
    let id = item.id.clone();
    let hovering = move || state.hovered_item.with(|h| h.as_deref() == Some(id.as_str()));
    let saved = item.saved;
    let pinned = item.is_pinned();
    let at = item.last_activity();

    dyn_container(hovering, move |hovered| {
        if !hovered {
            return Stack::horizontal((
                if pinned {
                    nav_row::mark(SidebarIcon::Pin, theme::FG_SUBTLE).into_any()
                } else {
                    Empty::new().into_any()
                },
                if saved {
                    nav_row::mark(SidebarIcon::BookmarkFilled, theme::ACCENT).into_any()
                } else {
                    Empty::new().into_any()
                },
                nav_row::age(state, at),
            ))
            .style(|s| s.items_center().flex_shrink(0.0_f32))
            .into_any();
        }
        let save_id = item.id.clone();
        let save_backend = backend.clone();
        let menu_item = item.clone();
        let menu_backend = backend.clone();
        Stack::horizontal((
            icon_button(
                move || if saved { SidebarIcon::BookmarkFilled } else { SidebarIcon::Bookmark },
                move || if saved { theme::ACCENT } else { theme::FG_SUBTLE },
                move || save_backend.mark_saved(&save_id, !saved),
            ),
            nav_row::more_button(move || item_menu(state, menu_backend.clone(), menu_item.clone())),
        ))
        .style(|s| s.items_center().flex_shrink(0.0_f32).margin_left(4.0))
        .into_any()
    })
    .style(|s| s.flex_shrink(0.0_f32))
}

fn key(code: KeyCode) -> Accelerator {
    Accelerator::new(None, code)
}

/// The pull request's or the issue's page on github.com. A branch of the
/// reader's own has none.
pub fn github_url(item: &Item) -> Option<String> {
    match &item.subject {
        Some(ItemSubject::PullRequest(pr)) => {
            Some(format!("https://github.com/{}/pull/{}", pr.repo_full_name, pr.number))
        }
        Some(ItemSubject::Issue(i)) => {
            Some(format!("https://github.com/{}/issues/{}", i.repo_full_name, i.number))
        }
        None => None,
    }
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
    let github_url = github_url(&item);
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
                let navigation_id = if is_issue {
                    b.start_work(&i)
                } else {
                    b.fork_item(&i)
                };
                state.begin_open("Cutting a branch", navigation_id);
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
                    let navigation_id = b.restore_item(&i);
                    if let Some(review) = state.review.get_untracked() {
                        state.begin_item_review(
                            review,
                            "Checking out the files",
                            &i,
                            navigation_id,
                        );
                    } else {
                        state.expect_item_navigation(&i, navigation_id);
                    }
                } else {
                    b.archive_item(&i);
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
                // The one radius: the box stands in for a title inside a
                // row drawn at it, and a tighter one read as a foreign control.
                .border_radius(theme::RADIUS)
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
            s.font_size(theme::TEXT_META).font_weight(theme::weight_emphasis()).color(theme::FG).selectable(false)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_has_no_state_glyph_and_a_closed_issue_has_one() {
        let mut i = Item::new("criccomini/perch", "2026-09-18T10:00:00Z".parse().unwrap());
        assert_eq!(state_of(&i), None);
        i.subject = Some(ItemSubject::Issue(perch_github::Issue {
            node_id: String::new(),
            reactions: Default::default(),
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
