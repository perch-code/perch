//! Filtering, sorting, and grouping. Pure functions over thread lists.
//!
//! All of it runs in memory. Changing a filter never refetches, which is what
//! makes the sidebar feel instant.

use std::collections::{HashMap, HashSet};

use jiff::Timestamp;
use perch_github::{Mark, Reason, Thread};

/// Which of the three lists the inbox is showing.
///
/// Mutually exclusive, and orthogonal to every other filter: a search or a
/// repository narrows whichever view is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Inbox,
    Saved,
    Done,
}

impl View {
    pub const ALL: [View; 3] = [View::Inbox, View::Saved, View::Done];

    pub fn label(self) -> &'static str {
        match self {
            View::Inbox => "Inbox",
            View::Saved => "Saved",
            View::Done => "Done",
        }
    }

    /// Whether this view shows a thread, given its mark and whether the API
    /// still returns it.
    ///
    /// Only Saved and Done fall back on a snapshot. The inbox mirrors GitHub,
    /// so a bookmarked thread that has aged out of the API belongs in Saved
    /// and nowhere else; leaving it in the inbox would pin a notification
    /// there forever, with whatever unread flag it happened to be frozen at.
    fn accepts(self, mark: Option<&Mark>, live: bool) -> bool {
        match self {
            // Done hides a thread from the inbox. Saved does not.
            View::Inbox => live && !mark.is_some_and(Mark::is_done),
            View::Saved => mark.is_some_and(|m| m.saved),
            View::Done => mark.is_some_and(Mark::is_done),
        }
    }
}

impl std::fmt::Display for View {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

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
    pub view: View,
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

    fn accepts(&self, t: &Thread, mark: Option<&Mark>, live: bool) -> bool {
        if !self.view.accepts(mark, live) {
            return false;
        }
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

/// Every thread the app can show.
///
/// The live inbox, plus a snapshot for any marked thread GitHub has stopped
/// returning. Saved and Done have to keep listing a thread months after the
/// API forgets it, and this is where that copy comes back in.
///
/// Order is not meaningful here; `visible` sorts.
pub fn pool(live: &[Thread], marks: &HashMap<String, Mark>) -> Vec<Thread> {
    let present = live_ids(live);
    let mut out = live.to_vec();
    out.extend(
        marks
            .values()
            .filter(|m| !present.contains(&m.thread_id))
            .map(|m| m.snapshot.clone()),
    );
    out
}

/// The ids the API still returns, which is what separates a real inbox row
/// from a snapshot kept alive by a bookmark.
pub fn live_ids(live: &[Thread]) -> HashSet<String> {
    live.iter().map(|t| t.id.clone()).collect()
}

/// The run of rows between two ids, inclusive, in the order the list shows.
///
/// What a shift-click means. `None` when either end has left the list, which
/// happens when a poll lands between the two clicks; the caller then treats
/// the second click as an ordinary one rather than guessing at a range.
pub fn range_between<'a>(order: &'a [String], from: &str, to: &str) -> Option<&'a [String]> {
    let a = order.iter().position(|id| id == from)?;
    let b = order.iter().position(|id| id == to)?;
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    Some(&order[lo..=hi])
}

/// How many of these threads the view holds, before any other filter.
///
/// What the empty state needs: an empty Saved is a different thing to say
/// than a search that matched nothing inside it.
pub fn view_total(
    threads: &[Thread],
    live: &HashSet<String>,
    marks: &HashMap<String, Mark>,
    view: View,
) -> usize {
    threads
        .iter()
        .filter(|t| view.accepts(marks.get(&t.id), live.contains(&t.id)))
        .count()
}

/// Apply filters, then sort.
pub fn visible(
    threads: &[Thread],
    live: &HashSet<String>,
    marks: &HashMap<String, Mark>,
    filters: &Filters,
    sort: Sort,
) -> Vec<Thread> {
    let mut out: Vec<Thread> = threads
        .iter()
        .filter(|t| filters.accepts(t, marks.get(&t.id), live.contains(&t.id)))
        .cloned()
        .collect();
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

/// Counts for the sidebar badges.
///
/// Two kinds live here. `unread`, `saved` and `done` name the three views and
/// mean the same thing wherever you are standing. `by_reason` and `repos`
/// describe the view that is open, so switching to Done renumbers them
/// against done threads. Neither kind is narrowed by the reason, repository
/// or search filters: a badge has to be able to say what selecting it would
/// show.
#[derive(Debug, Clone, Default)]
pub struct Counts {
    /// Unread and not done. A dismissed thread stops nagging.
    pub unread: usize,
    /// Nothing draws these two any more: the panel names the view it is
    /// showing rather than counting the other two, and the only number left
    /// on screen is the rail's unread badge. They stay because one pass
    /// computes all three anyway, and because the test that pins down what
    /// the three views mean reads them.
    #[allow(dead_code)]
    pub saved: usize,
    #[allow(dead_code)]
    pub done: usize,
    pub by_reason: HashMap<ReasonFilter, usize>,
    /// Repository, thread count, owner avatar. Ordered by count, then name.
    pub repos: Vec<(String, usize, String)>,
}

pub fn counts(
    threads: &[Thread],
    live: &HashSet<String>,
    marks: &HashMap<String, Mark>,
    view: View,
) -> Counts {
    let mut by_reason: HashMap<ReasonFilter, usize> = HashMap::new();
    let mut repos: HashMap<String, (usize, String)> = HashMap::new();
    let (mut unread, mut saved, mut done) = (0, 0, 0);

    for t in threads {
        let mark = marks.get(&t.id);
        let is_done = mark.is_some_and(Mark::is_done);
        if mark.is_some_and(|m| m.saved) {
            saved += 1;
        }
        if is_done {
            done += 1;
        }
        // Only a live thread can be unread: a snapshot's flag is frozen at
        // whatever it was when the API last mentioned it.
        if t.unread && !is_done && live.contains(&t.id) {
            unread += 1;
        }

        if !view.accepts(mark, live.contains(&t.id)) {
            continue;
        }
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
        unread,
        saved,
        done,
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

    fn no_marks() -> HashMap<String, Mark> {
        HashMap::new()
    }

    /// (thread, saved, done) into the map `visible` and `counts` expect.
    fn marks(entries: &[(&Thread, bool, bool)]) -> HashMap<String, Mark> {
        entries
            .iter()
            .map(|(t, saved, done)| {
                (
                    t.id.clone(),
                    Mark {
                        thread_id: t.id.clone(),
                        saved: *saved,
                        done_at: done.then_some(t.updated_at),
                        marked_at: t.updated_at,
                        snapshot: (*t).clone(),
                    },
                )
            })
            .collect()
    }

    /// The unmarked case, where every thread is one the API still returns.
    fn vis(all: &[Thread], f: &Filters, sort: Sort) -> Vec<Thread> {
        visible(all, &live_ids(all), &no_marks(), f, sort)
    }

    fn ids(threads: &[Thread]) -> Vec<&str> {
        threads.iter().map(|t| t.id.as_str()).collect()
    }

    /// `threads` is what the app can draw on; `live` is what the API still
    /// returns, which is only different once a snapshot is in play.
    fn in_view(
        threads: &[Thread],
        live: &[Thread],
        m: &HashMap<String, Mark>,
        view: View,
    ) -> Vec<String> {
        visible(
            threads,
            &live_ids(live),
            m,
            &Filters { view, ..Default::default() },
            Sort::Newest,
        )
        .iter()
        .map(|t| t.id.clone())
        .collect()
    }

    #[test]
    fn no_filters_keeps_everything() {
        let all = fixture();
        assert_eq!(vis(&all, &Filters::default(), Sort::Newest).len(), 5);
    }

    #[test]
    fn newest_first_is_the_default_order() {
        let all = fixture();
        let v = vis(&all, &Filters::default(), Sort::Newest);
        let ids: Vec<&str> = v.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["1", "2", "3", "4", "5"]);
    }

    #[test]
    fn oldest_first_reverses_the_order() {
        let all = fixture();
        let v = vis(&all, &Filters::default(), Sort::Oldest);
        let ids: Vec<&str> = v.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["5", "4", "3", "2", "1"]);
    }

    #[test]
    fn unread_is_a_filter_not_a_query() {
        let all = fixture();
        let f = Filters { unread_only: true, ..Default::default() };
        let v = vis(&all, &f, Sort::Newest);
        assert_eq!(v.len(), 3);
        assert!(v.iter().all(|t| t.unread));
    }

    #[test]
    fn participating_spans_several_reasons() {
        let all = fixture();
        let f = Filters { reason: Some(ReasonFilter::Participating), ..Default::default() };
        let ids: Vec<String> = vis(&all, &f, Sort::Newest).iter().map(|t| t.id.clone()).collect();
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
            let ids: Vec<String> = vis(&all, &f, Sort::Newest).iter().map(|t| t.id.clone()).collect();
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
            view: View::Inbox,
        };
        let ids: Vec<String> = vis(&all, &f, Sort::Newest).iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids, ["3", "5"]);
    }

    #[test]
    fn search_matches_title_or_repo_case_insensitively() {
        let all = fixture();

        let by_title = Filters { search: "COMPACTION".into(), ..Default::default() };
        assert_eq!(vis(&all, &by_title, Sort::Newest).len(), 1);

        let by_repo = Filters { search: "samza".into(), ..Default::default() };
        assert_eq!(vis(&all, &by_repo, Sort::Newest).len(), 2);

        let nothing = Filters { search: "   ".into(), ..Default::default() };
        assert_eq!(vis(&all, &nothing, Sort::Newest).len(), 5);
    }

    #[test]
    fn repository_sections_follow_the_sort_direction() {
        let all = fixture();
        let v = vis(&all, &Filters::default(), Sort::Newest);
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
        let v = vis(&all, &Filters::default(), Sort::Oldest);
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
        let v = vis(&all, &Filters::default(), Sort::Newest);
        let s = sections(&v, Group::Date, Sort::Newest, now);
        let titles: Vec<&str> = s.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(titles, ["Today", "Yesterday", "This week", "Earlier"]);
    }

    #[test]
    fn oldest_sort_flips_the_date_sections_too() {
        let now = Timestamp::now();
        let all = dated_fixture(now);
        let v = vis(&all, &Filters::default(), Sort::Oldest);
        let s = sections(&v, Group::Date, Sort::Oldest, now);
        let titles: Vec<&str> = s.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(titles, ["Earlier", "This week", "Yesterday", "Today"]);
    }

    #[test]
    fn grouping_by_none_yields_one_section() {
        let all = fixture();
        let v = vis(&all, &Filters::default(), Sort::Newest);
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
        let v = vis(&all, &Filters::default(), Sort::Newest);
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
        let all = fixture();
        let c = counts(&all, &live_ids(&all), &no_marks(), View::Inbox);
        assert_eq!(c.unread, 3);
        assert_eq!(c.by_reason.get(&ReasonFilter::Participating), Some(&4));
        assert_eq!(c.by_reason.get(&ReasonFilter::Mentioned), Some(&1));
        assert_eq!(c.repos[0], ("slatedb/slatedb".into(), 3, "https://avatars/slatedb/slatedb".into()));
        assert_eq!(c.repos[1].0, "apache/samza");
    }

    #[test]
    fn done_leaves_the_inbox_and_saved_does_not() {
        let all = fixture();
        let m = marks(&[(&all[0], false, true), (&all[1], true, false)]);

        assert_eq!(in_view(&all, &all, &m, View::Inbox), ["2", "3", "4", "5"]);
        assert_eq!(in_view(&all, &all, &m, View::Saved), ["2"]);
        assert_eq!(in_view(&all, &all, &m, View::Done), ["1"]);
    }

    #[test]
    fn a_thread_can_be_both_saved_and_done() {
        let all = fixture();
        let m = marks(&[(&all[0], true, true)]);

        assert!(in_view(&all, &all, &m, View::Inbox).iter().all(|id| id != "1"));
        assert_eq!(in_view(&all, &all, &m, View::Saved), ["1"]);
        assert_eq!(in_view(&all, &all, &m, View::Done), ["1"]);
    }

    #[test]
    fn filters_stack_on_top_of_a_view() {
        let all = fixture();
        // Everything done, so the Done view has something to narrow.
        let m = marks(&all.iter().map(|t| (t, false, true)).collect::<Vec<_>>());

        let f = Filters {
            view: View::Done,
            repo: Some("apache/samza".into()),
            ..Default::default()
        };
        let got: Vec<String> = visible(&all, &live_ids(&all), &m, &f, Sort::Newest)
            .iter()
            .map(|t| t.id.clone())
            .collect();
        assert_eq!(got, ["3", "5"]);

        let searched = Filters {
            view: View::Done,
            search: "compaction".into(),
            ..Default::default()
        };
        assert_eq!(visible(&all, &live_ids(&all), &m, &searched, Sort::Newest).len(), 1);
    }

    #[test]
    fn an_unmarked_inbox_is_the_whole_inbox() {
        let all = fixture();
        assert_eq!(in_view(&all, &all, &no_marks(), View::Inbox).len(), 5);
        assert!(in_view(&all, &all, &no_marks(), View::Saved).is_empty());
        assert!(in_view(&all, &all, &no_marks(), View::Done).is_empty());
    }

    #[test]
    fn the_pool_adds_snapshots_the_api_forgot() {
        let all = fixture();
        let gone = t("9", "a/b", Reason::Mention, false, "2026-09-12T10:00:00Z", "Long gone");
        let m = marks(&[(&all[0], true, false), (&gone, true, false)]);

        // The live list never mentions thread 9; the snapshot carries it.
        let pooled = pool(&all, &m);
        assert_eq!(pooled.len(), 6);
        assert_eq!(in_view(&pooled, &all, &m, View::Saved), ["1", "9"]);
    }

    #[test]
    fn the_pool_prefers_the_live_thread_over_its_snapshot() {
        let all = fixture();
        let mut stale = all[0].clone();
        stale.subject.title = "Stale title".into();
        let m = marks(&[(&stale, true, false)]);

        let pooled = pool(&all, &m);
        assert_eq!(pooled.len(), 5);
        assert_eq!(pooled[0].subject.title, "Incremental compaction");
    }

    #[test]
    fn the_pool_is_ordered_once_it_is_sorted() {
        let all = fixture();
        let gone = t("9", "a/b", Reason::Mention, false, "2026-09-12T10:00:00Z", "Long gone");
        let other = t("8", "a/b", Reason::Mention, false, "2026-09-14T10:00:00Z", "Also gone");
        let m = marks(&[
            (&all[0], true, false),
            (&all[2], true, false),
            (&other, true, false),
            (&gone, true, false),
        ]);

        // Saved is where live threads and snapshots sit side by side. HashMap
        // iteration is unordered, so the sort is the only thing making this
        // list the same on two runs.
        let pooled = pool(&all, &m);
        let v = visible(
            &pooled,
            &live_ids(&all),
            &m,
            &Filters { view: View::Saved, ..Default::default() },
            Sort::Newest,
        );
        assert_eq!(ids(&v), ["1", "3", "8", "9"]);
    }

    #[test]
    fn a_saved_thread_the_api_forgot_leaves_the_inbox() {
        let all = fixture();
        let gone = t("9", "a/b", Reason::Mention, true, "2026-09-12T10:00:00Z", "Long gone");
        let m = marks(&[(&gone, true, false)]);
        let pooled = pool(&all, &m);

        // It is still bookmarked, so Saved keeps it.
        assert_eq!(in_view(&pooled, &all, &m, View::Saved), ["9"]);
        // The inbox mirrors the API, which has not mentioned it in months.
        assert!(!in_view(&pooled, &all, &m, View::Inbox).contains(&"9".to_string()));
        // And its frozen unread flag does not nag from the badge.
        assert_eq!(counts(&pooled, &live_ids(&all), &m, View::Inbox).unread, 3);
        assert_eq!(counts(&pooled, &live_ids(&all), &m, View::Inbox).saved, 1);
    }

    #[test]
    fn done_threads_stop_counting_as_unread() {
        let all = fixture();
        // Thread 1 is unread; dismissing it should quiet the badge.
        let m = marks(&[(&all[0], false, true)]);
        assert_eq!(counts(&all, &live_ids(&all), &m, View::Inbox).unread, 2);
    }

    #[test]
    fn the_three_view_counts_read_the_same_from_anywhere() {
        let all = fixture();
        let m = marks(&[(&all[0], true, true), (&all[1], true, false)]);

        for view in [View::Inbox, View::Saved, View::Done] {
            let c = counts(&all, &live_ids(&all), &m, view);
            assert_eq!(c.saved, 2, "{view:?}");
            assert_eq!(c.done, 1, "{view:?}");
            assert_eq!(c.unread, 2, "{view:?}");
        }
    }

    #[test]
    fn reason_and_repository_counts_follow_the_open_view() {
        let all = fixture();
        // Two done, both on slatedb.
        let m = marks(&[(&all[0], false, true), (&all[3], false, true)]);

        let done = counts(&all, &live_ids(&all), &m, View::Done);
        assert_eq!(done.repos.len(), 1);
        assert_eq!(done.repos[0].0, "slatedb/slatedb");
        assert_eq!(done.repos[0].1, 2);
        assert_eq!(done.by_reason.get(&ReasonFilter::Mentioned), Some(&1));
        assert_eq!(done.by_reason.get(&ReasonFilter::TeamMentioned), None);

        // The same marks, seen from the inbox, describe what is left there.
        let inbox = counts(&all, &live_ids(&all), &m, View::Inbox);
        assert_eq!(inbox.repos[0].1, 2);
        assert_eq!(inbox.by_reason.get(&ReasonFilter::Mentioned), None);
    }

    #[test]
    fn grouping_reorders_the_rows_a_shift_click_runs_between() {
        let all = fixture();
        let v = vis(&all, &Filters::default(), Sort::Newest);
        let grouped: Vec<String> = sections(&v, Group::Repository, Sort::Newest, now())
            .into_iter()
            .flat_map(|s| s.threads)
            .map(|t| t.id)
            .collect();

        // Sorting interleaves the two repositories; the screen does not, so a
        // shift-click has to run down the cards rather than down the sort.
        assert_eq!(ids(&v), ["1", "2", "3", "4", "5"]);
        assert_eq!(grouped, ["1", "2", "4", "3", "5"]);
    }

    #[test]
    fn a_shift_click_takes_the_run_between_two_rows() {
        let order: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();

        assert_eq!(range_between(&order, "b", "d").unwrap(), ["b", "c", "d"]);
        // Backwards is the same run.
        assert_eq!(range_between(&order, "d", "b").unwrap(), ["b", "c", "d"]);
        assert_eq!(range_between(&order, "c", "c").unwrap(), ["c"]);
    }

    #[test]
    fn a_shift_click_past_a_row_that_left_the_list_is_no_run_at_all() {
        let order: Vec<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        assert!(range_between(&order, "gone", "b").is_none());
        assert!(range_between(&order, "a", "gone").is_none());
        assert!(range_between(&[], "a", "b").is_none());
    }

    #[test]
    fn narrowing_is_detected_for_the_empty_state_message() {
        assert!(!Filters::default().is_narrowed());
        assert!(Filters { unread_only: true, ..Default::default() }.is_narrowed());
        assert!(Filters { search: "x".into(), ..Default::default() }.is_narrowed());
        assert!(!Filters { search: "  ".into(), ..Default::default() }.is_narrowed());
    }
}
