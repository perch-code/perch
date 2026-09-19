//! Workflow runs: the checks pill on an item's header, and the branch a
//! CI notification is about.
//!
//! A `CheckSuite` notification carries a title and nothing else: no URL, no
//! run id, no check suite id. The workflow's name and the branch are read
//! out of the title, which is enough to say whether the notification is
//! about the open pull request's branch and its checks want asking after.
//!
//! The pill reads the runs for one commit and shows the worst of them.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::model::{Thread, UserPayload};

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
        let (workflow, branch) = parse_title(&thread.subject.title)?;
        Some(Self {
            repo_full_name: thread.repository.full_name.clone(),
            workflow,
            branch,
            at: thread.updated_at,
        })
    }
}

/// `("Hourly DST", "main")` from `Hourly DST workflow run failed for main
/// branch`.
fn parse_title(title: &str) -> Option<(String, String)> {
    let (workflow, rest) = title.split_once(" workflow run")?;
    // `run` may be followed by ` failed` or by `, Attempt #2 failed`.
    if !rest.starts_with(' ') && !rest.starts_with(',') {
        return None;
    }
    let rest = rest.strip_suffix(" branch").unwrap_or(rest);
    let (_, branch) = rest.rsplit_once(" for ")?;
    let workflow = workflow.trim();
    let branch = branch.trim();
    if workflow.is_empty() || branch.is_empty() {
        return None;
    }
    Some((workflow.to_string(), branch.to_string()))
}

/// The checks pill: the worst of one commit's runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    Passing,
    /// `workflow` is the name of the run that failed, for the hover.
    Failing { workflow: String },
    Running,
    /// No runs, or none that finished with a verdict.
    Unknown,
}

/// The pill and where clicking it goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checks {
    pub state: CheckState,
    /// The failing run's page, else the newest run's. `None` with no runs.
    pub url: Option<String>,
}

impl Checks {
    /// The pill for these runs, and the run it opens: the one that failed
    /// when one did, since that is what the reader wants to see, else the
    /// newest.
    pub fn of(runs: &[WorkflowRunPayload]) -> Self {
        let run = failing(runs).or_else(|| runs.iter().max_by_key(|r| r.created_at));
        Checks {
            state: worst(runs),
            url: run.map(|r| r.html_url.clone()),
        }
    }
}

/// The state the pill shows for these runs.
///
/// Red beats yellow beats green: one failure among many passes is a
/// failure, and one run still going among passes is still going. Cancelled
/// and action-required count as failing here, unlike in a notification's
/// detail, because the pill has three colours and a run that stopped short
/// of green is not green.
pub fn worst(runs: &[WorkflowRunPayload]) -> CheckState {
    if let Some(run) = failing(runs) {
        return CheckState::Failing { workflow: run.name.clone() };
    }
    if runs.iter().any(WorkflowRunPayload::is_running) {
        return CheckState::Running;
    }
    if runs.iter().any(WorkflowRunPayload::is_passing) {
        return CheckState::Passing;
    }
    CheckState::Unknown
}

/// The first run that failed, which is the one the pill names and opens.
fn failing(runs: &[WorkflowRunPayload]) -> Option<&WorkflowRunPayload> {
    runs.iter().find(|r| r.is_failing())
}

/// `GET /repos/{owner}/{repo}/actions/runs`.
#[derive(Debug, Deserialize)]
pub(crate) struct WorkflowRunsPayload {
    #[serde(default)]
    pub workflow_runs: Vec<WorkflowRunPayload>,
}

/// One workflow run, as GitHub lists it and as the checks are kept.
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

impl WorkflowRunPayload {
    fn is_failing(&self) -> bool {
        matches!(
            self.conclusion.as_deref(),
            Some("failure" | "timed_out" | "cancelled" | "action_required" | "startup_failure")
        )
    }

    fn is_running(&self) -> bool {
        matches!(
            self.status.as_str(),
            "queued" | "in_progress" | "waiting" | "pending" | "requested"
        )
    }

    fn is_passing(&self) -> bool {
        self.status == "completed"
            && matches!(self.conclusion.as_deref(), Some("success" | "neutral" | "skipped"))
    }
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
            }),
        }
    }

    #[test]
    fn titles_name_the_workflow_and_the_branch() {
        assert_eq!(
            parse_title("Hourly DST workflow run failed for main branch"),
            Some(("Hourly DST".into(), "main".into()))
        );
        assert_eq!(
            parse_title("PR Checks workflow run succeeded for feature/foo-bar branch"),
            Some(("PR Checks".into(), "feature/foo-bar".into()))
        );
        assert_eq!(
            parse_title("CI workflow run, Attempt #2 failed for main branch"),
            Some(("CI".into(), "main".into()))
        );
        // A workflow with " for " in its name is read from the right.
        assert_eq!(
            parse_title("Build for release workflow run failed for main branch"),
            Some(("Build for release".into(), "main".into()))
        );
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
        let [run] = payload.workflow_runs.as_slice() else { panic!("one run") };
        assert_eq!(run.name, "Hourly DST");
        assert_eq!(run.run_number, 912);
        assert_eq!(run.head_branch.as_deref(), Some("main"));
        assert_eq!(run.html_url, "https://github.com/slatedb/slatedb/actions/runs/35035566266");
        assert_eq!(run.actor.as_ref().map(|a| a.login.as_str()), Some("criccomini"));
    }

    /// A run with the given status and conclusion, created `number`
    /// minutes into the hour so the newest is the highest number.
    fn run_in(name: &str, number: u64, status: &str, conclusion: Option<&str>) -> WorkflowRunPayload {
        let mut r = run(name, number, "2026-09-15T23:00:00Z", "2026-09-15T23:45:08Z", "");
        r.created_at = format!("2026-09-15T23:{number:02}:00Z").parse().unwrap();
        r.status = status.into();
        r.conclusion = conclusion.map(str::to_string);
        r
    }

    #[test]
    fn every_conclusion_and_status_has_a_pill_state() {
        let one = |status: &str, conclusion: Option<&str>| worst(&[run_in("CI", 1, status, conclusion)]);
        let failing = CheckState::Failing { workflow: "CI".into() };
        for c in ["failure", "timed_out", "cancelled", "action_required", "startup_failure"] {
            assert_eq!(one("completed", Some(c)), failing, "{c}");
        }
        for s in ["queued", "in_progress", "waiting", "pending", "requested"] {
            assert_eq!(one(s, None), CheckState::Running, "{s}");
        }
        for c in ["success", "neutral", "skipped"] {
            assert_eq!(one("completed", Some(c)), CheckState::Passing, "{c}");
        }
        // Stale, or a conclusion this build has never heard of, is neither
        // red nor green. Nor is a completed run with no conclusion at all.
        assert_eq!(one("completed", Some("stale")), CheckState::Unknown);
        assert_eq!(one("completed", None), CheckState::Unknown);
        assert_eq!(worst(&[]), CheckState::Unknown);
    }

    #[test]
    fn a_failure_beats_a_run_in_progress_beats_a_pass() {
        let pass = run_in("Build", 1, "completed", Some("success"));
        let going = run_in("Lint", 2, "in_progress", None);
        let fail = run_in("Test", 3, "completed", Some("failure"));
        let fail_too = run_in("DST", 4, "completed", Some("timed_out"));

        assert_eq!(worst(&[pass.clone(), going.clone()]), CheckState::Running);
        assert_eq!(worst(&[pass.clone(), pass.clone()]), CheckState::Passing);
        assert_eq!(
            worst(&[pass.clone(), going.clone(), fail.clone(), fail_too.clone()]),
            CheckState::Failing { workflow: "Test".into() },
            "the first failing run in the list is the one named"
        );
        // Unknown runs alongside real ones do not drag the pill down.
        let stale = run_in("Old", 5, "completed", Some("stale"));
        assert_eq!(worst(&[stale.clone(), pass.clone()]), CheckState::Passing);
        assert_eq!(worst(&[stale, going]), CheckState::Running);
    }

    #[test]
    fn the_pill_opens_the_failing_run_else_the_newest() {
        let older_pass = run_in("Build", 1, "completed", Some("success"));
        let newer_pass = run_in("Lint", 5, "completed", Some("success"));
        let fail = run_in("Test", 3, "completed", Some("failure"));

        let green = Checks::of(&[newer_pass.clone(), older_pass.clone()]);
        assert_eq!(green.state, CheckState::Passing);
        assert_eq!(green.url.as_deref(), Some(newer_pass.html_url.as_str()));

        let red = Checks::of(&[newer_pass, fail.clone(), older_pass]);
        assert_eq!(red.state, CheckState::Failing { workflow: "Test".into() });
        assert_eq!(red.url.as_deref(), Some(fail.html_url.as_str()), "not the newest: the one that failed");

        let none = Checks::of(&[]);
        assert_eq!(none, Checks { state: CheckState::Unknown, url: None });
    }

    #[test]
    fn checks_round_trip_through_json() {
        let checks = Checks {
            state: CheckState::Failing { workflow: "Hourly DST".into() },
            url: Some("https://github.com/slatedb/slatedb/actions/runs/912".into()),
        };
        let json = serde_json::to_string(&checks).unwrap();
        assert_eq!(serde_json::from_str::<Checks>(&json).unwrap(), checks);
        let plain = serde_json::to_string(&CheckState::Passing).unwrap();
        assert_eq!(serde_json::from_str::<CheckState>(&plain).unwrap(), CheckState::Passing);
    }
}
