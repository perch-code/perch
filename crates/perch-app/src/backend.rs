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
    resolve_token, Anchor, Author, Cache, CommentMessage, CommentThread, Config, DetailSource,
    GitHub, Item, ItemCheckout, ItemKind, ItemSubject, Mark, Project, PullRequest, Reason,
    RunRecord, RunStatus, SubjectDetail, SubjectKind, Thread, ThreadOrigin,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Semaphore;

/// How many per-thread detail requests may be in flight at once.
///
/// A cold inbox of 100 threads is 100 requests against a 5000/hour budget.
/// Eight at a time keeps the first screen filling quickly without burning
/// through the limit in a burst.
const DETAIL_CONCURRENCY: usize = 8;
const AVATAR_CONCURRENCY: usize = 6;
const LAST_MODIFIED_KEY: &str = "notifications_last_modified";
/// Settings the UI keeps between launches. Sent back at startup.
pub const UI_META_KEYS: [&str; 2] = ["item_filters", "collapsed_projects"];

/// What the per-thread fetches share.
#[derive(Clone)]
struct Fetches {
    details: Arc<Semaphore>,
    avatars: Arc<Semaphore>,
    /// Workflow run lookups that came back empty, by thread id and
    /// `updated_at`. A run GitHub no longer lists, or a title the branch's
    /// runs do not match, would otherwise be looked up again on every inbox
    /// refresh. Kept for the process only, so a restart asks once more in
    /// case the answer has changed.
    unresolved: Arc<std::sync::Mutex<std::collections::HashSet<(String, String)>>>,
}

impl Fetches {
    fn new() -> Self {
        Self {
            details: Arc::new(Semaphore::new(DETAIL_CONCURRENCY)),
            avatars: Arc::new(Semaphore::new(AVATAR_CONCURRENCY)),
            unresolved: Default::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Command {
    Refresh,
    /// Open an item's view: the review for a pull request or a branch, the
    /// issue view for an issue. Prepares the checkout when there is one to
    /// prepare.
    OpenItem { id: String },
    /// Turn a notification into an item, or find the one it already has,
    /// and open it.
    OpenSubject { repo_full_name: String, kind: ItemKind, number: u64 },
    /// Read, diff, and highlight one file of the open checkout.
    OpenFile { path: String },
    /// Ask the language server about a position. `token` lets the UI drop an
    /// answer that arrives after the pointer has moved on.
    Hover { token: u64, path: String, line: u32, character: u32 },
    Definition { token: u64, path: String, line: u32, character: u32 },
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
    /// `project_id` of `None` with `new_project` set makes an implicit
    /// project of that name first.
    MoveItem { id: String, project_id: Option<String>, new_project: Option<String> },
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

/// A mark write.
///
/// On its own channel, drained by its own task, because the main loop awaits
/// the network inline. Holding a click behind a poll is fine for the poll and
/// not fine for the click.
#[derive(Debug, Clone)]
pub enum MarkCommand {
    /// Save or unsave every thread named, as one act.
    ///
    /// A single row is a batch of one. Bulk selection is the reason these
    /// take a list: a hundred rows should cost one transaction and one
    /// event, not a hundred of each.
    SetSaved { thread_ids: Vec<String>, saved: bool },
    /// Mark done, or restore, every thread named, as one act.
    SetDone { thread_ids: Vec<String>, done: bool },
}

#[derive(Debug, Clone)]
pub enum Event {
    /// A whole inbox, from the cache on startup or from the network after.
    Threads { threads: Vec<Thread>, from_cache: bool },
    Detail { thread_id: String, detail: SubjectDetail },
    Avatar { url: String, bytes: Vec<u8> },
    /// Every mark Perch holds. Sent whole rather than as a patch, because
    /// the set is small and a replacement cannot drift.
    Marks(Vec<Mark>),
    /// Every project, declared or implicit. Whole, like marks.
    Projects(Vec<Project>),
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
    /// A pull request's discussion was archived under the knowledge root.
    /// `added` counts records that were new or changed.
    Archived { repo_full_name: String, number: u64, added: usize },
    /// Something worth a line in the status bar and nothing more.
    Notice(String),
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
    FileLoaded { path: String, file: Arc<crate::review::model::LoadedFile> },
    FileFailed { path: String, message: String },

    LspStatus(perch_lsp::Status),
    LspHover { token: u64, text: String },
    /// A definition inside the worktree, or `None` when it lands outside it.
    LspDefinition { token: u64, path: Option<String>, line: u32 },
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
    marks: UnboundedSender<MarkCommand>,
}

impl Backend {
    pub fn refresh(&self) {
        let _ = self.commands.send(Command::Refresh);
    }

    /// Open a pull request or an issue by number, making its item if it has
    /// none yet.
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

    pub fn move_item(&self, id: &str, project_id: Option<String>, new_project: Option<String>) {
        let _ = self.commands.send(Command::MoveItem { id: id.to_string(), project_id, new_project });
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

    pub fn set_saved(&self, thread_ids: Vec<String>, saved: bool) {
        if thread_ids.is_empty() {
            return;
        }
        let _ = self.marks.send(MarkCommand::SetSaved { thread_ids, saved });
    }

    pub fn set_done(&self, thread_ids: Vec<String>, done: bool) {
        if thread_ids.is_empty() {
            return;
        }
        let _ = self.marks.send(MarkCommand::SetDone { thread_ids, done });
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
        let (marks, _) = tokio::sync::mpsc::unbounded_channel();
        Backend { commands, marks }
    }
}

/// Start the worker thread. Returns the command handle and the event receiver.
pub fn spawn() -> (Backend, std::sync::mpsc::Receiver<Event>) {
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (mark_tx, mark_rx) = tokio::sync::mpsc::unbounded_channel();
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
            runtime.block_on(run(cmd_rx, mark_rx, evt_tx));
        })
        .expect("spawn backend thread");

    (
        Backend {
            commands: cmd_tx,
            marks: mark_tx,
        },
        evt_rx,
    )
}

async fn run(
    mut commands: UnboundedReceiver<Command>,
    marks: UnboundedReceiver<MarkCommand>,
    events: EventSender<Event>,
) {
    tracing::info!("backend started");
    let config = Config::load();
    tracing::info!(poll = config.poll_interval_secs, "config loaded");
    let _ = events.send(Event::AgentConfig(config.agent.clone()));

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

    // Marks run beside the loop rather than in it. A poll can hold the loop
    // for as long as the network takes, and a reader who presses Done should
    // not find out how long that is.
    spawn_mark_writer(marks, cache.clone(), events.clone());

    let fetches = Fetches::new();

    // Paint from the cache first. A cold launch with no network still shows
    // the inbox as it was.
    //
    // This goes through the same path a network response does, which is the
    // point: it reads the details AND the avatars back off disk. Sending the
    // details alone left every row showing a letter placeholder whenever the
    // poll came back unchanged, because nothing else ever loads an avatar.
    if let Some(cache) = cache.as_ref() {
        // Marks go out before the list they narrow. Sent the other way
        // round, a dismissed row would paint for a frame before vanishing.
        if let Ok(guard) = cache.lock() {
            send_marks(&guard, &events);
            // Projects follow the file on every launch, then the sidebar
            // paints from the cache before the network is asked anything.
            sync_projects(&guard, &config);
            send_projects(&guard, &events);
            send_items(&guard, &events);
            for key in UI_META_KEYS {
                if let Ok(Some(value)) = guard.meta(key) {
                    let _ = events.send(Event::Meta { key: key.to_string(), value });
                }
            }
        }

        let cached = cache.lock().ok().and_then(|guard| {
            let threads = guard.load_threads().ok()?;
            (!threads.is_empty()).then_some(threads)
        });

        if let Some(threads) = cached {
            let _ = events.send(Event::Threads {
                threads: threads.clone(),
                from_cache: true,
            });
            fetch_details(
                github.clone(),
                Some(cache.clone()),
                events.clone(),
                fetches.clone(),
                threads,
            );
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
        avatars: fetches.avatars.clone(),
    };

    // Bring the table and the disk into agreement before any command can
    // make a directory. Inline, not spawned: a checkout made by a launch
    // hook while this ran would have no row yet, and would be taken for a
    // stray and removed from under it. It is a directory listing and at
    // most a prune, so the inbox is not kept waiting.
    reconcile_checkouts(&ctx).await;
    {
        // Then put away whatever nobody has opened in a fortnight. Archived,
        // not deleted: the branch and the row stay, so restoring is a
        // checkout. Only owned checkouts are touched, so this can run
        // beside whatever the reader does next.
        let ctx = ctx.clone();
        tokio::spawn(async move { sweep_items(&ctx).await });
    }

    {
        // Roots and the pull requests behind subject-less branches, after
        // the cache has painted and off the loop.
        let ctx = ctx.clone();
        tokio::spawn(async move { refresh_items(&ctx).await });
    }

    // A refresh happens immediately, then on whatever cadence the server asks
    // for. A command from the UI short-circuits the wait.
    let mut poll_after = Duration::from_secs(0);

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
                poll_after = refresh(&ctx, &source, &fetches).await;
                let ctx = ctx.clone();
                tokio::spawn(async move { refresh_items(&ctx).await });
            }
            Some(Command::OpenSubject { repo_full_name, kind, number }) => {
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
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if open_item(&ctx, &id).await {
                        start_language_server(ctx.slot, ctx.language_server, ctx.events).await;
                    }
                });
            }
            Some(Command::ArchiveItem { id }) => {
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if let Err(message) = archive_item(&ctx, &id).await {
                        tracing::warn!(id, %message, "could not archive item");
                        let _ = ctx.events.send(Event::Notice(message));
                    }
                });
            }
            Some(Command::DeleteItem { id }) => {
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
            Some(Command::MoveItem { id, project_id, new_project }) => {
                move_item(&ctx, &id, project_id, new_project);
            }
            Some(Command::StartWork { issue_item_id }) => {
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

/// Drain mark writes on their own task.
///
/// Ends when the UI drops its handle, which is also when the loop ends.
fn spawn_mark_writer(
    mut marks: UnboundedReceiver<MarkCommand>,
    cache: SharedCache,
    events: EventSender<Event>,
) {
    tokio::spawn(async move {
        while let Some(command) = marks.recv().await {
            match command {
                MarkCommand::SetSaved { thread_ids, saved } => {
                    write_marks(&cache, &events, &thread_ids, |c, threads| {
                        c.set_saved(threads, saved)
                    });
                }
                MarkCommand::SetDone { thread_ids, done } => {
                    write_marks(&cache, &events, &thread_ids, |c, threads| {
                        c.set_done(threads, done)
                    });
                }
            }
        }
    });
}

/// Write one flag across a batch of threads, then publish the new mark set.
///
/// The whole batch is one transaction inside the cache, so a bulk action
/// either lands or does not, and the UI hears about it once.
///
/// Resolving and writing happen under one lock, so a poll landing in the
/// middle cannot leave a mark describing a thread the write never saw.
fn write_marks(
    cache: &SharedCache,
    events: &EventSender<Event>,
    thread_ids: &[String],
    write: impl FnOnce(&mut Cache, &[&Thread]) -> Result<(), perch_github::CacheError>,
) {
    let Some(cache) = cache.as_ref() else {
        tracing::warn!("no cache, so nothing to mark");
        return;
    };
    let Ok(mut guard) = cache.lock() else { return };

    let threads = resolve_threads(&guard, thread_ids);
    let refs: Vec<&Thread> = threads.iter().collect();
    if let Err(e) = write(&mut guard, &refs) {
        tracing::warn!(error = %e, "could not write marks");
        return;
    }
    send_marks(&guard, events);
}

/// Thread ids to the threads they name.
///
/// The live inbox wins, and a mark's snapshot covers a thread the API has
/// stopped returning. An id in neither is one the UI believes in and the
/// cache does not, so skip it rather than invent a thread to mark.
fn resolve_threads(cache: &Cache, ids: &[String]) -> Vec<Thread> {
    let live = cache.load_threads().unwrap_or_default();
    let marks = cache.load_marks().unwrap_or_default();

    let mut by_id: std::collections::HashMap<&str, &Thread> = std::collections::HashMap::new();
    for m in &marks {
        by_id.insert(m.thread_id.as_str(), &m.snapshot);
    }
    for t in &live {
        by_id.insert(t.id.as_str(), t);
    }

    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        match by_id.get(id.as_str()) {
            Some(t) => out.push((*t).clone()),
            None => tracing::warn!(thread_id = %id, "asked to mark a thread nothing knows about"),
        }
    }
    out
}

fn send_marks(cache: &Cache, events: &EventSender<Event>) {
    match cache.load_marks() {
        Ok(marks) => {
            let _ = events.send(Event::Marks(marks));
        }
        Err(e) => tracing::warn!(error = %e, "could not read marks"),
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
    /// The avatar download limit, shared with the inbox's fetches.
    avatars: Arc<Semaphore>,
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

    fn send_projects(&self) {
        self.with_cache(|c| send_projects(c, &self.events));
    }

    fn notice(&self, message: impl Into<String>) {
        let _ = self.events.send(Event::Notice(message.into()));
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

    /// The project a repository's items go in, made implicit if nothing
    /// declares it. Returns its id.
    fn project_for(&self, repo_full_name: &str) -> String {
        let name = self.config.project_for(repo_full_name);
        let id = Project::id_for(&name);
        self.with_cache(|c| {
            let known = c.load_projects().unwrap_or_default();
            if !known.iter().any(|p| p.id == id) {
                if let Err(e) = c.upsert_project(&Project::implicit(&name)) {
                    tracing::warn!(error = %e, "could not make an implicit project");
                }
                send_projects(c, &self.events);
            }
        });
        id
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

fn send_projects(cache: &Cache, events: &EventSender<Event>) {
    match cache.load_projects() {
        Ok(projects) => {
            let _ = events.send(Event::Projects(projects));
        }
        Err(e) => tracing::warn!(error = %e, "could not read projects"),
    }
}

/// Make the projects table follow config.toml.
///
/// Declared projects are upserted whole. A row the file no longer mentions
/// keeps its items and becomes implicit; one with no items and no entry is
/// deleted.
fn sync_projects(cache: &Cache, config: &Config) {
    let declared: Vec<Project> = config
        .project
        .iter()
        .map(|p| Project {
            id: Project::id_for(&p.name),
            name: p.name.trim().to_string(),
            repos: p.repos.clone(),
            roots: p.root_paths(),
            implicit: false,
        })
        .collect();
    for p in &declared {
        if let Err(e) = cache.upsert_project(p) {
            tracing::warn!(project = %p.id, error = %e, "could not record the project");
        }
    }
    let items = cache.load_items().unwrap_or_default();
    for existing in cache.load_projects().unwrap_or_default() {
        if declared.iter().any(|d| d.id == existing.id) {
            continue;
        }
        let has_items = items.iter().any(|i| i.project_id == existing.id);
        if has_items {
            if !existing.implicit {
                let _ = cache.upsert_project(&Project {
                    implicit: true,
                    repos: Vec::new(),
                    roots: Vec::new(),
                    ..existing
                });
            }
        } else {
            let _ = cache.delete_project(&existing.id);
        }
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

/// Put an item in another project, making the project when asked.
fn move_item(ctx: &Ctx, id: &str, project_id: Option<String>, new_project: Option<String>) {
    let target = match (project_id, new_project) {
        (Some(id), _) => id,
        (None, Some(name)) if !name.trim().is_empty() => {
            let project = Project::implicit(&name);
            if let Some(Err(e)) = ctx.with_cache(|c| c.upsert_project(&project)) {
                ctx.notice(format!("Could not make the project: {e}"));
                return;
            }
            project.id
        }
        _ => return,
    };
    let from = ctx.item(id).map(|i| i.project_id);
    edit_item(ctx, id, |item| item.project_id = target.clone());
    // An implicit project holds only what is moved there, and goes when
    // its last item does.
    if let Some(from) = from {
        ctx.with_cache(|c| drop_empty_implicit(c, &from));
    }
    ctx.send_projects();
}

/// Delete a project if it is implicit and nothing is left in it.
fn drop_empty_implicit(cache: &Cache, project_id: &str) {
    let empty = cache
        .load_items()
        .unwrap_or_default()
        .iter()
        .all(|i| i.project_id != project_id);
    let implicit = cache
        .load_projects()
        .unwrap_or_default()
        .iter()
        .any(|p| p.id == project_id && p.implicit);
    if empty && implicit {
        let _ = cache.delete_project(project_id);
    }
}

/// Make the items table and the disk agree about owned checkouts.
///
/// An owned checkout whose directory is gone is marked off disk; someone
/// deleted it by hand. A directory under the store's worktrees that no
/// item claims is removed and pruned: nothing can open it, and it is how
/// what an older schema left behind gets retired.
async fn reconcile_checkouts(ctx: &Ctx) {
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

    let known: std::collections::HashSet<std::path::PathBuf> = items
        .iter()
        .filter_map(|i| i.checkout.as_ref().map(|c| c.path.clone()))
        .collect();
    let mut removed = false;
    for dir in ctx.preparer.worktree_dirs() {
        if known.contains(&dir) {
            continue;
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
async fn sweep_items(ctx: &Ctx) {
    let stale = ctx.preparer.stale_worktrees(CHECKOUT_MAX_AGE);
    if stale.is_empty() {
        return;
    }
    let items = ctx.with_cache(|c| c.load_items().ok()).flatten().unwrap_or_default();

    for path in stale {
        let owner = items
            .iter()
            .find(|i| i.checkout.as_ref().is_some_and(|c| c.path == path && c.owned));
        match owner {
            Some(item) if !item.is_archived() => match archive_item(ctx, &item.id).await {
                Ok(()) => tracing::info!(id = %item.id, "archived a stale checkout"),
                // Dirty, most likely. It stays; the reader has work in it.
                Err(message) => tracing::info!(id = %item.id, %message, "left a stale checkout alone"),
            },
            Some(_) => {}
            None => {
                // Reconciliation runs first, so this is a directory that
                // appeared in between. Same treatment.
                tracing::info!(path = %path.display(), "removing a stale worktree directory no item claims");
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
}

/// Leave Active. An owned checkout's directory goes; the branch, the row,
/// the threads and the knowledge stay.
async fn archive_item(ctx: &Ctx, id: &str) -> Result<(), String> {
    let Some(mut item) = ctx.item(id) else {
        return Err("That item is gone".into());
    };
    let Some(repo) = RepoId::parse(&item.repo_full_name) else {
        return Err(format!("{} is not a repository name", item.repo_full_name));
    };
    if let Some(checkout) = item.checkout.as_mut().filter(|c| c.owned && c.on_disk) {
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
        checkout.on_disk = false;
    } else if ctx.is_open(id) {
        ctx.close_review().await;
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
    ctx.with_cache(|c| drop_empty_implicit(c, &item.project_id));
    ctx.send_projects();
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

    let mut item = item;
    if let Some(c) = item.checkout.as_mut() {
        c.head_sha = git_checkout.head_sha.clone();
        c.on_disk = true;
    }
    item.archived_at = None;
    item.last_opened_at = Timestamp::now();
    ctx.with_cache(|c| c.update_item(&item));
    ctx.send_items();

    finish_open(ctx, item, None, git_checkout, false).await
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

/// Everything after a checkout is in hand: the file list, the tree, the
/// review context, `ReviewReady`, and the threads.
async fn finish_open(
    ctx: &Ctx,
    item: Item,
    pull_request: Option<Arc<PullRequest>>,
    checkout: perch_git::Checkout,
    offline: bool,
) -> bool {
    let events = &ctx.events;
    let _ = events.send(Event::ReviewProgress { message: "Computing the diff".into(), percent: None });

    let files = match changed_files(&ctx.preparer, &checkout).await {
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
    let head_for_threads = checkout.head_sha.clone();
    let worktree_for_threads = checkout.worktree.clone();
    let item = Arc::new(item);
    if let Ok(mut guard) = ctx.slot.lock() {
        *guard = Some(ReviewContext {
            checkout,
            files: files.clone(),
            item_id: item.id.clone(),
            pull_request: pull_request.clone(),
            body: None,
            issue: None,
        });
    }
    tracing::info!(item = %item.id, files = files.len(), tree = tree.len(), "review ready");
    let _ = events.send(Event::ReviewReady {
        item: item.clone(),
        pull_request,
        files,
        tree: Arc::new(tree),
        stale_head,
        offline,
    });
    {
        let ctx = ctx.clone();
        let item_id = item.id.clone();
        tokio::spawn(async move {
            mark_outdated_threads(&ctx, &item_id, &head_for_threads, &worktree_for_threads).await;
            ctx.send_threads(&item_id);
        });
    }
    true
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
        .cut_branch(&repo, &wanted, &perch_git::home_ref(&default))
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
    let mut item = Item::new(&issue_item.project_id, &issue_item.repo_full_name, now);
    item.checkout = Some(ItemCheckout {
        branch: cut.branch,
        base_ref: default,
        head_sha,
        path: cut.worktree,
        owned: true,
        on_disk: true,
        churn: Some((0, 0)),
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
    let cut = ctx
        .preparer
        .cut_branch(&repo, &checkout.branch, &head)
        .await
        .map_err(|e| e.to_string())?;

    let now = Timestamp::now();
    let mut item = Item::new(&source.project_id, &source.repo_full_name, now);
    item.checkout = Some(ItemCheckout {
        branch: cut.branch,
        // The source's head, so what is shown is what has been done on top.
        base_ref: head.clone(),
        head_sha: head,
        path: cut.worktree,
        owned: true,
        on_disk: true,
        churn: Some((0, 0)),
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
    files: Arc<Vec<perch_git::ChangedFile>>,
    item_id: String,
    /// `None` for a branch with no pull request.
    pull_request: Option<Arc<PullRequest>>,
    /// The description, once the discussion has been fetched.
    body: Option<String>,
    /// The issue and its comments, for a question about an issue.
    issue: Option<Arc<(perch_github::Issue, Vec<perch_github::IssueComment>)>>,
}

/// One poll of the notifications API. Returns how long to wait before the next.
async fn refresh(ctx: &Ctx, source: &str, fetches: &Fetches) -> Duration {
    let github = &ctx.github;
    let cache = &ctx.cache;
    let events = &ctx.events;
    let config = ctx.config.as_ref();
    let _ = events.send(Event::Status(Status::Refreshing));

    let since = cache
        .as_ref()
        .and_then(|c| c.lock().ok())
        .and_then(|c| c.meta(LAST_MODIFIED_KEY).ok().flatten());

    match github.list_threads(since.as_deref()).await {
        Ok(result) => {
            let poll_after = result
                .poll_interval
                .max(Duration::from_secs(config.poll_interval_secs.max(1)));

            if result.threads.is_none() {
                tracing::info!("inbox unchanged, keeping the cached list");
            }

            if let Some(threads) = result.threads {
                tracing::info!(count = threads.len(), "inbox refreshed");
                if let Some(cache) = cache.as_ref() {
                    if let Ok(mut guard) = cache.lock() {
                        match guard.store_threads(&threads) {
                            // The stored timestamp must describe the stored
                            // list. Writing it on any other path would let a
                            // later probe answer "unchanged" for a list we
                            // never saved, and the cache could never heal.
                            Ok(()) => {
                                if let Some(lm) = result.last_modified.as_ref() {
                                    let _ = guard.set_meta(LAST_MODIFIED_KEY, lm);
                                }
                                // A thread with activity newer than the
                                // moment it was dismissed stops being done.
                                // This runs before the list goes out so the
                                // UI never renders a new inbox against marks
                                // that inbox has already invalidated.
                                match guard.reconcile_marks(&threads) {
                                    Ok(true) => send_marks(&guard, events),
                                    Ok(false) => {}
                                    Err(e) => {
                                        tracing::warn!(error = %e, "could not reconcile marks")
                                    }
                                }
                            }
                            Err(e) => tracing::warn!(error = %e, "could not cache threads"),
                        }
                    }
                }

                let _ = events.send(Event::Threads {
                    threads: threads.clone(),
                    from_cache: false,
                });

                // A notification about an item is the item's attention, and
                // a review request makes the item it is about.
                attend(ctx, &threads).await;

                fetch_details(
                    github.clone(),
                    cache.clone(),
                    events.clone(),
                    fetches.clone(),
                    threads,
                );
            }

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

/// Set attention on the items these notifications are about, and make an
/// item for a review request that has none.
///
/// The one case a notification makes an item is the case the tool exists
/// for. Every other reason on an unknown subject stays in the inbox.
async fn attend(ctx: &Ctx, threads: &[Thread]) {
    let mut changed = false;
    for thread in threads {
        let kind = match thread.subject.kind {
            SubjectKind::PullRequest => ItemKind::PullRequest,
            SubjectKind::Issue => ItemKind::Issue,
            _ => continue,
        };
        let Some(number) = thread.number_hint() else { continue };
        let repo = thread.repository.full_name.as_str();
        let existing = ctx
            .with_cache(|c| c.find_item_by_subject(repo, kind, number).ok().flatten())
            .flatten();
        let reason = thread.reason.wire().to_string();

        let item = match existing {
            Some(item) => item,
            None if thread.reason == Reason::ReviewRequested && kind == ItemKind::PullRequest => {
                match ctx.github.fetch_pull_request(repo, number).await {
                    Ok(pr) => {
                        let project = ctx.project_for(&pr.repo_full_name);
                        let mut item = Item::new(&project, &pr.repo_full_name, Timestamp::now());
                        item.subject = Some(ItemSubject::PullRequest(pr));
                        match ctx.with_cache(|c| c.insert_item(&item)) {
                            Some(Ok(())) => {
                                tracing::info!(repo, number, "review request made an item");
                                changed = true;
                                item
                            }
                            Some(Err(e)) => {
                                tracing::warn!(repo, number, error = %e, "could not make the item");
                                continue;
                            }
                            None => continue,
                        }
                    }
                    Err(e) => {
                        tracing::warn!(repo, number, error = %e, "could not fetch the requested review");
                        continue;
                    }
                }
            }
            None => continue,
        };
        if let Some(Ok(true)) =
            ctx.with_cache(|c| c.touch_attention(&item.id, thread.updated_at, &reason))
        {
            changed = true;
        }
    }
    if changed {
        ctx.send_items();
    }
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

/// Every worktree under a project's roots is an item, and one whose
/// directory has gone is deleted, or archived when it has threads. Returns
/// whether anything changed.
async fn scan_roots(ctx: &Ctx) -> bool {
    let projects = ctx.with_cache(|c| c.load_projects().unwrap_or_default()).unwrap_or_default();
    let git = ctx.preparer.git();
    let mut changed = false;
    // Which unowned items were seen, so the rest can be judged gone.
    let mut seen: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
    let mut scanned_repos: std::collections::HashSet<String> = std::collections::HashSet::new();

    for project in &projects {
        for root in &project.roots {
            let scan = match perch_git::scan_root(git, root).await {
                Ok(s) => s,
                Err(e) => {
                    ctx.notice(format!("{}: {e}", root.display()));
                    continue;
                }
            };
            let Some(repo) = scan.repo_full_name.clone() else {
                ctx.notice(format!("{}: origin is not a GitHub repository", root.display()));
                continue;
            };
            if !project.claims(&repo) {
                ctx.notice(format!("{}: {repo} is not one of {}'s repositories", root.display(), project.name));
                continue;
            }
            scanned_repos.insert(repo.to_lowercase());
            for entry in &scan.entries {
                let Some(branch) = entry.branch.clone() else { continue };
                if branch == scan.default_branch {
                    continue;
                }
                let path = entry.path.canonicalize().unwrap_or_else(|_| entry.path.clone());
                seen.insert(path.clone());
                let churn = perch_git::branch_churn(git, &entry.path, &format!("origin/{}", scan.default_branch))
                    .await
                    .ok();
                let existing = ctx.with_cache(|c| c.find_item_by_branch(&repo, &branch).ok().flatten()).flatten();
                match existing {
                    Some(mut item) => {
                        let Some(c) = item.checkout.as_mut() else { continue };
                        let moved = c.head_sha != entry.head_sha || !c.on_disk || c.churn != churn || c.path != path;
                        if moved && !c.owned {
                            c.head_sha = entry.head_sha.clone();
                            c.on_disk = true;
                            c.churn = churn;
                            c.path = path;
                            ctx.with_cache(|cache| cache.update_item(&item));
                            changed = true;
                        }
                    }
                    None => {
                        let mut item = Item::new(&project.id, &repo, Timestamp::now());
                        item.checkout = Some(ItemCheckout {
                            branch,
                            base_ref: scan.default_branch.clone(),
                            head_sha: entry.head_sha.clone(),
                            path,
                            owned: false,
                            on_disk: true,
                            churn,
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

    // Unowned items in scanned repositories whose directory was not seen.
    let items = ctx.with_cache(|c| c.load_items().unwrap_or_default()).unwrap_or_default();
    for item in items {
        let Some(c) = item.checkout.as_ref() else { continue };
        if c.owned || !scanned_repos.contains(&item.repo_full_name.to_lowercase()) {
            continue;
        }
        let path = c.path.canonicalize().unwrap_or_else(|_| c.path.clone());
        if seen.contains(&path) || (item.is_archived() && !c.on_disk) {
            continue;
        }
        if c.path.join(".git").exists() {
            // Still there, on some other branch than we knew; the scan will
            // have made a row for that branch. Leave this one for the rule.
        }
        let has_threads = ctx
            .with_cache(|cache| cache.load_comment_threads(&item.id).map(|t| !t.is_empty()).unwrap_or(false))
            .unwrap_or(false);
        if has_threads {
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
    let projects = ctx.with_cache(|c| c.load_projects().unwrap_or_default()).unwrap_or_default();
    let mut changed = false;
    for item in items {
        if item.subject.is_some() || item.is_archived() {
            continue;
        }
        let Some(checkout) = item.checkout.as_ref() else { continue };
        let Some((head_owner, _)) = item.repo_full_name.split_once('/') else { continue };
        // The pull request may be against any of the project's repositories
        // when the checkout is a fork.
        let mut candidates: Vec<String> = vec![item.repo_full_name.clone()];
        if let Some(p) = projects.iter().find(|p| p.id == item.project_id) {
            for r in &p.repos {
                if !candidates.iter().any(|c| c.eq_ignore_ascii_case(r)) {
                    candidates.push(r.clone());
                }
            }
        }
        for repo in candidates {
            let key = format!("pull_lookup_etag:{}:{}:{}", repo.to_lowercase(), head_owner, checkout.branch);
            let etag = ctx.with_cache(|c| c.meta(&key).ok().flatten()).flatten();
            match ctx.github.find_pull_for_branch(&repo, head_owner, &checkout.branch, etag.as_deref()).await {
                Ok(perch_github::PullLookup::Unchanged) => {}
                Ok(perch_github::PullLookup::Answer { pull, etag }) => {
                    if let Some(etag) = etag {
                        ctx.with_cache(|c| c.set_meta(&key, &etag));
                    }
                    if let Some(pr) = pull {
                        tracing::info!(item = %item.id, number = pr.number, "branch gained a pull request");
                        let mut item = item.clone();
                        if let Some(c) = item.checkout.as_mut() {
                            c.base_ref = pr.base_ref.clone();
                        }
                        item.subject = Some(ItemSubject::PullRequest(*pr));
                        match ctx.with_cache(|c| c.update_item(&item)) {
                            Some(Err(e)) => tracing::warn!(error = %e, "could not attach the pull request"),
                            _ => changed = true,
                        }
                        break;
                    }
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

    let (issue, comments, offline) = match ctx.github.fetch_issue(&repo_full_name, number).await {
        Ok(issue) => {
            let comments = match ctx.github.fetch_issue_comments(&repo_full_name, number).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(repo = %repo_full_name, number, error = %e, "could not fetch the comments");
                    Vec::new()
                }
            };
            (issue, comments, false)
        }
        Err(e) => match existing.as_ref().and_then(|i| i.issue().cloned()) {
            Some(snapshot) => {
                tracing::info!(repo = %repo_full_name, number, error = %e, "showing the issue from its snapshot");
                ctx.notice("Offline; showing the issue as it was last fetched");
                (snapshot, Vec::new(), true)
            }
            None => {
                let _ = events.send(Event::ReviewFailed { message: e.to_string() });
                return false;
            }
        },
    };

    let now = Timestamp::now();
    let is_new = existing.is_none();
    let mut item = match existing {
        Some(item) => item,
        None => {
            let project = ctx.project_for(&issue.repo_full_name);
            Item::new(&project, &issue.repo_full_name, now)
        }
    };
    item.subject = Some(ItemSubject::Issue(issue.clone()));
    item.archived_at = None;
    item.last_opened_at = now;
    let written = ctx.with_cache(|c| if is_new { c.insert_item(&item) } else { c.update_item(&item) });
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
            item_id: item.id.clone(),
            pull_request: None,
            body: None,
            issue: Some(Arc::new(((*issue).clone(), (*comments).clone()))),
        });
    }
    tracing::info!(repo = %issue.repo_full_name, number, comments = comments.len(), offline, "issue ready");
    let _ = events.send(Event::IssueReady { item: item.clone(), issue: issue.clone(), comments: comments.clone(), offline });
    ctx.want_avatars(
        std::iter::once(issue.author_avatar_url.clone())
            .chain(comments.iter().map(|c| c.user_avatar_url.clone())),
    );
    ctx.send_threads(&item.id);

    if !offline {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let project = ctx.config.project_for(&issue.repo_full_name);
            let messages = crate::knowledge::issue_to_messages(&issue, &comments, &project);
            match ingest(&ctx, messages).await {
                Ok(n) => {
                    let _ = ctx.events.send(Event::Archived { repo_full_name: issue.repo_full_name.clone(), number, added: n });
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
    let knowledge = ctx.knowledge.clone();
    let write_skill = ctx.config.knowledge.skills;
    // File writes, off the runtime's threads.
    let outcome = tokio::task::spawn_blocking(move || {
        let before = knowledge.projects()?;
        let ingested = knowledge.ingest(&messages)?;
        let after = knowledge.projects()?;
        // The skill names every project, so it is rewritten when the set
        // changes, and written the first time anything is here at all.
        let dirs = crate::knowledge::skill_dirs();
        let missing = dirs.iter().any(|d| !d.join("SKILL.md").is_file());
        if write_skill && (before != after || missing) {
            knowledge.write_skill(&perch_knowledge::SkillConfig { projects: after, dirs })?;
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
/// the reader's own in the same repository attaches to that item rather
/// than making a second one.
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
        None => ctx
            .with_cache(|c| {
                c.find_item_by_branch(&pull_request.repo_full_name, &pull_request.head_ref)
                    .ok()
                    .flatten()
            })
            .flatten()
            .filter(|i| i.subject.is_none()),
    };
    let own_checkout = existing
        .as_ref()
        .and_then(|i| i.checkout.as_ref())
        .filter(|c| !c.owned)
        .cloned();
    let existing_churn = existing.as_ref().and_then(|i| i.checkout.as_ref()).and_then(|c| c.churn);
    let branch = match &own_checkout {
        Some(c) => c.branch.clone(),
        None => format!("perch/pr-{number}"),
    };

    let checkout = if let Some(own) = &own_checkout {
        open_own_checkout(ctx, &repo, own, &pull_request.base_ref).await
    } else if offline {
        let path = existing.as_ref().and_then(|i| i.checkout.as_ref()).map(|c| c.path.clone());
        reopen_from_disk(ctx, &repo, number, &branch, path).await
    } else {
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
    };

    let checkout = match checkout {
        Ok(c) => c,
        Err(e) => fail!("{e}"),
    };

    // The row. Every column that can move is rewritten from what just
    // happened, and an archived row is back in service.
    let now = Timestamp::now();
    let is_new = existing.is_none();
    let mut item = match existing {
        Some(item) => item,
        None => {
            let project = ctx.project_for(&pull_request.repo_full_name);
            Item::new(&project, &pull_request.repo_full_name, now)
        }
    };
    // CI turning red is attention. Nothing else about the snapshot is.
    let ci_failed_now = pull_request.checks == Some(perch_github::CheckStatus::Failure)
        && item.pull_request().and_then(|p| p.checks) != Some(perch_github::CheckStatus::Failure);
    item.subject = Some(ItemSubject::PullRequest(pull_request.clone()));
    item.checkout = Some(ItemCheckout {
        branch: branch.clone(),
        base_ref: pull_request.base_ref.clone(),
        head_sha: checkout.head_sha.clone(),
        path: checkout.worktree.clone(),
        owned: own_checkout.is_none(),
        on_disk: true,
        churn: existing_churn,
    });
    item.archived_at = None;
    item.last_opened_at = now;
    if ci_failed_now {
        item.attention_at = Some(now);
        item.attention_reason = Some("ci_failed".into());
    }
    let written = ctx.with_cache(|c| if is_new { c.insert_item(&item) } else { c.update_item(&item) });
    if let Some(Err(e)) = written {
        tracing::warn!(id = %item.id, error = %e, "could not record the item");
    }
    ctx.send_items();

    let _ = events.send(Event::ReviewProgress {
        message: "Computing the diff".into(),
        percent: None,
    });

    let files = match changed_files(&ctx.preparer, &checkout).await {
        Ok(f) => Arc::new(f),
        Err(e) => fail!("{e}"),
    };

    let worktree = checkout.worktree.clone();
    let tree = tokio::task::spawn_blocking(move || {
        perch_git::list_tree(&worktree, 50_000).unwrap_or_default()
    })
    .await
    .unwrap_or_default();

    let stale_head = checkout.stale.clone();
    let worktree_label = checkout.worktree.display().to_string();
    let head_for_threads = checkout.head_sha.clone();
    let worktree_for_threads = checkout.worktree.clone();
    let pull_request = Arc::new(pull_request);
    let item = Arc::new(item);
    if let Ok(mut guard) = ctx.slot.lock() {
        *guard = Some(ReviewContext {
            checkout,
            files: files.clone(),
            item_id: item.id.clone(),
            pull_request: Some(pull_request.clone()),
            body: None,
            issue: None,
        });
    }

    tracing::info!(
        repo = %pull_request.repo_full_name,
        number,
        files = files.len(),
        tree = tree.len(),
        worktree = %worktree_label,
        stale = stale_head.is_some(),
        offline,
        "review ready"
    );
    let _ = events.send(Event::ReviewReady {
        item: item.clone(),
        pull_request: Some(pull_request.clone()),
        files,
        tree: Arc::new(tree),
        stale_head,
        offline,
    });

    // Threads paint after the code, not before, and the outdated check is
    // a git call per moved thread, so both go on their own task.
    {
        let ctx = ctx.clone();
        let item_id = item.id.clone();
        tokio::spawn(async move {
            mark_outdated_threads(&ctx, &item_id, &head_for_threads, &worktree_for_threads).await;
            ctx.send_threads(&item_id);
        });
    }

    // The discussion arrives after the checkout, on its own task, so the
    // review paints first. Offline there is nothing to fetch.
    if !offline {
        let ctx = ctx.clone();
        tokio::spawn(async move { archive_discussion(&ctx, &pull_request).await });
    }
    true
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
async fn archive_discussion(ctx: &Ctx, pr: &perch_github::PullRequest) {
    let discussion = match ctx.github.fetch_discussion(&pr.repo_full_name, pr.number).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(repo = %pr.repo_full_name, number = pr.number, error = %e, "could not fetch the discussion");
            let _ = ctx.events.send(Event::Notice(format!("Could not archive the discussion: {e}")));
            return;
        }
    };

    if let Ok(mut guard) = ctx.slot.lock() {
        if let Some(open) = guard
            .as_mut()
            .filter(|c| c.pull_request.as_ref().is_some_and(|p| p.number == pr.number))
        {
            open.body = Some(discussion.body.clone());
        }
    }

    let project = ctx.config.project_for(&pr.repo_full_name);
    let messages = crate::knowledge::to_messages(pr, &discussion, &project);
    match ingest(ctx, messages).await {
        Ok(added) => {
            tracing::info!(repo = %pr.repo_full_name, number = pr.number, added, "discussion archived");
            let _ = ctx.events.send(Event::Archived {
                repo_full_name: pr.repo_full_name.clone(),
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
            (c.checkout.clone(), c.pull_request.clone(), c.body.clone(), c.issue.clone())
        })
    });
    let Some((checkout, pull_request, body, issue)) = open else {
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
        Some(a) => crate::ask::load_display(&ctx.preparer, &checkout, &a.path).await.ok(),
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
async fn changed_files(
    preparer: &Preparer,
    checkout: &perch_git::Checkout,
) -> Result<Vec<perch_git::ChangedFile>, perch_git::Error> {
    let git = preparer.git();
    let args = |mode: &str| {
        [
            "diff".to_string(),
            "-M".to_string(),
            mode.to_string(),
            "-z".to_string(),
            checkout.merge_base.clone(),
            checkout.head_sha.clone(),
        ]
    };
    let timeout = Duration::from_secs(60);

    let name_status = git.run(Some(&checkout.bare), args("--name-status"), timeout).await?;
    let numstat = git.run(Some(&checkout.bare), args("--numstat"), timeout).await?;

    Ok(perch_git::diff::merge_file_lists(
        perch_git::diff::parse_name_status(&name_status),
        perch_git::diff::parse_numstat(&numstat),
    ))
}

/// Read one file, diff it against the base, and highlight both sides.
async fn open_file(
    preparer: Arc<Preparer>,
    slot: Arc<std::sync::Mutex<Option<ReviewContext>>>,
    events: EventSender<Event>,
    path: String,
) {
    let Some((checkout, changed)) = slot
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|c| (c.checkout.clone(), c.files.clone())))
    else {
        let _ = events.send(Event::FileFailed {
            path,
            message: "no checkout is open".into(),
        });
        return;
    };

    let entry = changed.iter().find(|f| f.path == path).cloned();

    // The head side comes from the worktree, so a file the pull request never
    // touched costs no git call at all.
    let head = match &entry {
        Some(f) if f.status == perch_git::ChangeStatus::Deleted => String::new(),
        _ => {
            let full = checkout.worktree.join(&path);
            match tokio::fs::read(&full).await {
                Ok(bytes) if bytes.len() > MAX_FILE_BYTES => {
                    let _ = events.send(Event::FileFailed {
                        path,
                        message: "file is too large to display".into(),
                    });
                    return;
                }
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(text) => text,
                    Err(_) => {
                        let _ = events.send(Event::FileFailed {
                            path,
                            message: "file is not text".into(),
                        });
                        return;
                    }
                },
                Err(e) => {
                    let _ = events.send(Event::FileFailed { path, message: e.to_string() });
                    return;
                }
            }
        }
    };

    // The base side only exists for files the pull request changed.
    let base = match &entry {
        None | Some(perch_git::ChangedFile { status: perch_git::ChangeStatus::Added, .. }) => {
            String::new()
        }
        Some(f) => {
            let source = f.old_path.clone().unwrap_or_else(|| f.path.clone());
            preparer
                .show_blob(&checkout.bare, &checkout.merge_base, &source)
                .await
                .unwrap_or_default()
        }
    };

    let status = entry.as_ref().map(|f| f.status);
    let binary = entry.as_ref().map(|f| f.binary).unwrap_or(false);

    // Parsing and diffing are CPU work; keep them off the async runtime's
    // reactor threads.
    let loaded = tokio::task::spawn_blocking(move || {
        build_loaded_file(path, head, base, status, binary)
    })
    .await;

    match loaded {
        Ok(file) => {
            tracing::info!(
                path = %file.path,
                lines = file.buffer.len(),
                changes = file.buffer.change_starts.len(),
                "file loaded"
            );
            let _ = events.send(Event::FileLoaded { path: file.path.clone(), file: Arc::new(file) });
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
        };
    }

    let buffer = match status {
        Some(_) => {
            let diff = file_diff(&base, &head);
            DisplayBuffer::unified(&head, &base, &diff)
        }
        // A file outside the pull request is shown plainly, which is what
        // makes browsing the whole tree useful during a review.
        None => DisplayBuffer::plain(&head),
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
    }
}

type SharedCache = Option<Arc<std::sync::Mutex<Cache>>>;

/// The cache, and whether an older one had to be thrown away.
fn open_cache() -> (SharedCache, bool) {
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

/// Queue detail and avatar fetches for a freshly loaded inbox.
fn fetch_details(
    github: Arc<GitHub>,
    cache: SharedCache,
    events: EventSender<Event>,
    fetches: Fetches,
    threads: Vec<Thread>,
) {
    // Repository owner avatars, for the section headers.
    let mut seen_avatars = std::collections::HashSet::new();
    for t in &threads {
        if !t.repository.owner_avatar_url.is_empty()
            && seen_avatars.insert(t.repository.owner_avatar_url.clone())
        {
            spawn_avatar(
                github.clone(),
                cache.clone(),
                events.clone(),
                fetches.avatars.clone(),
                t.repository.owner_avatar_url.clone(),
            );
        }
    }

    for thread in threads {
        let Some(source) = thread.detail_source() else {
            continue;
        };

        let updated_at = thread.updated_at.to_string();

        // A thread whose `updated_at` has not moved keeps its cached detail,
        // so a restart costs no requests at all.
        //
        // The lock is released before anything else runs. `spawn_avatar`
        // takes the same mutex to look on disk, and a std Mutex is not
        // reentrant, so calling it while still holding the guard deadlocked
        // the whole pipeline on the first cached thread. That is why avatars
        // stopped after the handful loaded before this loop.
        let cached_detail = cache
            .as_ref()
            .and_then(|handle| handle.lock().ok())
            .and_then(|guard| guard.load_detail(&thread.id, &updated_at).ok().flatten());

        if let Some(detail) = cached_detail {
            let avatar = detail.author_avatar_url.clone();
            let _ = events.send(Event::Detail {
                thread_id: thread.id.clone(),
                detail,
            });
            if !avatar.is_empty() && seen_avatars.insert(avatar.clone()) {
                spawn_avatar(
                    github.clone(),
                    cache.clone(),
                    events.clone(),
                    fetches.avatars.clone(),
                    avatar,
                );
            }
            continue;
        }

        let key = (thread.id.clone(), updated_at.clone());
        if fetches.unresolved.lock().is_ok_and(|set| set.contains(&key)) {
            continue;
        }

        let github = github.clone();
        let cache = cache.clone();
        let events = events.clone();
        let fetches = fetches.clone();
        let thread_id = thread.id.clone();

        tokio::spawn(async move {
            let Ok(_permit) = fetches.details.acquire().await else {
                return;
            };
            let fetched = match &source {
                DetailSource::Subject { url, kind } => {
                    github.fetch_detail(url, kind).await.map(Some)
                }
                DetailSource::WorkflowRun(query) => github.fetch_workflow_run_detail(query).await,
            };
            match fetched {
                Ok(Some(detail)) => {
                    if let Some(cache) = cache.as_ref() {
                        if let Ok(guard) = cache.lock() {
                            let _ = guard.store_detail(&thread_id, &updated_at, &detail);
                        }
                    }
                    let avatar = detail.author_avatar_url.clone();
                    tracing::debug!(thread_id, number = detail.number, "detail fetched");
                    let _ = events.send(Event::Detail { thread_id, detail });
                    if !avatar.is_empty() {
                        spawn_avatar(github, cache, events, fetches.avatars.clone(), avatar);
                    }
                }
                Ok(None) => {
                    tracing::debug!(thread_id, ?source, "no run fits the notification");
                    if let Ok(mut set) = fetches.unresolved.lock() {
                        set.insert(key);
                    }
                }
                Err(e) => tracing::debug!(error = %e, ?source, "detail fetch failed"),
            }
        });
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

/// Wall-clock time in the user's zone, for the status bar.
fn local_clock_time(epoch: i64) -> Option<String> {
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

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn ids_resolve_against_the_live_inbox() {
        let mut cache = Cache::open_in_memory().unwrap();
        cache.store_threads(&[thread("1", "one"), thread("2", "two")]).unwrap();

        let got = resolve_threads(&cache, &ids(&["2", "1"]));
        // In the order asked for, so a batch marks exactly what it named.
        assert_eq!(
            got.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["2", "1"]
        );
    }

    #[test]
    fn a_snapshot_covers_a_thread_the_api_has_dropped() {
        let mut cache = Cache::open_in_memory().unwrap();
        let t = thread("1", "one");
        cache.store_threads(std::slice::from_ref(&t)).unwrap();
        cache.set_saved(&[&t], true).unwrap();
        // GitHub stops returning it, but the bookmark keeps a copy.
        cache.store_threads(&[]).unwrap();

        let got = resolve_threads(&cache, &ids(&["1"]));
        assert_eq!(got, vec![t]);
    }

    #[test]
    fn the_live_thread_wins_over_a_stale_snapshot() {
        let mut cache = Cache::open_in_memory().unwrap();
        let old = thread("1", "old title");
        cache.set_saved(&[&old], true).unwrap();

        let fresh = thread("1", "new title");
        cache.store_threads(std::slice::from_ref(&fresh)).unwrap();

        let got = resolve_threads(&cache, &ids(&["1"]));
        assert_eq!(got[0].subject.title, "new title");
    }

    #[test]
    fn an_id_nothing_knows_about_is_skipped_rather_than_invented() {
        let mut cache = Cache::open_in_memory().unwrap();
        cache.store_threads(&[thread("1", "one")]).unwrap();

        let got = resolve_threads(&cache, &ids(&["1", "ghost"]));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "1");
    }
}
