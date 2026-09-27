//! The sidebar's tabs and their lists: filtering and sorting items. Pure
//! functions over the declared repositories and the items the cache holds.
//!
//! Everything here runs in memory. Changing a setting never refetches, and
//! the sidebar is paintable from the cache alone.

use std::collections::HashSet;

use jiff::Timestamp;
use perch_github::{DeclaredRepo, Item, ItemSubject};

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

/// The three tabs at the top of the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarTab {
    /// GitHub's notifications.
    #[default]
    Inbox,
    /// Every worktree on disk.
    Worktrees,
    /// GitHub's fixed views, and what was opened lately.
    Browse,
}

impl SidebarTab {
    pub const ALL: [SidebarTab; 3] = [SidebarTab::Inbox, SidebarTab::Worktrees, SidebarTab::Browse];

    pub fn label(self) -> &'static str {
        match self {
            SidebarTab::Inbox => "Inbox",
            SidebarTab::Worktrees => "Worktrees",
            SidebarTab::Browse => "Browse",
        }
    }

    fn key(self) -> &'static str {
        match self {
            SidebarTab::Inbox => "inbox",
            SidebarTab::Worktrees => "worktrees",
            SidebarTab::Browse => "browse",
        }
    }

    fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == s)
    }
}

impl std::fmt::Display for SidebarTab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// What Browse lists: one of GitHub's fixed views, or Recent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BrowseView {
    ReviewRequested,
    Yours,
    Assigned,
    Mentioned,
    /// The open pull requests and issues of the repository picked beside
    /// it.
    Repository,
    /// What was opened lately and has no worktree. Local.
    #[default]
    Recent,
}

impl BrowseView {
    pub const ALL: [BrowseView; 6] = [
        BrowseView::ReviewRequested,
        BrowseView::Yours,
        BrowseView::Assigned,
        BrowseView::Mentioned,
        BrowseView::Repository,
        BrowseView::Recent,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BrowseView::ReviewRequested => "Review requested",
            BrowseView::Yours => "Yours",
            BrowseView::Assigned => "Assigned",
            BrowseView::Mentioned => "Mentioned",
            BrowseView::Repository => "By repository",
            BrowseView::Recent => "Recent",
        }
    }

    fn key(self) -> &'static str {
        match self {
            BrowseView::ReviewRequested => "review_requested",
            BrowseView::Yours => "yours",
            BrowseView::Assigned => "assigned",
            BrowseView::Mentioned => "mentioned",
            BrowseView::Repository => "repository",
            BrowseView::Recent => "recent",
        }
    }

    fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == s)
    }
}

impl std::fmt::Display for BrowseView {
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

    /// The time this sort orders by across all repositories.
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

/// Which end of the sort the rows start from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    Newest,
    Oldest,
}

impl Direction {
    pub const ALL: [Direction; 2] = [Direction::Newest, Direction::Oldest];

    pub fn label(self) -> &'static str {
        match self {
            Direction::Newest => "Newest first",
            Direction::Oldest => "Oldest first",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Direction::Newest => "newest",
            Direction::Oldest => "oldest",
        }
    }

    fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == s)
    }
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Everything the tabs, their filter menus and the search box hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemFilters {
    pub tab: SidebarTab,
    /// Repository identity for Inbox and Worktrees; `None` includes every
    /// repository.
    pub repository: Option<String>,
    pub sort: ItemSort,
    pub show_pr_status: bool,
    pub direction: Direction,
    /// Inbox's Show: only the unread threads.
    pub inbox_unread_only: bool,
    pub browse: BrowseView,
    /// The repository Browse › By repository lists, last picked.
    pub browse_repository: Option<String>,
    /// Browse's Open · Closed toggle.
    pub browse_closed: bool,
    pub query: String,
}

impl Default for ItemFilters {
    fn default() -> Self {
        Self {
            tab: SidebarTab::Inbox,
            repository: None,
            sort: ItemSort::LastActivity,
            show_pr_status: true,
            direction: Direction::Newest,
            inbox_unread_only: false,
            browse: BrowseView::Recent,
            browse_repository: None,
            browse_closed: false,
            query: String::new(),
        }
    }
}

impl ItemFilters {
    /// The form kept in the `meta` table: `key=value;` pairs. Search is
    /// deliberately transient. Repository identities are percent-encoded
    /// so local names can contain delimiters, percent signs, or spaces.
    pub fn to_meta(&self) -> String {
        format!(
            "tab={};repository={};sort={};show_pr_status={};direction={};unread={};browse={};browse_repository={};closed={}",
            self.tab.key(),
            self.repository.as_deref().map(encode_meta_value).unwrap_or_default(),
            self.sort.key(),
            u8::from(self.show_pr_status),
            self.direction.key(),
            u8::from(self.inbox_unread_only),
            self.browse.key(),
            self.browse_repository.as_deref().map(encode_meta_value).unwrap_or_default(),
            u8::from(self.browse_closed),
        )
    }

    /// Missing keys keep their defaults. The old scopes map onto the tabs:
    /// All is Worktrees, Notifications is Inbox, and Authored is Browse ›
    /// Yours. Settings that have gone are validated and ignored.
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut out = Self::default();
        for pair in s.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let (key, value) =
                pair.split_once('=').ok_or_else(|| format!("expected key=value, got {pair:?}"))?;
            let (key, value) = (key.trim(), value.trim());
            let bad = || format!("bad value for {key}: {value:?}");
            let repository = |value: &str| -> Result<Option<String>, String> {
                if value.is_empty() {
                    Ok(None)
                } else {
                    decode_meta_value(value).map(Some).ok_or_else(bad)
                }
            };
            match key {
                "tab" => out.tab = SidebarTab::from_key(value).ok_or_else(bad)?,
                "scope" => match value {
                    "all" => out.tab = SidebarTab::Worktrees,
                    "notifications" => out.tab = SidebarTab::Inbox,
                    "authored" => {
                        out.tab = SidebarTab::Browse;
                        out.browse = BrowseView::Yours;
                    }
                    _ => return Err(bad()),
                },
                "repository" => out.repository = repository(value)?,
                "status" if matches!(value, "active" | "closed" | "archived" | "all") => {}
                "status" => return Err(bad()),
                "kinds" => {
                    let known = value
                        .split(',')
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .all(|p| matches!(p, "pr" | "issue" | "branch"));
                    if !known {
                        return Err(bad());
                    }
                }
                "group" if matches!(value, "repo" | "owner" | "date") => {}
                "group" => return Err(bad()),
                "show_empty" | "saved" => {
                    parse_flag(value).ok_or_else(bad)?;
                }
                "sort" => out.sort = ItemSort::from_key(value).ok_or_else(bad)?,
                "show_pr_status" => out.show_pr_status = parse_flag(value).ok_or_else(bad)?,
                "direction" => out.direction = Direction::from_key(value).ok_or_else(bad)?,
                "unread" => out.inbox_unread_only = parse_flag(value).ok_or_else(bad)?,
                "browse" => out.browse = BrowseView::from_key(value).ok_or_else(bad)?,
                "browse_repository" => out.browse_repository = repository(value)?,
                "closed" => out.browse_closed = parse_flag(value).ok_or_else(bad)?,
                "query" => out.query = value.to_string(),
                _ => return Err(format!("unknown key {key:?}")),
            }
        }
        Ok(out)
    }

    /// The repository Browse is listing, while it shows By repository.
    pub fn browse_repository(&self) -> Option<&str> {
        (self.tab == SidebarTab::Browse && self.browse == BrowseView::Repository)
            .then_some(self.browse_repository.as_deref())
            .flatten()
    }
}

/// Whether the search matches the item. Case-insensitive substring over
/// the title as shown, the title before any Rename, the number, the
/// branch, and the repository's `owner/repo`, so a renamed pull request
/// is found by either name and `#57` finds number 57.
fn matches_query(item: &Item, query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    let number = item.subject.as_ref().map(|s| format!("#{}", s.number())).unwrap_or_default();
    [
        item.title(),
        item.given_title(),
        number.as_str(),
        item.branch().unwrap_or(""),
        item.repo_full_name.as_str(),
    ]
    .iter()
    .any(|hay| !hay.is_empty() && hay.to_lowercase().contains(&needle))
}

fn encode_meta_value(value: &str) -> String {
    value.bytes().fold(String::new(), |mut out, byte| {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.') {
            out.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(out, "%{byte:02X}").expect("writing to a String cannot fail");
        }
        out
    })
}

fn decode_meta_value(value: &str) -> Option<String> {
    let mut bytes = value.bytes();
    let mut out = Vec::new();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = char::from(bytes.next()?).to_digit(16)?;
            let low = char::from(bytes.next()?).to_digit(16)?;
            out.push((high * 16 + low) as u8);
        } else {
            out.push(byte);
        }
    }
    String::from_utf8(out).ok()
}

fn parse_flag(s: &str) -> Option<bool> {
    match s {
        "1" | "true" | "on" => Some(true),
        "0" | "false" | "off" => Some(false),
        _ => None,
    }
}

/// Whether the item is a worktree on disk, and so a row of Worktrees.
/// Everything else is Recent's.
pub fn in_worktrees(item: &Item) -> bool {
    !item.is_archived() && item.checkout.as_ref().is_some_and(|c| c.on_disk)
}

/// Worktrees: every worktree on disk under the repository filter and the
/// search, most recently pinned first and then by the sort.
pub fn worktree_items(items: &[Item], filters: &ItemFilters) -> Vec<Item> {
    let rows: Vec<&Item> = items
        .iter()
        .filter(|item| {
            in_worktrees(item)
                && matches_query(item, &filters.query)
                && filters
                    .repository
                    .as_ref()
                    .is_none_or(|repo| repo.eq_ignore_ascii_case(&item.repo_full_name))
        })
        .collect();
    let (mut pinned, mut rest): (Vec<&Item>, Vec<&Item>) = rows.into_iter().partition(|item| item.is_pinned());
    sort_pins(&mut pinned);
    sort_items(&mut rest, filters.sort, filters.direction);
    pinned.into_iter().chain(rest).cloned().collect()
}

/// How many unpinned rows Recent shows.
pub const RECENT_LIMIT: usize = 50;

/// Recent: what has no worktree on disk, pinned first, then the
/// `RECENT_LIMIT` most recently opened. The search narrows what is shown;
/// an older row is found by opening it again, which brings it back.
pub fn recent_items(items: &[Item], query: &str) -> Vec<Item> {
    let (mut pinned, mut rest): (Vec<&Item>, Vec<&Item>) =
        items.iter().filter(|item| !in_worktrees(item)).partition(|item| item.is_pinned());
    sort_pins(&mut pinned);
    rest.sort_by(|a, b| b.last_opened_at.cmp(&a.last_opened_at).then_with(|| a.id.cmp(&b.id)));
    rest.truncate(RECENT_LIMIT);
    pinned
        .into_iter()
        .chain(rest)
        .filter(|item| matches_query(item, query))
        .cloned()
        .collect()
}

/// The local items the tab in `filters` lists: Worktrees, Browse ›
/// Recent, or none, for Inbox and Browse's GitHub views.
pub fn local_items(items: &[Item], filters: &ItemFilters) -> Vec<Item> {
    match (filters.tab, filters.browse) {
        (SidebarTab::Worktrees, _) => worktree_items(items, filters),
        (SidebarTab::Browse, BrowseView::Recent) => recent_items(items, &filters.query),
        _ => Vec::new(),
    }
}

/// Whether a Work run may be sent on an item: a branch of the reader's
/// own, or a pull request the reader opened, with its checkout on disk.
/// Someone else's pull request, an issue, and anything archived are
/// Ask-only: their text and code are other people's.
pub fn work_allowed(item: &Item, viewer: Option<&str>) -> bool {
    if item.is_archived() {
        return false;
    }
    if !item.checkout.as_ref().is_some_and(|c| c.on_disk) {
        return false;
    }
    match &item.subject {
        None => true,
        Some(ItemSubject::PullRequest(pr)) => viewer
            .filter(|login| !login.is_empty())
            .is_some_and(|login| pr.author_login.eq_ignore_ascii_case(login)),
        Some(ItemSubject::Issue(_)) => false,
    }
}

/// Repository identities from config and the cache, including repositories
/// without visible rows. A declared spelling wins over the cache's spelling.
pub fn repository_options(declared: &[DeclaredRepo], items: &[Item]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut repositories: Vec<String> = declared
        .iter()
        .map(|repo| &repo.identity)
        .chain(items.iter().map(|item| &item.repo_full_name))
        .filter(|identity| seen.insert(identity.to_lowercase()))
        .cloned()
        .collect();
    repositories.sort_by_key(|identity| identity.to_lowercase());
    repositories
}

/// Most recently pinned first, ties by id.
fn sort_pins(items: &mut [&Item]) {
    items.sort_by(|a, b| b.pinned_at.cmp(&a.pinned_at).then_with(|| a.id.cmp(&b.id)));
}

/// Newest first, ties by id so the order never jitters between refreshes,
/// then the whole thing turned over for Oldest first. Reversing after the
/// sort rather than flipping the comparison keeps one ordering to reason
/// about, and the reversed tiebreak is the price.
fn sort_items(items: &mut [&Item], sort: ItemSort, direction: Direction) {
    items.sort_by(|a, b| sort.time(b).cmp(&sort.time(a)).then_with(|| a.id.cmp(&b.id)));
    if direction == Direction::Oldest {
        items.reverse();
    }
}

/// The row's second line always includes the repository: `owner/repo #57 · branch`,
/// `owner/repo #42`, or `owner/repo · branch`. A checkout
/// of a fork uses the fork's identity, as repository filtering does.
pub fn full_reference(item: &Item) -> String {
    let place = &item.repo_full_name;
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

#[cfg(test)]
mod tests {
    use super::*;
    use perch_github::{Issue, ItemCheckout, PullRequest};

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn pr(number: u64, title: &str, branch: &str, state: &str, updated: &str) -> ItemSubject {
        ItemSubject::PullRequest(PullRequest {
            number,
            html_url: format!("https://github.com/criccomini/perch/pull/{number}"),
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
            node_id: String::new(),
            reactions: Default::default(),
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
            divergence: None,
            source: None,
        }
    }

    fn item(id: &str, repo: &str, created: &str) -> Item {
        let mut i = Item::new(repo, at(created));
        i.id = id.into();
        i
    }

    /// Declared as `github = "owner/repo"` with no `name`, so each shows
    /// as the repo half, the way config.toml defaults it.
    fn declared(identities: &[&str]) -> Vec<DeclaredRepo> {
        identities
            .iter()
            .map(|id| DeclaredRepo {
                identity: id.to_string(),
                name: id.split_once('/').map_or(*id, |(_, repo)| repo).to_string(),
            })
            .collect()
    }

    /// Three declared repositories. criccomini/perch has an open pull
    /// request, a closed pull request, a merged one, an issue, an archived
    /// branch, and a plain branch. slatedb/slatedb has one open pull request.
    /// x/empty has nothing.
    fn fixture() -> (Vec<DeclaredRepo>, Vec<Item>) {
        let declared = declared(&["criccomini/perch", "slatedb/slatedb", "x/empty"]);

        let mut wanted = item("pr-wanted", "criccomini/perch", "2026-09-10T10:00:00Z");
        wanted.subject = Some(pr(57, "Fence the manifest", "fence", "open", "2026-09-17T10:00:00Z"));
        wanted.checkout = Some(checkout("fence"));

        let mut closed = item("pr-closed", "criccomini/perch", "2026-09-11T10:00:00Z");
        closed.subject = Some(pr(58, "Tracing spans", "spans", "closed", "2026-09-16T10:00:00Z"));

        let mut merged = item("pr-merged", "criccomini/perch", "2026-09-01T10:00:00Z");
        merged.subject = Some(pr(50, "Old thing", "old", "merged", "2026-09-02T10:00:00Z"));

        let mut bug = item("issue", "criccomini/perch", "2026-09-12T10:00:00Z");
        bug.subject = Some(issue(42, "Compaction stalls", "open", "2026-09-15T10:00:00Z"));

        let mut archived = item("branch-archived", "criccomini/perch", "2026-08-01T10:00:00Z");
        archived.checkout = Some(checkout("perch/issue-7"));
        archived.archived_at = Some(at("2026-09-01T10:00:00Z"));

        let mut branch = item("branch", "criccomini/perch", "2026-09-14T10:00:00Z");
        branch.checkout = Some(checkout("perch/issue-42"));
        branch.from_item_id = Some("issue".into());

        let mut other = item("slatedb-pr", "slatedb/slatedb", "2026-09-13T10:00:00Z");
        other.subject = Some(pr(2092, "Two-phase checkpoint", "checkpoint", "open", "2026-09-14T10:00:00Z"));
        if let Some(ItemSubject::PullRequest(p)) = &mut other.subject {
            p.repo_full_name = "slatedb/slatedb".into();
        }

        (declared, vec![wanted, closed, merged, bug, archived, branch, other])
    }

    fn ids(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn worktrees_are_the_checkouts_on_disk_and_recent_is_the_rest() {
        let (_, items) = fixture();
        assert_eq!(ids(&worktree_items(&items, &ItemFilters::default())), ["pr-wanted", "branch"]);
        // By when each was last opened, which the fixture leaves at when
        // it was made.
        assert_eq!(
            ids(&recent_items(&items, "")),
            ["slatedb-pr", "issue", "pr-closed", "pr-merged", "branch-archived"]
        );
    }

    #[test]
    fn a_worktree_that_goes_moves_to_recent() {
        let (_, mut items) = fixture();
        items[5].checkout.as_mut().unwrap().on_disk = false;
        assert_eq!(ids(&worktree_items(&items, &ItemFilters::default())), ["pr-wanted"]);
        assert!(ids(&recent_items(&items, "")).contains(&"branch"));

        let (_, mut items) = fixture();
        items[0].archived_at = Some(at("2026-09-18T10:00:00Z"));
        assert_eq!(ids(&worktree_items(&items, &ItemFilters::default())), ["branch"]);
        assert!(ids(&recent_items(&items, "")).contains(&"pr-wanted"));
    }

    #[test]
    fn worktrees_sort_and_pin_and_filter_by_repository() {
        let (_, mut items) = fixture();
        let filters = ItemFilters { sort: ItemSort::Created, ..Default::default() };
        assert_eq!(ids(&worktree_items(&items, &filters)), ["branch", "pr-wanted"]);
        let filters = ItemFilters { direction: Direction::Oldest, ..filters };
        assert_eq!(ids(&worktree_items(&items, &filters)), ["pr-wanted", "branch"]);

        items[0].pinned_at = Some(at("2026-09-18T10:00:00Z"));
        let filters = ItemFilters { sort: ItemSort::Created, ..Default::default() };
        assert_eq!(ids(&worktree_items(&items, &filters)), ["pr-wanted", "branch"], "the pin leads");

        items[0].repo_full_name = "chris/perch".into();
        let filters = ItemFilters { repository: Some("CHRIS/Perch".into()), ..Default::default() };
        assert_eq!(ids(&worktree_items(&items, &filters)), ["pr-wanted"]);
        let filters = ItemFilters { repository: Some("missing/repo".into()), ..Default::default() };
        assert!(worktree_items(&items, &filters).is_empty());
    }

    #[test]
    fn recent_keeps_pins_on_top_and_the_fifty_last_opened_after_them() {
        let base: Timestamp = at("2026-09-01T00:00:00Z");
        let mut items: Vec<Item> = (0..60)
            .map(|n| {
                let mut i = item(&format!("r{n:02}"), "a/b", "2026-09-01T00:00:00Z");
                i.subject = Some(issue(n + 1, "t", "open", "2026-09-01T00:00:00Z"));
                i.last_opened_at = base + jiff::SignedDuration::from_hours(n as i64);
                i
            })
            .collect();
        // The oldest two, pinned: they stay, however old.
        items[0].pinned_at = Some(at("2026-09-20T00:00:00Z"));
        items[1].pinned_at = Some(at("2026-09-21T00:00:00Z"));
        let rows = recent_items(&items, "");
        assert_eq!(rows.len(), 2 + RECENT_LIMIT);
        assert_eq!(ids(&rows)[..4], ["r01", "r00", "r59", "r58"]);
        assert!(!ids(&rows).contains(&"r09"), "fifty after the pins, the rest not shown");
        assert!(ids(&rows).contains(&"r10"));

        // Opening an old one brings it back to the top.
        items[5].last_opened_at = at("2026-09-25T00:00:00Z");
        assert_eq!(ids(&recent_items(&items, ""))[2], "r05");
    }

    #[test]
    fn search_hits_the_number_titles_branch_and_repository() {
        let (_, mut items) = fixture();
        items[0].name = Some("the fence one".into());
        let everything = |query: &str| {
            let mut rows = worktree_items(&items, &ItemFilters { query: query.into(), ..Default::default() });
            rows.extend(recent_items(&items, query));
            rows
        };
        assert_eq!(ids(&everything("#57")), ["pr-wanted"]);
        assert_eq!(ids(&everything("57")), ["pr-wanted"]);
        assert_eq!(ids(&everything("fence the manifest")), ["pr-wanted"]);
        assert_eq!(ids(&everything("fence one")), ["pr-wanted"]);
        assert_eq!(ids(&everything("ISSUE-42")), ["branch"]);
        assert_eq!(ids(&everything("slatedb")), ["slatedb-pr"]);
        assert_eq!(everything("criccomini/").len(), 6);
        assert!(everything("nothing like this").is_empty());
    }

    #[test]
    fn work_is_for_the_reader_s_own_branches_and_pull_requests() {
        let (_, items) = fixture();
        let mut branch = items
            .iter()
            .find(|i| i.subject.is_none() && i.checkout.is_some())
            .cloned()
            .expect("a branch in the fixture");
        branch.archived_at = None;
        branch.checkout.as_mut().unwrap().on_disk = true;
        assert!(work_allowed(&branch, None));
        branch.checkout.as_mut().unwrap().on_disk = false;
        assert!(!work_allowed(&branch, None), "nothing on disk to work in");
        branch.checkout.as_mut().unwrap().on_disk = true;
        branch.archived_at = Some(at("2026-09-18T10:00:00Z"));
        assert!(!work_allowed(&branch, None), "archived");

        let mut pull = items
            .iter()
            .find(|i| matches!(i.subject, Some(ItemSubject::PullRequest(_))))
            .cloned()
            .unwrap();
        pull.archived_at = None;
        pull.checkout = branch.checkout.clone();
        let author = match &pull.subject {
            Some(ItemSubject::PullRequest(pr)) => pr.author_login.clone(),
            _ => unreachable!(),
        };
        assert!(work_allowed(&pull, Some(&author.to_uppercase())), "the reader's own");
        assert!(!work_allowed(&pull, Some("someone-else")));
        assert!(!work_allowed(&pull, None), "nobody signed in");

        let mut issue = items
            .iter()
            .find(|i| matches!(i.subject, Some(ItemSubject::Issue(_))))
            .cloned()
            .unwrap();
        issue.archived_at = None;
        issue.checkout = branch.checkout.clone();
        assert!(!work_allowed(&issue, Some(&author)));
    }

    #[test]
    fn repository_options_include_declared_empty_and_undeclared_repositories_once() {
        let (mut declared, mut items) = fixture();
        declared[0].identity = "Criccomini/Perch".into();
        declared.push(DeclaredRepo { identity: "CRICCOMINI/PERCH".into(), name: "duplicate".into() });
        declared.push(DeclaredRepo { identity: "local/notes".into(), name: "notes".into() });
        items.push(item("extra", "Aardvark/first", "2026-09-13T10:00:00Z"));
        items.push(item("extra-case", "aardvark/FIRST", "2026-09-13T10:00:00Z"));
        assert_eq!(
            repository_options(&declared, &items),
            ["Aardvark/first", "Criccomini/Perch", "local/notes", "slatedb/slatedb", "x/empty",]
        );
    }

    #[test]
    fn the_second_line_always_names_the_repository_and_pr_head_branch() {
        let (_, items) = fixture();
        assert_eq!(full_reference(&items[0]), "criccomini/perch #57 · fence");
        assert_eq!(full_reference(&items[1]), "criccomini/perch #58 · spans");
        assert_eq!(full_reference(&items[3]), "criccomini/perch #42");
        assert_eq!(full_reference(&items[5]), "criccomini/perch · perch/issue-42");
    }

    #[test]
    fn filters_round_trip_with_all_preferences_and_safe_repository_encoding() {
        let filters = ItemFilters {
            tab: SidebarTab::Browse,
            repository: Some("local/notes;scope=all% ☃ ".into()),
            sort: ItemSort::LastOpened,
            show_pr_status: false,
            direction: Direction::Oldest,
            inbox_unread_only: true,
            browse: BrowseView::Repository,
            browse_repository: Some("slatedb/slatedb".into()),
            browse_closed: true,
            query: String::new(),
        };
        let meta = filters.to_meta();
        assert!(meta.contains("repository=local/notes%3Bscope%3Dall%25%20%E2%98%83%20;"));
        assert_eq!(ItemFilters::parse(&meta).unwrap(), filters);
        assert_eq!(filters.browse_repository(), Some("slatedb/slatedb"));
        assert_eq!(ItemFilters::parse("").unwrap(), ItemFilters::default());
        assert_eq!(ItemFilters::parse(&ItemFilters::default().to_meta()).unwrap(), ItemFilters::default());
        assert_eq!(ItemFilters::default().tab, SidebarTab::Inbox, "Inbox is where it opens");
        assert_eq!(
            ItemFilters::parse("repository=owner/repo").unwrap().repository.as_deref(),
            Some("owner/repo")
        );
        assert_eq!(ItemFilters::parse("repository=").unwrap().repository, None);
        for gone in [";group=", ";show_empty=", ";status=", ";kinds=", ";saved=", ";scope="] {
            assert!(!meta.contains(gone), "{gone}");
        }
    }

    #[test]
    fn the_old_scopes_map_onto_the_tabs_and_gone_settings_are_ignored() {
        let old = "status=closed;kinds=pr;scope=all;group=owner;sort=created;show_empty=1;show_pr_status=0;saved=1;direction=oldest";
        let filters = ItemFilters::parse(old).unwrap();
        assert_eq!(filters.tab, SidebarTab::Worktrees);
        assert_eq!(filters.sort, ItemSort::Created);
        assert!(!filters.show_pr_status);
        assert_eq!(filters.direction, Direction::Oldest);

        assert_eq!(ItemFilters::parse("scope=notifications").unwrap().tab, SidebarTab::Inbox);
        let authored = ItemFilters::parse("scope=authored").unwrap();
        assert_eq!((authored.tab, authored.browse), (SidebarTab::Browse, BrowseView::Yours));
        for group in ["repo", "owner", "date"] {
            assert_eq!(
                ItemFilters::parse(&format!("group={group};show_empty=0")).unwrap(),
                ItemFilters::default()
            );
        }
        assert_eq!(ItemFilters::parse("kinds=").unwrap(), ItemFilters::default());
    }

    #[test]
    fn search_is_transient() {
        let filters = ItemFilters { query: "fence".into(), ..Default::default() };
        assert!(!filters.to_meta().contains("fence"));
        assert_eq!(ItemFilters::parse("query=fence").unwrap().query, "fence");
    }

    #[test]
    fn invalid_settings_and_malformed_repository_encoding_are_rejected() {
        for (setting, key) in [
            ("status=open", "status"),
            ("colour=red", "colour"),
            ("kinds=pr,tags", "kinds"),
            ("group=bogus", "group"),
            ("show_empty=maybe", "show_empty"),
            ("saved=yes", "saved"),
            ("unread=yes", "unread"),
            ("closed=maybe", "closed"),
            ("attention=maybe", "attention"),
            ("reason=bogus", "reason"),
            ("direction=sideways", "direction"),
            ("scope=reviewing", "scope"),
            ("tab=elsewhere", "tab"),
            ("browse=everything", "browse"),
            ("repository=local/%", "repository"),
            ("repository=%ZZ", "repository"),
            ("browse_repository=%FF", "browse_repository"),
            ("nonsense", "key=value"),
        ] {
            assert!(ItemFilters::parse(setting).unwrap_err().contains(key), "{setting}");
        }
    }

    #[test]
    fn every_option_has_a_label_and_a_key_that_reads_back() {
        for value in SidebarTab::ALL {
            assert_eq!(SidebarTab::from_key(value.key()), Some(value));
            assert_eq!(value.to_string(), value.label());
        }
        for value in BrowseView::ALL {
            assert_eq!(BrowseView::from_key(value.key()), Some(value));
            assert_eq!(value.to_string(), value.label());
        }
        for value in ItemSort::ALL {
            assert_eq!(ItemSort::from_key(value.key()), Some(value));
            assert_eq!(value.to_string(), value.label());
        }
        for value in Direction::ALL {
            assert_eq!(Direction::from_key(value.key()), Some(value));
            assert_eq!(value.to_string(), value.label());
        }
    }

    #[test]
    fn row_kinds_remain_readable() {
        let (_, items) = fixture();
        assert_eq!(RowKind::of(&items[0]), RowKind::PullRequest);
        assert_eq!(RowKind::of(&items[3]), RowKind::Issue);
        assert_eq!(RowKind::of(&items[5]), RowKind::Branch);
        assert_eq!(RowKind::PullRequest.letter(), 'P');
        assert_eq!(RowKind::Issue.letter(), 'I');
        assert_eq!(RowKind::Branch.letter(), 'B');
    }
}
