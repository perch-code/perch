//! The sidebar's list: filtering, grouping and sorting items. Pure functions
//! over the declared repositories and the items the cache holds.
//!
//! Everything here runs in memory. Changing a setting never refetches, and
//! the sidebar is paintable from the cache alone.

use std::collections::{HashMap, HashSet};

use jiff::Timestamp;
use perch_github::{Item, ItemSubject};

use crate::timefmt::{bucket, DateBucket};

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

    /// Whether the status shows archived items. Priority and Pinned use
    /// this rather than `accepts`: those sections ignore the status filter
    /// otherwise, but an archived item was put away on purpose and should
    /// not keep nagging from the top of the list.
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

/// What the sidebar's sections are: one per repository, one per owner, or
/// one per date bucket. Owner is how two repositories share a heading;
/// there is no named group of the reader's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemGroup {
    #[default]
    Repository,
    Owner,
    Date,
}

impl ItemGroup {
    pub const ALL: [ItemGroup; 3] = [ItemGroup::Repository, ItemGroup::Owner, ItemGroup::Date];

    pub fn label(self) -> &'static str {
        match self {
            ItemGroup::Repository => "Repository",
            ItemGroup::Owner => "Owner",
            ItemGroup::Date => "Date",
        }
    }

    /// The persisted form, and the prefix of every section key this
    /// grouping makes, so a section can say which grouping made it.
    fn key(self) -> &'static str {
        match self {
            ItemGroup::Repository => "repo",
            ItemGroup::Owner => "owner",
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
}

impl AttentionReason {
    pub const ALL: [AttentionReason; 7] = [
        AttentionReason::ReviewRequested,
        AttentionReason::Mention,
        AttentionReason::Assign,
        AttentionReason::Comment,
        AttentionReason::CiFailed,
        AttentionReason::Manual,
        AttentionReason::Author,
    ];

    /// The words a Priority row says next to its dot.
    pub fn label(self) -> &'static str {
        match self {
            AttentionReason::ReviewRequested => "review requested",
            AttentionReason::Mention => "mentioned",
            AttentionReason::Assign => "assigned",
            AttentionReason::Comment => "commented",
            AttentionReason::CiFailed => "CI failed",
            AttentionReason::Manual => "manual",
            AttentionReason::Author => "authored",
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
    pub group: ItemGroup,
    pub sort: ItemSort,
    pub show_empty: bool,
    pub show_pr_status: bool,
    /// Only saved items, whatever the status.
    pub saved_only: bool,
    /// Only items with attention newer than their Done: the sections become
    /// Priority grouped.
    pub attention_only: bool,
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
            group: ItemGroup::Repository,
            sort: ItemSort::LastActivity,
            show_empty: false,
            show_pr_status: true,
            saved_only: false,
            attention_only: false,
            reason: None,
            direction: Direction::Newest,
            query: String::new(),
        }
    }
}

impl ItemFilters {
    /// The form kept in the `meta` table: `key=value;` pairs. The search is
    /// left out on purpose; a search box that comes back full at launch is
    /// a surprise. The six original keys keep their names and order so a
    /// stored string from before the last four still reads.
    pub fn to_meta(&self) -> String {
        format!(
            "status={};kinds={};group={};sort={};show_empty={};show_pr_status={};saved={};attention={};reason={};direction={}",
            self.status.key(),
            self.kinds.key(),
            self.group.key(),
            self.sort.key(),
            u8::from(self.show_empty),
            u8::from(self.show_pr_status),
            u8::from(self.saved_only),
            u8::from(self.attention_only),
            self.reason.map_or("any", AttentionReason::key),
            self.direction.key(),
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
                "saved" => out.saved_only = parse_flag(value).ok_or_else(bad)?,
                "attention" => out.attention_only = parse_flag(value).ok_or_else(bad)?,
                "reason" if value == "any" => out.reason = None,
                "reason" => out.reason = Some(AttentionReason::from_str(value).ok_or_else(bad)?),
                "direction" => out.direction = Direction::from_key(value).ok_or_else(bad)?,
                "query" => out.query = value.to_string(),
                _ => return Err(format!("unknown key {key:?}")),
            }
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

    /// The narrowings that apply to every section alike: saved, needing
    /// attention, and the reason. An item matches the reason on the latest
    /// one it was given, done or not, so a done review request is still a
    /// review request under Done.
    fn narrows_to(&self, item: &Item) -> bool {
        (!self.saved_only || item.saved)
            && (!self.attention_only || item.needs_attention())
            && self.reason.is_none_or(|wanted| {
                item.attention_reason.as_deref().and_then(AttentionReason::from_str) == Some(wanted)
            })
    }
}

fn parse_flag(s: &str) -> Option<bool> {
    match s {
        "1" | "true" | "on" => Some(true),
        "0" | "false" | "off" => Some(false),
        _ => None,
    }
}

/// One repository, one owner, or one date bucket, and its rows.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemSection {
    /// Stable identity for the view diffing key: the grouping's key, a
    /// colon, and the lower-cased repository, owner, or the date label.
    pub key: String,
    pub title: String,
    /// For a repository, its active items before any filter, and for an
    /// owner the sum over its repositories. The badge says how much is
    /// going on there, whatever the list is narrowed to. For a date
    /// bucket, the rows it holds.
    pub count: usize,
    pub items: Vec<Item>,
}

impl ItemSection {
    /// Which grouping made the section, read back from its key, so a view
    /// that is handed a section alone knows how to draw its rows.
    pub fn group(&self) -> ItemGroup {
        ItemGroup::ALL
            .into_iter()
            .find(|g| self.key.strip_prefix(g.key()).is_some_and(|rest| rest.starts_with(':')))
            .unwrap_or_default()
    }
}

/// What the sidebar draws, top to bottom.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sections {
    /// Items with attention newer than their Done mark, newest attention
    /// first. These still appear under their repository.
    pub priority: Vec<Item>,
    /// Pinned items, most recently pinned first. These do not appear under
    /// their repository.
    pub pinned: Vec<Item>,
    pub groups: Vec<ItemSection>,
}

/// Build the sidebar from the declared repositories, `owner/repo` in config
/// order, and the cache's items.
///
/// Priority and Pinned ignore the group, sort and kind settings and respect
/// the search. Saved only, Needs attention and the reason narrow the
/// searched set before Priority and Pinned are cut from it, so the three
/// lists agree on what is in play. The status filter touches Priority and
/// Pinned in one way only: archived items stay out of both unless the
/// status shows archived items. Group sections take every filter. The
/// direction reverses the rows and the date buckets and nothing else;
/// attention order and pin order are not the sort.
pub fn sections(declared: &[String], items: &[Item], filters: &ItemFilters, now: Timestamp) -> Sections {
    let searched: Vec<&Item> =
        items.iter().filter(|i| filters.matches_query(i) && filters.narrows_to(i)).collect();
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
    sort_items(&mut rows, filters.sort, filters.direction);

    let groups = match filters.group {
        ItemGroup::Repository => by_repository(by_repository_order(declared, items, &rows), filters.show_empty),
        ItemGroup::Owner => by_owner(by_repository_order(declared, items, &rows), filters.show_empty),
        ItemGroup::Date => by_date(&rows, filters.sort, filters.direction, now),
    };

    Sections { priority, pinned, groups }
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

/// One repository's rows, on the way to becoming a section of its own or
/// part of its owner's.
struct RepoBucket {
    /// `owner/repo`, lower-cased: the identity, since a checkout's origin
    /// and the API can spell one repository differently.
    key: String,
    /// The declared spelling when declared, else the first row's.
    name: String,
    declared: bool,
    items: Vec<Item>,
    /// Active items in the repository, before any filter.
    active: usize,
}

/// The rows bucketed by repository, declared repositories first in config
/// order, then the rest by name. Grouping is by the item's own repository,
/// so a checkout of a fork sits under the fork whatever its pull request
/// is against, the same repository the row's second line names. A declared
/// repository with no rows is still here, empty, for Show Empty.
///
/// Every repository with an item gets a bucket, whether or not any of its
/// items survive the filters, so its active count reaches its owner's
/// badge the same as a declared repository's does. The callers drop an
/// undeclared bucket with no rows.
fn by_repository_order(declared: &[String], all: &[Item], rows: &[&Item]) -> Vec<RepoBucket> {
    let mut buckets: Vec<RepoBucket> = declared
        .iter()
        .map(|name| RepoBucket { key: name.to_lowercase(), name: name.clone(), declared: true, items: Vec::new(), active: 0 })
        .collect();
    let mut extra: Vec<RepoBucket> = Vec::new();
    for item in all {
        let key = item.repo_full_name.to_lowercase();
        if !buckets.iter().chain(extra.iter()).any(|b| b.key == key) {
            extra.push(RepoBucket {
                key,
                name: item.repo_full_name.clone(),
                declared: false,
                items: Vec::new(),
                active: 0,
            });
        }
    }
    extra.sort_by(|a, b| a.key.cmp(&b.key));
    buckets.extend(extra);

    for item in rows {
        let key = item.repo_full_name.to_lowercase();
        if let Some(bucket) = buckets.iter_mut().find(|b| b.key == key) {
            bucket.items.push((*item).clone());
        }
    }

    let mut active: HashMap<String, usize> = HashMap::new();
    for item in all.iter().filter(|i| i.is_active()) {
        *active.entry(item.repo_full_name.to_lowercase()).or_default() += 1;
    }
    for bucket in &mut buckets {
        bucket.active = active.get(&bucket.key).copied().unwrap_or(0);
    }
    buckets
}

/// One section per repository with rows, in the order the buckets came,
/// plus the declared ones with nothing when asked to show them.
fn by_repository(buckets: Vec<RepoBucket>, show_empty: bool) -> Vec<ItemSection> {
    buckets
        .into_iter()
        .filter(|b| !b.items.is_empty() || (show_empty && b.declared))
        .map(|b| ItemSection {
            key: format!("{}:{}", ItemGroup::Repository.key(), b.key),
            title: b.name,
            count: b.active,
            items: b.items,
        })
        .collect()
}

/// One flat section per owner, the part before the slash, with the rows
/// of its repositories one after another in the repository order and no
/// heading between them. The count is the sum over the repositories. With
/// Show Empty, an owner all of whose declared repositories have nothing is
/// listed too.
fn by_owner(buckets: Vec<RepoBucket>, show_empty: bool) -> Vec<ItemSection> {
    struct OwnerBucket {
        key: String,
        name: String,
        declared: bool,
        items: Vec<Item>,
        active: usize,
    }
    let mut owners: Vec<OwnerBucket> = Vec::new();
    for bucket in buckets {
        let key = bucket.key.split('/').next().unwrap_or_default().to_string();
        let owner = match owners.iter().position(|o| o.key == key) {
            Some(i) => &mut owners[i],
            None => {
                let name = bucket.name.split('/').next().unwrap_or_default().to_string();
                owners.push(OwnerBucket { key, name, declared: false, items: Vec::new(), active: 0 });
                let last = owners.len() - 1;
                &mut owners[last]
            }
        };
        owner.declared |= bucket.declared;
        owner.items.extend(bucket.items);
        owner.active += bucket.active;
    }
    owners
        .into_iter()
        .filter(|o| !o.items.is_empty() || (show_empty && o.declared))
        .map(|o| ItemSection {
            key: format!("{}:{}", ItemGroup::Owner.key(), o.key),
            title: o.name,
            count: o.active,
            items: o.items,
        })
        .collect()
}

/// Today, Yesterday, This week and Earlier, in the sort's direction. Rows
/// are bucketed on the time the sort orders by, so a section and its rows
/// tell the same story, and the rows already carry the direction from
/// `sort_items`.
fn by_date(rows: &[&Item], sort: ItemSort, direction: Direction, now: Timestamp) -> Vec<ItemSection> {
    let mut by_bucket: HashMap<DateBucket, Vec<Item>> = HashMap::new();
    for item in rows {
        by_bucket.entry(bucket(sort.time(item), now)).or_default().push((*item).clone());
    }
    let mut buckets: Vec<DateBucket> = by_bucket.keys().copied().collect();
    buckets.sort();
    if direction == Direction::Oldest {
        buckets.reverse();
    }
    buckets
        .into_iter()
        .map(|b| {
            let items = by_bucket.remove(&b).unwrap_or_default();
            ItemSection {
                key: format!("{}:{}", ItemGroup::Date.key(), b.label()),
                title: b.label().to_string(),
                count: items.len(),
                items,
            }
        })
        .collect()
}

/// The row's second line: `owner/repo #57 · branch` for a pull request,
/// `owner/repo #42` for an issue, `owner/repo · branch` for a checkout with
/// no subject. Under Repository the heading already names the repository,
/// so the line is `#57 · branch`, `#42`, or the branch alone. Under Owner
/// and Date nothing above the row says which repository it is in, so the
/// line does. The repository is the item's own: for a checkout of a fork,
/// the fork.
pub fn reference(item: &Item, group: ItemGroup) -> String {
    let place = match group {
        ItemGroup::Repository => "",
        ItemGroup::Owner | ItemGroup::Date => item.repo_full_name.as_str(),
    };
    let with_place = |rest: String| if place.is_empty() { rest } else { format!("{place} {rest}") };
    match &item.subject {
        Some(ItemSubject::PullRequest(pr)) => {
            let branch = item.branch().unwrap_or(pr.head_ref.as_str());
            if branch.is_empty() {
                with_place(format!("#{}", pr.number))
            } else {
                with_place(format!("#{} · {branch}", pr.number))
            }
        }
        Some(ItemSubject::Issue(issue)) => with_place(format!("#{}", issue.number)),
        None => match item.branch() {
            Some(branch) if place.is_empty() => branch.to_string(),
            Some(branch) => format!("{place} · {branch}"),
            None => place.to_string(),
        },
    }
}

/// The second line with the repository in it, whatever the grouping. For
/// Priority and Pinned rows, which sit under no heading that names one.
pub fn full_reference(item: &Item) -> String {
    reference(item, ItemGroup::Date)
}

/// The words a Priority row says next to its dot, from the reason the
/// attention was set with. A reason the menu does not know is still shown,
/// with its underscores as spaces, rather than dropped from the row.
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

    fn now() -> Timestamp {
        at("2026-09-18T12:00:00Z")
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

    fn declared(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn titles(s: &Sections) -> Vec<&str> {
        s.groups.iter().map(|g| g.title.as_str()).collect()
    }

    /// Three declared repositories. criccomini/perch has a pull request
    /// with attention, a done pull request, a merged one, an issue, an
    /// archived branch, and a plain branch. slatedb/slatedb has one open
    /// pull request. x/empty has nothing.
    fn fixture() -> (Vec<String>, Vec<Item>) {
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

    fn section_ids(s: &ItemSection) -> Vec<&str> {
        ids(&s.items)
    }

    #[test]
    fn priority_holds_only_items_with_attention_past_done() {
        let (declared, mut items) = fixture();
        // A second wanted item, older attention, so the order is checked too.
        let mut older = item("issue-mentioned", "criccomini/perch", "2026-09-12T10:00:00Z");
        older.subject = Some(issue(43, "Mentioned issue", "open", "2026-09-15T10:00:00Z"));
        older.attention_at = Some(at("2026-09-17T09:00:00Z"));
        older.attention_reason = Some("mention".into());
        items.push(older);

        let s = sections(&declared, &items, &ItemFilters::default(), now());
        assert_eq!(ids(&s.priority), ["pr-wanted", "issue-mentioned"]);
        // Done is not in there, and neither is anything without attention.
        assert!(s.priority.iter().all(Item::needs_attention));
    }

    #[test]
    fn priority_keeps_the_item_under_its_repository_too() {
        let (declared, items) = fixture();
        let s = sections(&declared, &items, &ItemFilters::default(), now());
        assert!(section_ids(&s.groups[0]).contains(&"pr-wanted"));
    }

    #[test]
    fn priority_leaves_out_an_archived_item_unless_the_status_shows_archived() {
        let (declared, mut items) = fixture();
        items[4].attention_at = Some(at("2026-09-18T11:00:00Z"));
        items[4].attention_reason = Some("comment".into());

        let s = sections(&declared, &items, &ItemFilters::default(), now());
        assert_eq!(ids(&s.priority), ["pr-wanted"]);

        let f = ItemFilters { status: ItemStatus::Archived, ..Default::default() };
        let s = sections(&declared, &items, &f, now());
        assert_eq!(ids(&s.priority), ["branch-archived", "pr-wanted"]);
    }

    #[test]
    fn priority_ignores_the_kind_filter() {
        let (declared, items) = fixture();
        let f = ItemFilters {
            kinds: ItemKinds { pull_requests: false, issues: true, branches: true },
            ..Default::default()
        };
        let s = sections(&declared, &items, &f, now());
        assert_eq!(ids(&s.priority), ["pr-wanted"]);
        assert!(!section_ids(&s.groups[0]).contains(&"pr-wanted"));
    }

    #[test]
    fn active_excludes_closed_done_and_archived() {
        let (declared, items) = fixture();
        let s = sections(&declared, &items, &ItemFilters::default(), now());
        let perch = &s.groups[0];
        assert_eq!(perch.title, "criccomini/perch");
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
    fn closed_holds_merged_but_not_done_or_archived() {
        let (declared, items) = fixture();
        let f = ItemFilters { status: ItemStatus::Closed, ..Default::default() };
        let s = sections(&declared, &items, &f, now());
        assert_eq!(s.groups.len(), 1);
        assert_eq!(section_ids(&s.groups[0]), ["pr-merged"]);
        // The badge still counts what is active there.
        assert_eq!(s.groups[0].count, 3);
    }

    #[test]
    fn done_and_closed_do_not_overlap() {
        let (declared, mut items) = fixture();
        // A done item whose subject has since been merged is done, not
        // closed: Done is the reader's mark and wins over the subject.
        let mut merged_done = item("pr-merged-done", "criccomini/perch", "2026-09-03T10:00:00Z");
        merged_done.subject = Some(pr(51, "Merged and done", "md", "merged", "2026-09-04T10:00:00Z"));
        merged_done.attention_at = Some(at("2026-09-04T10:00:00Z"));
        merged_done.done_at = Some(at("2026-09-04T10:00:00Z"));
        items.push(merged_done);
        // A done item that was then archived is under Archived alone.
        let mut archived_done = items[1].clone();
        archived_done.id = "pr-done-archived".into();
        archived_done.archived_at = Some(at("2026-09-17T10:00:00Z"));
        items.push(archived_done);

        let under = |status: ItemStatus| {
            let f = ItemFilters { status, ..Default::default() };
            let s = sections(&declared, &items, &f, now());
            s.groups.iter().flat_map(|g| g.items.iter().map(|i| i.id.clone())).collect::<Vec<_>>()
        };
        let done = under(ItemStatus::Done);
        let closed = under(ItemStatus::Closed);
        assert_eq!(done, ["pr-done", "pr-merged-done"]);
        assert_eq!(closed, ["pr-merged"]);
        assert!(done.iter().all(|id| !closed.contains(id)));
        assert_eq!(under(ItemStatus::Archived), ["pr-done-archived", "branch-archived"]);
        // Active is what it was: done is still not active.
        assert!(!under(ItemStatus::Active).contains(&"pr-done".to_string()));
    }

    #[test]
    fn archived_and_all_show_what_they_say() {
        let (declared, items) = fixture();
        let f = ItemFilters { status: ItemStatus::Archived, ..Default::default() };
        let s = sections(&declared, &items, &f, now());
        assert_eq!(section_ids(&s.groups[0]), ["branch-archived"]);

        let f = ItemFilters { status: ItemStatus::All, ..Default::default() };
        let s = sections(&declared, &items, &f, now());
        let total: usize = s.groups.iter().map(|g| g.items.len()).sum();
        assert_eq!(total, items.len());
    }

    #[test]
    fn kinds_come_from_the_shape_of_the_item() {
        let (declared, items) = fixture();
        let f = ItemFilters {
            status: ItemStatus::All,
            kinds: ItemKinds { pull_requests: false, issues: false, branches: true },
            ..Default::default()
        };
        let s = sections(&declared, &items, &f, now());
        assert_eq!(section_ids(&s.groups[0]), ["branch", "branch-archived"]);

        let f = ItemFilters {
            status: ItemStatus::All,
            kinds: ItemKinds { pull_requests: false, issues: true, branches: false },
            ..Default::default()
        };
        let s = sections(&declared, &items, &f, now());
        assert_eq!(section_ids(&s.groups[0]), ["issue"]);
    }

    #[test]
    fn declared_repositories_lead_in_config_order_and_hide_when_empty() {
        let (_, items) = fixture();
        // Config order, not name order: slatedb is declared first.
        let declared = declared(&["slatedb/slatedb", "criccomini/perch", "x/empty"]);
        let s = sections(&declared, &items, &ItemFilters::default(), now());
        assert_eq!(titles(&s), ["slatedb/slatedb", "criccomini/perch"]);
        assert_eq!(s.groups[0].key, "repo:slatedb/slatedb");
        assert_eq!(s.groups[0].group(), ItemGroup::Repository);

        let f = ItemFilters { show_empty: true, ..Default::default() };
        let s = sections(&declared, &items, &f, now());
        assert_eq!(titles(&s), ["slatedb/slatedb", "criccomini/perch", "x/empty"]);
        assert_eq!(s.groups[2].count, 0);
        assert!(s.groups[2].items.is_empty());
    }

    #[test]
    fn an_undeclared_repository_follows_the_declared_ones_by_name() {
        let (declared, mut items) = fixture();
        items[6].repo_full_name = "zed/later".into();
        let mut first = item("stray", "Aardvark/first", "2026-09-13T10:00:00Z");
        first.checkout = Some(checkout("stray"));
        items.push(first);

        let s = sections(&declared, &items, &ItemFilters::default(), now());
        assert_eq!(titles(&s), ["criccomini/perch", "Aardvark/first", "zed/later"], "by name, case aside");
        assert_eq!(s.groups[1].key, "repo:aardvark/first");
        assert_eq!(s.groups[2].count, 1, "an undeclared repository counts its active items too");
    }

    #[test]
    fn a_repository_spelled_two_ways_is_one_section_with_the_declared_spelling() {
        let (declared, mut items) = fixture();
        items[6].repo_full_name = "Criccomini/Perch".into();
        let s = sections(&declared, &items, &ItemFilters::default(), now());
        assert_eq!(titles(&s), ["criccomini/perch"]);
        assert!(section_ids(&s.groups[0]).contains(&"slatedb-pr"));
        assert_eq!(s.groups[0].count, 4);
    }

    #[test]
    fn a_fork_checkout_groups_under_the_fork() {
        let (declared, mut items) = fixture();
        // The pull request is criccomini/perch's; the checkout is of a fork.
        items[0].repo_full_name = "chris/perch".into();
        let s = sections(&declared, &items, &ItemFilters::default(), now());
        assert_eq!(titles(&s), ["criccomini/perch", "slatedb/slatedb", "chris/perch"]);
        assert_eq!(section_ids(&s.groups[2]), ["pr-wanted"]);
        assert_eq!(reference(&items[0], ItemGroup::Owner), "chris/perch #57 · fence");
    }

    #[test]
    fn group_by_owner_is_one_flat_section_per_owner_with_rows_by_repository() {
        let (_, mut items) = fixture();
        let declared = declared(&["slatedb/slatedb-go", "slatedb/slatedb", "criccomini/perch", "x/empty"]);
        // Newer than the slatedb/slatedb row, and in a repository declared
        // after it, so the repository order beats the sort.
        let mut go = item("go-pr", "slatedb/slatedb-go", "2026-09-13T10:00:00Z");
        go.subject = Some(pr(12, "Go bindings", "bindings", "open", "2026-09-17T11:00:00Z"));
        let mut go_old = item("go-old", "slatedb/slatedb-go", "2026-09-13T10:00:00Z");
        go_old.subject = Some(pr(11, "Older bindings", "older", "open", "2026-09-10T11:00:00Z"));
        items.push(go_old);
        items.push(go);

        let f = ItemFilters { group: ItemGroup::Owner, ..Default::default() };
        let s = sections(&declared, &items, &f, now());
        assert_eq!(titles(&s), ["slatedb", "criccomini"]);
        assert_eq!(s.groups[0].key, "owner:slatedb");
        assert_eq!(s.groups[0].group(), ItemGroup::Owner);
        // slatedb-go first, as declared, then slatedb; the sort inside each.
        assert_eq!(section_ids(&s.groups[0]), ["go-pr", "go-old", "slatedb-pr"]);
        assert_eq!(s.groups[0].count, 3, "the sum over the owner's repositories");
        assert_eq!(s.groups[1].count, 3);

        // An owner with nothing shows only with Show Empty.
        let f = ItemFilters { group: ItemGroup::Owner, show_empty: true, ..Default::default() };
        let s = sections(&declared, &items, &f, now());
        assert_eq!(titles(&s), ["slatedb", "criccomini", "x"]);
        assert!(s.groups[2].items.is_empty());
    }

    #[test]
    fn the_owner_badge_counts_an_undeclared_repository_whose_rows_are_filtered_out() {
        // slatedb/slatedb is declared with one open issue and one open pull
        // request; slatedb/slatedb-go is not declared and has two open pull
        // requests. Under Issues only, the owner's badge still says four,
        // the same as it would with slatedb-go declared.
        let (_, mut items) = fixture();
        let two = declared(&["slatedb/slatedb", "criccomini/perch"]);
        let mut bug = item("slatedb-issue", "slatedb/slatedb", "2026-09-12T10:00:00Z");
        bug.subject = Some(issue(7, "Flaky test", "open", "2026-09-15T10:00:00Z"));
        items.push(bug);
        for (id, n) in [("go-pr", 12), ("go-old", 11)] {
            let mut go = item(id, "slatedb/slatedb-go", "2026-09-13T10:00:00Z");
            go.subject = Some(pr(n, "Go bindings", "bindings", "open", "2026-09-17T11:00:00Z"));
            items.push(go);
        }

        let kinds = ItemKinds { pull_requests: false, issues: true, branches: false };
        let f = ItemFilters { group: ItemGroup::Owner, kinds, ..Default::default() };
        let s = sections(&two, &items, &f, now());
        assert_eq!(titles(&s), ["slatedb", "criccomini"]);
        assert_eq!(section_ids(&s.groups[0]), ["slatedb-issue"]);
        assert_eq!(s.groups[0].count, 4, "the badge is not narrowed by the kind filter");

        // Declaring it changes nothing about the count.
        let three = declared(&["slatedb/slatedb", "slatedb/slatedb-go", "criccomini/perch"]);
        let s = sections(&three, &items, &f, now());
        assert_eq!(s.groups[0].count, 4);

        // Under Repository, an undeclared repository with no rows is not a
        // section, with or without Show Empty.
        let f = ItemFilters { kinds, show_empty: true, ..Default::default() };
        let s = sections(&two, &items, &f, now());
        assert_eq!(titles(&s), ["slatedb/slatedb", "criccomini/perch"]);
        assert_eq!(s.groups[0].count, 2);
    }

    #[test]
    fn sorts_order_by_the_time_they_name() {
        let (declared, mut items) = fixture();
        items[3].last_opened_at = at("2026-09-18T11:00:00Z");
        items[5].last_opened_at = at("2026-09-18T10:00:00Z");
        items[0].last_opened_at = at("2026-09-18T09:00:00Z");

        let by = |sort: ItemSort| {
            let f = ItemFilters { sort, ..Default::default() };
            let s = sections(&declared, &items, &f, now());
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
        let now = crate::timefmt::tests::noon_today();
        let ago = |h: i64| now - jiff::SignedDuration::from_hours(h);
        let (declared, _) = fixture();
        let mut a = item("a", "criccomini/perch", "2026-09-01T10:00:00Z");
        a.checkout = Some(checkout("a"));
        a.attention_at = Some(ago(2));
        let mut b = item("b", "criccomini/perch", "2026-09-01T10:00:00Z");
        b.checkout = Some(checkout("b"));
        b.attention_at = Some(ago(26));
        let mut c = item("c", "slatedb/slatedb", "2026-09-01T10:00:00Z");
        c.checkout = Some(checkout("c"));
        c.attention_at = Some(ago(24 * 40));
        let items = vec![c.clone(), b.clone(), a.clone()];

        let f = ItemFilters { group: ItemGroup::Date, ..Default::default() };
        let s = sections(&declared, &items, &f, now);
        assert_eq!(titles(&s), ["Today", "Yesterday", "Earlier"]);
        assert_eq!(s.groups[0].key, "date:Today");
        assert_eq!(s.groups[0].group(), ItemGroup::Date);
        assert_eq!(section_ids(&s.groups[0]), ["a"]);
        assert_eq!(s.groups[0].count, 1);

        // Created puts all three on the same day.
        let f = ItemFilters { group: ItemGroup::Date, sort: ItemSort::Created, ..Default::default() };
        let s = sections(&declared, &items, &f, now);
        assert_eq!(s.groups.len(), 1);
        assert_eq!(section_ids(&s.groups[0]), ["a", "b", "c"]);
    }

    #[test]
    fn the_second_line_drops_the_repository_under_repository_only() {
        let (_, items) = fixture();
        assert_eq!(reference(&items[0], ItemGroup::Repository), "#57 · fence");
        assert_eq!(reference(&items[0], ItemGroup::Owner), "criccomini/perch #57 · fence");
        assert_eq!(reference(&items[0], ItemGroup::Date), "criccomini/perch #57 · fence");
        assert_eq!(reference(&items[3], ItemGroup::Repository), "#42");
        assert_eq!(reference(&items[3], ItemGroup::Owner), "criccomini/perch #42");
        assert_eq!(reference(&items[5], ItemGroup::Repository), "perch/issue-42");
        assert_eq!(reference(&items[5], ItemGroup::Owner), "criccomini/perch · perch/issue-42");
        assert_eq!(reference(&items[5], ItemGroup::Date), "criccomini/perch · perch/issue-42");
        // Priority and Pinned sit under no repository, so they always say it.
        assert_eq!(full_reference(&items[0]), "criccomini/perch #57 · fence");
        assert_eq!(full_reference(&items[5]), "criccomini/perch · perch/issue-42");
    }

    #[test]
    fn a_pull_request_without_a_checkout_names_its_head_branch() {
        let (_, items) = fixture();
        assert_eq!(reference(&items[1], ItemGroup::Repository), "#58 · spans");
        assert_eq!(reference(&items[1], ItemGroup::Owner), "criccomini/perch #58 · spans");
    }

    #[test]
    fn search_hits_branch_names() {
        let (declared, items) = fixture();
        let f = ItemFilters { query: "ISSUE-42".into(), ..Default::default() };
        let s = sections(&declared, &items, &f, now());
        assert_eq!(s.groups.len(), 1);
        assert_eq!(section_ids(&s.groups[0]), ["branch"]);
        // The pull request with attention does not match, so Priority is empty.
        assert!(s.priority.is_empty());
    }

    #[test]
    fn search_hits_the_number_the_title_the_name_and_the_repository() {
        let (declared, mut items) = fixture();
        items[0].name = Some("the fence one".into());

        let hits = |q: &str| {
            let f = ItemFilters { status: ItemStatus::All, query: q.into(), ..Default::default() };
            let s = sections(&declared, &items, &f, now());
            s.groups.iter().flat_map(|g| g.items.iter().map(|i| i.id.clone())).collect::<Vec<_>>()
        };
        assert_eq!(hits("#57"), ["pr-wanted"]);
        assert_eq!(hits("57"), ["pr-wanted"]);
        assert_eq!(hits("fence the manifest"), ["pr-wanted"], "the given title still matches");
        assert_eq!(hits("fence one"), ["pr-wanted"], "and so does the name");
        assert_eq!(hits("slatedb").len(), 1, "the repository name matches");
        assert_eq!(hits("criccomini/").len(), 6, "and so does the owner");
        assert!(hits("nothing like this").is_empty());
    }

    #[test]
    fn pinned_items_are_not_repeated_under_their_repository() {
        let (declared, mut items) = fixture();
        items[3].pinned_at = Some(at("2026-09-17T10:00:00Z"));
        items[5].pinned_at = Some(at("2026-09-18T10:00:00Z"));

        let s = sections(&declared, &items, &ItemFilters::default(), now());
        assert_eq!(ids(&s.pinned), ["branch", "issue"], "most recently pinned first");
        assert_eq!(section_ids(&s.groups[0]), ["pr-wanted"]);
        // The badge is not narrowed by pinning.
        assert_eq!(s.groups[0].count, 3);
    }

    #[test]
    fn pinned_is_hidden_when_empty_and_respects_the_search() {
        let (declared, mut items) = fixture();
        assert!(sections(&declared, &items, &ItemFilters::default(), now()).pinned.is_empty());

        items[3].pinned_at = Some(at("2026-09-17T10:00:00Z"));
        let f = ItemFilters { query: "fence".into(), ..Default::default() };
        let s = sections(&declared, &items, &f, now());
        assert!(s.pinned.is_empty());
        assert_eq!(section_ids(&s.groups[0]), ["pr-wanted"]);
    }

    #[test]
    fn saved_only_narrows_priority_and_pinned_as_well_as_the_sections() {
        let (declared, mut items) = fixture();
        // Two items with attention, one saved; two pinned, one saved.
        let mut mentioned = item("issue-mentioned", "criccomini/perch", "2026-09-12T10:00:00Z");
        mentioned.subject = Some(issue(43, "Mentioned issue", "open", "2026-09-15T10:00:00Z"));
        mentioned.attention_at = Some(at("2026-09-17T09:00:00Z"));
        mentioned.attention_reason = Some("mention".into());
        mentioned.saved = true;
        items.push(mentioned);
        items[3].saved = true;
        items[3].pinned_at = Some(at("2026-09-17T10:00:00Z"));
        items[5].pinned_at = Some(at("2026-09-18T10:00:00Z"));
        items[1].saved = true;

        let f = ItemFilters { saved_only: true, ..Default::default() };
        let s = sections(&declared, &items, &f, now());
        assert_eq!(ids(&s.priority), ["issue-mentioned"], "the unsaved wanted item is cut");
        assert_eq!(ids(&s.pinned), ["issue"], "the unsaved pinned branch is cut");
        assert_eq!(titles(&s), ["criccomini/perch"]);
        assert_eq!(section_ids(&s.groups[0]), ["issue-mentioned"]);
        // The badge is not narrowed.
        assert_eq!(s.groups[0].count, 4);

        // Across whatever status is set: the saved done item under Done.
        let f = ItemFilters { saved_only: true, status: ItemStatus::Done, ..Default::default() };
        let s = sections(&declared, &items, &f, now());
        assert_eq!(section_ids(&s.groups[0]), ["pr-done"]);
        assert_eq!(ids(&s.priority), ["issue-mentioned"]);
    }

    #[test]
    fn attention_only_leaves_priority_alone_and_makes_the_sections_priority_grouped() {
        let (declared, mut items) = fixture();
        items[6].attention_at = Some(at("2026-09-17T12:00:00Z"));
        items[6].attention_reason = Some("comment".into());
        items[5].pinned_at = Some(at("2026-09-18T10:00:00Z"));

        let before = sections(&declared, &items, &ItemFilters::default(), now());
        let f = ItemFilters { attention_only: true, ..Default::default() };
        let s = sections(&declared, &items, &f, now());
        assert_eq!(s.priority, before.priority);
        assert_eq!(ids(&s.priority), ["pr-wanted", "slatedb-pr"]);
        assert!(s.pinned.is_empty(), "the pinned branch has no attention");
        assert_eq!(titles(&s), ["criccomini/perch", "slatedb/slatedb"]);
        assert_eq!(section_ids(&s.groups[0]), ["pr-wanted"]);
        assert_eq!(section_ids(&s.groups[1]), ["slatedb-pr"]);
        let listed: Vec<&str> = s.groups.iter().flat_map(section_ids).collect();
        let mut wanted = ids(&s.priority);
        wanted.sort_unstable();
        let mut listed_sorted = listed.clone();
        listed_sorted.sort_unstable();
        assert_eq!(listed_sorted, wanted, "the sections hold exactly the Priority items");
        assert_eq!(s.groups[0].count, 3, "the badge is not narrowed");

        // Under Group by Date the same items, bucketed.
        let f = ItemFilters { attention_only: true, group: ItemGroup::Date, ..Default::default() };
        let s = sections(&declared, &items, &f, now());
        let by_date: Vec<&str> = s.groups.iter().flat_map(section_ids).collect();
        assert_eq!(by_date, ["pr-wanted", "slatedb-pr"]);
    }

    #[test]
    fn a_reason_matches_the_latest_reason_and_team_mention_is_mentioned() {
        let (declared, mut items) = fixture();
        let mut mentioned = item("issue-mentioned", "criccomini/perch", "2026-09-12T10:00:00Z");
        mentioned.subject = Some(issue(43, "Mentioned issue", "open", "2026-09-15T10:00:00Z"));
        mentioned.attention_at = Some(at("2026-09-17T09:00:00Z"));
        mentioned.attention_reason = Some("team_mention".into());
        items.push(mentioned);
        // The done pull request was a review request; done or not, that is
        // its latest reason.
        items[1].attention_reason = Some("review_requested".into());
        // A reason was a review request once and is a comment now: only the
        // latest counts.
        items[6].attention_at = Some(at("2026-09-17T12:00:00Z"));
        items[6].attention_reason = Some("comment".into());
        // A reason the menu does not offer matches only Any.
        items[5].attention_at = Some(at("2026-09-17T13:00:00Z"));
        items[5].attention_reason = Some("state_change".into());

        let under = |reason: Option<AttentionReason>, status: ItemStatus| {
            let f = ItemFilters { reason, status, ..Default::default() };
            let s = sections(&declared, &items, &f, now());
            let rows: Vec<String> = s.groups.iter().flat_map(|g| g.items.iter().map(|i| i.id.clone())).collect();
            (ids(&s.priority).into_iter().map(String::from).collect::<Vec<_>>(), rows)
        };

        let (priority, rows) = under(Some(AttentionReason::Mention), ItemStatus::Active);
        assert_eq!(priority, ["issue-mentioned"]);
        assert_eq!(rows, ["issue-mentioned"], "Priority and the sections narrow together");

        let (priority, rows) = under(Some(AttentionReason::ReviewRequested), ItemStatus::Active);
        assert_eq!(priority, ["pr-wanted"]);
        assert_eq!(rows, ["pr-wanted"]);

        let (priority, rows) = under(Some(AttentionReason::ReviewRequested), ItemStatus::Done);
        assert!(priority.contains(&"pr-wanted".to_string()));
        assert_eq!(rows, ["pr-done"], "done, and still a review request");

        let (_, rows) = under(Some(AttentionReason::Comment), ItemStatus::Active);
        assert_eq!(rows, ["slatedb-pr"]);

        let (priority, rows) = under(None, ItemStatus::Active);
        assert!(priority.contains(&"branch".to_string()));
        assert!(rows.contains(&"branch".to_string()));
        for reason in AttentionReason::ALL {
            let (priority, rows) = under(Some(reason), ItemStatus::All);
            assert!(!priority.contains(&"branch".to_string()), "{reason:?}");
            assert!(!rows.contains(&"branch".to_string()), "{reason:?}");
        }
    }

    #[test]
    fn oldest_first_reverses_rows_but_not_priority_or_pinned() {
        let (declared, mut items) = fixture();
        let mut older = item("issue-mentioned", "criccomini/perch", "2026-09-12T10:00:00Z");
        older.subject = Some(issue(43, "Mentioned issue", "open", "2026-09-15T10:00:00Z"));
        older.attention_at = Some(at("2026-09-17T09:00:00Z"));
        older.attention_reason = Some("mention".into());
        items.push(older);
        items[3].pinned_at = Some(at("2026-09-17T10:00:00Z"));
        items[5].pinned_at = Some(at("2026-09-18T10:00:00Z"));

        let newest = sections(&declared, &items, &ItemFilters::default(), now());
        let f = ItemFilters { direction: Direction::Oldest, ..Default::default() };
        let oldest = sections(&declared, &items, &f, now());
        assert_eq!(section_ids(&newest.groups[0]), ["pr-wanted", "issue-mentioned"]);
        assert_eq!(section_ids(&oldest.groups[0]), ["issue-mentioned", "pr-wanted"]);
        assert_eq!(oldest.priority, newest.priority, "attention order is not the sort");
        assert_eq!(oldest.pinned, newest.pinned, "pin order is not the sort");
        assert_eq!(ids(&oldest.priority), ["pr-wanted", "issue-mentioned"]);
        assert_eq!(ids(&oldest.pinned), ["branch", "issue"]);
        // The repository order is config order either way.
        assert_eq!(titles(&oldest), titles(&newest));
    }

    #[test]
    fn oldest_first_puts_the_earlier_date_bucket_first() {
        let now = crate::timefmt::tests::noon_today();
        let ago = |h: i64| now - jiff::SignedDuration::from_hours(h);
        let (declared, _) = fixture();
        let mut a = item("a", "criccomini/perch", "2026-09-01T10:00:00Z");
        a.checkout = Some(checkout("a"));
        a.attention_at = Some(ago(2));
        let mut b = item("b", "criccomini/perch", "2026-09-01T10:00:00Z");
        b.checkout = Some(checkout("b"));
        b.attention_at = Some(ago(26));
        let mut c = item("c", "slatedb/slatedb", "2026-09-01T10:00:00Z");
        c.checkout = Some(checkout("c"));
        c.attention_at = Some(ago(24 * 40));
        let items = vec![c, b, a];

        let f = ItemFilters { group: ItemGroup::Date, direction: Direction::Oldest, ..Default::default() };
        let s = sections(&declared, &items, &f, now);
        assert_eq!(titles(&s), ["Earlier", "Yesterday", "Today"]);
        assert_eq!(section_ids(&s.groups[0]), ["c"]);

        // Created puts all three in one bucket; the rows inside it turn over
        // too, tiebreak included.
        let f = ItemFilters {
            group: ItemGroup::Date,
            sort: ItemSort::Created,
            direction: Direction::Oldest,
            ..Default::default()
        };
        let s = sections(&declared, &items, &f, now);
        assert_eq!(s.groups.len(), 1);
        assert_eq!(section_ids(&s.groups[0]), ["c", "b", "a"]);
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
            saved_only: true,
            attention_only: true,
            reason: Some(AttentionReason::Mention),
            direction: Direction::Oldest,
            query: String::new(),
        };
        let s = f.to_meta();
        assert_eq!(
            s,
            "status=closed;kinds=pr,branch;group=date;sort=last_opened;show_empty=1;show_pr_status=0;saved=1;attention=1;reason=mention;direction=oldest"
        );
        assert_eq!(ItemFilters::parse(&s).unwrap(), f);

        let d = ItemFilters::default();
        assert_eq!(ItemFilters::parse(&d.to_meta()).unwrap(), d);
        assert_eq!(ItemFilters::parse("").unwrap(), d, "nothing stored means the defaults");
        assert!(d.to_meta().ends_with(";saved=0;attention=0;reason=any;direction=newest"));

        // A string from before the last four keys still reads, at the defaults.
        let old = "status=done;kinds=pr;group=owner;sort=created;show_empty=0;show_pr_status=1";
        let f = ItemFilters::parse(old).unwrap();
        assert_eq!(f.status, ItemStatus::Done);
        assert!(!f.saved_only && !f.attention_only && f.reason.is_none());
        assert_eq!(f.direction, Direction::Newest);

        // The harness form from the acceptance list.
        let f = ItemFilters::parse("status=done;reason=mention;direction=oldest").unwrap();
        assert_eq!(f.status, ItemStatus::Done);
        assert_eq!(f.reason, Some(AttentionReason::Mention));
        assert_eq!(f.direction, Direction::Oldest);
        assert_eq!(ItemFilters::parse("reason=any").unwrap().reason, None);
        assert_eq!(ItemFilters::parse("reason=team_mention").unwrap().reason, Some(AttentionReason::Mention));
        assert!(ItemFilters::parse("saved=1;attention=on").unwrap().saved_only);

        // The three groupings by their keys, the default first.
        assert!(d.to_meta().contains(";group=repo;"));
        assert_eq!(ItemFilters::parse("group=repo").unwrap().group, ItemGroup::Repository);
        assert_eq!(ItemFilters::parse("group=owner").unwrap().group, ItemGroup::Owner);
        assert_eq!(ItemFilters::parse("group=date").unwrap().group, ItemGroup::Date);
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
        assert!(ItemFilters::parse("saved=yes").unwrap_err().contains("saved"));
        assert!(ItemFilters::parse("attention=maybe").unwrap_err().contains("attention"));
        assert!(ItemFilters::parse("reason=bogus").unwrap_err().contains("reason"));
        assert!(ItemFilters::parse("direction=sideways").unwrap_err().contains("direction"));
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
        for v in AttentionReason::ALL {
            assert_eq!(AttentionReason::from_str(v.key()), Some(v));
            assert_eq!(v.to_string(), v.label());
            assert_eq!(v.title().to_lowercase(), v.label().to_lowercase(), "the menu says what the row says");
            assert!(v.title().starts_with(|c: char| c.is_uppercase()));
        }
        for v in Direction::ALL {
            assert_eq!(Direction::from_key(v.key()), Some(v));
            assert_eq!(v.to_string(), v.label());
        }
    }

    #[test]
    fn attention_reasons_read_as_the_spec_words_them() {
        assert_eq!(attention_label("review_requested"), "review requested");
        assert_eq!(attention_label("mention"), "mentioned");
        assert_eq!(attention_label("team_mention"), "mentioned");
        assert_eq!(attention_label("assign"), "assigned");
        assert_eq!(attention_label("author"), "authored");
        assert_eq!(attention_label("comment"), "commented");
        assert_eq!(attention_label("manual"), "manual");
        assert_eq!(attention_label("ci_failed"), "CI failed");
        assert_eq!(attention_label("state_change"), "state change");

        assert_eq!(AttentionReason::from_str("team_mention"), Some(AttentionReason::Mention));
        assert_eq!(AttentionReason::from_str("mention"), Some(AttentionReason::Mention));
        assert_eq!(AttentionReason::from_str("author"), Some(AttentionReason::Author));
        assert_eq!(AttentionReason::from_str("state_change"), None, "matches only Any");
        assert_eq!(AttentionReason::from_str(""), None);
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
