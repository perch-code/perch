//! The background half of the app.
//!
//! Owns the tokio runtime, the GitHub client, and the SQLite cache. The UI
//! never touches any of them: it sends `Command`s and receives `Event`s.
//!
//! Commands travel on a tokio channel, which can be sent to from the UI
//! thread without a runtime context. Events travel back on a std channel,
//! because that is what floem's `update_signal_from_channel` consumes.

use std::sync::mpsc::Sender as EventSender;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use perch_git::{Preparer, RepoId, Store};
use perch_github::{
    resolve_token, Anchor, Author, Cache, CacheError, CommentMessage, CommentThread, Config,
    GitHub, Item, ItemCheckout, ItemKind, ItemSubject, PullRequest, RunRecord, RunStatus,
    SubjectKind, Thread, ThreadOrigin,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Semaphore;

pub(crate) mod backfill;
#[cfg(test)]
mod fake;
pub(crate) mod sync;

/// How many avatar downloads may be in flight at once.
const AVATAR_CONCURRENCY: usize = 6;
/// Settings the UI keeps between launches. Sent back at startup.
pub const UI_META_KEYS: [&str; 5] =
    ["item_filters", "collapsed_sections", "review.layout", "viewer.login", "viewer.avatar_url"];

#[derive(Debug, Clone)]
pub enum Command {
    Refresh,
    /// Open an item's view: the review for a pull request or a branch, the
    /// issue view for an issue. Prepares the checkout when there is one to
    /// prepare.
    OpenItem { id: String },
    /// Find or make the item for a pull request or issue named by number,
    /// and open it. Every sidebar row opens by id; this is for the launch
    /// hooks and the review header's "Closes #N" chip, which name a subject
    /// that may have no row yet.
    OpenSubject { repo_full_name: String, kind: ItemKind, number: u64 },
    /// Read, diff, and highlight one file of the open checkout, over the
    /// scope that is set.
    OpenFile { path: String },
    /// What the diff is over: the branch, a commit, the uncommitted work.
    /// The changed list is recomputed and every open file reloads.
    SetScope { item_id: String, scope: perch_git::Scope },
    /// Look at the checkout again: its head, its dirt and its commits, and
    /// the changed list if the head moved. Sent as the scope menu is about
    /// to open, and on every poll, so terminal work shows up.
    RefreshScope { item_id: String },
    /// Check or uncheck a file's content as viewed.
    SetViewed { item_id: String, path: String, blob: String, viewed: bool },
    /// Ask the language server about a position. `token` lets the UI drop an
    /// answer that arrives after the pointer has moved on.
    Hover { token: u64, path: String, line: u32, character: u32 },
    Definition { token: u64, path: String, line: u32, character: u32 },
    References { token: u64, path: String, line: u32, character: u32 },
    CloseReview,

    /// Leave Active. An owned checkout's directory goes too, refused when
    /// there is uncommitted work in it; a checkout under a root is left
    /// alone and only the row is hidden.
    ArchiveItem { id: String },
    /// Back to Active, and an owned checkout back on disk.
    RestoreItem { id: String },
    /// Remove the row and its threads. An owned checkout's directory and
    /// branch go too; anything under a root is not touched.
    DeleteItem { id: String },
    /// Saved and Done, each left alone when `None`.
    MarkItem { id: String, saved: Option<bool>, done: Option<bool> },
    PinItem { id: String, pinned: bool },
    /// Attention now, reason `manual`.
    MarkUnread { id: String },
    RenameItem { id: String, name: Option<String> },
    /// Cut `perch/issue-<n>` from the default branch and make a
    /// checkout-only item pointing back at the issue.
    StartWork { issue_item_id: String },
    /// Cut a branch from the item's head into a new checkout-only item.
    ForkItem { id: String },
    /// Remember a setting across launches.
    SetMeta { key: String, value: String },

    /// Ask the agent a question in an item's checkout. `thread_id` of `None`
    /// starts a thread; the anchor is ignored on a follow-up.
    Ask {
        item_id: String,
        thread_id: Option<String>,
        anchor: Option<Anchor>,
        question: String,
        backend: perch_agent::Backend,
        model: String,
        effort: perch_agent::Effort,
    },
    CancelRun { run_id: String },
    DeleteCommentThread { id: String },
}

#[derive(Debug, Clone)]
pub enum Event {
    Avatar { url: String, bytes: Vec<u8> },
    /// The repositories config.toml declares, `owner/repo` in config
    /// order, which is the order the sidebar lists them in. Sent once at
    /// startup; the file is read once.
    Repos(Vec<String>),
    /// Every item, active and archived. Whole, for the same reason.
    Items(Vec<Item>),
    /// A remembered setting, sent once at startup for each key the UI
    /// asked to keep.
    Meta { key: String, value: String },
    /// An issue is open: its snapshot, its comments, and the item.
    IssueReady {
        item: Arc<Item>,
        issue: Arc<perch_github::Issue>,
        comments: Arc<Vec<perch_github::IssueComment>>,
        /// True when the fetch failed and this is the snapshot alone.
        offline: bool,
    },
    /// A pull request's or an issue's discussion was archived under the
    /// knowledge root. `added` counts records that were new or changed.
    Archived { repo_full_name: String, kind: ItemKind, number: u64, added: usize },
    /// Something worth a line in the status bar and nothing more.
    Notice(String),
    /// Where the backfill is: `done` discussions archived of about
    /// `total` for `repo`, sent after each one. A `total` of 0 says every
    /// repository is caught up and the line comes down.
    Backfill { repo: String, done: usize, total: usize },
    /// What config.toml says about the agent, for the pickers.
    AgentConfig(perch_github::AgentConfig),
    /// Every thread on an item. Whole, like `Items`.
    CommentThreads { item_id: String, threads: Vec<CommentThread> },
    /// One step of a run, applied in place to the run it names.
    AgentStep { run_id: String, thread_id: String, event: perch_agent::RunEvent },
    Status(Status),

    /// A step of preparing a checkout.
    ReviewProgress { message: String, percent: Option<u8> },
    ReviewReady {
        item: Arc<Item>,
        /// `None` for a branch of the reader's own with no pull request.
        pull_request: Option<Arc<PullRequest>>,
        files: Arc<Vec<perch_git::ChangedFile>>,
        tree: Arc<Vec<perch_git::TreeEntry>>,
        /// The head the pull request moved to while the worktree had
        /// uncommitted changes. The files shown are the older ones on disk.
        stale_head: Option<String>,
        /// True when the pull request could not be fetched and the review
        /// is showing the checkout as it was last left.
        offline: bool,
    },
    ReviewFailed { message: String },
    FileLoaded { item_id: String, path: String, file: Arc<crate::review::model::LoadedFile> },
    FileFailed { path: String, message: String },

    /// Everything said on the pull request: from the cache first, then
    /// from GitHub.
    DiscussionReady { item_id: String, discussion: Arc<perch_github::Discussion> },
    /// An issue's comments, fetched again after a notification about it.
    IssueComments { item_id: String, comments: Arc<Vec<perch_github::IssueComment>> },
    /// The workflow runs' verdict for the item's head.
    ChecksReady { item_id: String, checks: Arc<perch_github::Checks> },
    /// The commits over the base and whether the checkout has work in it,
    /// for the scope menu, and the head as it was at the previous open,
    /// which "Since you last looked" ranges from.
    CommitsReady { item_id: String, commits: Arc<Vec<perch_git::Commit>>, dirt: perch_git::Dirt, since: Option<String> },
    /// The changed list under a scope, after `SetScope`.
    ScopeReady { item_id: String, scope: perch_git::Scope, files: Arc<Vec<perch_git::ChangedFile>> },
    /// Every (path, blob) the reader has viewed on the item. Whole.
    ViewedReady { item_id: String, viewed: Arc<std::collections::HashSet<(String, String)>> },

    LspStatus(perch_lsp::Status),
    LspHover { token: u64, text: String },
    /// A definition inside the worktree, or `None` when it lands outside it.
    LspDefinition { token: u64, path: Option<String>, line: u32 },
    /// Every use of a symbol inside the worktree, with the line's text, and
    /// how many uses lie outside it.
    LspReferences { token: u64, references: Vec<crate::review::model::Reference>, elsewhere: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// No usable token. The app shows setup instructions instead of a list.
    NoToken { detail: String },
    Refreshing,
    Idle { at: Timestamp, source: String },
    /// Network or API trouble. Any cached list stays on screen.
    Error { message: String },
    RateLimited { message: String },
}

/// Handle the UI keeps. Dropping it ends the worker.
pub struct Backend {
    commands: UnboundedSender<Command>,
}

impl Backend {
    pub fn refresh(&self) {
        let _ = self.commands.send(Command::Refresh);
    }

    /// Open a pull request or an issue by number, making its item if it has
    /// none yet. The launch hooks and the review header's "Closes #N" chip
    /// come through here; a sidebar row has an id and uses `open_item`.
    pub fn open_subject(&self, repo_full_name: &str, kind: ItemKind, number: u64) {
        let _ = self.commands.send(Command::OpenSubject {
            repo_full_name: repo_full_name.to_string(),
            kind,
            number,
        });
    }

    pub fn open_item(&self, id: &str) {
        let _ = self.commands.send(Command::OpenItem { id: id.to_string() });
    }

    pub fn open_file(&self, path: &str) {
        let _ = self.commands.send(Command::OpenFile { path: path.to_string() });
    }

    pub fn set_scope(&self, item_id: &str, scope: perch_git::Scope) {
        let _ = self.commands.send(Command::SetScope { item_id: item_id.to_string(), scope });
    }

    pub fn refresh_scope(&self, item_id: &str) {
        let _ = self.commands.send(Command::RefreshScope { item_id: item_id.to_string() });
    }

    pub fn set_viewed(&self, item_id: &str, path: &str, blob: &str, viewed: bool) {
        let _ = self.commands.send(Command::SetViewed {
            item_id: item_id.to_string(),
            path: path.to_string(),
            blob: blob.to_string(),
            viewed,
        });
    }

    pub fn references(&self, token: u64, path: &str, line: u32, character: u32) {
        let _ = self.commands.send(Command::References { token, path: path.to_string(), line, character });
    }

    pub fn close_review(&self) {
        let _ = self.commands.send(Command::CloseReview);
    }

    pub fn archive_item(&self, id: &str) {
        let _ = self.commands.send(Command::ArchiveItem { id: id.to_string() });
    }

    pub fn restore_item(&self, id: &str) {
        let _ = self.commands.send(Command::RestoreItem { id: id.to_string() });
    }

    pub fn delete_item(&self, id: &str) {
        let _ = self.commands.send(Command::DeleteItem { id: id.to_string() });
    }

    pub fn mark_item(&self, id: &str, saved: Option<bool>, done: Option<bool>) {
        let _ = self.commands.send(Command::MarkItem { id: id.to_string(), saved, done });
    }

    pub fn pin_item(&self, id: &str, pinned: bool) {
        let _ = self.commands.send(Command::PinItem { id: id.to_string(), pinned });
    }

    pub fn mark_unread(&self, id: &str) {
        let _ = self.commands.send(Command::MarkUnread { id: id.to_string() });
    }

    pub fn rename_item(&self, id: &str, name: Option<String>) {
        let _ = self.commands.send(Command::RenameItem { id: id.to_string(), name });
    }

    pub fn start_work(&self, issue_item_id: &str) {
        let _ = self.commands.send(Command::StartWork { issue_item_id: issue_item_id.to_string() });
    }

    pub fn fork_item(&self, id: &str) {
        let _ = self.commands.send(Command::ForkItem { id: id.to_string() });
    }

    pub fn set_meta(&self, key: &str, value: &str) {
        let _ = self.commands.send(Command::SetMeta { key: key.to_string(), value: value.to_string() });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ask(
        &self,
        item_id: &str,
        thread_id: Option<String>,
        anchor: Option<Anchor>,
        question: &str,
        backend: perch_agent::Backend,
        model: &str,
        effort: perch_agent::Effort,
    ) {
        let _ = self.commands.send(Command::Ask {
            item_id: item_id.to_string(),
            thread_id,
            anchor,
            question: question.to_string(),
            backend,
            model: model.to_string(),
            effort,
        });
    }

    pub fn cancel_run(&self, run_id: &str) {
        let _ = self.commands.send(Command::CancelRun { run_id: run_id.to_string() });
    }

    pub fn delete_comment_thread(&self, id: &str) {
        let _ = self.commands.send(Command::DeleteCommentThread { id: id.to_string() });
    }

    pub fn hover(&self, token: u64, path: &str, line: u32, character: u32) {
        let _ = self.commands.send(Command::Hover {
            token,
            path: path.to_string(),
            line,
            character,
        });
    }

    pub fn definition(&self, token: u64, path: &str, line: u32, character: u32) {
        let _ = self.commands.send(Command::Definition {
            token,
            path: path.to_string(),
            line,
            character,
        });
    }
}

impl Backend {
    /// A handle with nobody behind it: every command is dropped. For
    /// headless tests, which must not open the reader's database or talk
    /// to GitHub.
    #[cfg(test)]
    pub fn detached() -> Self {
        let (commands, _) = tokio::sync::mpsc::unbounded_channel();
        Backend { commands }
    }
}

/// Start the worker thread. Returns the command handle and the event receiver.
pub fn spawn() -> (Backend, std::sync::mpsc::Receiver<Event>) {
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (evt_tx, evt_rx) = std::sync::mpsc::channel();

    std::thread::Builder::new()
        .name("perch-backend".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = evt_tx.send(Event::Status(Status::Error {
                        message: format!("could not start the network runtime: {e}"),
                    }));
                    return;
                }
            };
            runtime.block_on(run(cmd_rx, evt_tx));
        })
        .expect("spawn backend thread");

    (Backend { commands: cmd_tx }, evt_rx)
}

async fn run(mut commands: UnboundedReceiver<Command>, events: EventSender<Event>) {
    tracing::info!("backend started");
    let mut config = Config::load();
    tracing::info!(poll = config.poll_interval_secs, "config loaded");

    // Which agents are installed, asked alongside the GitHub setup rather
    // than before it: Codex's catalog can take a few seconds on a cold
    // cache, and the sidebar should not wait for a picker.
    let agents = {
        let mut agent = config.agent.clone();
        let events = events.clone();
        tokio::spawn(async move {
            for problem in crate::ask::discover(&mut agent).await {
                tracing::warn!(%problem, "agent discovery");
            }
            tracing::info!(claude = %agent.binary, codex = ?agent.codex.models, "agents found");
            if agent.models.is_empty() && agent.codex.models.is_empty() {
                let _ = events.send(Event::Notice(
                    "No coding agent found. Install Claude Code or Codex, or set agent.binary in config.toml.".into(),
                ));
            }
            let _ = events.send(Event::AgentConfig(agent.clone()));
            agent
        })
    };

    let Some(token) = resolve_token(&config) else {
        tracing::warn!("no token resolved");
        let _ = events.send(Event::Status(Status::NoToken {
            detail: "Set GITHUB_TOKEN, or run `gh auth login` in a terminal.".into(),
        }));
        return;
    };

    let github = match GitHub::new(&token.value) {
        Ok(g) => Arc::new(g),
        Err(e) => {
            let _ = events.send(Event::Status(Status::Error {
                message: format!("could not build the GitHub client: {e}"),
            }));
            return;
        }
    };

    tracing::info!("github client ready");
    let (cache, reset) = open_cache();
    tracing::info!(cache = cache.is_some(), reset, "cache opened");
    if reset {
        let _ = events.send(Event::Notice(
            "The database was from an older Perch and has been reset; open a pull request to check it out again".into(),
        ));
    }
    for warning in &config.warnings {
        let _ = events.send(Event::Notice(warning.clone()));
    }
    // The sidebar's order, before the items it orders.
    let _ = events.send(Event::Repos(config.declared()));
    if let Some(cache) = cache.as_ref() {
        // A run still marked running is one Perch died in the middle of.
        if let Ok(guard) = cache.lock() {
            match guard.fail_running_runs("Perch quit") {
                Ok(n) if n > 0 => tracing::info!(n, "marked interrupted runs failed"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "could not clean up runs"),
            }
        }
    }

    let avatars = Arc::new(Semaphore::new(AVATAR_CONCURRENCY));

    // Paint from the cache first. A cold launch with no network still shows
    // the sidebar as it was, with the settings it was left with.
    if let Some(cache) = cache.as_ref() {
        if let Ok(guard) = cache.lock() {
            send_items(&guard, &events);
            for key in UI_META_KEYS {
                if let Ok(Some(value)) = guard.meta(key) {
                    let _ = events.send(Event::Meta { key: key.to_string(), value });
                }
            }
        }
    }
    let source = token.source.describe().to_string();

    // The review side of the app. Long operations run as their own tasks so a
    // clone never stalls the notification poll, and the active checkout is
    // shared so a file open knows where to read from.
    let preparer = Arc::new(Preparer::new(
        Store::new(
            perch_github::config::data_dir().unwrap_or_else(|| std::path::PathBuf::from(".")),
        ),
        Some(token.value.clone()),
    ));
    let checkout: Arc<std::sync::Mutex<Option<ReviewContext>>> =
        Arc::new(std::sync::Mutex::new(None));
    let language_server: Arc<tokio::sync::Mutex<Option<Arc<perch_lsp::Client>>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    let knowledge = Arc::new(perch_knowledge::Knowledge::open(config.knowledge.root_path()));
    tracing::info!(root = %knowledge.root().display(), "knowledge root");
    // The runs spawn what discovery found, so the shared config carries
    // its answer. A discovery that panicked leaves the file's own words.
    if let Ok(agent) = agents.await {
        config.agent = agent;
    }
    let config = Arc::new(config);
    let ctx = Ctx {
        github: github.clone(),
        preparer: preparer.clone(),
        cache: cache.clone(),
        config: config.clone(),
        knowledge,
        slot: checkout.clone(),
        language_server: language_server.clone(),
        events: events.clone(),
        runs: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        avatars,
        seen_ci: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        refreshing: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        passes: Arc::new(tokio::sync::Notify::new()),
    };

    // Who the reader is on GitHub, for their own byline: once per launch,
    // kept in meta so the next launch has it before the network does.
    {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            match ctx.github.viewer().await {
                Ok(user) if !user.login.is_empty() => {
                    for (key, value) in [("viewer.login", &user.login), ("viewer.avatar_url", &user.avatar_url)] {
                        ctx.with_cache(|c| c.set_meta(key, value));
                        let _ = ctx.events.send(Event::Meta { key: key.into(), value: value.clone() });
                    }
                    ctx.want_avatars(std::iter::once(user.avatar_url));
                }
                Ok(_) => {}
                Err(e) => tracing::info!(error = %e, "could not ask who the token belongs to"),
            }
        });
    }

    // Bring the table and the disk into agreement before any command can
    // make a directory. Inline, not spawned: a checkout made by a launch
    // hook while this ran would have no row yet, and would be taken for a
    // stray and removed from under it. It is a directory listing and at
    // most a prune, so the first poll is not kept waiting.
    reconcile_checkouts(&ctx, reset).await;
    {
        // Then put away whatever nobody has opened in a fortnight. Archived,
        // not deleted: the branch and the row stay, so restoring is a
        // checkout. Only owned checkouts are touched, so this can run
        // beside whatever the reader does next.
        let ctx = ctx.clone();
        tokio::spawn(async move { sweep_items(&ctx).await });
    }

    // The roots inline as well, before the first list pass: the pass gives
    // a pull request to a checkout of the reader's own on its head branch
    // only when the branch row is already there, and a scan spawned beside
    // the pass would lose that race for every branch it had not reached,
    // leaving a row for the pull request and a row for the branch. The
    // sidebar has painted from the cache already, and a scan is git over
    // a few directories. The lookups behind subject-less branches are
    // requests, and go off the loop.
    if scan_roots(&ctx).await {
        ctx.send_items();
    }
    {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            if attach_pull_requests(&ctx).await {
                ctx.send_items();
            }
        });
    }

    // A refresh happens immediately, then on whatever cadence the server asks
    // for. A command from the UI short-circuits the wait.
    let mut poll_after = Duration::from_secs(0);
    // Whether the session's first list passes have run. The backfill
    // starts after them and not before: it reads the rows they make.
    let mut first_pass_done = false;

    loop {
        let command = tokio::select! {
            _ = tokio::time::sleep(poll_after) => Some(Command::Refresh),
            cmd = commands.recv() => match cmd {
                Some(cmd) => Some(cmd),
                // The UI is gone.
                None => return,
            },
        };

        if let Some(c) = &command {
            if !matches!(c, Command::Refresh | Command::Hover { .. }) {
                tracing::debug!(command = ?c, "command");
            }
        }
        match command {
            Some(Command::Refresh) | None => {
                // The open lists before the notifications, so a notification
                // about a pull request the pass has just listed finds its
                // row. The passes are requests too, and the status bar says
                // so from the first of them, not from the poll after.
                let _ = ctx.events.send(Event::Status(Status::Refreshing));
                let pass_failure = list_passes(&ctx).await;
                if !first_pass_done {
                    first_pass_done = true;
                    // Once, after the rows it reads exist. Its handle is
                    // dropped, never awaited, so the loop is not held by
                    // it; the permit sent next is what starts its first
                    // round.
                    backfill::spawn(ctx.clone());
                }
                ctx.passes.notify_one();
                poll_after = refresh(&ctx, &source).await;
                // A pass that failed is what the status bar shows, whatever
                // the poll said: the sidebar is missing rows, and `refresh`
                // has just reported a healthy poll over it.
                if let Some((repo, e)) = pass_failure {
                    let _ = ctx.events.send(Event::Status(describe_pass_error(&repo, &e)));
                }
                let open = ctx.slot.lock().ok().and_then(|g| g.as_ref().map(|c| c.item_id.clone()));
                if let Some(item_id) = open {
                    let ctx = ctx.clone();
                    tokio::spawn(async move { refresh_scope(&ctx, &item_id).await });
                }
                let ctx = ctx.clone();
                tokio::spawn(async move { refresh_items(&ctx).await });
            }
            Some(Command::OpenSubject { repo_full_name, kind, number }) => {
                ctx.clear_notice();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if open_subject(&ctx, repo_full_name, kind, number).await {
                        start_language_server(ctx.slot, ctx.language_server, ctx.events).await;
                    }
                });
            }
            Some(Command::OpenItem { id }) | Some(Command::RestoreItem { id }) => {
                // Restoring is opening: the archived flag clears on the way
                // through and an owned checkout is put back by `prepare`.
                ctx.clear_notice();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if open_item(&ctx, &id).await {
                        start_language_server(ctx.slot, ctx.language_server, ctx.events).await;
                    }
                });
            }
            Some(Command::ArchiveItem { id }) => {
                ctx.clear_notice();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if let Err(message) = archive_item(&ctx, &id).await {
                        tracing::warn!(id, %message, "could not archive item");
                        let _ = ctx.events.send(Event::Notice(message));
                    }
                });
            }
            Some(Command::DeleteItem { id }) => {
                ctx.clear_notice();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if let Err(message) = delete_item(&ctx, &id).await {
                        tracing::warn!(id, %message, "could not delete item");
                        let _ = ctx.events.send(Event::Notice(message));
                    }
                });
            }
            Some(Command::MarkItem { id, saved, done }) => {
                edit_item(&ctx, &id, |item| {
                    if let Some(saved) = saved {
                        item.saved = saved;
                    }
                    match done {
                        // Done keeps the attention it answered, so newer
                        // attention can reopen the item and older cannot.
                        Some(true) => item.done_at = Some(item.attention_at.unwrap_or_else(Timestamp::now)),
                        Some(false) => item.done_at = None,
                        None => {}
                    }
                });
            }
            Some(Command::PinItem { id, pinned }) => {
                edit_item(&ctx, &id, |item| {
                    item.pinned_at = pinned.then(Timestamp::now);
                });
            }
            Some(Command::MarkUnread { id }) => {
                ctx.with_cache(|c| c.touch_attention(&id, Timestamp::now(), "manual"));
                ctx.send_items();
            }
            Some(Command::RenameItem { id, name }) => {
                edit_item(&ctx, &id, |item| {
                    item.name = name.clone().filter(|n| !n.trim().is_empty());
                });
            }
            Some(Command::StartWork { issue_item_id }) => {
                ctx.clear_notice();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    match start_work(&ctx, &issue_item_id).await {
                        Ok(id) => {
                            if open_item(&ctx, &id).await {
                                start_language_server(ctx.slot, ctx.language_server, ctx.events).await;
                            }
                        }
                        Err(message) => {
                            let _ = ctx.events.send(Event::ReviewFailed { message });
                        }
                    }
                });
            }
            Some(Command::ForkItem { id }) => {
                ctx.clear_notice();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    match fork_item(&ctx, &id).await {
                        Ok(id) => {
                            if open_item(&ctx, &id).await {
                                start_language_server(ctx.slot, ctx.language_server, ctx.events).await;
                            }
                        }
                        Err(message) => {
                            let _ = ctx.events.send(Event::ReviewFailed { message });
                        }
                    }
                });
            }
            Some(Command::SetMeta { key, value }) => {
                ctx.with_cache(|c| c.set_meta(&key, &value));
            }
            Some(Command::Ask { item_id, thread_id, anchor, question, backend, model, effort }) => {
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    ask(&ctx, item_id, thread_id, anchor, question, backend, model, effort).await;
                });
            }
            Some(Command::CancelRun { run_id }) => {
                if let Ok(runs) = ctx.runs.lock() {
                    if let Some((run, _)) = runs.get(&run_id) {
                        run.cancel();
                    }
                }
            }
            Some(Command::DeleteCommentThread { id }) => {
                let ctx = ctx.clone();
                tokio::spawn(async move { delete_comment_thread(&ctx, &id).await });
            }
            Some(Command::OpenFile { path }) => {
                let preparer = preparer.clone();
                let checkout = checkout.clone();
                let events = events.clone();
                let language_server = language_server.clone();
                tokio::spawn(async move {
                    open_file(preparer, checkout.clone(), events, path.clone()).await;
                    announce_file(checkout, language_server, path).await;
                });
            }
            Some(Command::Hover { token, path, line, character }) => {
                let checkout = checkout.clone();
                let language_server = language_server.clone();
                let events = events.clone();
                tokio::spawn(async move {
                    let Some((client, full)) =
                        resolve(&checkout, &language_server, &path).await
                    else {
                        return;
                    };
                    if let Some(hover) = client.hover(&full, line, character).await {
                        let text = render_hover(&hover);
                        if !text.trim().is_empty() {
                            let _ = events.send(Event::LspHover { token, text });
                        }
                    }
                });
            }
            Some(Command::SetScope { item_id, scope }) => {
                let ctx = ctx.clone();
                tokio::spawn(async move { set_scope(&ctx, &item_id, scope).await });
            }
            Some(Command::RefreshScope { item_id }) => {
                let ctx = ctx.clone();
                tokio::spawn(async move { refresh_scope(&ctx, &item_id).await });
            }
            Some(Command::SetViewed { item_id, path, blob, viewed }) => {
                if let Some(Err(e)) = ctx.with_cache(|c| c.set_viewed(&item_id, &path, &blob, viewed)) {
                    tracing::warn!(error = %e, "could not record viewed");
                }
                send_viewed(&ctx, &item_id);
            }
            Some(Command::References { token, path, line, character }) => {
                let checkout = checkout.clone();
                let language_server = language_server.clone();
                let events = events.clone();
                tokio::spawn(async move {
                    let Some((client, full)) = resolve(&checkout, &language_server, &path).await else {
                        return;
                    };
                    let worktree = checkout
                        .lock()
                        .ok()
                        .and_then(|g| g.as_ref().map(|c| c.checkout.worktree.clone()));
                    let Some(worktree) = worktree else { return };
                    let locations = client.references(&full, line, character).await;
                    let (references, elsewhere) = read_references(&worktree, &locations).await;
                    let _ = events.send(Event::LspReferences { token, references, elsewhere });
                });
            }
            Some(Command::Definition { token, path, line, character }) => {
                let checkout = checkout.clone();
                let language_server = language_server.clone();
                let events = events.clone();
                tokio::spawn(async move {
                    let Some((client, full)) =
                        resolve(&checkout, &language_server, &path).await
                    else {
                        return;
                    };
                    let worktree = checkout
                        .lock()
                        .ok()
                        .and_then(|g| g.as_ref().map(|c| c.checkout.worktree.clone()));
                    let Some(worktree) = worktree else { return };

                    if let Some(location) = client.definition(&full, line, character).await.first()
                    {
                        // A definition inside the worktree opens as a tab; one
                        // in a dependency has no path the rail can show.
                        let target = perch_lsp::client::uri_to_path(&location.uri.to_string());
                        let relative = target
                            .and_then(|p| p.strip_prefix(&worktree).ok().map(|r| r.to_path_buf()))
                            .map(|r| r.to_string_lossy().into_owned());
                        let _ = events.send(Event::LspDefinition {
                            token,
                            path: relative,
                            line: location.range.start.line,
                        });
                    }
                });
            }
            Some(Command::CloseReview) => {
                if let Ok(mut guard) = checkout.lock() {
                    *guard = None;
                }
                let mut guard = language_server.lock().await;
                if let Some(client) = guard.take() {
                    // Let the server exit cleanly rather than leaving it
                    // indexing a worktree nobody is looking at.
                    tokio::spawn(async move { client.shutdown().await });
                }
            }
        }
    }
}

/// How long an owned checkout stays on disk without being opened before
/// the sweep archives its item.
const CHECKOUT_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// Everything an item operation needs, so a spawned task takes one clone
/// rather than six.
#[derive(Clone)]
struct Ctx {
    github: Arc<GitHub>,
    preparer: Arc<Preparer>,
    cache: SharedCache,
    config: Arc<Config>,
    knowledge: Arc<perch_knowledge::Knowledge>,
    slot: Arc<std::sync::Mutex<Option<ReviewContext>>>,
    language_server: Arc<tokio::sync::Mutex<Option<Arc<perch_lsp::Client>>>>,
    events: EventSender<Event>,
    /// Running agents by run id, so a cancel can find its child.
    runs: Arc<std::sync::Mutex<std::collections::HashMap<String, (perch_agent::Run, String)>>>,
    /// The avatar download limit, shared by everything that wants a face.
    avatars: Arc<Semaphore>,
    /// CI notifications already acted on, by id and time, so one GitHub
    /// keeps returning does not fetch the checks again on every poll.
    seen_ci: Arc<std::sync::Mutex<std::collections::HashSet<(String, Timestamp)>>>,
    /// Items whose GitHub side is being fetched again right now.
    refreshing: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Notified after every poll's list passes. The backfill sleeps on it
    /// between rounds, so it starts a round on the rows a pass just
    /// wrote rather than on a clock of its own. `notify_one` keeps a
    /// permit when nothing is waiting, so a round that ran long is woken
    /// as soon as it next waits and a pass is never missed.
    passes: Arc<tokio::sync::Notify>,
}

impl Ctx {
    /// Have these faces on hand, from disk or the network.
    fn want_avatars(&self, urls: impl IntoIterator<Item = String>) {
        let mut seen = std::collections::HashSet::new();
        for url in urls {
            if url.is_empty() || !seen.insert(url.clone()) {
                continue;
            }
            spawn_avatar(self.github.clone(), self.cache.clone(), self.events.clone(), self.avatars.clone(), url);
        }
    }

    fn send_threads(&self, item_id: &str) {
        let threads = self.with_cache(|c| c.load_comment_threads(item_id));
        match threads {
            Some(Ok(threads)) => {
                let _ = self.events.send(Event::CommentThreads {
                    item_id: item_id.to_string(),
                    threads,
                });
            }
            Some(Err(e)) => tracing::warn!(error = %e, "could not load threads"),
            None => {}
        }
    }

    /// Run one closure against the cache. The guard never crosses an await.
    fn with_cache<T>(&self, f: impl FnOnce(&Cache) -> T) -> Option<T> {
        let cache = self.cache.as_ref()?;
        let guard = cache.lock().ok()?;
        Some(f(&guard))
    }

    fn item(&self, id: &str) -> Option<Item> {
        self.with_cache(|c| c.load_item(id).ok().flatten()).flatten()
    }

    fn send_items(&self) {
        self.with_cache(|c| send_items(c, &self.events));
    }

    fn notice(&self, message: impl Into<String>) {
        let _ = self.events.send(Event::Notice(message.into()));
    }

    /// The reader is doing something new, so the last complaint has been
    /// read. An empty notice is how the UI hears that.
    fn clear_notice(&self) {
        let _ = self.events.send(Event::Notice(String::new()));
    }

    /// Whether the open review is this item.
    fn is_open(&self, item_id: &str) -> bool {
        self.slot
            .lock()
            .ok()
            .is_some_and(|g| g.as_ref().is_some_and(|c| c.item_id == item_id))
    }

    /// Drop the open review and stop its language server.
    async fn close_review(&self) {
        if let Ok(mut guard) = self.slot.lock() {
            *guard = None;
        }
        let client = self.language_server.lock().await.take();
        if let Some(client) = client {
            client.shutdown().await;
        }
    }
}

fn send_items(cache: &Cache, events: &EventSender<Event>) {
    match cache.load_items() {
        Ok(items) => {
            let _ = events.send(Event::Items(items));
        }
        Err(e) => tracing::warn!(error = %e, "could not read items"),
    }
}

/// Apply one change to an item's row and tell the UI.
fn edit_item(ctx: &Ctx, id: &str, edit: impl FnOnce(&mut Item)) {
    let Some(mut item) = ctx.item(id) else {
        ctx.notice("That item is gone");
        return;
    };
    edit(&mut item);
    if let Some(Err(e)) = ctx.with_cache(|c| c.update_item(&item)) {
        ctx.notice(format!("Could not save the change: {e}"));
        return;
    }
    ctx.send_items();
}

/// Make the items table and the disk agree about owned checkouts.
///
/// An owned checkout whose directory is gone is marked off disk; someone
/// deleted it by hand. A directory under the store's worktrees that no
/// item claims is removed and pruned: nothing can open it, and it is how
/// what an older schema left behind gets retired.
async fn reconcile_checkouts(ctx: &Ctx, reset: bool) {
    let Some(items) = ctx.with_cache(|c| c.load_items().ok()).flatten() else {
        return;
    };

    let mut changed = false;
    for item in &items {
        let Some(checkout) = &item.checkout else { continue };
        if !checkout.owned {
            continue;
        }
        let present = checkout.path.join(".git").exists();
        if checkout.on_disk != present {
            tracing::info!(id = %item.id, path = %checkout.path.display(), present, "checkout presence changed");
            let mut updated = item.clone();
            if let Some(c) = updated.checkout.as_mut() {
                c.on_disk = present;
            }
            ctx.with_cache(|c| c.update_item(&updated));
            changed = true;
        }
    }

    // Directories no row claims. After a reset they are every checkout
    // there was, and they stay: opening the pull request again finds its
    // directory where it left it. Otherwise a clean one is a leftover and
    // goes; one with uncommitted work in it is somebody's, whatever the
    // table says, and is left alone.
    if reset {
        if changed {
            ctx.send_items();
        }
        return;
    }
    let known: std::collections::HashSet<std::path::PathBuf> = items
        .iter()
        .filter_map(|i| i.checkout.as_ref().map(|c| c.path.clone()))
        .collect();
    let mut removed = false;
    for dir in ctx.preparer.worktree_dirs() {
        if known.contains(&dir) {
            continue;
        }
        if dir.join(".git").exists() {
            match ctx.preparer.is_dirty(&dir).await {
                Ok(false) => {}
                Ok(true) => {
                    tracing::info!(path = %dir.display(), "leaving an unclaimed worktree with changes in it");
                    continue;
                }
                Err(e) => {
                    tracing::warn!(path = %dir.display(), error = %e, "could not tell if an unclaimed worktree is dirty; leaving it");
                    continue;
                }
            }
        }
        tracing::info!(path = %dir.display(), "removing a worktree directory no item claims");
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(path = %dir.display(), error = %e, "could not remove it");
        }
        removed = true;
    }
    if removed {
        prune_every_clone(ctx).await;
    }

    if changed {
        ctx.send_items();
    }
}

/// `git worktree prune` in every bare clone, after directories were removed
/// without knowing which clone they belonged to.
async fn prune_every_clone(ctx: &Ctx) {
    let repos = ctx.preparer.store().root().join("repos").join("github.com");
    let Ok(owners) = std::fs::read_dir(repos) else { return };
    for owner in owners.flatten() {
        let Ok(clones) = std::fs::read_dir(owner.path()) else { continue };
        for clone in clones.flatten() {
            let path = clone.path();
            if path.join("HEAD").is_file() {
                let _ = ctx
                    .preparer
                    .git()
                    .run(Some(&path), ["worktree", "prune"], Duration::from_secs(30))
                    .await;
            }
        }
    }
}

/// Archive items whose owned checkout has sat unopened for too long. An
/// issue, or a checkout under a root, is archived only by hand.
///
/// Age is the row's `last_opened_at`, which is what the rule is about. A
/// directory's mtime is not: an agent reading it, or a fetch touching
/// `.git`, would keep a forgotten checkout young for ever.
async fn sweep_items(ctx: &Ctx) {
    let items = ctx.with_cache(|c| c.load_items().ok()).flatten().unwrap_or_default();
    let cutoff = Timestamp::now() - jiff::SignedDuration::try_from(CHECKOUT_MAX_AGE).unwrap_or_default();

    for item in &items {
        let owned = item.checkout.as_ref().is_some_and(|c| c.owned && c.on_disk);
        if !owned || item.is_archived() || item.last_opened_at > cutoff {
            continue;
        }
        match archive_item(ctx, &item.id).await {
            Ok(()) => tracing::info!(id = %item.id, "archived a stale checkout"),
            // Dirty, most likely. It stays; the reader has work in it.
            Err(message) => tracing::info!(id = %item.id, %message, "left a stale checkout alone"),
        }
    }
}

/// Leave Active. An owned checkout's directory goes; the branch, the row,
/// the threads and the knowledge stay.
async fn archive_item(ctx: &Ctx, id: &str) -> Result<(), String> {
    let Some(item) = ctx.item(id) else {
        return Err("That item is gone".into());
    };
    let Some(repo) = RepoId::parse(&item.repo_full_name) else {
        return Err(format!("{} is not a repository name", item.repo_full_name));
    };
    let mut removed = false;
    if let Some(checkout) = item.checkout.as_ref().filter(|c| c.owned && c.on_disk) {
        if checkout.path.join(".git").exists() {
            let dirty = ctx.preparer.is_dirty(&checkout.path).await.map_err(|e| e.to_string())?;
            if dirty {
                return Err(format!("{} has uncommitted changes", checkout.branch));
            }
            if ctx.is_open(id) {
                ctx.close_review().await;
            }
            let bare = ctx.preparer.store().bare_path(&repo);
            ctx.preparer
                .remove_worktree(&bare, &checkout.path)
                .await
                .map_err(|e| e.to_string())?;
        }
        removed = true;
    } else if ctx.is_open(id) {
        ctx.close_review().await;
    }
    // Written from the row as it is after the git work, not before.
    let mut item = ctx.item(id).unwrap_or(item);
    if removed {
        if let Some(c) = item.checkout.as_mut() {
            c.on_disk = false;
        }
    }
    item.archived_at = Some(Timestamp::now());
    ctx.with_cache(|c| c.update_item(&item))
        .transpose()
        .map_err(|e| e.to_string())?;
    ctx.send_items();
    Ok(())
}

/// Remove the row and its threads. An owned checkout's directory and
/// branch go too; a checkout under a root is not touched.
async fn delete_item(ctx: &Ctx, id: &str) -> Result<(), String> {
    let Some(item) = ctx.item(id) else {
        return Err("That item is gone".into());
    };
    if let Some(checkout) = item.checkout.as_ref().filter(|c| c.owned) {
        if let Some(repo) = RepoId::parse(&item.repo_full_name) {
            let bare = ctx.preparer.store().bare_path(&repo);
            if checkout.path.join(".git").exists() {
                let dirty = ctx.preparer.is_dirty(&checkout.path).await.map_err(|e| e.to_string())?;
                if dirty {
                    return Err(format!("{} has uncommitted changes", checkout.branch));
                }
                if ctx.is_open(id) {
                    ctx.close_review().await;
                }
                ctx.preparer
                    .remove_worktree(&bare, &checkout.path)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            if bare.join("HEAD").is_file() {
                // A branch that is already gone is not a reason to keep the row.
                if let Err(e) = ctx.preparer.delete_branch(&bare, &checkout.branch).await {
                    tracing::info!(id, error = %e, "branch was not there to delete");
                }
            }
        }
    } else if ctx.is_open(id) {
        ctx.close_review().await;
    }
    // Stop anything still running in its threads.
    let thread_ids: Vec<String> = ctx
        .with_cache(|c| c.load_comment_threads(id).unwrap_or_default())
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.id)
        .collect();
    if let Ok(runs) = ctx.runs.lock() {
        for (run, thread) in runs.values() {
            if thread_ids.contains(thread) {
                run.cancel();
            }
        }
    }
    ctx.with_cache(|c| c.delete_item(id))
        .transpose()
        .map_err(|e| e.to_string())?;
    ctx.send_items();
    Ok(())
}

/// Open whatever an item is.
async fn open_item(ctx: &Ctx, id: &str) -> bool {
    let Some(item) = ctx.item(id) else {
        let _ = ctx.events.send(Event::ReviewFailed { message: "That item is gone".into() });
        return false;
    };
    match (&item.subject, &item.checkout) {
        (Some(ItemSubject::PullRequest(pr)), _) => {
            open_subject(ctx, pr.repo_full_name.clone(), ItemKind::PullRequest, pr.number).await
        }
        (Some(ItemSubject::Issue(issue)), _) => {
            open_subject(ctx, issue.repo_full_name.clone(), ItemKind::Issue, issue.number).await
        }
        (None, Some(_)) => open_branch_item(ctx, item).await,
        (None, None) => {
            let _ = ctx.events.send(Event::ReviewFailed {
                message: "That item has nothing to open".into(),
            });
            false
        }
    }
}

/// A branch of the reader's own, with no pull request: the review view over
/// the diff against its base, wherever the checkout is.
async fn open_branch_item(ctx: &Ctx, item: Item) -> bool {
    let events = &ctx.events;
    let Some(checkout) = item.checkout.clone() else {
        let _ = events.send(Event::ReviewFailed { message: "That item has no checkout".into() });
        return false;
    };
    let Some(repo) = RepoId::parse(&item.repo_full_name) else {
        let _ = events.send(Event::ReviewFailed { message: format!("{} is not a repository name", item.repo_full_name) });
        return false;
    };
    let _ = events.send(Event::ReviewProgress { message: "Checking out the files".into(), percent: None });

    let git_checkout = if checkout.owned {
        match open_owned_branch(ctx, &repo, &checkout).await {
            Ok(c) => c,
            Err(e) => {
                let _ = events.send(Event::ReviewFailed { message: e.to_string() });
                return false;
            }
        }
    } else {
        match open_own_checkout(ctx, &repo, &checkout, &checkout.base_ref).await {
            Ok(c) => c,
            Err(e) => {
                let _ = events.send(Event::ReviewFailed { message: e.to_string() });
                return false;
            }
        }
    };

    // The row as it is now: the git work took a while, and a refresh may
    // have attached a pull request or a poll marked it since.
    let mut item = ctx.item(&item.id).unwrap_or(item);
    let since = item.checkout.as_ref().and_then(|c| c.last_seen_head.clone());
    if let Some(c) = item.checkout.as_mut() {
        // The scan moves head_sha with the worktree, so the previous
        // open's head is kept on its own, and replaced by this one's.
        c.last_seen_head = Some(git_checkout.head_sha.clone()).filter(|h| !h.is_empty());
        c.head_sha = git_checkout.head_sha.clone();
        c.on_disk = true;
    }
    item.archived_at = None;
    item.last_opened_at = Timestamp::now();
    ctx.with_cache(|c| c.update_item(&item));
    ctx.send_items();

    let pull_request = item.pull_request().cloned().map(Arc::new);
    finish_open(ctx, item, pull_request, git_checkout, false, since).await
}

/// A checkout under the store on a branch Perch cut: back on disk if it
/// was archived, then read as it is.
async fn open_owned_branch(
    ctx: &Ctx,
    repo: &RepoId,
    checkout: &ItemCheckout,
) -> Result<perch_git::Checkout, perch_git::Error> {
    let bare = ctx.preparer.ensure_bare(repo, |_| {}).await?;
    if !checkout.path.join(".git").exists() {
        // Archived: the branch is still in the bare clone.
        let path = checkout.path.to_string_lossy().into_owned();
        let _ = ctx.preparer.git().run(Some(&bare), ["worktree", "prune"], Duration::from_secs(30)).await;
        ctx.preparer
            .git()
            .run(Some(&bare), ["worktree", "add", &path, &checkout.branch], Duration::from_secs(60))
            .await?;
    }
    let git = ctx.preparer.git();
    let head_sha = git.run(Some(&checkout.path), ["rev-parse", "HEAD"], Duration::from_secs(30)).await?.trim().to_string();
    // The base: a branch name resolves through the home ref the last
    // ensure_home fetched; a fork's base is the source's commit itself.
    let base = if checkout.base_ref.len() == 40 && checkout.base_ref.chars().all(|c| c.is_ascii_hexdigit()) {
        checkout.base_ref.clone()
    } else {
        perch_git::home_ref(&checkout.base_ref)
    };
    let merge_base = git
        .run(Some(&bare), ["merge-base", &base, &head_sha], Duration::from_secs(30))
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| head_sha.clone());
    Ok(perch_git::Checkout {
        worktree: checkout.path.clone(),
        bare,
        branch: checkout.branch.clone(),
        head_sha,
        merge_base,
        stale: None,
    })
}

/// A checkout of the reader's own on a pull request's head branch, with
/// no subject yet, that the pull request can attach to. The rule is the
/// list pass's, in `sync`, since the pass attaches the same way.
fn branch_item_for(ctx: &Ctx, pr: &PullRequest) -> Option<Item> {
    ctx.with_cache(|c| sync::branch_item_for(c, pr)).flatten()
}

/// Write the row an open made or brought up to date. A row the open took
/// for new is looked for once more, under the lock it is written under:
/// the open found nothing, then fetched and cloned, and a list pass that
/// ran meanwhile may have made a row for the same subject. Inserted
/// beside it, the open's row would be refused by the subject index and
/// the review would run under an id no row has, its threads and marks
/// written to nothing. When a row has appeared, what the open set goes
/// onto it, and its id is the one the review runs as.
fn record_open(cache: &Cache, item: &mut Item, is_new: bool) -> Result<(), CacheError> {
    if !is_new {
        return cache.update_item(item);
    }
    let found = match item.subject.as_ref() {
        Some(s) => cache.find_item_by_subject(s.repo_full_name(), s.kind(), s.number())?,
        None => None,
    };
    let Some(mut found) = found else {
        return cache.insert_item(item);
    };
    tracing::info!(id = %found.id, "a pass made the row while the open ran; opening as that row");
    // The pass's subject may be the later one, and its tag vouches for it.
    let fetched = item.subject.take();
    let stored = found.subject.as_ref().and_then(ItemSubject::updated_at);
    if !sync::stored_is_newer(stored, fetched.as_ref().and_then(ItemSubject::updated_at)) {
        found.subject = fetched;
    }
    found.checkout = item.checkout.take();
    found.archived_at = None;
    found.last_opened_at = item.last_opened_at;
    if item.attention_at.is_some() {
        found.attention_at = item.attention_at;
        found.attention_reason = item.attention_reason.take();
    }
    cache.update_item(&found)?;
    *item = found;
    Ok(())
}

/// The branches items already hold in a repository, which a cut must not
/// take: `items_branch` allows one row per branch.
fn branches_in(ctx: &Ctx, repo_full_name: &str) -> Vec<String> {
    ctx.with_cache(|c| c.load_items().unwrap_or_default())
        .unwrap_or_default()
        .into_iter()
        .filter(|i| i.repo_full_name.eq_ignore_ascii_case(repo_full_name))
        .filter_map(|i| i.checkout.map(|c| c.branch))
        .collect()
}

/// Everything after a checkout is in hand: the file list over the branch,
/// the tree, the review context, `ReviewReady`, and then, each on its own
/// task so the code paints first, the threads, what is viewed, the
/// commits and the dirt for the scope menu, and for a pull request the
/// discussion and the checks: the cached copy first, then GitHub's.
async fn finish_open(
    ctx: &Ctx,
    item: Item,
    pull_request: Option<Arc<PullRequest>>,
    checkout: perch_git::Checkout,
    offline: bool,
    since: Option<String>,
) -> bool {
    let events = &ctx.events;
    let _ = events.send(Event::ReviewProgress { message: "Computing the diff".into(), percent: None });

    let scope = perch_git::Scope::Branch;
    let files = match scoped_files(&ctx.preparer, &checkout, &scope).await {
        Ok(f) => Arc::new(f),
        Err(e) => {
            let _ = events.send(Event::ReviewFailed { message: e.to_string() });
            return false;
        }
    };
    let worktree = checkout.worktree.clone();
    let tree = tokio::task::spawn_blocking(move || perch_git::list_tree(&worktree, 50_000).unwrap_or_default())
        .await
        .unwrap_or_default();

    let stale_head = checkout.stale.clone();
    let head = checkout.head_sha.clone();
    let merge_base = checkout.merge_base.clone();
    let worktree = checkout.worktree.clone();
    let item = Arc::new(item);
    if let Ok(mut guard) = ctx.slot.lock() {
        *guard = Some(ReviewContext {
            checkout,
            files: files.clone(),
            scope,
            since: since.clone(),
            scope_gen: next_scope_gen(),
            item_id: item.id.clone(),
            pull_request: pull_request.clone(),
            body: None,
            issue: None,
        });
    }
    tracing::info!(
        item = %item.id,
        files = files.len(),
        tree = tree.len(),
        worktree = %worktree.display(),
        stale = stale_head.is_some(),
        offline,
        "review ready"
    );
    let _ = events.send(Event::ReviewReady {
        item: item.clone(),
        pull_request: pull_request.clone(),
        files,
        tree: Arc::new(tree),
        stale_head,
        offline,
    });

    // What was viewed is small and local; it goes straight away.
    send_viewed(ctx, &item.id);

    // The outdated check is a git call per moved thread; the log and the
    // status are two more. Off the open, on their own tasks.
    {
        let ctx = ctx.clone();
        let item_id = item.id.clone();
        let (head, worktree) = (head.clone(), worktree.clone());
        tokio::spawn(async move {
            mark_outdated_threads(&ctx, &item_id, &head, &worktree).await;
            ctx.send_threads(&item_id);
        });
    }
    {
        let ctx = ctx.clone();
        let item_id = item.id.clone();
        let head = head.clone();
        tokio::spawn(async move { send_commits(&ctx, &item_id, &worktree, &merge_base, &head, since).await });
    }

    if let Some(pr) = pull_request {
        // The cached discussion and checks paint before GitHub answers,
        // with the faces the last open left on disk and the description a
        // question needs. A checkout with no pull request has neither.
        let cached = ctx.with_cache(|c| c.load_discussion(&item.id).ok().flatten()).flatten();
        if let Some((_, d)) = cached {
            if let Ok(mut guard) = ctx.slot.lock() {
                if let Some(open) = guard.as_mut().filter(|c| c.item_id == item.id) {
                    open.body = Some(d.body.clone());
                }
            }
            ctx.want_avatars(d.comments.iter().map(|c| c.user_avatar_url.clone()));
            let _ = ctx.events.send(Event::DiscussionReady { item_id: item.id.clone(), discussion: Arc::new(d) });
        }
        // The author's face is on the header, whether or not they said
        // anything in the discussion.
        ctx.want_avatars(
            std::iter::once(pr.author_avatar_url.clone())
                .chain(pr.requested_reviewers.iter().map(|r| r.avatar_url.clone())),
        );
        let cached = ctx.with_cache(|c| c.load_checks(&item.id, &pr.head_sha).ok().flatten()).flatten();
        if let Some(checks) = cached {
            let _ = ctx.events.send(Event::ChecksReady { item_id: item.id.clone(), checks: Arc::new(checks) });
        }
        if !offline {
            let (ctx2, item_id2, pr2) = (ctx.clone(), item.id.clone(), pr.clone());
            tokio::spawn(async move { archive_discussion(&ctx2, &item_id2, &pr2).await });
            let (ctx2, item_id2) = (ctx.clone(), item.id.clone());
            tokio::spawn(async move { fetch_checks(&ctx2, &item_id2, &pr, None).await });
        }
    }
    true
}

/// The changed list over a scope, in the worktree. Every scope produces
/// the same shape, and nothing downstream knows which made it.
async fn scoped_files(
    preparer: &Preparer,
    checkout: &perch_git::Checkout,
    scope: &perch_git::Scope,
) -> Result<Vec<perch_git::ChangedFile>, perch_git::Error> {
    perch_git::changed_files(preparer.git(), &checkout.worktree, scope, &checkout.merge_base, &checkout.head_sha).await
}

/// Every (path, blob) the reader has viewed on an item.
fn send_viewed(ctx: &Ctx, item_id: &str) {
    let viewed = ctx.with_cache(|c| c.load_viewed(item_id).unwrap_or_default()).unwrap_or_default();
    let _ = ctx.events.send(Event::ViewedReady { item_id: item_id.to_string(), viewed: Arc::new(viewed) });
}

/// The commits over the base and whether there is work in the checkout,
/// for the scope menu, with the head as it was at the previous open. A
/// checkout with no history over its base, an issue's home say, sends an
/// empty list.
async fn send_commits(ctx: &Ctx, item_id: &str, worktree: &Path, merge_base: &str, head: &str, since: Option<String>) {
    let git = ctx.preparer.git();
    let commits = if merge_base.is_empty() || merge_base == head {
        Vec::new()
    } else {
        match perch_git::log(git, worktree, merge_base, head).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(item = item_id, error = %e, "could not list the commits");
                Vec::new()
            }
        }
    };
    let dirt = perch_git::dirt(git, worktree).await.unwrap_or_default();
    let _ = ctx.events.send(Event::CommitsReady { item_id: item_id.to_string(), commits: Arc::new(commits), dirt, since });
}

/// Look at the checkout again. The head may have moved under a terminal
/// commit: then the review context follows it and the changed list is
/// recomputed under the scope that is set; the commits and the dirt are
/// read again either way.
async fn refresh_scope(ctx: &Ctx, item_id: &str) {
    let open = ctx.slot.lock().ok().and_then(|g| {
        g.as_ref()
            .filter(|c| c.item_id == item_id)
            .map(|c| (c.checkout.clone(), c.scope.clone(), c.since.clone(), c.scope_gen))
    });
    let Some((mut checkout, scope, since, gen)) = open else { return };
    if !checkout.worktree.join(".git").exists() {
        return;
    }
    let git = ctx.preparer.git();
    let head = git
        .run(Some(&checkout.worktree), ["rev-parse", "HEAD"], Duration::from_secs(30))
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    // A head that moved is a commit the reader made: the branch grew, or
    // was rebased, so the base is found again too. A pull request checkout
    // left stale by a dirty tree is not this.
    if !head.is_empty() && head != checkout.head_sha && checkout.stale.is_none() {
        checkout.head_sha = head.clone();
        if let Some(item) = ctx.item(item_id) {
            for base in base_revs_for(&item) {
                if let Ok(mb) = git.run(Some(&checkout.worktree), ["merge-base", &base, &head], Duration::from_secs(30)).await {
                    let mb = mb.trim().to_string();
                    if !mb.is_empty() {
                        checkout.merge_base = mb;
                        break;
                    }
                }
            }
        }
        let files = match scoped_files(&ctx.preparer, &checkout, &scope).await {
            Ok(f) => Arc::new(f),
            Err(e) => {
                tracing::info!(item = item_id, error = %e, "could not reread the changed list");
                return;
            }
        };
        // The reader may have picked a scope while this looked; theirs
        // wins, and the next look reads under it.
        let written = ctx.slot.lock().ok().is_some_and(|mut guard| {
            match guard.as_mut().filter(|c| c.item_id == item_id && c.scope_gen == gen) {
                Some(open) => {
                    open.checkout.head_sha = head.clone();
                    open.checkout.merge_base = checkout.merge_base.clone();
                    open.files = files.clone();
                    true
                }
                None => false,
            }
        });
        if !written {
            return;
        }
        // The row's head is what the view derives anchors and the menu
        // from, so it follows too.
        if let Some(mut item) = ctx.item(item_id) {
            if let Some(c) = item.checkout.as_mut() {
                c.head_sha = head.clone();
            }
            ctx.with_cache(|c| c.update_item(&item));
            ctx.send_items();
        }
        let _ = ctx.events.send(Event::ScopeReady { item_id: item_id.to_string(), scope, files });
    }
    send_commits(ctx, item_id, &checkout.worktree, &checkout.merge_base, &checkout.head_sha, since).await;
}

/// A number no earlier review context or scope pick has had, so a look at
/// the checkout that began under one cannot write into the next.
fn next_scope_gen() -> u64 {
    static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// The revisions an item's diff may be taken against, as the opens find
/// them, best first: a root checkout's origin branch, since it is the
/// reader's own clone and has none of Perch's refs; for a checkout in the
/// store, a pull request's fetched base, then the home ref or the commit
/// the branch was forked from. The first that resolves is the one.
fn base_revs_for(item: &Item) -> Vec<String> {
    let Some(checkout) = item.checkout.as_ref() else { return Vec::new() };
    let base = &checkout.base_ref;
    let is_sha = base.len() == 40 && base.chars().all(|c| c.is_ascii_hexdigit());
    if !checkout.owned {
        return vec![format!("origin/{base}"), base.clone()];
    }
    let mut out = Vec::new();
    if let Some(pr) = item.pull_request() {
        out.push(format!("refs/perch/base/{}", pr.number));
    }
    out.push(if is_sha { base.clone() } else { perch_git::home_ref(base) });
    out
}

/// One request for the workflow runs on the pull request's head, kept and
/// shown. The pull request's, not the checkout's: a root or a branch of
/// the reader's own may sit ahead of or behind what GitHub ran. A failure
/// shows nothing: absent means unknown.
///
/// `ci_at` is the time of the CI notification this fetch answers, when it
/// answers one. The checks coming back red is then the item's attention,
/// dated to the notification rather than to now, so the same one GitHub
/// returns again next poll cannot move it again. An open, and a
/// notification about something said, pass `None`: red checks the reader
/// is already looking at are not news, and a comment's reason is its own.
async fn fetch_checks(ctx: &Ctx, item_id: &str, pr: &PullRequest, ci_at: Option<Timestamp>) {
    let head = pr.head_sha.as_str();
    match ctx.github.list_workflow_runs_for_sha(&pr.repo_full_name, head).await {
        Ok(runs) => {
            let checks = perch_github::Checks::of(&runs);
            let failing = matches!(checks.state, perch_github::CheckState::Failing { .. });
            ctx.with_cache(|c| c.store_checks(item_id, head, &checks));
            if ctx.is_open(item_id) {
                let _ = ctx.events.send(Event::ChecksReady { item_id: item_id.to_string(), checks: Arc::new(checks) });
            }
            if let Some(at) = ci_at.filter(|_| failing) {
                if let Some(Ok(true)) = ctx.with_cache(|c| c.touch_attention(item_id, at, "ci_failed")) {
                    tracing::info!(item = item_id, "the checks failed, and the item is attention");
                    ctx.send_items();
                }
            }
        }
        Err(e) => tracing::info!(item = item_id, error = %e, "could not fetch the checks"),
    }
}

/// The scope changed: the changed list is recomputed under it, kept in the
/// review context for the file loads that follow, and sent.
async fn set_scope(ctx: &Ctx, item_id: &str, scope: perch_git::Scope) {
    let checkout = ctx.slot.lock().ok().and_then(|g| {
        g.as_ref().filter(|c| c.item_id == item_id).map(|c| c.checkout.clone())
    });
    let Some(checkout) = checkout else { return };
    match scoped_files(&ctx.preparer, &checkout, &scope).await {
        Ok(files) => {
            let files = Arc::new(files);
            if let Ok(mut guard) = ctx.slot.lock() {
                if let Some(open) = guard.as_mut().filter(|c| c.item_id == item_id) {
                    open.scope = scope.clone();
                    open.files = files.clone();
                    open.scope_gen = next_scope_gen();
                }
            }
            let _ = ctx.events.send(Event::ScopeReady { item_id: item_id.to_string(), scope, files });
        }
        Err(e) => ctx.notice(format!("Could not read that scope: {e}")),
    }
}

/// Cut `perch/issue-<n>` from the default branch and make the item for it,
/// or find the one that already has that branch.
async fn start_work(ctx: &Ctx, issue_item_id: &str) -> Result<String, String> {
    let Some(issue_item) = ctx.item(issue_item_id) else {
        return Err("That item is gone".into());
    };
    let Some(issue) = issue_item.issue() else {
        return Err("Start work needs an issue".into());
    };
    let Some(repo) = RepoId::parse(&issue_item.repo_full_name) else {
        return Err(format!("{} is not a repository name", issue_item.repo_full_name));
    };
    let wanted = format!("perch/issue-{}", issue.number);
    if let Some(existing) = ctx
        .with_cache(|c| c.find_item_by_branch(&issue_item.repo_full_name, &wanted).ok().flatten())
        .flatten()
    {
        return Ok(existing.id);
    }

    let _ = ctx.events.send(Event::ReviewProgress { message: "Fetching the default branch".into(), percent: None });
    let progress = ctx.events.clone();
    let label = repo.full_name();
    ctx.preparer
        .ensure_bare(&repo, move |step| {
            if let perch_git::Step::Cloning { phase, percent } = step {
                let _ = progress.send(Event::ReviewProgress { message: format!("Cloning {label}: {phase}"), percent });
            }
        })
        .await
        .map_err(|e| e.to_string())?;
    let default = ctx.preparer.default_branch(&repo).await.map_err(|e| e.to_string())?;
    ctx.preparer.ensure_home(&repo, &default).await.map_err(|e| e.to_string())?;
    let cut = ctx
        .preparer
        .cut_branch(&repo, &wanted, &perch_git::home_ref(&default), &branches_in(ctx, &issue_item.repo_full_name))
        .await
        .map_err(|e| e.to_string())?;
    let head_sha = ctx
        .preparer
        .git()
        .run(Some(&cut.worktree), ["rev-parse", "HEAD"], Duration::from_secs(30))
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let now = Timestamp::now();
    let mut item = Item::new(&issue_item.repo_full_name, now);
    item.checkout = Some(ItemCheckout {
        branch: cut.branch,
        base_ref: default,
        head_sha,
        path: cut.worktree,
        owned: true,
        on_disk: true,
        churn: Some((0, 0)),
        last_seen_head: None,
    });
    item.from_item_id = Some(issue_item.id.clone());
    ctx.with_cache(|c| c.insert_item(&item)).transpose().map_err(|e| e.to_string())?;
    ctx.send_items();
    Ok(item.id)
}

/// Cut a new branch from an item's head into a new checkout-only item.
async fn fork_item(ctx: &Ctx, id: &str) -> Result<String, String> {
    let Some(source) = ctx.item(id) else {
        return Err("That item is gone".into());
    };
    if source.is_archived() {
        return Err("Restore the item before forking it".into());
    }
    let Some(checkout) = source.checkout.as_ref() else {
        return Err("Fork needs a checkout; open the pull request first".into());
    };
    let Some(repo) = RepoId::parse(&source.repo_full_name) else {
        return Err(format!("{} is not a repository name", source.repo_full_name));
    };
    ctx.preparer.ensure_bare(&repo, |_| {}).await.map_err(|e| e.to_string())?;
    // A root's branch may not be in the store's clone; fetch its head
    // there by commit so the cut has something to start from.
    let head = if checkout.owned {
        checkout.head_sha.clone()
    } else {
        let git = ctx.preparer.git();
        // The root may have been switched since the row was made; a fork
        // of another branch under this one's name would be a lie.
        let on_branch = git
            .run(Some(&checkout.path), ["rev-parse", "--abbrev-ref", "HEAD"], Duration::from_secs(30))
            .await
            .map_err(|e| e.to_string())?;
        if on_branch.trim() != checkout.branch {
            return Err(format!("{} has {} checked out now, not {}", checkout.path.display(), on_branch.trim(), checkout.branch));
        }
        let sha = git
            .run(Some(&checkout.path), ["rev-parse", "HEAD"], Duration::from_secs(30))
            .await
            .map_err(|e| e.to_string())?
            .trim()
            .to_string();
        let bare = ctx.preparer.store().bare_path(&repo);
        let from = checkout.path.to_string_lossy().into_owned();
        git.run(Some(&bare), ["fetch", "--no-tags", &from, &sha], Duration::from_secs(120))
            .await
            .map_err(|e| e.to_string())?;
        sha
    };
    // The source's own name is spoken for by the source; the cut takes
    // the next one, whether or not this clone has heard of it.
    let cut = ctx
        .preparer
        .cut_branch(&repo, &checkout.branch, &head, &branches_in(ctx, &source.repo_full_name))
        .await
        .map_err(|e| e.to_string())?;

    let now = Timestamp::now();
    let mut item = Item::new(&source.repo_full_name, now);
    item.checkout = Some(ItemCheckout {
        branch: cut.branch,
        // The source's head, so what is shown is what has been done on top.
        base_ref: head.clone(),
        head_sha: head,
        path: cut.worktree,
        owned: true,
        on_disk: true,
        churn: Some((0, 0)),
        last_seen_head: None,
    });
    item.from_item_id = Some(source.id.clone());
    ctx.with_cache(|c| c.insert_item(&item)).transpose().map_err(|e| e.to_string())?;
    ctx.send_items();
    Ok(item.id)
}

/// A file larger than this opens as a notice rather than as text.
const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

/// What an open review needs in order to read and diff its files.
struct ReviewContext {
    checkout: perch_git::Checkout,
    /// The changed list under `scope`.
    files: Arc<Vec<perch_git::ChangedFile>>,
    /// What the diff is over. Branch when the item opens.
    scope: perch_git::Scope,
    /// The head at the previous open, for "Since you last looked".
    since: Option<String>,
    /// Rises with every scope pick, so a look at the checkout that began
    /// under an earlier scope does not write its list over the reader's.
    scope_gen: u64,
    item_id: String,
    /// `None` for a branch with no pull request.
    pull_request: Option<Arc<PullRequest>>,
    /// The description, once the discussion has been fetched.
    body: Option<String>,
    /// The issue and its comments, for a question about an issue.
    issue: Option<Arc<(perch_github::Issue, Vec<perch_github::IssueComment>)>>,
}

/// One list pass over each declared repository, in config order, which is
/// the order the sidebar shows them in. A repository whose pass failed is
/// logged and the next is tried: one being unreachable says nothing about
/// the rest, and a pass that stopped part way has usually written rows
/// before it did. The first failure is returned for the status bar, with
/// the repository whose pass it was: a pass has as many endpoints as
/// there are declared repositories, and GitHub's answer names none of
/// them.
async fn list_passes(ctx: &Ctx) -> Option<(String, sync::Error)> {
    let mut first_failure = None;
    for repo in ctx.config.declared() {
        match sync::list_pass_for(ctx, &repo).await {
            Ok(report) if !report.is_empty() => tracing::info!(
                repo,
                added = report.added.len(),
                updated = report.updated.len(),
                closed = report.closed.len(),
                "list pass"
            ),
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(repo, error = %e, "list pass failed");
                if first_failure.is_none() {
                    first_failure = Some((repo, e));
                }
            }
        }
    }
    first_failure
}

/// One poll of the notifications API. Returns how long to wait before the next.
///
/// One request: the first page, every time. The notifications live for
/// the length of `attend` and are dropped; nothing is stored, so nothing
/// is compared against last time. `X-Poll-Interval` still paces the next.
async fn refresh(ctx: &Ctx, source: &str) -> Duration {
    let events = &ctx.events;
    let config = ctx.config.as_ref();
    let _ = events.send(Event::Status(Status::Refreshing));

    match ctx.github.list_threads().await {
        Ok(result) => {
            let poll_after = result
                .poll_interval
                .max(Duration::from_secs(config.poll_interval_secs.max(1)));
            tracing::info!(count = result.threads.len(), "notifications polled");

            // A notification about an item is the item's attention, when
            // its reason asks for the reader.
            attend(ctx, &result.threads).await;

            let _ = events.send(Event::Status(Status::Idle {
                at: Timestamp::now(),
                source: source.to_string(),
            }));
            poll_after
        }
        Err(e) => {
            tracing::warn!(error = %e, "list_threads failed");
            let status = describe_error(&e);
            let wait = if let perch_github::Error::RateLimited { reset_epoch } = &e {
                backoff_until(*reset_epoch)
            } else {
                Duration::from_secs(config.poll_interval_secs.max(30))
            };
            let _ = events.send(Event::Status(status));
            wait
        }
    }
}

/// Set attention on the items these notifications are about, for the
/// reasons that ask for the reader.
///
/// A notification makes no item. The list pass has made the row for
/// everything open in a declared repository, and a subject with no row is
/// let go. Nor is every reason attention: subscribed activity and a state
/// change are the repository moving, which the pass already shows by
/// moving the row, and putting every push on every watched pull request
/// into Priority would be noise. A CI notification
/// fetches the open pull request's checks again instead, and it is the
/// checks coming back red that is the attention.
async fn attend(ctx: &Ctx, threads: &[Thread]) {
    let mut changed = false;
    for thread in threads {
        // A CI notification for the open pull request's branch is the one
        // time the checks are asked about again.
        if thread.subject.kind == SubjectKind::CheckSuite {
            refresh_open_checks(ctx, thread).await;
            continue;
        }
        let kind = match thread.subject.kind {
            SubjectKind::PullRequest => ItemKind::PullRequest,
            SubjectKind::Issue => ItemKind::Issue,
            _ => continue,
        };
        let Some(number) = thread.number_hint() else { continue };
        let repo = thread.repository.full_name.as_str();
        // An archived item does not claim its notifications; what the
        // reader put away stays put away.
        let item = ctx
            .with_cache(|c| c.find_item_by_subject(repo, kind, number).ok().flatten())
            .flatten()
            .filter(|i| !i.is_archived());
        let Some(item) = item else { continue };

        if thread.reason.makes_attention() {
            let reason = thread.reason.wire();
            if let Some(Ok(true)) =
                ctx.with_cache(|c| c.touch_attention(&item.id, thread.updated_at, reason))
            {
                changed = true;
            }
        }
        // Something was said about the item that is open: its Conversation
        // follows, whatever the reason. Once per notification, as with CI.
        if ctx.is_open(&item.id) {
            let key = (thread.id.clone(), thread.updated_at);
            let fresh = ctx.seen_ci.lock().map(|mut s| s.insert(key)).unwrap_or(false);
            if fresh {
                let ctx = ctx.clone();
                let item_id = item.id.clone();
                tokio::spawn(async move { refresh_open_item(&ctx, &item_id, None).await });
            }
        }
    }
    if changed {
        ctx.send_items();
    }
}

/// Fetch again what the open item shows from GitHub: the pull request and
/// its discussion and checks, or the issue's comments. `ci_at` is the CI
/// notification's time when that is what prompted the fetch; see
/// `fetch_checks`.
async fn refresh_open_item(ctx: &Ctx, item_id: &str, ci_at: Option<Timestamp>) {
    // One at a time per item: two notifications in one poll would fetch
    // and archive the same discussion twice, side by side.
    let taken = ctx.refreshing.lock().map(|mut s| s.insert(item_id.to_string())).unwrap_or(false);
    if !taken {
        return;
    }
    refresh_open_item_inner(ctx, item_id, ci_at).await;
    if let Ok(mut s) = ctx.refreshing.lock() {
        s.remove(item_id);
    }
}

async fn refresh_open_item_inner(ctx: &Ctx, item_id: &str, ci_at: Option<Timestamp>) {
    let open = ctx.slot.lock().ok().and_then(|g| {
        g.as_ref().filter(|c| c.item_id == item_id).map(|c| (c.pull_request.clone(), c.issue.clone()))
    });
    let Some((pull_request, issue)) = open else { return };
    if let Some(pr) = pull_request {
        // The snapshot first, so the checks are for the head GitHub ran.
        let pr = match ctx.github.fetch_pull_request(&pr.repo_full_name, pr.number).await {
            Ok(fresh) => {
                let fresh = Arc::new(fresh);
                if let Ok(mut guard) = ctx.slot.lock() {
                    if let Some(open) = guard.as_mut().filter(|c| c.item_id == item_id) {
                        open.pull_request = Some(fresh.clone());
                    }
                }
                if let Some(mut item) = ctx.item(item_id) {
                    // Unless a pass stored a later one while this was
                    // asked, and its tag now vouches for the row.
                    let stored = item.subject.as_ref().and_then(ItemSubject::updated_at);
                    if !sync::stored_is_newer(stored, fresh.updated_at) {
                        item.subject = Some(ItemSubject::PullRequest((*fresh).clone()));
                        ctx.with_cache(|c| c.update_item(&item));
                        ctx.send_items();
                    }
                }
                fresh
            }
            Err(e) => {
                tracing::info!(item = item_id, error = %e, "could not fetch the pull request again");
                pr
            }
        };
        archive_discussion(ctx, item_id, &pr).await;
        fetch_checks(ctx, item_id, &pr, ci_at).await;
    } else if let Some(issue) = issue {
        let (issue, _) = &*issue;
        match ctx.github.fetch_issue_comments(&issue.repo_full_name, issue.number).await {
            Ok(comments) => {
                keep_issue_comments(ctx, item_id, issue, &comments);
                // A question asked next is prompted with these.
                if let Ok(mut guard) = ctx.slot.lock() {
                    if let Some(open) = guard.as_mut().filter(|c| c.item_id == item_id) {
                        open.issue = Some(Arc::new((issue.clone(), comments.clone())));
                    }
                }
                if ctx.is_open(item_id) {
                    ctx.want_avatars(comments.iter().map(|c| c.user_avatar_url.clone()));
                    let _ = ctx.events.send(Event::IssueComments { item_id: item_id.to_string(), comments: Arc::new(comments) });
                }
            }
            Err(e) => tracing::info!(item = item_id, error = %e, "could not fetch the comments again"),
        }
    }
}

/// An issue's comments are kept the way a pull request's discussion is,
/// in the same table, so the next open paints them before the network
/// answers and the harness can print them.
fn keep_issue_comments(ctx: &Ctx, item_id: &str, issue: &perch_github::Issue, comments: &[perch_github::IssueComment]) {
    ctx.with_cache(|c| c.store_discussion(item_id, &issue_discussion(issue, comments)));
}

/// An issue and its comments in the shape a pull request's discussion
/// has, which is the one shape the table keeps.
pub(super) fn issue_discussion(issue: &perch_github::Issue, comments: &[perch_github::IssueComment]) -> perch_github::Discussion {
    perch_github::Discussion {
        opened_at: issue.created_at,
        body: issue.body.clone(),
        comments: comments.to_vec(),
        reviews: Vec::new(),
        review_comments: Vec::new(),
        events: Vec::new(),
    }
}

/// Fetch the checks again when a CI notification names the open pull
/// request's repository and head branch, and make the item attention,
/// reason `ci_failed`, when they come back red. Only the open pull
/// request's: a run on any other branch is a row the list pass moves,
/// and the reader will see the checks pill when they open it.
async fn refresh_open_checks(ctx: &Ctx, thread: &Thread) {
    let Some(query) = perch_github::WorkflowRunQuery::from_thread(thread) else { return };
    let open = ctx.slot.lock().ok().and_then(|g| {
        g.as_ref().and_then(|c| c.pull_request.clone().map(|p| (c.item_id.clone(), p)))
    });
    let Some((item_id, pr)) = open else { return };
    if !(pr.repo_full_name.eq_ignore_ascii_case(&query.repo_full_name) && pr.head_ref == query.branch) {
        return;
    }
    // The snapshot is refetched on the way, so the checks are for the head
    // the run was on, not the one the item opened at. Once per
    // notification, not once per poll GitHub still returns it, and off
    // the poll, which the command loop waits on.
    let key = (thread.id.clone(), thread.updated_at);
    if ctx.seen_ci.lock().map(|mut s| !s.insert(key)).unwrap_or(true) {
        return;
    }
    let ctx = ctx.clone();
    let at = thread.updated_at;
    tokio::spawn(async move { refresh_open_item(&ctx, &item_id, Some(at)).await });
}

/// Rescan roots, then ask GitHub about subject-less branches.
async fn refresh_items(ctx: &Ctx) {
    if scan_roots(ctx).await {
        ctx.send_items();
    }
    if attach_pull_requests(ctx).await {
        ctx.send_items();
    }
}

/// Every worktree under a declared repository's roots is an item, and one
/// whose directory has gone is deleted, or archived when it has threads.
/// Returns whether anything changed.
async fn scan_roots(ctx: &Ctx) -> bool {
    let git = ctx.preparer.git();
    let mut changed = false;
    // Which unowned items were seen, by directory and branch, so the rest
    // can be judged gone. A root that could not be scanned takes its
    // repository out of the judgement: its worktrees are not in any other
    // root's listing when it is a clone of its own, and which repository
    // it was for is exactly what the scan would have said.
    let mut seen: std::collections::HashSet<(std::path::PathBuf, String)> = std::collections::HashSet::new();
    let mut scanned_repos: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut failed_repos: std::collections::HashSet<String> = std::collections::HashSet::new();

    for declared in &ctx.config.repo {
        for root in declared.root_paths() {
            let scan = match perch_git::scan_root(git, &root).await {
                Ok(s) => s,
                Err(e) => {
                    ctx.notice(format!("{}: {e}", root.display()));
                    failed_repos.insert(declared.name.to_lowercase());
                    continue;
                }
            };
            let Some(repo) = scan.repo_full_name.clone() else {
                ctx.notice(format!("{}: origin is not a GitHub repository", root.display()));
                continue;
            };
            // A root belongs to the one repository its origin names.
            if !repo.eq_ignore_ascii_case(&declared.name) {
                ctx.notice(format!("{}: origin is {repo}, not {}", root.display(), declared.name));
                continue;
            }
            scanned_repos.insert(repo.to_lowercase());
            for entry in &scan.entries {
                let Some(branch) = entry.branch.clone() else { continue };
                if branch == scan.default_branch || entry.prunable || !entry.path.join(".git").exists() {
                    continue;
                }
                let path = entry.path.canonicalize().unwrap_or_else(|_| entry.path.clone());
                seen.insert((path.clone(), branch.clone()));
                let churn = perch_git::branch_churn(git, &entry.path, &format!("origin/{}", scan.default_branch))
                    .await
                    .ok();
                let existing = ctx.with_cache(|c| c.find_item_by_branch(&repo, &branch).ok().flatten()).flatten();
                match existing {
                    Some(mut item) => {
                        let Some(c) = item.checkout.as_mut() else { continue };
                        if c.owned {
                            continue;
                        }
                        let moved = c.head_sha != entry.head_sha || !c.on_disk || c.churn != churn || c.path != path;
                        // A row the scan put away, because the branch was
                        // not checked out anywhere, comes back when it is.
                        // One the reader archived keeps its directory in
                        // the row and stays put.
                        let returned = item.archived_at.is_some() && !c.on_disk;
                        if moved {
                            c.head_sha = entry.head_sha.clone();
                            c.on_disk = true;
                            c.churn = churn;
                            c.path = path;
                            if returned {
                                item.archived_at = None;
                            }
                            ctx.with_cache(|cache| cache.update_item(&item));
                            changed = true;
                        }
                    }
                    None => {
                        let mut item = Item::new(&repo, Timestamp::now());
                        item.checkout = Some(ItemCheckout {
                            branch,
                            base_ref: scan.default_branch.clone(),
                            head_sha: entry.head_sha.clone(),
                            path,
                            owned: false,
                            on_disk: true,
                            churn,
                            last_seen_head: None,
                        });
                        if let Some(Err(e)) = ctx.with_cache(|c| c.insert_item(&item)) {
                            tracing::warn!(error = %e, "could not record a root worktree");
                        } else {
                            changed = true;
                        }
                    }
                }
            }
        }
    }

    // Unowned items in scanned repositories whose branch no worktree has.
    // A directory that is gone takes its row with it, unless the row has
    // threads, which are worth an archived row. A directory still there on
    // another branch is the reader switching; the row is put away with its
    // directory marked gone, and the scan brings it back when the branch
    // is checked out again. A row the reader archived is theirs, and is
    // left exactly as it is.
    let items = ctx.with_cache(|c| c.load_items().unwrap_or_default()).unwrap_or_default();
    for item in items {
        let Some(c) = item.checkout.as_ref() else { continue };
        if c.owned
            || item.is_archived()
            || failed_repos.contains(&item.repo_full_name.to_lowercase())
            || !scanned_repos.contains(&item.repo_full_name.to_lowercase())
        {
            continue;
        }
        let path = c.path.canonicalize().unwrap_or_else(|_| c.path.clone());
        if seen.contains(&(path, c.branch.clone())) {
            continue;
        }
        let still_there = c.path.join(".git").exists();
        let has_threads = ctx
            .with_cache(|cache| cache.load_comment_threads(&item.id).map(|t| !t.is_empty()).unwrap_or(false))
            .unwrap_or(false);
        if still_there || has_threads {
            let mut item = item;
            if let Some(c) = item.checkout.as_mut() {
                c.on_disk = false;
            }
            item.archived_at = Some(Timestamp::now());
            ctx.with_cache(|cache| cache.update_item(&item));
        } else {
            ctx.with_cache(|cache| cache.delete_item(&item.id));
        }
        changed = true;
    }
    changed
}

/// For each item with a branch and no subject, ask GitHub whether a pull
/// request has been opened on it, conditionally. When one has, the item
/// becomes that pull request's, in the same row.
async fn attach_pull_requests(ctx: &Ctx) -> bool {
    let items = ctx.with_cache(|c| c.load_items().unwrap_or_default()).unwrap_or_default();
    let mut changed = false;
    for item in items {
        if item.subject.is_some() || item.is_archived() {
            continue;
        }
        let Some(checkout) = item.checkout.as_ref() else { continue };
        let Some((head_owner, _)) = item.repo_full_name.split_once('/') else { continue };
        // The pull request may be against any declared repository when the
        // checkout is a fork of one. The lookups are conditional, so a
        // repository asked every poll is one request that mostly says 304.
        let mut candidates: Vec<String> = vec![item.repo_full_name.clone()];
        for r in ctx.config.declared() {
            if !candidates.iter().any(|c| c.eq_ignore_ascii_case(&r)) {
                candidates.push(r);
            }
        }
        for repo in candidates {
            let key = format!("pull_lookup_etag:{}:{}:{}", repo.to_lowercase(), head_owner, checkout.branch);
            let etag = ctx.with_cache(|c| c.meta(&key).ok().flatten()).flatten();
            match ctx.github.find_pull_for_branch(&repo, head_owner, &checkout.branch, etag.as_deref()).await {
                Ok(perch_github::PullLookup::Unchanged) => {}
                Ok(perch_github::PullLookup::Answer { pull, etag }) => {
                    // `head=owner:branch` matches a branch of that name on
                    // any of the owner's repositories with a pull request
                    // here; only one on this item's repository is this
                    // branch.
                    // A head with no repository is a deleted fork, which
                    // cannot be a checkout of the reader's.
                    let pull = pull.filter(|pr| {
                        pr.head_repo_full_name
                            .as_deref()
                            .is_some_and(|h| h.eq_ignore_ascii_case(&item.repo_full_name))
                    });
                    let Some(pr) = pull else {
                        if let Some(etag) = etag {
                            ctx.with_cache(|c| c.set_meta(&key, &etag));
                        }
                        continue;
                    };
                    // The row as it is now, not as it was when the loop
                    // began: a click may have opened it since.
                    let Some(mut item) = ctx.item(&item.id) else { break };
                    if item.subject.is_some() {
                        break;
                    }
                    let number = pr.number;
                    match ctx.with_cache(|c| take_pull_request(c, &mut item, *pr)) {
                        Some(Ok(true)) => {
                            tracing::info!(item = %item.id, number, "branch gained a pull request");
                            if let Some(etag) = etag {
                                ctx.with_cache(|c| c.set_meta(&key, &etag));
                            }
                            changed = true;
                        }
                        // Held by a row with a checkout of its own: that
                        // row keeps it, and this one stays a branch.
                        Some(Ok(false)) => {
                            if let Some(etag) = etag {
                                ctx.with_cache(|c| c.set_meta(&key, &etag));
                            }
                        }
                        Some(Err(e)) => tracing::warn!(error = %e, "could not attach the pull request"),
                        None => {}
                    }
                    break;
                }
                Err(e) => {
                    tracing::info!(item = %item.id, repo, error = %e, "pull request lookup failed");
                    break;
                }
            }
        }
    }
    changed
}

/// Give a branch row its pull request, under the one lock. `Ok(false)`
/// when another row has the pull request and a checkout of its own, one
/// opened in Perch before the branch was checked out in a root: that row
/// keeps it, and the branch stays a branch. A row with the subject and
/// nothing else is the list pass's, made when the pass listed the pull
/// request before the scan had seen its branch, at launch or in the poll
/// after the branch was pushed; the branch row is the reader's, with its
/// threads, so it takes the subject and the pass's row goes, its marks
/// carried across. The subject index allows one row per pull request, so
/// the pass's row goes first.
fn take_pull_request(cache: &Cache, item: &mut Item, pr: PullRequest) -> Result<bool, CacheError> {
    let holder = cache
        .find_item_by_subject(&pr.repo_full_name, ItemKind::PullRequest, pr.number)?
        .filter(|other| other.id != item.id);
    if let Some(holder) = holder {
        if holder.checkout.is_some() {
            return Ok(false);
        }
        tracing::info!(item = %item.id, from = %holder.id, number = pr.number, "the branch takes the pull request from the row the pass made");
        item.saved |= holder.saved;
        if item.pinned_at.is_none() {
            item.pinned_at = holder.pinned_at;
        }
        if item.done_at.is_none() {
            item.done_at = holder.done_at;
        }
        if item.attention_at.is_none() {
            item.attention_at = holder.attention_at;
            item.attention_reason = holder.attention_reason;
        }
        if item.name.is_none() {
            item.name = holder.name;
        }
        cache.delete_item(&holder.id)?;
    }
    if let Some(c) = item.checkout.as_mut() {
        c.base_ref = pr.base_ref.clone();
    }
    item.subject = Some(ItemSubject::PullRequest(pr));
    cache.update_item(item)?;
    Ok(true)
}

/// Find or make the item for a pull request or issue, and open it.
async fn open_subject(ctx: &Ctx, repo_full_name: String, kind: ItemKind, number: u64) -> bool {
    match kind {
        ItemKind::PullRequest => open_pull_request(ctx, repo_full_name, number).await,
        ItemKind::Issue => open_issue(ctx, repo_full_name, number).await,
    }
}

/// Fetch an issue and its comments, store the snapshot on its item, archive
/// the discussion, and show the issue view. The agent's questions about it
/// run in the repository's home checkout. Returns false always: there is
/// no language server to start for an issue.
async fn open_issue(ctx: &Ctx, repo_full_name: String, number: u64) -> bool {
    let events = &ctx.events;
    let _ = events.send(Event::ReviewProgress { message: "Reading the issue".into(), percent: None });

    let existing = ctx
        .with_cache(|c| c.find_item_by_subject(&repo_full_name, ItemKind::Issue, number).ok().flatten())
        .flatten();

    // The comments as they were last kept, for offline and for a first
    // paint that does not wait on the network.
    let kept: Vec<perch_github::IssueComment> = existing
        .as_ref()
        .and_then(|i| ctx.with_cache(|c| c.load_discussion(&i.id).ok().flatten()).flatten())
        .map(|(_, d)| d.comments)
        .unwrap_or_default();
    let (issue, comments, offline) = match ctx.github.fetch_issue(&repo_full_name, number).await {
        Ok(issue) => {
            let comments = match ctx.github.fetch_issue_comments(&repo_full_name, number).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(repo = %repo_full_name, number, error = %e, "could not fetch the comments");
                    kept.clone()
                }
            };
            (issue, comments, false)
        }
        Err(e) => match existing.as_ref().and_then(|i| i.issue().cloned()) {
            Some(snapshot) => {
                tracing::info!(repo = %repo_full_name, number, error = %e, "showing the issue from its snapshot");
                ctx.notice("Offline; showing the issue as it was last fetched");
                (snapshot, kept.clone(), true)
            }
            None => {
                let _ = events.send(Event::ReviewFailed { message: e.to_string() });
                return false;
            }
        },
    };

    let now = Timestamp::now();
    let is_new = existing.is_none();
    // The row as it is now, not as it was before the fetch.
    let mut item = match existing {
        Some(item) => ctx.item(&item.id).unwrap_or(item),
        None => Item::new(&issue.repo_full_name, now),
    };
    item.subject = Some(ItemSubject::Issue(issue.clone()));
    item.archived_at = None;
    item.last_opened_at = now;
    let written = ctx.with_cache(|c| record_open(c, &mut item, is_new));
    if let Some(Err(e)) = written {
        tracing::warn!(id = %item.id, error = %e, "could not record the item");
    }
    ctx.send_items();

    // Where a question about it runs: the home checkout, made if needed.
    let home = match RepoId::parse(&issue.repo_full_name) {
        Some(repo) if !offline => {
            let progress = events.clone();
            let label = repo.full_name();
            let cloned = ctx
                .preparer
                .ensure_bare(&repo, move |step| {
                    if let perch_git::Step::Cloning { phase, percent } = step {
                        let _ = progress.send(Event::ReviewProgress { message: format!("Cloning {label}: {phase}"), percent });
                    }
                })
                .await;
            match cloned {
                Ok(bare) => match ctx.preparer.default_branch(&repo).await {
                    Ok(default) => match ctx.preparer.ensure_home(&repo, &default).await {
                        Ok(path) => Some((bare, path, default)),
                        Err(e) => {
                            tracing::warn!(error = %e, "no home checkout");
                            None
                        }
                    },
                    Err(e) => {
                        tracing::warn!(error = %e, "no default branch");
                        None
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "no clone for the issue's repository");
                    None
                }
            }
        }
        Some(repo) => {
            let bare = ctx.preparer.store().bare_path(&repo);
            let home = ctx.preparer.store().home_path(&repo);
            home.join(".git").exists().then(|| (bare, home, String::from("HEAD")))
        }
        None => None,
    };
    let checkout = match home {
        Some((bare, worktree, branch)) => {
            let head_sha = ctx
                .preparer
                .git()
                .run(Some(&worktree), ["rev-parse", "HEAD"], Duration::from_secs(30))
                .await
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            perch_git::Checkout { worktree, bare, branch, merge_base: head_sha.clone(), head_sha, stale: None }
        }
        None => {
            // No checkout at all: questions cannot run, but the issue can
            // be read. A directory that does not exist keeps `ask` honest.
            let missing = ctx.preparer.store().root().join("homes").join("missing");
            perch_git::Checkout {
                worktree: missing.clone(),
                bare: missing,
                branch: String::new(),
                head_sha: String::new(),
                merge_base: String::new(),
                stale: None,
            }
        }
    };

    let item = Arc::new(item);
    let issue = Arc::new(issue);
    let comments = Arc::new(comments);
    if let Ok(mut guard) = ctx.slot.lock() {
        *guard = Some(ReviewContext {
            checkout,
            files: Arc::new(Vec::new()),
            scope: perch_git::Scope::Branch,
            since: None,
            scope_gen: next_scope_gen(),
            item_id: item.id.clone(),
            pull_request: None,
            body: None,
            issue: Some(Arc::new(((*issue).clone(), (*comments).clone()))),
        });
    }
    tracing::info!(repo = %issue.repo_full_name, number, comments = comments.len(), offline, "issue ready");
    if !offline {
        keep_issue_comments(ctx, &item.id, &issue, &comments);
    }
    let _ = events.send(Event::IssueReady { item: item.clone(), issue: issue.clone(), comments: comments.clone(), offline });
    ctx.want_avatars(
        std::iter::once(issue.author_avatar_url.clone())
            .chain(comments.iter().map(|c| c.user_avatar_url.clone())),
    );
    ctx.send_threads(&item.id);

    if !offline {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let messages = crate::knowledge::issue_to_messages(&issue, &comments);
            match ingest(&ctx, messages).await {
                Ok(n) => {
                    let _ = ctx.events.send(Event::Archived {
                        repo_full_name: issue.repo_full_name.clone(),
                        kind: ItemKind::Issue,
                        number,
                        added: n,
                    });
                }
                Err(e) => ctx.notice(format!("Could not archive the discussion: {e}")),
            }
        });
    }
    false
}

/// File records under the knowledge root and keep the skill current.
/// Returns how many records were new or changed.
async fn ingest(ctx: &Ctx, messages: Vec<perch_knowledge::Message>) -> Result<usize, String> {
    ingest_into(ctx.knowledge.clone(), ctx.config.knowledge.skills, messages).await
}

/// The same over the store alone, for the backfill, which has no
/// context. One ingest at a time across the process: an ingest reads a
/// repository's raw file, merges and writes it back, and two at once on
/// the same file would each write over what the other added. An open and
/// the backfill archive into the same file all the time.
pub(super) async fn ingest_into(
    knowledge: Arc<perch_knowledge::Knowledge>,
    write_skill: bool,
    messages: Vec<perch_knowledge::Message>,
) -> Result<usize, String> {
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // File writes, off the runtime's threads.
    let outcome = tokio::task::spawn_blocking(move || {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
        let before = knowledge.repos()?;
        let ingested = knowledge.ingest(&messages)?;
        let after = knowledge.repos()?;
        // The skill names every repository, so it is rewritten when the
        // set changes, and written the first time anything is here at all.
        let dirs = crate::knowledge::skill_dirs();
        let missing = dirs.iter().any(|d| !d.join("SKILL.md").is_file());
        if write_skill && (before != after || missing) {
            knowledge.write_skill(&perch_knowledge::SkillConfig { repos: after, dirs })?;
        }
        Ok::<_, perch_knowledge::Error>(ingested)
    })
    .await;
    match outcome {
        Ok(Ok(ingested)) => Ok(ingested.total()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(format!("archive task panicked: {e}")),
    }
}

/// Resolve a pull request, lay down a checkout on its branch, and report
/// its files. Returns whether a review is now open.
///
/// The item is found or made on the way, and every open refreshes what
/// its row knows. A pull request whose head branch matches a checkout of
/// the reader's own on the head repository attaches to that item rather
/// than making a second one.
///
/// An item that has a checkout keeps it, wherever it is and whatever
/// branch it is on: a root checkout is read where it stands, a branch
/// Perch cut for an issue is read as it is, and only the branch Perch cut
/// for the pull request itself is brought up to its head. A fresh item
/// gets `perch/pr-N`.
async fn open_pull_request(ctx: &Ctx, repo_full_name: String, number: u64) -> bool {
    let events = &ctx.events;
    macro_rules! fail {
        ($($arg:tt)*) => {{
            let message = format!($($arg)*);
            tracing::warn!(repo = %repo_full_name, number, %message, "could not open pull request");
            let _ = events.send(Event::ReviewFailed { message });
            return false;
        }};
    }

    let _ = events.send(Event::ReviewProgress {
        message: "Reading the pull request".into(),
        percent: None,
    });

    let existing = ctx
        .with_cache(|c| {
            c.find_item_by_subject(&repo_full_name, ItemKind::PullRequest, number)
                .ok()
                .flatten()
        })
        .flatten();

    let (pull_request, offline) = match ctx.github.fetch_pull_request(&repo_full_name, number).await {
        Ok(pr) => (pr, false),
        Err(e) => {
            // No network, or no GitHub. A checkout that is already on disk
            // opens anyway, as it was last left, from the snapshot.
            let on_disk = existing
                .as_ref()
                .and_then(|i| i.checkout.as_ref())
                .is_some_and(|c| c.path.join(".git").exists());
            match existing.as_ref().and_then(|i| i.pull_request().cloned()) {
                Some(snapshot) if on_disk => {
                    tracing::info!(repo = %repo_full_name, number, error = %e, "opening the checkout offline");
                    (snapshot, true)
                }
                _ => fail!("{e}"),
            }
        }
    };

    let Some(repo) = RepoId::parse(&pull_request.repo_full_name) else {
        fail!("{} is not a repository name", pull_request.repo_full_name)
    };

    // The item: the one for this pull request, else a checkout of the
    // reader's own on its head branch, else a new one.
    let existing = match existing {
        Some(item) => Some(item),
        None => branch_item_for(ctx, &pull_request),
    };
    let kept = existing.as_ref().and_then(|i| i.checkout.clone());
    // The clone a kept checkout belongs to is its item's repository, which
    // is the fork when the branch was cut there; a fresh checkout is cut
    // in the pull request's own.
    let clone = existing
        .as_ref()
        .and_then(|i| RepoId::parse(&i.repo_full_name))
        .unwrap_or_else(|| repo.clone());
    let pr_branch = format!("perch/pr-{number}");
    let branch = kept.as_ref().map(|c| c.branch.clone()).unwrap_or_else(|| pr_branch.clone());

    let checkout = match &kept {
        Some(own) if !own.owned => open_own_checkout(ctx, &clone, own, &pull_request.base_ref).await,
        _ if offline => {
            let path = kept.as_ref().map(|c| c.path.clone());
            reopen_from_disk(ctx, &clone, number, &branch, path).await
        }
        // Cut for something else, an issue most likely, and since pushed
        // as this pull request. Read as it is: its commits are the reader's.
        Some(owned) if owned.branch != pr_branch => {
            open_owned_branch_for_pull(ctx, &clone, &repo, owned, number, &pull_request.head_sha, &pull_request.base_ref)
                .await
        }
        _ => {
            let progress = events.clone();
            let label = repo.full_name();
            ctx.preparer
                .prepare(
                    &repo,
                    number,
                    &pull_request.head_sha,
                    &pull_request.base_ref,
                    &branch,
                    move |step| {
                        let (message, percent) = match step {
                            perch_git::Step::Cloning { phase, percent } => {
                                (format!("Cloning {label}: {phase}"), percent)
                            }
                            perch_git::Step::Fetching => ("Fetching the pull request".into(), None),
                            perch_git::Step::CreatingWorktree => ("Checking out the files".into(), None),
                            perch_git::Step::ComputingDiff => ("Computing the diff".into(), None),
                        };
                        let _ = progress.send(Event::ReviewProgress { message, percent });
                    },
                )
                .await
        }
    };

    let checkout = match checkout {
        Ok(c) => c,
        Err(e) => fail!("{e}"),
    };

    // The row. Every column that can move is rewritten from what just
    // happened, and an archived row is back in service. The row is read
    // again first: the network and git took a while, and a poll may have
    // marked it in the meantime.
    let now = Timestamp::now();
    let is_new = existing.is_none();
    let mut item = match existing {
        Some(item) => ctx.item(&item.id).unwrap_or(item),
        None => {
            // A fresh item's checkout is of the pull request's own
            // repository, whichever fork the head is on: that is the clone
            // it is cut in.
            Item::new(&pull_request.repo_full_name, now)
        }
    };
    // CI turning red is attention. Nothing else about the snapshot is.
    let ci_failed_now = pull_request.checks == Some(perch_github::CheckStatus::Failure)
        && item.pull_request().and_then(|p| p.checks) != Some(perch_github::CheckStatus::Failure);
    // The snapshot was fetched before the clone, and a list pass that ran
    // during the clone may have stored a later one and a tag vouching for
    // it. The pass's rule applies here too: what is newer stays.
    let stored = item.subject.as_ref().and_then(ItemSubject::updated_at);
    if !sync::stored_is_newer(stored, pull_request.updated_at) {
        item.subject = Some(ItemSubject::PullRequest(pull_request.clone()));
    }
    item.checkout = Some(ItemCheckout {
        branch: branch.clone(),
        base_ref: pull_request.base_ref.clone(),
        head_sha: checkout.head_sha.clone(),
        path: checkout.worktree.clone(),
        owned: kept.as_ref().is_none_or(|c| c.owned),
        on_disk: true,
        churn: kept.as_ref().and_then(|c| c.churn),
        // The head being opened now, which the next open will look back
        // to. What the row held until now is this open's "since".
        last_seen_head: Some(checkout.head_sha.clone()).filter(|h| !h.is_empty()),
    });
    let since = kept.as_ref().and_then(|c| c.last_seen_head.clone());
    item.archived_at = None;
    item.last_opened_at = now;
    if ci_failed_now {
        item.attention_at = Some(now);
        item.attention_reason = Some("ci_failed".into());
    }
    let written = ctx.with_cache(|c| record_open(c, &mut item, is_new));
    if let Some(Err(e)) = written {
        tracing::warn!(id = %item.id, error = %e, "could not record the item");
    }
    ctx.send_items();

    finish_open(ctx, item, Some(Arc::new(pull_request)), checkout, offline, since).await
}

/// A branch Perch cut, read against a pull request that was opened from
/// it. The pull request's refs are fetched so the base is known, the
/// directory comes back if it was archived, and nothing is reset: the
/// commits on it are the reader's, pushed or not. When the pull request
/// has moved past what is here, the review says so.
///
/// `clone` is the repository the checkout was cut in, and `upstream` the
/// one the pull request is on; when they differ the pull request's refs
/// are fetched across, so the diff, the base and the reader's unpushed
/// commits are all in one object store.
#[allow(clippy::too_many_arguments)]
async fn open_owned_branch_for_pull(
    ctx: &Ctx,
    clone: &RepoId,
    upstream: &RepoId,
    owned: &ItemCheckout,
    number: u64,
    pr_head_sha: &str,
    base_ref: &str,
) -> Result<perch_git::Checkout, perch_git::Error> {
    let events = &ctx.events;
    let _ = events.send(Event::ReviewProgress { message: "Fetching the pull request".into(), percent: None });
    let bare = ctx.preparer.ensure_bare(clone, |_| {}).await?;
    let from = if clone == upstream { "origin".to_string() } else { upstream.clone_url() };
    ctx.preparer.fetch_pull(&bare, &from, number, base_ref).await?;
    let _ = events.send(Event::ReviewProgress { message: "Checking out the files".into(), percent: None });
    let mut checkout = open_owned_branch(ctx, clone, owned).await?;
    let base = format!("refs/perch/base/{number}");
    checkout.merge_base = ctx
        .preparer
        .git()
        .run(Some(&bare), ["merge-base", &base, &checkout.head_sha], Duration::from_secs(30))
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or(checkout.merge_base);
    if checkout.head_sha != pr_head_sha {
        checkout.stale = Some(pr_head_sha.to_string());
    }
    Ok(checkout)
}

/// A checkout under one of the reader's roots, opened where it is.
///
/// Nothing is fetched into it and nothing is reset. The diff is against
/// the base as the checkout's own `origin` last saw it, and every git
/// command runs in the directory itself, which works in a worktree as it
/// does in a bare clone.
async fn open_own_checkout(
    ctx: &Ctx,
    _repo: &RepoId,
    own: &ItemCheckout,
    base_ref: &str,
) -> Result<perch_git::Checkout, perch_git::Error> {
    let git = ctx.preparer.git();
    let timeout = Duration::from_secs(30);
    if !own.path.join(".git").exists() {
        return Err(perch_git::Error::Io(format!("{} is not there any more", own.path.display())));
    }
    // The directory is the reader's to switch branches in, and the item
    // is about one branch. Showing another's files under its name would
    // be worse than saying so.
    let on_branch = git.run(Some(&own.path), ["rev-parse", "--abbrev-ref", "HEAD"], timeout).await?.trim().to_string();
    if on_branch != own.branch {
        return Err(perch_git::Error::Io(format!(
            "{} has {on_branch} checked out now, not {}",
            own.path.display(),
            own.branch
        )));
    }
    let head_sha = git.run(Some(&own.path), ["rev-parse", "HEAD"], timeout).await?.trim().to_string();
    let remote_base = format!("origin/{base_ref}");
    let base = match git.run(Some(&own.path), ["rev-parse", "--verify", "--quiet", &remote_base], timeout).await {
        Ok(sha) if !sha.trim().is_empty() => sha.trim().to_string(),
        _ => git.run(Some(&own.path), ["rev-parse", "--verify", "--quiet", base_ref], timeout).await?.trim().to_string(),
    };
    let merge_base = git
        .run(Some(&own.path), ["merge-base", &base, &head_sha], timeout)
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| head_sha.clone());
    Ok(perch_git::Checkout {
        worktree: own.path.clone(),
        bare: own.path.clone(),
        branch: own.branch.clone(),
        head_sha,
        merge_base,
        stale: None,
    })
}

/// Fetch what was said on a pull request and file it under the knowledge
/// root. A fetch error is a line in the status bar, not a failed review.
async fn archive_discussion(ctx: &Ctx, item_id: &str, pr: &perch_github::PullRequest) {
    let discussion = match ctx.github.fetch_discussion(&pr.repo_full_name, pr.number).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(repo = %pr.repo_full_name, number = pr.number, error = %e, "could not fetch the discussion");
            let _ = ctx.events.send(Event::Notice(format!("Could not fetch the discussion: {e}")));
            return;
        }
    };

    // Kept whole, so the next open paints it before the network answers,
    // and shown now if the item is still the one open.
    ctx.with_cache(|c| c.store_discussion(item_id, &discussion));
    if let Ok(mut guard) = ctx.slot.lock() {
        if let Some(open) = guard.as_mut().filter(|c| c.item_id == item_id) {
            open.body = Some(discussion.body.clone());
        }
    }
    if ctx.is_open(item_id) {
        ctx.want_avatars(discussion.comments.iter().map(|c| c.user_avatar_url.clone()));
        let _ = ctx.events.send(Event::DiscussionReady {
            item_id: item_id.to_string(),
            discussion: Arc::new(discussion.clone()),
        });
    }

    let messages = crate::knowledge::to_messages(pr, &discussion);
    match ingest(ctx, messages).await {
        Ok(added) => {
            tracing::info!(repo = %pr.repo_full_name, number = pr.number, added, "discussion archived");
            let _ = ctx.events.send(Event::Archived {
                repo_full_name: pr.repo_full_name.clone(),
                kind: ItemKind::PullRequest,
                number: pr.number,
                added,
            });
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not archive the discussion");
            let _ = ctx.events.send(Event::Notice(format!("Could not archive the discussion: {e}")));
        }
    }
}

/// A thread whose file changed since its anchor was made may have moved.
///
/// `git diff --quiet <sha> <head> -- <path>` per thread whose sha differs
/// from the head; the common case, same sha, costs nothing.
async fn mark_outdated_threads(ctx: &Ctx, item_id: &str, head: &str, worktree: &Path) {
    let Some(Ok(threads)) = ctx.with_cache(|c| c.load_comment_threads(item_id)) else {
        return;
    };
    for thread in threads {
        let Some(anchor) = &thread.anchor else { continue };
        if anchor.commit_sha == head || thread.outdated {
            continue;
        }
        let changed = ctx
            .preparer
            .git()
            .run(
                Some(worktree),
                ["diff", "--quiet", &anchor.commit_sha, head, "--", &anchor.path],
                Duration::from_secs(30),
            )
            .await
            .is_err();
        if changed {
            tracing::info!(thread = %thread.id, path = %anchor.path, "thread is outdated");
            ctx.with_cache(|c| c.set_thread_outdated(&thread.id, true));
        }
    }
}

/// Start a thread or continue one, run the agent, and keep the record
/// current as events arrive.
#[allow(clippy::too_many_arguments)]
async fn ask(
    ctx: &Ctx,
    item_id: String,
    thread_id: Option<String>,
    anchor: Option<Anchor>,
    question: String,
    backend: perch_agent::Backend,
    model: String,
    effort: perch_agent::Effort,
) {
    let open = ctx.slot.lock().ok().and_then(|g| {
        g.as_ref().filter(|c| c.item_id == item_id).map(|c| {
            (c.checkout.clone(), c.pull_request.clone(), c.body.clone(), c.issue.clone(), c.scope.clone())
        })
    });
    let Some((checkout, pull_request, body, issue, scope)) = open else {
        let _ = ctx.events.send(Event::Notice("That item is no longer open".into()));
        return;
    };
    if !checkout.worktree.join(".git").exists() {
        let _ = ctx.events.send(Event::Notice("There is no checkout to ask in".into()));
        return;
    }
    let now = Timestamp::now();

    // The thread: found, or made with the question as its first message.
    let existing = match &thread_id {
        Some(id) => ctx
            .with_cache(|c| c.load_comment_threads(&item_id))
            .and_then(Result::ok)
            .and_then(|all| all.into_iter().find(|t| &t.id == id)),
        None => None,
    };
    let thread = match existing {
        Some(t) => t,
        None => CommentThread {
            id: perch_github::comments::new_id(),
            worktree_id: item_id.clone(),
            origin: ThreadOrigin::Local,
            anchor: anchor.clone(),
            outdated: false,
            resolved: false,
            created_at: now,
            messages: Vec::new(),
        },
    };
    let is_new = thread_id.is_none();

    let run = RunRecord {
        id: perch_github::comments::new_id(),
        thread_id: thread.id.clone(),
        backend: backend.name().into(),
        model: model.clone(),
        effort: effort.name().into(),
        session_id: None,
        status: RunStatus::Running,
        cost_usd: None,
        duration_ms: None,
        steps: Vec::new(),
        error: None,
        started_at: now,
        ended_at: None,
    };
    // The question carries the run while it runs, so the thread has
    // somewhere to show the steps before there is an answer.
    let question_message = CommentMessage {
        id: perch_github::comments::new_id(),
        thread_id: thread.id.clone(),
        origin: ThreadOrigin::Local,
        author: Author::User,
        body: question.clone(),
        created_at: now,
        run: Some(run.clone()),
    };
    let written = ctx.with_cache(|c| {
        if is_new {
            c.insert_comment_thread(&thread)?;
        }
        c.insert_comment_message(&question_message)
    });
    if let Some(Err(e)) = written {
        let _ = ctx.events.send(Event::Notice(format!("Could not save the question: {e}")));
        return;
    }
    ctx.send_threads(&item_id);

    // The prompt. A follow-up resumes the last finished run when it was
    // this backend's; a thread with none, or one that switched backends,
    // starts over with its history folded in.
    let file = match &thread.anchor {
        Some(a) => crate::ask::load_display(&ctx.preparer, &checkout, &scope, &a.commit_sha, &a.path).await.ok(),
        None => None,
    };
    let history: Vec<(Author, String)> =
        thread.messages.iter().map(|m| (m.author.clone(), m.body.clone())).collect();
    let resume = thread.resumable_session(backend.name());
    let input = crate::ask::PromptInput {
        pull_request: pull_request.as_deref(),
        issue: issue.as_ref().map(|i| (&i.0, i.1.as_slice())),
        body: body.as_deref(),
        anchor: thread.anchor.as_ref(),
        file: file.as_ref(),
        outdated: thread.outdated,
        question: &question,
        history: if is_new || resume.is_some() { &[] } else { &history },
    };
    let prompt = if is_new { crate::ask::first_prompt(&input) } else { crate::ask::follow_up_prompt(&input) };
    let spec = crate::ask::run_spec(&ctx.config.agent, backend, &model, effort, checkout.worktree.clone(), &prompt, resume);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut run = run;
    let handle = match perch_agent::Run::spawn(spec, tx) {
        Ok(h) => h,
        Err(e) => {
            let message = format!("{e}. {}", crate::ask::binary_hint(backend));
            crate::ask::apply_run_event(&mut run, &perch_agent::RunEvent::Failed { message });
            ctx.with_cache(|c| c.update_run(&run));
            ctx.send_threads(&item_id);
            return;
        }
    };
    if let Ok(mut runs) = ctx.runs.lock() {
        runs.insert(run.id.clone(), (handle, thread.id.clone()));
    }

    let mut answer: Vec<String> = Vec::new();
    while let Some(event) = rx.recv().await {
        if let Some(text) = crate::ask::apply_run_event(&mut run, &event) {
            answer.push(text);
        }
        // Persist every step as it comes, so a crash mid-run loses nothing
        // that was shown.
        ctx.with_cache(|c| c.update_run(&run));
        let _ = ctx.events.send(Event::AgentStep {
            run_id: run.id.clone(),
            thread_id: thread.id.clone(),
            event: event.clone(),
        });
        match event {
            perch_agent::RunEvent::Finished { .. } => {
                let reply = CommentMessage {
                    id: perch_github::comments::new_id(),
                    thread_id: thread.id.clone(),
                    origin: ThreadOrigin::Local,
                    author: Author::Agent { backend: run.backend.clone(), model: run.model.clone() },
                    body: answer.join("\n\n"),
                    created_at: Timestamp::now(),
                    run: Some(run.clone()),
                };
                ctx.with_cache(|c| {
                    c.insert_comment_message(&reply)?;
                    // The answer carries the run now; the question lets go.
                    c.set_message_run(&question_message.id, None)
                });
                break;
            }
            perch_agent::RunEvent::Failed { .. } => break,
            _ => {}
        }
    }
    if let Ok(mut runs) = ctx.runs.lock() {
        runs.remove(&run.id);
    }
    tracing::info!(thread = %thread.id, run = %run.id, status = run.status.name(), steps = run.steps.len(), "run ended");
    ctx.send_threads(&item_id);

    // An answered question goes under knowledge beside the item's GitHub
    // discussion, marked as the reader's own, so an agent asked about the
    // same lines later can find it.
    if run.status == perch_github::RunStatus::Finished {
        let threads = ctx.with_cache(|c| c.load_comment_threads(&item_id).unwrap_or_default()).unwrap_or_default();
        if let Some(item) = ctx.item(&item_id) {
            let messages = crate::knowledge::threads_to_messages(&item, &threads);
            if let Err(e) = ingest(ctx, messages).await {
                tracing::warn!(error = %e, "could not archive the thread");
            }
        }
    }
}

async fn delete_comment_thread(ctx: &Ctx, id: &str) {
    // Stop anything still running in it first.
    let running: Vec<perch_agent::Run> = ctx
        .runs
        .lock()
        .map(|runs| runs.values().filter(|(_, t)| t == id).map(|(r, _)| r.clone()).collect())
        .unwrap_or_default();
    for run in running {
        run.cancel();
    }
    if let Some(Err(e)) = ctx.with_cache(|c| c.delete_comment_thread(id)) {
        let _ = ctx.events.send(Event::Notice(format!("Could not delete the thread: {e}")));
        return;
    }
    let item_id = ctx.slot.lock().ok().and_then(|g| g.as_ref().map(|c| c.item_id.clone()));
    if let Some(item_id) = item_id {
        ctx.send_threads(&item_id);
    }
}

/// A checkout for a worktree already on disk, without touching the network.
///
/// The head is whatever the worktree is at, and the merge base comes from
/// the base ref the last fetch left in the bare clone.
async fn reopen_from_disk(
    ctx: &Ctx,
    repo: &RepoId,
    number: u64,
    branch: &str,
    path: Option<std::path::PathBuf>,
) -> Result<perch_git::Checkout, perch_git::Error> {
    let store = ctx.preparer.store();
    let worktree = path.unwrap_or_else(|| store.worktree_path(repo, branch));
    let bare = store.bare_path(repo);
    let git = ctx.preparer.git();
    let timeout = Duration::from_secs(30);

    let head_sha = git
        .run(Some(&worktree), ["rev-parse", "HEAD"], timeout)
        .await?
        .trim()
        .to_string();
    let base = format!("refs/perch/base/{number}");
    let merge_base = git
        .run(Some(&bare), ["merge-base", &base, &head_sha], timeout)
        .await?
        .trim()
        .to_string();
    if merge_base.is_empty() {
        return Err(perch_git::Error::NoMergeBase);
    }

    Ok(perch_git::Checkout {
        worktree,
        bare,
        branch: branch.to_string(),
        head_sha,
        merge_base,
        stale: None,
    })
}

/// The pull request's file list, from a local diff rather than the API.
async fn open_file(
    preparer: Arc<Preparer>,
    slot: Arc<std::sync::Mutex<Option<ReviewContext>>>,
    events: EventSender<Event>,
    path: String,
) {
    let Some((checkout, changed, scope, item_id)) = slot
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|c| (c.checkout.clone(), c.files.clone(), c.scope.clone(), c.item_id.clone())))
    else {
        let _ = events.send(Event::FileFailed {
            path,
            message: "no checkout is open".into(),
        });
        return;
    };

    let entry = changed.iter().find(|f| f.path == path).cloned();
    let (base_rev, head_rev) = scope.sides(&checkout.merge_base, &checkout.head_sha);
    let git = preparer.git();

    // Both sides of a changed file come from the scope: a commit's tree,
    // the index, or the disk. A file outside the changed list is the same
    // under every scope, so it is read from disk, where it may be binary,
    // untracked or too big, which the changed list would have said.
    let head = match &entry {
        Some(f) if f.status == perch_git::ChangeStatus::Deleted => String::new(),
        Some(_) => match perch_git::read_side(git, &checkout.worktree, &head_rev, &path).await {
            Ok(Some(text)) if text.len() > MAX_FILE_BYTES => {
                let _ = events.send(Event::FileFailed { path, message: "file is too large to display".into() });
                return;
            }
            Ok(Some(text)) => text,
            Ok(None) => {
                let _ = events.send(Event::FileFailed { path, message: "file is not there".into() });
                return;
            }
            Err(e) => {
                let _ = events.send(Event::FileFailed { path, message: e.to_string() });
                return;
            }
        },
        None => match tokio::fs::read(checkout.worktree.join(&path)).await {
            Ok(bytes) if bytes.len() > MAX_FILE_BYTES => {
                let _ = events.send(Event::FileFailed { path, message: "file is too large to display".into() });
                return;
            }
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(_) => {
                    let _ = events.send(Event::FileFailed { path, message: "file is not text".into() });
                    return;
                }
            },
            Err(e) => {
                let _ = events.send(Event::FileFailed { path, message: e.to_string() });
                return;
            }
        },
    };
    let base = match &entry {
        None | Some(perch_git::ChangedFile { status: perch_git::ChangeStatus::Added, .. }) => String::new(),
        Some(f) => {
            let source = f.old_path.clone().unwrap_or_else(|| f.path.clone());
            perch_git::read_side(git, &checkout.worktree, &base_rev, &source)
                .await
                .ok()
                .flatten()
                .unwrap_or_default()
        }
    };

    let status = entry.as_ref().map(|f| f.status);
    let binary = entry.as_ref().map(|f| f.binary).unwrap_or(false);

    // Parsing and diffing are CPU work; keep them off the async runtime's
    // reactor threads.
    let loaded = tokio::task::spawn_blocking(move || {
        build_loaded_file(path, head, base, status, binary, scope)
    })
    .await;

    match loaded {
        Ok(file) => {
            tracing::info!(
                path = %file.path,
                lines = file.buffer.len(),
                changes = file.buffer.change_starts.len(),
                scope = %file.scope,
                "file loaded"
            );
            let _ = events.send(Event::FileLoaded { item_id, path: file.path.clone(), file: Arc::new(file) });
        }
        Err(e) => tracing::warn!(error = %e, "file load panicked"),
    }
}

/// Start a language server for the open checkout, if one is installed.
async fn start_language_server(
    checkout: Arc<std::sync::Mutex<Option<ReviewContext>>>,
    slot: Arc<tokio::sync::Mutex<Option<Arc<perch_lsp::Client>>>>,
    events: EventSender<Event>,
) {
    let Some(worktree) = checkout
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|c| c.checkout.worktree.clone()))
    else {
        return;
    };

    let Some(spec) = perch_lsp::find_server("rust") else {
        let _ = events.send(Event::LspStatus(perch_lsp::Status::Absent));
        return;
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    {
        let events = events.clone();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    perch_lsp::client::ServerEvent::Status(status) => {
                        tracing::debug!(status = %status.describe(), "language server");
                        let _ = events.send(Event::LspStatus(status));
                    }
                    // Diagnostics are collected but not yet drawn; the editor
                    // needs a decoration pass this milestone does not have.
                    perch_lsp::client::ServerEvent::Diagnostics { .. } => {}
                }
            }
        });
    }

    match perch_lsp::Client::start(spec, &worktree, tx).await {
        Ok(client) => {
            tracing::info!("language server started");
            *slot.lock().await = Some(client);
        }
        Err(e) => {
            let _ = events.send(Event::LspStatus(perch_lsp::Status::Failed {
                reason: e.to_string(),
            }));
        }
    }
}

/// Tell the language server about a file the user just opened.
async fn announce_file(
    checkout: Arc<std::sync::Mutex<Option<ReviewContext>>>,
    slot: Arc<tokio::sync::Mutex<Option<Arc<perch_lsp::Client>>>>,
    path: String,
) {
    let Some((client, full)) = resolve(&checkout, &slot, &path).await else {
        return;
    };
    if let Ok(text) = tokio::fs::read_to_string(&full).await {
        client.did_open(&full, &text).await;
    }
}

/// The server and the absolute path for a file of the open checkout.
async fn resolve(
    checkout: &Arc<std::sync::Mutex<Option<ReviewContext>>>,
    slot: &Arc<tokio::sync::Mutex<Option<Arc<perch_lsp::Client>>>>,
    path: &str,
) -> Option<(Arc<perch_lsp::Client>, std::path::PathBuf)> {
    let client = slot.lock().await.clone()?;
    let worktree = checkout
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|c| c.checkout.worktree.clone()))?;
    Some((client, worktree.join(path)))
}

/// Each location inside the worktree with its line's text, read here so
/// the panel needs no file loads, and a count of those outside it, in a
/// dependency, which the rail could not open. Files are read once each.
async fn read_references(
    worktree: &Path,
    locations: &[perch_lsp::lsp_types::Location],
) -> (Vec<crate::review::model::Reference>, usize) {
    let mut texts: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let mut out = Vec::new();
    let mut elsewhere = 0;
    for location in locations {
        let target = perch_lsp::client::uri_to_path(&location.uri.to_string());
        let Some(relative) = target
            .and_then(|p| p.strip_prefix(worktree).ok().map(|r| r.to_string_lossy().into_owned()))
        else {
            elsewhere += 1;
            continue;
        };
        if !texts.contains_key(&relative) {
            let lines = tokio::fs::read_to_string(worktree.join(&relative))
                .await
                .map(|t| t.lines().map(str::to_string).collect())
                .unwrap_or_default();
            texts.insert(relative.clone(), lines);
        }
        let line = location.range.start.line as usize;
        let text = texts[&relative].get(line).map(|l| l.trim().to_string()).unwrap_or_default();
        out.push(crate::review::model::Reference {
            path: relative,
            line: line + 1,
            character: location.range.start.character as usize,
            text,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    out.dedup();
    (out, elsewhere)
}

/// Flatten a hover reply into the plain text the popup shows.
fn render_hover(hover: &perch_lsp::lsp_types::Hover) -> String {
    use perch_lsp::lsp_types::{HoverContents, MarkedString};
    match &hover.contents {
        HoverContents::Scalar(MarkedString::String(s)) => s.clone(),
        HoverContents::Scalar(MarkedString::LanguageString(s)) => s.value.clone(),
        HoverContents::Markup(m) => m.value.clone(),
        HoverContents::Array(items) => items
            .iter()
            .map(|i| match i {
                MarkedString::String(s) => s.clone(),
                MarkedString::LanguageString(s) => s.value.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

pub(crate) fn build_loaded_file(
    path: String,
    head: String,
    base: String,
    status: Option<perch_git::ChangeStatus>,
    binary: bool,
    scope: perch_git::Scope,
) -> crate::review::model::LoadedFile {
    use perch_git::{file_diff, DisplayBuffer};
    use perch_syntax::{highlight, Language, LineSpans};

    let language = perch_syntax::Language::from_path(&path);

    if binary {
        return crate::review::model::LoadedFile {
            path,
            language: None,
            buffer: DisplayBuffer::plain("This file is binary and has no text to show."),
            head_spans: LineSpans::default(),
            base_spans: LineSpans::default(),
            status,
            truncated: true,
            scope,
            base_buffer: None,
            alignment: Vec::new(),
            blob: String::new(),
        };
    }

    // A changed file is diffed for the stacked view, and the same diff
    // gives the split view its base side and its row pairing. A file
    // outside the list is shown plainly, which is what makes browsing the
    // whole tree useful during a review.
    let (buffer, base_buffer, alignment) = match status {
        Some(_) => {
            let diff = file_diff(&base, &head);
            let alignment = perch_git::alignment(&diff, &base, &head);
            let base_buffer = (!base.is_empty()).then(|| DisplayBuffer::plain(&base));
            (DisplayBuffer::unified(&head, &base, &diff), base_buffer, alignment)
        }
        None => (DisplayBuffer::plain(&head), None, Vec::new()),
    };

    let spans_for = |text: &str, lang: Option<Language>| match lang {
        Some(lang) if !text.is_empty() => LineSpans::new(text, &highlight(lang, text)),
        _ => LineSpans::default(),
    };

    crate::review::model::LoadedFile {
        head_spans: spans_for(&head, language),
        base_spans: spans_for(&base, language),
        path,
        language,
        buffer,
        status,
        truncated: false,
        scope,
        base_buffer,
        alignment,
        blob: String::new(),
    }
}

type SharedCache = Option<Arc<std::sync::Mutex<Cache>>>;

/// The cache, and whether an older one had to be thrown away. The
/// harness that runs a list pass opens it the same way, so what it
/// writes is what the window reads.
pub(crate) fn open_cache() -> (SharedCache, bool) {
    let Some(path) = perch_github::config::db_path() else {
        return (None, false);
    };
    let avatars = perch_github::config::avatar_dir();
    match Cache::open_checked(&path, avatars) {
        Ok((c, reset)) => (Some(Arc::new(std::sync::Mutex::new(c))), reset),
        Err(e) => {
            // A broken cache must not stop the app; it only costs a cold start.
            tracing::warn!(error = %e, "running without a local cache");
            (None, false)
        }
    }
}

fn spawn_avatar(
    github: Arc<GitHub>,
    cache: SharedCache,
    events: EventSender<Event>,
    limit: Arc<Semaphore>,
    url: String,
) {
    // Disk first. Avatars change rarely and a hit costs no request.
    if let Some(cache) = cache.as_ref() {
        if let Ok(guard) = cache.lock() {
            if let Some(bytes) = guard.load_avatar(&url) {
                tracing::debug!(url, bytes = bytes.len(), "avatar from disk");
                let _ = events.send(Event::Avatar { url, bytes });
                return;
            }
        }
    }

    tokio::spawn(async move {
        let Ok(_permit) = limit.acquire().await else {
            return;
        };
        match github.fetch_avatar(&url).await {
            Ok(bytes) => {
                if let Some(cache) = cache.as_ref() {
                    if let Ok(guard) = cache.lock() {
                        let _ = guard.store_avatar(&url, &bytes);
                    }
                }
                let _ = events.send(Event::Avatar { url, bytes });
            }
            Err(e) => tracing::debug!(error = %e, url, "avatar fetch failed"),
        }
    });
}

fn describe_error(e: &perch_github::Error) -> Status {
    match e {
        perch_github::Error::Unauthorized => Status::NoToken {
            detail: "GitHub rejected the token. It may be expired, or missing the \
                     notifications scope."
                .into(),
        },
        perch_github::Error::RateLimited { reset_epoch } => Status::RateLimited {
            message: match reset_epoch.and_then(local_clock_time) {
                Some(t) => format!("Rate limited. Retrying after {t}."),
                None => "Rate limited. Retrying shortly.".into(),
            },
        },
        other => Status::Error { message: other.to_string() },
    }
}

/// A list pass's failure as the status bar shows it: GitHub's the way a
/// failed poll is described, so a rate limit reads as one, and the
/// cache's in its own words. An answer GitHub gave about the repository,
/// a 404 for a `[[repo]]` mistyped or a body that would not parse, is
/// prefixed with the repository, since with several declared the reader
/// cannot otherwise tell which one to fix. A network failure, a rejected
/// token or a rate limit is about every repository, and names none.
fn describe_pass_error(repo: &str, e: &sync::Error) -> Status {
    match e {
        sync::Error::GitHub(e @ (perch_github::Error::Status { .. } | perch_github::Error::Decode(_))) => {
            Status::Error { message: format!("{repo}: {e}") }
        }
        sync::Error::GitHub(e) => describe_error(e),
        other => Status::Error { message: other.to_string() },
    }
}

/// Wall-clock time in the user's zone, for the status bar.
pub(crate) fn local_clock_time(epoch: i64) -> Option<String> {
    let ts = Timestamp::from_second(epoch).ok()?;
    let zoned = ts.to_zoned(jiff::tz::TimeZone::system());
    Some(format!("{:02}:{:02}", zoned.hour(), zoned.minute()))
}

/// How long to wait out a rate limit, with a floor and a ceiling so a bad
/// clock or a far-future reset cannot wedge the app.
fn backoff_until(reset_epoch: Option<i64>) -> Duration {
    const MIN: u64 = 60;
    const MAX: u64 = 15 * 60;
    let Some(reset) = reset_epoch else {
        return Duration::from_secs(MIN);
    };
    let now = Timestamp::now().as_second();
    let wait = (reset - now).max(0) as u64;
    Duration::from_secs(wait.clamp(MIN, MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_has_a_floor_when_the_reset_is_past() {
        let past = Timestamp::now().as_second() - 500;
        assert_eq!(backoff_until(Some(past)), Duration::from_secs(60));
    }

    #[test]
    fn backoff_has_a_ceiling_when_the_reset_is_far_off() {
        let far = Timestamp::now().as_second() + 86_400;
        assert_eq!(backoff_until(Some(far)), Duration::from_secs(15 * 60));
    }

    #[test]
    fn backoff_without_a_reset_header_still_waits() {
        assert_eq!(backoff_until(None), Duration::from_secs(60));
    }

    #[test]
    fn an_unauthorized_error_asks_for_a_token_rather_than_showing_a_stack() {
        let s = describe_error(&perch_github::Error::Unauthorized);
        assert!(matches!(s, Status::NoToken { .. }));
    }

    #[test]
    fn a_rate_limit_error_reports_when_it_lifts() {
        let s = describe_error(&perch_github::Error::RateLimited { reset_epoch: Some(0) });
        match s {
            Status::RateLimited { message } => assert!(message.contains("Retrying after")),
            other => panic!("expected a rate limit status, got {other:?}"),
        }
    }

    use perch_github::{Reason, Repository, Subject, SubjectKind};

    fn thread(id: &str, title: &str) -> Thread {
        Thread {
            id: id.into(),
            unread: true,
            reason: Reason::Subscribed,
            updated_at: "2026-09-17T18:00:00Z".parse().unwrap(),
            last_read_at: None,
            subject: Subject {
                title: title.into(),
                kind: SubjectKind::PullRequest,
                url: Some(format!("https://api.github.com/repos/a/b/pulls/{id}")),
                latest_comment_url: None,
            },
            repository: Repository {
                full_name: "a/b".into(),
                owner_avatar_url: String::new(),
            },
        }
    }

    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc::Receiver;
    use std::sync::Mutex;

    /// A GitHub that answers every request the same way and writes down
    /// the paths it was asked, so a test can say what the code asked for
    /// and what it did not. The listener lives as long as the test binary
    /// does.
    fn serve(status: &'static str, body: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        serve_by_path(move |_| (status, body.to_string()))
    }

    /// A GitHub that answers each request by its path.
    fn serve_by_path(
        answer: impl Fn(&str) -> (&'static str, String) + Send + 'static,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let paths = Arc::new(Mutex::new(Vec::new()));
        let seen = paths.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let Some(path) = request_path(&mut stream) else { continue };
                let (status, body) = answer(&path);
                seen.lock().unwrap().push(path);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (base, paths)
    }

    /// The path of one GET, read past its headers so the client sees the
    /// answer as the answer to it.
    fn request_path(stream: &mut TcpStream) -> Option<String> {
        let mut reader = BufReader::new(stream.try_clone().ok()?);
        let mut start = String::new();
        reader.read_line(&mut start).ok()?;
        let path = start.split_whitespace().nth(1)?.to_string();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).ok()? == 0 || line.trim_end().is_empty() {
                break;
            }
        }
        Some(path)
    }

    /// A base nothing listens on, so a request the code should not make
    /// is refused at once instead of reaching GitHub.
    fn nowhere() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    }

    /// A context with no window behind it: a client at `base`, a cache
    /// and a knowledge root in a directory of their own, and the events
    /// kept so a test can say what the sidebar was told.
    fn ctx(dir: &Path, base: &str, declared: &[&str]) -> (Ctx, Receiver<Event>) {
        let (events, received) = std::sync::mpsc::channel();
        let cache = Cache::open(&dir.join("perch.db"), None).unwrap();
        let mut config = Config::default();
        config.repo = declared
            .iter()
            .map(|name| perch_github::RepoConfig { name: (*name).to_string(), roots: Vec::new() })
            .collect();
        let ctx = Ctx {
            github: Arc::new(GitHub::with_base_uri("token", base).unwrap()),
            preparer: Arc::new(Preparer::new(Store::new(dir.join("store")), None)),
            cache: Some(Arc::new(std::sync::Mutex::new(cache))),
            config: Arc::new(config),
            knowledge: Arc::new(perch_knowledge::Knowledge::open(dir.join("knowledge"))),
            slot: Arc::new(std::sync::Mutex::new(None)),
            language_server: Arc::new(tokio::sync::Mutex::new(None)),
            events,
            runs: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            avatars: Arc::new(Semaphore::new(1)),
            seen_ci: Default::default(),
            refreshing: Default::default(),
            passes: Arc::new(tokio::sync::Notify::new()),
        };
        (ctx, received)
    }

    fn pull_request(number: u64) -> PullRequest {
        PullRequest {
            number,
            title: format!("pull {number}"),
            author_login: "u".into(),
            author_avatar_url: String::new(),
            head_sha: format!("h{number}"),
            head_ref: format!("branch-{number}"),
            base_sha: "m".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            repo_full_name: "a/b".into(),
            head_repo_full_name: Some("a/b".into()),
            updated_at: Some("2026-09-16T11:40:00Z".parse().unwrap()),
            review_requested: false,
            checks: None,
            review_decision: None,
            merged_at: None,
            merged_by: None,
            closed_at: None,
            requested_reviewers: Vec::new(),
            milestone: None,
            labels: Vec::new(),
            assignees: Vec::new(),
        }
    }

    /// A row for `a/b#<number>`, as the list pass makes it: no attention.
    fn store_pull(ctx: &Ctx, number: u64) -> Item {
        let mut item = Item::new("a/b", "2026-09-10T00:00:00Z".parse().unwrap());
        item.subject = Some(ItemSubject::PullRequest(pull_request(number)));
        ctx.with_cache(|c| c.insert_item(&item)).unwrap().unwrap();
        item
    }

    fn with_reason(mut t: Thread, reason: Reason) -> Thread {
        t.reason = reason;
        t
    }

    fn items_sent(received: &Receiver<Event>) -> usize {
        received.try_iter().filter(|e| matches!(e, Event::Items(_))).count()
    }

    #[tokio::test]
    async fn a_subscribed_notification_about_an_item_is_not_attention() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, received) = ctx(dir.path(), &nowhere(), &[]);
        let item = store_pull(&ctx, 7);

        attend(&ctx, &[with_reason(thread("7", "seven"), Reason::Subscribed)]).await;

        let item = ctx.item(&item.id).unwrap();
        assert_eq!(item.attention_at, None, "a push on a watched pull request is not Priority");
        assert_eq!(item.attention_reason, None);
        assert_eq!(items_sent(&received), 0, "nothing changed, so the sidebar is not told");
    }

    #[tokio::test]
    async fn a_mention_about_an_item_is_its_attention_dated_to_the_notification() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, received) = ctx(dir.path(), &nowhere(), &[]);
        let item = store_pull(&ctx, 7);
        let mention = with_reason(thread("7", "seven"), Reason::Mention);

        attend(&ctx, std::slice::from_ref(&mention)).await;

        let item = ctx.item(&item.id).unwrap();
        assert_eq!(item.attention_at, Some(mention.updated_at));
        assert_eq!(item.attention_reason.as_deref(), Some("mention"));
        assert_eq!(items_sent(&received), 1);
    }

    /// `a/b#7` as `GET /repos/a/b/pulls/7` sends it, so an `attend` that
    /// still fetched and made a row could, and would be seen to.
    const PULL_7: &str = r#"{"number":7,"title":"seven","state":"open","draft":false,
        "user":{"login":"u","avatar_url":""},
        "head":{"sha":"h7","ref":"branch-7","repo":{"full_name":"a/b"}},
        "base":{"sha":"m","ref":"main","repo":{"full_name":"a/b"}},
        "updated_at":"2026-09-16T11:40:00Z"}"#;

    #[tokio::test]
    async fn a_review_request_with_no_item_makes_none() {
        let dir = tempfile::tempdir().unwrap();
        // A GitHub that would answer for the pull request, so a fetch
        // could not fail its way to the same outcome: the list pass is
        // what makes rows now, and the notification is let go.
        let (base, paths) = serve("200 OK", PULL_7);
        let (ctx, received) = ctx(dir.path(), &base, &[]);

        attend(&ctx, &[with_reason(thread("7", "seven"), Reason::ReviewRequested)]).await;

        let items = ctx.with_cache(|c| c.load_items().unwrap()).unwrap();
        assert!(items.is_empty(), "a notification does not decide what exists");
        assert_eq!(items_sent(&received), 0);
        assert!(paths.lock().unwrap().is_empty(), "and nothing was asked to find out");
    }

    const FAILING_RUN: &str = r#"{"workflow_runs":[{"name":"ci","run_number":1,"status":"completed","conclusion":"failure","head_branch":"branch-7","html_url":"https://github.com/a/b/actions/runs/1","created_at":"2026-09-18T11:00:00Z","updated_at":"2026-09-18T11:05:00Z"}]}"#;

    #[tokio::test]
    async fn red_checks_after_a_ci_notification_are_attention() {
        let dir = tempfile::tempdir().unwrap();
        let (base, paths) = serve("200 OK", FAILING_RUN);
        let (ctx, received) = ctx(dir.path(), &base, &[]);
        let item = store_pull(&ctx, 7);
        let at: Timestamp = "2026-09-18T11:06:00Z".parse().unwrap();

        fetch_checks(&ctx, &item.id, &pull_request(7), Some(at)).await;

        let item = ctx.item(&item.id).unwrap();
        assert_eq!(item.attention_at, Some(at), "dated to the notification, not to now");
        assert_eq!(item.attention_reason.as_deref(), Some("ci_failed"));
        assert_eq!(items_sent(&received), 1);
        assert_eq!(paths.lock().unwrap().as_slice(), ["/repos/a/b/actions/runs?head_sha=h7&per_page=100"]);
    }

    #[tokio::test]
    async fn red_checks_on_an_open_are_kept_but_are_not_attention() {
        let dir = tempfile::tempdir().unwrap();
        let (base, _) = serve("200 OK", FAILING_RUN);
        let (ctx, received) = ctx(dir.path(), &base, &[]);
        let item = store_pull(&ctx, 7);

        fetch_checks(&ctx, &item.id, &pull_request(7), None).await;

        let checks = ctx.with_cache(|c| c.load_checks(&item.id, "h7").unwrap()).unwrap();
        assert!(matches!(checks, Some(perch_github::Checks { state: perch_github::CheckState::Failing { .. }, .. })));
        let item = ctx.item(&item.id).unwrap();
        assert_eq!(item.attention_at, None, "red checks the reader is looking at are not news");
        assert_eq!(items_sent(&received), 0);
    }

    #[tokio::test]
    async fn a_failed_pass_does_not_stop_the_next_repository_and_the_first_failure_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        // Each repository fails in its own way, and neither with a server
        // error, which the client retries. Not found for the first, gone
        // for the second, so which failure was kept can be told.
        let (base, paths) = serve_by_path(|path| {
            if path.starts_with("/repos/a/b/") {
                ("404 Not Found", r#"{"message":"a/b is not there"}"#.to_string())
            } else {
                ("410 Gone", r#"{"message":"c/d is gone"}"#.to_string())
            }
        });
        let (ctx, _received) = ctx(dir.path(), &base, &["a/b", "c/d"]);

        let failure = list_passes(&ctx).await;

        let (repo, error) = failure.expect("a failure");
        assert_eq!(repo, "a/b", "the first repository's failure is the one kept");
        assert!(
            matches!(&error, sync::Error::GitHub(perch_github::Error::Status { status: 404, body }) if body.contains("a/b")),
            "{error}"
        );
        // Both repositories were asked, each for its pull requests and no
        // further, since a list that failed ends its pass.
        assert_eq!(
            paths.lock().unwrap().as_slice(),
            ["/repos/a/b/pulls?state=open&per_page=100&page=1", "/repos/c/d/pulls?state=open&per_page=100&page=1"]
        );
    }

    #[test]
    fn a_pass_failure_is_described_the_way_a_poll_failure_is() {
        let limited = sync::Error::GitHub(perch_github::Error::RateLimited { reset_epoch: None });
        assert!(matches!(describe_pass_error("a/b", &limited), Status::RateLimited { .. }));
        match describe_pass_error("a/b", &sync::Error::NoCache) {
            Status::Error { message } => assert!(message.contains("no cache"), "{message}"),
            other => panic!("expected an error status, got {other:?}"),
        }
    }

    #[test]
    fn an_answer_about_one_repository_names_it_and_a_failure_about_all_of_them_does_not() {
        let not_found = sync::Error::GitHub(perch_github::Error::Status { status: 404, body: "{}".into() });
        match describe_pass_error("slatedb/slatedb", &not_found) {
            Status::Error { message } => {
                assert_eq!(message, "slatedb/slatedb: GitHub returned 404: {}", "which [[repo]] to fix");
            }
            other => panic!("expected an error status, got {other:?}"),
        }
        let offline = sync::Error::GitHub(perch_github::Error::Network("no route".into()));
        match describe_pass_error("slatedb/slatedb", &offline) {
            Status::Error { message } => assert!(!message.contains("slatedb"), "not one repository's fault: {message}"),
            other => panic!("expected an error status, got {other:?}"),
        }
    }

    /// A checkout under a root on `branch`, as the scan records it.
    fn root_checkout(branch: &str) -> ItemCheckout {
        ItemCheckout {
            branch: branch.into(),
            base_ref: "develop".into(),
            head_sha: "h7".into(),
            path: "/x/b".into(),
            owned: false,
            on_disk: true,
            churn: Some((3, 1)),
            last_seen_head: None,
        }
    }

    #[tokio::test]
    async fn a_branch_takes_its_pull_request_from_the_row_the_pass_made_and_carries_the_marks() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _received) = ctx(dir.path(), &nowhere(), &[]);
        // The pass listed #7 before the scan saw the branch; the reader
        // then pinned the pass's row.
        let mut listed = store_pull(&ctx, 7);
        listed.pinned_at = Some("2026-09-17T09:00:00Z".parse().unwrap());
        ctx.with_cache(|c| c.update_item(&listed)).unwrap().unwrap();
        let mut branch = Item::new("a/b", "2026-09-10T00:00:00Z".parse().unwrap());
        branch.checkout = Some(root_checkout("branch-7"));
        ctx.with_cache(|c| c.insert_item(&branch)).unwrap().unwrap();

        let taken = ctx.with_cache(|c| take_pull_request(c, &mut branch, pull_request(7))).unwrap().unwrap();

        assert!(taken);
        let items = ctx.with_cache(|c| c.load_items().unwrap()).unwrap();
        assert_eq!(items.len(), 1, "one row for the branch and its pull request");
        assert_eq!(items[0].id, branch.id, "the reader's row, with its threads, is the one kept");
        assert_eq!(items[0].pull_request().map(|p| p.number), Some(7));
        assert_eq!(items[0].checkout.as_ref().map(|c| c.base_ref.as_str()), Some("main"), "the pull request's base");
        assert_eq!(items[0].pinned_at, listed.pinned_at, "the pin on the pass's row came across");
    }

    #[tokio::test]
    async fn a_pull_request_held_by_a_row_with_its_own_checkout_stays_there() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _received) = ctx(dir.path(), &nowhere(), &[]);
        // Opened in Perch first, so the pull request's row has a checkout
        // Perch cut for it.
        let mut opened = store_pull(&ctx, 7);
        opened.checkout = Some(ItemCheckout { branch: "perch/pr-7".into(), owned: true, ..root_checkout("perch/pr-7") });
        ctx.with_cache(|c| c.update_item(&opened)).unwrap().unwrap();
        let mut branch = Item::new("a/b", "2026-09-10T00:00:00Z".parse().unwrap());
        branch.checkout = Some(root_checkout("branch-7"));
        ctx.with_cache(|c| c.insert_item(&branch)).unwrap().unwrap();

        let taken = ctx.with_cache(|c| take_pull_request(c, &mut branch, pull_request(7))).unwrap().unwrap();

        assert!(!taken, "the row that has it keeps it");
        let items = ctx.with_cache(|c| c.load_items().unwrap()).unwrap();
        assert_eq!(items.len(), 2, "and the branch stays a branch");
        assert!(ctx.item(&branch.id).unwrap().subject.is_none());
    }

    #[tokio::test]
    async fn an_open_that_found_no_row_writes_onto_one_a_pass_made_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _received) = ctx(dir.path(), &nowhere(), &[]);
        // Nothing when the open looked; then, while it cloned, the pass
        // listed #7 with a later snapshot.
        let mut listed = store_pull(&ctx, 7);
        let mut later = pull_request(7);
        later.title = "seven, renamed".into();
        later.updated_at = Some("2026-09-17T09:00:00Z".parse().unwrap());
        listed.subject = Some(ItemSubject::PullRequest(later));
        ctx.with_cache(|c| c.update_item(&listed)).unwrap().unwrap();
        let now: Timestamp = "2026-09-18T12:00:00Z".parse().unwrap();
        let mut opened = Item::new("a/b", now);
        opened.subject = Some(ItemSubject::PullRequest(pull_request(7)));
        opened.checkout = Some(ItemCheckout { branch: "perch/pr-7".into(), owned: true, ..root_checkout("perch/pr-7") });

        ctx.with_cache(|c| record_open(c, &mut opened, true)).unwrap().expect("no collision on the subject index");

        assert_eq!(opened.id, listed.id, "the review runs as the row that exists");
        let items = ctx.with_cache(|c| c.load_items().unwrap()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].checkout.as_ref().map(|c| c.branch.as_str()), Some("perch/pr-7"), "with the open's checkout");
        assert_eq!(items[0].last_opened_at, now);
        assert_eq!(items[0].title(), "seven, renamed", "the pass's later snapshot stands over the one fetched before the clone");
    }

    #[tokio::test]
    async fn an_open_that_found_no_row_and_still_finds_none_inserts_one() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _received) = ctx(dir.path(), &nowhere(), &[]);
        let mut opened = Item::new("a/b", "2026-09-18T12:00:00Z".parse().unwrap());
        opened.subject = Some(ItemSubject::PullRequest(pull_request(7)));
        opened.checkout = Some(ItemCheckout { branch: "perch/pr-7".into(), owned: true, ..root_checkout("perch/pr-7") });

        ctx.with_cache(|c| record_open(c, &mut opened, true)).unwrap().unwrap();

        assert_eq!(ctx.item(&opened.id).unwrap(), opened);
    }
}
