//! GitHub notification rows.
//!
//! Notifications are their own list, not a view over local items. The order
//! is the order from GitHub; filtering (`crate::inbox`) only removes rows
//! from that order.

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
    state.visible_inbox()
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
            nav_row::title_label(title, false, move || unread),
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
        .context_menu(move || notification_menu(state, menu_backend.clone(), &menu_thread))
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
/// and on a pull request its review pill and its churn, once a look has
/// found them: the shape an item's byline has. The repository's face
/// stands in until a look has found the person, and on a subject nobody
/// opened: a workflow run, a release.
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
        nav_row::chips(state, None, review),
        nav_row::churn(snapshot.and_then(|snapshot| snapshot.churn)),
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
                return Stack::horizontal((
                    nav_row::unread_dot(move || unread),
                    nav_row::age(state, at),
                ))
                .style(|s| s.items_center().flex_shrink(0.0_f32))
                .into_any();
            }
            let done_backend = backend.clone();
            let done_thread = thread.clone();
            let menu_backend = backend.clone();
            let menu_thread = thread.clone();
            Stack::horizontal((
                icon_button(
                    || SidebarIcon::Check,
                    || theme::FG_SUBTLE,
                    move || done(state, &done_backend, &done_thread),
                ),
                nav_row::more_button(move || {
                    notification_menu(state, menu_backend.clone(), &menu_thread)
                }),
            ))
            .style(|s| s.items_center().flex_shrink(0.0_f32).margin_left(4.0))
            .into_any()
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

/// Mark the thread Done. The reader is finished with it, so if the main
/// pane has its pull request or issue open, that closes, as going back
/// to the sidebar would.
fn done(state: AppState, backend: &Backend, thread: &Thread) {
    backend.done_notification(&thread.id);
    if !state.is_viewing_thread(thread) {
        return;
    }
    if let Some(review) = state.review.get_untracked() {
        let navigation_id = backend.close_review();
        state.begin_close(review, navigation_id);
    }
}

fn notification_menu(state: AppState, backend: Rc<Backend>, thread: &Thread) -> Menu {
    let unread_backend = backend.clone();
    let unread_id = thread.id.clone();
    let unread = thread.unread;
    let done_backend = backend.clone();
    let done_thread = thread.clone();
    let unsubscribe_backend = backend;
    let unsubscribe_id = thread.id.clone();
    Menu::new()
        .item("Done", move |item| {
            let backend = done_backend.clone();
            let thread = done_thread.clone();
            item.action(move || done(state, &backend, &thread))
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
