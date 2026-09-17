//! Reactive app state.
//!
//! Every field is a signal, so `AppState` is `Copy` and can be handed to any
//! view closure without lifetime plumbing. Derived lists are plain functions
//! over these signals, recomputed by whichever effect reads them.

use std::collections::HashMap;
use std::sync::Arc;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use jiff::Timestamp;
use perch_github::{SubjectDetail, Thread};

use crate::backend::{Event, Status};
use crate::filter::{self, Counts, Filters, Group, ReasonFilter, Section, Sort};

#[derive(Clone, Copy)]
pub struct AppState {
    pub threads: RwSignal<Vec<Thread>>,
    pub details: RwSignal<HashMap<String, SubjectDetail>>,
    /// Encoded image bytes, shared so a clone is a refcount bump.
    pub avatars: RwSignal<HashMap<String, Arc<Vec<u8>>>>,

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
        }
    }

    /// Fold one backend event into the inbox or the review view.
    pub fn apply_with_review(&self, event: Event, review: crate::review::model::ReviewState) {
        use crate::review::model::Preparing;

        match event {
            Event::ReviewProgress { message, percent } => {
                review.preparing.set(Preparing::Working { message, percent });
            }
            Event::ReviewReady { pull_request, files, tree } => {
                review.preparing.set(Preparing::Idle);
                review.changed_files.set(files);
                review.tree.set(tree);
                review.pull_request.set(Some(pull_request));
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
                let still_present = self.repo_filter.with(|f| {
                    f.as_ref().map(|name| {
                        self.threads
                            .with(|ts| ts.iter().any(|t| &t.repository.full_name == name))
                    })
                });
                if still_present == Some(false) {
                    self.repo_filter.set(None);
                }
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
            reason: self.reason_filter.get(),
            repo: self.repo_filter.get(),
            unread_only: self.unread_only.get(),
            search: self.search.get(),
        }
    }

    /// Filtered and sorted threads.
    pub fn visible(&self) -> Vec<Thread> {
        self.threads
            .with(|all| filter::visible(all, &self.filters(), self.sort.get()))
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
        self.threads.with(|all| filter::counts(all))
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

    pub fn clear_filters(&self) {
        self.reason_filter.set(None);
        self.repo_filter.set(None);
        self.unread_only.set(false);
        self.search.set(String::new());
    }
}
