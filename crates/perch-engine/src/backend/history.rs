//! Optional repository-wide GitHub history for the knowledge archive.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use jiff::Timestamp;
use perch_github::{Cache, Error, GitHub, HistoryStream, HistorySubject};
use serde::{Deserialize, Serialize};
use tokio::task::{JoinHandle, JoinSet};

use super::Ctx;

const SYNC_INTERVAL: Duration = Duration::from_secs(3600);
const RETRY_INTERVAL: Duration = Duration::from_secs(300);
const CONFIG_INTERVAL: Duration = Duration::from_secs(10);
const TRANSPORT_FAILURE_LIMIT: usize = 3;
// GitHub's timestamps have second precision. Revisit the boundary, also
// allowing a little time for changes to become visible in the listings.
const OVERLAP_SECONDS: i64 = 60;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Serialize, Deserialize)]
enum Checkpoint {
    Complete {
        watermark: Timestamp,
        deferred: Vec<HistorySubject>,
    },
    Pending(Batch),
}

#[derive(Serialize, Deserialize)]
struct Batch {
    started_at: Timestamp,
    backfill: bool,
    subjects: Vec<HistorySubject>,
}

#[derive(Serialize, Deserialize)]
struct Progress {
    started_at: Timestamp,
    next: usize,
    deferred: Vec<HistorySubject>,
}

pub(super) fn start(ctx: Ctx) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut workers = JoinSet::new();
        let mut running = HashMap::new();
        let mut clock = tokio::time::interval(CONFIG_INTERVAL);
        clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            clock.tick().await;
            while let Some(result) = workers.try_join_next() {
                if let Err(error) = result {
                    if !error.is_cancelled() {
                        tracing::warn!(%error, "GitHub history worker stopped");
                    }
                }
            }
            let enabled: HashSet<_> = ctx
                .config()
                .history_sync_repos()
                .map(str::to_ascii_lowercase)
                .collect();
            running.retain(|repo, task: &mut tokio::task::AbortHandle| {
                if !enabled.contains(repo) {
                    task.abort();
                    return false;
                }
                !task.is_finished()
            });
            for repo in enabled {
                running.entry(repo.clone()).or_insert_with(|| {
                    let ctx = ctx.clone();
                    workers.spawn(async move { run_repo(&ctx, &repo).await })
                });
            }
        }
    })
}

async fn run_repo(ctx: &Ctx, repo: &str) {
    let mut retry = RETRY_INTERVAL;
    loop {
        let Some(github) = ctx.github() else {
            tokio::time::sleep(CONFIG_INTERVAL).await;
            continue;
        };
        let delay = match sync(ctx, &github, repo).await {
            Ok(delay) => {
                retry = RETRY_INTERVAL;
                delay
            }
            Err(error) => {
                tracing::warn!(repo, %error, "could not sync GitHub history");
                let now = Timestamp::now().as_second();
                let quota_wait = github
                    .history_rate_limited_until()
                    .and_then(|epoch| u64::try_from(epoch - now).ok())
                    .map(Duration::from_secs);
                if let Some(wait) = quota_wait {
                    wait
                } else {
                    let delay = retry;
                    retry = (retry * 2).min(SYNC_INTERVAL);
                    delay
                }
            }
        };
        tokio::time::sleep(delay).await;
    }
}

fn until_next_sync(started_at: Timestamp, now: Timestamp) -> Duration {
    let elapsed = now.as_second().saturating_sub(started_at.as_second());
    SYNC_INTERVAL.saturating_sub(Duration::from_secs(elapsed.max(0) as u64))
}

async fn sync(ctx: &Ctx, github: &GitHub, repo: &str) -> Result<Duration> {
    let canonical_repo = github.history_repo_name(repo).await?;
    let checkpoint_key = format!("github.history.{repo}");
    let progress_key = format!("github.history.progress.{repo}");
    let (checkpoint, progress): (Option<Checkpoint>, Option<Progress>) =
        with_cache(ctx, |cache| {
            Ok((
                read_meta(cache, &checkpoint_key)?,
                read_meta(cache, &progress_key)?,
            ))
        })?;
    let resumed = matches!(checkpoint, Some(Checkpoint::Pending(_)));
    let batch = match checkpoint {
        Some(Checkpoint::Pending(batch)) => batch,
        Some(Checkpoint::Complete {
            watermark,
            deferred,
        }) => {
            discover_and_save(
                ctx,
                github,
                repo,
                &checkpoint_key,
                Some(watermark),
                deferred,
            )
            .await?
        }
        None => discover_and_save(ctx, github, repo, &checkpoint_key, None, Vec::new()).await?,
    };
    let mut progress = progress
        .filter(|progress| progress.started_at == batch.started_at)
        .unwrap_or(Progress {
            started_at: batch.started_at,
            next: 0,
            deferred: Vec::new(),
        });
    let mut transport_failures = 0;
    for (offset, subject) in batch
        .subjects
        .iter()
        .copied()
        .enumerate()
        .skip(progress.next)
    {
        let snapshot = github
            .fetch_history_subject(repo, &canonical_repo, subject)
            .await;
        if matches!(
            &snapshot,
            Err(Error::Network(_)
                | Error::Status {
                    status: 500..=599,
                    ..
                })
        ) {
            transport_failures += 1;
        } else {
            transport_failures = 0;
        }
        with_cache(ctx, |cache| {
            match snapshot {
                Ok(Some((subject, discussion))) => {
                    cache.store_history_subject(&subject, &discussion)?
                }
                Ok(None) => cache.delete_history_subject(&perch_github::history_subject_id(
                    repo,
                    subject.number,
                ))?,
                Err(error) if defer_subject(&error) => {
                    tracing::warn!(repo, number = subject.number, %error, "will retry GitHub history subject in the next pass");
                    progress.deferred.push(subject);
                }
                Err(error) => return Err(error.into()),
            }
            // If interrupted between these two writes, retry this subject.
            progress.next = offset + 1;
            cache.set_meta(&progress_key, &serde_json::to_string(&progress)?)?;
            Ok(())
        })?;
        if transport_failures == TRANSPORT_FAILURE_LIMIT {
            return Err("GitHub history reads failed repeatedly; pausing this pass".into());
        }
    }
    let checkpoint = serde_json::to_string(&Checkpoint::Complete {
        watermark: batch.started_at,
        deferred: progress.deferred,
    })?;
    with_cache(ctx, |cache| {
        cache.set_meta(&checkpoint_key, &checkpoint)?;
        Ok(())
    })?;
    tracing::debug!(
        repo,
        backfill = batch.backfill,
        subjects = batch.subjects.len(),
        "synced GitHub history"
    );
    // Catch changes that landed during a long first import or while this
    // checkpoint was waiting for a restart, a retry or an API quota reset.
    let delay = if batch.backfill || resumed {
        Duration::ZERO
    } else {
        until_next_sync(batch.started_at, Timestamp::now())
    };
    Ok(delay)
}

fn defer_subject(error: &Error) -> bool {
    matches!(
        error,
        Error::HistorySubjectUnavailable
            | Error::Decode(_)
            | Error::Network(_)
            | Error::Status {
                status: 400 | 422 | 500..=599,
                ..
            }
    )
}

async fn discover_and_save(
    ctx: &Ctx,
    github: &GitHub,
    repo: &str,
    checkpoint_key: &str,
    previous: Option<Timestamp>,
    deferred: Vec<HistorySubject>,
) -> Result<Batch> {
    let started_at = Timestamp::now();
    let since = previous.map(|time| {
        Timestamp::from_second(time.as_second() - OVERLAP_SECONDS).unwrap_or(Timestamp::MIN)
    });
    let mut seen = HashSet::new();
    let mut subjects = Vec::new();
    for stream in [
        HistoryStream::Items,
        HistoryStream::IssueComments,
        HistoryStream::ReviewComments,
    ] {
        if previous.is_none() && stream != HistoryStream::Items {
            break;
        }
        let mut next_path = None;
        loop {
            let result = github
                .history_page(repo, stream, since, next_path.as_deref())
                .await?;
            subjects.extend(
                result
                    .subjects
                    .into_iter()
                    .filter(|subject| seen.insert(subject.number)),
            );
            let Some(next) = result.next_path else {
                break;
            };
            next_path = Some(next);
        }
    }
    subjects.extend(
        deferred
            .into_iter()
            .filter(|subject| seen.insert(subject.number)),
    );
    let checkpoint = Checkpoint::Pending(Batch {
        started_at,
        backfill: previous.is_none(),
        subjects,
    });
    let serialized = serde_json::to_string(&checkpoint)?;
    with_cache(ctx, |cache| {
        cache.set_meta(checkpoint_key, &serialized)?;
        Ok(())
    })?;
    let Checkpoint::Pending(batch) = checkpoint else {
        unreachable!()
    };
    Ok(batch)
}

fn with_cache<T>(ctx: &Ctx, run: impl FnOnce(&Cache) -> Result<T>) -> Result<T> {
    ctx.with_cache(run)
        .unwrap_or_else(|| Err("GitHub history cache is unavailable".into()))
}

fn read_meta<T: serde::de::DeserializeOwned>(cache: &Cache, key: &str) -> Result<Option<T>> {
    cache
        .meta(key)?
        .map(|text| {
            serde_json::from_str(&text).map_err(|error| format!("reading {key}: {error}").into())
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::backend::tests::{ctx, serve_by_method};

    #[tokio::test]
    async fn a_bad_subject_is_retried_without_blocking_the_rest_of_the_batch() {
        let first = Arc::new(AtomicU8::new(1));
        let server_first = first.clone();
        let (base, requests) = serve_by_method(move |_, path, _| {
            let first = server_first.load(Ordering::Relaxed) == 1;
            let (path, _) = path.split_once('?').unwrap_or((path, ""));
            let body = match path {
                "/repos/a/b" => r#"{"full_name":"a/b"}"#,
                "/repos/a/b/issues" if first => r#"[{"number":1},{"number":2}]"#,
                "/repos/a/b/issues/1" if first => "{}",
                "/repos/a/b/issues/1" => r#"{"number":1,"updated_at":"2025-01-01T00:00:00Z"}"#,
                "/repos/a/b/issues/2" => r#"{"number":2,"updated_at":"2025-01-01T00:00:00Z"}"#,
                _ => "[]",
            };
            ("200 OK", body.into())
        });
        let dir = tempfile::tempdir().unwrap();
        {
            let (engine, _) = ctx(dir.path(), &base, &["a/b"]);
            sync(&engine, &engine.github().unwrap(), "a/b")
                .await
                .unwrap();
            engine.with_cache(|cache| {
                assert_eq!(cache.load_history_ids().unwrap(), ["github-history:a/b:2"]);
            });
        }
        first.store(0, Ordering::Relaxed);
        requests.lock().unwrap().clear();
        let (engine, _) = ctx(dir.path(), &base, &["a/b"]);
        sync(&engine, &engine.github().unwrap(), "a/b")
            .await
            .unwrap();
        assert!(requests
            .lock()
            .unwrap()
            .iter()
            .any(|path| path == "/repos/a/b/issues/1"));
        engine.with_cache(|cache| {
            assert_eq!(cache.load_history_ids().unwrap().len(), 2);
        });
    }

    #[tokio::test]
    async fn resumes_backfill_then_refreshes_a_changed_discussion() {
        const REPO: &str = "a/b";
        let mode = Arc::new(AtomicU8::new(0));
        let server_mode = mode.clone();
        let (base, requests) = serve_by_method(move |_, path, _| {
            let mode = server_mode.load(Ordering::Relaxed);
            let (path, query) = path.split_once('?').unwrap_or((path, ""));
            let body = match path {
                "/repos/a/b" => r#"{"full_name":"a/b"}"#,
                "/repos/a/b/issues" if mode == 0 => r#"[{"number":1},{"number":2}]"#,
                "/repos/a/b/issues" if query.contains("since=") => "[]",
                "/repos/a/b/issues/comments" if query.contains("since=") => {
                    r#"[{"html_url":"https://github.com/a/b/issues/1#issuecomment-4"},{"html_url":"https://github.com/a/b/issues/1#issuecomment-5"}]"#
                }
                "/repos/a/b/issues/1" => {
                    r#"{"number":1,"title":"First issue","state":"closed","updated_at":"2025-01-01T00:00:00Z"}"#
                }
                "/repos/a/b/issues/2" if mode == 0 => return ("403 Forbidden", "{}".into()),
                "/repos/a/b/issues/2" => {
                    r#"{"number":2,"title":"Second issue","state":"open","updated_at":"2025-01-01T00:00:00Z"}"#
                }
                "/repos/a/b/issues/1/comments" if mode == 2 => {
                    r#"[{"id":4,"body":"A design decision","created_at":"2025-01-01T00:00:00Z"}]"#
                }
                "/repos/a/b/issues/1/comments" | "/repos/a/b/issues/2/comments" => "[]",
                "/repos/a/b/pulls/comments" if query.contains("since=") => "[]",
                _ => {
                    return (
                        "500 Internal Server Error",
                        format!("unexpected path {path}?{query}"),
                    )
                }
            };
            ("200 OK", body.into())
        });
        let dir = tempfile::tempdir().unwrap();
        {
            let (engine, _) = ctx(dir.path(), &base, &[REPO]);
            assert!(sync(&engine, &engine.github().unwrap(), REPO)
                .await
                .is_err());
            engine.with_cache(|cache| {
                assert_eq!(cache.load_history_ids().unwrap().len(), 1);
                assert!(cache.load_items().unwrap().is_empty());
            });
        }

        mode.store(1, Ordering::Relaxed);
        requests.lock().unwrap().clear();
        let (engine, _) = ctx(dir.path(), &base, &[REPO]);
        let github = engine.github().unwrap();
        assert_eq!(sync(&engine, &github, REPO).await.unwrap(), Duration::ZERO);
        let resumed_requests = std::mem::take(&mut *requests.lock().unwrap());
        assert_eq!(resumed_requests.len(), 3);
        assert_eq!(resumed_requests[0], "/repos/a/b");
        assert!(resumed_requests[1..]
            .iter()
            .all(|path| path.starts_with("/repos/a/b/issues/2")));

        mode.store(2, Ordering::Relaxed);
        let delay = sync(&engine, &github, REPO).await.unwrap();
        assert!(delay > SYNC_INTERVAL - Duration::from_secs(30));
        assert_eq!(
            requests
                .lock()
                .unwrap()
                .iter()
                .filter(|path| *path == "/repos/a/b/issues/1")
                .count(),
            1
        );
        let subject = engine
            .with_cache(|cache| {
                let id = perch_github::history_subject_id(REPO, 1);
                let (subject, discussion) = cache.load_history_subject(&id).unwrap().unwrap();
                assert_eq!(discussion.comments[0].body, "A design decision");
                assert_eq!(cache.load_history_ids().unwrap().len(), 2);
                assert!(cache.load_items().unwrap().is_empty());
                subject
            })
            .unwrap();

        let export = engine.knowledge.start(engine.item_lifecycle.clone());
        let repo_dir = dir.path().join("store/knowledge/perch/repos/a/b");
        let archived = repo_dir.join("github/issues/1");
        tokio::time::timeout(Duration::from_secs(5), async {
            while !repo_dir.join("INDEX.md").is_file() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let index = std::fs::read_to_string(repo_dir.join("INDEX.md")).unwrap();
        assert!(index.contains("github/issues/1"));
        assert!(index.contains("github/issues/2"));
        assert!(std::fs::read_to_string(archived.join("INDEX.md"))
            .unwrap()
            .contains("https://github.com/a/b/issues/1"));
        assert!(std::fs::read_to_string(archived.join("conversation.md"))
            .unwrap()
            .contains("A design decision"));
        let mut item = perch_github::Item::new(REPO, Timestamp::now());
        item.subject = Some(subject);
        assert_eq!(engine.knowledge_dir(&item).await, Some(repo_dir));
        export.abort();
    }
}
