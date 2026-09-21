//! GitHub notification rows.
//!
//! Notifications are their own list, not a view over local items. The order
//! is the order from GitHub; filtering only removes rows from that order.

use std::rc::Rc;

use floem::menu::Menu;
use floem::prelude::*;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::views::{
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, ScrollExt, Stack,
};
use perch_github::{CheckRunState, SubjectKind, SubjectState, Thread};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::state::AppState;
use crate::theme;
use crate::ui::nav_row;
use crate::ui::widgets::{icon_button, open_in_browser, sidebar_glyph, state_glyph};

/// The notification list under the current search, repository, and reason
/// filters. GitHub's ordering is retained.
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
        dyn_stack(
            move || visible_notifications(state),
            |thread: &Thread| {
                (
                    thread.id.clone(),
                    thread.unread,
                    thread.updated_at.to_string(),
                    // The face and the state arrive after the row does.
                    thread.subject_snapshot.clone(),
                )
            },
            move |thread| notification_row(state, backend.clone(), thread),
        )
        .style(|s| s.flex_col().width_full().gap(1.0)),
    ))
    .style(nav_row::list_style)
    .scroll()
    .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0))
}

/// The rows visible under the shared sidebar controls. Item-only settings
/// such as status, saved, and sort deliberately have no effect here.
pub fn visible_notifications(state: AppState) -> Vec<Thread> {
    let repository = state
        .item_filters
        .with(|filters| filters.repository.clone());
    let reason = state.notification_reason.get();
    let query = state.item_query.get();
    state.notifications.with(|threads| {
        filter_notifications(threads, repository.as_deref(), reason.as_deref(), &query)
    })
}

pub(crate) fn filter_notifications(
    threads: &[Thread],
    repository: Option<&str>,
    reason: Option<&str>,
    query: &str,
) -> Vec<Thread> {
    let query = query.trim().to_lowercase();
    threads
        .iter()
        .filter(|thread| {
            repository.is_none_or(|repo| thread.repository.full_name.eq_ignore_ascii_case(repo))
        })
        .filter(|thread| reason.is_none_or(|reason| thread.reason.wire() == reason))
        .filter(|thread| {
            query.is_empty()
                || thread.subject.title.to_lowercase().contains(&query)
                || thread.repository.full_name.to_lowercase().contains(&query)
                || thread.reason.label().to_lowercase().contains(&query)
                || thread.subject.kind.wire().to_lowercase().contains(&query)
                || thread
                    .number_hint()
                    .is_some_and(|number| number.to_string().contains(&query))
        })
        .cloned()
        .collect()
}

/// Repository choices for the notification filter, without disturbing the
/// spelling or order of the threads themselves.
pub fn repository_options(threads: &[Thread]) -> Vec<String> {
    let mut repos: Vec<String> = threads
        .iter()
        .map(|thread| thread.repository.full_name.clone())
        .collect();
    repos.sort_by_key(|repo| repo.to_lowercase());
    repos.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    repos
}

/// The reasons GitHub currently has in the inbox, as wire value and display
/// label. New reasons appear automatically instead of being dropped by a
/// closed app enum.
pub fn reason_options(threads: &[Thread]) -> Vec<(String, String)> {
    let mut reasons: Vec<(String, String)> = threads
        .iter()
        .map(|thread| {
            (
                thread.reason.wire().to_string(),
                thread.reason.label().to_string(),
            )
        })
        .collect();
    reasons.sort_by_key(|(wire, label)| (label.to_lowercase(), wire.clone()));
    reasons.dedup_by(|a, b| a.0 == b.0);
    reasons
}

fn notification_row(state: AppState, backend: Rc<Backend>, thread: Thread) -> impl IntoView {
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
            nav_row::title_label(title, false, unread),
            notification_trailing(state, hovered, action_backend, action_thread)
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
                }
            } else {
                if let Some(url) = &url {
                    open_in_browser(url);
                }
            }
        })
        .on_event_cont(el::PointerEnter, move |_, _| hovered.set(true))
        .on_event_cont(el::PointerLeave, move |_, _| hovered.set(false))
        .context_menu(move || notification_menu(menu_backend.clone(), &menu_thread))
        .style(|s| nav_row::row_style(s, false));
    Container::new(row).style(nav_row::row_gutter)
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
/// and why GitHub sent it at the right end. The repository's face stands
/// in until a look has found the person, and on a subject nobody opened:
/// a workflow run, a release.
fn byline(state: AppState, thread: &Thread) -> impl IntoView {
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
    nav_row::byline(
        state,
        move || avatar_url.clone(),
        seed,
        thread.repository.full_name.clone(),
        reference(thread),
        Empty::new(),
        nav_row::figure(thread.reason.label().to_string()),
    )
}

/// The title's right end: the unread dot and the age, or, while the
/// pointer is over the row, Done and the menu in their place.
fn notification_trailing(
    state: AppState,
    hovered: RwSignal<bool>,
    backend: Rc<Backend>,
    thread: Thread,
) -> impl IntoView {
    let unread = thread.unread;
    let at = thread.updated_at;
    dyn_container(
        move || hovered.get(),
        move |hovered| {
            if !hovered {
                return Stack::horizontal((unread_dot(unread), nav_row::age(state, at)))
                    .style(|s| s.items_center().flex_shrink(0.0_f32))
                    .into_any();
            }
            let done_backend = backend.clone();
            let done_id = thread.id.clone();
            let menu_backend = backend.clone();
            let menu_thread = thread.clone();
            Stack::horizontal((
                icon_button(
                    || SidebarIcon::Check,
                    || theme::FG_SUBTLE,
                    move || done_backend.done_notification(&done_id),
                ),
                nav_row::more_button(move || notification_menu(menu_backend.clone(), &menu_thread)),
            ))
            .style(|s| s.items_center().flex_shrink(0.0_f32).margin_left(4.0))
            .into_any()
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

fn notification_menu(backend: Rc<Backend>, thread: &Thread) -> Menu {
    let unread_backend = backend.clone();
    let unread_id = thread.id.clone();
    let unread = thread.unread;
    let done_backend = backend.clone();
    let done_id = thread.id.clone();
    let unsubscribe_backend = backend;
    let unsubscribe_id = thread.id.clone();
    Menu::new()
        .item("Done", move |item| {
            let backend = done_backend.clone();
            let id = done_id.clone();
            item.action(move || backend.done_notification(&id))
        })
        .item("Mark Unread", move |item| {
            let backend = unread_backend.clone();
            let id = unread_id.clone();
            item.enabled(!unread)
                .action(move || backend.mark_notification_unread(&id))
        })
        .item("Unsubscribe", move |item| {
            let backend = unsubscribe_backend.clone();
            let id = unsubscribe_id.clone();
            item.action(move || backend.unsubscribe_notification(&id))
        })
}

/// The unread reminder, before the age, in GitHub's blue.
fn unread_dot(unread: bool) -> impl IntoView {
    if unread {
        Empty::new()
            .style(|s| {
                s.size(7.0, 7.0)
                    .border_radius(4.0)
                    .background(theme::UNREAD)
                    .flex_shrink(0.0_f32)
                    .margin_left(6.0)
            })
            .into_any()
    } else {
        Empty::new().into_any()
    }
}

/// "#2085", or the kind of thing it is when it has no number.
fn reference(thread: &Thread) -> String {
    match thread.number_hint() {
        Some(number) => format!("#{number}"),
        None => thread.subject.kind.wire().to_string(),
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
        let visible = filter_notifications(&threads, None, None, "");
        assert_eq!(
            visible
                .iter()
                .map(|thread| thread.id.as_str())
                .collect::<Vec<_>>(),
            ["3", "2", "1"]
        );
        assert!(!visible[0].unread);
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
        let visible = filter_notifications(&threads, Some("ORG/ONE"), Some("mention"), "manifest");
        assert_eq!(
            visible
                .iter()
                .map(|thread| thread.id.as_str())
                .collect::<Vec<_>>(),
            ["8"]
        );
        assert_eq!(repository_options(&threads), ["org/one", "org/two"]);
        assert_eq!(
            reason_options(&threads),
            [
                ("mention".into(), "mention".into()),
                ("review_requested".into(), "review requested".into())
            ]
        );
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
