//! What the review view holds about a pull request and its files.

use std::collections::HashMap;
use std::sync::Arc;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use perch_git::{ChangeStatus, ChangedFile, Commit, Dirt, DisplayBuffer, Scope, TreeEntry};
use perch_github::{Anchor, Checks, CommentThread, Discussion, Issue, IssueComment, Item, PullRequest};
use perch_syntax::{Language, LineSpans};

/// A file prepared for the editor: text to render, and everything needed to
/// colour it and map a rendered line back to a real one.
pub struct LoadedFile {
    pub path: String,
    pub language: Option<Language>,
    /// Head text with the removed lines spliced in.
    pub buffer: DisplayBuffer,
    /// Highlight spans for the head file, by head line.
    pub head_spans: LineSpans,
    /// Highlight spans for the base file, by base line. Used for ghost lines.
    pub base_spans: LineSpans,
    /// `None` when the file is not part of the pull request.
    pub status: Option<ChangeStatus>,
    /// True when the file was too large or not valid text.
    pub truncated: bool,
    /// The scope the two sides came from. A file loaded under another
    /// scope is stale, and reloads rather than repaints.
    pub scope: Scope,
    /// The base side on its own, for the left of a split. `None` when the
    /// file is not in the changed list, or was added.
    pub base_buffer: Option<DisplayBuffer>,
    /// Which base line sits beside which head line, both 1-based, for a
    /// split: `(Some, Some)` is an unchanged or modified pair, `(Some,
    /// None)` a removed line, `(None, Some)` an added one. Empty for a
    /// file outside the changed list.
    pub alignment: Vec<(Option<usize>, Option<usize>)>,
}

impl std::fmt::Debug for LoadedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The buffer can be megabytes; a log line wants the shape, not the text.
        f.debug_struct("LoadedFile")
            .field("path", &self.path)
            .field("language", &self.language.map(|l| l.name()))
            .field("lines", &self.buffer.len())
            .field("status", &self.status)
            .finish()
    }
}

impl LoadedFile {
    pub fn line_count(&self) -> usize {
        self.buffer.len()
    }

    pub fn change_count(&self) -> usize {
        self.buffer.change_starts.len()
    }
}

/// Which list the rail is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailMode {
    Changed,
    AllFiles,
}

/// The item view's two tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemTab {
    #[default]
    Conversation,
    Files,
}

/// How Files draws a changed file: head with the removed lines spliced in,
/// or base and head side by side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    #[default]
    Stacked,
    Split,
}

impl Layout {
    /// The word `meta` stores.
    pub fn name(self) -> &'static str {
        match self {
            Layout::Stacked => "stacked",
            Layout::Split => "split",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "stacked" => Some(Layout::Stacked),
            "split" => Some(Layout::Split),
            _ => None,
        }
    }
}

/// One place a symbol is used, as rust-analyzer lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// Relative to the worktree.
    pub path: String,
    /// 1-based.
    pub line: usize,
    /// 0-based column of the symbol on that line.
    pub character: usize,
    /// The line's text, trimmed of its indent.
    pub text: String,
}

/// The answer to Find references: what was asked about and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct References {
    pub symbol: String,
    pub locations: Vec<Reference>,
    /// How many locations were outside the worktree, in a dependency, and
    /// are not listed.
    pub elsewhere: usize,
    /// True until the answer lands, so an empty answer can say so rather
    /// than look like one still on its way.
    pub pending: bool,
    /// The request's token. The answer is matched against this, not the
    /// shared counter, which every hover moves on.
    pub token: u64,
}

/// What the review view has on screen. One signal, written once per
/// event after every field it depends on, so the view rebuilds when the
/// shape changes and not once per field that changes on the way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Preparing,
    Failed,
    /// A checkout and its files, with or without a pull request.
    Ready,
    /// An issue, which has no files.
    Issue,
}

/// How far along preparing a checkout is.
#[derive(Debug, Clone, PartialEq)]
pub enum Preparing {
    Idle,
    Working { message: String, percent: Option<u8> },
    Failed { message: String },
}

/// How a question runs: which agent, which of its models, and how hard
/// it thinks.
pub type AgentChoice = (perch_agent::Backend, String, perch_agent::Effort);

/// A composer that is open: where the question will be anchored and how
/// it will be run.
#[derive(Debug, Clone, PartialEq)]
pub struct Composer {
    /// `None` starts a thread; `Some` is a follow-up in that thread.
    pub thread_id: Option<String>,
    pub anchor: Option<Anchor>,
    /// Which agent the model belongs to. Set with the model, never on
    /// its own: the picker lists models, and config says whose each is.
    pub backend: perch_agent::Backend,
    pub model: String,
    pub effort: perch_agent::Effort,
    /// A question to start with, when a failed run is retried.
    pub draft: String,
    /// Which opening this is, so a view keyed on what the composer is
    /// about still starts afresh when it is opened again about the same
    /// thing with a new draft. In the composer rather than beside it so
    /// opening is one write.
    pub opening: u64,
}

/// One open editor tab.
#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    pub path: String,
}

/// Everything the review view renders from. All fields are signals, so the
/// struct is `Copy` and can be handed to any view closure.
#[derive(Clone, Copy)]
pub struct ReviewState {
    /// `None` when the inbox is showing.
    pub pull_request: RwSignal<Option<Arc<PullRequest>>>,
    /// The item this review is reading: its checkout, and its pull request
    /// when it has one.
    pub item: RwSignal<Option<Arc<Item>>>,
    /// True once the checkout is ready, whether or not there is a pull
    /// request to show.
    pub ready: RwSignal<bool>,
    /// The issue, when the item is one; the view is the issue view then.
    pub issue: RwSignal<Option<Arc<Issue>>>,
    pub issue_comments: RwSignal<Arc<Vec<IssueComment>>>,
    /// The head the pull request moved to while the worktree had work in
    /// it. The files on screen are the older ones.
    pub stale_head: RwSignal<Option<String>>,
    /// The pull request could not be fetched; this is the checkout as it
    /// was last left.
    pub offline: RwSignal<bool>,
    /// The notification this was opened from, so finishing a review can
    /// dismiss it. `None` when a pull request was opened by any other route,
    /// such as the launch hook, and there is no notification to mark.
    pub thread_id: RwSignal<Option<String>>,
    /// True from the click until the review view is left, so the progress
    /// card can show before the pull request itself has resolved.
    pub opening: RwSignal<bool>,
    pub phase: RwSignal<Phase>,
    pub preparing: RwSignal<Preparing>,
    /// Conversation or Files. Remembered per item in `tabs_by_item` for
    /// the life of the app.
    pub tab: RwSignal<ItemTab>,
    pub tabs_by_item: RwSignal<HashMap<String, ItemTab>>,
    /// Stacked or Split, remembered across items in `meta`.
    pub layout: RwSignal<Layout>,
    /// What the diff is over. Per item; Branch when the item opens.
    pub scope: RwSignal<Scope>,
    /// The commits over the base, newest first, for the scope menu.
    pub commits: RwSignal<Arc<Vec<Commit>>>,
    /// Whether the checkout has unstaged or staged work, for greying the
    /// scope menu's entries.
    pub dirty: RwSignal<Dirt>,
    /// The head at the previous open of this item, which "Since you last
    /// looked" ranges from. `None` before the first.
    pub since_head: RwSignal<Option<String>>,
    /// Everything said on the pull request on GitHub, once fetched or read
    /// back from the cache.
    pub discussion: RwSignal<Option<Arc<Discussion>>>,
    /// The workflow runs' verdict for the head, once fetched. `None` means
    /// unknown, and the header shows no pill.
    pub checks: RwSignal<Option<Arc<Checks>>>,
    /// The (path, blob) pairs the reader has checked as viewed.
    pub viewed: RwSignal<Arc<std::collections::HashSet<(String, String)>>>,
    /// The references panel's content, when it is open.
    pub references: RwSignal<Option<Arc<References>>>,
    /// The panel's height. Dragged.
    pub references_height: RwSignal<f64>,
    pub changed_files: RwSignal<Arc<Vec<ChangedFile>>>,
    pub tree: RwSignal<Arc<Vec<TreeEntry>>>,
    pub rail_mode: RwSignal<RailMode>,
    pub tabs: RwSignal<Vec<Tab>>,
    pub active: RwSignal<Option<String>>,
    pub files: RwSignal<HashMap<String, Arc<LoadedFile>>>,
    /// Paths whose load is in flight, so a tab can show a spinner and a
    /// second click does not queue the work twice.
    pub loading: RwSignal<HashMap<String, ()>>,
    /// Display line the cursor sits on, for the status line.
    pub cursor_line: RwSignal<usize>,
    /// Bumped whenever the active file's content changes, to rebuild the
    /// editor view rather than mutate a read-only document in place.
    pub editor_epoch: RwSignal<u64>,

    /// How many discussion records the archive took in, once it has.
    pub archived: RwSignal<Option<usize>>,

    /// Every thread in the open worktree, with messages and runs.
    pub comment_threads: RwSignal<Arc<Vec<CommentThread>>>,
    /// Whether the thread column is showing.
    pub threads_open: RwSignal<bool>,
    /// An open composer, and where it is anchored.
    pub composer: RwSignal<Option<Composer>>,
    /// How many composers have been opened, for `Composer::opening`.
    pub composer_opened: RwSignal<u64>,
    /// The editor's selection as display lines, first and last inclusive.
    pub selection: RwSignal<Option<(usize, usize)>>,
    /// A gutter marker was clicked: the thread to scroll the column to.
    pub focus_thread: RwSignal<Option<String>>,
    /// A thread whose Delete is waiting for a yes, under its byline.
    pub confirm_thread: RwSignal<Option<String>>,
    /// How wide the thread column is. Dragged.
    pub threads_width: RwSignal<f64>,
    /// Runs whose step list the reader opened.
    pub expanded_runs: RwSignal<std::collections::HashSet<String>>,
    /// The backend, model and effort last sent, which the next composer
    /// starts on.
    pub agent_last: RwSignal<Option<AgentChoice>>,
    /// The display line the pointer is over in the gutter, for the `+`.
    pub hovered_line: RwSignal<Option<usize>>,
    /// True while the composer's text box has the keyboard, so `a` stands
    /// down.
    pub composer_focused: RwSignal<bool>,
    /// What is in the composer's box, so Escape can tell an empty one
    /// from one with a question in it.
    pub composer_draft: RwSignal<String>,
    /// Whether a composer takes the keyboard when it opens. On, except in
    /// the headless tests: a focused editor asks the window for IME, and
    /// floem's headless window cannot give it and panics.
    pub composer_autofocus: RwSignal<bool>,
    /// The text box of a composer laid in place on Conversation, while one
    /// is open, so `a` can focus it and scroll to it.
    pub composer_box: RwSignal<Option<floem::ViewId>>,
    pub lsp_status: RwSignal<perch_lsp::Status>,
    /// Rises with every hover and jump request. An answer carrying an older
    /// token is stale, because the pointer has moved on since it was asked.
    pub request_token: RwSignal<u64>,
    /// Where the popup should appear, in editor-local coordinates.
    pub hover_at: RwSignal<Option<(f64, f64)>>,
    pub hover_text: RwSignal<Option<String>>,
    /// A go-to-definition result waiting to be opened, as (path, head line).
    pub pending_jump: RwSignal<Option<(String, usize)>>,
    /// A head line to scroll to once the file it belongs to is loaded.
    pub jump_to_line: RwSignal<Option<usize>>,
    /// The head position the last request was made about, so the definition
    /// button asks about the same symbol the panel is describing.
    pub last_position: RwSignal<Option<(usize, usize)>>,
    /// Directories the reader has opened in the All files tree. A checkout
    /// has thousands of entries, so everything starts collapsed.
    pub expanded: RwSignal<std::collections::HashSet<String>>,
}

/// How many tabs stay open before the oldest is dropped.
const MAX_TABS: usize = 20;

impl Default for ReviewState {
    fn default() -> Self {
        Self::new()
    }
}

impl ReviewState {
    pub fn new() -> Self {
        Self {
            pull_request: RwSignal::new(None),
            item: RwSignal::new(None),
            ready: RwSignal::new(false),
            issue: RwSignal::new(None),
            issue_comments: RwSignal::new(Arc::new(Vec::new())),
            stale_head: RwSignal::new(None),
            offline: RwSignal::new(false),
            thread_id: RwSignal::new(None),
            opening: RwSignal::new(false),
            phase: RwSignal::new(Phase::Preparing),
            preparing: RwSignal::new(Preparing::Idle),
            tab: RwSignal::new(ItemTab::Conversation),
            tabs_by_item: RwSignal::new(HashMap::new()),
            layout: RwSignal::new(Layout::Stacked),
            scope: RwSignal::new(Scope::Branch),
            commits: RwSignal::new(Arc::new(Vec::new())),
            dirty: RwSignal::new(Dirt::default()),
            since_head: RwSignal::new(None),
            discussion: RwSignal::new(None),
            checks: RwSignal::new(None),
            viewed: RwSignal::new(Arc::new(std::collections::HashSet::new())),
            references: RwSignal::new(None),
            references_height: RwSignal::new(crate::theme::REFERENCES_HEIGHT),
            changed_files: RwSignal::new(Arc::new(Vec::new())),
            tree: RwSignal::new(Arc::new(Vec::new())),
            rail_mode: RwSignal::new(RailMode::Changed),
            tabs: RwSignal::new(Vec::new()),
            active: RwSignal::new(None),
            files: RwSignal::new(HashMap::new()),
            loading: RwSignal::new(HashMap::new()),
            cursor_line: RwSignal::new(0),
            editor_epoch: RwSignal::new(0),
            archived: RwSignal::new(None),
            comment_threads: RwSignal::new(Arc::new(Vec::new())),
            threads_open: RwSignal::new(false),
            composer: RwSignal::new(None),
            composer_opened: RwSignal::new(0),
            selection: RwSignal::new(None),
            focus_thread: RwSignal::new(None),
            confirm_thread: RwSignal::new(None),
            threads_width: RwSignal::new(crate::theme::THREADS_WIDTH),
            expanded_runs: RwSignal::new(std::collections::HashSet::new()),
            agent_last: RwSignal::new(None),
            hovered_line: RwSignal::new(None),
            composer_focused: RwSignal::new(false),
            composer_draft: RwSignal::new(String::new()),
            composer_autofocus: RwSignal::new(true),
            composer_box: RwSignal::new(None),
            lsp_status: RwSignal::new(perch_lsp::Status::Absent),
            request_token: RwSignal::new(0),
            hover_at: RwSignal::new(None),
            hover_text: RwSignal::new(None),
            pending_jump: RwSignal::new(None),
            jump_to_line: RwSignal::new(None),
            last_position: RwSignal::new(None),
            expanded: RwSignal::new(std::collections::HashSet::new()),
        }
    }

    pub fn is_open(&self) -> bool {
        self.ready.get()
    }

    /// Move to a phase. A write of the same value would still rebuild the
    /// view, so it is skipped.
    pub fn set_phase(&self, phase: Phase) {
        if self.phase.get_untracked() != phase {
            self.phase.set(phase);
        }
    }

    /// Start opening something: the progress card, over whatever was
    /// showing, until the next Ready or Failed. Everything that belonged
    /// to the last checkout goes, so the next one's files are not shown
    /// under its name.
    pub fn begin(&self, message: &str) {
        self.reset_checkout();
        // Whatever the last item still has on its way, a discussion or a
        // list of commits, must not land on the one being opened. Until
        // Ready says which item this is, there is none.
        self.item.set(None);
        self.preparing.set(Preparing::Working { message: message.into(), percent: None });
        self.opening.set(true);
        self.set_phase(Phase::Preparing);
    }

    /// Forget the open checkout: its files, tabs, threads and the rest.
    /// The item and `opening` are the caller's to set.
    fn reset_checkout(&self) {
        self.pull_request.set(None);
        self.ready.set(false);
        self.issue.set(None);
        self.issue_comments.set(Arc::new(Vec::new()));
        self.stale_head.set(None);
        self.offline.set(false);
        self.tab.set(ItemTab::Conversation);
        self.scope.set(Scope::Branch);
        self.commits.set(Arc::new(Vec::new()));
        self.dirty.set(Dirt::default());
        self.since_head.set(None);
        self.discussion.set(None);
        self.checks.set(None);
        self.viewed.set(Arc::new(std::collections::HashSet::new()));
        self.references.set(None);
        self.changed_files.set(Arc::new(Vec::new()));
        self.tree.set(Arc::new(Vec::new()));
        self.tabs.set(Vec::new());
        self.active.set(None);
        self.files.set(HashMap::new());
        self.loading.set(HashMap::new());
        self.rail_mode.set(RailMode::Changed);
        self.lsp_status.set(perch_lsp::Status::Absent);
        self.archived.set(None);
        self.comment_threads.set(Arc::new(Vec::new()));
        self.composer.set(None);
        self.selection.set(None);
        self.focus_thread.set(None);
        self.confirm_thread.set(None);
        self.hovered_line.set(None);
        self.composer_focused.set(false);
        self.composer_draft.set(String::new());
        self.expanded.set(std::collections::HashSet::new());
        self.editor_epoch.update(|e| *e += 1);
        self.dismiss_hover();
    }

    /// Show a tab, and remember it for the item.
    pub fn show_tab(&self, tab: ItemTab) {
        self.tab.set(tab);
        if let Some(id) = self.item.with(|i| i.as_ref().map(|i| i.id.clone())) {
            self.tabs_by_item.update(|m| {
                m.insert(id, tab);
            });
        }
    }

    /// The tab the item was on when it was last open, else Conversation.
    pub fn restore_tab(&self) {
        let tab = self
            .item
            .with(|i| i.as_ref().and_then(|i| self.tabs_by_item.with(|m| m.get(&i.id).copied())))
            .unwrap_or_default();
        self.tab.set(tab);
    }

    /// Whether the open item is this one.
    pub fn is_item(&self, id: &str) -> bool {
        self.item.with(|i| i.as_ref().is_some_and(|i| i.id == id))
    }

    /// Whether the review view should be on screen at all.
    pub fn is_showing(&self) -> bool {
        self.opening.get() || self.is_open()
    }

    /// Leave the review view and return to the inbox.
    pub fn close(&self) {
        self.reset_checkout();
        self.opening.set(false);
        self.item.set(None);
        self.thread_id.set(None);
        self.preparing.set(Preparing::Idle);
        self.set_phase(Phase::Preparing);
    }

    /// Start a new request, invalidating any answer still in flight.
    pub fn next_token(&self) -> u64 {
        self.request_token.update(|t| *t += 1);
        self.request_token.get()
    }

    pub fn dismiss_hover(&self) {
        self.hover_at.set(None);
        self.hover_text.set(None);
    }

    pub fn file(&self, path: &str) -> Option<Arc<LoadedFile>> {
        self.files.with(|m| m.get(path).cloned())
    }

    pub fn active_file(&self) -> Option<Arc<LoadedFile>> {
        let path = self.active.get()?;
        self.file(&path)
    }

    pub fn is_loading(&self, path: &str) -> bool {
        self.loading.with(|m| m.contains_key(path))
    }

    /// Focus a tab, opening one if the file is not already open.
    ///
    /// Returns true when the caller should ask the backend to load the file.
    pub fn open_tab(&self, path: &str) -> bool {
        let already_open = self.tabs.with(|t| t.iter().any(|x| x.path == path));
        if !already_open {
            self.tabs.update(|tabs| {
                tabs.push(Tab { path: path.to_string() });
                // Oldest first, so the cap drops the least recently opened.
                if tabs.len() > MAX_TABS {
                    tabs.remove(0);
                }
            });
        }
        self.active.set(Some(path.to_string()));
        self.cursor_line.set(0);
        self.editor_epoch.update(|e| *e += 1);

        let needs_load = self.file(path).is_none() && !self.is_loading(path);
        if needs_load {
            self.loading.update(|m| {
                m.insert(path.to_string(), ());
            });
        }
        needs_load
    }

    pub fn close_tab(&self, path: &str) {
        let was_active = self.active.with(|a| a.as_deref() == Some(path));
        let mut neighbour = None;

        self.tabs.update(|tabs| {
            if let Some(i) = tabs.iter().position(|t| t.path == path) {
                tabs.remove(i);
                neighbour = tabs
                    .get(i)
                    .or_else(|| tabs.get(i.saturating_sub(1)))
                    .map(|t| t.path.clone());
            }
        });

        // Keep the loaded content: reopening a closed tab should not refetch.
        if was_active {
            self.active.set(neighbour);
            self.cursor_line.set(0);
            self.editor_epoch.update(|e| *e += 1);
        }
    }

    pub fn finish_load(&self, path: &str, file: Arc<LoadedFile>) {
        self.loading.update(|m| {
            m.remove(path);
        });
        // A load from a scope since left is thrown away; the reload for
        // the current one is on its way.
        if self.scope.with(|s| *s != file.scope) {
            return;
        }
        self.files.update(|m| {
            m.insert(path.to_string(), file);
        });
        if self.active.with(|a| a.as_deref() == Some(path)) {
            self.editor_epoch.update(|e| *e += 1);
        }
    }

    /// The scope changed: every loaded file was diffed under the old one.
    /// Forget them all and reload the open tabs, active one first, so the
    /// editor repaints from the new sides rather than the stale ones.
    /// Returns the paths to ask the backend for.
    pub fn change_scope(&self, scope: Scope, files: Arc<Vec<ChangedFile>>) -> Vec<String> {
        self.scope.set(scope);
        self.changed_files.set(files);
        // Every loaded file, not only the tabs: a thread card on
        // Conversation shows lines from a file no tab has, and would go
        // blank until something loaded it again.
        let mut paths: Vec<String> = self.files.with(|m| m.keys().cloned().collect());
        self.files.set(HashMap::new());
        self.loading.set(HashMap::new());
        for tab in self.tabs.with(|t| t.iter().map(|t| t.path.clone()).collect::<Vec<_>>()) {
            if !paths.contains(&tab) {
                paths.push(tab);
            }
        }
        if let Some(active) = self.active.get() {
            paths.retain(|p| *p != active);
            paths.insert(0, active);
        }
        self.loading.update(|m| {
            for p in &paths {
                m.insert(p.clone(), ());
            }
        });
        self.editor_epoch.update(|e| *e += 1);
        paths
    }

    /// Whether a changed file's content has been marked viewed.
    pub fn is_viewed(&self, file: &ChangedFile) -> bool {
        match &file.head_blob {
            Some(blob) => self.viewed.with(|v| v.contains(&(file.path.clone(), blob.clone()))),
            None => false,
        }
    }

    /// How many of the changed files are viewed, and how many there are.
    ///
    /// A deletion has no content to view, so it is in neither number.
    pub fn viewed_count(&self) -> (usize, usize) {
        self.changed_files.with(|files| {
            let viewable = files.iter().filter(|f| f.head_blob.is_some());
            let viewed = viewable.clone().filter(|f| self.is_viewed(f)).count();
            (viewed, viewable.count())
        })
    }

    /// The next changed file after `from` in rail order that is not viewed,
    /// wrapping round; `None` when every file is viewed.
    pub fn next_unviewed(&self, from: Option<&str>, forward: bool) -> Option<String> {
        self.changed_files.with(|files| {
            if files.is_empty() {
                return None;
            }
            let n = files.len();
            let start = from.and_then(|p| files.iter().position(|f| f.path == p));
            let order: Vec<usize> = (1..=n)
                .map(|k| {
                    let base = start.map(|s| s as isize).unwrap_or(if forward { -1 } else { n as isize });
                    let k = k as isize;
                    ((base + if forward { k } else { -k }).rem_euclid(n as isize)) as usize
                })
                .collect();
            order
                .into_iter()
                .map(|i| &files[i])
                .find(|f| f.head_blob.is_some() && !self.is_viewed(f))
                .map(|f| f.path.clone())
        })
    }

    pub fn fail_load(&self, path: &str) {
        self.loading.update(|m| {
            m.remove(path);
        });
    }

    /// Status of a path in the pull request, for the tree's change markers.
    /// Fold a run event into the thread list in place, so the running block
    /// updates without a reload.
    pub fn apply_run_event(&self, run_id: &str, event: &perch_agent::RunEvent) {
        self.comment_threads.update(|threads| {
            let mut all: Vec<CommentThread> = threads.as_ref().clone();
            for thread in &mut all {
                for message in &mut thread.messages {
                    if let Some(run) = message.run.as_mut().filter(|r| r.id == run_id) {
                        crate::ask::apply_run_event(run, event);
                    }
                }
            }
            *threads = Arc::new(all);
        });
    }

    /// The head lines under the editor's selection, as an anchor. `None`
    /// when nothing is selected or the selection is all removed lines.
    pub fn anchor_from_selection(&self) -> Option<Anchor> {
        let (first, last) = self.selection.get()?;
        let file = self.active_file()?;
        self.anchor_for_display_range(&file, first, last)
    }

    /// An anchor for a display range in a file.
    pub fn anchor_for_display_range(&self, file: &LoadedFile, first: usize, last: usize) -> Option<Anchor> {
        let (first, last) = (first.min(last), first.max(last));
        let mut heads = (first..=last).filter_map(|d| file.buffer.head_line(d));
        let start = heads.next()?;
        let end = heads.next_back().unwrap_or(start);
        // The commit the lines are of. Under a commit scope that is the
        // commit, not the branch head: the numbers are its, and the quote
        // and the outdated check read from it.
        let head = self.item.with(|i| i.as_ref().and_then(|i| i.checkout.as_ref()).map(|c| c.head_sha.clone()))?;
        let commit_sha = match self.scope.get_untracked() {
            Scope::Commit { sha } => sha,
            _ => head,
        };
        Some(Anchor {
            path: file.path.clone(),
            start_line: start as u32 + 1,
            line: end as u32 + 1,
            side: perch_github::Side::Right,
            commit_sha,
        })
    }

    /// Open a composer, for a new thread on `anchor` or the worktree.
    pub fn open_composer(&self, thread_id: Option<String>, anchor: Option<Anchor>, draft: String, defaults: AgentChoice) {
        let (backend, model, effort) = self.agent_last.get().unwrap_or(defaults);
        self.composer_draft.set(draft.clone());
        let opening = self.composer_opened.get_untracked() + 1;
        self.composer_opened.set(opening);
        self.composer.set(Some(Composer { thread_id, anchor, backend, model, effort, draft, opening }));
        self.show_threads();
    }

    /// Open the thread column, if it is not open already.
    ///
    /// Only writes when the value changes. A signal write notifies whether
    /// or not the value changed, and the column is a `dyn_container` on
    /// this signal, so a redundant write rebuilds the whole column: every
    /// card, and any composer that was just put in it. Rebuilding is not
    /// only wasteful, it crashes: the first composer's editor registers a
    /// layout listener through a deferred message, and by the time that
    /// message runs the editor has been torn down with the old column.
    /// floem then registers the dead view anyway, and the next box tree
    /// commit asks it for a window origin it no longer has.
    ///
    /// The read is untracked. Two of the callers are effects, and a tracked
    /// read would subscribe them to the very signal the close button
    /// writes: closing the column would re-run the effect that opens it
    /// for a file with threads, and the column would come straight back.
    pub fn show_threads(&self) {
        if !self.threads_open.get_untracked() {
            self.threads_open.set(true);
        }
    }

    /// Close the composer and forget its draft.
    pub fn close_composer(&self) {
        self.composer.set(None);
        self.composer_draft.set(String::new());
        // The box goes with the composer and is never told it lost focus,
        // so the flag that stands the keys down is cleared here.
        self.composer_focused.set(false);
    }

    /// The thread whose anchor starts on a display line of the open file.
    pub fn thread_starting_at(&self, file: &LoadedFile, display_line: usize) -> Option<CommentThread> {
        let head = file.buffer.head_line(display_line)? as u32 + 1;
        self.comment_threads.with(|t| {
            t.iter()
                .find(|t| t.anchor.as_ref().is_some_and(|a| a.path == file.path && a.start_line == head))
                .cloned()
        })
    }

    /// GitHub's review comments on a file, each with its replies, in time
    /// order. Only those on the head side with a line GitHub still places:
    /// one on a removed line, or outdated, has nowhere in the file to be.
    pub fn github_threads_in(&self, path: &str) -> Vec<crate::review::timeline::LineThread> {
        self.discussion.with(|d| {
            let Some(d) = d.as_ref() else { return Vec::new() };
            let on_file: Vec<perch_github::ReviewComment> =
                d.review_comments.iter().filter(|c| c.path == path).cloned().collect();
            crate::review::timeline::line_threads(&on_file)
                .into_iter()
                .filter(|t| t.comment.side == perch_github::Side::Right && t.comment.line.is_some())
                .collect()
        })
    }

    /// The GitHub review comment that starts on a display line of the open
    /// file, for the gutter's hollow marker.
    pub fn github_comment_starting_at(&self, file: &LoadedFile, display_line: usize) -> Option<u64> {
        let head = file.buffer.head_line(display_line)? as u64 + 1;
        self.github_threads_in(&file.path)
            .into_iter()
            .find(|t| t.comment.line == Some(head))
            .map(|t| t.comment.id)
    }

    /// Threads anchored in a file, newest first.
    pub fn threads_in(&self, path: &str) -> Vec<CommentThread> {
        let mut out: Vec<CommentThread> = self
            .comment_threads
            .with(|t| t.iter().filter(|t| t.anchor.as_ref().is_some_and(|a| a.path == path)).cloned().collect());
        out.sort_by_key(|t| std::cmp::Reverse(t.created_at));
        out
    }

    /// Everything else: other files and the worktree as a whole, newest first.
    pub fn threads_elsewhere(&self, path: Option<&str>) -> Vec<CommentThread> {
        let mut out: Vec<CommentThread> = self
            .comment_threads
            .with(|t| {
                t.iter()
                    .filter(|t| !t.anchor.as_ref().is_some_and(|a| Some(a.path.as_str()) == path))
                    .cloned()
                    .collect()
            });
        out.sort_by_key(|t| std::cmp::Reverse(t.created_at));
        out
    }

    pub fn status_of(&self, path: &str) -> Option<ChangeStatus> {
        self.changed_files
            .with(|files| files.iter().find(|f| f.path == path).map(|f| f.status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed(path: &str, blob: Option<&str>) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            old_path: None,
            status: if blob.is_some() { ChangeStatus::Modified } else { ChangeStatus::Deleted },
            additions: 1,
            deletions: 1,
            binary: false,
            head_blob: blob.map(String::from),
        }
    }

    /// Three files with content and one deletion, with `b` viewed.
    fn state() -> ReviewState {
        let state = ReviewState::new();
        state.changed_files.set(Arc::new(vec![
            changed("a.rs", Some("blob-a")),
            changed("b.rs", Some("blob-b")),
            changed("gone.rs", None),
            changed("c.rs", Some("blob-c")),
        ]));
        state.viewed.set(Arc::new([("b.rs".to_string(), "blob-b".to_string())].into_iter().collect()));
        state
    }

    #[test]
    fn viewed_is_by_path_and_blob_and_a_deletion_never_is() {
        let state = state();
        let files = state.changed_files.get();
        assert!(state.is_viewed(&files[1]));
        assert!(!state.is_viewed(&files[0]));
        assert!(!state.is_viewed(&changed("b.rs", Some("blob-b2"))), "a new blob is new content");
        assert!(!state.is_viewed(&files[2]));
        // The deletion has nothing to view, so it is not counted.
        assert_eq!(state.viewed_count(), (1, 3));
    }

    #[test]
    fn next_unviewed_skips_viewed_files_and_wraps_both_ways() {
        let state = state();
        // b is viewed and the deletion has nothing to view, so both are
        // stepped over.
        assert_eq!(state.next_unviewed(Some("a.rs"), true).as_deref(), Some("c.rs"));
        assert_eq!(state.next_unviewed(Some("c.rs"), true).as_deref(), Some("a.rs"), "wraps forward");
        assert_eq!(state.next_unviewed(Some("a.rs"), false).as_deref(), Some("c.rs"), "wraps back");
        assert_eq!(state.next_unviewed(Some("gone.rs"), false).as_deref(), Some("a.rs"));
        // No open file: the first from either end.
        assert_eq!(state.next_unviewed(None, true).as_deref(), Some("a.rs"));
        assert_eq!(state.next_unviewed(None, false).as_deref(), Some("c.rs"));
        // A file outside the list counts as none open.
        assert_eq!(state.next_unviewed(Some("elsewhere.rs"), true).as_deref(), Some("a.rs"));
    }

    #[test]
    fn next_unviewed_is_none_once_everything_is_viewed() {
        let state = state();
        state.viewed.set(Arc::new(
            [("a.rs", "blob-a"), ("b.rs", "blob-b"), ("c.rs", "blob-c")]
                .into_iter()
                .map(|(p, b)| (p.to_string(), b.to_string()))
                .collect(),
        ));
        // The deletion has no blob and is not in the round, so a review
        // that deletes a file can still be viewed whole.
        assert_eq!(state.next_unviewed(Some("a.rs"), true), None);
        assert_eq!(state.viewed_count(), (3, 3));

        state.changed_files.set(Arc::new(vec![changed("a.rs", Some("blob-a"))]));
        assert_eq!(state.next_unviewed(Some("a.rs"), true), None);
        assert_eq!(state.next_unviewed(None, false), None);
        assert_eq!(state.viewed_count(), (1, 1));

        state.changed_files.set(Arc::new(Vec::new()));
        assert_eq!(state.next_unviewed(None, true), None);
        assert_eq!(state.viewed_count(), (0, 0));
    }
}
