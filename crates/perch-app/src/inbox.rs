//! The Inbox list's filters: GitHub's threads under the View, repository,
//! State, Unread Only and search the sidebar has picked. GitHub's
//! order is kept, or turned over for Oldest first; filtering only removes
//! rows from it.

use perch_github::{Reason, SubjectState, Thread};

use crate::items::{BrowseView, Direction, InboxFilters};

/// The threads under the menu's narrowing and the search, in order.
/// The view's list, which the threads come from, is the caller's.
/// `viewer` is the signed-in login, for Yours.
pub fn filter(
    threads: &[Thread],
    filters: &InboxFilters,
    viewer: Option<&str>,
    repository: Option<&str>,
    query: &str,
) -> Vec<Thread> {
    let query = query.trim().to_lowercase();
    let mut rows: Vec<Thread> = threads
        .iter()
        .filter(|thread| !filters.unread_only || thread.unread)
        .filter(|thread| involves(filters.whose, thread, viewer))
        .filter(|thread| {
            repository.is_none_or(|repo| thread.repository.full_name.eq_ignore_ascii_case(repo))
        })
        .filter(|thread| filters.state.admits(closed(thread)))
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
        .collect();
    if filters.direction == Direction::Oldest {
        rows.reverse();
    }
    rows
}

/// Whether the thread is under View › Review requested, Yours, Assigned
/// or Mentioned. GitHub gives one reason, the latest, so a mention on the
/// reader's own pull request is no longer `author`: Yours also takes a
/// thread a look found the reader opened.
pub fn involves(whose: BrowseView, thread: &Thread, viewer: Option<&str>) -> bool {
    match whose {
        BrowseView::ReviewRequested => thread.reason == Reason::ReviewRequested,
        BrowseView::Yours => {
            thread.reason == Reason::Author
                || viewer
                    .filter(|login| !login.is_empty())
                    .is_some_and(|login| {
                        thread.subject_snapshot.as_ref().is_some_and(|snapshot| {
                            snapshot.author_login.eq_ignore_ascii_case(login)
                        })
                    })
        }
        BrowseView::Assigned => thread.reason == Reason::Assign,
        BrowseView::Mentioned => matches!(thread.reason, Reason::Mention | Reason::TeamMention),
        BrowseView::All => true,
    }
}

/// Whether a look has found the thread's pull request merged or closed,
/// or its issue closed. A thread not yet looked at, or with nothing to
/// look at, is open.
pub fn closed(thread: &Thread) -> bool {
    thread.subject_snapshot.as_ref().is_some_and(|snapshot| {
        matches!(
            snapshot.state,
            SubjectState::PrMerged
                | SubjectState::PrClosed
                | SubjectState::IssueClosed
                | SubjectState::IssueNotPlanned
        )
    })
}

/// Repository choices for the filter: every thread's, once, by name.
pub fn repository_options(threads: &[Thread]) -> Vec<String> {
    let mut repos: Vec<String> = threads
        .iter()
        .map(|thread| thread.repository.full_name.clone())
        .collect();
    repos.sort_by_key(|repo| repo.to_lowercase());
    repos.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    repos
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_github::{Reason, Repository, Subject, SubjectKind};

    fn thread(
        id: &str,
        repo: &str,
        title: &str,
        kind: SubjectKind,
        reason: Reason,
        unread: bool,
    ) -> Thread {
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
                url: Some(format!("https://api.github.com/repos/{repo}/issues/{id}")),
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
        let visible = filter(&threads, &InboxFilters::default(), None, None, "");
        assert_eq!(
            visible
                .iter()
                .map(|thread| thread.id.as_str())
                .collect::<Vec<_>>(),
            ["3", "2", "1"]
        );
        assert!(!visible[0].unread);

        // Unread Only leaves the read one out, in the same order.
        let unread_only = InboxFilters {
            unread_only: true,
            ..Default::default()
        };
        let unread = filter(&threads, &unread_only, None, None, "");
        assert_eq!(
            unread
                .iter()
                .map(|thread| thread.id.as_str())
                .collect::<Vec<_>>(),
            ["2", "1"]
        );
    }

    #[test]
    fn repository_and_search_filter_notification_fields() {
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
        let visible = filter(
            &threads,
            &InboxFilters::default(),
            None,
            Some("ORG/ONE"),
            "manifest",
        );
        assert_eq!(
            visible
                .iter()
                .map(|thread| thread.id.as_str())
                .collect::<Vec<_>>(),
            ["8"]
        );
        assert_eq!(repository_options(&threads), ["org/one", "org/two"]);
    }

    /// State goes by what a look found; a thread not yet looked at, or a
    /// kind with nothing to look at, is open. Oldest first turns GitHub's
    /// order over.
    #[test]
    fn state_narrows_on_the_look_and_oldest_first_turns_the_order_over() {
        use crate::items::StateFilter;
        let snapshot = |state| perch_github::SubjectSnapshot {
            author_login: String::new(),
            author_avatar_url: String::new(),
            state,
            churn: None,
            review_requested: false,
            participants: None,
            at: "2026-09-19T12:00:00Z".parse().unwrap(),
        };
        let mut merged = thread(
            "4",
            "org/one",
            "Merged",
            SubjectKind::PullRequest,
            Reason::Author,
            false,
        );
        merged.subject_snapshot = Some(snapshot(SubjectState::PrMerged));
        let mut not_planned = thread(
            "3",
            "org/one",
            "Not planned",
            SubjectKind::Issue,
            Reason::Author,
            false,
        );
        not_planned.subject_snapshot = Some(snapshot(SubjectState::IssueNotPlanned));
        let mut open = thread(
            "2",
            "org/one",
            "Open",
            SubjectKind::PullRequest,
            Reason::Author,
            false,
        );
        open.subject_snapshot = Some(snapshot(SubjectState::PrDraft));
        let unlooked = thread(
            "1",
            "org/one",
            "Unlooked",
            SubjectKind::Release,
            Reason::Author,
            false,
        );
        let threads = vec![merged, not_planned, open, unlooked];
        let ids = |filters: &InboxFilters| {
            filter(&threads, filters, None, None, "")
                .into_iter()
                .map(|thread| thread.id)
                .collect::<Vec<_>>()
        };
        let closed = InboxFilters {
            state: StateFilter::Closed,
            ..Default::default()
        };
        assert_eq!(ids(&closed), ["4", "3"]);
        let open_only = InboxFilters {
            state: StateFilter::Open,
            ..Default::default()
        };
        assert_eq!(ids(&open_only), ["2", "1"]);
        let oldest = InboxFilters {
            direction: Direction::Oldest,
            ..Default::default()
        };
        assert_eq!(ids(&oldest), ["1", "2", "3", "4"]);
    }

    /// View › Review requested, Yours, Assigned and Mentioned go by
    /// GitHub's reason; Yours also by who a look found opened it.
    #[test]
    fn whose_narrows_on_the_reason_and_yours_on_the_author_too() {
        let mut mentioned_on_mine = thread(
            "6",
            "org/one",
            "Mine",
            SubjectKind::PullRequest,
            Reason::Mention,
            true,
        );
        mentioned_on_mine.subject_snapshot = Some(perch_github::SubjectSnapshot {
            author_login: "Reader".into(),
            author_avatar_url: String::new(),
            state: SubjectState::PrOpen,
            churn: None,
            review_requested: false,
            participants: None,
            at: "2026-09-19T12:00:00Z".parse().unwrap(),
        });
        let threads = vec![
            mentioned_on_mine,
            thread(
                "5",
                "org/one",
                "Team",
                SubjectKind::Issue,
                Reason::TeamMention,
                true,
            ),
            thread(
                "4",
                "org/one",
                "Assigned",
                SubjectKind::Issue,
                Reason::Assign,
                true,
            ),
            thread(
                "3",
                "org/one",
                "Authored",
                SubjectKind::PullRequest,
                Reason::Author,
                true,
            ),
            thread(
                "2",
                "org/one",
                "Review",
                SubjectKind::PullRequest,
                Reason::ReviewRequested,
                true,
            ),
            thread(
                "1",
                "org/one",
                "Watching",
                SubjectKind::Issue,
                Reason::Subscribed,
                true,
            ),
        ];
        let ids = |whose, viewer| {
            let filters = InboxFilters {
                whose,
                ..Default::default()
            };
            filter(&threads, &filters, viewer, None, "")
                .into_iter()
                .map(|thread| thread.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(BrowseView::ReviewRequested, Some("reader")), ["2"]);
        assert_eq!(ids(BrowseView::Yours, Some("reader")), ["6", "3"]);
        assert_eq!(ids(BrowseView::Yours, None), ["3"], "no login yet");
        assert_eq!(ids(BrowseView::Assigned, Some("reader")), ["4"]);
        assert_eq!(ids(BrowseView::Mentioned, Some("reader")), ["6", "5"]);
        assert_eq!(ids(BrowseView::All, Some("reader")).len(), 6);
    }
}
