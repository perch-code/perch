//! The Inbox's rows: GitHub's notifications, and a row for each worktree
//! without a pull request where an agent run or a review ended.
//!
//! The order is the order from GitHub, with the local rows fitted in by
//! time; filtering (`crate::inbox`) only removes rows from that order.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate};
use floem::views::{
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, ScrollExt, Stack,
};
use jiff::Timestamp;
use perch_github::{CheckRunState, Item, ItemActivity, SubjectKind, SubjectState, Thread};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::inbox::{self, InboxRow};
use crate::items::InboxView;
use crate::review::start_work;
use crate::snooze;
use crate::state::AppState;
use crate::theme;
use crate::timefmt;
use crate::ui::items::OpenItem;
use crate::ui::menu::{Menu, MenuExt};
use crate::ui::nav_row;
use crate::ui::widgets::{icon_button, open_in_browser, sidebar_glyph, state_glyph};

/// The notification list under the current search, repository, and reason
/// filters. GitHub's ordering is retained.
pub fn notification_list(
    state: AppState,
    backend: Rc<Backend>,
    on_open: OpenItem,
) -> impl IntoView {
    let show = move || state.item_filters.with(|f| f.inbox.view);
    Stack::vertical((
        dyn_container(
            move || (visible_notifications(state).is_empty(), show()),
            |(empty, show)| {
                if empty {
                    Label::new(match show {
                        InboxView::Done => "No notifications marked Done",
                        InboxView::Snoozed => "No snoozed notifications",
                        InboxView::Inbox => "No matching notifications",
                    })
                    .style(|s| {
                        s.font_size(theme::TEXT_META)
                            .color(theme::nav_subtle())
                            .padding(24.0)
                    })
                    .into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        ),
        dyn_stack(
            move || {
                let away = show().is_put_away();
                // Done and Snoozed are Perch's lists, not in GitHub's
                // order, so their rows keep their own times.
                let listed = (!away).then(|| state.notifications.with(|t| inbox::listed_at(t)));
                visible_notifications(state).into_iter().map(move |row| {
                    let at = match &row {
                        InboxRow::Thread(thread) => listed
                            .as_ref()
                            .and_then(|listed| listed.get(&thread.id).copied())
                            .unwrap_or(thread.updated_at),
                        InboxRow::Local { activity, .. } => activity.at,
                    };
                    (row, away, at)
                })
            },
            |(row, away, at): &(InboxRow, bool, Timestamp)| match row {
                InboxRow::Thread(thread) => RowKey::Thread(
                    thread.id.clone(),
                    *away,
                    thread.unread,
                    // Done marks the revision the row holds.
                    thread.updated_at.to_string(),
                    at.to_string(),
                    thread.snoozed_until,
                    // The face and the state arrive after the row does.
                    thread.subject_snapshot.clone(),
                ),
                // A new run, a read, or a rename redraws it.
                InboxRow::Local { activity, item } => RowKey::Local(
                    item.id.clone(),
                    activity.at.to_string(),
                    activity.is_unread(),
                    item.title().to_string(),
                ),
            },
            move |(row, away, at)| match row {
                InboxRow::Thread(thread) => {
                    notification_row(state, backend.clone(), thread, away, at).into_any()
                }
                InboxRow::Local { activity, item } => {
                    local_row(state, backend.clone(), on_open.clone(), item, activity).into_any()
                }
            },
        )
        .style(|s| s.flex_col().width_full().gap(1.0)),
    ))
    .style(nav_row::list_style)
    .scroll()
    .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0))
}

/// What a row of the list is drawn again on.
#[derive(Clone, PartialEq, Eq, Hash)]
enum RowKey {
    Thread(
        String,
        bool,
        bool,
        String,
        String,
        Option<Timestamp>,
        Option<perch_github::SubjectSnapshot>,
    ),
    Local(String, String, bool, String),
}

/// The rows visible under the shared sidebar controls. Item-only settings
/// such as status, saved, and sort deliberately have no effect here.
pub fn visible_notifications(state: AppState) -> Vec<InboxRow> {
    state.visible_inbox()
}

/// The row for an item without a pull request where a run or a review
/// ended: its glyph and title as Worktrees draws them, the branch and what
/// happened in the byline. Opening it opens the item, which reads it.
fn local_row(
    state: AppState,
    backend: Rc<Backend>,
    on_open: OpenItem,
    item: Item,
    activity: ItemActivity,
) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let unread = activity.is_unread();
    let open_item = item.clone();
    let menu_item = item.clone();
    let menu_backend = backend.clone();
    let content = Stack::horizontal((
        nav_row::glyph_box(crate::ui::items::glyph(&item)),
        nav_row::lines(
            nav_row::title_label(item.title().to_string(), false, move || unread),
            local_trailing(state, hovered, backend, item.clone(), unread, activity.at)
                .style(|s| s.pointer_events_auto()),
            crate::ui::items::inbox_byline(state, &item, &activity.event),
        ),
    ))
    .style(|s| {
        s.items_center()
            .width_full()
            .min_width(0.0)
            .pointer_events_none()
    });
    let row = Container::new(content)
        .debug_name(format!("notification row:item:{}", item.id))
        .on_event_stop(el::Click, move |_, _| on_open(&open_item))
        .on_event_cont(el::PointerEnter, move |_, _| hovered.set(true))
        .on_event_cont(el::PointerLeave, move |_, _| hovered.set(false))
        .right_click_menu(move || local_menu(menu_backend.clone(), &menu_item, unread))
        .style(|s| nav_row::row_style(s, false));
    Container::new(row).style(nav_row::row_gutter)
}

/// A local row's right end: the unread dot and the age, or while the
/// pointer is over it, Done and the menu.
fn local_trailing(
    state: AppState,
    hovered: RwSignal<bool>,
    backend: Rc<Backend>,
    item: Item,
    unread: bool,
    at: Timestamp,
) -> impl IntoView {
    dyn_container(
        move || hovered.get(),
        move |hovered| {
            if !hovered {
                return Stack::horizontal((
                    nav_row::unread_dot(move || unread),
                    nav_row::age(state, at),
                ))
                .style(|s| s.items_center().flex_shrink(0.0_f32))
                .into_any();
            }
            let done_backend = backend.clone();
            let done_item = item.clone();
            let menu_backend = backend.clone();
            let menu_item = item.clone();
            Stack::horizontal((
                icon_button(
                    || SidebarIcon::Check,
                    theme::fg_subtle,
                    move || local_done(&done_backend, &done_item),
                )
                .debug_name("mark done"),
                nav_row::more_button(move || local_menu(menu_backend.clone(), &menu_item, unread)),
            ))
            .style(|s| s.items_center().flex_shrink(0.0_f32).margin_left(4.0))
            .into_any()
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

fn local_menu(backend: Rc<Backend>, item: &Item, unread: bool) -> Menu {
    let done_backend = backend.clone();
    let done_item = item.clone();
    let id = item.id.clone();
    Menu::new()
        .item("Done", move |entry| {
            let backend = done_backend.clone();
            let item = done_item.clone();
            entry
                .icon(SidebarIcon::Check)
                .action(move || local_done(&backend, &item))
        })
        .item("Mark Unread", move |entry| {
            let backend = backend.clone();
            let id = id.clone();
            entry
                .icon(SidebarIcon::Unread)
                .enabled(!unread)
                .action(move || backend.mark_item_activity_unread(&id))
        })
}

/// Done on a local row: it goes until the next run ends there. The item
/// and its worktree stay, and so does the item if it is open.
fn local_done(backend: &Backend, item: &Item) {
    backend.done_item_activity(&item.id);
}

/// A row of the inbox, or with `away` of View › Done or Snoozed, where
/// the row's action moves it back instead. `at` is the time its age shows.
fn notification_row(
    state: AppState,
    backend: Rc<Backend>,
    thread: Thread,
    away: bool,
    at: Timestamp,
) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let unread = thread.unread;
    let title = thread.subject.title.clone();
    let open_thread = thread.clone();
    let open_backend = backend.clone();
    let url = github_url(&thread);
    let opens_inside = matches!(
        &thread.subject.kind,
        SubjectKind::PullRequest | SubjectKind::Issue
    ) && thread.number_hint().is_some();
    let menu_thread = thread.clone();
    let menu_backend = backend.clone();
    let action_thread = thread.clone();
    let action_backend = backend.clone();

    let content = Stack::horizontal((
        nav_row::glyph_box(notification_glyph(&thread)),
        nav_row::lines(
            nav_row::title_label(title, false, move || unread),
            notification_trailing(state, hovered, action_backend, action_thread, away, at)
                .style(|s| s.pointer_events_auto()),
            byline(state, &thread),
        ),
    ))
    .style(|s| {
        s.items_center()
            .width_full()
            .min_width(0.0)
            .pointer_events_none()
    });

    let row = Container::new(content)
        .debug_name(format!("notification row:{}", thread.id))
        .on_event_stop(el::Click, move |_, _| {
            let navigation_id = open_backend.open_notification(&open_thread);
            if opens_inside {
                if let Some(navigation_id) = navigation_id {
                    state.begin_open("Reading the notification", navigation_id);
                    if let Some(review) = state.review.get_untracked() {
                        crate::review::notified::follow(review, &open_thread);
                    }
                }
            } else {
                if let Some(url) = &url {
                    open_in_browser(url);
                }
            }
        })
        .on_event_cont(el::PointerEnter, move |_, _| hovered.set(true))
        .on_event_cont(el::PointerLeave, move |_, _| hovered.set(false))
        .right_click_menu(move || {
            notification_menu(state, menu_backend.clone(), &menu_thread, away)
        })
        .style(|s| nav_row::row_style(s, false));
    Container::new(row).style(nav_row::row_gutter)
}

/// The glyph GitHub's inbox draws for the subject: a pull request or an
/// issue in the state a look found it in, and open until one has, since
/// that is what most of an inbox is; a workflow run by the verdict its
/// title announces; and a muted mark for the rest.
fn notification_glyph(thread: &Thread) -> impl IntoView {
    let (glyph, color): (_, fn() -> floem::peniko::Color) = match &thread.subject.kind {
        SubjectKind::PullRequest | SubjectKind::Issue => {
            let open = if thread.subject.kind == SubjectKind::PullRequest {
                SubjectState::PrOpen
            } else {
                SubjectState::IssueOpen
            };
            let state = thread
                .subject_snapshot
                .as_ref()
                .map_or(open, |snapshot| snapshot.state);
            return state_glyph(state, move || icons::state_color(state)).into_any();
        }
        SubjectKind::CheckSuite => match perch_github::announced_verdict(&thread.subject.title) {
            Some(CheckRunState::Failure) => (SidebarIcon::Close, theme::closed),
            Some(CheckRunState::Success) => (SidebarIcon::Check, theme::open),
            Some(CheckRunState::Cancelled) => (SidebarIcon::Skip, theme::nav_muted),
            _ => (SidebarIcon::Check, theme::nav_muted),
        },
        SubjectKind::Commit => (SidebarIcon::Branch, theme::nav_muted),
        SubjectKind::Discussion => (SidebarIcon::ThreadHollow, theme::nav_muted),
        SubjectKind::RepositoryVulnerabilityAlert => (SidebarIcon::Warning, theme::nav_muted),
        SubjectKind::Release => (SidebarIcon::Bookmark, theme::nav_muted),
        SubjectKind::Other(_) => (SidebarIcon::Comment, theme::nav_muted),
    };
    sidebar_glyph(glyph, color).into_any()
}

/// Who acted last on the subject, the repository, the number, and on a
/// pull request its review pill and its churn, once a look has found
/// them: the shape an item's byline has. Until a look has found who acted
/// there, whoever opened it; the repository's face until a look has found
/// even that, and on a subject nobody opened: a workflow run, a release.
fn byline(state: AppState, thread: &Thread) -> impl IntoView {
    let faces = match thread.subject_snapshot.as_ref() {
        Some(snapshot)
            if snapshot
                .participants
                .as_ref()
                .is_some_and(|p| !p.is_empty()) =>
        {
            nav_row::participant_faces(snapshot.participants.as_deref().unwrap_or_default())
        }
        Some(snapshot) if !snapshot.author_login.is_empty() => vec![nav_row::Face::new(
            snapshot.author_avatar_url.clone(),
            snapshot.author_login.clone(),
        )],
        _ => vec![nav_row::Face::new(
            thread.repository.owner_avatar_url.clone(),
            thread
                .repository
                .full_name
                .split('/')
                .next()
                .unwrap_or_default(),
        )],
    };
    let snapshot = thread.subject_snapshot.as_ref();
    let review = snapshot
        .is_some_and(|snapshot| snapshot.review_requested)
        .then_some(perch_github::ReviewDecision::ReviewRequested);
    nav_row::byline(
        state,
        move || faces.clone(),
        thread.repository.full_name.clone(),
        nav_row::reference(number(thread)),
        nav_row::chips(state, None, review),
        nav_row::churn(snapshot.and_then(|snapshot| snapshot.churn)),
    )
}

/// The title's right end: the unread dot and the age, or on a snoozed row
/// when it comes back; while the pointer is over the row, Done, Snooze
/// and the menu in their place, or on a Done or snoozed row Move to
/// Inbox and the menu.
fn notification_trailing(
    state: AppState,
    hovered: RwSignal<bool>,
    backend: Rc<Backend>,
    thread: Thread,
    away: bool,
    at: Timestamp,
) -> impl IntoView {
    let unread = thread.unread;
    let snoozed_until = thread.snoozed_until;
    dyn_container(
        move || hovered.get(),
        move |hovered| {
            if !hovered {
                let when = match snoozed_until {
                    Some(until) => snoozed_until_label(state, until).into_any(),
                    None => nav_row::age(state, at).into_any(),
                };
                return Stack::horizontal((nav_row::unread_dot(move || unread), when))
                    .style(|s| s.items_center().flex_shrink(0.0_f32))
                    .into_any();
            }
            let done_backend = backend.clone();
            let done_thread = thread.clone();
            let snooze_backend = backend.clone();
            let snooze_thread = thread.clone();
            let menu_backend = backend.clone();
            let menu_thread = thread.clone();
            let actions = if away {
                icon_button(
                    || SidebarIcon::Undo,
                    theme::fg_subtle,
                    move || done_backend.restore_notification(&done_thread.id),
                )
                .debug_name("move to inbox")
                .into_any()
            } else {
                Stack::horizontal((
                    icon_button(
                        || SidebarIcon::Check,
                        theme::fg_subtle,
                        move || done(state, &done_backend, &done_thread),
                    )
                    .debug_name("mark done"),
                    nav_row::menu_button(SidebarIcon::Clock, "snooze button", move || {
                        snooze_menu(state, snooze_backend.clone(), &snooze_thread)
                    }),
                ))
                .style(|s| s.items_center())
                .into_any()
            };
            Stack::horizontal((
                actions,
                nav_row::more_button(move || {
                    notification_menu(state, menu_backend.clone(), &menu_thread, away)
                }),
            ))
            .style(|s| s.items_center().flex_shrink(0.0_f32).margin_left(4.0))
            .into_any()
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

/// When a snoozed row comes back, in the age's place: "6:00 PM",
/// "tomorrow 8:00 AM", "Sat 8:00 AM".
fn snoozed_until_label(state: AppState, until: jiff::Timestamp) -> impl IntoView {
    // A memo, so the label is set again only when its text changes.
    let text = Memo::new(move |_| timefmt::ahead(until, state.now.get()));
    Stack::horizontal((
        nav_row::mark(SidebarIcon::Clock, theme::nav_muted),
        Label::derived(move || text.get())
            .debug_name("snoozed until")
            .style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::nav_muted())
                    .flex_shrink(0.0_f32)
                    .margin_left(4.0)
            }),
    ))
    .style(|s| s.items_center().flex_shrink(0.0_f32))
}

/// Mark the thread Done. The reader is finished with it, so if the main
/// pane has its pull request or issue open, that closes, as going back
/// to the sidebar would.
fn done(state: AppState, backend: &Backend, thread: &Thread) {
    backend.done_notification(&thread.id);
    close_if_viewing(state, backend, thread);
}

/// Mark N Done: every row given, as each row's Done would, the threads in
/// one request to the backend so a failure is told once.
pub(crate) fn done_all(state: AppState, backend: &Backend, rows: &[InboxRow]) {
    let threads: Vec<&Thread> = rows.iter().filter_map(InboxRow::thread).collect();
    for row in rows {
        if let InboxRow::Local { item, .. } = row {
            backend.done_item_activity(&item.id);
        }
    }
    backend.done_notifications(threads.iter().map(|thread| thread.id.clone()).collect());
    if let Some(viewing) = threads
        .iter()
        .find(|thread| state.is_viewing_thread(thread))
    {
        close_if_viewing(state, backend, viewing);
    }
}

/// Snooze the thread until `until`. The reader has put it off, so its
/// pull request or issue closes as it would for Done.
fn snooze(state: AppState, backend: &Backend, thread: &Thread, until: jiff::Timestamp) {
    backend.snooze_notification(&thread.id, until);
    close_if_viewing(state, backend, thread);
}

fn close_if_viewing(state: AppState, backend: &Backend, thread: &Thread) {
    if !state.is_viewing_thread(thread) {
        return;
    }
    if let Some(review) = state.review.get_untracked() {
        let navigation_id = backend.close_review();
        state.begin_close(review, navigation_id);
    }
}

/// Gmail's snooze choices, each with when it ends, as of the moment the
/// menu opens.
fn snooze_menu(state: AppState, backend: Rc<Backend>, thread: &Thread) -> Menu {
    let mut menu = Menu::new();
    for choice in snooze::choices(jiff::Timestamp::now()) {
        let backend = backend.clone();
        let thread = thread.clone();
        menu = menu.item(choice.label, move |item| {
            item.detail(choice.when)
                .action(move || snooze(state, &backend, &thread, choice.until))
        });
    }
    menu
}

fn notification_menu(state: AppState, backend: Rc<Backend>, thread: &Thread, away: bool) -> Menu {
    if away {
        let id = thread.id.clone();
        return Menu::new().item("Move to Inbox", move |item| {
            let backend = backend.clone();
            let id = id.clone();
            item.icon(SidebarIcon::Inbox)
                .action(move || backend.restore_notification(&id))
        });
    }
    let unread_backend = backend.clone();
    let unread_id = thread.id.clone();
    let unread = thread.unread;
    let done_backend = backend.clone();
    let done_thread = thread.clone();
    let unsubscribe_backend = backend.clone();
    let unsubscribe_id = thread.id.clone();
    let snooze_backend = backend.clone();
    let snooze_thread = thread.clone();
    let mut menu = Menu::new();
    // A pull request's or an issue's row starts work on it, greyed once a
    // look has found it merged or closed. Other kinds have nothing to cut.
    if start_work::thread_subject(thread).is_some() {
        let open = start_work::thread_open_for_work(thread);
        let thread = thread.clone();
        menu = menu
            .item("Start Work", move |item| {
                let thread = thread.clone();
                item.icon(SidebarIcon::Branch)
                    .enabled(open)
                    .action(move || {
                        start_work::ask(
                            state,
                            start_work::Pending::Thread(Box::new(thread.clone())),
                        )
                    })
            })
            .separator();
    }
    menu.item("Done", move |item| {
        let backend = done_backend.clone();
        let thread = done_thread.clone();
        item.icon(SidebarIcon::Check)
            .action(move || done(state, &backend, &thread))
    })
    .submenu("Snooze", move |_| {
        snooze_menu(state, snooze_backend, &snooze_thread).icon(SidebarIcon::Clock)
    })
    .item("Mark Unread", move |item| {
        let backend = unread_backend.clone();
        let id = unread_id.clone();
        item.icon(SidebarIcon::Unread)
            .enabled(!unread)
            .action(move || backend.mark_notification_unread(&id))
    })
    .item("Unsubscribe", move |item| {
        let backend = unsubscribe_backend.clone();
        let id = unsubscribe_id.clone();
        item.icon(SidebarIcon::BellOff)
            .action(move || backend.unsubscribe_notification(&id))
    })
}

/// The number GitHub shows for the subject: a pull request's, an issue's
/// or a discussion's. A release's URL ends in an id nobody sees, and a
/// commit's in its hash.
fn number(thread: &Thread) -> Option<u64> {
    match thread.subject.kind {
        SubjectKind::PullRequest | SubjectKind::Issue | SubjectKind::Discussion => {
            thread.number_hint()
        }
        _ => None,
    }
}

/// A GitHub web URL derived without trusting notification text as a URL.
/// Subjects without a reliable page fall back to their repository. A
/// workflow run opens the run once a look has found it, and the
/// repository's Actions page until then.
pub(crate) fn github_url(thread: &Thread) -> Option<String> {
    let repo = &thread.repository.full_name;
    let repository = format!("https://github.com/{repo}");
    match &thread.subject.kind {
        SubjectKind::PullRequest => thread
            .number_hint()
            .map(|number| format!("https://github.com/{repo}/pull/{number}"))
            .or(Some(repository)),
        SubjectKind::Issue => thread
            .number_hint()
            .map(|number| format!("https://github.com/{repo}/issues/{number}"))
            .or(Some(repository)),
        SubjectKind::Discussion => thread
            .number_hint()
            .map(|number| format!("https://github.com/{repo}/discussions/{number}"))
            .or(Some(repository)),
        SubjectKind::Commit => {
            let sha = thread
                .subject
                .url
                .as_deref()
                .and_then(|url| url.rsplit('/').next());
            sha.filter(|sha| !sha.is_empty())
                .map(|sha| format!("https://github.com/{repo}/commit/{sha}"))
                .or(Some(repository))
        }
        SubjectKind::Release => Some(format!("{repository}/releases")),
        SubjectKind::CheckSuite => thread
            .run_snapshot
            .as_ref()
            .map(|run| run.html_url.clone())
            .or_else(|| Some(format!("{repository}/actions"))),
        SubjectKind::RepositoryVulnerabilityAlert => {
            Some(format!("{repository}/security/dependabot"))
        }
        SubjectKind::Other(_) => Some(repository),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_github::{Reason, Repository, Subject};

    fn thread(
        id: &str,
        repo: &str,
        title: &str,
        kind: SubjectKind,
        reason: Reason,
        unread: bool,
    ) -> Thread {
        let tail = if matches!(&kind, SubjectKind::Commit) {
            "abc123"
        } else {
            id
        };
        Thread {
            id: id.into(),
            unread,
            local_unread: false,
            local_read: false,
            reason,
            updated_at: "2026-09-19T12:00:00Z".parse().unwrap(),
            last_read_at: (!unread).then(|| "2026-09-19T11:00:00Z".parse().unwrap()),
            subject: Subject {
                title: title.into(),
                kind,
                url: Some(format!("https://api.github.com/repos/{repo}/issues/{tail}")),
                latest_comment_url: None,
            },
            repository: Repository {
                full_name: repo.into(),
                owner_avatar_url: String::new(),
            },
            subject_snapshot: None,
            run_snapshot: None,
            snoozed_until: None,
        }
    }

    /// Start Work is on a pull request's or an issue's row, and greyed
    /// once a look has found it merged or closed. A workflow run has
    /// nothing to cut from.
    #[test]
    fn work_starts_from_a_pull_request_or_an_issue_still_open() {
        use crate::review::start_work::{thread_open_for_work, thread_subject};
        use perch_github::{ItemKind, SubjectSnapshot, SubjectState};

        let mut pull = thread(
            "42",
            "org/repo",
            "Pull",
            SubjectKind::PullRequest,
            Reason::Mention,
            true,
        );
        assert_eq!(thread_subject(&pull), Some((ItemKind::PullRequest, 42)));
        assert!(
            thread_open_for_work(&pull),
            "not looked at yet: the backend judges"
        );
        let look = |state| SubjectSnapshot {
            author_login: "rohan".into(),
            author_avatar_url: String::new(),
            state,
            churn: None,
            review_requested: false,
            participants: None,
            at: "2026-09-19T12:00:00Z".parse().unwrap(),
        };
        for (state, open) in [
            (SubjectState::PrOpen, true),
            (SubjectState::PrDraft, true),
            (SubjectState::PrClosed, true),
            (SubjectState::PrMerged, false),
            (SubjectState::IssueOpen, true),
            (SubjectState::IssueClosed, false),
            (SubjectState::IssueNotPlanned, false),
        ] {
            pull.subject_snapshot = Some(look(state));
            assert_eq!(thread_open_for_work(&pull), open, "{state:?}");
        }

        let issue = thread(
            "7",
            "org/repo",
            "Issue",
            SubjectKind::Issue,
            Reason::Assign,
            true,
        );
        assert_eq!(thread_subject(&issue), Some((ItemKind::Issue, 7)));
        let run = thread(
            "9",
            "org/repo",
            "Run",
            SubjectKind::CheckSuite,
            Reason::CiActivity,
            true,
        );
        assert_eq!(thread_subject(&run), None);
        let talk = thread(
            "3",
            "org/repo",
            "Talk",
            SubjectKind::Discussion,
            Reason::Mention,
            true,
        );
        assert_eq!(thread_subject(&talk), None);
    }

    #[test]
    fn the_byline_numbers_only_what_github_numbers() {
        let pull = thread(
            "42",
            "org/repo",
            "Pull",
            SubjectKind::PullRequest,
            Reason::Mention,
            true,
        );
        assert_eq!(number(&pull), Some(42));
        let mut release = thread(
            "246813",
            "org/repo",
            "v1.0",
            SubjectKind::Release,
            Reason::Subscribed,
            true,
        );
        release.subject.url = Some("https://api.github.com/repos/org/repo/releases/246813".into());
        assert_eq!(number(&release), None);
        let run = thread(
            "9",
            "org/repo",
            "Run",
            SubjectKind::CheckSuite,
            Reason::CiActivity,
            true,
        );
        assert_eq!(number(&run), None);
    }

    #[test]
    fn web_urls_use_specific_subject_pages_or_the_repository_fallback() {
        let pull = thread(
            "42",
            "org/repo",
            "Pull",
            SubjectKind::PullRequest,
            Reason::Mention,
            true,
        );
        assert_eq!(
            github_url(&pull).as_deref(),
            Some("https://github.com/org/repo/pull/42")
        );
        let commit = thread(
            "9",
            "org/repo",
            "Commit",
            SubjectKind::Commit,
            Reason::Subscribed,
            false,
        );
        assert_eq!(
            github_url(&commit).as_deref(),
            Some("https://github.com/org/repo/commit/abc123")
        );
        let release = thread(
            "8",
            "org/repo",
            "Release",
            SubjectKind::Release,
            Reason::Subscribed,
            true,
        );
        assert_eq!(
            github_url(&release).as_deref(),
            Some("https://github.com/org/repo/releases")
        );
    }

    #[test]
    fn a_workflow_run_opens_its_run_once_found_and_the_actions_page_until_then() {
        let mut run = thread(
            "5",
            "org/repo",
            "CI workflow run failed for main branch",
            SubjectKind::CheckSuite,
            Reason::CiActivity,
            true,
        );
        assert_eq!(
            github_url(&run).as_deref(),
            Some("https://github.com/org/repo/actions")
        );
        run.run_snapshot = Some(perch_github::RunSnapshot {
            html_url: "https://github.com/org/repo/actions/runs/35614559713".into(),
            at: run.updated_at,
        });
        assert_eq!(
            github_url(&run).as_deref(),
            Some("https://github.com/org/repo/actions/runs/35614559713")
        );
    }
}
