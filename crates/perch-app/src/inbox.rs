//! The Notifications list's filters: GitHub's threads under the
//! repository, reason and search the sidebar has picked. GitHub's order is
//! kept; filtering only removes rows from it.

use perch_github::Thread;

/// The threads under the Show, repository, reason and search filters, in
/// order.
pub fn filter(
    threads: &[Thread],
    unread_only: bool,
    repository: Option<&str>,
    reason: Option<&str>,
    query: &str,
) -> Vec<Thread> {
    let query = query.trim().to_lowercase();
    threads
        .iter()
        .filter(|thread| !unread_only || thread.unread)
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

/// The reasons GitHub currently has in the inbox, as wire value and
/// display label. New reasons appear automatically instead of being
/// dropped by a closed app enum.
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
        let visible = filter(&threads, false, None, None, "");
        assert_eq!(
            visible
                .iter()
                .map(|thread| thread.id.as_str())
                .collect::<Vec<_>>(),
            ["3", "2", "1"]
        );
        assert!(!visible[0].unread);

        // Show › Unread leaves the read one out, in the same order.
        let unread = filter(&threads, true, None, None, "");
        assert_eq!(
            unread
                .iter()
                .map(|thread| thread.id.as_str())
                .collect::<Vec<_>>(),
            ["2", "1"]
        );
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
        let visible = filter(&threads, false, Some("ORG/ONE"), Some("mention"), "manifest");
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
}
