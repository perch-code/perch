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
use perch_github::{resolve_token, Cache, Config, GitHub, SubjectDetail, Thread};
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
    CloseReview,
}

#[derive(Debug, Clone)]
pub enum Event {
    /// A whole inbox, from the cache on startup or from the network after.
    Threads { threads: Vec<Thread>, from_cache: bool },
    Detail { thread_id: String, detail: SubjectDetail },
    Avatar { url: String, bytes: Vec<u8> },
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

    // Paint from the cache first. A cold launch with no network still shows
    // the inbox as it was.
    if let Some(cache) = cache.as_ref() {
        if let Ok(guard) = cache.lock() {
            if let Ok(threads) = guard.load_threads() {
                if !threads.is_empty() {
                    let _ = events.send(Event::Threads { threads, from_cache: true });
                }
            }
            if let Ok(details) = guard.load_all_details() {
                for (thread_id, detail) in details {
                    let _ = events.send(Event::Detail { thread_id, detail });
                }
            }
            // Touch the guard so clippy sees the mutable borrow is intended.
            let _ = guard.meta(LAST_MODIFIED_KEY);
        }
    }

    let detail_limit = Arc::new(Semaphore::new(DETAIL_CONCURRENCY));
    let avatar_limit = Arc::new(Semaphore::new(AVATAR_CONCURRENCY));
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
                tokio::spawn(async move {
                    open_pull_request(github, preparer, checkout, events, repo_full_name, number)
                        .await;
                });
            }
            Some(Command::OpenFile { path }) => {
                let preparer = preparer.clone();
                let checkout = checkout.clone();
                let events = events.clone();
                tokio::spawn(async move {
                    open_file(preparer, checkout, events, path).await;
                });
            }
            Some(Command::CloseReview) => {
                if let Ok(mut guard) = checkout.lock() {
                    *guard = None;
                }
            }
        }
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

    if let Ok(mut guard) = slot.lock() {
        *guard = Some(ReviewContext { checkout, files: files.clone() });
    }

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
            let _ = events.send(Event::FileLoaded { path: file.path.clone(), file: Arc::new(file) });
        }
        Err(e) => tracing::warn!(error = %e, "file load panicked"),
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
        if let Some(handle) = cache.as_ref() {
            if let Ok(guard) = handle.lock() {
                if let Ok(Some(detail)) = guard.load_detail(&thread.id, &updated_at) {
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
            }
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
}
