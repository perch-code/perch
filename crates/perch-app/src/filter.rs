//! Filtering, sorting, and grouping. Pure functions over thread lists.
//!
//! All of it runs in memory. Changing a filter never refetches, which is what
//! makes the sidebar feel instant.

use std::collections::HashMap;

use jiff::Timestamp;
use perch_github::{Reason, Thread};

use crate::timefmt::{bucket, DateBucket};

/// The five mutually exclusive reason filters in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReasonFilter {
    Assigned,
    Participating,
    Mentioned,
    TeamMentioned,
    ReviewRequested,
}

impl ReasonFilter {
    pub const ALL: [ReasonFilter; 5] = [
        ReasonFilter::Assigned,
        ReasonFilter::Participating,
        ReasonFilter::Mentioned,
        ReasonFilter::TeamMentioned,
        ReasonFilter::ReviewRequested,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ReasonFilter::Assigned => "Assigned",
            ReasonFilter::Participating => "Participating",
            ReasonFilter::Mentioned => "Mentioned",
            ReasonFilter::TeamMentioned => "Team mentioned",
            ReasonFilter::ReviewRequested => "Review requested",
        }
    }

    pub fn matches(self, reason: &Reason) -> bool {
        match self {
            ReasonFilter::Assigned => matches!(reason, Reason::Assign),
            ReasonFilter::Participating => reason.is_participating(),
            ReasonFilter::Mentioned => matches!(reason, Reason::Mention),
            ReasonFilter::TeamMentioned => matches!(reason, Reason::TeamMention),
            ReasonFilter::ReviewRequested => matches!(reason, Reason::ReviewRequested),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Newest,
    Oldest,
}

impl Sort {
    pub const ALL: [Sort; 2] = [Sort::Newest, Sort::Oldest];
}

impl std::fmt::Display for Sort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Sort::Newest => "Newest to oldest",
            Sort::Oldest => "Oldest to newest",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Repository,
    Date,
    None,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Repository, Group::Date, Group::None];
}

impl std::fmt::Display for Group {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Group::Repository => "Repository",
            Group::Date => "Date",
            Group::None => "None",
        })
    }
}

/// Everything narrowing the list right now. Filters combine with AND.
#[derive(Debug, Clone, Default)]
pub struct Filters {
    pub reason: Option<ReasonFilter>,
    pub repo: Option<String>,
    pub unread_only: bool,
    pub search: String,
}

impl Filters {
    pub fn is_narrowed(&self) -> bool {
        self.reason.is_some()
            || self.repo.is_some()
            || self.unread_only
            || !self.search.trim().is_empty()
    }

    fn accepts(&self, t: &Thread) -> bool {
        if self.unread_only && !t.unread {
            return false;
        }
        if let Some(r) = self.reason {
            if !r.matches(&t.reason) {
                return false;
            }
        }
        if let Some(repo) = &self.repo {
            if &t.repository.full_name != repo {
                return false;
            }
        }
        let needle = self.search.trim().to_lowercase();
        if !needle.is_empty() {
            let title = t.subject.title.to_lowercase();
            let repo = t.repository.full_name.to_lowercase();
            if !title.contains(&needle) && !repo.contains(&needle) {
                return false;
            }
        }
        true
    }
}

/// A rendered section of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// Stable identity for the view diffing key.
    pub key: String,
    pub title: String,
    /// Repository owner avatar, when the section is a repository.
    pub avatar_url: Option<String>,
    pub threads: Vec<Thread>,
}

/// Apply filters, then sort.
pub fn visible(threads: &[Thread], filters: &Filters, sort: Sort) -> Vec<Thread> {
    let mut out: Vec<Thread> = threads.iter().filter(|t| filters.accepts(t)).cloned().collect();
    sort_threads(&mut out, sort);
    out
}

pub fn sort_threads(threads: &mut [Thread], sort: Sort) {
    match sort {
        // Ties broken by id so the order never jitters between refreshes.
        Sort::Newest => threads.sort_by(|a, b| {
            b.updated_at.cmp(&a.updated_at).then_with(|| a.id.cmp(&b.id))
        }),
        Sort::Oldest => threads.sort_by(|a, b| {
            a.updated_at.cmp(&b.updated_at).then_with(|| a.id.cmp(&b.id))
        }),
    }
}

/// Split an already filtered and sorted list into sections.
pub fn sections(threads: &[Thread], group: Group, sort: Sort, now: Timestamp) -> Vec<Section> {
    match group {
        Group::None => {
            if threads.is_empty() {
                Vec::new()
            } else {
                vec![Section {
                    key: "all".into(),
                    title: String::new(),
                    avatar_url: None,
                    threads: threads.to_vec(),
                }]
            }
        }
        Group::Repository => {
            let mut order: Vec<String> = Vec::new();
            let mut by_repo: HashMap<String, Vec<Thread>> = HashMap::new();
            for t in threads {
                let key = t.repository.full_name.clone();
                if !by_repo.contains_key(&key) {
                    order.push(key.clone());
                }
                by_repo.entry(key).or_default().push(t.clone());
            }

            // Sections follow the same direction as the rows inside them: with
            // a sorted input, the first thread of each group is already its
            // extreme, so insertion order is the right section order.
            order
                .into_iter()
                .map(|key| {
                    let threads = by_repo.remove(&key).unwrap_or_default();
                    let avatar_url = threads
                        .first()
                        .map(|t| t.repository.owner_avatar_url.clone())
                        .filter(|u| !u.is_empty());
                    Section {
                        key: format!("repo:{key}"),
                        title: key,
                        avatar_url,
                        threads,
                    }
                })
                .collect()
        }
        Group::Date => {
            let mut by_bucket: HashMap<DateBucket, Vec<Thread>> = HashMap::new();
            for t in threads {
                by_bucket.entry(bucket(t.updated_at, now)).or_default().push(t.clone());
            }

            let mut buckets: Vec<DateBucket> = by_bucket.keys().copied().collect();
            buckets.sort();
            if sort == Sort::Oldest {
                buckets.reverse();
            }

            buckets
                .into_iter()
                .map(|b| Section {
                    key: format!("date:{}", b.label()),
                    title: b.label().to_string(),
                    avatar_url: None,
                    threads: by_bucket.remove(&b).unwrap_or_default(),
                })
                .collect()
        }
    }
}

/// Counts for the sidebar badges, computed over the unfiltered inbox.
#[derive(Debug, Clone, Default)]
pub struct Counts {
    pub unread: usize,
    pub by_reason: HashMap<ReasonFilter, usize>,
    /// Repository, thread count, owner avatar. Ordered by count, then name.
    pub repos: Vec<(String, usize, String)>,
}

pub fn counts(threads: &[Thread]) -> Counts {
    let mut by_reason: HashMap<ReasonFilter, usize> = HashMap::new();
    let mut repos: HashMap<String, (usize, String)> = HashMap::new();

    for t in threads {
        for f in ReasonFilter::ALL {
            if f.matches(&t.reason) {
                *by_reason.entry(f).or_default() += 1;
            }
        }
        let entry = repos
            .entry(t.repository.full_name.clone())
            .or_insert((0, t.repository.owner_avatar_url.clone()));
        entry.0 += 1;
        if entry.1.is_empty() {
            entry.1 = t.repository.owner_avatar_url.clone();
        }
    }

    let mut repos: Vec<(String, usize, String)> =
        repos.into_iter().map(|(k, (n, a))| (k, n, a)).collect();
    repos.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    Counts {
        unread: threads.iter().filter(|t| t.unread).count(),
        by_reason,
        repos,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_github::{Repository, Subject, SubjectKind};

    fn t(id: &str, repo: &str, reason: Reason, unread: bool, updated: &str, title: &str) -> Thread {
        Thread {
            id: id.into(),
            unread,
            reason,
            updated_at: updated.parse().unwrap(),
            last_read_at: None,
            subject: Subject {
                title: title.into(),
                kind: SubjectKind::PullRequest,
                url: Some(format!("https://api.github.com/repos/{repo}/pulls/{id}")),
                latest_comment_url: None,
            },
            repository: Repository {
                full_name: repo.into(),
                owner_avatar_url: format!("https://avatars/{repo}"),
            },
        }
    }

    fn fixture() -> Vec<Thread> {
        vec![
            t("1", "slatedb/slatedb", Reason::Mention, true, "2026-09-17T18:00:00Z", "Incremental compaction"),
            t("2", "slatedb/slatedb", Reason::Subscribed, false, "2026-09-17T09:00:00Z", "Tracing spans"),
            t("3", "apache/samza", Reason::TeamMention, true, "2026-09-16T10:00:00Z", "Publish build scans"),
            t("4", "slatedb/slatedb", Reason::ReviewRequested, false, "2026-09-10T10:00:00Z", "Two-phase checkpoint"),
            t("5", "apache/samza", Reason::Assign, true, "2026-08-01T10:00:00Z", "Old assigned thing"),
        ]
    }

    fn now() -> Timestamp {
        "2026-09-17T20:00:00Z".parse().unwrap()
    }

    #[test]
    fn no_filters_keeps_everything() {
        let all = fixture();
        assert_eq!(visible(&all, &Filters::default(), Sort::Newest).len(), 5);
    }

    #[test]
    fn newest_first_is_the_default_order() {
        let all = fixture();
        let v = visible(&all, &Filters::default(), Sort::Newest);
        let ids: Vec<&str> = v.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["1", "2", "3", "4", "5"]);
    }

    #[test]
    fn oldest_first_reverses_the_order() {
        let all = fixture();
        let v = visible(&all, &Filters::default(), Sort::Oldest);
        let ids: Vec<&str> = v.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["5", "4", "3", "2", "1"]);
    }

    #[test]
    fn unread_is_a_filter_not_a_query() {
        let all = fixture();
        let f = Filters { unread_only: true, ..Default::default() };
        let v = visible(&all, &f, Sort::Newest);
        assert_eq!(v.len(), 3);
        assert!(v.iter().all(|t| t.unread));
    }

    #[test]
    fn participating_spans_several_reasons() {
        let all = fixture();
        let f = Filters { reason: Some(ReasonFilter::Participating), ..Default::default() };
        let ids: Vec<String> = visible(&all, &f, Sort::Newest).iter().map(|t| t.id.clone()).collect();
        // Mention, team mention, review requested, assign. Not "subscribed".
        assert_eq!(ids, ["1", "3", "4", "5"]);
    }

    #[test]
    fn each_reason_filter_selects_its_own() {
        let all = fixture();
        for (filter, want) in [
            (ReasonFilter::Mentioned, vec!["1"]),
            (ReasonFilter::TeamMentioned, vec!["3"]),
            (ReasonFilter::ReviewRequested, vec!["4"]),
            (ReasonFilter::Assigned, vec!["5"]),
        ] {
            let f = Filters { reason: Some(filter), ..Default::default() };
            let ids: Vec<String> = visible(&all, &f, Sort::Newest).iter().map(|t| t.id.clone()).collect();
            assert_eq!(ids, want, "{filter:?}");
        }
    }

    #[test]
    fn filters_stack_with_and() {
        let all = fixture();
        let f = Filters {
            reason: Some(ReasonFilter::Participating),
            repo: Some("apache/samza".into()),
            unread_only: true,
            search: String::new(),
        };
        let ids: Vec<String> = visible(&all, &f, Sort::Newest).iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids, ["3", "5"]);
    }

    #[test]
    fn search_matches_title_or_repo_case_insensitively() {
        let all = fixture();

        let by_title = Filters { search: "COMPACTION".into(), ..Default::default() };
        assert_eq!(visible(&all, &by_title, Sort::Newest).len(), 1);

        let by_repo = Filters { search: "samza".into(), ..Default::default() };
        assert_eq!(visible(&all, &by_repo, Sort::Newest).len(), 2);

        let nothing = Filters { search: "   ".into(), ..Default::default() };
        assert_eq!(visible(&all, &nothing, Sort::Newest).len(), 5);
    }

    #[test]
    fn repository_sections_follow_the_sort_direction() {
        let all = fixture();
        let v = visible(&all, &Filters::default(), Sort::Newest);
        let s = sections(&v, Group::Repository, Sort::Newest, now());
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].title, "slatedb/slatedb");
        assert_eq!(s[1].title, "apache/samza");
        assert_eq!(s[0].threads.len(), 3);
        assert_eq!(s[0].avatar_url.as_deref(), Some("https://avatars/slatedb/slatedb"));
    }

    #[test]
    fn oldest_sort_flips_which_repository_leads() {
        let all = fixture();
        let v = visible(&all, &Filters::default(), Sort::Oldest);
        let s = sections(&v, Group::Repository, Sort::Oldest, now());
        assert_eq!(s[0].title, "apache/samza");
    }

    /// Timestamps offset from `now`, so the buckets land the same way in any
    /// time zone. The margins are wide enough to survive a DST shift.
    fn dated_fixture(now: Timestamp) -> Vec<Thread> {
        let hours = |h: i64| -> String {
            (now - jiff::SignedDuration::from_hours(h)).to_string()
        };
        vec![
            t("1", "a/b", Reason::Mention, true, &hours(2), "today"),
            t("2", "a/b", Reason::Mention, true, &hours(26), "yesterday"),
            t("3", "a/b", Reason::Mention, true, &hours(24 * 3 + 2), "this week"),
            t("4", "a/b", Reason::Mention, true, &hours(24 * 40), "earlier"),
        ]
    }

    #[test]
    fn date_sections_run_newest_bucket_first() {
        let now = Timestamp::now();
        let all = dated_fixture(now);
        let v = visible(&all, &Filters::default(), Sort::Newest);
        let s = sections(&v, Group::Date, Sort::Newest, now);
        let titles: Vec<&str> = s.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(titles, ["Today", "Yesterday", "This week", "Earlier"]);
    }

    #[test]
    fn oldest_sort_flips_the_date_sections_too() {
        let now = Timestamp::now();
        let all = dated_fixture(now);
        let v = visible(&all, &Filters::default(), Sort::Oldest);
        let s = sections(&v, Group::Date, Sort::Oldest, now);
        let titles: Vec<&str> = s.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(titles, ["Earlier", "This week", "Yesterday", "Today"]);
    }

    #[test]
    fn grouping_by_none_yields_one_section() {
        let all = fixture();
        let v = visible(&all, &Filters::default(), Sort::Newest);
        let s = sections(&v, Group::None, Sort::Newest, now());
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].threads.len(), 5);
    }

    #[test]
    fn empty_input_produces_no_sections() {
        for g in Group::ALL {
            assert!(sections(&[], g, Sort::Newest, now()).is_empty(), "{g:?}");
        }
    }

    #[test]
    fn every_thread_survives_grouping() {
        let all = fixture();
        let v = visible(&all, &Filters::default(), Sort::Newest);
        for g in Group::ALL {
            let total: usize = sections(&v, g, Sort::Newest, now())
                .iter()
                .map(|s| s.threads.len())
                .sum();
            assert_eq!(total, 5, "{g:?}");
        }
    }

    #[test]
    fn counts_are_computed_over_the_whole_inbox() {
        let c = counts(&fixture());
        assert_eq!(c.unread, 3);
        assert_eq!(c.by_reason.get(&ReasonFilter::Participating), Some(&4));
        assert_eq!(c.by_reason.get(&ReasonFilter::Mentioned), Some(&1));
        assert_eq!(c.repos[0], ("slatedb/slatedb".into(), 3, "https://avatars/slatedb/slatedb".into()));
        assert_eq!(c.repos[1].0, "apache/samza");
    }

    #[test]
    fn narrowing_is_detected_for_the_empty_state_message() {
        assert!(!Filters::default().is_narrowed());
        assert!(Filters { unread_only: true, ..Default::default() }.is_narrowed());
        assert!(Filters { search: "x".into(), ..Default::default() }.is_narrowed());
        assert!(!Filters { search: "  ".into(), ..Default::default() }.is_narrowed());
    }
}
