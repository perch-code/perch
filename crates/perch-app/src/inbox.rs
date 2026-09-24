//! The Notifications list: GitHub's threads and each item's local
//! activity, one row per item.
//!
//! A thread about an item that has visible local activity is that item's
//! row, and carries both. Local activity with no such thread is a row of
//! its own. GitHub's rows keep GitHub's order; a local row, and a merged
//! row whose local side is the newer, is fitted in by its time.

use std::collections::HashMap;

use jiff::Timestamp;
use perch_github::{ActivityEvent, ActivityOutcome, Item, ItemActivity, RunMode, Thread};

/// The reason the filter offers for rows with local activity.
pub const AGENT_REASON: &str = "agent";
pub const AGENT_REASON_LABEL: &str = "agent";

#[derive(Debug, Clone, PartialEq)]
pub enum InboxRow {
    Thread(Thread),
    Local {
        activity: ItemActivity,
        item: Item,
    },
    Merged {
        thread: Thread,
        activity: ItemActivity,
        item: Item,
    },
}

impl InboxRow {
    /// What the row is known by across lists: its thread's id, or the
    /// item's for a row with no thread.
    pub fn key(&self) -> String {
        match self {
            InboxRow::Thread(thread) | InboxRow::Merged { thread, .. } => {
                format!("thread:{}", thread.id)
            }
            InboxRow::Local { item, .. } => format!("item:{}", item.id),
        }
    }

    pub fn thread(&self) -> Option<&Thread> {
        match self {
            InboxRow::Thread(thread) | InboxRow::Merged { thread, .. } => Some(thread),
            InboxRow::Local { .. } => None,
        }
    }

    pub fn activity(&self) -> Option<&ItemActivity> {
        match self {
            InboxRow::Local { activity, .. } | InboxRow::Merged { activity, .. } => Some(activity),
            InboxRow::Thread(_) => None,
        }
    }

    pub fn item(&self) -> Option<&Item> {
        match self {
            InboxRow::Local { item, .. } | InboxRow::Merged { item, .. } => Some(item),
            InboxRow::Thread(_) => None,
        }
    }

    pub fn unread(&self) -> bool {
        self.thread().is_some_and(|t| t.unread)
            || self.activity().is_some_and(ItemActivity::is_unread)
    }

    /// The later of the thread's activity and the item's.
    pub fn time(&self) -> Timestamp {
        match self {
            InboxRow::Thread(thread) => thread.updated_at,
            InboxRow::Local { activity, .. } => activity.at,
            InboxRow::Merged {
                thread, activity, ..
            } => thread.updated_at.max(activity.at),
        }
    }

    /// Whether the newest thing on the row happened locally: what opening
    /// it lands on.
    pub fn local_is_latest(&self) -> bool {
        match self {
            InboxRow::Thread(_) => false,
            InboxRow::Local { .. } => true,
            InboxRow::Merged {
                thread, activity, ..
            } => activity.at > thread.updated_at,
        }
    }

    pub fn repository(&self) -> &str {
        match self {
            InboxRow::Thread(thread) | InboxRow::Merged { thread, .. } => {
                &thread.repository.full_name
            }
            InboxRow::Local { item, .. } => &item.repo_full_name,
        }
    }
}

/// The inbox: `threads` in GitHub's order, merged with the visible local
/// activity of the items it's about, and the rest of the activity fitted
/// in by time.
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
        .collect();
    // Newest first; ties by item id so the order is stable.
    local.sort_by(|a, b| b.0.at.cmp(&a.0.at).then_with(|| a.1.id.cmp(&b.1.id)));

    let mut github = Vec::with_capacity(threads.len());
    let mut floating = Vec::new();
    for thread in threads {
        let about = local.iter().position(|(_, item)| {
            item.subject.as_ref().is_some_and(|subject| {
                thread.is_about(subject.repo_full_name(), subject.kind(), subject.number())
            })
        });
        match about {
            Some(at) => {
                let (activity, item) = local.remove(at);
                let row = InboxRow::Merged {
                    thread: thread.clone(),
                    activity: activity.clone(),
                    item: item.clone(),
                };
                // A merged row whose item moved last goes where its time
                // puts it, as a local row does.
                if row.local_is_latest() {
                    floating.push(row);
                } else {
                    github.push(row);
                }
            }
            None => github.push(InboxRow::Thread(thread.clone())),
        }
    }
    floating.extend(local.into_iter().map(|(activity, item)| InboxRow::Local {
        activity: activity.clone(),
        item: item.clone(),
    }));
    floating.sort_by_key(|row| std::cmp::Reverse(row.time()));

    // Before each GitHub row, whatever floats above its time.
    let mut out = Vec::with_capacity(github.len() + floating.len());
    let mut floating = floating.into_iter().peekable();
    for row in github {
        while floating.peek().is_some_and(|f| f.time() > row.time()) {
            out.extend(floating.next());
        }
        out.push(row);
    }
    out.extend(floating);
    out
}

/// The rows under the repository, reason and search filters, in order.
pub fn filter(
    rows: &[InboxRow],
    repository: Option<&str>,
    reason: Option<&str>,
    query: &str,
) -> Vec<InboxRow> {
    let query = query.trim().to_lowercase();
    rows.iter()
        .filter(|row| repository.is_none_or(|repo| row.repository().eq_ignore_ascii_case(repo)))
        .filter(|row| {
            reason.is_none_or(|reason| {
                (reason == AGENT_REASON && row.activity().is_some())
                    || row.thread().is_some_and(|t| t.reason.wire() == reason)
            })
        })
        .filter(|row| query.is_empty() || matches_query(row, &query))
        .cloned()
        .collect()
}

fn matches_query(row: &InboxRow, query: &str) -> bool {
    let thread = row.thread().is_some_and(|thread| {
        thread.subject.title.to_lowercase().contains(query)
            || thread.repository.full_name.to_lowercase().contains(query)
            || thread.reason.label().to_lowercase().contains(query)
            || thread.subject.kind.wire().to_lowercase().contains(query)
            || thread
                .number_hint()
                .is_some_and(|number| number.to_string().contains(query))
    });
    let item = row.item().is_some_and(|item| {
        item.title().to_lowercase().contains(query)
            || item.repo_full_name.to_lowercase().contains(query)
            || item
                .branch()
                .is_some_and(|b| b.to_lowercase().contains(query))
    });
    thread || item || (row.activity().is_some() && AGENT_REASON_LABEL.contains(query))
}

/// Repository choices for the filter: every row's, once, by name.
pub fn repository_options(rows: &[InboxRow]) -> Vec<String> {
    let mut repos: Vec<String> = rows
        .iter()
        .map(|row| row.repository().to_string())
        .collect();
    repos.sort_by_key(|repo| repo.to_lowercase());
    repos.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    repos
}

/// The reasons the inbox has now, as wire value and label: GitHub's, and
/// Agent while any row has local activity.
pub fn reason_options(rows: &[InboxRow]) -> Vec<(String, String)> {
    let mut reasons: Vec<(String, String)> = rows
        .iter()
        .filter_map(InboxRow::thread)
        .map(|thread| {
            (
                thread.reason.wire().to_string(),
                thread.reason.label().to_string(),
            )
        })
        .collect();
    if rows.iter().any(|row| row.activity().is_some()) {
        reasons.push((AGENT_REASON.to_string(), AGENT_REASON_LABEL.to_string()));
    }
    reasons.sort_by_key(|(wire, label)| (label.to_lowercase(), wire.clone()));
    reasons.dedup_by(|a, b| a.0 == b.0);
    reasons
}

/// What the row's chip says of the latest local event:
/// `Claude · Work · 3 files`, `Codex · Ask`, `Review · 2 findings`,
/// `2 new commits`.
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
            let mode = match mode {
                RunMode::Ask => "Ask",
                RunMode::Work => "Work",
            };
            let tail = match (outcome, mode, files) {
                (ActivityOutcome::Failed, _, _) => Some("failed".to_string()),
                (ActivityOutcome::Interrupted, _, _) => Some("interrupted".to_string()),
                (ActivityOutcome::Completed, "Work", Some(0)) => Some("no changes".to_string()),
                (ActivityOutcome::Completed, "Work", Some(n)) => Some(plural(*n, "file", "files")),
                _ => None,
            };
            match tail {
                Some(tail) => format!("{agent} \u{b7} {mode} \u{b7} {tail}"),
                None => format!("{agent} \u{b7} {mode}"),
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
        ActivityEvent::HeadMoved { commits, .. } => match commits {
            Some(n) if *n > 0 => plural(*n, "new commit", "new commits"),
            _ => "Head moved".to_string(),
        },
        ActivityEvent::NewWorktree { commits, .. } => {
            format!(
                "New worktree \u{b7} {}",
                plural(*commits, "commit", "commits")
            )
        }
    }
}

/// Where opening a row whose local side is the newer lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Landing {
    /// Code, on what the Work run changed.
    RunChanges { run_id: String },
    /// Conversation, at the run.
    Run { run_id: String },
    /// Conversation.
    Conversation,
    /// Code, on Since you last looked.
    Since,
}

pub fn landing(event: &ActivityEvent) -> Landing {
    match event {
        ActivityEvent::Run {
            run_id,
            mode: RunMode::Work,
            outcome: ActivityOutcome::Completed,
            files: Some(n),
            ..
        } if *n > 0 => Landing::RunChanges {
            run_id: run_id.clone(),
        },
        ActivityEvent::Run { run_id, .. } => Landing::Run {
            run_id: run_id.clone(),
        },
        ActivityEvent::Review { .. } => Landing::Conversation,
        ActivityEvent::HeadMoved { .. } | ActivityEvent::NewWorktree { .. } => Landing::Since,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_github::{ItemCheckout, ItemSubject, Reason, Repository, Subject, SubjectKind};

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn thread(id: &str, number: u64, updated: &str) -> Thread {
        Thread {
            id: id.into(),
            unread: false,
            local_unread: false,
            local_read: false,
            reason: Reason::Mention,
            updated_at: at(updated),
            last_read_at: None,
            subject: Subject {
                title: format!("Thread {id}"),
                kind: SubjectKind::PullRequest,
                url: Some(format!(
                    "https://api.github.com/repos/org/one/pulls/{number}"
                )),
                latest_comment_url: None,
            },
            repository: Repository {
                full_name: "org/one".into(),
                owner_avatar_url: String::new(),
            },
            subject_snapshot: None,
            run_snapshot: None,
        }
    }

    fn branch(id: &str, repo: &str) -> Item {
        let mut item = Item::new(repo, at("2026-09-01T00:00:00Z"));
        item.id = id.into();
        item.checkout = Some(ItemCheckout {
            branch: format!("branch-{id}"),
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
        item
    }

    fn pr_item(id: &str, number: u64) -> Item {
        let mut item = branch(id, "org/one");
        let pr: perch_github::PullRequest = serde_json::from_value(serde_json::json!({
            "number": number, "title": "t", "html_url": "", "author_login": "a",
            "author_avatar_url": "", "head_sha": "h", "head_ref": "b", "base_sha": "",
            "base_ref": "main", "draft": false, "merged": false, "state": "open",
            "additions": 0, "deletions": 0, "changed_files": 0, "repo_full_name": "org/one"
        }))
        .unwrap();
        item.subject = Some(ItemSubject::PullRequest(pr));
        item
    }

    fn activity(item_id: &str, when: &str) -> ItemActivity {
        ItemActivity {
            item_id: item_id.into(),
            at: at(when),
            event: ActivityEvent::HeadMoved {
                from: "a".into(),
                to: "b".into(),
                commits: Some(2),
            },
            read_at: None,
            done_at: None,
            local_unread: false,
        }
    }

    fn keys(rows: &[InboxRow]) -> Vec<String> {
        rows.iter().map(InboxRow::key).collect()
    }

    fn by_item(all: Vec<ItemActivity>) -> HashMap<String, ItemActivity> {
        all.into_iter().map(|a| (a.item_id.clone(), a)).collect()
    }

    #[test]
    fn local_rows_fit_in_by_time_without_moving_githubs() {
        // GitHub's order isn't strictly by time; it's kept as it is.
        let threads = vec![
            thread("t1", 1, "2026-09-23T10:00:00Z"),
            thread("t2", 2, "2026-09-23T11:00:00Z"),
            thread("t3", 3, "2026-09-23T08:00:00Z"),
        ];
        let items = vec![branch("a", "local/notes"), branch("b", "org/one")];
        let activity = by_item(vec![
            activity("a", "2026-09-23T12:00:00Z"),
            activity("b", "2026-09-23T09:00:00Z"),
        ]);
        assert_eq!(
            keys(&rows(&threads, &activity, &items)),
            ["item:a", "thread:t1", "thread:t2", "item:b", "thread:t3"]
        );
    }

    #[test]
    fn an_items_thread_and_its_activity_are_one_row() {
        let threads = vec![
            thread("t1", 7, "2026-09-23T10:00:00Z"),
            thread("t2", 8, "2026-09-23T09:00:00Z"),
        ];
        let items = vec![pr_item("p7", 7), pr_item("p8", 8)];
        let mut older = activity("p7", "2026-09-23T08:00:00Z");
        older.read_at = Some(older.at);
        let mut t1 = threads.clone();
        t1[0].unread = true;
        let activity = by_item(vec![older, activity("p8", "2026-09-23T11:00:00Z")]);
        let got = rows(&t1, &activity, &items);
        assert_eq!(
            keys(&got),
            ["thread:t2", "thread:t1"],
            "p8's newer run lifts its row"
        );
        assert!(matches!(&got[1], InboxRow::Merged { .. }));
        assert!(got[1].unread(), "unread if either side is");
        assert!(!got[1].local_is_latest());
        assert!(got[0].local_is_latest());
        assert_eq!(got[0].time(), at("2026-09-23T11:00:00Z"));
    }

    #[test]
    fn done_activity_and_activity_of_no_item_draw_nothing() {
        let items = vec![branch("a", "local/notes")];
        let mut done = activity("a", "2026-09-23T12:00:00Z");
        done.done_at = Some(done.at);
        let activity = by_item(vec![done, self::activity("gone", "2026-09-23T12:00:00Z")]);
        assert!(rows(&[], &activity, &items).is_empty());
    }

    #[test]
    fn the_agent_reason_and_the_items_repository_filter_local_rows() {
        let threads = vec![
            thread("t1", 7, "2026-09-23T10:00:00Z"),
            thread("t2", 9, "2026-09-23T09:00:00Z"),
        ];
        let items = vec![pr_item("p7", 7), branch("a", "local/notes")];
        let activity = by_item(vec![
            activity("p7", "2026-09-23T08:00:00Z"),
            activity("a", "2026-09-23T07:00:00Z"),
        ]);
        let all = rows(&threads, &activity, &items);
        assert_eq!(
            keys(&filter(&all, None, Some(AGENT_REASON), "")),
            ["thread:t1", "item:a"]
        );
        assert_eq!(
            keys(&filter(&all, None, Some("mention"), "")),
            ["thread:t1", "thread:t2"],
            "a merged row matches its thread's reason too"
        );
        assert_eq!(
            keys(&filter(&all, Some("LOCAL/notes"), None, "")),
            ["item:a"]
        );
        assert_eq!(keys(&filter(&all, None, None, "branch-a")), ["item:a"]);
        assert_eq!(repository_options(&all), ["local/notes", "org/one"]);
        assert_eq!(
            reason_options(&all),
            [
                ("agent".to_string(), "agent".to_string()),
                ("mention".to_string(), "mention".to_string())
            ]
        );
    }

    #[test]
    fn the_chip_says_what_happened_and_opening_lands_on_it() {
        let run = |mode, outcome, files| ActivityEvent::Run {
            run_id: "r".into(),
            backend: "claude-code".into(),
            mode,
            outcome,
            files,
            head: None,
        };
        let work = run(RunMode::Work, ActivityOutcome::Completed, Some(3));
        assert_eq!(chip(&work), "Claude \u{b7} Work \u{b7} 3 files");
        assert_eq!(landing(&work), Landing::RunChanges { run_id: "r".into() });
        let nothing = run(RunMode::Work, ActivityOutcome::Completed, Some(0));
        assert_eq!(chip(&nothing), "Claude \u{b7} Work \u{b7} no changes");
        assert_eq!(landing(&nothing), Landing::Run { run_id: "r".into() });
        let ask = run(RunMode::Ask, ActivityOutcome::Completed, None);
        assert_eq!(chip(&ask), "Claude \u{b7} Ask");
        let failed = run(RunMode::Work, ActivityOutcome::Failed, Some(2));
        assert_eq!(chip(&failed), "Claude \u{b7} Work \u{b7} failed");
        assert_eq!(landing(&failed), Landing::Run { run_id: "r".into() });
        let review = ActivityEvent::Review {
            group_id: "g".into(),
            outcome: ActivityOutcome::Completed,
            findings: 1,
        };
        assert_eq!(chip(&review), "Review \u{b7} 1 finding");
        assert_eq!(landing(&review), Landing::Conversation);
        let moved = ActivityEvent::HeadMoved {
            from: "a".into(),
            to: "b".into(),
            commits: Some(2),
        };
        assert_eq!(chip(&moved), "2 new commits");
        assert_eq!(landing(&moved), Landing::Since);
    }
}
