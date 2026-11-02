//! The Inbox list: GitHub's threads, and a row for each worktree without
//! a pull request where an agent run or a review ended, under the View,
//! repository, State, Unread Only and search the sidebar has picked.
//! GitHub's order is kept and the local rows are fitted in by time, or
//! the whole list turned over for Oldest first; filtering only removes
//! rows from it.

use std::collections::HashMap;

use jiff::Timestamp;
use perch_github::{
    ActivityEvent, ActivityOutcome, Item, ItemActivity, Reason, RunMode, SubjectState, Thread,
};

use crate::items::{BrowseView, Direction, InboxFilters};

/// A row of the inbox: a GitHub thread, or what last happened on one of
/// the reader's worktrees.
#[derive(Debug, Clone, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "a list of a few hundred rows, rebuilt as the inbox changes; a box per row buys nothing"
)]
pub enum InboxRow {
    Thread(Thread),
    Local { activity: ItemActivity, item: Item },
}

impl InboxRow {
    /// What the row is known by across lists.
    pub fn key(&self) -> String {
        match self {
            InboxRow::Thread(thread) => format!("thread:{}", thread.id),
            InboxRow::Local { item, .. } => format!("item:{}", item.id),
        }
    }

    pub fn thread(&self) -> Option<&Thread> {
        match self {
            InboxRow::Thread(thread) => Some(thread),
            InboxRow::Local { .. } => None,
        }
    }
}

/// The inbox: `threads` in GitHub's order, with the visible activity of
/// items that have no pull request fitted in by time. An item with a
/// pull request has GitHub's notifications instead, so a row it had
/// before one was opened goes. `threads` is the whole list, before any
/// filter, so a row's place doesn't depend on what is hidden.
pub fn rows(
    threads: &[Thread],
    activity: &HashMap<String, ItemActivity>,
    items: &[Item],
) -> Vec<InboxRow> {
    let items: HashMap<&str, &Item> = items.iter().map(|item| (item.id.as_str(), item)).collect();
    let mut local: Vec<(&ItemActivity, &Item)> = activity
        .values()
        .filter(|a| a.is_visible())
        .filter_map(|a| items.get(a.item_id.as_str()).map(|item| (a, *item)))
        .filter(|(_, item)| item.pull_request().is_none())
        .collect();
    // Newest first; ties by item id so the order is stable.
    local.sort_by(|a, b| b.0.at.cmp(&a.0.at).then_with(|| a.1.id.cmp(&b.1.id)));

    // Before each thread, whatever happened locally after the time its
    // row shows.
    let listed = listed_at(threads);
    let mut out = Vec::with_capacity(threads.len() + local.len());
    let mut local = local.into_iter().peekable();
    for thread in threads {
        let at = listed.get(&thread.id).copied().unwrap_or(thread.updated_at);
        while let Some((activity, item)) = local.next_if(|(a, _)| a.at > at) {
            out.push(InboxRow::Local {
                activity: activity.clone(),
                item: item.clone(),
            });
        }
        out.push(InboxRow::Thread(thread.clone()));
    }
    out.extend(local.map(|(activity, item)| InboxRow::Local {
        activity: activity.clone(),
        item: item.clone(),
    }));
    out
}

/// The rows under the menu's narrowing and the search, in order. The
/// view's list, which the rows come from, is the caller's. `viewer` is
/// the signed-in login, for Yours. A local row is the reader's own work,
/// so it is under Yours and All, and its search reads the item as a
/// Worktrees row's does, and what the chip says.
pub fn filter_rows(
    rows: &[InboxRow],
    filters: &InboxFilters,
    viewer: Option<&str>,
    repository: Option<&str>,
    query: &str,
) -> Vec<InboxRow> {
    let needle = query.trim().to_lowercase();
    let mut out: Vec<InboxRow> = rows
        .iter()
        .filter(|row| match row {
            InboxRow::Thread(thread) => admits_thread(thread, filters, viewer, repository, &needle),
            InboxRow::Local { activity, item } => {
                (!filters.unread_only || activity.is_unread())
                    && matches!(filters.whose, BrowseView::Yours | BrowseView::All)
                    && repository.is_none_or(|repo| item.repo_full_name.eq_ignore_ascii_case(repo))
                    && filters.state.admits(item.is_closed())
                    && (crate::items::matches_query(item, &needle)
                        || chip(&activity.event).to_lowercase().contains(&needle))
            }
        })
        .cloned()
        .collect();
    if filters.direction == Direction::Oldest {
        out.reverse();
    }
    out
}

/// Whether the thread passes the menu's narrowing and `query`, already
/// trimmed and lowercased.
fn admits_thread(
    thread: &Thread,
    filters: &InboxFilters,
    viewer: Option<&str>,
    repository: Option<&str>,
    query: &str,
) -> bool {
    (!filters.unread_only || thread.unread)
        && involves(filters.whose, thread, viewer)
        && repository.is_none_or(|repo| thread.repository.full_name.eq_ignore_ascii_case(repo))
        && filters.state.admits(closed(thread))
        && (query.is_empty()
            || thread.subject.title.to_lowercase().contains(query)
            || thread.repository.full_name.to_lowercase().contains(query)
            || thread.reason.label().to_lowercase().contains(query)
            || thread.subject.kind.wire().to_lowercase().contains(query)
            || thread
                .number_hint()
                .is_some_and(|number| number.to_string().contains(query)))
}

/// What a local row's chip says happened: `Claude · Work · 3 files`,
/// `Codex · Ask`, `Claude · Work · failed`, `Review · 2 findings`.
pub fn chip(event: &ActivityEvent) -> String {
    let plural = |n: u32, one: &str, many: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    match event {
        ActivityEvent::Run {
            backend,
            mode,
            outcome,
            files,
            ..
        } => {
            let agent = match perch_agent::Backend::parse(backend) {
                Some(perch_agent::Backend::ClaudeCode) => "Claude",
                Some(perch_agent::Backend::Codex) => "Codex",
                None => "Agent",
            };
            let mode_label = match mode {
                RunMode::Ask => "Ask",
                RunMode::Work => "Work",
            };
            let tail = match (outcome, mode, files) {
                (ActivityOutcome::Failed, _, _) => Some("failed".to_string()),
                (ActivityOutcome::Interrupted, _, _) => Some("interrupted".to_string()),
                (ActivityOutcome::Completed, RunMode::Work, Some(0)) => {
                    Some("no changes".to_string())
                }
                (ActivityOutcome::Completed, RunMode::Work, Some(n)) => {
                    Some(plural(*n, "file", "files"))
                }
                _ => None,
            };
            match tail {
                Some(tail) => format!("{agent} \u{b7} {mode_label} \u{b7} {tail}"),
                None => format!("{agent} \u{b7} {mode_label}"),
            }
        }
        ActivityEvent::Review {
            outcome, findings, ..
        } => match outcome {
            ActivityOutcome::Completed => {
                format!("Review \u{b7} {}", plural(*findings, "finding", "findings"))
            }
            _ => "Review \u{b7} failed".to_string(),
        },
    }
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

/// The time each thread's age shows: its `updated_at`, held no newer
/// than the row above. GitHub orders the list by when it last notified
/// the reader, which it doesn't send, while `updated_at` also moves for
/// things that notify no one, such as an edited comment. Taken over the
/// whole list, before filtering, so a hidden row still bounds the rows
/// under it.
pub fn listed_at(threads: &[Thread]) -> HashMap<String, Timestamp> {
    let mut bound = Timestamp::MAX;
    threads
        .iter()
        .map(|thread| {
            bound = bound.min(thread.updated_at);
            (thread.id.clone(), bound)
        })
        .collect()
}

/// Repository choices for the filter: every thread's, once, by name.
pub fn repository_options(threads: &[Thread]) -> Vec<String> {
    let rows: Vec<InboxRow> = threads.iter().cloned().map(InboxRow::Thread).collect();
    row_repository_options(&rows)
}

/// The same over the inbox's rows, local ones included.
pub fn row_repository_options(rows: &[InboxRow]) -> Vec<String> {
    let mut repos: Vec<String> = rows
        .iter()
        .map(|row| match row {
            InboxRow::Thread(thread) => thread.repository.full_name.clone(),
            InboxRow::Local { item, .. } => item.repo_full_name.clone(),
        })
        .collect();
    repos.sort_by_key(|repo| repo.to_lowercase());
    repos.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    repos
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`filter_rows`] over threads alone.
    fn filter(
        threads: &[Thread],
        filters: &InboxFilters,
        viewer: Option<&str>,
        repository: Option<&str>,
        query: &str,
    ) -> Vec<Thread> {
        let rows: Vec<InboxRow> = threads.iter().cloned().map(InboxRow::Thread).collect();
        filter_rows(&rows, filters, viewer, repository, query)
            .into_iter()
            .filter_map(|row| row.thread().cloned())
            .collect()
    }
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
    fn an_edit_does_not_make_a_row_newer_than_the_one_above() {
        let at = |id: &str, time: &str| Thread {
            updated_at: time.parse().unwrap(),
            ..thread(
                id,
                "org/one",
                id,
                SubjectKind::Discussion,
                Reason::Subscribed,
                false,
            )
        };
        let threads = vec![
            at("1", "2026-10-12T19:52:00Z"),
            at("2", "2026-10-12T19:16:00Z"),
            at("edited", "2026-10-12T23:52:00Z"),
            at("4", "2026-10-05T10:00:00Z"),
        ];
        let listed = listed_at(&threads);
        let shown = |id: &str| listed[id].to_string();
        assert_eq!(shown("1"), "2026-10-12T19:52:00Z");
        assert_eq!(shown("2"), "2026-10-12T19:16:00Z");
        assert_eq!(shown("edited"), "2026-10-12T19:16:00Z");
        assert_eq!(shown("4"), "2026-10-05T10:00:00Z");
    }

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

    fn ran(item_id: &str, at: &str, outcome: ActivityOutcome) -> ItemActivity {
        ItemActivity {
            item_id: item_id.into(),
            at: at.parse().unwrap(),
            event: ActivityEvent::Run {
                run_id: "r".into(),
                backend: "codex".into(),
                mode: RunMode::Ask,
                outcome,
                files: None,
            },
            read_at: None,
            done_at: None,
            local_unread: false,
        }
    }

    fn branch(id: &str) -> Item {
        let mut item = Item::new("org/one", "2026-09-19T12:00:00Z".parse().unwrap());
        item.id = id.into();
        item.checkout = Some(perch_github::ItemCheckout {
            branch: format!("fix-{id}"),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: "/x".into(),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
            divergence: None,
            source: None,
        });
        item
    }

    fn keys(rows: &[InboxRow]) -> Vec<String> {
        rows.iter().map(InboxRow::key).collect()
    }

    /// A local row goes above the first thread it is newer than, by the
    /// time that thread's row shows, and GitHub's order is left alone.
    /// Activity put away with Done has no row.
    #[test]
    fn local_rows_fit_in_by_time_among_github_s() {
        let mut newer = thread(
            "2",
            "org/one",
            "Newer",
            SubjectKind::Issue,
            Reason::Mention,
            true,
        );
        newer.updated_at = "2026-09-20T12:00:00Z".parse().unwrap();
        // Edited after the row above it: its row shows the one above's time.
        let mut edited = thread(
            "1",
            "org/one",
            "Edited",
            SubjectKind::Issue,
            Reason::Mention,
            true,
        );
        edited.updated_at = "2026-09-21T12:00:00Z".parse().unwrap();
        let mut older = thread(
            "0",
            "org/one",
            "Older",
            SubjectKind::Issue,
            Reason::Mention,
            true,
        );
        older.updated_at = "2026-09-18T12:00:00Z".parse().unwrap();
        let threads = vec![newer, edited, older];

        let items = vec![branch("a"), branch("b"), branch("done")];
        let mut done = ran("done", "2026-09-22T00:00:00Z", ActivityOutcome::Completed);
        done.done_at = Some(done.at);
        let activity: HashMap<String, ItemActivity> = [
            ran("a", "2026-09-20T06:00:00Z", ActivityOutcome::Completed),
            ran("b", "2026-09-17T00:00:00Z", ActivityOutcome::Failed),
            done,
        ]
        .into_iter()
        .map(|a| (a.item_id.clone(), a))
        .collect();
        let rows = rows(&threads, &activity, &items);
        assert_eq!(
            keys(&rows),
            ["thread:2", "thread:1", "item:a", "thread:0", "item:b"]
        );

        // Mentioned is GitHub's; Unread Only reads the run; Oldest first
        // turns the whole list over.
        let mentioned = InboxFilters {
            whose: BrowseView::Mentioned,
            ..Default::default()
        };
        assert_eq!(
            keys(&filter_rows(&rows, &mentioned, None, None, "")),
            ["thread:2", "thread:1", "thread:0"]
        );
        let oldest = InboxFilters {
            direction: Direction::Oldest,
            ..Default::default()
        };
        assert_eq!(
            keys(&filter_rows(&rows, &oldest, None, None, ""))[0],
            "item:b"
        );
        assert_eq!(
            keys(&filter_rows(
                &rows,
                &InboxFilters::default(),
                None,
                None,
                "fix-a"
            )),
            ["item:a"],
            "the search reads the branch"
        );
        assert_eq!(
            keys(&filter_rows(
                &rows,
                &InboxFilters::default(),
                None,
                None,
                "failed"
            )),
            ["item:b"],
            "and the chip"
        );
    }

    #[test]
    fn the_chip_says_who_ran_what_and_how_it_ended() {
        let run = |mode, outcome, files| ActivityEvent::Run {
            run_id: "r".into(),
            backend: "claude-code".into(),
            mode,
            outcome,
            files,
        };
        use ActivityOutcome::*;
        assert_eq!(
            chip(&run(RunMode::Work, Completed, Some(3))),
            "Claude \u{b7} Work \u{b7} 3 files"
        );
        assert_eq!(
            chip(&run(RunMode::Work, Completed, Some(1))),
            "Claude \u{b7} Work \u{b7} 1 file"
        );
        assert_eq!(
            chip(&run(RunMode::Work, Completed, Some(0))),
            "Claude \u{b7} Work \u{b7} no changes"
        );
        assert_eq!(
            chip(&run(RunMode::Ask, Completed, None)),
            "Claude \u{b7} Ask"
        );
        assert_eq!(
            chip(&run(RunMode::Ask, Failed, None)),
            "Claude \u{b7} Ask \u{b7} failed"
        );
        assert_eq!(
            chip(&ActivityEvent::Review {
                group_id: "g".into(),
                outcome: Completed,
                findings: 2,
            }),
            "Review \u{b7} 2 findings"
        );
    }
}
