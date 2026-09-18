//! The background half of the app.
//!
//! Owns the tokio runtime, the GitHub client, and the SQLite cache. The UI
//! never touches any of them: it sends `Command`s and receives `Event`s.
//!
//! Commands travel on a tokio channel, which can be sent to from the UI
//! thread without a runtime context. Events travel back on a std channel,
//! because that is what floem's `update_signal_from_channel` consumes.

use std::sync::mpsc::Sender as EventSender;
use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use perch_git::{Preparer, RepoId, Store};
use perch_github::{resolve_token, Cache, Config, GitHub, Mark, SubjectDetail, Thread};
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
    Status(Status),

    /// A step of preparing a checkout.
    ReviewProgress { message: String, percent: Option<u8> },
    ReviewReady {
        pull_request: Arc<perch_github::PullRequest>,
        files: Arc<Vec<perch_git::ChangedFile>>,
        tree: Arc<Vec<perch_git::TreeEntry>>,
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

    // Marks run beside the loop rather than in it. A poll can hold the loop
    // for as long as the network takes, and a reader who presses Done should
    // not find out how long that is.
    spawn_mark_writer(marks, cache.clone(), events.clone());

    let detail_limit = Arc::new(Semaphore::new(DETAIL_CONCURRENCY));
    let avatar_limit = Arc::new(Semaphore::new(AVATAR_CONCURRENCY));

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
                detail_limit.clone(),
                avatar_limit.clone(),
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

    {
        // Reclaim disk from pull requests nobody has looked at in a fortnight.
        let preparer = preparer.clone();
        tokio::spawn(async move {
            let removed = preparer.sweep_worktrees(WORKTREE_MAX_AGE).await;
            if removed > 0 {
                tracing::info!(removed, "swept stale worktrees");
            }
        });
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
                    &config,
                    &source,
                    &detail_limit,
                    &avatar_limit,
                )
                .await;
            }
            Some(Command::OpenPullRequest { repo_full_name, number }) => {
                let github = github.clone();
                let preparer = preparer.clone();
                let checkout = checkout.clone();
                let events = events.clone();
                let language_server = language_server.clone();
                tokio::spawn(async move {
                    open_pull_request(
                        github,
                        preparer,
                        checkout.clone(),
                        events.clone(),
                        repo_full_name,
                        number,
                    )
                    .await;
                    start_language_server(checkout, language_server, events).await;
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

/// How long a worktree survives without being opened.
const WORKTREE_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);
/// A file larger than this opens as a notice rather than as text.
const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

/// What an open review needs in order to read and diff its files.
struct ReviewContext {
    checkout: perch_git::Checkout,
    files: Arc<Vec<perch_git::ChangedFile>>,
}

/// One poll of the notifications API. Returns how long to wait before the next.
async fn refresh(
    github: &Arc<GitHub>,
    cache: &SharedCache,
    events: &EventSender<Event>,
    config: &Config,
    source: &str,
    detail_limit: &Arc<Semaphore>,
    avatar_limit: &Arc<Semaphore>,
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
                    detail_limit.clone(),
                    avatar_limit.clone(),
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

/// Resolve a pull request, lay down a checkout, and report its files.
async fn open_pull_request(
    github: Arc<GitHub>,
    preparer: Arc<Preparer>,
    slot: Arc<std::sync::Mutex<Option<ReviewContext>>>,
    events: EventSender<Event>,
    repo_full_name: String,
    number: u64,
) {
    macro_rules! fail {
        ($($arg:tt)*) => {{
            let message = format!($($arg)*);
            tracing::warn!(repo = %repo_full_name, number, %message, "could not open pull request");
            let _ = events.send(Event::ReviewFailed { message });
            return;
        }};
    }

    let _ = events.send(Event::ReviewProgress {
        message: "Reading the pull request".into(),
        percent: None,
    });

    let pull_request = match github.fetch_pull_request(&repo_full_name, number).await {
        Ok(pr) => pr,
        Err(e) => fail!("{e}"),
    };

    let Some(id) = RepoId::parse(&pull_request.repo_full_name) else {
        fail!("{} is not a repository name", pull_request.repo_full_name)
    };

    let progress = events.clone();
    let label = id.full_name();
    let checkout = preparer
        .prepare(
            &id,
            number,
            &pull_request.head_sha,
            &pull_request.base_ref,
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
        .await;

    let checkout = match checkout {
        Ok(c) => c,
        Err(e) => fail!("{e}"),
    };

    let _ = events.send(Event::ReviewProgress {
        message: "Computing the diff".into(),
        percent: None,
    });

    let files = match changed_files(&preparer, &checkout).await {
        Ok(f) => Arc::new(f),
        Err(e) => fail!("{e}"),
    };

    let worktree = checkout.worktree.clone();
    let tree = tokio::task::spawn_blocking(move || {
        perch_git::list_tree(&worktree, 50_000).unwrap_or_default()
    })
    .await
    .unwrap_or_default();

    let worktree_label = checkout.worktree.display().to_string();
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(ReviewContext { checkout, files: files.clone() });
    }

    tracing::info!(
        repo = %pull_request.repo_full_name,
        number,
        files = files.len(),
        tree = tree.len(),
        worktree = %worktree_label,
        "review ready"
    );
    let _ = events.send(Event::ReviewReady {
        pull_request: Arc::new(pull_request),
        files,
        tree: Arc::new(tree),
    });
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
    detail_limit: Arc<Semaphore>,
    avatar_limit: Arc<Semaphore>,
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
                avatar_limit.clone(),
                t.repository.owner_avatar_url.clone(),
            );
        }
    }

    for thread in threads {
        if !thread.has_detail() {
            continue;
        }
        let Some(url) = thread.subject.url.clone() else {
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
                    avatar_limit.clone(),
                    avatar,
                );
            }
            continue;
        }

        let github = github.clone();
        let cache = cache.clone();
        let events = events.clone();
        let limit = detail_limit.clone();
        let avatar_limit = avatar_limit.clone();
        let kind = thread.subject.kind.clone();
        let thread_id = thread.id.clone();

        tokio::spawn(async move {
            let Ok(_permit) = limit.acquire().await else {
                return;
            };
            match github.fetch_detail(&url, &kind).await {
                Ok(detail) => {
                    if let Some(cache) = cache.as_ref() {
                        if let Ok(guard) = cache.lock() {
                            let _ = guard.store_detail(&thread_id, &updated_at, &detail);
                        }
                    }
                    let avatar = detail.author_avatar_url.clone();
                    tracing::debug!(thread_id, number = detail.number, "detail fetched");
                    let _ = events.send(Event::Detail { thread_id, detail });
                    if !avatar.is_empty() {
                        spawn_avatar(github, cache, events, avatar_limit, avatar);
                    }
                }
                Err(e) => tracing::debug!(error = %e, url, "detail fetch failed"),
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
