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
use perch_github::{
    resolve_token, Cache, Config, GitHub, SubjectDetail, Thread,
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

#[derive(Debug, Clone)]
pub enum Command {
    Refresh,
}

#[derive(Debug, Clone)]
pub enum Event {
    /// A whole inbox, from the cache on startup or from the network after.
    Threads { threads: Vec<Thread>, from_cache: bool },
    Detail { thread_id: String, detail: SubjectDetail },
    Avatar { url: String, bytes: Vec<u8> },
    Status(Status),
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

    // A refresh happens immediately, then on whatever cadence the server asks
    // for. A command from the UI short-circuits the wait.
    let mut poll_after = Duration::from_secs(0);

    loop {
        tokio::select! {
            _ = tokio::time::sleep(poll_after) => {}
            cmd = commands.recv() => {
                match cmd {
                    Some(Command::Refresh) => {}
                    // The UI is gone.
                    None => return,
                }
            }
        }

        tracing::info!("refreshing");
        let _ = events.send(Event::Status(Status::Refreshing));

        let since = cache
            .as_ref()
            .and_then(|c| c.lock().ok())
            .and_then(|c| c.meta(LAST_MODIFIED_KEY).ok().flatten());

        match github.list_threads(since.as_deref()).await {
            Ok(result) => {
                poll_after = result.poll_interval.max(Duration::from_secs(
                    config.poll_interval_secs.max(1),
                ));

                if let Some(cache) = cache.as_ref() {
                    if let (Some(lm), Ok(guard)) = (result.last_modified.as_ref(), cache.lock()) {
                        let _ = guard.set_meta(LAST_MODIFIED_KEY, lm);
                    }
                }

                if result.threads.is_none() {
                    tracing::info!("inbox unchanged (304), keeping the cached list");
                }

                if let Some(threads) = result.threads {
                    tracing::info!(count = threads.len(), "inbox refreshed");
                    if let Some(cache) = cache.as_ref() {
                        if let Ok(mut guard) = cache.lock() {
                            if let Err(e) = guard.store_threads(&threads) {
                                tracing::warn!(error = %e, "could not cache threads");
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
                    source: source.clone(),
                }));
            }
            Err(e) => {
                tracing::warn!(error = %e, "list_threads failed");
                let status = describe_error(&e);
                // Back off hard on a rate limit rather than hammering.
                if let perch_github::Error::RateLimited { reset_epoch } = &e {
                    poll_after = backoff_until(*reset_epoch);
                } else {
                    poll_after = Duration::from_secs(config.poll_interval_secs.max(30));
                }
                let _ = events.send(Event::Status(status));
            }
        }
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
