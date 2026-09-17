//! What the review view holds about a pull request and its files.

use std::collections::HashMap;
use std::sync::Arc;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use perch_git::{ChangeStatus, ChangedFile, DisplayBuffer, TreeEntry};
use perch_github::PullRequest;
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
        }
    }

    pub fn is_open(&self) -> bool {
        self.pull_request.with(|p| p.is_some())
    }

    /// Whether the review view should be on screen at all.
    pub fn is_showing(&self) -> bool {
        self.opening.get() || self.is_open()
    }

    /// Leave the review view and return to the inbox.
    pub fn close(&self) {
        self.opening.set(false);
        self.pull_request.set(None);
        self.preparing.set(Preparing::Idle);
        self.changed_files.set(Arc::new(Vec::new()));
        self.tree.set(Arc::new(Vec::new()));
        self.tabs.set(Vec::new());
        self.active.set(None);
        self.files.set(HashMap::new());
        self.loading.set(HashMap::new());
        self.rail_mode.set(RailMode::Changed);
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
    pub fn status_of(&self, path: &str) -> Option<ChangeStatus> {
        self.changed_files
            .with(|files| files.iter().find(|f| f.path == path).map(|f| f.status))
    }
}
