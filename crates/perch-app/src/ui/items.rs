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
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{Container, Decorators, Empty, Label, Stack, TextInput, dyn_container};
use perch_github::{Item, ItemSubject, SubjectState};

use crate::backend::{Backend, LossPrompt};
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
    let unread = unread(state, &item);

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
        {
            let unread = unread.clone();
            move |renaming| {
                if renaming {
                    rename_box(state, backend.clone(), title_item.clone()).into_any()
                } else {
                    nav_row::title_label(title_item.title().to_string(), archived, unread.clone())
                        .into_any()
                }
            }
        },
    )
    .style(|s| s.width_full().min_width(0.0));
    let content = Stack::horizontal((
        nav_row::glyph_box(glyph(&item)),
        nav_row::lines(
            title,
            row_trailing(state, actions_backend, row_item, unread)
                .style(|s| s.pointer_events_auto()),
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

    // What opens under the row: a Delete confirmation, or the backend
    // asking before an archive or delete loses something. Keyed on a
    // memo: the signals are written for every row, and each write would
    // otherwise rebuild every row's panel.
    let under_item = item.clone();
    let under = {
        let id = item.id.clone();
        Memo::new(move |_| {
            if let Some(prompt) = state
                .confirm_loss
                .with(|p| p.clone().filter(|p| p.item_id == id))
            {
                Under::Loss(prompt)
            } else if state.confirm_delete.with(|d| d.as_deref() == Some(id.as_str())) {
                Under::Delete
            } else {
                Under::Nothing
            }
        })
    };
    Stack::vertical((
        row,
        dyn_container(
            move || under.get(),
            move |under| match under {
                Under::Delete => {
                    delete_confirmation(state, under_backend.clone(), under_item.clone()).into_any()
                }
                Under::Loss(prompt) => {
                    loss_confirmation(state, under_backend.clone(), under_item.title().to_string(), prompt)
                        .into_any()
                }
                Under::Nothing => Empty::new().into_any(),
            },
        )
        .style(|s| s.width_full()),
    ))
    .style(|s| s.flex_col().width_full())
}

/// Whether the inbox has an unread notification about the item's pull
/// request or issue: what bolds the title and draws the dot, as the
/// Notifications scope's row for the same thread has them. Read from
/// the inbox each time, so viewing the item, which reads the thread,
/// clears both without the row being rebuilt. A branch with no subject
/// has nothing to be unread.
fn unread(state: AppState, item: &Item) -> impl Fn() -> bool + Clone + 'static {
    let about = item
        .subject
        .as_ref()
        .map(|subject| (subject.repo_full_name().to_string(), subject.kind(), subject.number()));
    move || {
        about
            .as_ref()
            .is_some_and(|(repo, kind, number)| state.subject_unread(repo, *kind, *number))
    }
}

/// The notifications about the item's subject that GitHub has as read:
/// what Mark Unread puts the reminder back on. None on a branch, or on a
/// subject the inbox has nothing about.
fn read_threads(state: AppState, item: &Item) -> Vec<String> {
    let Some(subject) = item.subject.as_ref() else {
        return Vec::new();
    };
    state.notifications.with_untracked(|threads| {
        threads
            .iter()
            .filter(|thread| {
                !thread.unread
                    && thread.is_about(subject.repo_full_name(), subject.kind(), subject.number())
            })
            .map(|thread| thread.id.clone())
            .collect()
    })
}

pub(crate) fn glyph(item: &Item) -> impl IntoView {
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

/// The faces the byline stacks: who acted last on the pull request or
/// the issue once a look has found them, most recent first; its author
/// until then; the reader's own on a branch of their own.
fn faces(state: AppState, item: &Item) -> impl Fn() -> Vec<nav_row::Face> + 'static {
    let subject = item.subject.as_ref().map(|subject| {
        let author = match subject {
            ItemSubject::PullRequest(pr) => nav_row::Face::new(pr.author_avatar_url.clone(), pr.author_login.clone()),
            ItemSubject::Issue(issue) => nav_row::Face::new(issue.author_avatar_url.clone(), issue.author_login.clone()),
        };
        ((subject.repo_full_name().to_string(), subject.number()), author)
    });
    move || match &subject {
        Some((key, author)) => state
            .participants
            .with(|found| found.get(key).filter(|p| !p.is_empty()).map(|p| nav_row::participant_faces(p)))
            .unwrap_or_else(|| vec![author.clone()]),
        // The viewer arrives after the rows do, so the face follows it.
        None => state.viewer.with(|v| {
            v.as_ref()
                .map(|(login, url)| vec![nav_row::Face::new(url.clone(), login.clone())])
                .unwrap_or_else(|| vec![nav_row::Face::new("", "")])
        }),
    }
}

/// Who acted last, the repository, then the number, then the churn: what
/// Codex's byline says of a branch. A branch with no pull request or
/// issue has no number, and its name is already the title.
fn byline(state: AppState, item: &Item) -> impl IntoView {
    let faces = faces(state, item);
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
        faces,
        item.repo_full_name.clone(),
        nav_row::reference(item.subject.as_ref().map(|subject| subject.number())),
        Stack::horizontal((
            nav_row::chips(state, checks, review),
            nav_row::note(item.divergence().map(|d| d.label()), theme::DRAFT_YELLOW),
        ))
        .style(|s| s.items_center().min_width(0.0)),
        nav_row::churn(churn),
    )
}

/// The title's right end: the pin, the unread dot and the age, or, while
/// the pointer is over the row, Pin and the menu in their place.
fn row_trailing(
    state: AppState,
    backend: Rc<Backend>,
    item: Item,
    unread: impl Fn() -> bool + Clone + 'static,
) -> impl IntoView {
    let id = item.id.clone();
    // A memo: the pointer moving from one row to the next writes
    // `hovered_item` for every row, and scrolling moves rows under the
    // pointer; keyed on the signal, each write would rebuild every row's
    // trailing views and lay the list out again.
    let hovering = Memo::new(move |_| state.hovered_item.with(|h| h.as_deref() == Some(id.as_str())));
    let pinned = item.is_pinned();
    let at = item.last_activity();

    dyn_container(move || hovering.get(), move |hovered| {
        if !hovered {
            return Stack::horizontal((
                if pinned {
                    nav_row::mark(SidebarIcon::Pin, theme::FG_SUBTLE).into_any()
                } else {
                    Empty::new().into_any()
                },
                nav_row::unread_dot(unread.clone()),
                nav_row::age(state, at),
            ))
            .style(|s| s.items_center().flex_shrink(0.0_f32))
            .into_any();
        }
        let pin_id = item.id.clone();
        let pin_backend = backend.clone();
        let menu_item = item.clone();
        let menu_backend = backend.clone();
        Stack::horizontal((
            icon_button(
                move || SidebarIcon::Pin,
                move || if pinned { theme::ACCENT } else { theme::FG_SUBTLE },
                move || pin_backend.pin_item(&pin_id, !pinned),
            )
            .debug_name("row pin"),
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
        // Put the unread reminder back on the subject's notification,
        // the way the Notifications scope's row does. Greyed while there
        // is none to put it on: a branch, a subject the inbox has nothing
        // about, or one still unread.
        let read = read_threads(state, &item);
        let b = backend.clone();
        menu = menu.item("Mark Unread", move |m| {
            let (b, read) = (b.clone(), read.clone());
            m.enabled(!read.is_empty())
                .accelerator(key(KeyCode::KeyU))
                .action(move || {
                    for thread_id in &read {
                        b.mark_notification_unread(thread_id);
                    }
                })
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
    menu = menu.separator();

    {
        let (b, i) = (backend.clone(), id.clone());
        // A worktree Perch made can be removed, and put back once it has
        // gone. One the root scan found is the reader's own checkout, and
        // comes and goes with it.
        let on_disk = crate::items::in_worktrees(&item);
        let owned = item.checkout.as_ref().is_some_and(|c| c.owned);
        menu = menu.item(if on_disk { "Remove Worktree" } else { "Restore Worktree" }, move |m| {
            let (b, i) = (b.clone(), i.clone());
            m.enabled(owned).accelerator(key(KeyCode::KeyA)).action(move || {
                if !on_disk {
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
                    state.confirm_loss.set(None);
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
                state.confirm_loss.set(None);
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
                backend.delete_item(&id, false);
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

/// What is open under a row.
#[derive(Clone, PartialEq)]
enum Under {
    Nothing,
    Delete,
    Loss(LossPrompt),
}

/// What a loss prompt says: the question, then what goes.
fn loss_text(title: &str, prompt: &LossPrompt) -> (String, String, &'static str) {
    let commits = match prompt.commits {
        1 => "1 commit".to_string(),
        n => format!("{n} commits"),
    };
    (
        format!("Delete {title} and its commits?"),
        format!(
            "{} has {commits} that no other branch, tag or remote Perch knows of holds. \
             Deleting the branch loses them.",
            prompt.branch
        ),
        "Delete anyway",
    )
}

/// The backend's question before a delete that would lose what git
/// cannot bring back.
fn loss_confirmation(state: AppState, backend: Rc<Backend>, title: String, prompt: LossPrompt) -> impl IntoView {
    let (question, what, go) = loss_text(&title, &prompt);
    let id = prompt.item_id.clone();
    Stack::vertical((
        Label::derived(move || question.clone()).style(|s| {
            s.font_size(theme::TEXT_META).font_weight(theme::weight_emphasis()).color(theme::FG).selectable(false)
        }),
        Label::derived(move || what.clone()).style(|s| {
            s.font_size(theme::TEXT_BADGE).color(theme::FG_MUTED).text_wrap().width_full().margin_top(2.0)
        }),
        Stack::horizontal((
            text_button(
                move || go.to_string(),
                SidebarIcon::Trash,
                move || {
                    state.confirm_loss.set(None);
                    backend.delete_item(&id, true);
                },
            ),
            text_button(|| "Cancel".to_string(), SidebarIcon::Close, move || state.confirm_loss.set(None)),
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
    fn a_loss_prompt_names_what_goes() {
        let prompt = LossPrompt {
            item_id: "i".into(),
            branch: "chris/work".into(),
            commits: 1,
        };
        let (question, what, go) = loss_text("Work", &prompt);
        assert_eq!(question, "Delete Work and its commits?");
        assert!(what.starts_with("chris/work has 1 commit that"), "{what}");
        assert_eq!(go, "Delete anyway");
    }

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
