//! Workflow runs, for the notifications that announce them.
//!
//! A `CheckSuite` notification carries a title and nothing else: no URL, no
//! run id, no check suite id. github.com still shows it as `Hourly DST #912`
//! with a link to the run, so the run is found the way a person would find
//! it: by the workflow's name, the branch, and when the notification came.

use jiff::{SignedDuration, Timestamp};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;

use crate::model::{SubjectDetail, SubjectState, Thread, UserPayload};

/// How far before the notification a run may have been created. A run is
/// created when it is queued and finishes some time later; a week covers
/// anything short of a runner that never picked it up.
const LOOKBACK: SignedDuration = SignedDuration::from_hours(24 * 7);

/// How far a run's last update may sit from the notification and still be
/// the run it announced. The observed lag is about twenty seconds. The rest
/// is slack for a slow notification pipeline, not room for a different run:
/// the same workflow rarely finishes twice on one branch within it.
const TOLERANCE: SignedDuration = SignedDuration::from_mins(15);

/// Everything needed to find the run behind a `CheckSuite` notification.
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

    /// The API path that lists the candidates: the branch's runs created in
    /// the week before the notification, newest first.
    ///
    /// Not narrowed by conclusion. A run re-run since the notification may
    /// have a different one now, and that is the run to show.
    pub(crate) fn path(&self) -> String {
        const STAMP: &str = "%Y-%m-%dT%H:%M:%SZ";
        let from = (self.at - LOOKBACK).strftime(STAMP);
        let to = self.at.strftime(STAMP);
        let branch = utf8_percent_encode(&self.branch, NON_ALPHANUMERIC);
        format!(
            "/repos/{}/actions/runs?branch={branch}&created={from}..{to}&per_page=100",
            self.repo_full_name
        )
    }

    /// The run this notification announced, out of the candidates.
    pub(crate) fn pick(&self, runs: Vec<WorkflowRunPayload>) -> Option<SubjectDetail> {
        let candidates: Vec<WorkflowRunPayload> = runs
            .into_iter()
            .filter(|r| {
                r.name == self.workflow
                    && r.head_branch.as_deref() == Some(self.branch.as_str())
                    && r.created_at <= self.at
            })
            .collect();

        // The run finished just before the notification went out, so its
        // last update sits within the tolerance. The closest one is it.
        let announced = candidates
            .iter()
            .map(|r| (r.updated_at.duration_since(self.at).abs(), r))
            .filter(|(gap, _)| *gap <= TOLERANCE)
            .min_by_key(|(gap, _)| *gap)
            .map(|(_, r)| r);

        // A run re-run since then was updated when the re-run finished, not
        // when the notification went out. Of the runs still being worked on
        // at the notification, the most recently started is the one it
        // announced.
        let rerun = || {
            let floor = self.at - TOLERANCE;
            candidates
                .iter()
                .filter(|r| r.updated_at >= floor)
                .max_by_key(|r| r.created_at)
        };

        announced
            .or_else(rerun)
            .cloned()
            .map(WorkflowRunPayload::into_detail)
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

/// `GET /repos/{owner}/{repo}/actions/runs`.
#[derive(Debug, Deserialize)]
pub(crate) struct WorkflowRunsPayload {
    #[serde(default)]
    pub workflow_runs: Vec<WorkflowRunPayload>,
}

/// One workflow run, reduced to what finding it and showing it need.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WorkflowRunPayload {
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
    fn into_detail(self) -> SubjectDetail {
        let state = match (self.status.as_str(), self.conclusion.as_deref()) {
            ("completed", Some("success")) => SubjectState::RunSuccess,
            ("completed", Some("failure" | "timed_out" | "startup_failure")) => {
                SubjectState::RunFailure
            }
            // Waiting on someone to approve it, which is closer to running
            // than to stopped.
            ("completed", Some("action_required")) => SubjectState::RunInProgress,
            // Cancelled, skipped, stale, neutral: nothing ran.
            ("completed", _) => SubjectState::RunCancelled,
            // Queued, in progress, waiting, pending, requested.
            _ => SubjectState::RunInProgress,
        };
        let (author_login, author_avatar_url) = self
            .actor
            .map(|u| (u.login, u.avatar_url))
            .unwrap_or_default();

        SubjectDetail {
            number: self.run_number,
            state,
            author_login,
            author_avatar_url,
            workflow: Some(self.name),
            html_url: Some(self.html_url),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> Timestamp {
        "2026-09-15T23:45:29Z".parse().unwrap()
    }

    fn query() -> WorkflowRunQuery {
        WorkflowRunQuery {
            repo_full_name: "slatedb/slatedb".into(),
            workflow: "Hourly DST".into(),
            branch: "main".into(),
            at: at(),
        }
    }

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
    fn the_path_covers_the_week_before_the_notification() {
        assert_eq!(
            query().path(),
            "/repos/slatedb/slatedb/actions/runs?branch=main\
             &created=2026-09-08T23:45:29Z..2026-09-15T23:45:29Z&per_page=100"
        );
    }

    #[test]
    fn branches_are_encoded_into_the_query() {
        let q = WorkflowRunQuery {
            branch: "feature/a+b".into(),
            ..query()
        };
        assert!(q.path().contains("branch=feature%2Fa%2Bb&"), "{}", q.path());
    }

    #[test]
    fn the_run_that_finished_just_before_the_notification_wins() {
        let picked = query()
            .pick(vec![
                run(
                    "Hourly DST",
                    912,
                    "2026-09-15T23:25:07Z",
                    "2026-09-15T23:45:08Z",
                    "failure",
                ),
                run(
                    "Hourly DST",
                    911,
                    "2026-09-15T20:01:11Z",
                    "2026-09-15T20:14:21Z",
                    "success",
                ),
                run(
                    "PR Checks",
                    8205,
                    "2026-09-15T23:40:00Z",
                    "2026-09-15T23:45:20Z",
                    "failure",
                ),
            ])
            .unwrap();
        assert_eq!(picked.number, 912);
        assert_eq!(picked.state, SubjectState::RunFailure);
        assert_eq!(picked.workflow.as_deref(), Some("Hourly DST"));
        assert_eq!(
            picked.html_url.as_deref(),
            Some("https://github.com/slatedb/slatedb/actions/runs/912")
        );
        assert_eq!(picked.author_login, "criccomini");
        assert_eq!(picked.reference(), "Hourly DST #912");
    }

    #[test]
    fn a_run_that_started_after_the_notification_is_not_it() {
        let picked = query().pick(vec![run(
            "Hourly DST",
            913,
            "2026-09-15T23:46:00Z",
            "2026-09-15T23:47:00Z",
            "failure",
        )]);
        assert_eq!(picked, None);
    }

    #[test]
    fn a_run_re_run_since_is_still_found_and_shows_its_new_state() {
        // Re-run an hour later and green now. Its updated_at moved past the
        // tolerance, but it was the run underway when the notification went
        // out.
        let picked = query()
            .pick(vec![
                run(
                    "Hourly DST",
                    912,
                    "2026-09-15T23:25:07Z",
                    "2026-09-16T00:50:00Z",
                    "success",
                ),
                run(
                    "Hourly DST",
                    911,
                    "2026-09-15T20:01:11Z",
                    "2026-09-15T20:14:21Z",
                    "failure",
                ),
            ])
            .unwrap();
        assert_eq!(picked.number, 912);
        assert_eq!(picked.state, SubjectState::RunSuccess);
    }

    #[test]
    fn an_older_run_is_not_mistaken_for_a_missing_one() {
        // The announced run is gone, and the previous one finished hours
        // earlier. Nothing fits, and nothing is better than the wrong number.
        let picked = query().pick(vec![run(
            "Hourly DST",
            911,
            "2026-09-15T20:01:11Z",
            "2026-09-15T20:14:21Z",
            "failure",
        )]);
        assert_eq!(picked, None);
    }

    #[test]
    fn another_branch_is_not_it_either() {
        let mut other = run(
            "Hourly DST",
            912,
            "2026-09-15T23:25:07Z",
            "2026-09-15T23:45:08Z",
            "failure",
        );
        other.head_branch = Some("release".into());
        assert_eq!(query().pick(vec![other]), None);
    }

    #[test]
    fn conclusions_map_to_states() {
        let state = |status: &str, conclusion: Option<&str>| {
            let mut r = run("w", 1, "2026-09-15T23:25:07Z", "2026-09-15T23:45:08Z", "");
            r.status = status.into();
            r.conclusion = conclusion.map(str::to_string);
            r.into_detail().state
        };
        assert_eq!(
            state("completed", Some("success")),
            SubjectState::RunSuccess
        );
        assert_eq!(
            state("completed", Some("failure")),
            SubjectState::RunFailure
        );
        assert_eq!(
            state("completed", Some("timed_out")),
            SubjectState::RunFailure
        );
        assert_eq!(
            state("completed", Some("cancelled")),
            SubjectState::RunCancelled
        );
        assert_eq!(
            state("completed", Some("skipped")),
            SubjectState::RunCancelled
        );
        assert_eq!(
            state("completed", Some("action_required")),
            SubjectState::RunInProgress
        );
        assert_eq!(state("in_progress", None), SubjectState::RunInProgress);
        assert_eq!(state("queued", None), SubjectState::RunInProgress);
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
        let picked = query().pick(payload.workflow_runs).unwrap();
        assert_eq!(picked.reference(), "Hourly DST #912");
        assert_eq!(
            picked.html_url.as_deref(),
            Some("https://github.com/slatedb/slatedb/actions/runs/35035566266")
        );
    }
}
