//! The Inbox's rows: GitHub's notifications, and a row for each worktree
//! without a pull request where an agent run or a review ended.
//!
//! The order is the order from GitHub, with the local rows fitted in by
//! time; filtering (`crate::inbox`) only removes rows from that order.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use floem::prelude::*;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate};
use floem::views::{
    dyn_container, virtual_stack, Container, Decorators, Empty, Label, ScrollExt, Stack,
    VirtualVector,
};
use jiff::Timestamp;
use perch_github::{CheckRunState, Item, ItemActivity, SubjectKind, SubjectState, Thread};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::inbox::{self, InboxRow};
use crate::items::InboxView;
use crate::keymap::Action;
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
///
/// Only the rows in view are built: an inbox can hold thousands, and every
/// row is a stack of labels, faces and a glyph.
pub fn notification_list(
    state: AppState,
    backend: Rc<Backend>,
    on_open: OpenItem,
) -> impl IntoView {
    let show = move || state.item_filters.with(|f| f.inbox.view);
    // The virtual stack reads its rows again at every step of a scroll, so
    // they are filtered once here, not there.
    let rows = Memo::new(move |_| Rows(Arc::new(placed_rows(state, show()))));
    Stack::vertical((
        dyn_container(
            move || (rows.with(|r| r.0.is_empty()), show()),
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
        virtual_stack(
            move || rows.get(),
            |(row, away, at): &PlacedRow| match row {
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
                // A new run, a read, a rename, or a close redraws it.
                InboxRow::Local { activity, item } => RowKey::Local(
                    item.id.clone(),
                    activity.at.to_string(),
                    activity.is_unread(),
                    item.title().to_string(),
                    crate::ui::items::state_of(item),
                ),
            },
            move |(row, away, at)| {
                let row = match row {
                    InboxRow::Thread(thread) => {
                        notification_row(state, backend.clone(), thread, away, at).into_any()
                    }
                    InboxRow::Local { activity, item } => {
                        local_row(state, backend.clone(), on_open.clone(), item, activity)
                            .into_any()
                    }
                };
                // A virtual stack places rows by their size alone and takes
                // no gap, so the hairline between rows is part of each.
                Container::new(row).style(|s| {
                    s.width_full()
                        .height(ROW_PITCH)
                        .padding_bottom(ROW_PITCH - theme::ROW_HEIGHT)
                        .flex_shrink(0.0_f32)
                })
            },
        )
        .item_size_fixed(|| ROW_PITCH)
        .style(|s| s.flex_col().width_full()),
    ))
    // The last row's hairline is part of the padding below the list.
    .style(|s| nav_row::list_style(s).padding_bottom(16.0 - (ROW_PITCH - theme::ROW_HEIGHT)))
    .scroll()
    .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0))
}

/// A row and the gap below it, as the list's `gap` spaced them before it
/// was virtual.
const ROW_PITCH: f64 = theme::ROW_HEIGHT + 1.0;

/// A visible row, whether it is in Done or Snoozed, and the time its age
/// shows.
type PlacedRow = (InboxRow, bool, Timestamp);

/// The visible rows with their times. Done and Snoozed are Perch's lists,
/// not in GitHub's order, so their rows keep their own times.
fn placed_rows(state: AppState, show: InboxView) -> Vec<PlacedRow> {
    let away = show.is_put_away();
    let listed = (!away).then(|| state.notifications.with(|t| inbox::listed_at(t)));
    visible_notifications(state)
        .into_iter()
        .map(|row| {
            let at = match &row {
                InboxRow::Thread(thread) => listed
                    .as_ref()
                    .and_then(|listed| listed.get(&thread.id).copied())
                    .unwrap_or(thread.updated_at),
                InboxRow::Local { activity, .. } => activity.at,
            };
            (row, away, at)
        })
        .collect()
}

/// The rows as the virtual stack reads them, a slice at a time.
#[derive(Clone, PartialEq)]
struct Rows(Arc<Vec<PlacedRow>>);

impl VirtualVector<PlacedRow> for Rows {
    fn total_len(&self) -> usize {
        self.0.len()
    }

    fn slice(&self, range: Range<usize>) -> impl Iterator<Item = PlacedRow> {
        self.0[range].iter().cloned()
    }
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
    Local(String, String, bool, String, Option<SubjectState>),
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
    let row_key = format!("item:{}", item.id);
    let row = Container::new(content)
        .debug_name(format!("notification row:item:{}", item.id))
        .on_event_stop(el::Click, move |_, _| on_open(&open_item))
        .on_event_cont(el::PointerEnter, {
            let row_key = row_key.clone();
            move |_, _| hover(state, hovered, &row_key, true)
        })
        .on_event_cont(el::PointerLeave, move |_, _| {
            hover(state, hovered, &row_key, false)
        })
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
                .accelerator_for(Action::MarkDone)
                .action(move || local_done(&backend, &item))
        })
        .item("Mark Unread", move |entry| {
            let backend = backend.clone();
            let id = id.clone();
            entry
                .icon(SidebarIcon::Unread)
                .enabled(!unread)
                .accelerator_for(Action::MarkUnread)
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
    let opened = thread.clone();
    let open_backend = backend.clone();
    let url = github_url(&thread);
    let opens_inside = opens_inside(&thread);
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

    let row_key = format!("thread:{}", thread.id);
    let row = Container::new(content)
        .debug_name(format!("notification row:{}", thread.id))
        .on_event_stop(el::Click, move |_, _| {
            if opens_inside {
                open_thread(state, &open_backend, &opened);
            } else {
                open_backend.open_notification(&opened);
                if let Some(url) = &url {
                    open_in_browser(url);
                }
            }
        })
        .on_event_cont(el::PointerEnter, {
            let row_key = row_key.clone();
            move |_, _| hover(state, hovered, &row_key, true)
        })
        .on_event_cont(el::PointerLeave, move |_, _| {
            hover(state, hovered, &row_key, false)
        })
        .right_click_menu(move || {
            notification_menu(state, menu_backend.clone(), &menu_thread, away)
        })
        .style(|s| nav_row::row_style(s, false));
    Container::new(row).style(nav_row::row_gutter)
}

/// The pointer coming onto the row with `key`, or leaving it: the row's
/// own buttons, and the inbox's keys, follow it.
fn hover(state: AppState, hovered: RwSignal<bool>, key: &str, on: bool) {
    hovered.set(on);
    if on {
        state.hovered_row.set(Some(key.to_string()));
    } else {
        state.hovered_row.update(|h| {
            if h.as_deref() == Some(key) {
                *h = None;
            }
        });
    }
}

/// Whether the thread's subject opens in Perch, a pull request or an
/// issue, rather than in the browser.
pub(crate) fn opens_inside(thread: &Thread) -> bool {
    matches!(
        &thread.subject.kind,
        SubjectKind::PullRequest | SubjectKind::Issue
    ) && thread.number_hint().is_some()
}

/// Open a thread that opens inside, as a click on its row does.
pub(crate) fn open_thread(state: AppState, backend: &Backend, thread: &Thread) {
    if let Some(navigation_id) = backend.open_notification(thread) {
        state.begin_open("Reading the notification", navigation_id);
        if let Some(review) = state.review.get_untracked() {
            crate::review::notified::follow(review, thread);
        }
    }
}

/// What the inbox's keys act on.
pub(crate) enum KeyTarget {
    /// A row of GitHub's, and whether it is in Done or Snoozed, where
    /// its one action is Move to Inbox.
    Thread(Thread, bool),
    /// A row of a worktree's, for the activity it has.
    Local(Item, ItemActivity),
    /// A Worktrees row, or the open item.
    Item(Item),
}

/// The row under the pointer, on the tab the drawer shows, else the open
/// item. Browse's rows have no buttons of their own, so over one it is
/// the open item too.
pub(crate) fn key_target(state: AppState) -> Option<KeyTarget> {
    let (tab, away) = state
        .item_filters
        .with_untracked(|f| (f.tab, f.inbox.view.is_put_away()));
    match tab {
        crate::items::SidebarTab::Inbox => {
            if let Some(key) = state.hovered_row.get_untracked() {
                let row = floem::reactive::Effect::untrack(|| state.visible_inbox())
                    .into_iter()
                    .find(|row| row.key() == key);
                match row {
                    Some(InboxRow::Thread(thread)) => return Some(KeyTarget::Thread(thread, away)),
                    Some(InboxRow::Local { activity, item }) => {
                        return Some(KeyTarget::Local(item, activity))
                    }
                    None => {}
                }
            }
        }
        crate::items::SidebarTab::Worktrees => {
            let hovered = state.hovered_item.get_untracked();
            if let Some(item) = hovered.and_then(|id| state.item(&id)) {
                return Some(KeyTarget::Item(item));
            }
        }
        crate::items::SidebarTab::Browse => {}
    }
    let review = state.review.get_untracked()?;
    let item = review.item.get_untracked()?;
    Some(KeyTarget::Item((*item).clone()))
}

/// The inbox's row about `item`: a notification about its pull request or
/// issue, or the row for its last run. The whole inbox, filters aside.
fn inbox_row_of(state: AppState, item: &Item) -> Option<InboxRow> {
    let rows = floem::reactive::Effect::untrack(|| state.inbox_rows(InboxView::Inbox));
    row_about(rows, item)
}

fn row_about(rows: Vec<InboxRow>, item: &Item) -> Option<InboxRow> {
    rows.into_iter().find(|row| match row {
        InboxRow::Thread(thread) => item.subject.as_ref().is_some_and(|subject| {
            thread.is_about(subject.repo_full_name(), subject.kind(), subject.number())
        }),
        InboxRow::Local { item: local, .. } => local.id == item.id,
    })
}

/// Where an item stands in the inbox, as the Inbox row of its details
/// says it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum InboxStanding {
    /// The inbox has a row about it. `notification` is whether the row is
    /// GitHub's, which can be snoozed and unsubscribed from, rather than
    /// the row for a worktree's last run; `read` is whether Mark Unread
    /// has anything to do.
    Here { notification: bool, read: bool },
    /// Its notification was marked Done.
    Done,
    /// Its notification is snoozed, until then.
    Snoozed(Option<Timestamp>),
}

/// Where `item` stands in the inbox, tracked. None when the inbox never
/// had anything about it, or has nothing left to bring back.
pub(crate) fn standing(state: AppState, item: &Item) -> Option<InboxStanding> {
    if let Some(row) = row_about(state.inbox_rows(InboxView::Inbox), item) {
        return Some(match row {
            InboxRow::Thread(thread) => InboxStanding::Here {
                notification: true,
                read: !thread.unread,
            },
            InboxRow::Local { activity, .. } => InboxStanding::Here {
                notification: false,
                read: !activity.is_unread(),
            },
        });
    }
    let subject = item.subject.as_ref()?;
    let about = |thread: &Thread| {
        thread.is_about(subject.repo_full_name(), subject.kind(), subject.number())
    };
    if state
        .done_notifications
        .with(|threads| threads.iter().any(about))
    {
        return Some(InboxStanding::Done);
    }
    state.snoozed_notifications.with(|threads| {
        threads
            .iter()
            .find(|thread| about(thread))
            .map(|thread| InboxStanding::Snoozed(thread.snoozed_until))
    })
}

/// Done on `item`'s row in the inbox, as the row's check does. False when
/// the inbox has no row about it.
pub(crate) fn done_item(state: AppState, backend: &Backend, item: &Item) -> bool {
    match inbox_row_of(state, item) {
        Some(InboxRow::Thread(thread)) => done(state, backend, &thread),
        Some(InboxRow::Local { item, .. }) => local_done(backend, &item),
        None => return false,
    }
    true
}

/// The Snooze choices for `item`'s notification, as its row's Snooze has
/// them. Empty when the inbox has no notification about it.
pub(crate) fn snooze_menu_of(state: AppState, backend: Rc<Backend>, item: &Item) -> Menu {
    match inbox_row_of(state, item) {
        Some(InboxRow::Thread(thread)) => snooze_menu(state, backend, &thread),
        _ => Menu::new(),
    }
}

/// The rest of what `item`'s row in the inbox offers: Mark Unread, and on
/// a notification of GitHub's, Unsubscribe.
pub(crate) fn more_menu_of(state: AppState, backend: Rc<Backend>, item: &Item) -> Menu {
    let row = inbox_row_of(state, item);
    let read = match &row {
        Some(InboxRow::Thread(thread)) => !thread.unread,
        Some(InboxRow::Local { activity, .. }) => !activity.is_unread(),
        None => false,
    };
    let unread_backend = backend.clone();
    let unread_item = item.clone();
    let mut menu = Menu::new().item("Mark Unread", move |entry| {
        entry
            .icon(SidebarIcon::Unread)
            .enabled(read)
            .accelerator_for(Action::MarkUnread)
            .action(move || {
                crate::ui::items::mark_unread(state, &unread_backend, &unread_item);
            })
    });
    if let Some(InboxRow::Thread(thread)) = row {
        let id = thread.id;
        menu = menu.item("Unsubscribe", move |entry| {
            entry
                .icon(SidebarIcon::BellOff)
                .action(move || backend.unsubscribe_notification(&id))
        });
    }
    menu
}

/// Move `item`'s notification back to the inbox from Done or Snoozed, as
/// its row's Move to Inbox does. False when none was put away.
pub(crate) fn move_to_inbox(state: AppState, backend: &Backend, item: &Item) -> bool {
    let Some(subject) = item.subject.as_ref() else {
        return false;
    };
    match state.put_away_thread(subject.repo_full_name(), subject.kind(), subject.number()) {
        Some(id) => {
            backend.restore_notification(&id);
            true
        }
        None => false,
    }
}

/// Done on what the key is for, as its row's check does. False when it
/// has none: a row in Done or Snoozed, or an item with no row in the
/// inbox.
pub(crate) fn done_by_key(state: AppState, backend: &Backend) -> bool {
    let row = match key_target(state) {
        Some(KeyTarget::Thread(_, true)) | None => return false,
        Some(KeyTarget::Thread(thread, false)) => InboxRow::Thread(thread),
        Some(KeyTarget::Local(item, activity)) => InboxRow::Local { activity, item },
        Some(KeyTarget::Item(item)) => return done_item(state, backend, &item),
    };
    match row {
        InboxRow::Thread(thread) => done(state, backend, &thread),
        InboxRow::Local { item, .. } => local_done(backend, &item),
    }
    true
}

/// Mark Unread on what the key is for, as its row's menu does, or for an
/// item the item menu's. False where that is greyed: the row is unread
/// already, or in Done or Snoozed.
pub(crate) fn mark_unread_by_key(state: AppState, backend: &Backend) -> bool {
    match key_target(state) {
        Some(KeyTarget::Thread(thread, false)) if !thread.unread => {
            backend.mark_notification_unread(&thread.id);
            true
        }
        Some(KeyTarget::Local(item, activity)) if !activity.is_unread() => {
            backend.mark_item_activity_unread(&item.id);
            true
        }
        Some(KeyTarget::Item(item)) => crate::ui::items::mark_unread(state, backend, &item),
        _ => false,
    }
}

/// The page on github.com of what the key is for, as its row's or the
/// details' Open on GitHub goes to.
pub(crate) fn github_url_by_key(state: AppState) -> Option<String> {
    match key_target(state)? {
        KeyTarget::Thread(thread, _) => github_url(&thread),
        KeyTarget::Local(item, _) | KeyTarget::Item(item) => crate::ui::items::github_url(&item),
    }
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
        nav_row::chips(None, review),
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

/// Mark the thread Done, as its row shows it. The reader is finished
/// with it, so if the main pane has its pull request or issue open, that
/// closes, as going back to the sidebar would. Not when the pill is
/// holding back newer unread activity on it: the backend will leave the
/// row for that, and the pane is where it shows.
pub(super) fn done(state: AppState, backend: &Backend, thread: &Thread) {
    // Asked before the command goes, whose answer clears what is held.
    let stays = state.held_back_unseen(thread);
    backend.done_notification(thread);
    if !stays {
        close_if_viewing(state, backend, thread);
    }
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
    // Asked before the command goes, as in `done`.
    let viewing = threads
        .iter()
        .find(|thread| state.is_viewing_thread(thread))
        .filter(|thread| !state.held_back_unseen(thread));
    backend.done_notifications(&threads);
    if let Some(viewing) = viewing {
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
            .accelerator_for(Action::MarkDone)
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
            .accelerator_for(Action::MarkUnread)
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
            restored_at: None,
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

    /// The Done commands sent, and whether the pane was closed after.
    fn done_and_close(
        commands: &mut tokio::sync::mpsc::UnboundedReceiver<crate::backend::Command>,
    ) -> (Vec<(String, Timestamp)>, bool) {
        use crate::backend::Command;
        let (mut done, mut closed) = (Vec::new(), false);
        while let Ok(command) = commands.try_recv() {
            match command {
                Command::DoneNotification {
                    thread_id,
                    shown_at,
                } => done.push((thread_id, shown_at)),
                Command::DoneNotifications { shown } => done.extend(shown),
                Command::CloseReview { .. } => closed = true,
                _ => {}
            }
        }
        (done, closed)
    }

    /// Done closes the pane showing the thread: the reader is finished
    /// with it. Not when the pill is holding newer unread activity on it.
    /// The backend leaves that row and says there is something new, and
    /// closing the pane that would show it took the reader away from it.
    #[test]
    fn done_keeps_the_pane_open_for_activity_the_pill_is_holding_back() {
        let state = AppState::new();
        let (backend, mut commands) = Backend::recording();
        let shown = thread(
            "7",
            "org/repo",
            "Issue",
            SubjectKind::Issue,
            Reason::Mention,
            true,
        );
        let mut item = Item::new("org/repo", shown.updated_at);
        item.subject = Some(perch_github::ItemSubject::Issue(perch_github::Issue {
            viewer_can_update: false,
            node_id: String::new(),
            reactions: Default::default(),
            number: 7,
            title: "Issue".into(),
            body: String::new(),
            state: "open".into(),
            state_reason: None,
            author_login: "u".into(),
            author_avatar_url: String::new(),
            labels: vec![],
            assignees: vec![],
            repo_full_name: "org/repo".into(),
            comments: 0,
            created_at: Some(shown.updated_at),
            updated_at: shown.updated_at,
        }));
        let item = Arc::new(item);
        let view = || {
            let review = crate::review::model::ReviewState::new();
            review.item.set(Some(item.clone()));
            state.review.set(Some(review));
            assert!(state.is_viewing_thread(&shown));
        };
        let mut newer = shown.clone();
        newer.updated_at = "2026-09-19T13:00:00Z".parse().unwrap();
        let sent = vec![("7".to_string(), shown.updated_at)];
        let rows = [InboxRow::Thread(shown.clone())];

        // The row's Done and Mark N Done both send the row's revision.
        view();
        state.pending_notifications.set(Some(vec![newer.clone()]));
        done(state, &backend, &shown);
        assert_eq!(done_and_close(&mut commands), (sent.clone(), false));
        done_all(state, &backend, &rows);
        assert_eq!(done_and_close(&mut commands), (sent.clone(), false));

        // Nothing held back, or the reader's own late stamp: it closes.
        newer.unread = false;
        state.pending_notifications.set(Some(vec![newer]));
        done(state, &backend, &shown);
        assert_eq!(done_and_close(&mut commands), (sent.clone(), true));
        view();
        state.pending_notifications.set(None);
        done_all(state, &backend, &rows);
        assert_eq!(done_and_close(&mut commands), (sent, true));
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
            closed_at: None,
            closed_by: None,
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
