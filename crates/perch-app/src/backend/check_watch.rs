//! Following an open pull request's checks until they stop moving.
//!
//! GitHub has nothing a desktop app can listen on for checks: webhooks want
//! a public endpoint, and a CI notification only comes for runs the reader
//! started, and by default only failed ones. So the head commit is probed.
//! Its check runs and statuses are asked for with the tags they were last
//! read with, which costs two 304s and none of the rate limit while they
//! stand still, and the rollup is fetched again only when one moved.
//!
//! Checks turn up after others finish: a workflow started by another's
//! `workflow_run`, a CI service that reports late. So nothing running is
//! not the end of it. Probing goes on until the checks have stood still
//! for `QUIET`, and a required check that has not reported holds the
//! rollup at pending for as long as it is missing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use perch_github::{CheckState, CheckTags, Checks, PullRequest};
use tokio::time::Instant;

use super::{publish_pull_request_overview, Ctx, Event};

/// How often the open items are looked over. Each has its own pace.
const TICK: Duration = Duration::from_secs(5);
/// The pace after a change, and to start with.
const FASTEST: Duration = Duration::from_secs(10);
/// The pace a commit that keeps still slows to.
const SLOWEST: Duration = Duration::from_secs(60);
/// How long settled checks must stand still before probing stops.
const QUIET: Duration = Duration::from_secs(3 * 60);

/// One open pull request's head, as followed so far.
struct Watch {
    head: String,
    /// `None` until the first probe.
    tags: Option<CheckTags>,
    /// The checks as last read from the cache.
    seen: Option<Checks>,
    /// When the checks were last seen to move. Starts at the open.
    moved: Timestamp,
    every: Duration,
    due: Instant,
}

impl Watch {
    fn new(head: &str) -> Self {
        Self {
            head: head.to_string(),
            tags: None,
            seen: None,
            moved: Timestamp::now(),
            every: FASTEST,
            due: Instant::now(),
        }
    }
}

/// Follow every window's open pull request for as long as Perch runs.
pub(super) fn spawn(ctx: &Ctx) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let mut watches = HashMap::new();
        let mut clock = tokio::time::interval(TICK);
        clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            clock.tick().await;
            tick(&ctx, &mut watches).await;
        }
    });
}

async fn tick(base: &Ctx, watches: &mut HashMap<String, Watch>) {
    let open: Vec<(Ctx, String, Arc<PullRequest>)> = base
        .views()
        .into_iter()
        .filter_map(|ctx| {
            let (item_id, pr) = ctx.slot.lock().ok().and_then(|slot| {
                slot.as_ref()
                    .and_then(|c| Some((c.item_id.clone(), c.pull_request.clone()?)))
            })?;
            Some((ctx, item_id, pr))
        })
        .collect();
    // Closing an item ends its watch, so opening it again starts afresh.
    watches.retain(|item_id, _| open.iter().any(|(_, id, _)| id == item_id));

    for (ctx, item_id, pr) in open {
        let watch = watches
            .entry(item_id.clone())
            .or_insert_with(|| Watch::new(&pr.head_sha));
        if watch.head != pr.head_sha {
            *watch = Watch::new(&pr.head_sha);
        }
        if Instant::now() < watch.due {
            continue;
        }
        // Nothing until the open's own read of the checks is in.
        let Some(checks) = ctx
            .with_cache(|c| c.load_checks(&item_id, &pr.head_sha).ok().flatten())
            .flatten()
        else {
            continue;
        };
        // Whoever read them, this watch or a CI notification.
        if watch.seen.as_ref() != Some(&checks) {
            if watch.seen.is_some() {
                watch.moved = Timestamp::now();
                watch.every = FASTEST;
            }
            watch.seen = Some(checks.clone());
        }
        if !may_move(&checks, watch.moved, Timestamp::now()) {
            continue;
        }
        let Some(github) = ctx.github() else {
            continue;
        };
        let asked = watch.tags.clone().unwrap_or_default();
        match github
            .probe_checks(&pr.repo_full_name, &pr.head_sha, &asked)
            .await
        {
            Ok(probe) if watch.tags.as_ref() == Some(&probe.tags) => {
                watch.every = (watch.every * 2).min(SLOWEST);
            }
            // The first probe counts as a change: the open's read may
            // have been before something moved, and then no tag ever would.
            // Unless the listings it read say what that read said, check
            // for check, which is the usual answer a moment after an open.
            // The tags are kept only once the rollup is read, so a read
            // that fails is tried again at the next probe.
            Ok(probe) => {
                watch.every = FASTEST;
                if probe.agrees_with(&checks) || read_again(&ctx, &item_id, &pr).await {
                    watch.tags = Some(probe.tags);
                }
            }
            Err(e) => {
                tracing::info!(item = item_id, error = %e, "could not probe the checks");
                watch.every = SLOWEST;
            }
        }
        watch.due = Instant::now() + watch.every;
    }
}

/// Whether checks may still change: one is pending, a required one has
/// not reported, or they moved within `QUIET`. A commit with no rollup at
/// all is given `QUIET` for its first check to turn up, so a repository
/// without CI is not probed for as long as it is open.
fn may_move(checks: &Checks, moved: Timestamp, now: Timestamp) -> bool {
    if checks.state == CheckState::Running || checks.runs.iter().any(|run| run.state.is_pending()) {
        return true;
    }
    // A check that finished shortly before the open is still recent.
    let last = checks
        .runs
        .iter()
        .filter_map(|run| run.completed_at)
        .fold(moved, Timestamp::max);
    now.duration_since(last).as_secs() < QUIET.as_secs() as i64
}

/// The rollup again, without the loading state an open shows: the panel
/// keeps what it has until the new answer replaces it, and a failure is
/// only logged. A head that has moved on is news the view offers to fetch.
/// False when GitHub could not be asked.
async fn read_again(ctx: &Ctx, item_id: &str, pr: &PullRequest) -> bool {
    let Some(github) = ctx.github() else {
        return false;
    };
    let status_generation = ctx.status_requests.begin(item_id);
    let result = github
        .fetch_pull_request_overview(&pr.repo_full_name, pr.number)
        .await;
    if !ctx.status_requests.is_current(item_id, status_generation) || !ctx.is_open(item_id) {
        return true;
    }
    match result {
        Ok(overview) if overview.head_sha == pr.head_sha => {
            publish_pull_request_overview(ctx, item_id, pr, status_generation, overview);
        }
        Ok(_) => {
            let _ = ctx.events.send(Event::OpenItemChanged {
                item_id: item_id.to_string(),
            });
        }
        Err(e) => {
            tracing::info!(item = item_id, error = %e, "could not read the checks again");
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_github::{Check, CheckKind, CheckRunState};

    fn run(state: CheckRunState, completed_at: Option<Timestamp>) -> Check {
        Check {
            name: "tests".into(),
            workflow: None,
            event: None,
            state,
            description: None,
            url: None,
            started_at: None,
            completed_at,
            kind: CheckKind::CheckRun,
        }
    }

    fn checks(state: CheckState, runs: Vec<Check>) -> Checks {
        Checks {
            state,
            url: None,
            runs,
        }
    }

    fn ago(now: Timestamp, secs: i64) -> Timestamp {
        now - jiff::SignedDuration::from_secs(secs)
    }

    #[test]
    fn running_checks_keep_the_watch_however_long_ago_they_moved() {
        let now = Timestamp::now();
        let running = checks(CheckState::Running, vec![run(CheckRunState::Pending, None)]);
        assert!(may_move(&running, ago(now, 3600), now));
    }

    #[test]
    fn a_required_check_not_yet_reported_keeps_the_watch() {
        // An EXPECTED rollup reads as running with nothing pending in it.
        let now = Timestamp::now();
        let expected = checks(
            CheckState::Running,
            vec![run(CheckRunState::Success, Some(ago(now, 3600)))],
        );
        assert!(may_move(&expected, ago(now, 3600), now));
    }

    #[test]
    fn no_rollup_is_waited_on_a_while_then_left() {
        let none = checks(CheckState::Unknown, vec![]);
        let now = Timestamp::now();
        assert!(may_move(&none, ago(now, 60), now));
        assert!(!may_move(&none, ago(now, 600), now));
    }

    #[test]
    fn settled_checks_are_watched_a_while_for_late_ones_then_left() {
        let now = Timestamp::now();
        let green = checks(
            CheckState::Passing,
            vec![run(CheckRunState::Success, Some(ago(now, 3600)))],
        );
        assert!(may_move(&green, ago(now, 60), now));
        assert!(!may_move(&green, ago(now, 600), now));
    }

    #[test]
    fn a_check_finished_just_before_the_open_counts_as_a_move() {
        let now = Timestamp::now();
        let red = checks(
            CheckState::Failing {
                workflow: "CI".into(),
            },
            vec![run(CheckRunState::Failure, Some(ago(now, 30)))],
        );
        assert!(may_move(&red, ago(now, 3600), now));
    }
}
