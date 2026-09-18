//! The sidebar's list: filtering, grouping and sorting items. Pure functions
//! over the projects and items the cache holds.
//!
//! Everything here runs in memory, the way `filter` does for the inbox.
//! Changing a setting never refetches, and the sidebar is paintable from the
//! cache alone.

use std::collections::{HashMap, HashSet};

use jiff::Timestamp;
use perch_github::{Item, ItemSubject, Project};

use crate::timefmt::{bucket, DateBucket};

/// The Status row of the filter menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemStatus {
    #[default]
    Active,
    Closed,
    Archived,
    All,
}

impl ItemStatus {
    pub const ALL: [ItemStatus; 4] =
        [ItemStatus::Active, ItemStatus::Closed, ItemStatus::Archived, ItemStatus::All];

    pub fn label(self) -> &'static str {
        match self {
            ItemStatus::Active => "Active",
            ItemStatus::Closed => "Closed",
            ItemStatus::Archived => "Archived",
            ItemStatus::All => "All",
        }
    }

    /// Whether the status shows archived items. Priority and Pinned use
    /// this rather than `accepts`: those sections ignore the status filter
    /// otherwise, but an archived item was put away on purpose and should
    /// not keep nagging from the top of the list.
    pub fn shows_archived(self) -> bool {
        matches!(self, ItemStatus::Archived | ItemStatus::All)
    }

    fn accepts(self, item: &Item) -> bool {
        match self {
            ItemStatus::Active => item.is_active(),
            ItemStatus::Closed => item.is_closed() && !item.is_archived(),
            ItemStatus::Archived => item.is_archived(),
            ItemStatus::All => true,
        }
    }

    fn key(self) -> &'static str {
        match self {
            ItemStatus::Active => "active",
            ItemStatus::Closed => "closed",
            ItemStatus::Archived => "archived",
            ItemStatus::All => "all",
        }
    }

    fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == s)
    }
}

impl std::fmt::Display for ItemStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// The Kind row: a multi-select, so three flags rather than an enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemKinds {
    pub pull_requests: bool,
    pub issues: bool,
    pub branches: bool,
}

impl Default for ItemKinds {
    fn default() -> Self {
        Self { pull_requests: true, issues: true, branches: true }
    }
}

impl ItemKinds {
    pub fn accepts(self, item: &Item) -> bool {
        match RowKind::of(item) {
            RowKind::PullRequest => self.pull_requests,
            RowKind::Issue => self.issues,
            RowKind::Branch => self.branches,
        }
    }

    fn key(self) -> String {
        let mut parts = Vec::new();
        if self.pull_requests {
            parts.push("pr");
        }
        if self.issues {
            parts.push("issue");
        }
        if self.branches {
            parts.push("branch");
        }
        parts.join(",")
    }

    fn from_key(s: &str) -> Option<Self> {
        let mut kinds = Self { pull_requests: false, issues: false, branches: false };
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part {
                "pr" => kinds.pull_requests = true,
                "issue" => kinds.issues = true,
                "branch" => kinds.branches = true,
                _ => return None,
            }
        }
        Some(kinds)
    }
}

/// What an item's shape makes it. Not stored anywhere: derived from which
/// parts the item has, as the spec asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    PullRequest,
    Issue,
    Branch,
}

impl RowKind {
    pub fn of(item: &Item) -> Self {
        match &item.subject {
            Some(ItemSubject::PullRequest(_)) => RowKind::PullRequest,
            Some(ItemSubject::Issue(_)) => RowKind::Issue,
            None => RowKind::Branch,
        }
    }

    /// The letter `perch --items` prints where the window draws a glyph.
    pub fn letter(self) -> char {
        match self {
            RowKind::PullRequest => 'P',
            RowKind::Issue => 'I',
            RowKind::Branch => 'B',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemGroup {
    #[default]
    Project,
    Date,
}

impl ItemGroup {
    pub const ALL: [ItemGroup; 2] = [ItemGroup::Project, ItemGroup::Date];

    pub fn label(self) -> &'static str {
        match self {
            ItemGroup::Project => "Project",
            ItemGroup::Date => "Date",
        }
    }

    fn key(self) -> &'static str {
        match self {
            ItemGroup::Project => "project",
            ItemGroup::Date => "date",
        }
    }

    fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == s)
    }
}

impl std::fmt::Display for ItemGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemSort {
    #[default]
    LastActivity,
    LastOpened,
    Created,
}

impl ItemSort {
    pub const ALL: [ItemSort; 3] = [ItemSort::LastActivity, ItemSort::LastOpened, ItemSort::Created];

    pub fn label(self) -> &'static str {
        match self {
            ItemSort::LastActivity => "Last activity",
            ItemSort::LastOpened => "Last opened",
            ItemSort::Created => "Created",
        }
    }

    /// The time this sort orders by. Group by Date buckets on the same
    /// time, so the sections and the rows inside them agree.
    pub fn time(self, item: &Item) -> Timestamp {
        match self {
            ItemSort::LastActivity => item.last_activity(),
            ItemSort::LastOpened => item.last_opened_at,
            ItemSort::Created => item.created_at,
        }
    }

    fn key(self) -> &'static str {
        match self {
            ItemSort::LastActivity => "last_activity",
            ItemSort::LastOpened => "last_opened",
            ItemSort::Created => "created",
        }
    }

    fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == s)
    }
}

impl std::fmt::Display for ItemSort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Everything the filter menu and the search box hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemFilters {
    pub status: ItemStatus,
    pub kinds: ItemKinds,
    pub group: ItemGroup,
    pub sort: ItemSort,
    pub show_empty: bool,
    pub show_pr_status: bool,
    pub query: String,
}

impl Default for ItemFilters {
    fn default() -> Self {
        Self {
            status: ItemStatus::Active,
            kinds: ItemKinds::default(),
            group: ItemGroup::Project,
            sort: ItemSort::LastActivity,
            show_empty: false,
            show_pr_status: true,
            query: String::new(),
        }
    }
}

impl ItemFilters {
    /// The form kept in the `meta` table: `key=value;` pairs. The search is
    /// left out on purpose; a search box that comes back full at launch is
    /// a surprise, and the inbox's does not persist either.
    pub fn to_meta(&self) -> String {
        format!(
            "status={};kinds={};group={};sort={};show_empty={};show_pr_status={}",
            self.status.key(),
            self.kinds.key(),
            self.group.key(),
            self.sort.key(),
            u8::from(self.show_empty),
            u8::from(self.show_pr_status),
        )
    }

    /// The inverse of `to_meta`. A key that is missing keeps its default, so
    /// a setting added later reads an older string without fuss; a key or
    /// value that is not understood is an error, so a typo on the command
    /// line is not silently a default.
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut out = Self::default();
        for pair in s.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let (key, value) = pair.split_once('=').ok_or_else(|| format!("expected key=value, got {pair:?}"))?;
            let (key, value) = (key.trim(), value.trim());
            let bad = || format!("bad value for {key}: {value:?}");
            match key {
                "status" => out.status = ItemStatus::from_key(value).ok_or_else(bad)?,
                "kinds" => out.kinds = ItemKinds::from_key(value).ok_or_else(bad)?,
                "group" => out.group = ItemGroup::from_key(value).ok_or_else(bad)?,
                "sort" => out.sort = ItemSort::from_key(value).ok_or_else(bad)?,
                "show_empty" => out.show_empty = parse_flag(value).ok_or_else(bad)?,
                "show_pr_status" => out.show_pr_status = parse_flag(value).ok_or_else(bad)?,
                "query" => out.query = value.to_string(),
                _ => return Err(format!("unknown key {key:?}")),
            }
        }
        Ok(out)
    }

    /// Whether the search matches the item. Case-insensitive substring over
    /// the title as shown, the title before any Rename, the number, the
    /// branch, and the project name, so a renamed pull request is found by
    /// either name and `#57` finds number 57.
    fn matches_query(&self, item: &Item, project_name: &str) -> bool {
        let needle = self.query.trim().to_lowercase();
        if needle.is_empty() {
            return true;
        }
        let number = item.subject.as_ref().map(|s| format!("#{}", s.number())).unwrap_or_default();
        [
            item.title(),
            item.given_title(),
            number.as_str(),
            item.branch().unwrap_or(""),
            project_name,
        ]
        .iter()
        .any(|hay| !hay.is_empty() && hay.to_lowercase().contains(&needle))
    }
}

fn parse_flag(s: &str) -> Option<bool> {
    match s {
        "1" | "true" | "on" => Some(true),
        "0" | "false" | "off" => Some(false),
        _ => None,
    }
}

/// One project, or one date bucket, and its rows.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemSection {
    /// Stable identity for the view diffing key.
    pub key: String,
    pub title: String,
    /// For a project, its active items, before any filter. The badge says
    /// how much is going on there, whatever the list is narrowed to. For a
    /// date bucket, the rows it holds.
    pub count: usize,
    pub items: Vec<Item>,
}

/// What the sidebar draws, top to bottom.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sections {
    /// Items with attention newer than their Done mark, newest attention
    /// first. These still appear under their project.
    pub priority: Vec<Item>,
    /// Pinned items, most recently pinned first. These do not appear under
    /// their project.
    pub pinned: Vec<Item>,
    pub groups: Vec<ItemSection>,
}

/// Build the sidebar from the cache's projects and items.
///
/// Priority and Pinned ignore the group, sort and kind settings and respect
/// the search. The status filter touches them in one way only: archived
/// items stay out of both unless the status shows archived items. Group
/// sections take every filter.
pub fn sections(projects: &[Project], items: &[Item], filters: &ItemFilters, now: Timestamp) -> Sections {
    let names: HashMap<&str, &str> =
        projects.iter().map(|p| (p.id.as_str(), p.name.as_str())).collect();
    let searched: Vec<&Item> = items
        .iter()
        .filter(|i| {
            let name = names.get(i.project_id.as_str()).copied().unwrap_or(&i.project_id);
            filters.matches_query(i, name)
        })
        .collect();
    let top_level = |i: &Item| !i.is_archived() || filters.status.shows_archived();

    let mut priority: Vec<Item> = searched
        .iter()
        .copied()
        .filter(|i| i.needs_attention() && top_level(i))
        .cloned()
        .collect();
    // Ties broken by id so the order never jitters between refreshes.
    priority.sort_by(|a, b| b.attention_at.cmp(&a.attention_at).then_with(|| a.id.cmp(&b.id)));

    let mut pinned: Vec<Item> =
        searched.iter().copied().filter(|i| i.is_pinned() && top_level(i)).cloned().collect();
    pinned.sort_by(|a, b| b.pinned_at.cmp(&a.pinned_at).then_with(|| a.id.cmp(&b.id)));
    let in_pinned: HashSet<&str> = pinned.iter().map(|i| i.id.as_str()).collect();

    let mut rows: Vec<&Item> = searched
        .iter()
        .copied()
        .filter(|i| !in_pinned.contains(i.id.as_str()))
        .filter(|i| filters.status.accepts(i) && filters.kinds.accepts(i))
        .collect();
    sort_items(&mut rows, filters.sort);

    let groups = match filters.group {
        ItemGroup::Project => by_project(projects, items, &rows, filters.show_empty),
        ItemGroup::Date => by_date(&rows, filters.sort, now),
    };

    Sections { priority, pinned, groups }
}

fn sort_items(items: &mut [&Item], sort: ItemSort) {
    items.sort_by(|a, b| sort.time(b).cmp(&sort.time(a)).then_with(|| a.id.cmp(&b.id)));
}

/// One section per project in the order given, which is config order for
/// declared projects and creation order for implicit ones. An item whose
/// project is not in the list gets a section of its own at the end, named
/// by the id, rather than vanishing; the cache's foreign key means this
/// should not happen, but a row is better than a hole.
fn by_project(projects: &[Project], all: &[Item], rows: &[&Item], show_empty: bool) -> Vec<ItemSection> {
    let mut by_id: HashMap<&str, Vec<Item>> = HashMap::new();
    let mut extra: Vec<&str> = Vec::new();
    for item in rows {
        let id = item.project_id.as_str();
        if !by_id.contains_key(id) && !projects.iter().any(|p| p.id == id) {
            extra.push(id);
        }
        by_id.entry(id).or_default().push((*item).clone());
    }

    let mut active: HashMap<&str, usize> = HashMap::new();
    for item in all.iter().filter(|i| i.is_active()) {
        *active.entry(item.project_id.as_str()).or_default() += 1;
    }

    projects
        .iter()
        .map(|p| (p.id.as_str(), p.name.as_str()))
        .chain(extra.into_iter().map(|id| (id, id)))
        .filter_map(|(id, name)| {
            let items = by_id.remove(id).unwrap_or_default();
            if items.is_empty() && !show_empty {
                return None;
            }
            Some(ItemSection {
                key: format!("project:{id}"),
                title: name.to_string(),
                count: active.get(id).copied().unwrap_or(0),
                items,
            })
        })
        .collect()
}

/// Today, Yesterday, and the buckets the inbox uses, newest first. Rows are
/// bucketed on the time the sort orders by, so a section and its rows tell
/// the same story.
fn by_date(rows: &[&Item], sort: ItemSort, now: Timestamp) -> Vec<ItemSection> {
    let mut by_bucket: HashMap<DateBucket, Vec<Item>> = HashMap::new();
    for item in rows {
        by_bucket.entry(bucket(sort.time(item), now)).or_default().push((*item).clone());
    }
    let mut buckets: Vec<DateBucket> = by_bucket.keys().copied().collect();
    buckets.sort();
    buckets
        .into_iter()
        .map(|b| {
            let items = by_bucket.remove(&b).unwrap_or_default();
            ItemSection {
                key: format!("date:{}", b.label()),
                title: b.label().to_string(),
                count: items.len(),
                items,
            }
        })
        .collect()
}

/// The row's second line: `owner/repo #57 · branch` for a pull request,
/// `owner/repo #42` for an issue, `owner/repo · branch` for a checkout with
/// no subject. Under Group by Date the project name stands in for
/// `owner/repo`, since the date sections no longer say which project a row
/// belongs to.
pub fn reference(item: &Item, project: Option<&Project>, group: ItemGroup) -> String {
    let place = match (group, project) {
        (ItemGroup::Date, Some(p)) => p.name.as_str(),
        _ => item.repo_full_name.as_str(),
    };
    match &item.subject {
        Some(ItemSubject::PullRequest(pr)) => {
            let branch = item.branch().unwrap_or(pr.head_ref.as_str());
            if branch.is_empty() {
                format!("{place} #{}", pr.number)
            } else {
                format!("{place} #{} · {branch}", pr.number)
            }
        }
        Some(ItemSubject::Issue(issue)) => format!("{place} #{}", issue.number),
        None => match item.branch() {
            Some(branch) => format!("{place} · {branch}"),
            None => place.to_string(),
        },
    }
}

/// The words a Priority row says next to its dot, from the reason the
/// attention was set with.
pub fn attention_label(reason: &str) -> String {
    match reason {
        "review_requested" => "review requested".into(),
        "mention" | "team_mention" => "mentioned".into(),
        "comment" => "commented".into(),
        "ci_failed" => "CI failed".into(),
        other => other.replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_github::{Issue, ItemCheckout, PullRequest};

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn now() -> Timestamp {
        at("2026-09-18T12:00:00Z")
    }

    fn project(name: &str, repo: &str) -> Project {
        Project {
            id: Project::id_for(name),
            name: name.into(),
            repos: vec![repo.into()],
            roots: vec![],
            implicit: false,
        }
    }

    fn pr(number: u64, title: &str, branch: &str, state: &str, updated: &str) -> ItemSubject {
        ItemSubject::PullRequest(PullRequest {
            number,
            title: title.into(),
            author_login: "a".into(),
            author_avatar_url: String::new(),
            head_sha: "h".into(),
            head_ref: branch.into(),
            base_sha: "b".into(),
            base_ref: "main".into(),
            draft: false,
            merged: state == "merged",
            state: if state == "merged" { "closed".into() } else { state.into() },
            additions: 0,
            deletions: 0,
            changed_files: 0,
            repo_full_name: "criccomini/perch".into(),
            head_repo_full_name: None,
            updated_at: Some(at(updated)),
            review_requested: false,
            checks: None,
            review_decision: None,
            merged_at: None,
            merged_by: None,
            closed_at: None,
            requested_reviewers: vec![],
            milestone: None,
            labels: vec![],
            assignees: vec![],
        })
    }

    fn issue(number: u64, title: &str, state: &str, updated: &str) -> ItemSubject {
        ItemSubject::Issue(Issue {
            number,
            title: title.into(),
            body: String::new(),
            state: state.into(),
            state_reason: None,
            author_login: "a".into(),
            author_avatar_url: String::new(),
            labels: vec![],
            assignees: vec![],
            comments: 0,
            created_at: None,
            updated_at: at(updated),
            repo_full_name: "criccomini/perch".into(),
        })
    }

    fn checkout(branch: &str) -> ItemCheckout {
        ItemCheckout {
            branch: branch.into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: format!("/w/{branch}").into(),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
        }
    }

    fn item(id: &str, project: &str, repo: &str, created: &str) -> Item {
        let mut i = Item::new(project, repo, at(created));
        i.id = id.into();
        i
    }

    /// Two projects. perch has a pull request with attention, a done pull
    /// request, a merged one, an issue, an archived branch, and a plain
    /// branch. slatedb has one open pull request. empty has nothing.
    fn fixture() -> (Vec<Project>, Vec<Item>) {
        let projects = vec![
            project("perch", "criccomini/perch"),
            project("slatedb", "slatedb/slatedb"),
            project("empty", "x/empty"),
        ];

        let mut wanted = item("pr-wanted", "perch", "criccomini/perch", "2026-09-10T10:00:00Z");
        wanted.subject = Some(pr(57, "Fence the manifest", "fence", "open", "2026-09-17T10:00:00Z"));
        wanted.checkout = Some(checkout("fence"));
        wanted.attention_at = Some(at("2026-09-18T09:00:00Z"));
        wanted.attention_reason = Some("review_requested".into());

        let mut done = item("pr-done", "perch", "criccomini/perch", "2026-09-11T10:00:00Z");
        done.subject = Some(pr(58, "Tracing spans", "spans", "open", "2026-09-16T10:00:00Z"));
        done.attention_at = Some(at("2026-09-16T10:00:00Z"));
        done.done_at = Some(at("2026-09-16T10:00:00Z"));

        let mut merged = item("pr-merged", "perch", "criccomini/perch", "2026-09-01T10:00:00Z");
        merged.subject = Some(pr(50, "Old thing", "old", "merged", "2026-09-02T10:00:00Z"));

        let mut bug = item("issue", "perch", "criccomini/perch", "2026-09-12T10:00:00Z");
        bug.subject = Some(issue(42, "Compaction stalls", "open", "2026-09-15T10:00:00Z"));

        let mut archived = item("branch-archived", "perch", "criccomini/perch", "2026-08-01T10:00:00Z");
        archived.checkout = Some(checkout("perch/issue-7"));
        archived.archived_at = Some(at("2026-09-01T10:00:00Z"));

        let mut branch = item("branch", "perch", "criccomini/perch", "2026-09-14T10:00:00Z");
        branch.checkout = Some(checkout("perch/issue-42"));
        branch.from_item_id = Some("issue".into());

        let mut other = item("slatedb-pr", "slatedb", "slatedb/slatedb", "2026-09-13T10:00:00Z");
        other.subject = Some(pr(2092, "Two-phase checkpoint", "checkpoint", "open", "2026-09-14T10:00:00Z"));
        if let Some(ItemSubject::PullRequest(p)) = &mut other.subject {
            p.repo_full_name = "slatedb/slatedb".into();
        }

        (projects, vec![wanted, done, merged, bug, archived, branch, other])
    }

    fn ids(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.id.as_str()).collect()
    }

    fn section_ids(s: &ItemSection) -> Vec<&str> {
        ids(&s.items)
    }

    #[test]
    fn priority_holds_only_items_with_attention_past_done() {
        let (projects, mut items) = fixture();
        // A second wanted item, older attention, so the order is checked too.
        let mut older = item("issue-mentioned", "perch", "criccomini/perch", "2026-09-12T10:00:00Z");
        older.subject = Some(issue(43, "Mentioned issue", "open", "2026-09-15T10:00:00Z"));
        older.attention_at = Some(at("2026-09-17T09:00:00Z"));
        older.attention_reason = Some("mention".into());
        items.push(older);

        let s = sections(&projects, &items, &ItemFilters::default(), now());
        assert_eq!(ids(&s.priority), ["pr-wanted", "issue-mentioned"]);
        // Done is not in there, and neither is anything without attention.
        assert!(s.priority.iter().all(Item::needs_attention));
    }

    #[test]
    fn priority_keeps_the_item_under_its_project_too() {
        let (projects, items) = fixture();
        let s = sections(&projects, &items, &ItemFilters::default(), now());
        assert!(section_ids(&s.groups[0]).contains(&"pr-wanted"));
    }

    #[test]
    fn priority_leaves_out_an_archived_item_unless_the_status_shows_archived() {
        let (projects, mut items) = fixture();
        items[4].attention_at = Some(at("2026-09-18T11:00:00Z"));
        items[4].attention_reason = Some("comment".into());

        let s = sections(&projects, &items, &ItemFilters::default(), now());
        assert_eq!(ids(&s.priority), ["pr-wanted"]);

        let f = ItemFilters { status: ItemStatus::Archived, ..Default::default() };
        let s = sections(&projects, &items, &f, now());
        assert_eq!(ids(&s.priority), ["branch-archived", "pr-wanted"]);
    }

    #[test]
    fn priority_ignores_the_kind_filter() {
        let (projects, items) = fixture();
        let f = ItemFilters {
            kinds: ItemKinds { pull_requests: false, issues: true, branches: true },
            ..Default::default()
        };
        let s = sections(&projects, &items, &f, now());
        assert_eq!(ids(&s.priority), ["pr-wanted"]);
        assert!(!section_ids(&s.groups[0]).contains(&"pr-wanted"));
    }

    #[test]
    fn active_excludes_closed_done_and_archived() {
        let (projects, items) = fixture();
        let s = sections(&projects, &items, &ItemFilters::default(), now());
        let perch = &s.groups[0];
        assert_eq!(perch.title, "perch");
        let rows = section_ids(perch);
        assert!(rows.contains(&"pr-wanted"));
        assert!(rows.contains(&"issue"));
        assert!(rows.contains(&"branch"));
        assert!(!rows.contains(&"pr-done"), "done is closed");
        assert!(!rows.contains(&"pr-merged"), "merged is closed");
        assert!(!rows.contains(&"branch-archived"));
        assert_eq!(perch.count, 3);
    }

    #[test]
    fn closed_holds_merged_and_done_but_not_archived() {
        let (projects, items) = fixture();
        let f = ItemFilters { status: ItemStatus::Closed, ..Default::default() };
        let s = sections(&projects, &items, &f, now());
        assert_eq!(s.groups.len(), 1);
        assert_eq!(section_ids(&s.groups[0]), ["pr-done", "pr-merged"]);
        // The badge still counts what is active there.
        assert_eq!(s.groups[0].count, 3);
    }

    #[test]
    fn archived_and_all_show_what_they_say() {
        let (projects, items) = fixture();
        let f = ItemFilters { status: ItemStatus::Archived, ..Default::default() };
        let s = sections(&projects, &items, &f, now());
        assert_eq!(section_ids(&s.groups[0]), ["branch-archived"]);

        let f = ItemFilters { status: ItemStatus::All, ..Default::default() };
        let s = sections(&projects, &items, &f, now());
        let total: usize = s.groups.iter().map(|g| g.items.len()).sum();
        assert_eq!(total, items.len());
    }

    #[test]
    fn kinds_come_from_the_shape_of_the_item() {
        let (projects, items) = fixture();
        let f = ItemFilters {
            status: ItemStatus::All,
            kinds: ItemKinds { pull_requests: false, issues: false, branches: true },
            ..Default::default()
        };
        let s = sections(&projects, &items, &f, now());
        assert_eq!(section_ids(&s.groups[0]), ["branch", "branch-archived"]);

        let f = ItemFilters {
            status: ItemStatus::All,
            kinds: ItemKinds { pull_requests: false, issues: true, branches: false },
            ..Default::default()
        };
        let s = sections(&projects, &items, &f, now());
        assert_eq!(section_ids(&s.groups[0]), ["issue"]);
    }

    #[test]
    fn projects_keep_their_given_order_and_hide_when_empty() {
        let (projects, items) = fixture();
        let s = sections(&projects, &items, &ItemFilters::default(), now());
        let titles: Vec<&str> = s.groups.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["perch", "slatedb"]);

        let f = ItemFilters { show_empty: true, ..Default::default() };
        let s = sections(&projects, &items, &f, now());
        let titles: Vec<&str> = s.groups.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["perch", "slatedb", "empty"]);
        assert_eq!(s.groups[2].count, 0);
        assert!(s.groups[2].items.is_empty());
    }

    #[test]
    fn an_item_whose_project_is_unknown_still_gets_a_row() {
        let (projects, mut items) = fixture();
        items[6].project_id = "ghost".into();
        let s = sections(&projects, &items, &ItemFilters::default(), now());
        let titles: Vec<&str> = s.groups.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["perch", "ghost"]);
    }

    #[test]
    fn sorts_order_by_the_time_they_name() {
        let (projects, mut items) = fixture();
        items[3].last_opened_at = at("2026-09-18T11:00:00Z");
        items[5].last_opened_at = at("2026-09-18T10:00:00Z");
        items[0].last_opened_at = at("2026-09-18T09:00:00Z");

        let by = |sort: ItemSort| {
            let f = ItemFilters { sort, ..Default::default() };
            let s = sections(&projects, &items, &f, now());
            section_ids(&s.groups[0]).into_iter().map(String::from).collect::<Vec<_>>()
        };
        // Attention at the 18th, an issue updated on the 15th, a branch made on the 14th.
        assert_eq!(by(ItemSort::LastActivity), ["pr-wanted", "issue", "branch"]);
        assert_eq!(by(ItemSort::LastOpened), ["issue", "branch", "pr-wanted"]);
        // Made on the 14th, 12th, 10th.
        assert_eq!(by(ItemSort::Created), ["branch", "issue", "pr-wanted"]);
    }

    #[test]
    fn group_by_date_buckets_on_the_sort_time_newest_first() {
        let now = Timestamp::now();
        let ago = |h: i64| now - jiff::SignedDuration::from_hours(h);
        let (projects, _) = fixture();
        let mut a = item("a", "perch", "criccomini/perch", "2026-09-01T10:00:00Z");
        a.checkout = Some(checkout("a"));
        a.attention_at = Some(ago(2));
        let mut b = item("b", "perch", "criccomini/perch", "2026-09-01T10:00:00Z");
        b.checkout = Some(checkout("b"));
        b.attention_at = Some(ago(26));
        let mut c = item("c", "slatedb", "slatedb/slatedb", "2026-09-01T10:00:00Z");
        c.checkout = Some(checkout("c"));
        c.attention_at = Some(ago(24 * 40));
        let items = vec![c.clone(), b.clone(), a.clone()];

        let f = ItemFilters { group: ItemGroup::Date, ..Default::default() };
        let s = sections(&projects, &items, &f, now);
        let titles: Vec<&str> = s.groups.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["Today", "Yesterday", "Earlier"]);
        assert_eq!(section_ids(&s.groups[0]), ["a"]);
        assert_eq!(s.groups[0].count, 1);

        // Created puts all three on the same day.
        let f = ItemFilters { group: ItemGroup::Date, sort: ItemSort::Created, ..Default::default() };
        let s = sections(&projects, &items, &f, now);
        assert_eq!(s.groups.len(), 1);
        assert_eq!(section_ids(&s.groups[0]), ["a", "b", "c"]);
    }

    #[test]
    fn group_by_date_puts_the_project_in_the_reference() {
        let (projects, items) = fixture();
        let perch = Some(&projects[0]);
        assert_eq!(reference(&items[0], perch, ItemGroup::Project), "criccomini/perch #57 · fence");
        assert_eq!(reference(&items[0], perch, ItemGroup::Date), "perch #57 · fence");
        assert_eq!(reference(&items[3], perch, ItemGroup::Project), "criccomini/perch #42");
        assert_eq!(reference(&items[3], perch, ItemGroup::Date), "perch #42");
        assert_eq!(reference(&items[5], perch, ItemGroup::Project), "criccomini/perch · perch/issue-42");
        assert_eq!(reference(&items[5], perch, ItemGroup::Date), "perch · perch/issue-42");
        // No project to name: the repository stands in.
        assert_eq!(reference(&items[5], None, ItemGroup::Date), "criccomini/perch · perch/issue-42");
    }

    #[test]
    fn a_pull_request_without_a_checkout_names_its_head_branch() {
        let (_, items) = fixture();
        assert_eq!(reference(&items[1], None, ItemGroup::Project), "criccomini/perch #58 · spans");
    }

    #[test]
    fn search_hits_branch_names() {
        let (projects, items) = fixture();
        let f = ItemFilters { query: "ISSUE-42".into(), ..Default::default() };
        let s = sections(&projects, &items, &f, now());
        assert_eq!(s.groups.len(), 1);
        assert_eq!(section_ids(&s.groups[0]), ["branch"]);
        // The pull request with attention does not match, so Priority is empty.
        assert!(s.priority.is_empty());
    }

    #[test]
    fn search_hits_the_number_the_title_the_name_and_the_project() {
        let (projects, mut items) = fixture();
        items[0].name = Some("the fence one".into());

        let hits = |q: &str| {
            let f = ItemFilters { status: ItemStatus::All, query: q.into(), ..Default::default() };
            let s = sections(&projects, &items, &f, now());
            s.groups.iter().flat_map(|g| g.items.iter().map(|i| i.id.clone())).collect::<Vec<_>>()
        };
        assert_eq!(hits("#57"), ["pr-wanted"]);
        assert_eq!(hits("57"), ["pr-wanted"]);
        assert_eq!(hits("fence the manifest"), ["pr-wanted"], "the given title still matches");
        assert_eq!(hits("fence one"), ["pr-wanted"], "and so does the name");
        assert_eq!(hits("slatedb").len(), 1, "the project name matches");
        assert!(hits("nothing like this").is_empty());
    }

    #[test]
    fn pinned_items_are_not_repeated_under_their_project() {
        let (projects, mut items) = fixture();
        items[3].pinned_at = Some(at("2026-09-17T10:00:00Z"));
        items[5].pinned_at = Some(at("2026-09-18T10:00:00Z"));

        let s = sections(&projects, &items, &ItemFilters::default(), now());
        assert_eq!(ids(&s.pinned), ["branch", "issue"], "most recently pinned first");
        assert_eq!(section_ids(&s.groups[0]), ["pr-wanted"]);
        // The badge is not narrowed by pinning.
        assert_eq!(s.groups[0].count, 3);
    }

    #[test]
    fn pinned_is_hidden_when_empty_and_respects_the_search() {
        let (projects, mut items) = fixture();
        assert!(sections(&projects, &items, &ItemFilters::default(), now()).pinned.is_empty());

        items[3].pinned_at = Some(at("2026-09-17T10:00:00Z"));
        let f = ItemFilters { query: "fence".into(), ..Default::default() };
        let s = sections(&projects, &items, &f, now());
        assert!(s.pinned.is_empty());
        assert_eq!(section_ids(&s.groups[0]), ["pr-wanted"]);
    }

    #[test]
    fn filter_settings_round_trip_through_the_string_form() {
        let f = ItemFilters {
            status: ItemStatus::Closed,
            kinds: ItemKinds { pull_requests: true, issues: false, branches: true },
            group: ItemGroup::Date,
            sort: ItemSort::LastOpened,
            show_empty: true,
            show_pr_status: false,
            query: String::new(),
        };
        let s = f.to_meta();
        assert_eq!(
            s,
            "status=closed;kinds=pr,branch;group=date;sort=last_opened;show_empty=1;show_pr_status=0"
        );
        assert_eq!(ItemFilters::parse(&s).unwrap(), f);

        let d = ItemFilters::default();
        assert_eq!(ItemFilters::parse(&d.to_meta()).unwrap(), d);
        assert_eq!(ItemFilters::parse("").unwrap(), d, "nothing stored means the defaults");
    }

    #[test]
    fn the_search_is_not_persisted_but_can_be_given() {
        let f = ItemFilters { query: "fence".into(), ..Default::default() };
        assert!(!f.to_meta().contains("fence"));
        assert_eq!(ItemFilters::parse("query=fence").unwrap().query, "fence");
    }

    #[test]
    fn a_string_with_no_kinds_means_none() {
        let f = ItemFilters::parse("kinds=").unwrap();
        assert_eq!(f.kinds, ItemKinds { pull_requests: false, issues: false, branches: false });
    }

    #[test]
    fn a_bad_setting_string_says_what_was_wrong() {
        assert!(ItemFilters::parse("status=open").unwrap_err().contains("status"));
        assert!(ItemFilters::parse("colour=red").unwrap_err().contains("colour"));
        assert!(ItemFilters::parse("kinds=pr,tags").unwrap_err().contains("kinds"));
        assert!(ItemFilters::parse("show_empty=maybe").unwrap_err().contains("show_empty"));
        assert!(ItemFilters::parse("nonsense").unwrap_err().contains("key=value"));
    }

    #[test]
    fn every_option_has_a_label_and_a_key_that_reads_back() {
        for v in ItemStatus::ALL {
            assert_eq!(ItemStatus::from_key(v.key()), Some(v));
            assert_eq!(v.to_string(), v.label());
        }
        for v in ItemGroup::ALL {
            assert_eq!(ItemGroup::from_key(v.key()), Some(v));
            assert_eq!(v.to_string(), v.label());
        }
        for v in ItemSort::ALL {
            assert_eq!(ItemSort::from_key(v.key()), Some(v));
            assert_eq!(v.to_string(), v.label());
        }
    }

    #[test]
    fn attention_reasons_read_as_the_spec_words_them() {
        assert_eq!(attention_label("review_requested"), "review requested");
        assert_eq!(attention_label("mention"), "mentioned");
        assert_eq!(attention_label("comment"), "commented");
        assert_eq!(attention_label("manual"), "manual");
        assert_eq!(attention_label("ci_failed"), "CI failed");
        assert_eq!(attention_label("state_change"), "state change");
    }

    #[test]
    fn row_kinds_and_letters_follow_the_shape() {
        let (_, items) = fixture();
        assert_eq!(RowKind::of(&items[0]), RowKind::PullRequest);
        assert_eq!(RowKind::of(&items[3]), RowKind::Issue);
        assert_eq!(RowKind::of(&items[5]), RowKind::Branch);
        assert_eq!(RowKind::PullRequest.letter(), 'P');
        assert_eq!(RowKind::Issue.letter(), 'I');
        assert_eq!(RowKind::Branch.letter(), 'B');
    }
}
