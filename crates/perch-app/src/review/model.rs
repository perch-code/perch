//! What the review view holds about a pull request and its files.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::text::Document;
use perch_git::{
    ChangeStatus, ChangedFile, Commit, Dirt, DisplayBuffer, LineStatus, Row, Scope, TreeEntry,
};
use perch_github::{
    Anchor, Checks, CommentThread, Discussion, Issue, IssueComment, Item, PullRequest,
    PullRequestOverview,
};
use perch_syntax::{Language, LineSpans};

use crate::review::doc::{self, Card, CardId, Doc, Reveals};

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
    /// The head blob, which the reveals are keyed by, as Viewed is: a new
    /// blob is new content and starts folded again. Empty for a file that
    /// has none: one outside the changed list, a deletion, or a working
    /// tree file git has not hashed. Such a file's reveals are kept for
    /// the window's life and never stored.
    pub blob: String,
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
    Working {
        message: String,
        percent: Option<u8>,
    },
    Failed {
        message: String,
    },
}

/// How a question runs: which agent, which of its models, and how hard
/// it thinks.
pub type AgentChoice = (perch_agent::Backend, String, perch_agent::Effort);

/// One effective reviewer captured by a launch. A disabled (`None`) row is
/// omitted from `ReviewLaunchRequest::reviewers`; it is a UI preference, not
/// an effort understood by the native CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewLaunchChoice {
    pub backend: perch_agent::Backend,
    pub model: String,
    pub effort: perch_agent::ReviewEffort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewLaunchRequest {
    pub item_id: String,
    pub base_ref: String,
    pub reviewers: Vec<ReviewLaunchChoice>,
}

/// The launcher's editable, durable preference for one backend. An
/// unavailable backend can retain its model/effort while `enabled` is false
/// in the effective draft shown by the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPreferenceChoice {
    pub backend: perch_agent::Backend,
    pub model: Option<String>,
    pub effort: Option<perch_agent::ReviewEffort>,
    pub enabled: bool,
}

/// Everything needed to draw one immutable review in Conversation/history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewGroupSummary {
    pub group: perch_github::ReviewGroup,
    pub snapshot: perch_github::ReviewSnapshot,
    pub attempts: Vec<perch_github::ReviewerAttempt>,
}

/// Durable finding state plus the compact provenance needed to identify it
/// in the ordinary conversation and Files thread surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFindingSummary {
    pub finding: perch_github::Finding,
    pub severity: perch_github::FindingSeverity,
    /// Every reviewer/model which has reported or verified this identity.
    pub attributions: Vec<(String, String)>,
    /// Review numbers containing an observation of this finding.
    pub seen_in_reviews: Vec<u64>,
    /// The latest observation's reviewer, retained for compact callers.
    pub backend: Option<String>,
    pub model: Option<String>,
    pub original_review_number: Option<u64>,
}

/// The last stable file context while one or more historical selections are
/// optimistic. A later click keeps this original baseline so failure of the
/// eventual winner can restore the view the reader actually came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoricalReviewRollback {
    pub navigation_id: Option<u64>,
    pub review: Option<ReviewGroupSummary>,
    pub request_id: Option<u64>,
    pub findings: Option<Arc<Vec<ReviewFindingSummary>>>,
}

impl ReviewFindingSummary {
    /// Resolved and explicitly linked findings stay in review history, but
    /// do not crowd the default list of actionable threads.
    pub fn is_actionable(&self) -> bool {
        self.finding.disposition == perch_github::FindingDisposition::Open
            && self.finding.verification != perch_github::FindingVerification::Fixed
            && self.finding.duplicate_of.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewFindingFilter {
    #[default]
    Open,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewLaunchDraft {
    pub item_id: String,
    pub base_ref: Option<String>,
    pub choices: Vec<ReviewPreferenceChoice>,
}

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

/// One editor's document, live: the handle the editor lays out from, which
/// a programmatic edit goes through, and the row table over it, which
/// everything that reads the editor's rows goes through. The table is a
/// signal so that a lookup after an edit sees the edited document rather
/// than a snapshot taken when the editor was built.
///
/// Both live with the editor's view: the signal is disposed when the
/// editor is torn down, so a reader that can run after that, on another
/// tab, reads it with the `try_` variants.
#[derive(Clone)]
pub struct LiveDoc {
    pub handle: Rc<dyn Document>,
    pub doc: RwSignal<Arc<Doc>>,
}

/// The documents the open editors hold, for the epoch they were built in.
/// A reader checks the epoch against `editor_epoch` first: an entry from an
/// editor since rebuilt is stale.
#[derive(Clone)]
pub struct LiveDocs {
    pub epoch: u64,
    pub layout: LiveLayout,
}

/// Which editors are open: the stacked one, or a split's two.
#[derive(Clone)]
pub enum LiveLayout {
    Stacked(LiveDoc),
    Split {
        #[cfg_attr(
            not(test),
            expect(
                dead_code,
                reason = "the view edits the base side through its own handle; only the tests read it here"
            )
        )]
        left: LiveDoc,
        right: LiveDoc,
    },
}

impl LiveLayout {
    /// The document the folds are counted from: the stacked one, or the
    /// head side of a split, whose dividers the base side mirrors.
    pub fn head(&self) -> &LiveDoc {
        match self {
            LiveLayout::Stacked(live) => live,
            LiveLayout::Split { right, .. } => right,
        }
    }
}

/// A write to the store's reveals, waiting for the backend. The state has
/// no handle to it, so a reveal queues one of these and the root view,
/// which has the handle, sends it on, as it sends a setting on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevealWrite {
    /// Head lines `first` to `last` of a file's content were opened.
    Reveal {
        path: String,
        blob: String,
        first: u32,
        last: u32,
    },
    /// Every reveal overlapping `first` to `last` is forgotten: a Reset.
    Reset {
        path: String,
        blob: String,
        first: u32,
        last: u32,
    },
}

/// The `meta` key that remembers Show whole for an item. Per item rather
/// than in the list the launch reads, because a reader folds one review
/// and not another; the backend reads it when the item opens.
pub fn folded_meta_key(item_id: &str) -> String {
    format!("review.folded.{item_id}")
}

/// What is written under the key: `1` folded, `0` whole.
pub fn folded_meta_value(folded: bool) -> &'static str {
    if folded {
        "1"
    } else {
        "0"
    }
}

/// What a value read from under the key means. Absent is folded: the
/// default, which only turning Show whole off ever writes over.
pub fn folded_from_meta(value: Option<&str>) -> bool {
    value.map(str::trim) != Some("0")
}

/// Everything the review view renders from. All fields are signals, so the
/// struct is `Copy` and can be handed to any view closure.
#[derive(Clone, Copy)]
pub struct ReviewState {
    /// `None` until a pull request is open: an issue and a bare branch
    /// have none.
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
    /// True from the click until the review view is left, so the progress
    /// card can show before the pull request itself has resolved.
    pub opening: RwSignal<bool>,
    pub phase: RwSignal<Phase>,
    pub preparing: RwSignal<Preparing>,
    /// The header's review launcher and the immutable history selection are
    /// view-local; durable preferences and groups live on `AppState`.
    pub review_launcher_open: RwSignal<bool>,
    pub review_launcher_draft: RwSignal<Option<ReviewLaunchDraft>>,
    pub review_launcher_opener: RwSignal<Option<floem::ViewId>>,
    pub historical_review: RwSignal<Option<ReviewGroupSummary>>,
    /// Identity of the in-flight or installed History request. A group id
    /// alone is insufficient when the same group is requested twice and
    /// its async results arrive out of order.
    pub historical_review_request: RwSignal<Option<u64>>,
    /// Finding state reconstructed at the selected group's cutoff. Current
    /// dashboard state remains in `AppState` and is restored on Back.
    pub historical_findings: RwSignal<Option<Arc<Vec<ReviewFindingSummary>>>>,
    historical_review_rollback: RwSignal<Option<HistoricalReviewRollback>>,
    /// Conversation or Files. Remembered per item in `tabs_by_item` for
    /// the life of the app.
    pub tab: RwSignal<ItemTab>,
    pub tabs_by_item: RwSignal<HashMap<String, ItemTab>>,
    /// Stacked or Split, remembered across items in `meta`.
    pub layout: RwSignal<Layout>,
    /// What the diff is over. Per item; Branch when the item opens.
    pub scope: RwSignal<Scope>,
    /// Identity of the backend file context currently rendered. File and
    /// metadata events must name this exact generation as well as the item
    /// and navigation request before they can update the view.
    pub scope_gen: RwSignal<Option<u64>>,
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
    /// Live GitHub mergeability, permissions, enabled merge methods, and
    /// detailed checks for the pull request's exact head. Unlike `checks`,
    /// this is never restored from disk: merge controls require a fresh
    /// answer from GitHub.
    pub pull_request_overview: RwSignal<Option<Arc<PullRequestOverview>>>,
    pub pull_request_overview_loading: RwSignal<bool>,
    pub pull_request_overview_error: RwSignal<Option<String>>,
    /// Latest live status cycle accepted for the open pull request. Merge UI
    /// uses this to discard consent captured against an older rules snapshot.
    pub pull_request_status_generation: RwSignal<u64>,
    /// State of the deliberate, confirmed merge mutation.
    pub merge_pending: RwSignal<bool>,
    pub merge_error: RwSignal<Option<String>>,
    /// Latest merge mutation observed for the displayed checkout. Every event
    /// also carries the item and pull-request identity, so checkout resets can
    /// clear this without letting a different subject settle the new view.
    pub merge_attempt_id: RwSignal<Option<u64>>,
    /// The (path, blob) pairs the reader has checked as viewed.
    pub viewed: RwSignal<Arc<std::collections::HashSet<(String, String)>>>,
    /// Whether the worktree's changed files are folded to their changes,
    /// which they are unless the reader asked for them whole. Per
    /// worktree: a reader folds one review and not another.
    pub folded: RwSignal<bool>,
    /// The runs the reader asked to see, by (path, blob), in head lines,
    /// so a reveal made in one layout holds in the other. Loaded with the
    /// viewed set when the item opens; a file with no blob keeps its
    /// reveals here under `(path, "")` and nowhere else.
    pub reveals: RwSignal<HashMap<(String, String), Reveals>>,
    /// Reveals and resets not yet sent to the store, drained by the root
    /// view, which holds the backend handle.
    pub pending_reveals: RwSignal<Vec<RevealWrite>>,
    /// The references panel's content, when it is open.
    pub references: RwSignal<Option<Arc<References>>>,
    /// The panel's height. Dragged.
    pub references_height: RwSignal<f64>,
    pub changed_files: RwSignal<Arc<Vec<ChangedFile>>>,
    pub tree: RwSignal<Arc<Vec<TreeEntry>>>,
    /// Whether the rail's tree is cut down to the changed files. On by
    /// default: the review is of the changes, and the whole worktree is
    /// a click away.
    pub changed_only: RwSignal<bool>,
    pub tabs: RwSignal<Vec<Tab>>,
    pub active: RwSignal<Option<String>>,
    pub files: RwSignal<HashMap<String, Arc<LoadedFile>>>,
    /// Paths whose load is in flight, so a tab can show a spinner and a
    /// second click does not queue the work twice.
    pub loading: RwSignal<HashMap<String, ()>>,
    /// Display line the cursor sits on, for the status line.
    pub cursor_line: RwSignal<usize>,
    /// Bumped whenever the active file's content changes, to rebuild the
    /// editor view: another file is another editor.
    pub editor_epoch: RwSignal<u64>,
    /// The documents the open editors hold, set by the view that builds
    /// them and `None` while no editor is open. What the editor lays out
    /// is edited in place through these when the document changes under
    /// it, rather than the editor being rebuilt.
    pub docs: RwSignal<Option<LiveDocs>>,

    /// Every thread in the open worktree, with messages and runs.
    pub comment_threads: RwSignal<Arc<Vec<CommentThread>>>,
    /// Finding threads hidden from ordinary Conversation/Files rendering.
    /// Their durable threads remain loaded and are revealed by the All filter.
    pub hidden_review_threads: RwSignal<HashSet<String>>,
    pub review_finding_filter: RwSignal<ReviewFindingFilter>,
    /// An open composer, and where it is anchored.
    pub composer: RwSignal<Option<Composer>>,
    /// How many composers have been opened, for `Composer::opening`.
    pub composer_opened: RwSignal<u64>,
    /// The anchor of a question sent from the composer's card in the
    /// code whose thread has not arrived yet. The card's rows stay under
    /// the anchor, blank, until the next thread list brings the thread
    /// and its card takes their place, so the code does not close over
    /// the rows and open again a moment later.
    pub sent_composer: RwSignal<Option<Anchor>>,
    /// The editor's selection as display lines, first and last inclusive.
    pub selection: RwSignal<Option<(usize, usize)>>,
    /// A gutter marker was clicked: the inline card to scroll to and flash.
    /// Left as it was set rather than taken so a second click on the same
    /// marker is still a second flash, while a card built after the click
    /// ignores what it finds here.
    pub focus_card: RwSignal<Option<CardId>>,
    /// A thread whose Delete is waiting for a yes, under its byline.
    pub confirm_thread: RwSignal<Option<String>>,
    /// Runs whose step list the reader opened.
    pub expanded_runs: RwSignal<std::collections::HashSet<String>>,
    /// The rows each card has now: the estimate until the card has been
    /// laid out, and what the layout said after. Empty for a card whose
    /// rows are the estimate.
    pub card_rows: RwSignal<HashMap<CardId, usize>>,
    /// The cards drawn as one row. Kept for the app's life and not per
    /// item, as `expanded_runs` is: a card's ids are unique across items.
    pub collapsed_cards: RwSignal<HashSet<CardId>>,
    /// Every card that has arrived since launch. A thread's card starts
    /// collapsed when the thread is outdated, and that default is applied
    /// once, when the thread first arrives; after that the reader's own
    /// choice stands, through closing the item and opening it again.
    pub known_cards: RwSignal<HashSet<CardId>>,
    /// The backend, model and effort last sent, which the next composer
    /// starts on.
    pub agent_last: RwSignal<Option<AgentChoice>>,
    /// The display line the pointer is over in the gutter, for the `+`,
    /// or the row of a divider, for its controls.
    pub hovered_line: RwSignal<Option<usize>>,
    /// The element that recorded `hovered_line`. A divider's row is
    /// several elements, its gutter row and the parts of the bar over
    /// the code, and floem delivers the enter to the next before the
    /// leave from the last; a leave clears the line only when it comes
    /// from the element that set it, so a move between the parts of one
    /// row keeps its controls.
    pub hovered_by: RwSignal<Option<floem::ElementId>>,
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
    /// Directories open in the rail's tree. A checkout has thousands of
    /// entries, so only the way to each changed file starts open.
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
            opening: RwSignal::new(false),
            phase: RwSignal::new(Phase::Preparing),
            preparing: RwSignal::new(Preparing::Idle),
            review_launcher_open: RwSignal::new(false),
            review_launcher_draft: RwSignal::new(None),
            review_launcher_opener: RwSignal::new(None),
            historical_review: RwSignal::new(None),
            historical_review_request: RwSignal::new(None),
            historical_findings: RwSignal::new(None),
            historical_review_rollback: RwSignal::new(None),
            tab: RwSignal::new(ItemTab::Conversation),
            tabs_by_item: RwSignal::new(HashMap::new()),
            layout: RwSignal::new(Layout::Stacked),
            scope: RwSignal::new(Scope::Branch),
            scope_gen: RwSignal::new(None),
            commits: RwSignal::new(Arc::new(Vec::new())),
            dirty: RwSignal::new(Dirt::default()),
            since_head: RwSignal::new(None),
            discussion: RwSignal::new(None),
            checks: RwSignal::new(None),
            pull_request_overview: RwSignal::new(None),
            pull_request_overview_loading: RwSignal::new(false),
            pull_request_overview_error: RwSignal::new(None),
            pull_request_status_generation: RwSignal::new(0),
            merge_pending: RwSignal::new(false),
            merge_error: RwSignal::new(None),
            merge_attempt_id: RwSignal::new(None),
            viewed: RwSignal::new(Arc::new(std::collections::HashSet::new())),
            folded: RwSignal::new(true),
            reveals: RwSignal::new(HashMap::new()),
            pending_reveals: RwSignal::new(Vec::new()),
            references: RwSignal::new(None),
            references_height: RwSignal::new(crate::theme::REFERENCES_HEIGHT),
            changed_files: RwSignal::new(Arc::new(Vec::new())),
            tree: RwSignal::new(Arc::new(Vec::new())),
            changed_only: RwSignal::new(true),
            tabs: RwSignal::new(Vec::new()),
            active: RwSignal::new(None),
            files: RwSignal::new(HashMap::new()),
            loading: RwSignal::new(HashMap::new()),
            cursor_line: RwSignal::new(0),
            editor_epoch: RwSignal::new(0),
            docs: RwSignal::new(None),
            comment_threads: RwSignal::new(Arc::new(Vec::new())),
            hidden_review_threads: RwSignal::new(HashSet::new()),
            review_finding_filter: RwSignal::new(ReviewFindingFilter::Open),
            composer: RwSignal::new(None),
            composer_opened: RwSignal::new(0),
            sent_composer: RwSignal::new(None),
            selection: RwSignal::new(None),
            focus_card: RwSignal::new(None),
            confirm_thread: RwSignal::new(None),
            expanded_runs: RwSignal::new(std::collections::HashSet::new()),
            card_rows: RwSignal::new(HashMap::new()),
            collapsed_cards: RwSignal::new(HashSet::new()),
            known_cards: RwSignal::new(HashSet::new()),
            agent_last: RwSignal::new(None),
            hovered_line: RwSignal::new(None),
            hovered_by: RwSignal::new(None),
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
        self.preparing.set(Preparing::Working {
            message: message.into(),
            percent: None,
        });
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
        self.review_launcher_open.set(false);
        self.review_launcher_draft.set(None);
        self.review_launcher_opener.set(None);
        self.clear_historical_review();
        self.tab.set(ItemTab::Conversation);
        self.scope.set(Scope::Branch);
        self.scope_gen.set(None);
        self.commits.set(Arc::new(Vec::new()));
        self.dirty.set(Dirt::default());
        self.since_head.set(None);
        self.discussion.set(None);
        self.checks.set(None);
        self.pull_request_overview.set(None);
        self.pull_request_overview_loading.set(false);
        self.pull_request_overview_error.set(None);
        self.pull_request_status_generation.set(0);
        self.merge_pending.set(false);
        self.merge_error.set(None);
        self.merge_attempt_id.set(None);
        self.viewed.set(Arc::new(std::collections::HashSet::new()));
        // Folded until the open's ViewedReady says what the item has. Only
        // written when they change: the open editor rebuilds its document
        // on either, and it is about to go.
        if !self.folded.get_untracked() {
            self.folded.set(true);
        }
        if !self.reveals.with_untracked(HashMap::is_empty) {
            self.reveals.set(HashMap::new());
        }
        self.pending_reveals.set(Vec::new());
        self.references.set(None);
        self.changed_files.set(Arc::new(Vec::new()));
        self.tree.set(Arc::new(Vec::new()));
        self.tabs.set(Vec::new());
        self.active.set(None);
        self.files.set(HashMap::new());
        self.loading.set(HashMap::new());
        self.changed_only.set(true);
        self.lsp_status.set(perch_lsp::Status::Absent);
        self.comment_threads.set(Arc::new(Vec::new()));
        self.hidden_review_threads.set(HashSet::new());
        self.review_finding_filter.set(ReviewFindingFilter::Open);
        self.composer.set(None);
        self.sent_composer.set(None);
        self.selection.set(None);
        self.focus_card.set(None);
        self.confirm_thread.set(None);
        // The rows are the next item's cards' to fill; whether a card is
        // open is the reader's and stays, as `collapsed_cards` says.
        self.card_rows.set(HashMap::new());
        self.hovered_line.set(None);
        self.hovered_by.set(None);
        self.composer_focused.set(false);
        self.composer_draft.set(String::new());
        self.expanded.set(std::collections::HashSet::new());
        self.editor_epoch.update(|e| *e += 1);
        self.docs.set(None);
        self.dismiss_hover();
    }

    /// Prepare to install an accepted live result. Agent history navigation
    /// keeps the prior owner visible while its new owner is resolving, so a
    /// cross-item result must clear that owner's checkout at commit time.
    pub(crate) fn prepare_ready_for(&self, item_id: &str) {
        if self
            .item
            .with_untracked(|item| item.as_ref().is_some_and(|item| item.id != item_id))
        {
            self.reset_checkout();
        } else {
            // A fresh live checkout always gets a fresh language server. Its
            // status event follows Ready and names the same navigation.
            self.lsp_status.set(perch_lsp::Status::Absent);
        }
    }

    /// Optimistically select an immutable review while retaining the last
    /// stable live or historical view. Repeated clicks do not replace the
    /// original rollback point with another optimistic selection.
    pub fn begin_historical_review(
        &self,
        navigation_id: Option<u64>,
        summary: ReviewGroupSummary,
        request_id: u64,
    ) {
        self.begin_historical_review_rollback(navigation_id);
        self.historical_review.set(Some(summary));
        self.historical_review_request.set(Some(request_id));
        self.historical_findings.set(None);
    }

    /// Capture the stable view before a history target whose summary has not
    /// been resolved yet, as with navigation from a reviewer child.
    pub(crate) fn begin_historical_review_rollback(&self, navigation_id: Option<u64>) {
        if self
            .historical_review_rollback
            .with_untracked(Option::is_none)
        {
            self.historical_review_rollback
                .set(Some(HistoricalReviewRollback {
                    navigation_id,
                    review: self.historical_review.get_untracked(),
                    request_id: self.historical_review_request.get_untracked(),
                    findings: self.historical_findings.get_untracked(),
                }));
        }
    }

    /// The selected request is now the stable baseline for any later switch.
    pub fn finish_historical_review(&self, findings: Arc<Vec<ReviewFindingSummary>>) {
        self.historical_findings.set(Some(findings));
        self.historical_review_rollback.set(None);
    }

    /// Restore the stable view captured by the first optimistic selection.
    /// With no captured baseline, failure falls back to the live view.
    pub(crate) fn rollback_historical_review(&self) -> Option<HistoricalReviewRollback> {
        let rollback = self.historical_review_rollback.get_untracked();
        if let Some(rollback) = rollback.as_ref() {
            self.historical_review.set(rollback.review.clone());
            self.historical_review_request.set(rollback.request_id);
            self.historical_findings.set(rollback.findings.clone());
        } else {
            self.historical_review.set(None);
            self.historical_review_request.set(None);
            self.historical_findings.set(None);
        }
        self.historical_review_rollback.set(None);
        rollback
    }

    /// Return to a live/current context and discard any optimistic rollback.
    pub fn clear_historical_review(&self) {
        self.historical_review.set(None);
        self.historical_review_request.set(None);
        self.historical_findings.set(None);
        self.historical_review_rollback.set(None);
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
            .with(|i| {
                i.as_ref()
                    .and_then(|i| self.tabs_by_item.with(|m| m.get(&i.id).copied()))
            })
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

    /// Leave the review view for the sidebar's empty main area.
    pub fn close(&self) {
        self.reset_checkout();
        self.opening.set(false);
        self.item.set(None);
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
                tabs.push(Tab {
                    path: path.to_string(),
                });
                // Oldest first, so the cap drops the least recently opened.
                if tabs.len() > MAX_TABS {
                    tabs.remove(0);
                }
            });
        }
        self.active.set(Some(path.to_string()));
        self.cursor_line.set(0);
        self.editor_epoch.update(|e| *e += 1);
        self.keep_composer_in_view();

        let needs_load = self.file(path).is_none() && !self.is_loading(path);
        if needs_load {
            self.loading.update(|m| {
                m.insert(path.to_string(), ());
            });
        }
        needs_load
    }

    /// The open file changed under an open composer. One drawn in the code
    /// of the file that was open would now be hidden, so show Conversation,
    /// where new questions and follow-ups always have a home.
    fn keep_composer_in_view(&self) {
        let hidden = floem::reactive::Effect::untrack(|| {
            self.composer.with(Option::is_some) && !self.composer_in_code()
        });
        if hidden && self.tab.get_untracked() != ItemTab::Conversation {
            self.show_tab(ItemTab::Conversation);
        }
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
            self.keep_composer_in_view();
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

    /// A new changed list, from an open, a scope change, or the poll
    /// finding the head moved.
    ///
    /// Kept in the tree's order rather than git's, which puts a file
    /// beside a directory before the directory's contents: `}` and `{`
    /// walk this list, and the rail shows the tree, so the two have to
    /// agree on what comes next.
    ///
    /// The path to each file the list did not have before is opened in
    /// the tree, so the filtered rail shows every changed file rather
    /// than a row of shut directories, and the whole tree, when the
    /// reader turns to it, is open at the changes. Only the new ones: a
    /// poll that rereads the same list must not spring open what the
    /// reader shut.
    pub fn set_changed_files(&self, files: Arc<Vec<ChangedFile>>) {
        let mut sorted = files.as_ref().clone();
        sorted.sort_by(|a, b| perch_git::compare_paths(&a.path, false, &b.path, false));
        let known: std::collections::HashSet<String> = self
            .changed_files
            .with_untracked(|old| old.iter().map(|f| f.path.clone()).collect());
        let new: Vec<&str> = sorted
            .iter()
            .map(|f| f.path.as_str())
            .filter(|p| !known.contains(*p))
            .collect();
        if !new.is_empty() {
            self.expanded.update(|expanded| {
                for path in new {
                    crate::review::tree::reveal(expanded, path);
                }
            });
        }
        self.changed_files.set(Arc::new(sorted));
    }

    /// The scope changed: every loaded file was diffed under the old one.
    /// Forget them all and reload the open tabs, active one first, so the
    /// editor repaints from the new sides rather than the stale ones.
    /// Returns the paths to ask the backend for.
    pub fn change_scope(&self, scope: Scope, files: Arc<Vec<ChangedFile>>) -> Vec<String> {
        self.scope.set(scope);
        self.set_changed_files(files);
        // Every loaded file, not only the tabs: a thread card on
        // Conversation shows lines from a file no tab has, and would go
        // blank until something loaded it again.
        let mut paths: Vec<String> = self.files.with(|m| m.keys().cloned().collect());
        self.files.set(HashMap::new());
        self.loading.set(HashMap::new());
        for tab in self
            .tabs
            .with(|t| t.iter().map(|t| t.path.clone()).collect::<Vec<_>>())
        {
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
            Some(blob) => self
                .viewed
                .with(|v| v.contains(&(file.path.clone(), blob.clone()))),
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
                    let base =
                        start
                            .map(|s| s as isize)
                            .unwrap_or(if forward { -1 } else { n as isize });
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

    /// The head lines a file's current threads are anchored on, for the folds to
    /// keep in view, and the composer card's while it has one: the thread
    /// being written keeps its lines in view as a written one does, so a
    /// run folded again cannot close over the card. Tracked, so a document
    /// built from them follows the threads, the discussion and the
    /// composer.
    pub fn anchors_in(&self, path: &str) -> Vec<(usize, usize)> {
        let file = self.files.with(|files| files.get(path).cloned());
        let mut out = self.comment_threads.with(|threads| {
            let visible = threads
                .iter()
                .filter(|thread| !thread.outdated && self.thread_visible(&thread.id))
                .cloned()
                .collect::<Vec<_>>();
            let mut out = self
                .discussion
                .with(|d| anchors_of(&visible, d.as_deref(), path));
            if let Some(file) = file.as_deref() {
                out.extend(
                    visible
                        .iter()
                        .filter_map(|thread| thread.anchor.as_ref())
                        .filter(|anchor| {
                            anchor.path == path && anchor.side == perch_github::Side::Left
                        })
                        .flat_map(|anchor| aligned_head_ranges(file, anchor)),
                );
            }
            out
        });
        if let Some(a) = self.composer_card_anchor(path) {
            match a.side {
                perch_github::Side::Right => {
                    let (first, last) = (a.start_line.min(a.line), a.start_line.max(a.line));
                    out.push((
                        first.saturating_sub(1) as usize,
                        last.saturating_sub(1) as usize,
                    ));
                }
                perch_github::Side::Left => {
                    if let Some(file) = file.as_deref() {
                        out.extend(aligned_head_ranges(file, &a));
                    }
                }
            }
        }
        out
    }

    /// The cards of a file: the reader's current threads anchored in it, GitHub's
    /// review threads on it, and the composer when it is a card in it,
    /// each with the rows it has now. Tracked on the threads, the
    /// discussion, the composer, the rows and which cards are collapsed,
    /// so a document built from them follows them all; the file itself is
    /// read as it is, since a file's content is fixed for the editor
    /// built over it.
    pub fn cards_in(&self, path: &str) -> Vec<Card> {
        let Some(file) = self.files.with_untracked(|m| m.get(path).cloned()) else {
            return Vec::new();
        };
        let github = self.github_threads_in(path);
        let composer = self.composer_card_anchor(path);
        self.comment_threads.with(|threads| {
            let visible = threads
                .iter()
                .filter(|thread| !thread.outdated && self.thread_visible(&thread.id))
                .cloned()
                .collect::<Vec<_>>();
            self.collapsed_cards.with(|collapsed| {
                self.card_rows.with(|rows| {
                    cards_of(&visible, &github, composer.as_ref(), &file, collapsed, rows)
                })
            })
        })
    }

    /// The anchor the composer's card in a file sits under, if it has
    /// one there: the open composer's, when it starts a thread on lines
    /// of the file, or else the one a sent question's card keeps until
    /// its thread arrives. Tracked on both.
    pub fn composer_card_anchor(&self, path: &str) -> Option<Anchor> {
        self.composer
            .with(|c| {
                c.as_ref()
                    .filter(|c| c.thread_id.is_none())
                    .and_then(|c| c.anchor.clone())
            })
            .or_else(|| self.sent_composer.get())
            .filter(|a| a.path == path)
    }

    /// Whether the open composer is drawn as a card in the code: it starts
    /// a thread on lines of the active file, which is loaded and has the
    /// lines. Tracked.
    pub fn composer_is_inline(&self) -> bool {
        let Some(file) = self.active_file() else {
            return false;
        };
        self.composer.with(|c| {
            c.as_ref()
                .filter(|c| c.thread_id.is_none())
                .and_then(|c| c.anchor.as_ref())
                .is_some_and(|a| a.path == file.path && card_line(&file, a).is_some())
        })
    }

    /// Whether a follow-up on a thread is drawn inside the thread's card
    /// in the code: the thread has a card in the active file, which is
    /// loaded and has its lines. Tracked.
    pub fn follow_up_is_inline(&self, thread_id: &str) -> bool {
        let Some(file) = self.active_file() else {
            return false;
        };
        self.comment_threads.with(|threads| {
            threads
                .iter()
                .find(|t| t.id == thread_id && !t.outdated && self.thread_visible(&t.id))
                .and_then(|t| t.anchor.as_ref())
                .is_some_and(|a| a.path == file.path && card_line(&file, a).is_some())
        })
    }

    /// Whether the open composer is drawn in the code: a new thread's as a
    /// card under its lines, a follow-up's inside its thread's card. Tracked.
    pub fn composer_in_code(&self) -> bool {
        match self
            .composer
            .with(|c| c.as_ref().map(|c| c.thread_id.clone()))
        {
            Some(Some(thread_id)) => self.follow_up_is_inline(&thread_id),
            Some(None) => self.composer_is_inline(),
            None => false,
        }
    }

    /// Open a card drawn as one row, so what is inside it can be seen.
    /// Nothing is written for a card that is open already.
    pub fn expand_card(&self, card: &CardId) {
        if self.collapsed_cards.with_untracked(|c| c.contains(card)) {
            self.collapsed_cards.update(|c| {
                c.remove(card);
            });
        }
    }

    /// Whether a card is drawn as one row.
    pub fn is_collapsed(&self, card: &CardId) -> bool {
        self.collapsed_cards.with(|c| c.contains(card))
    }

    /// A card has been laid out, and its height takes `rows`: what the
    /// document built from the cards gives it from here on, in place of
    /// the estimate. Nothing is written when that is what it already
    /// says, since the write rebuilds the open file's document.
    pub fn set_card_rows(&self, card: &CardId, rows: usize) {
        if self
            .card_rows
            .with_untracked(|r| r.get(card) == Some(&rows))
        {
            return;
        }
        self.card_rows.update(|r| {
            r.insert(card.clone(), rows);
        });
    }

    /// The chevron was clicked: a collapsed card opens, an open one
    /// collapses.
    pub fn toggle_card(&self, card: &CardId) {
        self.collapsed_cards.update(|c| {
            if !c.remove(card) {
                c.insert(card.clone());
            }
        });
    }

    /// The worktree's threads, as the backend sent them. A thread arriving
    /// for the first time since launch starts its card collapsed when it
    /// is outdated, and open otherwise; one seen before keeps whatever the
    /// reader left it as. The cards are settled before the threads are
    /// written, so the document built from the threads is built once.
    ///
    /// A sent question's card gives up its rows here, in the same pass
    /// as the threads land, so the thread's card takes the rows in the
    /// one edit rather than the code closing over them and opening
    /// again.
    pub fn set_comment_threads(&self, threads: Arc<Vec<CommentThread>>) {
        let new: Vec<(CardId, bool)> = self.known_cards.with_untracked(|known| {
            threads
                .iter()
                .filter(|t| t.anchor.is_some())
                .map(|t| (CardId::Thread(t.id.clone()), t.outdated))
                .filter(|(id, _)| !known.contains(id))
                .collect()
        });
        if !new.is_empty() {
            let collapsed: Vec<CardId> = new
                .iter()
                .filter(|(_, outdated)| *outdated)
                .map(|(id, _)| id.clone())
                .collect();
            if !collapsed.is_empty() {
                self.collapsed_cards.update(|c| c.extend(collapsed));
            }
            self.known_cards
                .update(|k| k.extend(new.into_iter().map(|(id, _)| id)));
        }
        floem::reactive::Effect::batch(|| {
            if self.sent_composer.with_untracked(Option::is_some) {
                self.sent_composer.set(None);
                self.forget_composer_rows();
            }
            self.comment_threads.set(threads);
        });
    }

    /// The composer's laid-out rows are the card's that was, and the next
    /// composer starts on the estimate.
    fn forget_composer_rows(&self) {
        if self
            .card_rows
            .with_untracked(|r| r.contains_key(&CardId::Composer))
        {
            self.card_rows.update(|r| {
                r.remove(&CardId::Composer);
            });
        }
    }

    /// The reveals for a file's content, tracked, so a document built from
    /// them is rebuilt when they change.
    pub fn reveals_for(&self, path: &str, blob: &str) -> Reveals {
        self.reveals.with(|m| {
            m.get(&(path.to_string(), blob.to_string()))
                .cloned()
                .unwrap_or_default()
        })
    }

    /// Show head lines `first` to `last` of a file, inclusive, whatever
    /// fold they are in, and remember it for the file's content.
    pub fn reveal(&self, path: &str, blob: &str, first: usize, last: usize) {
        self.reveals.update(|m| {
            m.entry((path.to_string(), blob.to_string()))
                .or_default()
                .add(first, last);
        });
        self.queue_reveal_write(
            blob,
            RevealWrite::Reveal {
                path: path.to_string(),
                blob: blob.to_string(),
                first: first as u32,
                last: last as u32,
            },
        );
    }

    /// Fold runs back to their default: forget every reveal overlapping
    /// each of `ranges`, the head lines of a run as the folds have it by
    /// default, here and in the store. A divider's Reset gives its own
    /// run's and those of the runs it carries; see `doc::reset_ranges`.
    pub fn reset(&self, path: &str, blob: &str, ranges: &[(usize, usize)]) {
        self.reveals.update(|m| {
            if let Some(reveals) = m.get_mut(&(path.to_string(), blob.to_string())) {
                for &(first, last) in ranges {
                    reveals.remove_overlapping(first, last);
                }
                if reveals.is_empty() {
                    m.remove(&(path.to_string(), blob.to_string()));
                }
            }
        });
        for &(first, last) in ranges {
            self.queue_reveal_write(
                blob,
                RevealWrite::Reset {
                    path: path.to_string(),
                    blob: blob.to_string(),
                    first: first as u32,
                    last: last as u32,
                },
            );
        }
    }

    /// Hand a write to the root view for the backend. A file with no blob
    /// has nothing to key a row by, so its reveals stay in memory.
    fn queue_reveal_write(&self, blob: &str, write: RevealWrite) {
        if blob.is_empty() {
            return;
        }
        self.pending_reveals.update(|w| w.push(write));
    }

    /// How many lines the open file's dividers hide between them, from the
    /// document the editor is laying out: the stacked one, or the head
    /// side of a split. Nothing when no editor is open, when the file is
    /// shown whole, or when the entry is an editor's since rebuilt.
    pub fn hidden_lines(&self) -> usize {
        self.docs
            .with(|docs| {
                let docs = docs
                    .as_ref()
                    .filter(|d| d.epoch == self.editor_epoch.get())?;
                // The document's signal lives with its editor; see `LiveDoc`.
                docs.layout
                    .head()
                    .doc
                    .try_with(|doc| doc.map(|d| d.hidden_lines()))
            })
            .unwrap_or(0)
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

    /// The pointer came onto `row`, on the element `by`.
    pub fn enter_row(&self, by: floem::ElementId, row: usize) {
        self.hovered_line.set(Some(row));
        self.hovered_by.set(Some(by));
    }

    /// The pointer left the element `by`. The row goes when `by` is the
    /// element that recorded it, and stays when another has since: see
    /// `hovered_by`.
    pub fn leave_row(&self, by: floem::ElementId) {
        if self.hovered_by.get_untracked() == Some(by) {
            self.hovered_line.set(None);
            self.hovered_by.set(None);
        }
    }

    /// The head lines under the editor's selection, as an anchor. `None`
    /// when nothing is selected or the selection is all removed lines.
    pub fn anchor_from_selection(&self) -> Option<Anchor> {
        let (first, last) = self.selection.get()?;
        let file = self.active_file()?;
        self.anchor_for_display_range(&file, first, last)
    }

    /// An anchor for a display range in a file.
    pub fn anchor_for_display_range(
        &self,
        file: &LoadedFile,
        first: usize,
        last: usize,
    ) -> Option<Anchor> {
        let (first, last) = (first.min(last), first.max(last));
        let mut heads = (first..=last).filter_map(|d| file.buffer.head_line(d));
        let start = heads.next()?;
        let end = heads.next_back().unwrap_or(start);
        // The commit the lines are of. Under a commit scope that is the
        // commit, not the branch head: the numbers are its, and the quote
        // and the outdated check read from it.
        let head = self.item.with(|i| {
            i.as_ref()
                .and_then(|i| i.checkout.as_ref())
                .map(|c| c.head_sha.clone())
        })?;
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

    /// Open a composer, for a new thread on `anchor` or the item as a whole,
    /// or a follow-up in `thread_id`.
    ///
    /// A composer on lines of the open file is a card under them, and a
    /// follow-up on a thread with a card in the open file sits inside that
    /// card. Anything that cannot be drawn inline moves to Conversation,
    /// where every item-level thread is available. A card the follow-up
    /// sits inside is opened if it was collapsed, so the box is seen. A
    /// sent question's card gives way to the new composer, since there is
    /// one composer card at a time, and whatever rows the last composer's
    /// card had are forgotten: they were that card's, and this one starts
    /// on the estimate.
    pub fn open_composer(
        &self,
        thread_id: Option<String>,
        anchor: Option<Anchor>,
        draft: String,
        defaults: AgentChoice,
    ) {
        let (backend, model, effort) = self.agent_last.get().unwrap_or(defaults);
        self.composer_draft.set(draft.clone());
        let opening = self.composer_opened.get_untracked() + 1;
        self.composer_opened.set(opening);
        floem::reactive::Effect::batch(|| {
            if self.sent_composer.with_untracked(Option::is_some) {
                self.sent_composer.set(None);
            }
            self.forget_composer_rows();
            self.composer.set(Some(Composer {
                thread_id,
                anchor,
                backend,
                model,
                effort,
                draft,
                opening,
            }));
        });
        // A caller can be an effect; do not make it depend on the file or
        // composer merely because placement was checked here.
        let in_code = floem::reactive::Effect::untrack(|| self.composer_in_code());
        if !in_code && self.tab.get_untracked() != ItemTab::Conversation {
            self.show_tab(ItemTab::Conversation);
        } else if let Some(id) = self
            .composer
            .with_untracked(|c| c.as_ref().and_then(|c| c.thread_id.clone()))
        {
            self.expand_card(&CardId::Thread(id));
        }
    }

    /// Close the composer and forget its draft. A composer card's rows go
    /// with it.
    pub fn close_composer(&self) {
        floem::reactive::Effect::batch(|| {
            self.composer.set(None);
            self.forget_composer_rows();
        });
        self.composer_draft.set(String::new());
        // The box goes with the composer and is never told it lost focus,
        // so the flag that stands the keys down is cleared here.
        self.composer_focused.set(false);
    }

    /// The composer's question went to the backend, and the composer is
    /// closed. A composer that was a card in the code leaves its rows
    /// where they are, under its anchor, for the thread's card to take
    /// when the thread arrives; see `sent_composer`.
    pub fn composer_sent(&self) {
        let card = floem::reactive::Effect::untrack(|| self.composer_is_inline())
            .then(|| {
                self.composer
                    .with_untracked(|c| c.as_ref().and_then(|c| c.anchor.clone()))
            })
            .flatten();
        floem::reactive::Effect::batch(|| {
            self.composer.set(None);
            match card {
                Some(anchor) => self.sent_composer.set(Some(anchor)),
                None => self.forget_composer_rows(),
            }
        });
        self.composer_draft.set(String::new());
        self.composer_focused.set(false);
    }

    /// The backend complained about something, which may be the send
    /// whose thread a sent question's card is waiting for. The card's rows
    /// are given up, so a failed send does not leave blank rows under the
    /// lines; a thread that does arrive after all brings its own card.
    pub fn drop_sent_placeholder(&self) {
        if self.sent_composer.with_untracked(Option::is_none) {
            return;
        }
        floem::reactive::Effect::batch(|| {
            self.sent_composer.set(None);
            self.forget_composer_rows();
        });
    }

    /// The thread whose anchor starts on a display line of the open file.
    /// A left-side anchor starts on its base row: an exact ghost when the
    /// line was changed or removed, otherwise the head row paired with it.
    pub fn thread_starting_at(
        &self,
        file: &LoadedFile,
        display_line: usize,
    ) -> Option<CommentThread> {
        self.comment_threads.with(|t| {
            t.iter()
                .filter(|thread| !thread.outdated && self.thread_visible(&thread.id))
                .find(|t| {
                    t.anchor.as_ref().is_some_and(|a| {
                        a.path == file.path
                            && display_line_of_anchor_line(file, a.side, a.start_line)
                                == Some(display_line)
                    })
                })
                .cloned()
        })
    }

    /// The current thread whose anchor starts on a zero-based line of one
    /// particular side. Split gutters use this so a paired row does not show
    /// a left-side marker in the head gutter, or vice versa.
    pub fn thread_starting_on(
        &self,
        file: &LoadedFile,
        side: perch_github::Side,
        line: usize,
    ) -> Option<CommentThread> {
        self.comment_threads.with(|threads| {
            threads
                .iter()
                .filter(|thread| !thread.outdated && self.thread_visible(&thread.id))
                .find(|thread| {
                    thread.anchor.as_ref().is_some_and(|anchor| {
                        anchor.path == file.path
                            && anchor.side == side
                            && anchor.start_line.checked_sub(1).map(|line| line as usize)
                                == Some(line)
                    })
                })
                .cloned()
        })
    }

    /// GitHub's review comments on a file, each with its replies, in time
    /// order. Only those on the head side with a line GitHub still places:
    /// one on a removed line, or outdated, has nowhere in the file to be.
    pub fn github_threads_in(&self, path: &str) -> Vec<crate::review::timeline::LineThread> {
        self.discussion
            .with(|d| github_threads_of(d.as_deref(), path))
    }

    /// The GitHub review comment that starts on a display line of the open
    /// file, for the gutter's hollow marker.
    pub fn github_comment_starting_at(
        &self,
        file: &LoadedFile,
        display_line: usize,
    ) -> Option<u64> {
        let head = file.buffer.head_line(display_line)? as u64 + 1;
        self.github_threads_in(&file.path)
            .into_iter()
            .find(|t| t.comment.line == Some(head))
            .map(|t| t.comment.id)
    }

    pub fn thread_visible(&self, thread_id: &str) -> bool {
        !self
            .hidden_review_threads
            .with(|hidden| hidden.contains(thread_id))
    }

    pub fn apply_review_finding_filter(
        &self,
        filter: ReviewFindingFilter,
        findings: &[ReviewFindingSummary],
    ) {
        let hidden = match filter {
            ReviewFindingFilter::Open => findings
                .iter()
                .filter(|summary| !summary.is_actionable())
                .map(|summary| summary.finding.thread_id.clone())
                .collect(),
            ReviewFindingFilter::All => HashSet::new(),
        };
        self.review_finding_filter.set(filter);
        self.hidden_review_threads.set(hidden);
    }

    pub fn status_of(&self, path: &str) -> Option<ChangeStatus> {
        self.changed_files
            .with(|files| files.iter().find(|f| f.path == path).map(|f| f.status))
    }
}

/// GitHub's review comment threads on a file that the head places: on
/// the head side, with a line. See `ReviewState::github_threads_in`.
pub fn github_threads_of(
    discussion: Option<&Discussion>,
    path: &str,
) -> Vec<crate::review::timeline::LineThread> {
    let Some(d) = discussion else {
        return Vec::new();
    };
    let on_file: Vec<perch_github::ReviewComment> = d
        .review_comments
        .iter()
        .filter(|c| c.path == path)
        .cloned()
        .collect();
    crate::review::timeline::line_threads(&on_file)
        .into_iter()
        .filter(|t| t.comment.side == perch_github::Side::Right && t.comment.line.is_some())
        .collect()
}

/// A file's cards: the reader's current threads anchored in it, oldest first, so
/// two under one line stack in the order they were asked; then GitHub's
/// review threads on it, `github`, by their first comment's id; then the
/// composer, when `composer` is the anchor of its card in the file. A
/// card sits under the last line of its anchor, after any ghost spliced
/// under that line, and takes the rows in `rows` when the card has been
/// laid out, else the estimate, and one row when collapsed. A thread
/// whose last line the file does not have, because its anchor has gone
/// stale past the end, has nowhere to sit and gets no card.
pub fn cards_of(
    threads: &[CommentThread],
    github: &[crate::review::timeline::LineThread],
    composer: Option<&Anchor>,
    file: &LoadedFile,
    collapsed: &HashSet<CardId>,
    rows: &HashMap<CardId, usize>,
) -> Vec<Card> {
    let rows_for = |id: &CardId, estimate: &dyn Fn(bool) -> usize| {
        let is_collapsed = collapsed.contains(id);
        match rows.get(id) {
            Some(&rows) if !is_collapsed => rows,
            _ => estimate(is_collapsed),
        }
    };

    let mut anchored: Vec<&CommentThread> = threads
        .iter()
        .filter(|t| !t.outdated)
        .filter(|t| t.anchor.as_ref().is_some_and(|a| a.path == file.path))
        .collect();
    anchored.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut out: Vec<Card> = anchored
        .into_iter()
        .filter_map(|t| {
            let after_display_line = card_line(file, t.anchor.as_ref()?)?;
            let id = CardId::Thread(t.id.clone());
            let rows = rows_for(&id, &|collapsed| doc::estimate_rows(t, collapsed));
            Some(Card {
                id,
                after_display_line,
                rows,
            })
        })
        .collect();

    let mut github: Vec<&crate::review::timeline::LineThread> = github.iter().collect();
    github.sort_by_key(|t| t.comment.id);
    out.extend(github.into_iter().filter_map(|t| {
        let line = t.comment.line?.saturating_sub(1) as usize;
        let after_display_line = display_line_with_ghosts(&file.buffer, line)?;
        let id = CardId::GitHub(t.comment.id);
        let rows = rows_for(&id, &|collapsed| doc::estimate_github_rows(t, collapsed));
        Some(Card {
            id,
            after_display_line,
            rows,
        })
    }));

    if let Some(after_display_line) = composer.and_then(|a| card_line(file, a)) {
        let rows = rows
            .get(&CardId::Composer)
            .copied()
            .unwrap_or(doc::COMPOSER_ROWS);
        out.push(Card {
            id: CardId::Composer,
            after_display_line,
            rows,
        });
    }
    out
}

/// The display line a card for an anchor goes after. A right-side card goes
/// after its head line and any deletion belonging under that line. A
/// left-side card goes after the exact base row: its ghost when changed or
/// removed, or the aligned head row when unchanged. `None` when the file
/// does not have the line on that side.
fn card_line(file: &LoadedFile, anchor: &Anchor) -> Option<usize> {
    let last = anchor.start_line.max(anchor.line);
    match anchor.side {
        perch_github::Side::Left => {
            let display = display_line_of_anchor_line(file, anchor.side, last)?;
            match file.buffer.row(display)? {
                Row::Deleted { .. } => Some(display),
                Row::Head { head_line, .. } => display_line_with_ghosts(&file.buffer, head_line),
            }
        }
        perch_github::Side::Right => {
            let last = last.checked_sub(1)? as usize;
            display_line_with_ghosts(&file.buffer, last)
        }
    }
}

/// Where a one-based line on one side of the diff appears in the unified
/// display. Head lines map directly. A base line prefers its ghost because a
/// rewritten line also has a paired head row; an unchanged base line has no
/// ghost and maps through the split alignment instead.
fn display_line_of_anchor_line(
    file: &LoadedFile,
    side: perch_github::Side,
    line: u32,
) -> Option<usize> {
    let line = line.checked_sub(1)? as usize;
    match side {
        perch_github::Side::Right => file.buffer.display_line(line),
        perch_github::Side::Left => {
            if let Some(display) = file
                .buffer
                .rows
                .iter()
                .position(|row| matches!(row, Row::Deleted { base_line } if *base_line == line))
            {
                return Some(display);
            }
            let base_line = line + 1;
            let head_line = file
                .alignment
                .iter()
                .find_map(|&(base, head)| (base == Some(base_line)).then_some(head).flatten())?;
            file.buffer.display_line(head_line.checked_sub(1)?)
        }
    }
}

/// The head ranges corresponding to a left-side anchor, for folds. Removed
/// base lines have no head range and need none: ghosts are changed rows and
/// are never folded. Consecutive aligned head lines stay one range.
fn aligned_head_ranges(file: &LoadedFile, anchor: &Anchor) -> Vec<(usize, usize)> {
    let first = anchor.start_line.min(anchor.line) as usize;
    let last = anchor.start_line.max(anchor.line) as usize;
    let mut out: Vec<(usize, usize)> = Vec::new();
    for head in file.alignment.iter().filter_map(|&(base, head)| {
        let base = base?;
        (first..=last)
            .contains(&base)
            .then_some(head)
            .flatten()?
            .checked_sub(1)
    }) {
        match out.last_mut() {
            Some((_, end)) if end.checked_add(1) == Some(head) => *end = head,
            _ => out.push((head, head)),
        }
    }
    out
}

/// The display line a card under a head line goes after: the line's own,
/// or the last of the ghosts spliced under it. The buffer splices a run
/// of removed lines above the head line that follows them, so the run
/// after a line is under it when that next line is unchanged, or there
/// is none: lines removed and nothing put in their place. When the next
/// line is changed, the run is its rewrite's old side, which the split
/// pairs with it, and the card goes before the run on both layouts.
fn display_line_with_ghosts(buffer: &DisplayBuffer, head_line: usize) -> Option<usize> {
    let display = buffer.display_line(head_line)?;
    let ghosts = (display + 1..)
        .take_while(|&d| matches!(buffer.row(d), Some(Row::Deleted { .. })))
        .count();
    let next = buffer.row(display + 1 + ghosts);
    let under = next.is_none_or(|r| r.status() == Some(LineStatus::Unchanged));
    Some(if under { display + ghosts } else { display })
}

/// The head lines current threads are anchored on in a file, as inclusive
/// zero-based ranges, which the folds keep in view: the reader's own from
/// their first line to their last, and GitHub's review
/// comments on the head side that still have a line. One on the base
/// side, or without a line, has nowhere in the head to be and does not
/// count.
pub fn anchors_of(
    threads: &[CommentThread],
    discussion: Option<&Discussion>,
    path: &str,
) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = threads
        .iter()
        .filter(|t| !t.outdated)
        .filter_map(|t| t.anchor.as_ref())
        .filter(|a| a.path == path && a.side == perch_github::Side::Right)
        .map(|a| {
            let (first, last) = (a.start_line.min(a.line), a.start_line.max(a.line));
            (
                first.saturating_sub(1) as usize,
                last.saturating_sub(1) as usize,
            )
        })
        .collect();
    out.extend(
        github_threads_of(discussion, path)
            .into_iter()
            .filter_map(|t| t.comment.line)
            .map(|line| {
                (
                    line.saturating_sub(1) as usize,
                    line.saturating_sub(1) as usize,
                )
            }),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed(path: &str, blob: Option<&str>) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            old_path: None,
            status: if blob.is_some() {
                ChangeStatus::Modified
            } else {
                ChangeStatus::Deleted
            },
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
        state.viewed.set(Arc::new(
            [("b.rs".to_string(), "blob-b".to_string())]
                .into_iter()
                .collect(),
        ));
        state
    }

    #[test]
    fn viewed_is_by_path_and_blob_and_a_deletion_never_is() {
        let state = state();
        let files = state.changed_files.get();
        assert!(state.is_viewed(&files[1]));
        assert!(!state.is_viewed(&files[0]));
        assert!(
            !state.is_viewed(&changed("b.rs", Some("blob-b2"))),
            "a new blob is new content"
        );
        assert!(!state.is_viewed(&files[2]));
        // The deletion has nothing to view, so it is not counted.
        assert_eq!(state.viewed_count(), (1, 3));
    }

    #[test]
    fn next_unviewed_skips_viewed_files_and_wraps_both_ways() {
        let state = state();
        // b is viewed and the deletion has nothing to view, so both are
        // stepped over.
        assert_eq!(
            state.next_unviewed(Some("a.rs"), true).as_deref(),
            Some("c.rs")
        );
        assert_eq!(
            state.next_unviewed(Some("c.rs"), true).as_deref(),
            Some("a.rs"),
            "wraps forward"
        );
        assert_eq!(
            state.next_unviewed(Some("a.rs"), false).as_deref(),
            Some("c.rs"),
            "wraps back"
        );
        assert_eq!(
            state.next_unviewed(Some("gone.rs"), false).as_deref(),
            Some("a.rs")
        );
        // No open file: the first from either end.
        assert_eq!(state.next_unviewed(None, true).as_deref(), Some("a.rs"));
        assert_eq!(state.next_unviewed(None, false).as_deref(), Some("c.rs"));
        // A file outside the list counts as none open.
        assert_eq!(
            state.next_unviewed(Some("elsewhere.rs"), true).as_deref(),
            Some("a.rs")
        );
    }

    #[test]
    fn a_new_changed_list_is_in_tree_order_and_opens_the_way_to_what_is_new() {
        let state = ReviewState::new();
        // Git's order: bytewise, so src/main.rs comes before src/ui/.
        state.set_changed_files(Arc::new(vec![
            changed("src/main.rs", Some("blob-m")),
            changed("src/ui/row.rs", Some("blob-r")),
            changed("README.md", Some("blob-d")),
        ]));
        let order: Vec<String> = state
            .changed_files
            .with(|f| f.iter().map(|f| f.path.clone()).collect());
        assert_eq!(
            order,
            ["src/ui/row.rs", "src/main.rs", "README.md"],
            "the tree's order: ui/ before main.rs"
        );
        assert_eq!(
            state.next_unviewed(None, true).as_deref(),
            Some("src/ui/row.rs"),
            "the next key starts at the top row"
        );
        let expanded = state.expanded.get();
        assert!(expanded.contains("src") && expanded.contains("src/ui"));

        // The reader shuts src/ui, and the poll rereads the same list.
        state
            .expanded
            .update(|e| crate::review::tree::toggle(e, "src/ui"));
        state.set_changed_files(state.changed_files.get());
        assert!(
            !state.expanded.get().contains("src/ui"),
            "nothing new, nothing reopened"
        );

        // A file in a new directory opens the way to it, and only it.
        state.set_changed_files(Arc::new(vec![
            changed("src/main.rs", Some("blob-m")),
            changed("src/ui/row.rs", Some("blob-r")),
            changed("docs/guide.md", Some("blob-g")),
        ]));
        let expanded = state.expanded.get();
        assert!(expanded.contains("docs"));
        assert!(!expanded.contains("src/ui"));
    }

    #[test]
    fn the_anchors_in_a_file_are_its_threads_lines_and_githubs_head_side_comments() {
        use perch_github::{CommentThread, Discussion, ReviewComment, Side, ThreadOrigin};
        let at = "2026-09-18T09:59:00Z".parse().unwrap();
        let thread =
            |id: &str, path: &str, start_line: u32, line: u32, outdated: bool| CommentThread {
                id: id.into(),
                worktree_id: "wt".into(),
                origin: ThreadOrigin::Local,
                anchor: Some(Anchor {
                    path: path.into(),
                    start_line,
                    line,
                    side: Side::Right,
                    commit_sha: "h".into(),
                }),
                outdated,
                resolved: false,
                created_at: at,
                messages: Vec::new(),
            };
        let comment = |id: u64, path: &str, line: Option<u64>, side: Side| ReviewComment {
            id,
            pull_request_review_id: None,
            in_reply_to_id: None,
            user: "chris".into(),
            body: String::new(),
            path: path.into(),
            line,
            original_line: None,
            diff_hunk: String::new(),
            side,
            created_at: at,
            updated_at: None,
            html_url: String::new(),
        };
        let mut worktree = thread("w", "", 0, 0, false);
        worktree.anchor = None;
        let threads = vec![
            thread("a", "src/x.rs", 41, 58, false),
            thread("b", "src/x.rs", 7, 7, true),
            thread("c", "src/y.rs", 1, 3, false),
            worktree,
        ];
        let discussion = Discussion {
            opened_at: None,
            body: String::new(),
            comments: Vec::new(),
            reviews: Vec::new(),
            review_comments: vec![
                comment(1, "src/x.rs", Some(100), Side::Right),
                comment(2, "src/x.rs", Some(200), Side::Left),
                comment(3, "src/x.rs", None, Side::Right),
                comment(4, "src/y.rs", Some(9), Side::Right),
            ],
            events: Vec::new(),
        };
        // Zero based and inclusive: the current thread over 41 to 58 and
        // GitHub's on 100. Not the outdated thread, the base side, the one
        // without a line, or anything from the other file or worktree.
        assert_eq!(
            anchors_of(&threads, Some(&discussion), "src/x.rs"),
            vec![(40, 57), (99, 99)]
        );
        assert_eq!(anchors_of(&threads, None, "src/x.rs"), vec![(40, 57)]);
        assert_eq!(
            anchors_of(&threads, Some(&discussion), "src/z.rs"),
            Vec::<(usize, usize)>::new()
        );

        let state = ReviewState::new();
        state.comment_threads.set(Arc::new(threads));
        state.discussion.set(Some(Arc::new(discussion)));
        assert_eq!(state.anchors_in("src/y.rs"), vec![(0, 2), (8, 8)]);
    }

    /// A thread of the reader's own anchored on head lines `start` to
    /// `line` of `src/x.rs`, one-based, asked at `at`.
    fn anchored(id: &str, start: u32, line: u32, at: &str, outdated: bool) -> CommentThread {
        use perch_github::{Author, CommentMessage, Side, ThreadOrigin};
        let at = at.parse().unwrap();
        CommentThread {
            id: id.into(),
            worktree_id: "wt".into(),
            origin: ThreadOrigin::Local,
            anchor: Some(Anchor {
                path: "src/x.rs".into(),
                start_line: start,
                line,
                side: Side::Right,
                commit_sha: "h".into(),
            }),
            outdated,
            resolved: false,
            created_at: at,
            messages: vec![CommentMessage {
                id: format!("{id}-m1"),
                thread_id: id.into(),
                origin: ThreadOrigin::Local,
                author: Author::User,
                body: "why?".into(),
                created_at: at,
                run: None,
            }],
        }
    }

    #[test]
    fn a_files_cards_sit_under_their_anchors_last_line_past_its_ghosts_oldest_first() {
        // base a b c d e f g; head a B c e F g: b and f rewritten, d
        // removed. Display: a, ghost(b), B, c, ghost(d), e, ghost(f), F, g.
        let file = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "a\nB\nc\ne\nF\ng\n".into(),
            "a\nb\nc\nd\ne\nf\ng\n".into(),
            Some(ChangeStatus::Modified),
            false,
            Scope::Branch,
            String::new(),
        );
        let threads = vec![
            // Asked later, on c, whose ghost of d is spliced under it:
            // nothing took d's place, so the card goes after the ghost.
            anchored("later", 3, 3, "2026-09-18T10:00:00Z", false),
            // Asked first, over a to B; B's ghost is above it, not under.
            anchored("first", 1, 2, "2026-09-18T09:00:00Z", false),
            // Asked last, on e: the ghost after it is F's old side, which
            // the split pairs with F, so the card goes before it.
            anchored("last", 4, 4, "2026-09-18T11:00:00Z", false),
            // Past the end of the file: nowhere to sit.
            anchored("gone", 9, 9, "2026-09-18T09:30:00Z", true),
        ];
        let mut elsewhere = anchored("elsewhere", 1, 1, "2026-09-18T08:00:00Z", false);
        elsewhere.anchor.as_mut().unwrap().path = "src/y.rs".into();
        let mut worktree = anchored("worktree", 1, 1, "2026-09-18T08:00:00Z", false);
        worktree.anchor = None;
        let threads = [threads, vec![elsewhere, worktree]].concat();

        let cards = cards_of(&threads, &[], None, &file, &HashSet::new(), &HashMap::new());
        let estimate = doc::estimate_rows(&threads[0], false);
        assert_eq!(
            cards,
            vec![
                Card {
                    id: CardId::Thread("first".into()),
                    after_display_line: 2,
                    rows: estimate
                },
                Card {
                    id: CardId::Thread("later".into()),
                    after_display_line: 4,
                    rows: estimate
                },
                Card {
                    id: CardId::Thread("last".into()),
                    after_display_line: 5,
                    rows: estimate
                },
            ]
        );
        // A run removed from the end of the file is under the last line.
        let cut = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "a\n".into(),
            "a\nb\nc\n".into(),
            Some(ChangeStatus::Modified),
            false,
            Scope::Branch,
            String::new(),
        );
        let on_a = [anchored("end", 1, 1, "2026-09-18T10:00:00Z", false)];
        assert_eq!(
            cards_of(&on_a, &[], None, &cut, &HashSet::new(), &HashMap::new())[0]
                .after_display_line,
            2
        );

        // Collapsed, a card is one row; laid out, it has the rows the
        // layout gave it, unless it is collapsed.
        let collapsed: HashSet<CardId> = [CardId::Thread("first".into())].into_iter().collect();
        let rows: HashMap<CardId, usize> = [
            (CardId::Thread("first".into()), 9),
            (CardId::Thread("later".into()), 7),
        ]
        .into_iter()
        .collect();
        let cards = cards_of(&threads, &[], None, &file, &collapsed, &rows);
        assert_eq!(cards[0].rows, doc::COLLAPSED_ROWS);
        assert_eq!(cards[1].rows, 7);

        let state = ReviewState::new();
        assert!(state.cards_in("src/x.rs").is_empty(), "no file, no cards");
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(file));
        });
        state.comment_threads.set(Arc::new(threads));
        assert_eq!(state.cards_in("src/x.rs").len(), 3);
        state.toggle_card(&CardId::Thread("later".into()));
        assert!(state.is_collapsed(&CardId::Thread("later".into())));
        assert_eq!(state.cards_in("src/x.rs")[1].rows, 1);
        state.toggle_card(&CardId::Thread("later".into()));
        assert_eq!(state.cards_in("src/x.rs")[1].rows, estimate);
    }

    #[test]
    fn base_side_anchors_use_ghosts_and_alignment_for_markers_and_cards() {
        use perch_github::Side;

        // base a b c d e f g; head a B c e F g. The old b and f are
        // rewritten ghosts, d is a deleted ghost, and unchanged e and g
        // have shifted head line numbers.
        let file = spliced_file();
        let on_side = |id: &str, line: u32, side: Side, at: &str| {
            let mut thread = anchored(id, line, line, at, false);
            thread.anchor.as_mut().unwrap().side = side;
            thread
        };
        let threads = vec![
            on_side("left-rewrite", 2, Side::Left, "2026-09-18T08:00:00Z"),
            on_side("right-rewrite", 2, Side::Right, "2026-09-18T09:00:00Z"),
            on_side("left-deleted", 4, Side::Left, "2026-09-18T10:00:00Z"),
            on_side("left-unchanged", 5, Side::Left, "2026-09-18T11:00:00Z"),
            on_side("left-shifted", 7, Side::Left, "2026-09-18T12:00:00Z"),
        ];

        let cards = cards_of(&threads, &[], None, &file, &HashSet::new(), &HashMap::new());
        let placed: Vec<(CardId, usize)> = cards
            .iter()
            .map(|card| (card.id.clone(), card.after_display_line))
            .collect();
        assert_eq!(
            placed,
            vec![
                (CardId::Thread("left-rewrite".into()), 1),
                (CardId::Thread("right-rewrite".into()), 2),
                (CardId::Thread("left-deleted".into()), 4),
                (CardId::Thread("left-unchanged".into()), 5),
                (CardId::Thread("left-shifted".into()), 8),
            ]
        );

        // A base line that is unchanged still owns a pure-deletion run
        // immediately below it, just as the corresponding head line does.
        let cut = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "a\n".into(),
            "a\nb\nc\n".into(),
            Some(ChangeStatus::Modified),
            false,
            Scope::Branch,
            String::new(),
        );
        let left_on_a = [on_side(
            "left-before-cut",
            1,
            Side::Left,
            "2026-09-18T13:00:00Z",
        )];
        assert_eq!(
            cards_of(
                &left_on_a,
                &[],
                None,
                &cut,
                &HashSet::new(),
                &HashMap::new()
            )[0]
            .after_display_line,
            2
        );

        let state = ReviewState::new();
        state.comment_threads.set(Arc::new(threads));
        let id_at = |display| {
            state
                .thread_starting_at(&file, display)
                .map(|thread| thread.id)
        };
        assert_eq!(id_at(1).as_deref(), Some("left-rewrite"));
        assert_eq!(id_at(2).as_deref(), Some("right-rewrite"));
        assert_eq!(id_at(4).as_deref(), Some("left-deleted"));
        assert_eq!(id_at(5).as_deref(), Some("left-unchanged"));
        assert_eq!(id_at(8).as_deref(), Some("left-shifted"));
        assert_eq!(
            state
                .thread_starting_on(&file, Side::Left, 1)
                .map(|thread| thread.id),
            Some("left-rewrite".into())
        );
        assert_eq!(
            state
                .thread_starting_on(&file, Side::Right, 1)
                .map(|thread| thread.id),
            Some("right-rewrite".into())
        );
    }

    #[test]
    fn base_side_anchor_ranges_use_aligned_head_lines_for_folds() {
        use perch_github::Side;

        let file = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "new\na\nb\nc\n".into(),
            "a\nb\nc\n".into(),
            Some(ChangeStatus::Modified),
            false,
            Scope::Branch,
            String::new(),
        );
        let mut on_base_b = anchored("left", 2, 2, "2026-09-18T10:00:00Z", false);
        on_base_b.anchor.as_mut().unwrap().side = Side::Left;

        let state = ReviewState::new();
        state.files.update(|files| {
            files.insert("src/x.rs".into(), Arc::new(file));
        });
        state.comment_threads.set(Arc::new(vec![on_base_b]));

        assert_eq!(state.anchors_in("src/x.rs"), vec![(2, 2)]);
    }

    #[test]
    fn outdated_reader_threads_stay_off_inline_surfaces() {
        let file = spliced_file();
        let threads = vec![
            anchored("outdated", 1, 1, "2026-09-18T09:00:00Z", true),
            anchored("current", 3, 3, "2026-09-18T10:00:00Z", false),
        ];

        let cards = cards_of(&threads, &[], None, &file, &HashSet::new(), &HashMap::new());
        let card_ids: Vec<CardId> = cards.iter().map(|card| card.id.clone()).collect();
        assert_eq!(card_ids, vec![CardId::Thread("current".into())]);
        assert_eq!(anchors_of(&threads, None, "src/x.rs"), vec![(2, 2)]);

        let state = ReviewState::new();
        state.files.update(|files| {
            files.insert("src/x.rs".into(), Arc::new(file));
        });
        state.active.set(Some("src/x.rs".into()));
        state.comment_threads.set(Arc::new(threads));
        let active = state.active_file().unwrap();

        assert!(state.thread_starting_at(&active, 0).is_none());
        assert_eq!(
            state.thread_starting_at(&active, 3).map(|thread| thread.id),
            Some("current".into())
        );
        let card_ids: Vec<CardId> = state
            .cards_in("src/x.rs")
            .iter()
            .map(|card| card.id.clone())
            .collect();
        assert_eq!(card_ids, vec![CardId::Thread("current".into())]);
        assert_eq!(state.anchors_in("src/x.rs"), vec![(2, 2)]);
        assert!(!state.follow_up_is_inline("outdated"));

        // A reply to an outdated thread has no inline card, so it moves to
        // Conversation without losing what was typed.
        state.show_tab(ItemTab::Files);
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        let outdated_anchor = state
            .comment_threads
            .with(|threads| threads[0].anchor.clone());
        state.open_composer(
            Some("outdated".into()),
            outdated_anchor,
            "reply".into(),
            choice,
        );
        assert!(!state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert_eq!(state.composer_draft.get(), "reply");
    }

    /// A review comment from GitHub on head line `line` of `src/x.rs`,
    /// one-based, with a reply.
    fn github_comment(
        id: u64,
        line: Option<u64>,
        side: perch_github::Side,
        reply_to: Option<u64>,
    ) -> perch_github::ReviewComment {
        perch_github::ReviewComment {
            id,
            pull_request_review_id: None,
            in_reply_to_id: reply_to,
            user: "octocat".into(),
            body: "nit".into(),
            path: "src/x.rs".into(),
            line,
            original_line: None,
            diff_hunk: String::new(),
            side,
            created_at: "2026-09-18T09:59:00Z".parse().unwrap(),
            updated_at: None,
            html_url: String::new(),
        }
    }

    /// The file of `a_files_cards_sit_under_their_anchors_last_line_past_its_ghosts_oldest_first`.
    fn spliced_file() -> LoadedFile {
        crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "a\nB\nc\ne\nF\ng\n".into(),
            "a\nb\nc\nd\ne\nf\ng\n".into(),
            Some(ChangeStatus::Modified),
            false,
            Scope::Branch,
            String::new(),
        )
    }

    #[test]
    fn githubs_threads_and_the_composer_are_cards_after_the_readers_under_the_same_line() {
        use perch_github::Side;
        let file = spliced_file();
        let threads = vec![anchored("mine", 3, 3, "2026-09-18T10:00:00Z", false)];
        // Two of GitHub's on c, the later by id first in the list, one on
        // e, and one on the base side, which has nowhere in the head to be.
        let discussion = Discussion {
            opened_at: None,
            body: String::new(),
            comments: Vec::new(),
            reviews: Vec::new(),
            review_comments: vec![
                github_comment(20, Some(3), Side::Right, None),
                github_comment(10, Some(3), Side::Right, None),
                github_comment(11, Some(3), Side::Right, Some(10)),
                github_comment(30, Some(4), Side::Right, None),
                github_comment(40, Some(2), Side::Left, None),
            ],
            events: Vec::new(),
        };
        let github = github_threads_of(Some(&discussion), "src/x.rs");
        assert_eq!(github.len(), 3, "the base side's is not placed");
        let composer = Anchor {
            path: "src/x.rs".into(),
            start_line: 1,
            line: 3,
            side: Side::Right,
            commit_sha: "h".into(),
        };
        let cards = cards_of(
            &threads,
            &github,
            Some(&composer),
            &file,
            &HashSet::new(),
            &HashMap::new(),
        );
        let ids: Vec<(CardId, usize)> = cards
            .iter()
            .map(|c| (c.id.clone(), c.after_display_line))
            .collect();
        // Under c, past the ghost of d at display line 4: the reader's,
        // then GitHub's by id, then the composer on the last selected
        // line; GitHub's on e is under e alone.
        assert_eq!(
            ids,
            vec![
                (CardId::Thread("mine".into()), 4),
                (CardId::GitHub(10), 4),
                (CardId::GitHub(20), 4),
                (CardId::GitHub(30), 5),
                (CardId::Composer, 4),
            ]
        );
        let two_messages = github.iter().find(|t| t.comment.id == 10).unwrap();
        assert_eq!(two_messages.replies.len(), 1);
        assert_eq!(
            cards[1].rows,
            doc::estimate_github_rows(two_messages, false)
        );
        let one_message = github.iter().find(|t| t.comment.id == 20).unwrap();
        assert_eq!(cards[2].rows, doc::estimate_github_rows(one_message, false));
        assert!(
            cards[2].rows < cards[1].rows,
            "a comment without a reply is shorter"
        );
        assert_eq!(
            cards[4].rows,
            doc::COMPOSER_ROWS,
            "the composer's estimate until it is laid out"
        );

        // Laid out and collapsed, as the reader's cards are.
        let collapsed: HashSet<CardId> = [CardId::GitHub(10)].into_iter().collect();
        let rows: HashMap<CardId, usize> = [(CardId::GitHub(20), 6), (CardId::Composer, 12)]
            .into_iter()
            .collect();
        let cards = cards_of(&threads, &github, Some(&composer), &file, &collapsed, &rows);
        assert_eq!(cards[1].rows, doc::COLLAPSED_ROWS);
        assert_eq!(cards[2].rows, 6);
        assert_eq!(cards[4].rows, 12);

        // Through the state: the discussion and the composer are read
        // with the threads.
        let state = ReviewState::new();
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(file));
        });
        state.active.set(Some("src/x.rs".into()));
        state.comment_threads.set(Arc::new(threads));
        state.discussion.set(Some(Arc::new(discussion)));
        assert_eq!(state.cards_in("src/x.rs").len(), 4);
        assert!(!state.composer_is_inline());
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        state.show_tab(ItemTab::Files);
        state.open_composer(None, Some(composer.clone()), String::new(), choice.clone());
        assert!(state.composer_is_inline());
        assert_eq!(state.tab.get(), ItemTab::Files);
        assert_eq!(
            state.composer_card_anchor("src/x.rs"),
            Some(composer.clone())
        );
        assert_eq!(state.composer_card_anchor("src/y.rs"), None);
        assert_eq!(state.cards_in("src/x.rs").len(), 5);
        // A follow-up is inside its thread's card, not a card of its own,
        // and stays on Files when that card is in the open file. A question
        // about the item as a whole moves to Conversation.
        state.open_composer(
            Some("mine".into()),
            Some(composer.clone()),
            String::new(),
            choice.clone(),
        );
        assert!(!state.composer_is_inline());
        assert!(state.follow_up_is_inline("mine"));
        assert!(
            !state.follow_up_is_inline("elsewhere"),
            "no such thread in the file"
        );
        assert!(state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Files);
        assert_eq!(state.cards_in("src/x.rs").len(), 4);
        state.open_composer(None, None, "whole item".into(), choice.clone());
        assert!(!state.composer_is_inline());
        assert!(!state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert_eq!(state.composer_draft.get(), "whole item");
        assert_eq!(state.cards_in("src/x.rs").len(), 4);

        // A follow-up on a thread whose card is collapsed opens the card,
        // so the box it sits inside is seen.
        state.show_tab(ItemTab::Files);
        state.toggle_card(&CardId::Thread("mine".into()));
        state.open_composer(Some("mine".into()), Some(composer), String::new(), choice);
        assert!(!state.is_collapsed(&CardId::Thread("mine".into())));
        assert_eq!(state.tab.get(), ItemTab::Files);
    }

    #[test]
    fn the_composers_card_anchor_counts_among_the_files_anchors_until_its_thread_arrives() {
        use perch_github::Side;
        let state = ReviewState::new();
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(spliced_file()));
        });
        state.active.set(Some("src/x.rs".into()));
        let anchor = Anchor {
            path: "src/x.rs".into(),
            start_line: 2,
            line: 3,
            side: Side::Right,
            commit_sha: "h".into(),
        };
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        assert!(state.anchors_in("src/x.rs").is_empty());

        // Open on lines 2 to 3, the composer's card keeps them out of the
        // folds as a thread's would; the other file has nothing.
        state.open_composer(None, Some(anchor.clone()), String::new(), choice.clone());
        assert_eq!(state.anchors_in("src/x.rs"), vec![(1, 2)]);
        assert!(state.anchors_in("src/y.rs").is_empty());

        // Sent, the placeholder keeps them until the thread's own anchor
        // takes over, and the thread is then the one anchor.
        state.composer_sent();
        assert_eq!(state.anchors_in("src/x.rs"), vec![(1, 2)]);
        state.set_comment_threads(Arc::new(vec![anchored(
            "new",
            2,
            3,
            "2026-09-18T10:00:00Z",
            false,
        )]));
        assert_eq!(state.anchors_in("src/x.rs"), vec![(1, 2)]);

        // Escape gives the lines back; a follow-up is no card and adds
        // none; a composer in Conversation adds none.
        state.open_composer(None, Some(anchor.clone()), String::new(), choice.clone());
        assert_eq!(state.anchors_in("src/x.rs"), vec![(1, 2), (1, 2)]);
        state.close_composer();
        assert_eq!(state.anchors_in("src/x.rs"), vec![(1, 2)]);
        state.open_composer(
            Some("new".into()),
            Some(anchor),
            String::new(),
            choice.clone(),
        );
        assert_eq!(state.anchors_in("src/x.rs"), vec![(1, 2)]);
        state.open_composer(None, None, String::new(), choice);
        assert_eq!(state.anchors_in("src/x.rs"), vec![(1, 2)]);
    }

    #[test]
    fn a_composer_replacing_an_open_one_starts_on_the_estimate() {
        use perch_github::Side;
        let state = ReviewState::new();
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(spliced_file()));
        });
        state.active.set(Some("src/x.rs".into()));
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        let on = |start: u32, line: u32| Anchor {
            path: "src/x.rs".into(),
            start_line: start,
            line,
            side: Side::Right,
            commit_sha: "h".into(),
        };

        // A composer typed into grows to fourteen rows; `a` on other
        // lines replaces it with an empty one, which is the estimate's
        // height, not the last box's.
        state.open_composer(None, Some(on(3, 3)), String::new(), choice.clone());
        state.set_card_rows(&CardId::Composer, 14);
        assert_eq!(state.cards_in("src/x.rs")[0].rows, 14);
        state.open_composer(None, Some(on(1, 2)), String::new(), choice.clone());
        assert!(
            !state.card_rows.get().contains_key(&CardId::Composer),
            "the last card's rows are not this one's"
        );
        assert_eq!(
            state.cards_in("src/x.rs"),
            vec![Card {
                id: CardId::Composer,
                after_display_line: 2,
                rows: doc::COMPOSER_ROWS
            }]
        );

        // A follow-up replacing it leaves nothing behind for the next
        // new composer either.
        state.set_card_rows(&CardId::Composer, 12);
        state.set_comment_threads(Arc::new(vec![anchored(
            "t",
            3,
            3,
            "2026-09-18T10:00:00Z",
            false,
        )]));
        state.open_composer(
            Some("t".into()),
            Some(on(3, 3)),
            String::new(),
            choice.clone(),
        );
        assert!(!state.card_rows.get().contains_key(&CardId::Composer));
        state.open_composer(None, Some(on(1, 2)), String::new(), choice);
        assert_eq!(
            state
                .cards_in("src/x.rs")
                .iter()
                .find(|c| c.id == CardId::Composer)
                .map(|c| c.rows),
            Some(doc::COMPOSER_ROWS)
        );
    }

    #[test]
    fn a_composer_moves_to_conversation_when_the_open_file_changes_under_it() {
        use perch_github::Side;
        let state = ReviewState::new();
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(spliced_file()));
        });
        state.open_tab("src/x.rs");
        let anchor = Anchor {
            path: "src/x.rs".into(),
            start_line: 3,
            line: 3,
            side: Side::Right,
            commit_sha: "h".into(),
        };
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );

        // Opened on lines of the open file, the composer is a card there
        // and Files stays visible. Another file opened over it has no card
        // for it, so Conversation takes over without losing the draft.
        state.show_tab(ItemTab::Files);
        state.open_composer(None, Some(anchor.clone()), "why?".into(), choice.clone());
        assert!(state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Files);
        state.open_tab("src/y.rs");
        assert!(!state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert_eq!(state.composer_draft.get(), "why?");
        state.open_tab("src/x.rs");
        assert!(state.composer_in_code());
        state.show_tab(ItemTab::Files);
        assert_eq!(state.tab.get(), ItemTab::Files);

        // Closing the composer's file for its neighbour does the same.
        state.close_tab("src/x.rs");
        assert_eq!(state.active.get().as_deref(), Some("src/y.rs"));
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert_eq!(state.composer_draft.get(), "why?");

        // A follow-up inside a card of the open file likewise.
        state.close_composer();
        state.open_tab("src/x.rs");
        state.show_tab(ItemTab::Files);
        state.set_comment_threads(Arc::new(vec![anchored(
            "t",
            3,
            3,
            "2026-09-18T10:00:00Z",
            false,
        )]));
        state.open_composer(
            Some("t".into()),
            Some(anchor),
            String::new(),
            choice.clone(),
        );
        assert!(state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Files);
        state.open_tab("src/y.rs");
        assert_eq!(state.tab.get(), ItemTab::Conversation);

        // Without a composer, changing files leaves Files selected.
        state.close_composer();
        state.open_tab("src/x.rs");
        state.show_tab(ItemTab::Files);
        state.open_tab("src/y.rs");
        assert_eq!(state.tab.get(), ItemTab::Files);
    }

    #[test]
    fn a_notice_gives_up_a_sent_questions_rows() {
        use perch_github::Side;
        let state = ReviewState::new();
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(spliced_file()));
        });
        state.active.set(Some("src/x.rs".into()));
        let anchor = Anchor {
            path: "src/x.rs".into(),
            start_line: 3,
            line: 3,
            side: Side::Right,
            commit_sha: "h".into(),
        };
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );

        // Nothing sent: nothing to give up.
        state.drop_sent_placeholder();
        assert!(state.sent_composer.get().is_none());

        // Sent and laid out at ten rows, the send fails: the rows go, and
        // the composer that was already closed stays closed.
        state.open_composer(None, Some(anchor.clone()), String::new(), choice);
        state.set_card_rows(&CardId::Composer, 10);
        state.composer_sent();
        assert_eq!(state.sent_composer.get(), Some(anchor));
        state.drop_sent_placeholder();
        assert!(state.sent_composer.get().is_none());
        assert!(
            state.cards_in("src/x.rs").is_empty(),
            "the placeholder's rows went with it"
        );
        assert!(!state.card_rows.get().contains_key(&CardId::Composer));
        assert!(state.composer.get().is_none());
    }

    #[test]
    fn a_sent_question_keeps_its_cards_rows_until_the_threads_arrive_and_escape_gives_them_up() {
        use perch_github::Side;
        let state = ReviewState::new();
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(spliced_file()));
        });
        state.active.set(Some("src/x.rs".into()));
        let anchor = Anchor {
            path: "src/x.rs".into(),
            start_line: 3,
            line: 3,
            side: Side::Right,
            commit_sha: "h".into(),
        };
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );

        // Opened, laid out at ten rows, and sent: the composer is gone,
        // and the card's rows stay under its lines.
        state.open_composer(None, Some(anchor.clone()), String::new(), choice.clone());
        state.set_card_rows(&CardId::Composer, 10);
        assert_eq!(
            state.cards_in("src/x.rs"),
            vec![Card {
                id: CardId::Composer,
                after_display_line: 4,
                rows: 10
            }]
        );
        state.composer_sent();
        assert!(state.composer.get().is_none());
        assert_eq!(state.sent_composer.get(), Some(anchor.clone()));
        assert_eq!(
            state.cards_in("src/x.rs"),
            vec![Card {
                id: CardId::Composer,
                after_display_line: 4,
                rows: 10
            }]
        );

        // The thread arrives, and its card takes the place of the rows.
        state.set_comment_threads(Arc::new(vec![anchored(
            "new",
            3,
            3,
            "2026-09-18T10:00:00Z",
            false,
        )]));
        assert_eq!(state.sent_composer.get(), None);
        let cards = state.cards_in("src/x.rs");
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id, CardId::Thread("new".into()));
        assert!(
            !state.card_rows.get().contains_key(&CardId::Composer),
            "the next composer starts on the estimate"
        );

        // Escape gives the rows up at once.
        state.open_composer(None, Some(anchor.clone()), String::new(), choice.clone());
        state.set_card_rows(&CardId::Composer, 10);
        state.close_composer();
        assert_eq!(state.cards_in("src/x.rs").len(), 1);
        assert!(!state.card_rows.get().contains_key(&CardId::Composer));

        // A question sent from Conversation keeps no rows: it was no card.
        state.open_composer(None, None, String::new(), choice.clone());
        state.composer_sent();
        assert_eq!(state.sent_composer.get(), None);

        // A new composer opened before the thread lands takes the
        // placeholder's place: there is one composer card at a time.
        state.open_composer(None, Some(anchor.clone()), String::new(), choice.clone());
        state.composer_sent();
        assert!(state.sent_composer.get().is_some());
        state.open_composer(None, Some(anchor), String::new(), choice);
        assert_eq!(state.sent_composer.get(), None);
        assert_eq!(
            state
                .cards_in("src/x.rs")
                .iter()
                .filter(|c| c.id == CardId::Composer)
                .count(),
            1
        );
    }

    #[test]
    fn a_cards_laid_out_rows_are_written_when_they_change_and_not_otherwise() {
        use floem::reactive::{Effect, SignalTrack};
        let state = ReviewState::new();
        let card = CardId::Thread("t1".into());
        // Every write rebuilds the open file's document, so the writes
        // are counted through what a write runs.
        let writes = RwSignal::new(0);
        Effect::new(move |_| {
            state.card_rows.track();
            writes.update(|w| *w += 1);
        });
        assert_eq!(writes.get(), 1, "the effect's first run");

        state.set_card_rows(&card, 8);
        assert_eq!(state.card_rows.get().get(&card), Some(&8));
        assert_eq!(writes.get(), 2);
        state.set_card_rows(&card, 8);
        assert_eq!(writes.get(), 2, "the same rows again is no write");
        state.set_card_rows(&card, 5);
        assert_eq!(state.card_rows.get().get(&card), Some(&5));
        assert_eq!(writes.get(), 3);
        // Another card's rows are its own.
        state.set_card_rows(&CardId::Thread("t2".into()), 5);
        assert_eq!(writes.get(), 4);
        assert_eq!(state.card_rows.get().len(), 2);
    }

    #[test]
    fn a_thread_arriving_outdated_starts_collapsed_once_and_the_readers_choice_stands_after() {
        let state = ReviewState::new();
        let stale = CardId::Thread("stale".into());
        let fresh = CardId::Thread("fresh".into());
        state.set_comment_threads(Arc::new(vec![
            anchored("stale", 1, 1, "2026-09-18T09:00:00Z", true),
            anchored("fresh", 2, 2, "2026-09-18T09:00:00Z", false),
        ]));
        assert!(state.is_collapsed(&stale));
        assert!(!state.is_collapsed(&fresh));
        assert_eq!(state.comment_threads.get().len(), 2);

        // Opened by the reader, it stays open when the threads come again,
        // as they do every time the item is opened.
        state.toggle_card(&stale);
        state.begin("Reading the pull request");
        assert!(state.comment_threads.get().is_empty());
        assert!(
            !state.is_collapsed(&stale),
            "the reader's choice outlives the item"
        );
        state.set_comment_threads(Arc::new(vec![anchored(
            "stale",
            1,
            1,
            "2026-09-18T09:00:00Z",
            true,
        )]));
        assert!(!state.is_collapsed(&stale));

        // A thread with no anchor is no card, and a new outdated one is
        // collapsed as the first was.
        let mut worktree = anchored("worktree", 1, 1, "2026-09-18T09:00:00Z", true);
        worktree.anchor = None;
        state.set_comment_threads(Arc::new(vec![
            worktree,
            anchored("another", 3, 3, "2026-09-18T09:00:00Z", true),
        ]));
        assert!(!state.is_collapsed(&CardId::Thread("worktree".into())));
        assert!(state.is_collapsed(&CardId::Thread("another".into())));
    }

    #[test]
    fn a_reveal_is_kept_by_path_and_blob_and_reset_forgets_the_run_it_is_in() {
        let state = ReviewState::new();
        assert!(state.reveals_for("src/x.rs", "b1").is_empty());
        state.reveal("src/x.rs", "b1", 4, 23);
        state.reveal("src/x.rs", "b2", 50, 60);
        assert!(state.reveals_for("src/x.rs", "b1").overlaps(4, 37));
        assert!(
            !state.reveals_for("src/x.rs", "b1").overlaps(50, 60),
            "another blob's reveal is another file's"
        );
        // Reset on a piece of the run folds the whole run: the piece
        // remembers the run it was cut from.
        let piece = crate::review::doc::Run {
            first: 25,
            last: 38,
            first_head: 24,
            default_heads: (4, 37),
        };
        state.reset("src/x.rs", "b1", &[piece.default_heads]);
        assert!(state.reveals_for("src/x.rs", "b1").is_empty());
        assert!(
            state.reveals_for("src/x.rs", "b2").overlaps(50, 60),
            "and no other"
        );
        let key = ("src/x.rs".to_string(), "b1".to_string());
        assert!(
            !state.reveals.get().contains_key(&key),
            "an empty set is dropped"
        );

        // Each went to the store as it happened, the reset over the whole
        // run the piece was cut from.
        assert_eq!(
            state.pending_reveals.get(),
            vec![
                RevealWrite::Reveal {
                    path: "src/x.rs".into(),
                    blob: "b1".into(),
                    first: 4,
                    last: 23
                },
                RevealWrite::Reveal {
                    path: "src/x.rs".into(),
                    blob: "b2".into(),
                    first: 50,
                    last: 60
                },
                RevealWrite::Reset {
                    path: "src/x.rs".into(),
                    blob: "b1".into(),
                    first: 4,
                    last: 37
                },
            ]
        );

        // A file with no blob keeps its reveals here and sends nothing.
        state.pending_reveals.set(Vec::new());
        state.reveal("untracked.rs", "", 0, 9);
        state.reset("untracked.rs", "", &[piece.default_heads]);
        assert!(state.pending_reveals.get().is_empty());
        assert!(
            state.reveals_for("untracked.rs", "").is_empty(),
            "the reset still took in memory"
        );

        // A divider carrying a run taken whole resets both, one write each.
        state.reveal("src/x.rs", "b1", 4, 23);
        state.reveal("src/x.rs", "b1", 43, 58);
        state.reveal("src/x.rs", "b1", 70, 79);
        state.pending_reveals.set(Vec::new());
        state.reset("src/x.rs", "b1", &[(4, 37), (43, 58)]);
        assert_eq!(
            state
                .reveals_for("src/x.rs", "b1")
                .iter()
                .collect::<Vec<_>>(),
            vec![(70, 79)]
        );
        assert_eq!(
            state.pending_reveals.get(),
            vec![
                RevealWrite::Reset {
                    path: "src/x.rs".into(),
                    blob: "b1".into(),
                    first: 4,
                    last: 37
                },
                RevealWrite::Reset {
                    path: "src/x.rs".into(),
                    blob: "b1".into(),
                    first: 43,
                    last: 58
                },
            ]
        );
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

        state
            .changed_files
            .set(Arc::new(vec![changed("a.rs", Some("blob-a"))]));
        assert_eq!(state.next_unviewed(Some("a.rs"), true), None);
        assert_eq!(state.next_unviewed(None, false), None);
        assert_eq!(state.viewed_count(), (1, 1));

        state.changed_files.set(Arc::new(Vec::new()));
        assert_eq!(state.next_unviewed(None, true), None);
        assert_eq!(state.viewed_count(), (0, 0));
    }
}
