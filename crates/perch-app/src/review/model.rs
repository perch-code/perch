//! What the review view holds about a pull request and its files.

use std::collections::HashMap;
use std::sync::Arc;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use perch_git::{ChangeStatus, ChangedFile, DisplayBuffer, TreeEntry};
use perch_github::{Anchor, CommentThread, Item, PullRequest};
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

/// How far along preparing a checkout is.
#[derive(Debug, Clone, PartialEq)]
pub enum Preparing {
    Idle,
    Working { message: String, percent: Option<u8> },
    Failed { message: String },
}

/// A composer that is open: where the question will be anchored and how
/// it will be run.
#[derive(Debug, Clone, PartialEq)]
pub struct Composer {
    /// `None` starts a thread; `Some` is a follow-up in that thread.
    pub thread_id: Option<String>,
    pub anchor: Option<Anchor>,
    pub model: String,
    pub effort: perch_agent::Effort,
    /// A question to start with, when a failed run is retried.
    pub draft: String,
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
    pub preparing: RwSignal<Preparing>,
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
    /// The editor's selection as display lines, first and last inclusive.
    pub selection: RwSignal<Option<(usize, usize)>>,
    /// A gutter marker was clicked: the thread to scroll the column to.
    pub focus_thread: RwSignal<Option<String>>,
    /// How wide the thread column is. Dragged.
    pub threads_width: RwSignal<f64>,
    /// Runs whose step list the reader opened.
    pub expanded_runs: RwSignal<std::collections::HashSet<String>>,
    /// The model and effort last sent, which the next composer starts on.
    pub agent_last: RwSignal<Option<(String, perch_agent::Effort)>>,
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
            stale_head: RwSignal::new(None),
            offline: RwSignal::new(false),
            thread_id: RwSignal::new(None),
            opening: RwSignal::new(false),
            preparing: RwSignal::new(Preparing::Idle),
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
            selection: RwSignal::new(None),
            focus_thread: RwSignal::new(None),
            threads_width: RwSignal::new(crate::theme::THREADS_WIDTH),
            expanded_runs: RwSignal::new(std::collections::HashSet::new()),
            agent_last: RwSignal::new(None),
            hovered_line: RwSignal::new(None),
            composer_focused: RwSignal::new(false),
            composer_draft: RwSignal::new(String::new()),
            composer_autofocus: RwSignal::new(true),
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

    /// Whether the review view should be on screen at all.
    pub fn is_showing(&self) -> bool {
        self.opening.get() || self.is_open()
    }

    /// Leave the review view and return to the inbox.
    pub fn close(&self) {
        self.opening.set(false);
        self.pull_request.set(None);
        self.item.set(None);
        self.ready.set(false);
        self.stale_head.set(None);
        self.offline.set(false);
        self.thread_id.set(None);
        self.preparing.set(Preparing::Idle);
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
        self.hovered_line.set(None);
        self.composer_focused.set(false);
        self.composer_draft.set(String::new());
        self.expanded.set(std::collections::HashSet::new());
        self.dismiss_hover();
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
        self.files.update(|m| {
            m.insert(path.to_string(), file);
        });
        if self.active.with(|a| a.as_deref() == Some(path)) {
            self.editor_epoch.update(|e| *e += 1);
        }
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
        let commit_sha = self.item.with(|i| i.as_ref().and_then(|i| i.checkout.as_ref()).map(|c| c.head_sha.clone()))?;
        Some(Anchor {
            path: file.path.clone(),
            start_line: start as u32 + 1,
            line: end as u32 + 1,
            side: perch_github::Side::Right,
            commit_sha,
        })
    }

    /// Open a composer, for a new thread on `anchor` or the worktree.
    pub fn open_composer(&self, thread_id: Option<String>, anchor: Option<Anchor>, draft: String, defaults: (String, perch_agent::Effort)) {
        let (model, effort) = self.agent_last.get().unwrap_or(defaults);
        self.composer_draft.set(draft.clone());
        self.composer.set(Some(Composer { thread_id, anchor, model, effort, draft }));
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
