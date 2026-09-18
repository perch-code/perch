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
    GitHub, Mark, RunRecord, RunStatus, SubjectDetail, Thread, ThreadOrigin, Worktree,
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
    /// Turn a notification into a checkout and open the review view.
    OpenPullRequest { repo_full_name: String, number: u64 },
    /// Read, diff, and highlight one file of the open checkout.
    OpenFile { path: String },
    /// Ask the language server about a position. `token` lets the UI drop an
    /// answer that arrives after the pointer has moved on.
    Hover { token: u64, path: String, line: u32, character: u32 },
    Definition { token: u64, path: String, line: u32, character: u32 },
    CloseReview,

    /// Open a checkout from the Worktrees list. Fetches the pull request
    /// when it can, and opens what is on disk when it cannot.
    OpenWorktree { id: String },
    /// Remove the directory, keep the branch and the row. Refused when
    /// there is uncommitted work in it.
    ArchiveWorktree { id: String },
    /// Put an archived worktree back on disk and open it.
    RestoreWorktree { id: String },
    /// Delete the branch and the row of an archived worktree.
    ForgetWorktree { id: String },

    /// Ask the agent a question in the open worktree. `thread_id` of `None`
    /// starts a thread; the anchor is ignored on a follow-up.
    Ask {
        worktree_id: String,
        thread_id: Option<String>,
        anchor: Option<Anchor>,
        question: String,
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
    /// Every worktree Perch has a record of, active and archived. Whole,
    /// for the same reason marks are.
    Worktrees(Vec<Worktree>),
    /// Something asked of a worktree could not be done. The message is for
    /// the status line: "has uncommitted changes" is the usual one.
    WorktreeFailed { id: String, message: String },
    /// A pull request's discussion was archived under the knowledge root.
    /// `added` counts records that were new or changed.
    Archived { repo_full_name: String, number: u64, added: usize },
    /// Something worth a line in the status bar and nothing more.
    Notice(String),
    /// What config.toml says about the agent, for the pickers.
    AgentConfig(perch_github::AgentConfig),
    /// Every thread in a worktree. Whole, like `Worktrees`.
    CommentThreads { worktree_id: String, threads: Vec<CommentThread> },
    /// One step of a run, applied in place to the run it names.
    AgentStep { run_id: String, thread_id: String, event: perch_agent::RunEvent },
    Status(Status),

    /// A step of preparing a checkout.
    ReviewProgress { message: String, percent: Option<u8> },
    ReviewReady {
        pull_request: Arc<perch_github::PullRequest>,
        worktree: Arc<Worktree>,
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

    pub fn open_pull_request(&self, repo_full_name: &str, number: u64) {
        let _ = self.commands.send(Command::OpenPullRequest {
            repo_full_name: repo_full_name.to_string(),
            number,
        });
    }

    pub fn open_file(&self, path: &str) {
        let _ = self.commands.send(Command::OpenFile { path: path.to_string() });
    }

    pub fn close_review(&self) {
        let _ = self.commands.send(Command::CloseReview);
    }

    pub fn open_worktree(&self, id: &str) {
        let _ = self.commands.send(Command::OpenWorktree { id: id.to_string() });
    }

    pub fn archive_worktree(&self, id: &str) {
        let _ = self.commands.send(Command::ArchiveWorktree { id: id.to_string() });
    }

    pub fn restore_worktree(&self, id: &str) {
        let _ = self.commands.send(Command::RestoreWorktree { id: id.to_string() });
    }

    pub fn forget_worktree(&self, id: &str) {
        let _ = self.commands.send(Command::ForgetWorktree { id: id.to_string() });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ask(
        &self,
        worktree_id: &str,
        thread_id: Option<String>,
        anchor: Option<Anchor>,
        question: &str,
        model: &str,
        effort: perch_agent::Effort,
    ) {
        let _ = self.commands.send(Command::Ask {
            worktree_id: worktree_id.to_string(),
            thread_id,
            anchor,
            question: question.to_string(),
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
    let cache = open_cache();
    tracing::info!(cache = cache.is_some(), "cache opened");
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
            send_worktrees(&guard, &events);
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
    };

    // Bring the table and the disk into agreement before any command can
    // make a directory. Inline, not spawned: a checkout made by a launch
    // hook while this ran would have no row yet, and would be taken for a
    // stray and removed from under it. It is a directory listing and at
    // most a prune, so the inbox is not kept waiting.
    reconcile_worktrees(&ctx).await;
    {
        // Then put away whatever nobody has opened in a fortnight. Archived,
        // not deleted: the branch and the row stay, so restoring is a
        // checkout. Only rows it knows are touched, so this can run beside
        // whatever the reader does next.
        let ctx = ctx.clone();
        tokio::spawn(async move { sweep_worktrees(&ctx).await });
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

        match command {
            Some(Command::Refresh) | None => {
                poll_after = refresh(
                    &github,
                    &cache,
                    &events,
                    config.as_ref(),
                    &source,
                    &fetches,
                )
                .await;
            }
            Some(Command::OpenPullRequest { repo_full_name, number }) => {
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if open_pull_request(&ctx, repo_full_name, number).await {
                        start_language_server(ctx.slot, ctx.language_server, ctx.events).await;
                    }
                });
            }
            Some(Command::OpenWorktree { id }) | Some(Command::RestoreWorktree { id }) => {
                // Restoring is opening: `prepare` checks the branch back out
                // when the directory is gone, and the row's archived flag
                // clears on the way through. One path, not two.
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if open_worktree(&ctx, id).await {
                        start_language_server(ctx.slot, ctx.language_server, ctx.events).await;
                    }
                });
            }
            Some(Command::ArchiveWorktree { id }) => {
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if let Err(message) = archive_worktree(&ctx, &id).await {
                        tracing::warn!(id, %message, "could not archive worktree");
                        let _ = ctx.events.send(Event::WorktreeFailed { id, message });
                    }
                });
            }
            Some(Command::Ask { worktree_id, thread_id, anchor, question, model, effort }) => {
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    ask(&ctx, worktree_id, thread_id, anchor, question, model, effort).await;
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
            Some(Command::ForgetWorktree { id }) => {
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if let Err(message) = forget_worktree(&ctx, &id).await {
                        tracing::warn!(id, %message, "could not forget worktree");
                        let _ = ctx.events.send(Event::WorktreeFailed { id, message });
                    }
                });
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

/// How long a worktree stays on disk without being opened before the sweep
/// archives it.
const WORKTREE_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// Everything a worktree operation needs, so a spawned task takes one
/// clone rather than six.
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
}

impl Ctx {
    fn send_threads(&self, worktree_id: &str) {
        let threads = self.with_cache(|c| c.load_comment_threads(worktree_id));
        match threads {
            Some(Ok(threads)) => {
                let _ = self.events.send(Event::CommentThreads {
                    worktree_id: worktree_id.to_string(),
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

    fn worktree(&self, id: &str) -> Option<Worktree> {
        self.with_cache(|c| c.load_worktrees().ok())
            .flatten()?
            .into_iter()
            .find(|w| w.id == id)
    }

    fn send_worktrees(&self) {
        self.with_cache(|c| send_worktrees(c, &self.events));
    }

    /// Whether the open review is this worktree.
    fn is_open(&self, worktree: &Worktree) -> bool {
        self.slot
            .lock()
            .ok()
            .is_some_and(|g| g.as_ref().is_some_and(|c| c.checkout.worktree == worktree.path))
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

fn send_worktrees(cache: &Cache, events: &EventSender<Event>) {
    match cache.load_worktrees() {
        Ok(worktrees) => {
            let _ = events.send(Event::Worktrees(worktrees));
        }
        Err(e) => tracing::warn!(error = %e, "could not read worktrees"),
    }
}

/// Make the table and the disk agree.
///
/// A row whose directory is gone becomes archived: someone deleted it by
/// hand, which is the same as archiving. A directory no row knows about is
/// removed: nothing can open it, and it is how the older commit-keyed
/// layout gets retired.
async fn reconcile_worktrees(ctx: &Ctx) {
    let Some(rows) = ctx.with_cache(|c| c.load_worktrees().ok()).flatten() else {
        return;
    };

    let mut changed = false;
    for row in &rows {
        if !row.is_archived() && !row.path.join(".git").exists() {
            tracing::info!(id = %row.id, path = %row.path.display(), "worktree directory is gone; archiving its row");
            ctx.with_cache(|c| c.set_worktree_archived(&row.id, Some(Timestamp::now())));
            changed = true;
        }
    }

    let known: std::collections::HashSet<std::path::PathBuf> =
        rows.iter().map(|r| r.path.clone()).collect();
    let mut removed = false;
    for dir in ctx.preparer.worktree_dirs() {
        if known.contains(&dir) {
            continue;
        }
        tracing::info!(path = %dir.display(), "removing a worktree directory no record claims");
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(path = %dir.display(), error = %e, "could not remove it");
        }
        removed = true;
    }
    if removed {
        prune_every_clone(ctx).await;
    }

    if changed {
        ctx.send_worktrees();
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

/// Archive whatever has sat unopened for too long.
async fn sweep_worktrees(ctx: &Ctx) {
    let stale = ctx.preparer.stale_worktrees(WORKTREE_MAX_AGE);
    if stale.is_empty() {
        return;
    }
    let rows = ctx.with_cache(|c| c.load_worktrees().ok()).flatten().unwrap_or_default();

    for path in stale {
        match rows.iter().find(|r| r.path == path) {
            Some(row) if !row.is_archived() => match archive_worktree(ctx, &row.id).await {
                Ok(()) => tracing::info!(id = %row.id, "archived a stale worktree"),
                // Dirty, most likely. It stays; the reader has work in it.
                Err(message) => tracing::info!(id = %row.id, %message, "left a stale worktree alone"),
            },
            Some(_) => {}
            None => {
                // Reconciliation runs first, so this is a directory that
                // appeared in between. Same treatment.
                tracing::info!(path = %path.display(), "removing a stale worktree directory no record claims");
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
}

/// Take a worktree off disk, keeping its branch and its row.
async fn archive_worktree(ctx: &Ctx, id: &str) -> Result<(), String> {
    let Some(row) = ctx.worktree(id) else {
        return Err(format!("no worktree called {id}"));
    };
    let Some(repo) = RepoId::parse(&row.repo_full_name) else {
        return Err(format!("{} is not a repository name", row.repo_full_name));
    };
    let bare = ctx.preparer.store().bare_path(&repo);

    if row.path.join(".git").exists() {
        let dirty = ctx.preparer.is_dirty(&row.path).await.map_err(|e| e.to_string())?;
        if dirty {
            return Err(format!("{} has uncommitted changes", row.branch));
        }
        if ctx.is_open(&row) {
            ctx.close_review().await;
        }
        ctx.preparer
            .remove_worktree(&bare, &row.path)
            .await
            .map_err(|e| e.to_string())?;
    }

    ctx.with_cache(|c| c.set_worktree_archived(id, Some(Timestamp::now())))
        .transpose()
        .map_err(|e| e.to_string())?;
    ctx.send_worktrees();
    Ok(())
}

/// Delete an archived worktree's branch and its row.
async fn forget_worktree(ctx: &Ctx, id: &str) -> Result<(), String> {
    let Some(row) = ctx.worktree(id) else {
        return Err(format!("no worktree called {id}"));
    };
    if !row.is_archived() {
        return Err(format!("{} is still checked out; archive it first", row.branch));
    }
    if let Some(repo) = RepoId::parse(&row.repo_full_name) {
        let bare = ctx.preparer.store().bare_path(&repo);
        if bare.join("HEAD").is_file() {
            // A branch that is already gone is not a reason to keep the row.
            if let Err(e) = ctx.preparer.delete_branch(&bare, &row.branch).await {
                tracing::info!(id, error = %e, "branch was not there to delete");
            }
        }
    }
    ctx.with_cache(|c| c.forget_worktree(id))
        .transpose()
        .map_err(|e| e.to_string())?;
    ctx.send_worktrees();
    Ok(())
}

/// Open a worktree from its row.
async fn open_worktree(ctx: &Ctx, id: String) -> bool {
    let Some(row) = ctx.worktree(&id) else {
        let message = format!("no worktree called {id}");
        let _ = ctx.events.send(Event::ReviewFailed { message });
        return false;
    };
    let Some(number) = row.pr_number else {
        let message = format!("{} has no pull request to open", row.branch);
        let _ = ctx.events.send(Event::ReviewFailed { message });
        return false;
    };
    open_pull_request(ctx, row.repo_full_name.clone(), number).await
}
/// A file larger than this opens as a notice rather than as text.
const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

/// What an open review needs in order to read and diff its files.
struct ReviewContext {
    checkout: perch_git::Checkout,
    files: Arc<Vec<perch_git::ChangedFile>>,
    worktree_id: String,
    pull_request: Arc<perch_github::PullRequest>,
    /// The description, once the discussion has been fetched.
    body: Option<String>,
}

/// One poll of the notifications API. Returns how long to wait before the next.
async fn refresh(
    github: &Arc<GitHub>,
    cache: &SharedCache,
    events: &EventSender<Event>,
    config: &Config,
    source: &str,
    fetches: &Fetches,
) -> Duration {
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

/// Resolve a pull request, lay down a checkout on its branch, and report
/// its files. Returns whether a review is now open.
///
/// The worktree row is found or made on the way: this is the one place a
/// worktree comes into being, and every open refreshes what the row knows.
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

    let id = Worktree::for_pull_request(&repo_full_name, number);
    let existing = ctx.worktree(&id);

    let (pull_request, offline) = match ctx.github.fetch_pull_request(&repo_full_name, number).await {
        Ok(pr) => (pr, false),
        Err(e) => {
            // No network, or no GitHub. A worktree that is already on disk
            // opens anyway, as it was last left, from the snapshot.
            match existing.as_ref().and_then(|w| w.pull_request.clone()) {
                Some(snapshot) if existing.as_ref().is_some_and(|w| w.path.join(".git").exists()) => {
                    tracing::info!(repo = %repo_full_name, number, error = %e, "opening the worktree offline");
                    (snapshot, true)
                }
                _ => fail!("{e}"),
            }
        }
    };

    let Some(repo) = RepoId::parse(&pull_request.repo_full_name) else {
        fail!("{} is not a repository name", pull_request.repo_full_name)
    };
    let branch = Worktree::pr_branch(number);

    let checkout = if offline {
        reopen_from_disk(ctx, &repo, number, &branch, existing.as_ref().map(|w| w.path.clone())).await
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
    let row = Worktree {
        id: id.clone(),
        repo_full_name: pull_request.repo_full_name.clone(),
        branch: branch.clone(),
        base_ref: pull_request.base_ref.clone(),
        pr_number: Some(number),
        head_sha: checkout.head_sha.clone(),
        path: checkout.worktree.clone(),
        created_at: existing.as_ref().map(|w| w.created_at).unwrap_or(now),
        last_opened_at: now,
        archived_at: None,
        pull_request: Some(pull_request.clone()),
    };
    if let Some(Err(e)) = ctx.with_cache(|c| c.upsert_worktree(&row)) {
        tracing::warn!(id, error = %e, "could not record the worktree");
    }
    ctx.send_worktrees();

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
    if let Ok(mut guard) = ctx.slot.lock() {
        *guard = Some(ReviewContext {
            checkout,
            files: files.clone(),
            worktree_id: id.clone(),
            pull_request: pull_request.clone(),
            body: None,
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
        pull_request: pull_request.clone(),
        worktree: Arc::new(row),
        files,
        tree: Arc::new(tree),
        stale_head,
        offline,
    });

    // Threads paint after the code, not before, and the outdated check is
    // a git call per moved thread, so both go on their own task.
    {
        let ctx = ctx.clone();
        let worktree_id = id.clone();
        tokio::spawn(async move {
            mark_outdated_threads(&ctx, &worktree_id, &head_for_threads, &worktree_for_threads).await;
            ctx.send_threads(&worktree_id);
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
        if let Some(open) = guard.as_mut().filter(|c| c.pull_request.number == pr.number) {
            open.body = Some(discussion.body.clone());
        }
    }

    let project = ctx.config.project_for(&pr.repo_full_name);
    let messages = crate::knowledge::to_messages(pr, &discussion, &project);
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
        Ok(Ok(ingested)) => {
            tracing::info!(repo = %pr.repo_full_name, number = pr.number, added = ingested.added, changed = ingested.changed, "discussion archived");
            let _ = ctx.events.send(Event::Archived {
                repo_full_name: pr.repo_full_name.clone(),
                number: pr.number,
                added: ingested.total(),
            });
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "could not archive the discussion");
            let _ = ctx.events.send(Event::Notice(format!("Could not archive the discussion: {e}")));
        }
        Err(e) => tracing::warn!(error = %e, "archive task panicked"),
    }
}

/// A thread whose file changed since its anchor was made may have moved.
///
/// `git diff --quiet <sha> <head> -- <path>` per thread whose sha differs
/// from the head; the common case, same sha, costs nothing.
async fn mark_outdated_threads(ctx: &Ctx, worktree_id: &str, head: &str, worktree: &Path) {
    let Some(Ok(threads)) = ctx.with_cache(|c| c.load_comment_threads(worktree_id)) else {
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
async fn ask(
    ctx: &Ctx,
    worktree_id: String,
    thread_id: Option<String>,
    anchor: Option<Anchor>,
    question: String,
    model: String,
    effort: perch_agent::Effort,
) {
    let open = ctx.slot.lock().ok().and_then(|g| {
        g.as_ref().filter(|c| c.worktree_id == worktree_id).map(|c| {
            (c.checkout.clone(), c.pull_request.clone(), c.body.clone())
        })
    });
    let Some((checkout, pull_request, body)) = open else {
        let _ = ctx.events.send(Event::Notice("That worktree is no longer open".into()));
        return;
    };
    let now = Timestamp::now();

    // The thread: found, or made with the question as its first message.
    let existing = match &thread_id {
        Some(id) => ctx
            .with_cache(|c| c.load_comment_threads(&worktree_id))
            .and_then(Result::ok)
            .and_then(|all| all.into_iter().find(|t| &t.id == id)),
        None => None,
    };
    let thread = match existing {
        Some(t) => t,
        None => CommentThread {
            id: perch_github::comments::new_id(),
            worktree_id: worktree_id.clone(),
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
        backend: perch_agent::Backend::ClaudeCode.name().into(),
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
    ctx.send_threads(&worktree_id);

    // The prompt. A follow-up resumes the last finished run; a thread with
    // none starts over with its history folded in.
    let file = match &thread.anchor {
        Some(a) => crate::ask::load_display(&ctx.preparer, &checkout, &a.path).await.ok(),
        None => None,
    };
    let history: Vec<(Author, String)> =
        thread.messages.iter().map(|m| (m.author.clone(), m.body.clone())).collect();
    let resume = thread.resumable_session();
    let input = crate::ask::PromptInput {
        pull_request: Some(&pull_request),
        body: body.as_deref(),
        anchor: thread.anchor.as_ref(),
        file: file.as_ref(),
        outdated: thread.outdated,
        question: &question,
        history: if is_new || resume.is_some() { &[] } else { &history },
    };
    let prompt = if is_new { crate::ask::first_prompt(&input) } else { crate::ask::follow_up_prompt(&input) };
    let spec = crate::ask::run_spec(&ctx.config.agent, &model, effort, checkout.worktree.clone(), &prompt, resume);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut run = run;
    let handle = match perch_agent::Run::spawn(spec, tx) {
        Ok(h) => h,
        Err(e) => {
            let message = format!("{e}. Set agent.binary in config.toml to where Claude Code is installed.");
            crate::ask::apply_run_event(&mut run, &perch_agent::RunEvent::Failed { message });
            ctx.with_cache(|c| c.update_run(&run));
            ctx.send_threads(&worktree_id);
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
    ctx.send_threads(&worktree_id);
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
    let worktree_id = ctx.slot.lock().ok().and_then(|g| g.as_ref().map(|c| c.worktree_id.clone()));
    if let Some(worktree_id) = worktree_id {
        ctx.send_threads(&worktree_id);
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

fn open_cache() -> SharedCache {
    let path = perch_github::config::db_path()?;
    let avatars = perch_github::config::avatar_dir();
    match Cache::open(&path, avatars) {
        Ok(c) => Some(Arc::new(std::sync::Mutex::new(c))),
        Err(e) => {
            // A broken cache must not stop the app; it only costs a cold start.
            tracing::warn!(error = %e, "running without a local cache");
            None
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
