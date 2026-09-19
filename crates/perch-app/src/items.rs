//! The sidebar's flat list: filtering and sorting items. Pure functions
//! over the declared repositories and the items the cache holds.
//!
//! Everything here runs in memory. Changing a setting never refetches, and
//! the sidebar is paintable from the cache alone.

use std::collections::HashSet;

use jiff::Timestamp;
use perch_github::{DeclaredRepo, Item, ItemSubject};

/// The Status row of the filter menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemStatus {
    #[default]
    Active,
    Closed,
    Done,
    Archived,
    All,
}

impl ItemStatus {
    pub const ALL: [ItemStatus; 5] =
        [ItemStatus::Active, ItemStatus::Closed, ItemStatus::Done, ItemStatus::Archived, ItemStatus::All];

    pub fn label(self) -> &'static str {
        match self {
            ItemStatus::Active => "Active",
            ItemStatus::Closed => "Closed",
            ItemStatus::Done => "Done",
            ItemStatus::Archived => "Archived",
            ItemStatus::All => "All",
        }
    }

    /// Whether the status shows archived items. Pinned uses this rather
    /// than `accepts`: pinned rows ignore the status filter otherwise,
    /// but an archived item was put away on purpose and should not keep
    /// nagging from the top of the list.
    pub fn shows_archived(self) -> bool {
        matches!(self, ItemStatus::Archived | ItemStatus::All)
    }

    /// Closed and Done do not overlap. `Item::is_closed` counts a done item
    /// as closed, which is right for Active, so Closed subtracts Done here
    /// rather than the item changing what closed means.
    fn accepts(self, item: &Item) -> bool {
        match self {
            ItemStatus::Active => item.is_active(),
            ItemStatus::Closed => item.is_closed() && !item.is_done() && !item.is_archived(),
            ItemStatus::Done => item.is_done() && !item.is_archived(),
            ItemStatus::Archived => item.is_archived(),
            ItemStatus::All => true,
        }
    }

    fn key(self) -> &'static str {
        match self {
            ItemStatus::Active => "active",
            ItemStatus::Closed => "closed",
            ItemStatus::Done => "done",
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

/// The three scopes above the sidebar search box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemScope {
    #[default]
    All,
    Notifications,
    Authored,
}

impl ItemScope {
    pub const ALL: [ItemScope; 3] = [ItemScope::All, ItemScope::Notifications, ItemScope::Authored];

    pub fn label(self) -> &'static str {
        match self {
            ItemScope::All => "All",
            ItemScope::Notifications => "Notifications",
            ItemScope::Authored => "Authored",
        }
    }

    fn accepts(self, item: &Item, viewer: Option<&str>) -> bool {
        match self {
            ItemScope::All => true,
            ItemScope::Notifications => item.needs_attention(),
            ItemScope::Authored => viewer.filter(|login| !login.is_empty()).is_some_and(|viewer| {
                let author = match &item.subject {
                    Some(ItemSubject::PullRequest(pr)) => &pr.author_login,
                    Some(ItemSubject::Issue(issue)) => &issue.author_login,
                    None => return false,
                };
                author.eq_ignore_ascii_case(viewer)
            }),
        }
    }

    fn key(self) -> &'static str {
        match self {
            ItemScope::All => "all",
            ItemScope::Notifications => "notifications",
            ItemScope::Authored => "authored",
        }
    }

    fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == s)
    }
}

impl std::fmt::Display for ItemScope {
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

/// Why an item was given attention: the strings `attention_reason` holds,
/// as the Reason submenu offers them. A `team_mention` is a mention; a
/// string not here matches only Any and is shown by `attention_label`'s
/// fallback rather than lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionReason {
    ReviewRequested,
    Mention,
    Assign,
    Comment,
    CiFailed,
    Manual,
    /// The reader opened the pull request or issue. The poll writes it and
    /// it makes attention, so a reader can narrow to their own.
    Author,
    /// Something happened on a thread the reader watches: a push, a
    /// review, a comment by somebody else. Most of an inbox.
    Subscribed,
    /// Merged, closed, or reopened.
    StateChange,
}

impl AttentionReason {
    pub const ALL: [AttentionReason; 9] = [
        AttentionReason::ReviewRequested,
        AttentionReason::Mention,
        AttentionReason::Assign,
        AttentionReason::Comment,
        AttentionReason::CiFailed,
        AttentionReason::Manual,
        AttentionReason::Author,
        AttentionReason::Subscribed,
        AttentionReason::StateChange,
    ];

    /// The reason in words, as `perch --items` prints it beside a row
    /// that needs attention.
    pub fn label(self) -> &'static str {
        match self {
            AttentionReason::ReviewRequested => "review requested",
            AttentionReason::Mention => "mentioned",
            AttentionReason::Assign => "assigned",
            AttentionReason::Comment => "commented",
            AttentionReason::CiFailed => "CI failed",
            AttentionReason::Manual => "manual",
            AttentionReason::Author => "authored",
            AttentionReason::Subscribed => "subscribed",
            AttentionReason::StateChange => "state changed",
        }
    }

    /// The menu's wording, sentence case, since a menu item is a title and
    /// a row's suffix is not.
    pub fn title(self) -> &'static str {
        match self {
            AttentionReason::ReviewRequested => "Review requested",
            AttentionReason::Mention => "Mentioned",
            AttentionReason::Assign => "Assigned",
            AttentionReason::Comment => "Commented",
            AttentionReason::CiFailed => "CI failed",
            AttentionReason::Manual => "Manual",
            AttentionReason::Author => "Authored",
            AttentionReason::Subscribed => "Subscribed",
            AttentionReason::StateChange => "State changed",
        }
    }

    /// The wire string the poll stores, which is also the persisted form.
    fn key(self) -> &'static str {
        match self {
            AttentionReason::ReviewRequested => "review_requested",
            AttentionReason::Mention => "mention",
            AttentionReason::Assign => "assign",
            AttentionReason::Comment => "comment",
            AttentionReason::CiFailed => "ci_failed",
            AttentionReason::Manual => "manual",
            AttentionReason::Author => "author",
            AttentionReason::Subscribed => "subscribed",
            AttentionReason::StateChange => "state_change",
        }
    }

    /// The reason a stored string names, or `None` for one the menu does
    /// not offer.
    pub fn from_str(s: &str) -> Option<Self> {
        if s == "team_mention" {
            return Some(AttentionReason::Mention);
        }
        Self::ALL.into_iter().find(|v| v.key() == s)
    }
}

impl std::fmt::Display for AttentionReason {
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

/// Everything the filter menu and the search box hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemFilters {
    pub status: ItemStatus,
    pub kinds: ItemKinds,
    pub scope: ItemScope,
    /// Repository identity; `None` includes every repository.
    pub repository: Option<String>,
    pub sort: ItemSort,
    pub show_pr_status: bool,
    /// Only saved items, whatever the status.
    pub saved_only: bool,
    /// Only items whose latest reason is this one; `None` is Any.
    pub reason: Option<AttentionReason>,
    pub direction: Direction,
    pub query: String,
}

impl Default for ItemFilters {
    fn default() -> Self {
        Self {
            status: ItemStatus::Active,
            kinds: ItemKinds::default(),
            scope: ItemScope::All,
            repository: None,
            sort: ItemSort::LastActivity,
            show_pr_status: true,
            saved_only: false,
            reason: None,
            direction: Direction::Newest,
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
            "status={};kinds={};scope={};repository={};sort={};show_pr_status={};saved={};reason={};direction={}",
            self.status.key(),
            self.kinds.key(),
            self.scope.key(),
            self.repository.as_deref().map(encode_meta_value).unwrap_or_default(),
            self.sort.key(),
            u8::from(self.show_pr_status),
            u8::from(self.saved_only),
            self.reason.map_or("any", AttentionReason::key),
            self.direction.key(),
        )
    }

    /// Missing keys keep their defaults. Legacy grouping settings are
    /// validated and ignored; Needs attention migrates to Notifications
    /// unless an explicit scope is present, regardless of key order.
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut out = Self::default();
        let mut explicit_scope = false;
        let mut legacy_attention = false;
        for pair in s.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let (key, value) =
                pair.split_once('=').ok_or_else(|| format!("expected key=value, got {pair:?}"))?;
            let (key, value) = (key.trim(), value.trim());
            let bad = || format!("bad value for {key}: {value:?}");
            match key {
                "status" => out.status = ItemStatus::from_key(value).ok_or_else(bad)?,
                "kinds" => out.kinds = ItemKinds::from_key(value).ok_or_else(bad)?,
                "scope" => {
                    out.scope = ItemScope::from_key(value).ok_or_else(bad)?;
                    explicit_scope = true;
                }
                "repository" => {
                    out.repository =
                        if value.is_empty() { None } else { Some(decode_meta_value(value).ok_or_else(bad)?) };
                }
                "group" if matches!(value, "repo" | "owner" | "date") => {}
                "group" => return Err(bad()),
                "show_empty" => {
                    parse_flag(value).ok_or_else(bad)?;
                }
                "sort" => out.sort = ItemSort::from_key(value).ok_or_else(bad)?,
                "show_pr_status" => out.show_pr_status = parse_flag(value).ok_or_else(bad)?,
                "saved" => out.saved_only = parse_flag(value).ok_or_else(bad)?,
                "attention" => legacy_attention = parse_flag(value).ok_or_else(bad)?,
                "reason" if value == "any" => out.reason = None,
                "reason" => out.reason = Some(AttentionReason::from_str(value).ok_or_else(bad)?),
                "direction" => out.direction = Direction::from_key(value).ok_or_else(bad)?,
                "query" => out.query = value.to_string(),
                _ => return Err(format!("unknown key {key:?}")),
            }
        }
        if legacy_attention && !explicit_scope {
            out.scope = ItemScope::Notifications;
        }
        Ok(out)
    }

    /// Whether the search matches the item. Case-insensitive substring over
    /// the title as shown, the title before any Rename, the number, the
    /// branch, and the repository's `owner/repo`, so a renamed pull request
    /// is found by either name and `#57` finds number 57.
    fn matches_query(&self, item: &Item) -> bool {
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
            item.repo_full_name.as_str(),
        ]
        .iter()
        .any(|hay| !hay.is_empty() && hay.to_lowercase().contains(&needle))
    }

    /// Scope, repository, saved state, and latest reason narrow pinned
    /// and unpinned items alike. The reason remains meaningful after Done.
    fn narrows_to(&self, item: &Item, viewer: Option<&str>) -> bool {
        self.scope.accepts(item, viewer)
            && self
                .repository
                .as_ref()
                .is_none_or(|repo| repo.to_lowercase() == item.repo_full_name.to_lowercase())
            && (!self.saved_only || item.saved)
            && self.reason.is_none_or(|wanted| {
                item.attention_reason.as_deref().and_then(AttentionReason::from_str) == Some(wanted)
            })
    }
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

/// Build one list, most recently pinned first and then globally sorted.
/// Pins retain their existing status/kind exceptions: only archived pins
/// require a matching status. All other filters apply to every row.
pub fn visible_items(items: &[Item], filters: &ItemFilters, viewer: Option<&str>) -> Vec<Item> {
    let searched: Vec<&Item> =
        items.iter().filter(|item| filters.matches_query(item) && filters.narrows_to(item, viewer)).collect();
    let mut pinned: Vec<&Item> = searched
        .iter()
        .copied()
        .filter(|item| item.is_pinned() && (!item.is_archived() || filters.status.shows_archived()))
        .collect();
    pinned.sort_by(|a, b| b.pinned_at.cmp(&a.pinned_at).then_with(|| a.id.cmp(&b.id)));
    let pinned_ids: HashSet<&str> = pinned.iter().map(|item| item.id.as_str()).collect();
    let mut rows: Vec<&Item> = searched
        .into_iter()
        .filter(|item| {
            !pinned_ids.contains(item.id.as_str())
                && filters.status.accepts(item)
                && filters.kinds.accepts(item)
        })
        .collect();
    sort_items(&mut rows, filters.sort, filters.direction);
    pinned.into_iter().chain(rows).cloned().collect()
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

/// The reason in words, from the string the attention was set with, as
/// `perch --items` prints it beside a row that needs attention. A reason
/// the menu does not know is still shown, with its underscores as spaces,
/// rather than dropped from the row.
pub fn attention_label(reason: &str) -> String {
    match AttentionReason::from_str(reason) {
        Some(known) => known.label().into(),
        None => reason.replace('_', " "),
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

    /// Three declared repositories. criccomini/perch has a pull request
    /// with attention, a done pull request, a merged one, an issue, an
    /// archived branch, and a plain branch. slatedb/slatedb has one open
    /// pull request. x/empty has nothing.
    fn fixture() -> (Vec<DeclaredRepo>, Vec<Item>) {
        let declared = declared(&["criccomini/perch", "slatedb/slatedb", "x/empty"]);

        let mut wanted = item("pr-wanted", "criccomini/perch", "2026-09-10T10:00:00Z");
        wanted.subject = Some(pr(57, "Fence the manifest", "fence", "open", "2026-09-17T10:00:00Z"));
        wanted.checkout = Some(checkout("fence"));
        wanted.attention_at = Some(at("2026-09-18T09:00:00Z"));
        wanted.attention_reason = Some("review_requested".into());

        let mut done = item("pr-done", "criccomini/perch", "2026-09-11T10:00:00Z");
        done.subject = Some(pr(58, "Tracing spans", "spans", "open", "2026-09-16T10:00:00Z"));
        done.attention_at = Some(at("2026-09-16T10:00:00Z"));
        done.done_at = Some(at("2026-09-16T10:00:00Z"));

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

        (declared, vec![wanted, done, merged, bug, archived, branch, other])
    }

    fn ids(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn rows_are_sorted_globally_across_repositories() {
        let (_, items) = fixture();
        let rows = visible_items(&items, &ItemFilters::default(), None);
        assert_eq!(ids(&rows), ["pr-wanted", "issue", "branch", "slatedb-pr"]);

        let filters = ItemFilters { sort: ItemSort::Created, ..Default::default() };
        assert_eq!(
            ids(&visible_items(&items, &filters, None)),
            ["branch", "slatedb-pr", "issue", "pr-wanted"]
        );
    }

    #[test]
    fn status_filters_exclude_done_closed_and_archived_from_active() {
        let (_, items) = fixture();
        let under = |status| visible_items(&items, &ItemFilters { status, ..Default::default() }, None);
        assert_eq!(ids(&under(ItemStatus::Closed)), ["pr-merged"]);
        assert_eq!(ids(&under(ItemStatus::Done)), ["pr-done"]);
        assert_eq!(ids(&under(ItemStatus::Archived)), ["branch-archived"]);
        assert_eq!(under(ItemStatus::All).len(), items.len());
    }

    #[test]
    fn done_and_closed_do_not_overlap_even_after_merge() {
        let (_, mut items) = fixture();
        if let Some(ItemSubject::PullRequest(pr)) = &mut items[1].subject {
            pr.state = "closed".into();
            pr.merged = true;
        }
        let mut archived_done = items[1].clone();
        archived_done.id = "pr-done-archived".into();
        archived_done.archived_at = Some(at("2026-09-17T10:00:00Z"));
        items.push(archived_done);
        let under = |status| visible_items(&items, &ItemFilters { status, ..Default::default() }, None);
        assert_eq!(ids(&under(ItemStatus::Done)), ["pr-done"]);
        assert_eq!(ids(&under(ItemStatus::Closed)), ["pr-merged"]);
        assert_eq!(ids(&under(ItemStatus::Archived)), ["pr-done-archived", "branch-archived"]);
    }

    #[test]
    fn kinds_come_from_the_shape_of_the_item() {
        let (_, items) = fixture();
        let filters = ItemFilters {
            status: ItemStatus::All,
            kinds: ItemKinds { pull_requests: false, issues: false, branches: true },
            ..Default::default()
        };
        assert_eq!(ids(&visible_items(&items, &filters, None)), ["branch", "branch-archived"]);
        let filters = ItemFilters {
            status: ItemStatus::All,
            kinds: ItemKinds { pull_requests: false, issues: true, branches: false },
            ..Default::default()
        };
        assert_eq!(ids(&visible_items(&items, &filters, None)), ["issue"]);
    }

    #[test]
    fn repository_filter_uses_the_item_identity_case_insensitively() {
        let (_, mut items) = fixture();
        items[0].repo_full_name = "chris/perch".into();
        let filters = ItemFilters { repository: Some("CHRIS/Perch".into()), ..Default::default() };
        assert_eq!(ids(&visible_items(&items, &filters, None)), ["pr-wanted"]);
        assert_eq!(full_reference(&items[0]), "chris/perch #57 · fence");
        let filters = ItemFilters { repository: Some("missing/repo".into()), ..Default::default() };
        assert!(visible_items(&items, &filters, None).is_empty());
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
    fn notifications_follow_attention_and_done_including_pinned_items() {
        let (_, mut items) = fixture();
        items[6].attention_at = Some(at("2026-09-17T12:00:00Z"));
        items[6].attention_reason = Some("comment".into());
        items[5].pinned_at = Some(at("2026-09-18T10:00:00Z"));
        let filters = ItemFilters { scope: ItemScope::Notifications, ..Default::default() };
        assert_eq!(ids(&visible_items(&items, &filters, None)), ["pr-wanted", "slatedb-pr"]);
        items[0].done_at = items[0].attention_at;
        assert_eq!(ids(&visible_items(&items, &filters, None)), ["slatedb-pr"]);
        items[0].attention_at = Some(at("2026-09-19T10:00:00Z"));
        assert_eq!(ids(&visible_items(&items, &filters, None)), ["pr-wanted", "slatedb-pr"]);
    }

    #[test]
    fn authored_uses_pr_and_issue_authors_independently_of_latest_attention_reason() {
        let (_, mut items) = fixture();
        if let Some(ItemSubject::Issue(issue)) = &mut items[3].subject {
            issue.author_login = "A".into();
        }
        // An authored notification is not proof of authorship, and a local
        // branch owned by the reader has no subject author to match.
        if let Some(ItemSubject::PullRequest(pr)) = &mut items[6].subject {
            pr.author_login = "someone-else".into();
        }
        items[6].attention_reason = Some("author".into());
        items[5].attention_reason = Some("author".into());
        items[6].pinned_at = Some(at("2026-09-18T10:00:00Z"));
        let filters = ItemFilters { scope: ItemScope::Authored, ..Default::default() };
        assert_eq!(ids(&visible_items(&items, &filters, Some("a"))), ["pr-wanted", "issue"]);
        assert_eq!(ids(&visible_items(&items, &filters, Some("A"))), ["pr-wanted", "issue"]);
        assert!(visible_items(&items, &filters, None).is_empty());
        assert!(visible_items(&items, &filters, Some("")).is_empty());
    }

    #[test]
    fn pins_lead_once_in_pin_order_and_ignore_status_and_kind_except_archived() {
        let (_, mut items) = fixture();
        items[1].pinned_at = Some(at("2026-09-17T10:00:00Z"));
        items[4].pinned_at = Some(at("2026-09-18T11:00:00Z"));
        items[5].pinned_at = Some(at("2026-09-18T10:00:00Z"));
        let filters = ItemFilters {
            kinds: ItemKinds { pull_requests: false, issues: true, branches: false },
            ..Default::default()
        };
        assert_eq!(ids(&visible_items(&items, &filters, None)), ["branch", "pr-done", "issue"]);
        let filters = ItemFilters { status: ItemStatus::Archived, ..filters };
        assert_eq!(ids(&visible_items(&items, &filters, None)), ["branch-archived", "branch", "pr-done"]);
    }

    #[test]
    fn pins_respect_repository_saved_reason_and_search_filters() {
        let (_, mut items) = fixture();
        for i in [0, 3, 5, 6] {
            items[i].pinned_at = Some(at("2026-09-18T10:00:00Z"));
        }
        items[0].saved = true;
        items[6].saved = true;
        let filters = ItemFilters { repository: Some("slatedb/slatedb".into()), ..Default::default() };
        assert_eq!(ids(&visible_items(&items, &filters, None)), ["slatedb-pr"]);
        let filters = ItemFilters { saved_only: true, ..Default::default() };
        assert_eq!(ids(&visible_items(&items, &filters, None)), ["pr-wanted", "slatedb-pr"]);
        let filters = ItemFilters { reason: Some(AttentionReason::ReviewRequested), ..Default::default() };
        assert_eq!(ids(&visible_items(&items, &filters, None)), ["pr-wanted"]);
        let filters = ItemFilters { query: "checkpoint".into(), ..Default::default() };
        assert_eq!(ids(&visible_items(&items, &filters, None)), ["slatedb-pr"]);
    }

    #[test]
    fn sort_and_direction_change_unpinned_order_but_leave_pin_order_alone() {
        let (_, mut items) = fixture();
        items[3].last_opened_at = at("2026-09-18T11:00:00Z");
        items[5].last_opened_at = at("2026-09-18T10:00:00Z");
        items[0].last_opened_at = at("2026-09-18T09:00:00Z");
        let filters = ItemFilters { sort: ItemSort::LastOpened, ..Default::default() };
        assert_eq!(
            ids(&visible_items(&items, &filters, None)),
            ["issue", "branch", "pr-wanted", "slatedb-pr"]
        );
        items[3].pinned_at = Some(at("2026-09-17T10:00:00Z"));
        items[5].pinned_at = Some(at("2026-09-18T10:00:00Z"));
        let newest = visible_items(&items, &ItemFilters::default(), None);
        let filters = ItemFilters { direction: Direction::Oldest, ..Default::default() };
        let oldest = visible_items(&items, &filters, None);
        assert_eq!(ids(&newest), ["branch", "issue", "pr-wanted", "slatedb-pr"]);
        assert_eq!(ids(&oldest), ["branch", "issue", "slatedb-pr", "pr-wanted"]);
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
    fn search_hits_the_number_titles_branch_and_repository() {
        let (_, mut items) = fixture();
        items[0].name = Some("the fence one".into());
        let hits = |query: &str| {
            visible_items(
                &items,
                &ItemFilters { status: ItemStatus::All, query: query.into(), ..Default::default() },
                None,
            )
        };
        assert_eq!(ids(&hits("#57")), ["pr-wanted"]);
        assert_eq!(ids(&hits("57")), ["pr-wanted"]);
        assert_eq!(ids(&hits("fence the manifest")), ["pr-wanted"]);
        assert_eq!(ids(&hits("fence one")), ["pr-wanted"]);
        assert_eq!(ids(&hits("ISSUE-42")), ["branch"]);
        assert_eq!(ids(&hits("slatedb")), ["slatedb-pr"]);
        assert_eq!(hits("criccomini/").len(), 6);
        assert!(hits("nothing like this").is_empty());
    }

    #[test]
    fn latest_reason_matches_even_when_done_and_team_mention_is_a_mention() {
        let (_, mut items) = fixture();
        items[3].attention_reason = Some("team_mention".into());
        items[1].attention_reason = Some("review_requested".into());
        items[6].attention_reason = Some("security_alert".into());
        let filters = ItemFilters { reason: Some(AttentionReason::Mention), ..Default::default() };
        assert_eq!(ids(&visible_items(&items, &filters, None)), ["issue"]);
        let filters = ItemFilters {
            reason: Some(AttentionReason::ReviewRequested),
            status: ItemStatus::Done,
            ..Default::default()
        };
        assert_eq!(ids(&visible_items(&items, &filters, None)), ["pr-done"]);
        for reason in AttentionReason::ALL {
            let filters = ItemFilters { reason: Some(reason), status: ItemStatus::All, ..Default::default() };
            assert!(!ids(&visible_items(&items, &filters, None)).contains(&"slatedb-pr"));
        }
    }

    #[test]
    fn filters_round_trip_with_all_preferences_and_safe_repository_encoding() {
        let filters = ItemFilters {
            status: ItemStatus::Closed,
            kinds: ItemKinds { pull_requests: true, issues: false, branches: true },
            scope: ItemScope::Authored,
            repository: Some("local/notes;scope=all% ☃ ".into()),
            sort: ItemSort::LastOpened,
            show_pr_status: false,
            saved_only: true,
            reason: Some(AttentionReason::Mention),
            direction: Direction::Oldest,
            query: String::new(),
        };
        let meta = filters.to_meta();
        assert!(meta.contains("repository=local/notes%3Bscope%3Dall%25%20%E2%98%83%20;"));
        assert_eq!(ItemFilters::parse(&meta).unwrap(), filters);
        assert_eq!(ItemFilters::parse("").unwrap(), ItemFilters::default());
        assert_eq!(ItemFilters::parse(&ItemFilters::default().to_meta()).unwrap(), ItemFilters::default());
        assert_eq!(
            ItemFilters::parse("repository=owner/repo").unwrap().repository.as_deref(),
            Some("owner/repo")
        );
        assert_eq!(ItemFilters::parse("repository=").unwrap().repository, None);
        assert!(!meta.contains(";group="));
        assert!(!meta.contains(";show_empty="));
        assert!(!meta.contains(";attention="));
    }

    #[test]
    fn legacy_grouping_and_attention_preferences_migrate_without_losing_other_filters() {
        let old = "status=done;kinds=pr;group=owner;sort=created;show_empty=1;show_pr_status=0;saved=1;attention=1;reason=mention;direction=oldest";
        let filters = ItemFilters::parse(old).unwrap();
        assert_eq!(filters.status, ItemStatus::Done);
        assert_eq!(filters.kinds, ItemKinds { pull_requests: true, issues: false, branches: false });
        assert_eq!(filters.scope, ItemScope::Notifications);
        assert_eq!(filters.repository, None);
        assert_eq!(filters.sort, ItemSort::Created);
        assert!(!filters.show_pr_status);
        assert!(filters.saved_only);
        assert_eq!(filters.reason, Some(AttentionReason::Mention));
        assert_eq!(filters.direction, Direction::Oldest);
        for group in ["repo", "owner", "date"] {
            assert_eq!(
                ItemFilters::parse(&format!("group={group};show_empty=0;attention=0")).unwrap(),
                ItemFilters::default()
            );
        }
        assert_eq!(ItemFilters::parse("attention=on").unwrap().scope, ItemScope::Notifications);
    }

    #[test]
    fn explicit_scope_overrides_legacy_attention_regardless_of_key_order() {
        for value in ["scope=all;attention=1", "attention=1;scope=all"] {
            assert_eq!(ItemFilters::parse(value).unwrap().scope, ItemScope::All);
        }
        for value in ["scope=authored;attention=1", "attention=1;scope=authored"] {
            assert_eq!(ItemFilters::parse(value).unwrap().scope, ItemScope::Authored);
        }
        for value in ["scope=notifications;attention=0", "attention=0;scope=notifications"] {
            assert_eq!(ItemFilters::parse(value).unwrap().scope, ItemScope::Notifications);
        }
    }

    #[test]
    fn search_is_transient_and_empty_kinds_means_none() {
        let filters = ItemFilters { query: "fence".into(), ..Default::default() };
        assert!(!filters.to_meta().contains("fence"));
        assert_eq!(ItemFilters::parse("query=fence").unwrap().query, "fence");
        assert_eq!(
            ItemFilters::parse("kinds=").unwrap().kinds,
            ItemKinds { pull_requests: false, issues: false, branches: false }
        );
        assert_eq!(ItemFilters::parse("reason=team_mention").unwrap().reason, Some(AttentionReason::Mention));
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
            ("attention=maybe", "attention"),
            ("reason=bogus", "reason"),
            ("direction=sideways", "direction"),
            ("scope=reviewing", "scope"),
            ("repository=local/%", "repository"),
            ("repository=%ZZ", "repository"),
            ("repository=%FF", "repository"),
            ("nonsense", "key=value"),
        ] {
            assert!(ItemFilters::parse(setting).unwrap_err().contains(key), "{setting}");
        }
    }

    #[test]
    fn every_option_has_a_label_and_a_key_that_reads_back() {
        for value in ItemStatus::ALL {
            assert_eq!(ItemStatus::from_key(value.key()), Some(value));
            assert_eq!(value.to_string(), value.label());
        }
        for value in ItemScope::ALL {
            assert_eq!(ItemScope::from_key(value.key()), Some(value));
            assert_eq!(value.to_string(), value.label());
        }
        for value in ItemSort::ALL {
            assert_eq!(ItemSort::from_key(value.key()), Some(value));
            assert_eq!(value.to_string(), value.label());
        }
        for value in AttentionReason::ALL {
            assert_eq!(AttentionReason::from_str(value.key()), Some(value));
            assert_eq!(value.to_string(), value.label());
            assert_eq!(value.title().to_lowercase(), value.label().to_lowercase());
        }
        for value in Direction::ALL {
            assert_eq!(Direction::from_key(value.key()), Some(value));
            assert_eq!(value.to_string(), value.label());
        }
    }

    #[test]
    fn attention_labels_and_row_kinds_remain_readable() {
        assert_eq!(attention_label("review_requested"), "review requested");
        assert_eq!(attention_label("team_mention"), "mentioned");
        assert_eq!(attention_label("ci_failed"), "CI failed");
        assert_eq!(attention_label("security_alert"), "security alert");
        let (_, items) = fixture();
        assert_eq!(RowKind::of(&items[0]), RowKind::PullRequest);
        assert_eq!(RowKind::of(&items[3]), RowKind::Issue);
        assert_eq!(RowKind::of(&items[5]), RowKind::Branch);
        assert_eq!(RowKind::PullRequest.letter(), 'P');
        assert_eq!(RowKind::Issue.letter(), 'I');
        assert_eq!(RowKind::Branch.letter(), 'B');
    }
}
