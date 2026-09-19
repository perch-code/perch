//! Reactive app state.
//!
//! Every field is a signal, so `AppState` is `Copy` and can be handed to any
//! view closure without lifetime plumbing. Derived lists are plain functions
//! over these signals, recomputed by whichever effect reads them.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use jiff::Timestamp;
use perch_github::{AgentConfig, Item};

use crate::backend::{Event, Status};

/// Whether a box is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checked {
    Empty,
    Full,
}

/// The backfill's progress, as `Event::Backfill` reports it: `done`
/// discussions of about `total` for `repo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archiving {
    pub repo: String,
    pub done: usize,
    pub total: usize,
}

#[derive(Clone, Copy)]
pub struct AppState {
    /// Encoded image bytes, shared so a clone is a refcount bump.
    pub avatars: RwSignal<HashMap<String, Arc<Vec<u8>>>>,
    /// The repositories config.toml declares, `owner/repo` in config
    /// order, which is the order the sidebar lists them in. A repository
    /// with items that nobody declared is a section all the same; only
    /// the items say so.
    pub repos: RwSignal<Vec<String>>,
    /// Every item, active and archived.
    pub items: RwSignal<Vec<Item>>,
    /// The item whose row is selected in the sidebar: the open one.
    pub open_item: RwSignal<Option<String>>,
    /// The agent section of config.toml: binary, defaults, and the models
    /// the picker offers.
    pub agent: RwSignal<AgentConfig>,
    /// Something a worktree action could not do, for the status line.
    /// Cleared by the next act the reader takes.
    pub notice: RwSignal<Option<String>>,
    /// Where the backfill is, for the status bar: the repository being
    /// archived and how far along it is. `None` once every repository
    /// is caught up.
    pub archiving: RwSignal<Option<Archiving>>,
    /// The review view's state, once the window has made it, so a menu
    /// that ends in something being opened can put the progress card up.
    pub review: RwSignal<Option<crate::review::model::ReviewState>>,
    /// The reader's own login and avatar on GitHub, once known, for the
    /// byline of a thread of their own.
    pub viewer: RwSignal<Option<(String, String)>>,
    /// Files the view should ask the backend to load again, after a scope
    /// change threw the loaded ones away. Drained by the window, which
    /// holds the backend handle.
    pub reload: RwSignal<Vec<String>>,

    /// The item the pointer is over, so a row can show its buttons and a
    /// bare keystroke knows what it means.
    pub hovered_item: RwSignal<Option<String>>,
    /// The sidebar's filter menu settings and search.
    pub item_filters: RwSignal<crate::items::ItemFilters>,
    /// Sections folded shut, by section key: `repo:`, `owner:` or `date:`
    /// and the lower-cased name or the label.
    pub collapsed: RwSignal<HashSet<String>>,
    /// The item whose title is a text box right now.
    pub renaming: RwSignal<Option<String>>,
    /// The item with a Delete confirmation open under it.
    pub confirm_delete: RwSignal<Option<String>>,
    /// The sidebar's search box, so Cmd-1 can focus it.
    pub search_box: RwSignal<Option<floem::ViewId>>,
    /// Cmd-1 was pressed with the drawer shut. The box does not exist
    /// until the drawer has been rebuilt, so it takes the keyboard when
    /// it is made and clears this.
    pub focus_search: RwSignal<bool>,
    /// What is typed in the sidebar's search box.
    pub item_query: RwSignal<String>,
    /// A setting the sidebar wants remembered, picked up by the root view
    /// which has the backend handle.
    pub pending_meta: RwSignal<Option<(String, String)>>,

    pub status: RwSignal<Status>,
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
            avatars: RwSignal::new(HashMap::new()),
            repos: RwSignal::new(Vec::new()),
            items: RwSignal::new(Vec::new()),
            open_item: RwSignal::new(None),
            notice: RwSignal::new(None),
            archiving: RwSignal::new(None),
            review: RwSignal::new(None),
            viewer: RwSignal::new(None),
            reload: RwSignal::new(Vec::new()),
            agent: RwSignal::new(AgentConfig::default()),
            hovered_item: RwSignal::new(None),
            item_filters: RwSignal::new(crate::items::ItemFilters::default()),
            collapsed: RwSignal::new(HashSet::new()),
            renaming: RwSignal::new(None),
            confirm_delete: RwSignal::new(None),
            search_box: RwSignal::new(None),
            focus_search: RwSignal::new(false),
            item_query: RwSignal::new(String::new()),
            pending_meta: RwSignal::new(None),
            status: RwSignal::new(Status::Refreshing),
            now: RwSignal::new(Timestamp::now()),
            sidebar_open: RwSignal::new(true),
            sidebar_width: RwSignal::new(crate::theme::SIDEBAR_WIDTH),
            sidebar_resizing: RwSignal::new(false),
            search_focused: RwSignal::new(false),
        }
    }

    /// Fold one backend event into the sidebar or the review view.
    pub fn apply_with_review(&self, event: Event, review: crate::review::model::ReviewState) {
        use crate::review::model::{ItemTab, Phase, Preparing};

        match event {
            Event::ReviewProgress { message, percent } => {
                review.preparing.set(Preparing::Working { message, percent });
                review.set_phase(Phase::Preparing);
            }
            // The phase goes last, after every field the view will read.
            Event::ReviewReady { item, pull_request, files, tree, stale_head, offline } => {
                review.preparing.set(Preparing::Idle);
                review.set_changed_files(files);
                review.tree.set(tree);
                self.open_item.set(Some(item.id.clone()));
                review.item.set(Some(item));
                review.stale_head.set(stale_head);
                review.offline.set(offline);
                review.pull_request.set(pull_request);
                review.issue.set(None);
                review.issue_comments.set(Arc::new(Vec::new()));
                // The threads follow on their own event; whatever is here
                // is the last item's.
                review.comment_threads.set(Arc::new(Vec::new()));
                review.restore_tab();
                review.ready.set(true);
                review.set_phase(Phase::Ready);
            }
            Event::IssueReady { item, issue, comments, offline } => {
                review.preparing.set(Preparing::Idle);
                review.changed_files.set(Arc::new(Vec::new()));
                review.tree.set(Arc::new(Vec::new()));
                self.open_item.set(Some(item.id.clone()));
                review.item.set(Some(item));
                review.pull_request.set(None);
                review.stale_head.set(None);
                review.offline.set(offline);
                review.issue.set(Some(issue));
                review.issue_comments.set(comments);
                review.comment_threads.set(Arc::new(Vec::new()));
                review.tab.set(ItemTab::Conversation);
                review.ready.set(true);
                review.set_phase(Phase::Issue);
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
            Event::Archived { repo_full_name, number, added, .. } => {
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
                            // The snapshot, when it changed: a notification
                            // refetches it, and the header reads this.
                            if let Some(pr) = i.pull_request() {
                                if review.pull_request.with(|p| p.as_deref() != Some(pr)) {
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
                review.set_phase(Phase::Failed);
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
            // A load for the last item, still on its way when the next one
            // opened, is nobody's now.
            Event::FileLoaded { item_id, path, file } => {
                if review.is_item(&item_id) {
                    review.finish_load(&path, file);
                }
            }
            Event::IssueComments { item_id, comments } => {
                if review.is_item(&item_id) {
                    review.issue_comments.set(comments);
                }
            }
            Event::FileFailed { path, message } => {
                tracing::warn!(path, message, "could not open file");
                review.fail_load(&path);
            }
            // Everything about the open item that arrives after it is
            // ready. Each names its item, so a slow answer for the last
            // one opened cannot land on the next.
            Event::DiscussionReady { item_id, discussion } => {
                if review.is_item(&item_id) {
                    review.discussion.set(Some(discussion));
                }
            }
            Event::ChecksReady { item_id, checks } => {
                if review.is_item(&item_id) {
                    review.checks.set(Some(checks));
                }
            }
            Event::CommitsReady { item_id, commits, dirt, since } => {
                if review.is_item(&item_id) {
                    review.commits.set(commits);
                    review.dirty.set(dirt);
                    review.since_head.set(since);
                }
            }
            Event::ScopeReady { item_id, scope, files } => {
                if review.is_item(&item_id) {
                    for path in review.change_scope(scope, files) {
                        self.reload.update(|r| r.push(path));
                    }
                }
            }
            Event::ViewedReady { item_id, viewed } => {
                if review.is_item(&item_id) {
                    review.viewed.set(viewed);
                }
            }
            Event::LspReferences { token, references, elsewhere } => {
                // Matched against the panel's own token: the shared counter
                // moves on with every hover while the answer is on its way.
                review.references.update(|r| {
                    if let Some(r) = r.as_mut().filter(|r| r.token == token) {
                        let mut open = (**r).clone();
                        open.locations = references;
                        open.elsewhere = elsewhere;
                        open.pending = false;
                        *r = Arc::new(open);
                    }
                });
            }
            other => self.apply(other),
        }
    }

    /// Fold one event into what the sidebar and the status bar read.
    pub fn apply(&self, event: Event) {
        match event {
            Event::Avatar { url, bytes } => {
                self.avatars.update(|m| {
                    m.insert(url, Arc::new(bytes));
                });
            }
            Event::Items(items) => self.items.set(items),
            Event::Repos(repos) => self.repos.set(repos),
            Event::Meta { key, value } => match key.as_str() {
                "item_filters" => match crate::items::ItemFilters::parse(&value) {
                    Ok(f) => self.item_filters.set(f),
                    Err(e) => tracing::warn!(error = %e, "ignoring saved item filters"),
                },
                "collapsed_sections" => {
                    self.collapsed.set(value.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect())
                }
                "review.layout" => {
                    if let (Some(layout), Some(review)) =
                        (crate::review::model::Layout::parse(&value), self.review.get_untracked())
                    {
                        review.layout.set(layout);
                    }
                }
                // The two arrive one after the other; whichever comes second
                // completes the pair.
                "viewer.login" => self.viewer.update(|v| {
                    let avatar = v.as_ref().map(|(_, a)| a.clone()).unwrap_or_default();
                    *v = Some((value, avatar));
                }),
                "viewer.avatar_url" => self.viewer.update(|v| {
                    let login = v.as_ref().map(|(l, _)| l.clone()).unwrap_or_default();
                    *v = Some((login, value));
                }),
                _ => {}
            },
            // An empty notice is the backend saying the last one has been
            // read: the reader has moved on to something else.
            Event::Notice(message) => self.notice.set(Some(message).filter(|m| !m.is_empty())),
            // A total of zero is the backfill saying it is caught up.
            Event::Backfill { repo, done, total } => {
                self.archiving.set((total > 0).then_some(Archiving { repo, done, total }))
            }
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
            | Event::LspDefinition { .. }
            | Event::DiscussionReady { .. }
            | Event::IssueComments { .. }
            | Event::ChecksReady { .. }
            | Event::CommitsReady { .. }
            | Event::ScopeReady { .. }
            | Event::ViewedReady { .. }
            | Event::LspReferences { .. } => {}
            Event::Status(status) => {
                if let Status::Idle { at, .. } = &status {
                    self.now.set(*at);
                }
                self.status.set(status);
            }
        }
    }

    // -----------------------------------------------------------------
    // The review view
    // -----------------------------------------------------------------

    /// Put the progress card up with `message` until the backend says what
    /// it opened.
    pub fn begin_review(&self, review: crate::review::model::ReviewState, message: &str) {
        review.begin(message);
    }

    /// The progress card, over the open review or the empty main area, for
    /// an act that ends in another item being opened: Start work, Fork.
    pub fn begin_open(&self, message: &str) {
        let Some(review) = self.review.get_untracked() else { return };
        review.begin(message);
    }

    /// Leave the review view. The sidebar is the only place to land.
    pub fn leave_review(&self, review: crate::review::model::ReviewState) {
        review.close();
    }

    pub fn item(&self, id: &str) -> Option<Item> {
        self.items.with(|all| all.iter().find(|i| i.id == id).cloned())
    }

    /// The sidebar as the filters cut it: Priority, Pinned, and the groups.
    pub fn item_sections(&self) -> crate::items::Sections {
        let mut filters = self.item_filters.get();
        filters.query = self.item_query.get();
        self.repos.with(|repos| {
            self.items.with(|items| crate::items::sections(repos, items, &filters, self.now.get()))
        })
    }

    pub fn avatar_for(&self, url: &str) -> Option<Arc<Vec<u8>>> {
        if url.is_empty() {
            return None;
        }
        self.avatars.with(|m| m.get(url).cloned())
    }
}
