//! The Notifications list: GitHub's notification rows, and each item's
//! local activity, one row per item.
//!
//! Notifications are their own list, not a view over local items. GitHub's
//! rows keep GitHub's order; an item's local activity is fitted in by time
//! (`crate::inbox`), and an item GitHub has a thread about shares that
//! thread's row. Filtering only removes rows from that order.

use std::rc::Rc;

use floem::menu::Menu;
use floem::prelude::*;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::views::{
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, ScrollExt, Stack,
};
use perch_github::{CheckRunState, Item, ItemActivity, SubjectKind, SubjectState, Thread};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::inbox::{InboxRow, Landing};
use crate::state::AppState;
use crate::theme;
use crate::ui::nav_row;
use crate::ui::widgets::{icon_button, open_in_browser, sidebar_glyph, state_glyph};

/// An item's side of a row: its latest local activity.
#[derive(Clone)]
struct Local {
    activity: ItemActivity,
    item: Item,
}

/// The inbox under the current search, repository, and reason filters.
pub fn notification_list(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    Stack::vertical((
        dyn_container(
            move || visible_notifications(state).is_empty(),
            |empty| {
                if empty {
                    Label::new("No matching notifications")
                        .style(|s| {
                            s.font_size(theme::TEXT_META)
                                .color(theme::NAV_SUBTLE)
                                .padding(24.0)
                        })
                        .into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        ),
        // `Rc`s: floem keeps a list's rows inline on the stack, and a row
        // with an item and its activity is large.
        dyn_stack(
            move || {
                visible_notifications(state)
                    .into_iter()
                    .map(Rc::new)
                    .collect::<Vec<_>>()
            },
            |row: &Rc<InboxRow>| {
                (
                    row.key(),
                    row.unread(),
                    row.thread().map(|thread| {
                        (
                            thread.unread,
                            thread.updated_at.to_string(),
                            // The face and the state arrive after the row does.
                            thread.subject_snapshot.clone(),
                        )
                    }),
                    row.activity().cloned(),
                    row.item().cloned(),
                )
            },
            // Behind `Rc`s too: a row's closures each hold one.
            move |row| match &*row {
                InboxRow::Thread(thread) => {
                    notification_row(state, backend.clone(), Rc::new(thread.clone()), None)
                        .into_any()
                }
                InboxRow::Merged {
                    thread,
                    activity,
                    item,
                } => notification_row(
                    state,
                    backend.clone(),
                    Rc::new(thread.clone()),
                    Some(Rc::new(Local {
                        activity: activity.clone(),
                        item: item.clone(),
                    })),
                )
                .into_any(),
                InboxRow::Local { activity, item } => local_row(
                    state,
                    backend.clone(),
                    Rc::new(Local {
                        activity: activity.clone(),
                        item: item.clone(),
                    }),
                )
                .into_any(),
            },
        )
        .style(|s| s.flex_col().width_full().gap(1.0)),
    ))
    .style(nav_row::list_style)
    .scroll()
    .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0))
}

/// The rows visible under the shared sidebar controls. Item-only settings
/// such as status, saved, and sort deliberately have no effect here.
pub fn visible_notifications(state: AppState) -> Vec<InboxRow> {
    state.visible_inbox()
}

/// A GitHub notification's row, carrying its item's local activity when
/// the item has some: unread if either side is, its age the later, and
/// opening it lands on whichever happened last.
fn notification_row(
    state: AppState,
    backend: Rc<Backend>,
    thread: Rc<Thread>,
    local: Option<Rc<Local>>,
) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let unread = thread.unread || local.as_ref().is_some_and(|l| l.activity.is_unread());
    let at = local
        .as_ref()
        .map_or(thread.updated_at, |l| thread.updated_at.max(l.activity.at));
    let local_is_latest = local
        .as_ref()
        .is_some_and(|l| l.activity.at > thread.updated_at);
    let title = thread.subject.title.clone();
    let open_thread = thread.clone();
    let open_local = local.clone();
    let open_backend = backend.clone();
    let url = github_url(&thread);
    let opens_inside = matches!(
        &thread.subject.kind,
        SubjectKind::PullRequest | SubjectKind::Issue
    ) && thread.number_hint().is_some();
    let menu_thread = thread.clone();
    let menu_local = local.clone();
    let menu_backend = backend.clone();
    let actions = Actions {
        thread: Some(thread.clone()),
        local: local.clone(),
        unread,
    };

    let content = Stack::horizontal((
        nav_row::glyph_box(notification_glyph(&thread)),
        nav_row::lines(
            nav_row::title_label(title, false, move || unread),
            trailing(state, hovered, backend, actions, at).style(|s| s.pointer_events_auto()),
            byline(state, &thread, local.as_deref()),
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
        .on_event_stop(el::Click, move |_, _| match &open_local {
            Some(local) if local_is_latest => open_at(state, &open_backend, local),
            _ => {
                let navigation_id = open_backend.open_notification(&open_thread);
                if opens_inside {
                    if let Some(navigation_id) = navigation_id {
                        state.begin_open("Reading the notification", navigation_id);
                    }
                } else if let Some(url) = &url {
                    open_in_browser(url);
                }
            }
        })
        .on_event_cont(el::PointerEnter, move |_, _| hovered.set(true))
        .on_event_cont(el::PointerLeave, move |_, _| hovered.set(false))
        .context_menu(move || {
            menu(
                state,
                menu_backend.clone(),
                Actions {
                    thread: Some(menu_thread.clone()),
                    local: menu_local.clone(),
                    unread,
                },
            )
        })
        .style(|s| nav_row::row_style(s, false));
    Container::new(row).style(nav_row::row_gutter)
}

/// An item's row for local activity GitHub has no thread about: the
/// item's own glyph and title, and its byline with what happened.
fn local_row(state: AppState, backend: Rc<Backend>, local: Rc<Local>) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let unread = local.activity.is_unread();
    let at = local.activity.at;
    let item = local.item.clone();
    let open_local = local.clone();
    let open_backend = backend.clone();
    let menu_local = local.clone();
    let menu_backend = backend.clone();
    let actions = Actions {
        thread: None,
        local: Some(local.clone()),
        unread,
    };
    let selected = {
        let id = item.id.clone();
        move || state.open_item.with(|o| o.as_deref() == Some(id.as_str()))
    };

    let content = Stack::horizontal((
        nav_row::glyph_box(crate::ui::items::glyph(&item)),
        nav_row::lines(
            nav_row::title_label(item.title().to_string(), false, move || unread),
            trailing(state, hovered, backend, actions, at).style(|s| s.pointer_events_auto()),
            crate::ui::items::inbox_byline(state, &item, &local.activity.event),
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
        .on_event_stop(el::Click, move |_, _| open_at(state, &open_backend, &open_local))
        .on_event_cont(el::PointerEnter, move |_, _| hovered.set(true))
        .on_event_cont(el::PointerLeave, move |_, _| hovered.set(false))
        .context_menu(move || {
            menu(
                state,
                menu_backend.clone(),
                Actions {
                    thread: None,
                    local: Some(menu_local.clone()),
                    unread,
                },
            )
        })
        .style(move |s| nav_row::row_style(s, selected()));
    Container::new(row).style(nav_row::row_gutter)
}

/// Open the item at what happened there last: Code on what a Work run
/// changed, or on Since you last looked after a head moved; the run on
/// Conversation after any other run; Conversation after a review. The
/// scope is for this open only.
fn open_at(state: AppState, backend: &Backend, local: &Local) {
    use crate::review::model::ItemTab;
    let Some(review) = state.review.get_untracked() else {
        return;
    };
    let item = &local.item;
    let landing = crate::inbox::landing(&local.activity.event);
    if let Landing::Run { run_id } = &landing {
        if !item.is_archived() {
            let navigation_id = backend.view_agent_owner(&item.id);
            state.begin_agent_run_navigation(item.id.clone(), run_id.clone(), navigation_id);
            state.begin_review(review, "Reading the discussion");
            return;
        }
    }
    state.pending_agent_navigation.set(None);
    state.agent_archived_owner.set(None);
    let message = match &item.subject {
        _ if item.is_archived() => "Checking out the files",
        Some(perch_github::ItemSubject::Issue(_)) => "Reading the issue",
        Some(perch_github::ItemSubject::PullRequest(_)) => "Reading the pull request",
        None => "Reading the branch",
    };
    let tab = match landing {
        Landing::RunChanges { .. } | Landing::Since => ItemTab::Code,
        Landing::Run { .. } | Landing::Conversation => ItemTab::Conversation,
    };
    let navigation_id = if item.is_archived() {
        backend.restore_item(&item.id)
    } else {
        backend.open_item_at(&item.id, landing)
    };
    state.pending_tab.set(Some((navigation_id, tab)));
    state.begin_item_review(review, message, &item.id, navigation_id);
}

/// What a row's Done and Mark Unread act on: its GitHub thread, its
/// item's local activity, or both.
#[derive(Clone)]
struct Actions {
    thread: Option<Rc<Thread>>,
    local: Option<Rc<Local>>,
    unread: bool,
}

/// The glyph GitHub's inbox draws for the subject: a pull request or an
/// issue in the state a look found it in, and open until one has, since
/// that is what most of an inbox is; a workflow run by the verdict its
/// title announces; and a muted mark for the rest.
fn notification_glyph(thread: &Thread) -> impl IntoView {
    let (glyph, color) = match &thread.subject.kind {
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
            return state_glyph(state, icons::state_color(state)).into_any();
        }
        SubjectKind::CheckSuite => match perch_github::announced_verdict(&thread.subject.title) {
            Some(CheckRunState::Failure) => (SidebarIcon::Close, theme::CLOSED),
            Some(CheckRunState::Success) => (SidebarIcon::Check, theme::OPEN),
            Some(CheckRunState::Cancelled) => (SidebarIcon::Skip, theme::NAV_MUTED),
            _ => (SidebarIcon::Check, theme::NAV_MUTED),
        },
        SubjectKind::Commit => (SidebarIcon::Branch, theme::NAV_MUTED),
        SubjectKind::Discussion => (SidebarIcon::ThreadHollow, theme::NAV_MUTED),
        SubjectKind::RepositoryVulnerabilityAlert => (SidebarIcon::Warning, theme::NAV_MUTED),
        SubjectKind::Release => (SidebarIcon::Bookmark, theme::NAV_MUTED),
        SubjectKind::Other(_) => (SidebarIcon::Comment, theme::NAV_MUTED),
    };
    sidebar_glyph(glyph, color).into_any()
}

/// The face of whoever opened the subject, the repository, the number,
/// and on a pull request its review pill and its churn, once a look has
/// found them: the shape an item's byline has. The repository's face
/// stands in until a look has found the person, and on a subject nobody
/// opened: a workflow run, a release.
fn byline(state: AppState, thread: &Thread, local: Option<&Local>) -> impl IntoView {
    let owner = thread
        .repository
        .full_name
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let (avatar_url, seed) = match thread
        .subject_snapshot
        .as_ref()
        .filter(|snapshot| !snapshot.author_login.is_empty())
    {
        Some(snapshot) => (snapshot.author_avatar_url.clone(), snapshot.author_login.clone()),
        None => (thread.repository.owner_avatar_url.clone(), owner),
    };
    let snapshot = thread.subject_snapshot.as_ref();
    let review = snapshot
        .is_some_and(|snapshot| snapshot.review_requested)
        .then_some(perch_github::ReviewDecision::ReviewRequested);
    nav_row::byline(
        state,
        move || avatar_url.clone(),
        seed,
        thread.repository.full_name.clone(),
        nav_row::reference(number(thread)),
        Stack::horizontal((
            nav_row::chips(state, None, review),
            crate::ui::items::activity_note(local.map(|local| &local.activity.event)),
            nav_row::note(
                local.and_then(|local| local.item.divergence()).map(|d| d.label()),
                theme::DRAFT_YELLOW,
            ),
        ))
        .style(|s| s.items_center().min_width(0.0)),
        nav_row::churn(snapshot.and_then(|snapshot| snapshot.churn)),
    )
}

/// The title's right end: the unread dot and the age, or, while the
/// pointer is over the row, Done and the menu in their place.
fn trailing(
    state: AppState,
    hovered: RwSignal<bool>,
    backend: Rc<Backend>,
    actions: Actions,
    at: jiff::Timestamp,
) -> impl IntoView {
    let unread = actions.unread;
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
            let done_actions = actions.clone();
            let menu_backend = backend.clone();
            let menu_actions = actions.clone();
            Stack::horizontal((
                icon_button(
                    || SidebarIcon::Check,
                    || theme::FG_SUBTLE,
                    move || done(state, &done_backend, &done_actions),
                ),
                nav_row::more_button(move || menu(state, menu_backend.clone(), menu_actions.clone())),
            ))
            .style(|s| s.items_center().flex_shrink(0.0_f32).margin_left(4.0))
            .into_any()
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

/// Mark the row Done: its thread on GitHub, and its item's activity in
/// Perch. Neither archives nor touches the item. The reader is finished
/// with it, so if the main pane has it open, that closes, as going back
/// to the sidebar would.
fn done(state: AppState, backend: &Backend, actions: &Actions) {
    if let Some(thread) = &actions.thread {
        backend.done_notification(&thread.id);
    }
    if let Some(local) = &actions.local {
        backend.done_item_activity(&local.item.id);
    }
    let viewing = actions
        .thread
        .as_ref()
        .is_some_and(|thread| state.is_viewing_thread(thread))
        || actions.local.as_ref().is_some_and(|local| {
            state
                .open_item
                .with_untracked(|open| open.as_deref() == Some(local.item.id.as_str()))
        });
    if !viewing {
        return;
    }
    if let Some(review) = state.review.get_untracked() {
        let navigation_id = backend.close_review();
        state.begin_close(review, navigation_id);
    }
}

fn menu(state: AppState, backend: Rc<Backend>, actions: Actions) -> Menu {
    let done_backend = backend.clone();
    let done_actions = actions.clone();
    let unread_backend = backend.clone();
    let unread_actions = actions.clone();
    let mut menu = Menu::new()
        .item("Done", move |item| {
            let backend = done_backend.clone();
            let actions = done_actions.clone();
            item.action(move || done(state, &backend, &actions))
        })
        .item("Mark Unread", move |item| {
            let backend = unread_backend.clone();
            let actions = unread_actions.clone();
            item.enabled(!actions.unread).action(move || {
                if let Some(thread) = actions.thread.as_ref().filter(|t| !t.unread) {
                    backend.mark_notification_unread(&thread.id);
                }
                if let Some(local) = &actions.local {
                    backend.mark_item_activity_unread(&local.item.id);
                }
            })
        });
    // Unsubscribe is GitHub's, for the thread alone.
    if let Some(thread) = &actions.thread {
        let unsubscribe_id = thread.id.clone();
        menu = menu.item("Unsubscribe", move |item| {
            let backend = backend.clone();
            let id = unsubscribe_id.clone();
            item.action(move || backend.unsubscribe_notification(&id))
        });
    }
    menu
}

/// The number GitHub shows for the subject: a pull request's, an issue's
/// or a discussion's. A release's URL ends in an id nobody sees, and a
/// commit's in its hash.
fn number(thread: &Thread) -> Option<u64> {
    match thread.subject.kind {
        SubjectKind::PullRequest | SubjectKind::Issue | SubjectKind::Discussion => thread.number_hint(),
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
        }
    }

    #[test]
    fn filtering_keeps_github_order_and_does_not_drop_read_or_unknown_kinds() {
        let threads = vec![
            thread(
                "3",
                "org/one",
                "A read discussion",
                SubjectKind::Discussion,
                Reason::Subscribed,
                false,
            ),
            thread(
                "2",
                "org/one",
                "An unread PR",
                SubjectKind::PullRequest,
                Reason::Mention,
                true,
            ),
            thread(
                "1",
                "org/two",
                "A release",
                SubjectKind::Release,
                Reason::Other("release".into()),
                true,
            ),
        ];
        let rows = crate::inbox::rows(&threads, &Default::default(), &[]);
        let visible = crate::inbox::filter(&rows, None, None, "");
        assert_eq!(
            visible.iter().map(InboxRow::key).collect::<Vec<_>>(),
            ["thread:3", "thread:2", "thread:1"]
        );
        assert!(!visible[0].unread());
    }

    #[test]
    fn repository_reason_and_search_filter_notification_fields() {
        let threads = vec![
            thread(
                "8",
                "org/one",
                "Manifest race",
                SubjectKind::Issue,
                Reason::Mention,
                true,
            ),
            thread(
                "7",
                "org/one",
                "Compaction",
                SubjectKind::PullRequest,
                Reason::ReviewRequested,
                true,
            ),
            thread(
                "6",
                "org/two",
                "Manifest docs",
                SubjectKind::Issue,
                Reason::Mention,
                true,
            ),
        ];
        let rows = crate::inbox::rows(&threads, &Default::default(), &[]);
        let visible = crate::inbox::filter(&rows, Some("ORG/ONE"), Some("mention"), "manifest");
        assert_eq!(visible.iter().map(InboxRow::key).collect::<Vec<_>>(), ["thread:8"]);
        assert_eq!(crate::inbox::repository_options(&rows), ["org/one", "org/two"]);
        assert_eq!(
            crate::inbox::reason_options(&rows),
            [
                ("mention".into(), "mention".into()),
                ("review_requested".into(), "review requested".into())
            ]
        );
    }

    fn local(item_id: &str, event: perch_github::ActivityEvent) -> Rc<Local> {
        let mut item = Item::new("org/one", "2026-09-19T12:00:00Z".parse().unwrap());
        item.id = item_id.into();
        item.checkout = Some(perch_github::ItemCheckout {
            branch: "fix".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: "/x".into(),
            owned: false,
            on_disk: true,
            churn: None,
            last_seen_head: None,
            divergence: None,
            source: None,
        });
        Rc::new(Local {
            activity: ItemActivity {
                item_id: item_id.into(),
                at: "2026-09-20T12:00:00Z".parse().unwrap(),
                event,
                read_at: None,
                done_at: None,
                local_unread: false,
            },
            item,
        })
    }

    fn sent(received: &mut tokio::sync::mpsc::UnboundedReceiver<crate::backend::Command>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(command) = received.try_recv() {
            out.push(format!("{command:?}"));
        }
        out
    }

    #[test]
    fn done_on_a_merged_row_puts_both_sides_away_and_never_archives() {
        let (backend, mut received) = Backend::recording();
        let state = AppState::new();
        let moved = perch_github::ActivityEvent::HeadMoved {
            from: "a".into(),
            to: "b".into(),
            commits: Some(1),
        };
        let thread = Rc::new(thread("57", "org/one", "Pull", SubjectKind::PullRequest, Reason::Mention, false));
        let actions = Actions {
            thread: Some(thread),
            local: Some(local("item-1", moved.clone())),
            unread: false,
        };
        done(state, &backend, &actions);
        let commands = sent(&mut received);
        assert_eq!(commands.len(), 2, "{commands:?}");
        assert!(commands[0].starts_with("DoneNotification { thread_id: \"57\""));
        assert!(commands[1].starts_with("ItemActivityDone { item_id: \"item-1\""));
        assert!(!commands.iter().any(|c| c.contains("Archive")));

        // A local row has no thread to put away.
        let actions = Actions { thread: None, local: Some(local("item-2", moved)), unread: true };
        done(state, &backend, &actions);
        let commands = sent(&mut received);
        assert_eq!(commands.len(), 1, "{commands:?}");
        assert!(commands[0].starts_with("ItemActivityDone"));
    }

    #[test]
    fn opening_a_local_row_lands_where_it_happened() {
        use crate::review::model::{ItemTab, ReviewState};
        let (backend, mut received) = Backend::recording();
        let state = AppState::new();
        state.review.set(Some(ReviewState::new()));
        let run = |files| perch_github::ActivityEvent::Run {
            run_id: "run-1".into(),
            backend: "claude-code".into(),
            mode: perch_github::RunMode::Work,
            outcome: perch_github::ActivityOutcome::Completed,
            files,
            head: None,
        };

        open_at(state, &backend, &local("item-1", run(Some(3))));
        let commands = sent(&mut received);
        assert!(commands[0].contains("OpenItemAt"), "{commands:?}");
        assert!(commands[0].contains("RunChanges { run_id: \"run-1\" }"), "{commands:?}");
        assert_eq!(state.pending_tab.get().map(|(_, tab)| tab), Some(ItemTab::Code));

        // A run that changed nothing is read on Conversation, at the run.
        open_at(state, &backend, &local("item-1", run(Some(0))));
        let commands = sent(&mut received);
        assert!(commands[0].starts_with("ViewAgentOwner"), "{commands:?}");
        assert_eq!(
            state.pending_agent_navigation.get().and_then(|p| p.run_id).as_deref(),
            Some("run-1")
        );

        open_at(
            state,
            &backend,
            &local(
                "item-1",
                perch_github::ActivityEvent::HeadMoved { from: "a".into(), to: "b".into(), commits: Some(1) },
            ),
        );
        let commands = sent(&mut received);
        assert!(commands[0].contains("landing: Since"), "{commands:?}");
        assert_eq!(state.pending_tab.get().map(|(_, tab)| tab), Some(ItemTab::Code));
    }

    #[test]
    fn the_byline_numbers_only_what_github_numbers() {
        let pull = thread("42", "org/repo", "Pull", SubjectKind::PullRequest, Reason::Mention, true);
        assert_eq!(number(&pull), Some(42));
        let mut release = thread("246813", "org/repo", "v1.0", SubjectKind::Release, Reason::Subscribed, true);
        release.subject.url = Some("https://api.github.com/repos/org/repo/releases/246813".into());
        assert_eq!(number(&release), None);
        let run = thread("9", "org/repo", "Run", SubjectKind::CheckSuite, Reason::CiActivity, true);
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
