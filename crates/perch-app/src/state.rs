//! Reactive app state.
//!
//! Every field is a signal, so `AppState` is `Copy` and can be handed to any
//! view closure without lifetime plumbing. Derived lists are plain functions
//! over these signals, recomputed by whichever effect reads them.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use floem::reactive::{Effect, RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith};
use jiff::Timestamp;
use perch_github::{AgentConfig, Item, ItemKind, Mark, Project, SubjectDetail, SubjectKind, Thread};

use crate::backend::{Event, Status};
use crate::filter::{self, Counts, Filters, Group, ReasonFilter, Section, Sort, View};

/// What the window is showing when no review is open: the sidebar with its
/// projects and an empty main area, or the inbox of notifications that are
/// not about any item.
///
/// The review view sits over either. `AppState::return_to` remembers which
/// one it was opened from, so leaving a review lands where it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Sidebar,
    Inbox,
}

/// How much of a set is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checked {
    Empty,
    /// Some but not all, which the box draws as a dash.
    Partial,
    Full,
}

#[derive(Clone, Copy)]
pub struct AppState {
    pub threads: RwSignal<Vec<Thread>>,
    pub details: RwSignal<HashMap<String, SubjectDetail>>,
    /// Encoded image bytes, shared so a clone is a refcount bump.
    pub avatars: RwSignal<HashMap<String, Arc<Vec<u8>>>>,
    /// Perch's own saved and done flags, keyed by thread id.
    pub marks: RwSignal<HashMap<String, Mark>>,
    /// Every project, declared in config or made for an item.
    pub projects: RwSignal<Vec<Project>>,
    /// Every item, active and archived.
    pub items: RwSignal<Vec<Item>>,
    /// The item whose row is selected in the sidebar: the open one.
    pub open_item: RwSignal<Option<String>>,
    /// The agent section of config.toml: binary, defaults, and the models
    /// the picker offers.
    pub agent: RwSignal<AgentConfig>,
    /// Something a worktree action could not do, for the status line.
    /// Cleared by the next thing that goes right.
    pub notice: RwSignal<Option<String>>,

    /// The sidebar, or the inbox.
    pub screen: RwSignal<Screen>,
    /// The screen a review was opened over, and returns to.
    pub return_to: RwSignal<Screen>,
    /// The item the pointer is over, so a row can show its buttons and a
    /// bare keystroke knows what it means.
    pub hovered_item: RwSignal<Option<String>>,
    /// The sidebar's filter menu settings and search.
    pub item_filters: RwSignal<crate::items::ItemFilters>,
    /// Project sections folded shut, by project id.
    pub collapsed: RwSignal<HashSet<String>>,
    /// The item whose title is a text box right now.
    pub renaming: RwSignal<Option<String>>,
    /// The item with a Delete confirmation open under it.
    pub confirm_delete: RwSignal<Option<String>>,
    /// The item with a "New project" box open under it.
    pub new_project_for: RwSignal<Option<String>>,
    /// The sidebar's search box, so Cmd-1 can focus it.
    pub search_box: RwSignal<Option<floem::ViewId>>,
    /// What is typed in the sidebar's search box.
    pub item_query: RwSignal<String>,
    /// A setting the sidebar wants remembered, picked up by the root view
    /// which has the backend handle.
    pub pending_meta: RwSignal<Option<(String, String)>>,
    /// Which of Inbox, Saved and Done is showing.
    pub view: RwSignal<View>,
    pub reason_filter: RwSignal<Option<ReasonFilter>>,
    pub repo_filter: RwSignal<Option<String>>,
    pub unread_only: RwSignal<bool>,
    pub search: RwSignal<String>,
    pub sort: RwSignal<Sort>,
    pub group: RwSignal<Group>,

    pub status: RwSignal<Status>,
    /// True once any inbox has been applied, so the empty state can tell
    /// "still loading" from "nothing matches".
    pub loaded: RwSignal<bool>,
    /// Ticks forward on refresh, to re-render relative timestamps.
    pub now: RwSignal<Timestamp>,
    /// Whether the left drawer is showing.
    pub sidebar_open: RwSignal<bool>,
    /// How wide the drawer is. Dragged, so a signal rather than a constant.
    pub sidebar_width: RwSignal<f64>,
    /// Whether the drawer's edge is being dragged. The cursor follows what
    /// is under the pointer, and a drag runs over rows that each want their
    /// own, so the window keeps the resize cursor up itself for the duration.
    pub sidebar_resizing: RwSignal<bool>,
    /// The thread the pointer is over, which is what a bare keystroke acts on.
    pub hovered: RwSignal<Option<String>>,
    /// Checked rows, by thread id. Bulk actions act on exactly this set.
    pub selected: RwSignal<HashSet<String>>,
    /// The last row checked, which is the anchor a shift-click extends from.
    pub last_toggled: RwSignal<Option<String>>,
    /// True while the search box has the keyboard.
    ///
    /// Bare letters are shortcuts everywhere except inside a text field, and
    /// the root handler cannot see focus on its own.
    pub search_focused: RwSignal<bool>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            threads: RwSignal::new(Vec::new()),
            details: RwSignal::new(HashMap::new()),
            avatars: RwSignal::new(HashMap::new()),
            marks: RwSignal::new(HashMap::new()),
            projects: RwSignal::new(Vec::new()),
            items: RwSignal::new(Vec::new()),
            open_item: RwSignal::new(None),
            notice: RwSignal::new(None),
            agent: RwSignal::new(AgentConfig::default()),
            screen: RwSignal::new(Screen::Sidebar),
            return_to: RwSignal::new(Screen::Sidebar),
            hovered_item: RwSignal::new(None),
            item_filters: RwSignal::new(crate::items::ItemFilters::default()),
            collapsed: RwSignal::new(HashSet::new()),
            renaming: RwSignal::new(None),
            confirm_delete: RwSignal::new(None),
            new_project_for: RwSignal::new(None),
            search_box: RwSignal::new(None),
            item_query: RwSignal::new(String::new()),
            pending_meta: RwSignal::new(None),
            view: RwSignal::new(View::default()),
            reason_filter: RwSignal::new(None),
            repo_filter: RwSignal::new(None),
            unread_only: RwSignal::new(false),
            search: RwSignal::new(String::new()),
            sort: RwSignal::new(Sort::Newest),
            group: RwSignal::new(Group::Repository),
            status: RwSignal::new(Status::Refreshing),
            loaded: RwSignal::new(false),
            now: RwSignal::new(Timestamp::now()),
            sidebar_open: RwSignal::new(true),
            sidebar_width: RwSignal::new(crate::theme::SIDEBAR_WIDTH),
            sidebar_resizing: RwSignal::new(false),
            hovered: RwSignal::new(None),
            selected: RwSignal::new(HashSet::new()),
            last_toggled: RwSignal::new(None),
            search_focused: RwSignal::new(false),
        }
    }

    /// Fold one backend event into the inbox or the review view.
    pub fn apply_with_review(&self, event: Event, review: crate::review::model::ReviewState) {
        use crate::review::model::Preparing;

        match event {
            Event::ReviewProgress { message, percent } => {
                review.preparing.set(Preparing::Working { message, percent });
            }
            Event::ReviewReady { item, pull_request, files, tree, stale_head, offline } => {
                review.preparing.set(Preparing::Idle);
                review.changed_files.set(files);
                review.tree.set(tree);
                self.open_item.set(Some(item.id.clone()));
                review.item.set(Some(item));
                review.stale_head.set(stale_head);
                review.offline.set(offline);
                review.pull_request.set(pull_request);
                review.ready.set(true);
            }
            Event::IssueReady { item, issue, comments, offline } => {
                review.preparing.set(Preparing::Idle);
                review.changed_files.set(Arc::new(Vec::new()));
                review.tree.set(Arc::new(Vec::new()));
                self.open_item.set(Some(item.id.clone()));
                review.item.set(Some(item));
                review.pull_request.set(None);
                review.offline.set(offline);
                review.issue.set(Some(issue));
                review.issue_comments.set(comments);
                review.ready.set(true);
            }
            Event::CommentThreads { item_id, threads } => {
                let open = review.item.with(|i| i.as_ref().map(|i| i.id.clone()));
                if open.as_deref() == Some(item_id.as_str()) {
                    review.comment_threads.set(Arc::new(threads));
                }
            }
            Event::AgentStep { run_id, thread_id, event } => {
                tracing::debug!(run_id, thread_id, ?event, "run event");
                review.apply_run_event(&run_id, &event);
            }
            Event::Archived { repo_full_name, number, added } => {
                // Only for the review that is still open; a slow archive of
                // a pull request since left has nothing to say.
                let same = review.pull_request.with(|p| {
                    p.as_ref()
                        .is_some_and(|p| p.repo_full_name == repo_full_name && p.number == number)
                });
                if same {
                    review.archived.set(Some(added));
                }
            }
            Event::Items(items) => {
                // The open review can be archived from the sidebar, or by
                // the sweep, or deleted. Its files are gone, so the view
                // goes with them. Otherwise the header follows the row, so
                // a branch that gained a pull request fills in.
                let open = review.item.with(|i| i.as_ref().map(|i| i.id.clone()));
                if let Some(open) = open {
                    match items.iter().find(|i| i.id == open) {
                        Some(i) if i.is_archived() => self.leave_review(review),
                        Some(i) => {
                            review.item.set(Some(Arc::new(i.clone())));
                            if let Some(pr) = i.pull_request() {
                                if review.pull_request.with(|p| p.as_ref().map(|p| p.number) != Some(pr.number)) {
                                    review.pull_request.set(Some(Arc::new(pr.clone())));
                                }
                            }
                        }
                        None => self.leave_review(review),
                    }
                }
                self.apply(Event::Items(items));
            }
            Event::ReviewFailed { message } => {
                review.preparing.set(Preparing::Failed { message });
            }
            Event::LspStatus(status) => review.lsp_status.set(status),
            Event::LspHover { token, text } => {
                // Drop an answer the pointer has already moved past.
                if review.request_token.get() == token {
                    review.hover_text.set(Some(text));
                }
            }
            Event::LspDefinition { token, path, line } => {
                if review.request_token.get() == token {
                    if let Some(path) = path {
                        review.pending_jump.set(Some((path, line as usize)));
                    }
                }
            }
            Event::FileLoaded { path, file } => review.finish_load(&path, file),
            Event::FileFailed { path, message } => {
                tracing::warn!(path, message, "could not open file");
                review.fail_load(&path);
            }
            other => self.apply(other),
        }
    }

    /// Fold one inbox event into the state.
    pub fn apply(&self, event: Event) {
        match event {
            Event::Threads { threads, from_cache } => {
                // The cache read races the first network response. If the
                // network won, its list is authoritative and the cached one
                // is stale by definition.
                if from_cache && self.loaded.get() {
                    return;
                }
                self.threads.set(threads);
                self.loaded.set(true);
                self.now.set(Timestamp::now());
                // A repository filter pointing at a repo that left the inbox
                // would show an empty list with no way to tell why.
                // Against the pool, not the live list: a repository can
                // survive in Saved and Done on snapshots alone, and dropping
                // the filter would empty the view with no explanation.
                let pool = self.pool();
                let still_present = self.repo_filter.with(|f| {
                    f.as_ref()
                        .map(|name| pool.iter().any(|t| &t.repository.full_name == name))
                });
                if still_present == Some(false) {
                    self.repo_filter.set(None);
                }
                self.prune_selection();
            }
            Event::Detail { thread_id, detail } => {
                self.details.update(|m| {
                    m.insert(thread_id, detail);
                });
            }
            Event::Avatar { url, bytes } => {
                self.avatars.update(|m| {
                    m.insert(url, Arc::new(bytes));
                });
            }
            // The backend sends the whole set, so replace rather than merge.
            Event::Marks(marks) => {
                self.marks.set(
                    marks
                        .into_iter()
                        .map(|m| (m.thread_id.clone(), m))
                        .collect(),
                );
                // Marking a row done takes it off the screen, and a
                // resurfaced one leaves Done the same way.
                self.prune_selection();
            }
            Event::Items(items) => {
                self.items.set(items);
                self.notice.set(None);
                // A notification that has become an item leaves the inbox,
                // and with it any check it had.
                self.prune_selection();
            }
            Event::Projects(projects) => self.projects.set(projects),
            Event::Meta { key, value } => match key.as_str() {
                "item_filters" => match crate::items::ItemFilters::parse(&value) {
                    Ok(f) => self.item_filters.set(f),
                    Err(e) => tracing::warn!(error = %e, "ignoring saved item filters"),
                },
                "collapsed_projects" => {
                    self.collapsed.set(value.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect())
                }
                _ => {}
            },
            Event::Notice(message) => self.notice.set(Some(message)),
            Event::AgentConfig(agent) => self.agent.set(agent),
            Event::Archived { .. }
            | Event::CommentThreads { .. }
            | Event::AgentStep { .. }
            | Event::IssueReady { .. } => {}
            Event::ReviewProgress { .. }
            | Event::ReviewReady { .. }
            | Event::ReviewFailed { .. }
            | Event::FileLoaded { .. }
            | Event::FileFailed { .. }
            | Event::LspStatus(_)
            | Event::LspHover { .. }
            | Event::LspDefinition { .. } => {}
            Event::Status(status) => {
                if let Status::Idle { at, .. } = &status {
                    self.now.set(*at);
                }
                self.status.set(status);
            }
        }
    }

    pub fn filters(&self) -> Filters {
        Filters {
            view: self.view.get(),
            reason: self.reason_filter.get(),
            repo: self.repo_filter.get(),
            unread_only: self.unread_only.get(),
            search: self.search.get(),
        }
    }

    /// Every thread the inbox can show: the live inbox, plus a snapshot for
    /// any marked thread GitHub has stopped returning, less every thread
    /// that is about an item. Those have become the item's attention and
    /// are acted on there; the two sets are disjoint by construction.
    pub fn pool(&self) -> Vec<Thread> {
        let pool = self
            .threads
            .with(|live| self.marks.with(|marks| filter::pool(live, marks)));
        self.items.with(|items| {
            pool.into_iter()
                .filter(|t| !resolves_to_item(t, items))
                .collect()
        })
    }

    /// The ids the API still returns.
    fn live_ids(&self) -> HashSet<String> {
        self.threads.with(|live| filter::live_ids(live))
    }

    /// Filtered and sorted threads.
    pub fn visible(&self) -> Vec<Thread> {
        let pool = self.pool();
        let live = self.live_ids();
        self.marks
            .with(|marks| filter::visible(&pool, &live, marks, &self.filters(), self.sort.get()))
    }

    /// The list as the view renders it.
    ///
    /// Reads only the signals that affect grouping, so an arriving detail or
    /// avatar updates individual rows without rebuilding the list.
    pub fn sections(&self) -> Vec<Section> {
        let visible = self.visible();
        filter::sections(&visible, self.group.get(), self.sort.get(), self.now.get())
    }

    pub fn counts(&self) -> Counts {
        let pool = self.pool();
        let live = self.live_ids();
        self.marks
            .with(|marks| filter::counts(&pool, &live, marks, self.view.get()))
    }

    /// How many threads the open view holds, before the other filters.
    pub fn view_total(&self) -> usize {
        let pool = self.pool();
        let live = self.live_ids();
        self.marks
            .with(|marks| filter::view_total(&pool, &live, marks, self.view.get()))
    }

    pub fn is_saved(&self, thread_id: &str) -> bool {
        self.marks.with(|m| m.get(thread_id).is_some_and(|m| m.saved))
    }

    pub fn is_done(&self, thread_id: &str) -> bool {
        self.marks
            .with(|m| m.get(thread_id).is_some_and(perch_github::Mark::is_done))
    }

    // -----------------------------------------------------------------
    // Selection
    // -----------------------------------------------------------------

    /// Forget the selection whenever the list underneath it changes shape.
    ///
    /// Checking rows and then narrowing the list would leave a selection the
    /// reader can no longer see, and acting on it would be a nasty surprise.
    /// Sort and group are deliberately absent: they reorder the same rows.
    ///
    /// Called once from the root view, which is where the reactive scope is.
    pub fn watch_filters(&self) {
        let state = *self;
        Effect::new(move |_| {
            state.view.track();
            state.reason_filter.track();
            state.repo_filter.track();
            state.unread_only.track();
            state.search.track();
            // Untracked: clear_selection reads the signals it writes, and
            // subscribing to those would make this effect depend on its own
            // output and run a second time for nothing.
            Effect::untrack(|| state.clear_selection());
        });
    }

    /// Whether the list is showing this thread right now.
    ///
    /// The hover follows pointer movement, so a row that leaves the screen
    /// under a still pointer stays named by it. Anything acting on the hover
    /// asks this first, rather than every path that can remove a row having
    /// to remember to clear it.
    pub fn is_visible(&self, thread_id: &str) -> bool {
        self.visible().iter().any(|t| t.id == thread_id)
    }

    pub fn is_selected(&self, thread_id: &str) -> bool {
        self.selected.with(|s| s.contains(thread_id))
    }

    pub fn selection(&self) -> Vec<String> {
        self.selected.with(|s| s.iter().cloned().collect())
    }

    pub fn selection_len(&self) -> usize {
        self.selected.with(HashSet::len)
    }

    /// Check or uncheck one row, or extend from the last one checked.
    ///
    /// Extending adds the whole run rather than toggling it, which is what a
    /// shift-click means everywhere else.
    pub fn toggle_selected(&self, thread_id: &str, extend: bool) {
        if extend {
            if let Some(anchor) = self.last_toggled.get() {
                let order = self.visible_order();
                // No run means the anchor has left the list, so fall through
                // and treat this as an ordinary click.
                if let Some(run) = filter::range_between(&order, &anchor, thread_id) {
                    let run: Vec<String> = run.to_vec();
                    self.selected.update(|s| s.extend(run));
                    self.last_toggled.set(Some(thread_id.to_string()));
                    return;
                }
            }
        }
        self.selected.update(|s| {
            if !s.remove(thread_id) {
                s.insert(thread_id.to_string());
            }
        });
        self.last_toggled.set(Some(thread_id.to_string()));
    }

    /// Thread ids in the order the list draws them, cards and all.
    ///
    /// Not the same as `visible()` once the list is grouped: sorting puts two
    /// repositories' threads in one sequence, while the screen shows each
    /// repository as its own card. A shift-click means the run between two
    /// rows as the reader sees them, so it has to follow the cards.
    fn visible_order(&self) -> Vec<String> {
        self.sections()
            .into_iter()
            .flat_map(|s| s.threads)
            .map(|t| t.id)
            .collect()
    }

    /// Check every row the filters currently show, scrolled into view or not.
    pub fn select_all_visible(&self) {
        let ids: HashSet<String> = self.visible().into_iter().map(|t| t.id).collect();
        self.selected.set(ids);
        self.last_toggled.set(None);
    }

    pub fn clear_selection(&self) {
        if self.selected.with(|s| !s.is_empty()) {
            self.selected.update(HashSet::clear);
        }
        if self.last_toggled.with(Option::is_some) {
            self.last_toggled.set(None);
        }
    }

    /// Drop checked rows that have left the list.
    ///
    /// Against what the reader can actually see, not against everything the
    /// app holds: a thread that resurfaces out of Done is still in the pool,
    /// and leaving it checked would have the bar count a row that is no
    /// longer on screen.
    ///
    /// A poll must never quietly empty the selection, so this removes only
    /// ids that are genuinely gone, and writes nothing when none are.
    fn prune_selection(&self) {
        if self.selected.with(HashSet::is_empty) {
            return;
        }
        let shown: HashSet<String> = self.visible().into_iter().map(|t| t.id).collect();
        let stale = self
            .selected
            .with(|s| s.iter().any(|id| !shown.contains(id)));
        if stale {
            self.selected.update(|s| s.retain(|id| shown.contains(id)));
        }
    }

    /// How the Select all box should draw itself.
    pub fn selection_state(&self) -> Checked {
        let visible = self.visible();
        if visible.is_empty() {
            return Checked::Empty;
        }
        let checked = self
            .selected
            .with(|s| visible.iter().filter(|t| s.contains(&t.id)).count());
        match checked {
            0 => Checked::Empty,
            n if n == visible.len() => Checked::Full,
            _ => Checked::Partial,
        }
    }

    /// Whether a dismissal across these threads should restore or mark done.
    ///
    /// Restore only when every one of them is already done. The Saved view
    /// can hold a thread that is both saved and done, so asking the view is
    /// not good enough.
    pub fn would_restore(&self, ids: &[String]) -> bool {
        !ids.is_empty() && ids.iter().all(|id| self.is_done(id))
    }

    /// Whether a save across these threads should save or unsave.
    ///
    /// Unsave only when every one of them is already saved, so a mixed
    /// selection saves the rest rather than clearing the lot.
    pub fn would_unsave(&self, ids: &[String]) -> bool {
        !ids.is_empty() && ids.iter().all(|id| self.is_saved(id))
    }

    /// Switch views. Filters carry over, the way the web keeps a search when
    /// you cross from the inbox into Done.
    pub fn set_view(&self, view: View) {
        self.view.set(view);
    }

    // -----------------------------------------------------------------
    // Screens and worktrees
    // -----------------------------------------------------------------

    /// Note where a review is being opened from, so leaving it can go back.
    pub fn begin_review(&self, review: crate::review::model::ReviewState) {
        self.return_to.set(self.screen.get());
        review.pull_request.set(None);
        review.opening.set(true);
    }

    /// Leave the review view for the screen it was opened over.
    pub fn leave_review(&self, review: crate::review::model::ReviewState) {
        review.close();
        self.screen.set(self.return_to.get());
    }

    /// Show a screen, leaving any review. The inbox's filters and selection
    /// are not touched: crossing to the sidebar and back should find the
    /// inbox exactly as it was left.
    pub fn show_screen(&self, review: crate::review::model::ReviewState, screen: Screen) {
        review.close();
        self.open_item.set(None);
        self.screen.set(screen);
        self.notice.set(None);
    }

    pub fn item(&self, id: &str) -> Option<Item> {
        self.items.with(|all| all.iter().find(|i| i.id == id).cloned())
    }

    pub fn project(&self, id: &str) -> Option<Project> {
        self.projects.with(|all| all.iter().find(|p| p.id == id).cloned())
    }

    /// The sidebar as the filters cut it: Priority, Pinned, and the groups.
    pub fn item_sections(&self) -> crate::items::Sections {
        let mut filters = self.item_filters.get();
        filters.query = self.item_query.get();
        self.projects.with(|projects| {
            self.items.with(|items| crate::items::sections(projects, items, &filters, self.now.get()))
        })
    }



    pub fn detail_for(&self, thread_id: &str) -> Option<SubjectDetail> {
        self.details.with(|m| m.get(thread_id).cloned())
    }

    pub fn avatar_for(&self, url: &str) -> Option<Arc<Vec<u8>>> {
        if url.is_empty() {
            return None;
        }
        self.avatars.with(|m| m.get(url).cloned())
    }

    /// Toggle a sidebar reason filter. Selecting the active one clears it.
    pub fn toggle_reason(&self, f: ReasonFilter) {
        self.reason_filter
            .update(|cur| *cur = if *cur == Some(f) { None } else { Some(f) });
    }

    /// Toggle a repository filter. Selecting the active one clears it.
    pub fn toggle_repo(&self, name: &str) {
        self.repo_filter.update(|cur| {
            *cur = if cur.as_deref() == Some(name) {
                None
            } else {
                Some(name.to_string())
            }
        });
    }

}

/// Whether a notification is about an item: same repository, same kind,
/// same number.
pub fn resolves_to_item(thread: &Thread, items: &[Item]) -> bool {
    let kind = match thread.subject.kind {
        SubjectKind::PullRequest => ItemKind::PullRequest,
        SubjectKind::Issue => ItemKind::Issue,
        _ => return false,
    };
    let Some(number) = thread.number_hint() else { return false };
    items.iter().any(|i| {
        i.repo_full_name.eq_ignore_ascii_case(&thread.repository.full_name)
            && i.subject.as_ref().is_some_and(|s| s.kind() == kind && s.number() == number)
    })
}
