//! The sidebar's tabs and their lists: filtering and sorting items. Pure
//! functions over the declared repositories and the items the cache holds.
//!
//! Everything here runs in memory. Changing a setting never refetches, and
//! the sidebar is paintable from the cache alone.
//!
//! Every tab has one filter menu of the same shape: View, then what the
//! list is narrowed to (Repository, State), then Sort By, then
//! what is shown and done. A section is left out only where it cannot
//! apply.

use std::collections::HashSet;

use jiff::Timestamp;
use perch_github::{DeclaredRepo, Item, ItemSubject, SearchHit};

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

/// The three tabs at the top of the sidebar, one source each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarTab {
    /// GitHub's notifications.
    #[default]
    Inbox,
    /// Perch's local items: the worktrees on disk, or what was opened
    /// lately.
    Worktrees,
    /// GitHub's searches.
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

/// Inbox's View: the inbox, or the threads put out of it, to move back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InboxView {
    #[default]
    Inbox,
    Snoozed,
    Done,
}

impl InboxView {
    pub const ALL: [InboxView; 3] = [InboxView::Inbox, InboxView::Snoozed, InboxView::Done];

    pub fn label(self) -> &'static str {
        match self {
            InboxView::Inbox => "Inbox",
            InboxView::Snoozed => "Snoozed",
            InboxView::Done => "Done",
        }
    }

    fn key(self) -> &'static str {
        match self {
            InboxView::Inbox => "inbox",
            InboxView::Snoozed => "snoozed",
            InboxView::Done => "done",
        }
    }

    fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == s)
    }

    /// Whether the view lists threads put out of the inbox, Done or
    /// snoozed, rather than the inbox itself.
    pub fn is_put_away(self) -> bool {
        matches!(self, InboxView::Done | InboxView::Snoozed)
    }
}

impl std::fmt::Display for InboxView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Worktrees' View: the worktrees on disk, or everything opened lately,
/// worktree or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorktreeView {
    #[default]
    Worktrees,
    Recent,
}

impl WorktreeView {
    pub const ALL: [WorktreeView; 2] = [WorktreeView::Worktrees, WorktreeView::Recent];

    pub fn label(self) -> &'static str {
        match self {
            WorktreeView::Worktrees => "Worktrees",
            WorktreeView::Recent => "Recent",
        }
    }

    fn key(self) -> &'static str {
        match self {
            WorktreeView::Worktrees => "worktrees",
            WorktreeView::Recent => "recent",
        }
    }

    fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == s)
    }
}

impl std::fmt::Display for WorktreeView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Browse's View: one of GitHub's searches. Inbox's View offers the
/// same, under its lists, from the reasons GitHub gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BrowseView {
    ReviewRequested,
    Yours,
    Assigned,
    Mentioned,
    /// Everything in the declared repositories, or in the one Repository
    /// picks.
    #[default]
    All,
}

impl BrowseView {
    pub const ALL: [BrowseView; 5] = [
        BrowseView::ReviewRequested,
        BrowseView::Yours,
        BrowseView::Assigned,
        BrowseView::Mentioned,
        BrowseView::All,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BrowseView::ReviewRequested => "Review requested",
            BrowseView::Yours => "Yours",
            BrowseView::Assigned => "Assigned",
            BrowseView::Mentioned => "Mentioned",
            BrowseView::All => "All",
        }
    }

    /// The search the view runs, leaving out archived repositories.
    /// `repository` narrows it to one; without it, All searches every
    /// repository in `github_repositories`. The sort is left out at
    /// GitHub's default, most recently updated first, so the search reads
    /// as it always has.
    ///
    /// `None` when there is nothing to search: a repository that is not
    /// on GitHub, All with no repositories, or a search longer than the
    /// 256 characters GitHub takes.
    pub fn query(
        self,
        repository: Option<&str>,
        github_repositories: &[String],
        state: StateFilter,
        order: Order,
    ) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        match self {
            BrowseView::ReviewRequested => parts.push("is:pr review-requested:@me".into()),
            BrowseView::Yours => parts.push("author:@me".into()),
            BrowseView::Assigned => parts.push("assignee:@me".into()),
            BrowseView::Mentioned => parts.push("mentions:@me".into()),
            BrowseView::All => {}
        }
        match repository {
            Some(repo) if perch_github::is_local(repo) => return None,
            Some(repo) => parts.push(format!("repo:{repo}")),
            None if self == BrowseView::All => {
                if github_repositories.is_empty() {
                    return None;
                }
                parts.extend(github_repositories.iter().map(|repo| format!("repo:{repo}")));
            }
            None => {}
        }
        match state {
            StateFilter::Any => {}
            StateFilter::Open => parts.push("is:open".into()),
            StateFilter::Closed => parts.push("is:closed".into()),
        }
        parts.push("archived:false".into());
        if order != Order::BROWSE {
            parts.push(format!("sort:{}-{}", order.sort.search_key(), order.direction.search_key()));
        }
        Some(parts.join(" ")).filter(|query| query.len() <= 256)
    }

    fn key(self) -> &'static str {
        match self {
            BrowseView::ReviewRequested => "review_requested",
            BrowseView::Yours => "yours",
            BrowseView::Assigned => "assigned",
            BrowseView::Mentioned => "mentioned",
            BrowseView::All => "all",
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

/// State: a pull request's or an issue's, whatever the row is. Merged,
/// closed and not planned are all Merged or Closed. A row with no state
/// to go by, a branch, a release, or a thread not yet looked at, counts as
/// Open, so nothing goes missing for want of a look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateFilter {
    Any,
    Open,
    Closed,
}

impl StateFilter {
    pub const ALL: [StateFilter; 3] = [StateFilter::Any, StateFilter::Open, StateFilter::Closed];

    pub fn label(self) -> &'static str {
        match self {
            StateFilter::Any => "Any",
            StateFilter::Open => "Open",
            StateFilter::Closed => "Merged or Closed",
        }
    }

    /// Whether a row that is `closed` or not passes.
    pub fn admits(self, closed: bool) -> bool {
        match self {
            StateFilter::Any => true,
            StateFilter::Open => !closed,
            StateFilter::Closed => closed,
        }
    }

    fn key(self) -> &'static str {
        match self {
            StateFilter::Any => "any",
            StateFilter::Open => "open",
            StateFilter::Closed => "closed",
        }
    }

    fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == s)
    }
}

impl std::fmt::Display for StateFilter {
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
    /// What GitHub's search can order by. Last opened is Perch's own.
    pub const BROWSE: [ItemSort; 2] = [ItemSort::LastActivity, ItemSort::Created];

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

    /// GitHub's search's name for it. Last opened has none and searches
    /// as Last activity.
    fn search_key(self) -> &'static str {
        match self {
            ItemSort::LastActivity | ItemSort::LastOpened => "updated",
            ItemSort::Created => "created",
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

    fn search_key(self) -> &'static str {
        match self {
            Direction::Newest => "desc",
            Direction::Oldest => "asc",
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

/// A list's Sort By: what it orders by, and from which end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Order {
    pub sort: ItemSort,
    pub direction: Direction,
}

impl Order {
    pub const WORKTREES: Order = Order { sort: ItemSort::LastActivity, direction: Direction::Newest };
    pub const RECENT: Order = Order { sort: ItemSort::LastOpened, direction: Direction::Newest };
    pub const BROWSE: Order = Order { sort: ItemSort::LastActivity, direction: Direction::Newest };
}

/// Inbox's menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxFilters {
    pub view: InboxView,
    /// The threads about the reader's part in them, as Browse's View
    /// searches for it.
    pub whose: BrowseView,
    pub unread_only: bool,
    pub state: StateFilter,
    /// GitHub's order, newest first, or turned over.
    pub direction: Direction,
}

impl Default for InboxFilters {
    fn default() -> Self {
        Self {
            view: InboxView::Inbox,
            whose: BrowseView::All,
            unread_only: false,
            state: StateFilter::Any,
            direction: Direction::Newest,
        }
    }
}

/// Worktrees' menu. Each view keeps its own Sort By.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeFilters {
    pub view: WorktreeView,
    pub unread_only: bool,
    pub state: StateFilter,
    pub worktrees: Order,
    pub recent: Order,
}

impl WorktreeFilters {
    /// The Sort By of the view showing.
    pub fn order(&self) -> Order {
        match self.view {
            WorktreeView::Worktrees => self.worktrees,
            WorktreeView::Recent => self.recent,
        }
    }

    pub fn order_mut(&mut self) -> &mut Order {
        match self.view {
            WorktreeView::Worktrees => &mut self.worktrees,
            WorktreeView::Recent => &mut self.recent,
        }
    }
}

impl Default for WorktreeFilters {
    fn default() -> Self {
        Self {
            view: WorktreeView::Worktrees,
            unread_only: false,
            state: StateFilter::Any,
            worktrees: Order::WORKTREES,
            recent: Order::RECENT,
        }
    }
}

/// Browse's menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseFilters {
    pub view: BrowseView,
    pub state: StateFilter,
    pub order: Order,
}

impl Default for BrowseFilters {
    fn default() -> Self {
        Self { view: BrowseView::All, state: StateFilter::Open, order: Order::BROWSE }
    }
}

/// Everything the tabs, their filter menus and the search box hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemFilters {
    pub tab: SidebarTab,
    /// Repository identity, shared by the three tabs; `None` includes
    /// every repository.
    pub repository: Option<String>,
    /// Shared too: the CI chip and the review pill on every row that has
    /// them.
    pub show_pr_status: bool,
    pub inbox: InboxFilters,
    pub worktrees: WorktreeFilters,
    pub browse: BrowseFilters,
    pub query: String,
}

impl Default for ItemFilters {
    fn default() -> Self {
        Self {
            tab: SidebarTab::Inbox,
            repository: None,
            show_pr_status: true,
            inbox: InboxFilters::default(),
            worktrees: WorktreeFilters::default(),
            browse: BrowseFilters::default(),
            query: String::new(),
        }
    }
}

impl ItemFilters {
    /// The form kept in the `meta` table: `key=value;` pairs. Search is
    /// deliberately transient. Repository identities are percent-encoded
    /// so local names can contain delimiters, percent signs, or spaces.
    ///
    /// `sort` and `direction` are Worktrees', as they were when it had
    /// the only Sort By.
    pub fn to_meta(&self) -> String {
        let (inbox, worktrees, browse) = (&self.inbox, &self.worktrees, &self.browse);
        format!(
            "tab={};repository={};show_pr_status={};\
             inbox={};inbox_whose={};inbox_unread={};inbox_state={};inbox_direction={};\
             worktrees={};worktrees_unread={};worktrees_state={};sort={};direction={};\
             recent_sort={};recent_direction={};\
             browse={};browse_state={};browse_sort={};browse_direction={}",
            self.tab.key(),
            self.repository.as_deref().map(encode_meta_value).unwrap_or_default(),
            u8::from(self.show_pr_status),
            inbox.view.key(),
            inbox.whose.key(),
            u8::from(inbox.unread_only),
            inbox.state.key(),
            inbox.direction.key(),
            worktrees.view.key(),
            u8::from(worktrees.unread_only),
            worktrees.state.key(),
            worktrees.worktrees.sort.key(),
            worktrees.worktrees.direction.key(),
            worktrees.recent.sort.key(),
            worktrees.recent.direction.key(),
            browse.view.key(),
            browse.state.key(),
            browse.order.sort.key(),
            browse.order.direction.key(),
        )
    }

    /// Missing keys keep their defaults. Older forms map onto the menus
    /// as they are now:
    ///
    /// - the old scopes onto the tabs: All is Worktrees, Notifications is
    ///   Inbox, and Authored is Browse › Yours;
    /// - Show's `inbox=all` and `inbox=unread` onto the Inbox view, the
    ///   second with Unread Only;
    /// - `browse=recent` onto Worktrees › Recent while Browse was the tab.
    ///   It was Browse's default, written whether or not anyone chose it,
    ///   so otherwise Worktrees keeps its own view;
    /// - `browse=repository` onto Browse › All, narrowed to the repository
    ///   it listed while Browse is the tab;
    /// - Open · Closed's `closed` onto Browse's State.
    ///
    /// Settings that have gone are validated and ignored.
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut out = Self::default();
        let mut browse_recent = false;
        let mut browse_repository_view = false;
        let mut browse_repository = None;
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
            let flag = || parse_flag(value).ok_or_else(bad);
            let state = || StateFilter::from_key(value).ok_or_else(bad);
            let sort = || ItemSort::from_key(value).ok_or_else(bad);
            let direction = || Direction::from_key(value).ok_or_else(bad);
            match key {
                "tab" => out.tab = SidebarTab::from_key(value).ok_or_else(bad)?,
                "scope" => match value {
                    "all" => out.tab = SidebarTab::Worktrees,
                    "notifications" => out.tab = SidebarTab::Inbox,
                    "authored" => {
                        out.tab = SidebarTab::Browse;
                        out.browse.view = BrowseView::Yours;
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
                    flag()?;
                }
                "show_pr_status" => out.show_pr_status = flag()?,
                "inbox" => match value {
                    "all" => out.inbox.view = InboxView::Inbox,
                    "unread" => {
                        out.inbox.view = InboxView::Inbox;
                        out.inbox.unread_only = true;
                    }
                    _ => out.inbox.view = InboxView::from_key(value).ok_or_else(bad)?,
                },
                "inbox_whose" => out.inbox.whose = BrowseView::from_key(value).ok_or_else(bad)?,
                // Before Done, Show was only ever Unread or All.
                "unread" | "inbox_unread" => out.inbox.unread_only = flag()?,
                "inbox_state" => out.inbox.state = state()?,
                "inbox_direction" => out.inbox.direction = direction()?,
                "worktrees" => out.worktrees.view = WorktreeView::from_key(value).ok_or_else(bad)?,
                "worktrees_unread" => out.worktrees.unread_only = flag()?,
                "worktrees_state" => out.worktrees.state = state()?,
                "sort" => out.worktrees.worktrees.sort = sort()?,
                "direction" => out.worktrees.worktrees.direction = direction()?,
                "recent_sort" => out.worktrees.recent.sort = sort()?,
                "recent_direction" => out.worktrees.recent.direction = direction()?,
                "browse" => match value {
                    "recent" => browse_recent = true,
                    "repository" => browse_repository_view = true,
                    _ => out.browse.view = BrowseView::from_key(value).ok_or_else(bad)?,
                },
                "browse_repository" => browse_repository = repository(value)?,
                "closed" => {
                    out.browse.state = if flag()? { StateFilter::Closed } else { StateFilter::Open }
                }
                "browse_state" => out.browse.state = state()?,
                "browse_sort" => {
                    out.browse.order.sort =
                        sort().ok().filter(|sort| ItemSort::BROWSE.contains(sort)).ok_or_else(bad)?
                }
                "browse_direction" => out.browse.order.direction = direction()?,
                "query" => out.query = value.to_string(),
                _ => return Err(format!("unknown key {key:?}")),
            }
        }
        if browse_recent && out.tab == SidebarTab::Browse {
            out.worktrees.view = WorktreeView::Recent;
            out.tab = SidebarTab::Worktrees;
        }
        if browse_repository_view {
            out.browse.view = BrowseView::All;
            if out.tab == SidebarTab::Browse && browse_repository.is_some() {
                out.repository = browse_repository;
            }
        }
        Ok(out)
    }

    /// The search Browse is showing, given the repositories on GitHub
    /// that All searches without one picked.
    pub fn browse_query(&self, github_repositories: &[String]) -> Option<String> {
        if self.tab != SidebarTab::Browse {
            return None;
        }
        let browse = &self.browse;
        browse.view.query(self.repository.as_deref(), github_repositories, browse.state, browse.order)
    }

    /// The repository on GitHub Browse is narrowed to, while it is the
    /// tab.
    pub fn browse_repository(&self) -> Option<&str> {
        (self.tab == SidebarTab::Browse)
            .then_some(self.repository.as_deref())
            .flatten()
            .filter(|repo| !perch_github::is_local(repo))
    }

    /// Whether the tab's menu is anywhere but its defaults, which lights
    /// the filter button.
    pub fn narrowed(&self) -> bool {
        let d = Self::default();
        self.repository.is_some()
            || self.show_pr_status != d.show_pr_status
            || match self.tab {
                SidebarTab::Inbox => self.inbox != d.inbox,
                SidebarTab::Worktrees => {
                    let w = &self.worktrees;
                    let order = match w.view {
                        WorktreeView::Worktrees => Order::WORKTREES,
                        WorktreeView::Recent => Order::RECENT,
                    };
                    w.view != d.worktrees.view
                        || w.unread_only != d.worktrees.unread_only
                        || w.state != d.worktrees.state
                        || w.order() != order
                }
                SidebarTab::Browse => self.browse != d.browse,
            }
    }

    /// The name of the list the tab shows, for the search box to say what
    /// it searches.
    pub fn view_label(&self) -> &'static str {
        match self.tab {
            SidebarTab::Inbox => self.inbox.view.label(),
            SidebarTab::Worktrees => self.worktrees.view.label(),
            SidebarTab::Browse => self.browse.view.label(),
        }
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
pub fn in_worktrees(item: &Item) -> bool {
    !item.is_archived() && item.checkout.as_ref().is_some_and(|c| c.on_disk)
}

/// Whether the item passes Worktrees' narrowing: the repository, State,
/// Unread Only and the search. `unread` says whether the inbox has an
/// unread notification about it, as its row's dot does.
fn narrowed_to(item: &Item, filters: &ItemFilters, unread: &dyn Fn(&Item) -> bool) -> bool {
    let w = &filters.worktrees;
    filters
        .repository
        .as_ref()
        .is_none_or(|repo| repo.eq_ignore_ascii_case(&item.repo_full_name))
        && w.state.admits(item.is_closed())
        && (!w.unread_only || unread(item))
        && matches_query(item, &filters.query)
}

/// Worktrees › Worktrees: every worktree on disk under the menu and the
/// search, most recently pinned first and then by the sort.
pub fn worktree_items(items: &[Item], filters: &ItemFilters, unread: &dyn Fn(&Item) -> bool) -> Vec<Item> {
    let rows: Vec<&Item> = items
        .iter()
        .filter(|item| in_worktrees(item) && narrowed_to(item, filters, unread))
        .collect();
    let (mut pinned, mut rest): (Vec<&Item>, Vec<&Item>) = rows.into_iter().partition(|item| item.is_pinned());
    sort_pins(&mut pinned);
    sort_items(&mut rest, filters.worktrees.worktrees);
    pinned.into_iter().chain(rest).cloned().collect()
}

/// How many unpinned rows Recent shows.
pub const RECENT_LIMIT: usize = 50;

/// Whether the reader has opened the item. A pull request, an issue or
/// a worktree Perch made was opened, or made, by the reader. One the root
/// scan found is a checkout of the reader's own that Perch only noticed,
/// until it is opened, which moves `last_opened_at` past when it was
/// found.
fn opened(item: &Item) -> bool {
    item.checkout.as_ref().is_none_or(|c| c.owned) || item.last_opened_at > item.created_at
}

/// Worktrees › Recent: everything opened, worktree or not, pinned first,
/// then the `RECENT_LIMIT` most recently opened in the view's sort. The
/// menu and the search narrow what is shown; an older row is found by
/// opening it again, which brings it back.
pub fn recent_items(items: &[Item], filters: &ItemFilters, unread: &dyn Fn(&Item) -> bool) -> Vec<Item> {
    let (mut pinned, mut rest): (Vec<&Item>, Vec<&Item>) =
        items.iter().filter(|item| opened(item)).partition(|item| item.is_pinned());
    sort_pins(&mut pinned);
    rest.sort_by(|a, b| b.last_opened_at.cmp(&a.last_opened_at).then_with(|| a.id.cmp(&b.id)));
    rest.truncate(RECENT_LIMIT);
    sort_items(&mut rest, filters.worktrees.recent);
    pinned
        .into_iter()
        .chain(rest)
        .filter(|item| narrowed_to(item, filters, unread))
        .cloned()
        .collect()
}

/// The local items the tab in `filters` lists: Worktrees' view, or none,
/// for Inbox and Browse.
pub fn local_items(items: &[Item], filters: &ItemFilters, unread: &dyn Fn(&Item) -> bool) -> Vec<Item> {
    match (filters.tab, filters.worktrees.view) {
        (SidebarTab::Worktrees, WorktreeView::Worktrees) => worktree_items(items, filters, unread),
        (SidebarTab::Worktrees, WorktreeView::Recent) => recent_items(items, filters, unread),
        _ => Vec::new(),
    }
}

/// A GitHub view's results under the search box, over the title, the
/// number and the repository, in GitHub's order.
pub fn search_hits(hits: &[SearchHit], query: &str) -> Vec<SearchHit> {
    let needle = query.trim().to_lowercase();
    hits.iter()
        .filter(|hit| {
            needle.is_empty()
                || hit.title.to_lowercase().contains(&needle)
                || format!("#{}", hit.number).contains(&needle)
                || hit.repo_full_name.to_lowercase().contains(&needle)
        })
        .cloned()
        .collect()
}

/// Whether a Work run may be sent on an item: a worktree of the reader's
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

/// The declared repositories on GitHub, in the file's order: what
/// Browse's Repository offers, and what All searches with none picked.
pub fn github_repositories(declared: &[DeclaredRepo]) -> Vec<String> {
    declared
        .iter()
        .filter(|repo| !perch_github::is_local(&repo.identity))
        .map(|repo| repo.identity.clone())
        .collect()
}

/// Most recently pinned first, ties by id.
fn sort_pins(items: &mut [&Item]) {
    items.sort_by(|a, b| b.pinned_at.cmp(&a.pinned_at).then_with(|| a.id.cmp(&b.id)));
}

/// Newest first, ties by id so the order never jitters between refreshes,
/// then the whole thing turned over for Oldest first. Reversing after the
/// sort rather than flipping the comparison keeps one ordering to reason
/// about, and the reversed tiebreak is the price.
fn sort_items(items: &mut [&Item], order: Order) {
    items.sort_by(|a, b| order.sort.time(b).cmp(&order.sort.time(a)).then_with(|| a.id.cmp(&b.id)));
    if order.direction == Direction::Oldest {
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

    fn worktrees_of(items: &[Item], filters: &ItemFilters) -> Vec<Item> {
        worktree_items(items, filters, &|_| false)
    }

    fn recent_of(items: &[Item], query: &str) -> Vec<Item> {
        recent_items(items, &ItemFilters { query: query.into(), ..Default::default() }, &|_| false)
    }

    fn sorted(sort: ItemSort, direction: Direction) -> ItemFilters {
        let mut filters = ItemFilters::default();
        filters.worktrees.worktrees = Order { sort, direction };
        filters
    }

    fn ids(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn worktrees_are_the_checkouts_on_disk_and_recent_is_everything_opened() {
        let (_, items) = fixture();
        assert_eq!(ids(&worktrees_of(&items, &ItemFilters::default())), ["pr-wanted", "branch"]);
        // By when each was last opened, which the fixture leaves at when
        // it was made. The worktrees are in both.
        assert_eq!(
            ids(&recent_of(&items, "")),
            ["branch", "slatedb-pr", "issue", "pr-closed", "pr-wanted", "pr-merged", "branch-archived"]
        );
    }

    #[test]
    fn a_worktree_that_goes_leaves_worktrees_and_stays_in_recent() {
        let (_, mut items) = fixture();
        items[5].checkout.as_mut().unwrap().on_disk = false;
        assert_eq!(ids(&worktrees_of(&items, &ItemFilters::default())), ["pr-wanted"]);
        assert!(ids(&recent_of(&items, "")).contains(&"branch"));

        let (_, mut items) = fixture();
        items[0].archived_at = Some(at("2026-09-18T10:00:00Z"));
        assert_eq!(ids(&worktrees_of(&items, &ItemFilters::default())), ["branch"]);
        assert!(ids(&recent_of(&items, "")).contains(&"pr-wanted"));
    }

    /// A worktree the root scan found is the reader's checkout, noticed:
    /// it is in Worktrees at once and in Recent once it is opened.
    #[test]
    fn a_scanned_worktree_is_recent_once_opened() {
        let (_, mut items) = fixture();
        items[5].checkout.as_mut().unwrap().owned = false;
        assert!(ids(&worktrees_of(&items, &ItemFilters::default())).contains(&"branch"));
        assert!(!ids(&recent_of(&items, "")).contains(&"branch"), "found, not opened");
        items[5].last_opened_at = at("2026-09-20T10:00:00Z");
        assert_eq!(ids(&recent_of(&items, ""))[0], "branch", "opened");
    }

    #[test]
    fn worktrees_sort_and_pin_and_filter_by_repository() {
        let (_, mut items) = fixture();
        let filters = sorted(ItemSort::Created, Direction::Newest);
        assert_eq!(ids(&worktrees_of(&items, &filters)), ["branch", "pr-wanted"]);
        let filters = sorted(ItemSort::Created, Direction::Oldest);
        assert_eq!(ids(&worktrees_of(&items, &filters)), ["pr-wanted", "branch"]);

        items[0].pinned_at = Some(at("2026-09-18T10:00:00Z"));
        let filters = sorted(ItemSort::Created, Direction::Newest);
        assert_eq!(ids(&worktrees_of(&items, &filters)), ["pr-wanted", "branch"], "the pin leads");

        items[0].repo_full_name = "chris/perch".into();
        let filters = ItemFilters { repository: Some("CHRIS/Perch".into()), ..Default::default() };
        assert_eq!(ids(&worktrees_of(&items, &filters)), ["pr-wanted"]);
        let filters = ItemFilters { repository: Some("missing/repo".into()), ..Default::default() };
        assert!(worktrees_of(&items, &filters).is_empty());
    }

    /// State narrows on the pull request's or the issue's own state; a
    /// branch with neither counts as open. Unread Only keeps the rows the
    /// inbox has unread news about.
    #[test]
    fn worktrees_narrow_to_a_state_and_to_the_unread() {
        let (_, mut items) = fixture();
        items[1].checkout = Some(checkout("spans"));
        items[2].checkout = Some(checkout("old"));
        let mut filters = ItemFilters::default();
        assert_eq!(ids(&worktrees_of(&items, &filters)), ["pr-wanted", "pr-closed", "branch", "pr-merged"]);
        filters.worktrees.state = StateFilter::Closed;
        assert_eq!(ids(&worktrees_of(&items, &filters)), ["pr-closed", "pr-merged"]);
        filters.worktrees.state = StateFilter::Open;
        assert_eq!(ids(&worktrees_of(&items, &filters)), ["pr-wanted", "branch"], "a branch is open");

        filters.worktrees.state = StateFilter::Any;
        filters.worktrees.unread_only = true;
        let unread = |item: &Item| item.id == "pr-merged";
        assert_eq!(ids(&worktree_items(&items, &filters, &unread)), ["pr-merged"]);
    }

    /// Recent keeps its own Sort By, applied to the fifty last opened;
    /// the repository and State narrow it as they do Worktrees.
    #[test]
    fn recent_sorts_and_narrows_like_worktrees() {
        let (_, items) = fixture();
        let mut filters = ItemFilters { tab: SidebarTab::Worktrees, ..Default::default() };
        filters.worktrees.view = WorktreeView::Recent;
        filters.worktrees.recent = Order { sort: ItemSort::LastActivity, direction: Direction::Newest };
        assert_eq!(
            ids(&local_items(&items, &filters, &|_| false)),
            ["pr-wanted", "pr-closed", "issue", "branch", "slatedb-pr", "pr-merged", "branch-archived"]
        );
        filters.worktrees.state = StateFilter::Closed;
        assert_eq!(ids(&local_items(&items, &filters, &|_| false)), ["pr-closed", "pr-merged"]);
        filters.worktrees.state = StateFilter::Any;
        filters.repository = Some("slatedb/slatedb".into());
        assert_eq!(ids(&local_items(&items, &filters, &|_| false)), ["slatedb-pr"]);

        filters.worktrees.view = WorktreeView::Worktrees;
        filters.repository = None;
        assert_eq!(ids(&local_items(&items, &filters, &|_| false)), ["pr-wanted", "branch"]);
        filters.tab = SidebarTab::Inbox;
        assert!(local_items(&items, &filters, &|_| false).is_empty());
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
        let rows = recent_of(&items, "");
        assert_eq!(rows.len(), 2 + RECENT_LIMIT);
        assert_eq!(ids(&rows)[..4], ["r01", "r00", "r59", "r58"]);
        assert!(!ids(&rows).contains(&"r09"), "fifty after the pins, the rest not shown");
        assert!(ids(&rows).contains(&"r10"));

        // Opening an old one brings it back to the top.
        items[5].last_opened_at = at("2026-09-25T00:00:00Z");
        assert_eq!(ids(&recent_of(&items, ""))[2], "r05");
    }

    #[test]
    fn search_hits_the_number_titles_branch_and_repository() {
        let (_, mut items) = fixture();
        items[0].name = Some("the fence one".into());
        // Recent has every row of the fixture; the worktrees are opened
        // Perch-made ones.
        let everything = |query: &str| recent_of(&items, query);
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
            show_pr_status: false,
            inbox: InboxFilters {
                view: InboxView::Done,
                whose: BrowseView::Mentioned,
                unread_only: true,
                state: StateFilter::Closed,
                direction: Direction::Oldest,
            },
            worktrees: WorktreeFilters {
                view: WorktreeView::Recent,
                unread_only: true,
                state: StateFilter::Open,
                worktrees: Order { sort: ItemSort::Created, direction: Direction::Oldest },
                recent: Order { sort: ItemSort::LastActivity, direction: Direction::Oldest },
            },
            browse: BrowseFilters {
                view: BrowseView::All,
                state: StateFilter::Any,
                order: Order { sort: ItemSort::Created, direction: Direction::Oldest },
            },
            query: String::new(),
        };
        let meta = filters.to_meta();
        assert!(meta.contains("repository=local/notes%3Bscope%3Dall%25%20%E2%98%83%20;"));
        assert_eq!(ItemFilters::parse(&meta).unwrap(), filters);
        assert_eq!(filters.browse_repository(), None, "not on GitHub");
        let github = ItemFilters { repository: Some("slatedb/slatedb".into()), ..filters.clone() };
        assert_eq!(github.browse_repository(), Some("slatedb/slatedb"));
        assert_eq!(ItemFilters::parse("").unwrap(), ItemFilters::default());
        assert_eq!(ItemFilters::parse(&ItemFilters::default().to_meta()).unwrap(), ItemFilters::default());
        assert_eq!(ItemFilters::default().tab, SidebarTab::Inbox, "Inbox is where it opens");
        assert_eq!(
            ItemFilters::parse("repository=owner/repo").unwrap().repository.as_deref(),
            Some("owner/repo")
        );
        assert_eq!(ItemFilters::parse("repository=").unwrap().repository, None);
        let gone = [";group=", ";show_empty=", ";status=", ";kinds=", ";saved=", ";scope=", ";closed=", ";browse_repository="];
        for gone in gone {
            assert!(!meta.contains(gone), "{gone}");
        }
    }

    /// Show's Unread and All are the Inbox view, the first with Unread
    /// Only; Done and Snoozed are views of their own.
    #[test]
    fn inbox_show_becomes_the_view_and_unread_only() {
        let inbox = |s: &str| ItemFilters::parse(s).unwrap().inbox;
        assert_eq!(inbox("inbox=unread"), InboxFilters { unread_only: true, ..Default::default() });
        assert_eq!(inbox("inbox=all"), InboxFilters::default());
        assert_eq!(inbox("inbox=snoozed").view, InboxView::Snoozed);
        assert_eq!(inbox("inbox=done").view, InboxView::Done);
        assert!(inbox("unread=1").unread_only, "as saved before Done");
        assert!(!inbox("unread=0").unread_only);
    }

    /// Browse › Recent moves to Worktrees when Browse was showing it; By
    /// repository becomes All narrowed to the repository it listed; Open ·
    /// Closed becomes State.
    #[test]
    fn browse_s_old_views_move_onto_the_menus() {
        let old = "tab=browse;repository=org/inbox;browse=recent;browse_repository=a/b;closed=0";
        let filters = ItemFilters::parse(old).unwrap();
        assert_eq!(filters.tab, SidebarTab::Worktrees);
        assert_eq!(filters.worktrees.view, WorktreeView::Recent);
        assert_eq!(filters.browse.view, BrowseView::All);
        assert_eq!(filters.repository.as_deref(), Some("org/inbox"), "not By repository's");
        assert_eq!(filters.browse.state, StateFilter::Open);

        // Recent was Browse's default, saved whether or not it was chosen.
        let filters = ItemFilters::parse("tab=inbox;browse=recent").unwrap();
        assert_eq!((filters.tab, filters.worktrees.view), (SidebarTab::Inbox, WorktreeView::Worktrees));

        let filters =
            ItemFilters::parse("tab=browse;repository=org/inbox;browse=repository;browse_repository=a/b;closed=1").unwrap();
        assert_eq!((filters.tab, filters.browse.view), (SidebarTab::Browse, BrowseView::All));
        assert_eq!(filters.repository.as_deref(), Some("a/b"), "the one it was listing");
        assert_eq!(filters.browse.state, StateFilter::Closed);

        let filters =
            ItemFilters::parse("tab=worktrees;repository=org/inbox;browse=repository;browse_repository=a/b").unwrap();
        assert_eq!(filters.browse.view, BrowseView::All);
        assert_eq!(filters.repository.as_deref(), Some("org/inbox"), "the tab showing keeps its own");
    }

    /// The button lights for whatever the tab showing is narrowed or
    /// ordered by, and for nothing on the other tabs.
    #[test]
    fn the_filter_button_lights_for_the_tab_showing() {
        let mut filters = ItemFilters::default();
        assert!(!filters.narrowed());
        filters.worktrees.state = StateFilter::Closed;
        assert!(!filters.narrowed(), "Worktrees' own");
        filters.tab = SidebarTab::Worktrees;
        assert!(filters.narrowed());
        filters.worktrees.state = StateFilter::Any;
        filters.worktrees.view = WorktreeView::Recent;
        assert!(filters.narrowed(), "a view other than the first");
        filters.worktrees.view = WorktreeView::Worktrees;
        assert!(!filters.narrowed());
        filters.worktrees.recent.direction = Direction::Oldest;
        assert!(!filters.narrowed(), "the order of a view not showing");
        filters.tab = SidebarTab::Browse;
        filters.browse.state = StateFilter::Any;
        assert!(filters.narrowed());
        filters.browse.state = StateFilter::Open;
        filters.repository = Some("a/b".into());
        assert!(filters.narrowed(), "the repository, on every tab");
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
            ("inbox=everything", "inbox"),
            ("worktrees=everything", "worktrees"),
            ("inbox_state=merged", "inbox_state"),
            ("browse_sort=last_opened", "browse_sort"),
            ("recent_direction=up", "recent_direction"),
            ("repository=local/%", "repository"),
            ("repository=%ZZ", "repository"),
            ("browse_repository=%FF", "browse_repository"),
            ("nonsense", "key=value"),
        ] {
            assert!(ItemFilters::parse(setting).unwrap_err().contains(key), "{setting}");
        }
    }

    #[test]
    fn each_github_view_is_a_search() {
        let open = StateFilter::Open;
        let newest = Order::BROWSE;
        let none: &[String] = &[];
        assert_eq!(
            BrowseView::ReviewRequested.query(None, none, open, newest).as_deref(),
            Some("is:pr review-requested:@me is:open archived:false")
        );
        assert_eq!(
            BrowseView::Yours.query(None, none, StateFilter::Closed, newest).as_deref(),
            Some("author:@me is:closed archived:false")
        );
        assert_eq!(
            BrowseView::Assigned.query(None, none, StateFilter::Any, newest).as_deref(),
            Some("assignee:@me archived:false")
        );
        assert_eq!(
            BrowseView::Mentioned.query(Some("a/b"), none, open, newest).as_deref(),
            Some("mentions:@me repo:a/b is:open archived:false"),
            "any view narrows to a repository"
        );
        let oldest_created = Order { sort: ItemSort::Created, direction: Direction::Oldest };
        assert_eq!(
            BrowseView::Yours.query(None, none, open, oldest_created).as_deref(),
            Some("author:@me is:open archived:false sort:created-asc")
        );
        assert_eq!(BrowseView::Yours.query(Some("local/notes"), none, open, newest), None, "not on GitHub");
    }

    /// All searches the repository picked, or every declared one on
    /// GitHub; with none, or more than a search holds, there is nothing
    /// to search.
    #[test]
    fn all_searches_the_declared_repositories() {
        let (declared, _) = fixture();
        let github = github_repositories(&declared);
        let open = StateFilter::Open;
        assert_eq!(
            BrowseView::All.query(None, &github, open, Order::BROWSE).as_deref(),
            Some("repo:criccomini/perch repo:slatedb/slatedb repo:x/empty is:open archived:false")
        );
        assert_eq!(
            BrowseView::All.query(Some("slatedb/slatedb"), &github, open, Order::BROWSE).as_deref(),
            Some("repo:slatedb/slatedb is:open archived:false")
        );
        assert_eq!(BrowseView::All.query(None, &[], open, Order::BROWSE), None);
        let many: Vec<String> = (0..20).map(|n| format!("organisation/repository-{n}")).collect();
        assert_eq!(BrowseView::All.query(None, &many, open, Order::BROWSE), None, "too long");

        let mut filters = ItemFilters::default();
        assert_eq!(filters.browse_query(&github), None, "not while Browse is not showing");
        filters.tab = SidebarTab::Browse;
        assert_eq!(
            filters.browse_query(&github).as_deref(),
            Some("repo:criccomini/perch repo:slatedb/slatedb repo:x/empty is:open archived:false"),
            "All is Browse's default"
        );
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
        for value in InboxView::ALL {
            assert_eq!(InboxView::from_key(value.key()), Some(value));
            assert_eq!(value.to_string(), value.label());
        }
        for value in WorktreeView::ALL {
            assert_eq!(WorktreeView::from_key(value.key()), Some(value));
            assert_eq!(value.to_string(), value.label());
        }
        for value in StateFilter::ALL {
            assert_eq!(StateFilter::from_key(value.key()), Some(value));
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
