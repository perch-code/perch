//! Workflow runs: the checks pill on an item's header, and the run a CI
//! notification is about.
//!
//! A `CheckSuite` notification carries a title and nothing else: no URL, no
//! run id, no check suite id. The workflow's name and the branch are read
//! out of the title, which is enough to say whether the notification is
//! about the open pull request's branch and its checks want asking after,
//! and to find the run itself among the branch's runs.
//!
//! The pill's [`Checks`] are read from the pull request's status-check
//! rollup, in `pulls.rs`, not from these listings.

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::model::{Thread, UserPayload};

/// How long after a run finishes its notification may be sent. GitHub
/// sends it about twenty seconds after; this leaves room for a slow day,
/// and is short of the minutes between a busy workflow's runs, so a
/// listing without the run does not give the one before it instead.
const ANNOUNCEMENT_DELAY: SignedDuration = SignedDuration::from_mins(5);

/// What a `CheckSuite` notification says about the run it announces.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowRunQuery {
    pub repo_full_name: String,
    /// The workflow's display name: `name:` in its YAML.
    pub workflow: String,
    pub branch: String,
    /// The notification's `updated_at`, which for a run is when it was sent.
    pub at: Timestamp,
}

impl WorkflowRunQuery {
    /// Read the workflow and branch out of a notification title.
    ///
    /// GitHub writes them as `<workflow> workflow run failed for <branch>
    /// branch`, with `succeeded` or `cancelled` in place of `failed`, and
    /// `, Attempt #2` after `run` when it was re-run. A title in any other
    /// shape names nothing we can look up.
    pub fn from_thread(thread: &Thread) -> Option<Self> {
        let (workflow, _, branch) = parse_title(&thread.subject.title)?;
        Some(Self {
            repo_full_name: thread.repository.full_name.clone(),
            workflow,
            branch,
            at: thread.updated_at,
        })
    }

    /// The days of runs to ask GitHub for, as `created` on the runs
    /// listing takes them: from a week before the notification to the day
    /// it was sent, since the run started before it was sent, and a run
    /// that took over a week is not one anyone is waiting on.
    pub fn created(&self) -> String {
        let from = self
            .at
            .checked_sub(SignedDuration::from_hours(7 * 24))
            .unwrap_or(self.at);
        format!(
            "{}..{}",
            from.strftime("%Y-%m-%d"),
            self.at.strftime("%Y-%m-%d")
        )
    }

    /// The run this notification is about, among the branch's runs: the
    /// workflow's, started by the time the notification was sent, and
    /// finished nearest to then.
    ///
    /// Nearest finished rather than latest started because a re-run keeps
    /// its `created_at` and moves its `updated_at` to the new attempt's
    /// end, which is when GitHub sends the "Attempt #2" notification; the
    /// hourly run that came in between is nowhere near it.
    ///
    /// A run that finished well before the notification was sent is not
    /// the one it announces, however near it is: `updated_at` only moves
    /// forward, so the announced run's is at or after its end. GitHub's
    /// filtered listing sometimes answers with a stale or partial set, and
    /// the nearest of that was once a run from two days before.
    ///
    /// A run may come more than once, from more than one listing.
    pub fn pick<'a>(
        &self,
        runs: impl IntoIterator<Item = &'a WorkflowRunPayload>,
    ) -> Option<&'a WorkflowRunPayload> {
        let earliest_end = self
            .at
            .checked_sub(ANNOUNCEMENT_DELAY)
            .unwrap_or(self.at);
        runs.into_iter()
            .filter(|run| {
                run.name == self.workflow
                    && run.head_branch.as_deref() == Some(self.branch.as_str())
                    && run.created_at <= self.at
                    && run.updated_at >= earliest_end
            })
            .min_by_key(|run| run.updated_at.duration_since(self.at).abs())
    }
}

/// The verdict a `CheckSuite` notification's title announces: the
/// `failed`, `succeeded` or `cancelled` GitHub writes before `for <branch>
/// branch`. `None` for a title in any other shape, or with a word there
/// this does not know.
pub fn announced_verdict(title: &str) -> Option<CheckRunState> {
    let (_, verdict, _) = parse_title(title)?;
    match verdict.as_str() {
        "failed" => Some(CheckRunState::Failure),
        "succeeded" => Some(CheckRunState::Success),
        "cancelled" => Some(CheckRunState::Cancelled),
        _ => None,
    }
}

/// `("Hourly DST", "failed", "main")` from `Hourly DST workflow run failed
/// for main branch`.
fn parse_title(title: &str) -> Option<(String, String, String)> {
    let (workflow, rest) = title.split_once(" workflow run")?;
    // `run` may be followed by ` failed` or by `, Attempt #2 failed`.
    if !rest.starts_with(' ') && !rest.starts_with(',') {
        return None;
    }
    let rest = rest.strip_suffix(" branch").unwrap_or(rest);
    let (verdict, branch) = rest.rsplit_once(" for ")?;
    let workflow = workflow.trim();
    let verdict = verdict.rsplit(' ').next().unwrap_or_default().trim();
    let branch = branch.trim();
    if workflow.is_empty() || branch.is_empty() {
        return None;
    }
    Some((workflow.to_string(), verdict.to_string(), branch.to_string()))
}

/// The checks pill: the head's status-check rollup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    Passing,
    /// `workflow` is the name of the run that failed, for the hover.
    Failing {
        workflow: String,
    },
    Running,
    /// No rollup, or a state this build does not know.
    Unknown,
}

/// The pill and where clicking it goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checks {
    pub state: CheckState,
    /// The failing check's page, else a pending one's, else the first
    /// with a page. `None` when no check has one.
    pub url: Option<String>,
    /// Each check or legacy commit status in GitHub's rollup. Older cached
    /// rows only held the aggregate fields above, so this must stay defaulted.
    #[serde(default)]
    pub runs: Vec<Check>,
}

/// One entry in GitHub's status-check rollup. A check run carries its
/// workflow and event when GitHub Actions made it; a legacy commit status
/// has neither and is distinguished by `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    #[serde(default)]
    pub workflow: Option<String>,
    #[serde(default)]
    pub event: Option<String>,
    pub state: CheckRunState,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub started_at: Option<Timestamp>,
    #[serde(default)]
    pub completed_at: Option<Timestamp>,
    pub kind: CheckKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    CheckRun,
    StatusContext,
}

/// A detailed check's state. The aggregate [`CheckState`] intentionally has
/// fewer values because the header pill only needs red, yellow, and green.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckRunState {
    Pending,
    /// A job a newer run of its workflow has not got to yet. GitHub has no
    /// check run for it; the row comes from an older run's.
    Waiting,
    Success,
    Failure,
    Neutral,
    Skipped,
    Cancelled,
    TimedOut,
    ActionRequired,
    StartupFailure,
    Stale,
    Unknown,
}

impl CheckRunState {
    pub fn is_pending(self) -> bool {
        matches!(self, Self::Pending | Self::Waiting)
    }

    pub fn is_passing(self) -> bool {
        matches!(self, Self::Success | Self::Neutral | Self::Skipped)
    }

    pub fn is_failing(self) -> bool {
        matches!(
            self,
            Self::Failure
                | Self::Cancelled
                | Self::TimedOut
                | Self::ActionRequired
                | Self::StartupFailure
                | Self::Stale
        )
    }
}

/// `GET /repos/{owner}/{repo}/actions/runs`.
#[derive(Debug, Deserialize)]
pub(crate) struct WorkflowRunsPayload {
    #[serde(default)]
    pub workflow_runs: Vec<WorkflowRunPayload>,
}

/// One workflow run, as GitHub lists it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRunPayload {
    #[serde(default)]
    pub name: String,
    pub run_number: u64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub head_branch: Option<String>,
    #[serde(default)]
    pub html_url: String,
    #[serde(with = "crate::model::ts")]
    pub created_at: Timestamp,
    #[serde(with = "crate::model::ts")]
    pub updated_at: Timestamp,
    #[serde(default)]
    pub actor: Option<UserPayload>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        name: &str,
        number: u64,
        created: &str,
        updated: &str,
        conclusion: &str,
    ) -> WorkflowRunPayload {
        WorkflowRunPayload {
            name: name.into(),
            run_number: number,
            status: "completed".into(),
            conclusion: Some(conclusion.into()),
            head_branch: Some("main".into()),
            html_url: format!("https://github.com/slatedb/slatedb/actions/runs/{number}"),
            created_at: created.parse().unwrap(),
            updated_at: updated.parse().unwrap(),
            actor: Some(UserPayload {
                login: "criccomini".into(),
                avatar_url: "https://avatars/1".into(),
                kind: String::new(),
            }),
        }
    }

    #[test]
    fn titles_name_the_workflow_the_verdict_and_the_branch() {
        assert_eq!(
            parse_title("Hourly DST workflow run failed for main branch"),
            Some(("Hourly DST".into(), "failed".into(), "main".into()))
        );
        assert_eq!(
            parse_title("PR Checks workflow run succeeded for feature/foo-bar branch"),
            Some(("PR Checks".into(), "succeeded".into(), "feature/foo-bar".into()))
        );
        assert_eq!(
            parse_title("CI workflow run, Attempt #2 failed for main branch"),
            Some(("CI".into(), "failed".into(), "main".into()))
        );
        // A workflow with " for " in its name is read from the right.
        assert_eq!(
            parse_title("Build for release workflow run failed for main branch"),
            Some(("Build for release".into(), "failed".into(), "main".into()))
        );
    }

    #[test]
    fn the_verdict_is_the_word_github_wrote_and_nothing_is_guessed() {
        assert_eq!(
            announced_verdict("CI workflow run failed for main branch"),
            Some(CheckRunState::Failure)
        );
        assert_eq!(
            announced_verdict("CI workflow run, Attempt #2 failed for main branch"),
            Some(CheckRunState::Failure)
        );
        assert_eq!(
            announced_verdict("CI workflow run succeeded for main branch"),
            Some(CheckRunState::Success)
        );
        assert_eq!(
            announced_verdict("CI workflow run cancelled for main branch"),
            Some(CheckRunState::Cancelled)
        );
        // "failed" in the workflow's own name says nothing of this run.
        assert_eq!(
            announced_verdict("Retry failed jobs workflow run succeeded for main branch"),
            Some(CheckRunState::Success)
        );
        assert_eq!(
            announced_verdict("CI workflow run timed out for main branch"),
            None
        );
        assert_eq!(announced_verdict("Non-repeatable reads within DbSnapshot"), None);
    }

    #[test]
    fn other_titles_are_not_runs() {
        assert_eq!(parse_title("Non-repeatable reads within DbSnapshot"), None);
        assert_eq!(parse_title(" workflow run failed for main branch"), None);
        assert_eq!(
            parse_title("CI workflow runner failed for main branch"),
            None
        );
        assert_eq!(parse_title("CI workflow run failed"), None);
    }

    fn query(workflow: &str, branch: &str, at: &str) -> WorkflowRunQuery {
        WorkflowRunQuery {
            repo_full_name: "slatedb/slatedb".into(),
            workflow: workflow.into(),
            branch: branch.into(),
            at: at.parse().unwrap(),
        }
    }

    #[test]
    fn the_run_picked_is_the_workflows_on_the_branch_that_finished_when_the_notification_came() {
        let runs = [
            run(
                "Hourly DST",
                911,
                "2026-09-15T20:01:11Z",
                "2026-09-15T20:14:21Z",
                "success",
            ),
            run(
                "Hourly DST",
                912,
                "2026-09-15T23:25:07Z",
                "2026-09-15T23:45:08Z",
                "failure",
            ),
            run(
                "PR Checks",
                913,
                "2026-09-15T23:26:00Z",
                "2026-09-15T23:44:00Z",
                "failure",
            ),
            run(
                "Hourly DST",
                914,
                "2026-09-16T00:01:00Z",
                "2026-09-16T00:14:00Z",
                "failure",
            ),
        ];
        let picked = query("Hourly DST", "main", "2026-09-15T23:45:10Z").pick(&runs);
        assert_eq!(picked.map(|r| r.run_number), Some(912));

        // Another workflow's run at the same moment is another workflow's.
        let picked = query("PR Checks", "main", "2026-09-15T23:44:02Z").pick(&runs);
        assert_eq!(picked.map(|r| r.run_number), Some(913));

        // Another branch's runs are not this branch's, and a run started
        // after the notification was sent cannot be what it announced.
        assert!(query("Hourly DST", "release", "2026-09-15T23:45:10Z")
            .pick(&runs)
            .is_none());
        assert!(query("Hourly DST", "main", "2026-09-15T20:00:00Z")
            .pick(&runs)
            .is_none());
    }

    #[test]
    fn a_re_run_is_picked_by_when_it_finished_not_when_it_started() {
        // Run 10 failed, run 11 ran an hour later, then 10 was re-run and
        // finished after both. Each notification finds its own run.
        let runs = [
            run(
                "Hourly DST",
                10,
                "2026-09-15T10:00:00Z",
                "2026-09-15T11:45:00Z",
                "failure",
            ),
            run(
                "Hourly DST",
                11,
                "2026-09-15T11:00:00Z",
                "2026-09-15T11:20:00Z",
                "failure",
            ),
        ];
        let attempt_two = query("Hourly DST", "main", "2026-09-15T11:45:02Z").pick(&runs);
        assert_eq!(attempt_two.map(|r| r.run_number), Some(10));
        let eleven = query("Hourly DST", "main", "2026-09-15T11:20:01Z").pick(&runs);
        assert_eq!(eleven.map(|r| r.run_number), Some(11));
        // The first attempt's notification still finds its run, now
        // finished later, since nothing else of its workflow was nearer.
        let attempt_one = query("Hourly DST", "main", "2026-09-15T10:20:00Z").pick(&runs);
        assert_eq!(attempt_one.map(|r| r.run_number), Some(10));
    }

    #[test]
    fn a_run_that_ended_long_before_the_notification_is_not_the_one_it_announces() {
        // The listing came back without the night's run, and the nearest
        // left was two nights before.
        let runs = [run(
            "Nightly Benchmarks",
            781,
            "2026-09-27T13:29:46Z",
            "2026-09-27T13:46:20Z",
            "success",
        )];
        let q = query("Nightly Benchmarks", "main", "2026-09-29T14:52:19Z");
        assert!(q.pick(&runs).is_none());

        let tonight = run(
            "Nightly Benchmarks",
            783,
            "2026-09-29T14:33:56Z",
            "2026-09-29T14:51:57Z",
            "failure",
        );
        let runs = [runs[0].clone(), tonight];
        assert_eq!(q.pick(&runs).map(|r| r.run_number), Some(783));
    }

    #[test]
    fn the_days_asked_for_run_from_a_week_before_to_the_day_it_was_sent() {
        assert_eq!(
            query("CI", "main", "2026-09-15T23:45:10Z").created(),
            "2026-09-08..2026-09-15"
        );
        // Days are GitHub's, in UTC, not the reader's.
        assert_eq!(
            query("CI", "main", "2026-10-01T00:00:00Z").created(),
            "2026-09-24..2026-10-01"
        );
    }

    #[test]
    fn the_runs_envelope_deserializes() {
        let json = r#"{"total_count":1,"workflow_runs":[{
            "id":35035566266,"name":"Hourly DST","run_number":912,"run_attempt":1,
            "status":"completed","conclusion":"failure","event":"schedule",
            "head_branch":"main",
            "html_url":"https://github.com/slatedb/slatedb/actions/runs/35035566266",
            "created_at":"2026-09-15T23:25:07Z","updated_at":"2026-09-15T23:45:08Z",
            "actor":{"login":"criccomini","avatar_url":"https://avatars/1"}
        }]}"#;
        let payload: WorkflowRunsPayload = serde_json::from_str(json).unwrap();
        let [run] = payload.workflow_runs.as_slice() else {
            panic!("one run")
        };
        assert_eq!(run.name, "Hourly DST");
        assert_eq!(run.run_number, 912);
        assert_eq!(run.head_branch.as_deref(), Some("main"));
        assert_eq!(
            run.html_url,
            "https://github.com/slatedb/slatedb/actions/runs/35035566266"
        );
        assert_eq!(
            run.actor.as_ref().map(|a| a.login.as_str()),
            Some("criccomini")
        );
    }

    #[test]
    fn checks_round_trip_through_json() {
        let checks = Checks {
            state: CheckState::Failing {
                workflow: "Hourly DST".into(),
            },
            url: Some("https://github.com/slatedb/slatedb/actions/runs/912".into()),
            runs: Vec::new(),
        };
        let json = serde_json::to_string(&checks).unwrap();
        assert_eq!(serde_json::from_str::<Checks>(&json).unwrap(), checks);
        let plain = serde_json::to_string(&CheckState::Passing).unwrap();
        assert_eq!(
            serde_json::from_str::<CheckState>(&plain).unwrap(),
            CheckState::Passing
        );
    }

    #[test]
    fn checks_cached_before_detailed_runs_still_load() {
        let checks: Checks = serde_json::from_str(
            r#"{"state":"passing","url":"https://github.com/a/b/actions/runs/1"}"#,
        )
        .unwrap();
        assert_eq!(checks.state, CheckState::Passing);
        assert!(checks.runs.is_empty());
    }
}
