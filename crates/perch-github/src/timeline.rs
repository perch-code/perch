//! What was done to a pull request or an issue, as GitHub's timeline
//! lists it: commits, labels, assignments, review requests, renames,
//! references, and the changes of state.
//!
//! One endpoint answers for both, since GitHub numbers issues and pull
//! requests together. What Perch already draws as a card of its own — a
//! comment, a review, a line comment — is left out here, and so is what
//! GitHub's own timeline keeps to itself: `mentioned` and `subscribed`
//! are sent to everyone and shown to no one. A kind with no line of its
//! own is dropped rather than guessed at, so a vocabulary GitHub grows
//! costs nothing until Perch has words for it.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::items::{Label, LabelRef};
use crate::pulls::UserRef;

/// One line in the timeline: what was done, by whom, and when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullEvent {
    pub kind: PullEventKind,
    pub at: Timestamp,
    /// The account that did it. Empty where GitHub names none: a commit
    /// carries a git author rather than an account, and a row an older
    /// build kept has no actor at all.
    #[serde(default)]
    pub actor: String,
}

impl PullEvent {
    pub fn new(kind: PullEventKind, at: Timestamp, actor: impl Into<String>) -> Self {
        Self {
            kind,
            at,
            actor: actor.into(),
        }
    }
}

/// What was done. `Opened` has no event of its own on the timeline and is
/// made from the pull request's or issue's own payload; everything else
/// here is a line GitHub sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullEventKind {
    Opened,
    Merged,
    Closed,
    Reopened,
    Draft,
    Ready,
    /// A commit on the head branch. Its author is a name out of git, not
    /// an account, so it is here rather than in the event's actor, and
    /// only the subject of its message is kept: a line is all that is
    /// ever drawn of it. `login` is the account the commit belongs to,
    /// when GitHub could tie its email to one; empty when it could not,
    /// and on a row an older build kept.
    Committed {
        sha: String,
        author: String,
        #[serde(default)]
        login: String,
        subject: String,
    },
    ForcePushed { sha: String },
    HeadDeleted,
    HeadRestored,
    Labeled { label: Label },
    Unlabeled { label: Label },
    Assigned { who: String },
    Unassigned { who: String },
    ReviewRequested { who: String },
    ReviewRequestRemoved { who: String },
    /// A review someone threw out. `who` wrote it.
    ReviewDismissed { who: String },
    Renamed { from: String, to: String },
    Milestoned { title: String },
    Demilestoned { title: String },
    /// A commit elsewhere that named this one.
    Referenced { sha: String },
    /// Another issue or pull request that named this one.
    CrossReferenced { reference: Reference },
    Pinned,
    Unpinned,
    Locked,
    Unlocked,
    AutoMergeEnabled,
    AutoMergeDisabled,
    MergeQueued,
    MergeDequeued,
}

/// The issue or pull request on the other end of a cross-reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    /// `owner/name`, which may not be the repository being read: a fork's
    /// pull request references the upstream issue it closes.
    pub repo_full_name: String,
    pub number: u64,
    pub title: String,
    /// A pull request rather than an issue.
    pub pull: bool,
    pub html_url: String,
}

impl Reference {
    /// `owner/name#7`, as a reference is written.
    pub fn slug(&self) -> String {
        format!("{}#{}", self.repo_full_name, self.number)
    }
}

/// How many pages of timeline one item is read for. A hundred to a page,
/// and nobody reads the thousandth line of a pull request.
pub(crate) const TIMELINE_MAX_PAGES: usize = 10;

/// The wire shape: one struct for every kind GitHub sends, since the list
/// is heterogeneous and only the keys the kind carries are present. Each
/// element is read on its own, so one shape Perch does not expect costs
/// its own line and not the page.
#[derive(Debug, Deserialize)]
pub(crate) struct TimelineEventPayload {
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub actor: Option<UserRef>,
    /// Absent on `committed`, which is dated by its git author instead.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
    #[serde(default)]
    pub commit_id: Option<String>,
    #[serde(default)]
    pub sha: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub author: Option<GitActorRef>,
    #[serde(default)]
    pub label: Option<LabelRef>,
    #[serde(default)]
    pub assignee: Option<UserRef>,
    #[serde(default)]
    pub requested_reviewer: Option<UserRef>,
    #[serde(default)]
    pub requested_team: Option<TeamRef>,
    #[serde(default)]
    pub dismissed_review: Option<DismissedReviewRef>,
    #[serde(default)]
    pub milestone: Option<MilestoneRef>,
    #[serde(default)]
    pub rename: Option<RenameRef>,
    #[serde(default)]
    pub source: Option<SourceRef>,
}

/// A commit's author, as git records one: a name, an email, and the date
/// it was written.
#[derive(Debug, Deserialize)]
pub(crate) struct GitActorRef {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub date: Option<Timestamp>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TeamRef {
    #[serde(default)]
    pub name: String,
}

/// GitHub names the review that was thrown out by its id and not by who
/// wrote it; the caller has the reviews and looks the author up.
#[derive(Debug, Deserialize)]
pub(crate) struct DismissedReviewRef {
    #[serde(default)]
    pub review_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MilestoneRef {
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RenameRef {
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
}

/// A cross-reference's other end, which GitHub nests one level deeper.
#[derive(Debug, Deserialize)]
pub(crate) struct SourceRef {
    #[serde(default)]
    pub issue: Option<SourceIssueRef>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SourceIssueRef {
    #[serde(default)]
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub html_url: String,
    /// Present only when the reference is a pull request.
    #[serde(default)]
    pub pull_request: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    pub repository: Option<SourceRepositoryRef>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SourceRepositoryRef {
    #[serde(default)]
    pub full_name: String,
}

impl TimelineEventPayload {
    /// The line this event draws, or `None` when Perch has none for it:
    /// what is already a card, what GitHub hides, and a kind this build
    /// does not know. An event GitHub sent without the parts its own kind
    /// needs is dropped the same way. `reviewer` answers who wrote the
    /// review with a given id, which is all a dismissal names.
    pub(crate) fn into_event(self, reviewer: &dyn Fn(u64) -> Option<String>) -> Option<PullEvent> {
        let actor = self.actor.map(|u| u.login).unwrap_or_default();
        if self.event == "committed" {
            let author = self.author?;
            return Some(PullEvent {
                at: author.date?,
                actor,
                kind: PullEventKind::Committed {
                    sha: self.sha.unwrap_or_default(),
                    login: noreply_login(&author.email).unwrap_or_default(),
                    author: author.name,
                    subject: subject(self.message.as_deref().unwrap_or_default()),
                },
            });
        }
        let at = self.created_at?;
        let label = |label: Option<LabelRef>| {
            label.map(|l| Label {
                name: l.name,
                color: l.color.unwrap_or_default(),
            })
        };
        // A review is requested of a person or of a team; the team's name
        // stands in where there is no login.
        let requested = || {
            self.requested_reviewer
                .as_ref()
                .map(|u| u.login.clone())
                .or_else(|| self.requested_team.as_ref().map(|t| t.name.clone()))
                .filter(|who| !who.is_empty())
        };
        let kind = match self.event.as_str() {
            "merged" => PullEventKind::Merged,
            "closed" => PullEventKind::Closed,
            "reopened" => PullEventKind::Reopened,
            "convert_to_draft" => PullEventKind::Draft,
            "ready_for_review" => PullEventKind::Ready,
            "head_ref_force_pushed" => PullEventKind::ForcePushed {
                sha: named(self.commit_id.clone().unwrap_or_default())?,
            },
            "head_ref_deleted" => PullEventKind::HeadDeleted,
            "head_ref_restored" => PullEventKind::HeadRestored,
            "labeled" => PullEventKind::Labeled {
                label: label(self.label)?,
            },
            "unlabeled" => PullEventKind::Unlabeled {
                label: label(self.label)?,
            },
            "assigned" => PullEventKind::Assigned {
                who: named(self.assignee?.login)?,
            },
            "unassigned" => PullEventKind::Unassigned {
                who: named(self.assignee?.login)?,
            },
            "review_requested" => PullEventKind::ReviewRequested { who: requested()? },
            "review_request_removed" => PullEventKind::ReviewRequestRemoved { who: requested()? },
            "review_dismissed" => PullEventKind::ReviewDismissed {
                who: self
                    .dismissed_review
                    .and_then(|r| r.review_id)
                    .and_then(reviewer)
                    .unwrap_or_default(),
            },
            "renamed" => {
                let rename = self.rename?;
                PullEventKind::Renamed {
                    from: rename.from,
                    to: rename.to,
                }
            }
            "milestoned" => PullEventKind::Milestoned {
                title: named(self.milestone?.title)?,
            },
            "demilestoned" => PullEventKind::Demilestoned {
                title: named(self.milestone?.title)?,
            },
            "referenced" => PullEventKind::Referenced {
                sha: named(self.commit_id.clone().unwrap_or_default())?,
            },
            "cross-referenced" => PullEventKind::CrossReferenced {
                reference: reference(self.source?)?,
            },
            "pinned" => PullEventKind::Pinned,
            "unpinned" => PullEventKind::Unpinned,
            "locked" => PullEventKind::Locked,
            "unlocked" => PullEventKind::Unlocked,
            "auto_merge_enabled" => PullEventKind::AutoMergeEnabled,
            "auto_merge_disabled" => PullEventKind::AutoMergeDisabled,
            "added_to_merge_queue" => PullEventKind::MergeQueued,
            "removed_from_merge_queue" => PullEventKind::MergeDequeued,
            _ => return None,
        };
        Some(PullEvent { kind, at, actor })
    }
}

/// The events in the order they are read in, with the noise GitHub's own
/// timeline hides taken out.
///
/// A commit is dated by when it was written and not by when it was
/// pushed, so the list as GitHub sends it is not in time order. And a
/// merge closes the pull request, which GitHub sends as a second event at
/// the same instant; only the merge is a line.
pub(crate) fn tidy(mut events: Vec<PullEvent>) -> Vec<PullEvent> {
    events.sort_by_key(|e| e.at);
    let merges: Vec<Timestamp> = events
        .iter()
        .filter(|e| e.kind == PullEventKind::Merged)
        .map(|e| e.at)
        .collect();
    events.retain(|e| e.kind != PullEventKind::Closed || !merges.contains(&e.at));
    events
}

/// A commit as a pull request's commit list sends one: the account GitHub
/// tied its author's email to, which the timeline leaves out.
#[derive(Debug, Deserialize)]
pub(crate) struct PullCommitPayload {
    #[serde(default)]
    pub sha: String,
    #[serde(default)]
    pub author: Option<UserRef>,
}

/// Whether a commit line waits on its account: the timeline sent no email
/// that says whose it is.
pub(crate) fn wants_login(events: &[PullEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(&e.kind, PullEventKind::Committed { login, .. } if login.is_empty()))
}

/// Give each commit line that has no account the one `commits` names for
/// its sha.
pub(crate) fn fill_logins(events: &mut [PullEvent], commits: &[PullCommitPayload]) {
    for event in events {
        let PullEventKind::Committed { sha, login, .. } = &mut event.kind else {
            continue;
        };
        if !login.is_empty() {
            continue;
        }
        let found = commits
            .iter()
            .find(|c| c.sha == *sha)
            .and_then(|c| c.author.as_ref())
            .map(|a| a.login.clone());
        if let Some(found) = found {
            *login = found;
        }
    }
}

/// The name, unless GitHub sent none: a line that would say what was
/// done to nobody, or to no commit, is not a line.
fn named(name: String) -> Option<String> {
    Some(name).filter(|name| !name.is_empty())
}

/// A commit message's first non-empty line, which is all a timeline
/// draws of it.
fn subject(message: &str) -> String {
    message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// The account a GitHub noreply address stands for: `login@` or, since
/// 2017, `id+login@users.noreply.github.com`. The timeline names a
/// commit's author by git's name and email alone, and this is the one
/// email that says whose it is without asking.
fn noreply_login(email: &str) -> Option<String> {
    let (local, host) = email.rsplit_once('@')?;
    if !host.eq_ignore_ascii_case("users.noreply.github.com") {
        return None;
    }
    let login = local.split_once('+').map_or(local, |(_, login)| login);
    Some(login.to_string()).filter(|login| !login.is_empty())
}

/// The other end of a cross-reference. One whose repository GitHub did
/// not name is dropped: a reference no one can follow is not a line.
fn reference(source: SourceRef) -> Option<Reference> {
    let issue = source.issue?;
    let repo_full_name = issue.repository?.full_name;
    if repo_full_name.is_empty() || issue.number == 0 {
        return None;
    }
    Some(Reference {
        repo_full_name,
        number: issue.number,
        title: issue.title,
        pull: issue.pull_request.is_some(),
        html_url: issue.html_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    /// One event, with nothing to look a dismissed review's author up in.
    fn event(json: &str) -> Option<PullEvent> {
        read(json, &|_| None)
    }

    fn read(json: &str, reviewer: &dyn Fn(u64) -> Option<String>) -> Option<PullEvent> {
        serde_json::from_str::<TimelineEventPayload>(json)
            .unwrap()
            .into_event(reviewer)
    }

    #[test]
    fn a_label_keeps_its_colour_and_a_missing_one_is_the_default() {
        let e = event(
            r#"{"event":"labeled","actor":{"login":"rohan"},"created_at":"2026-09-15T08:00:00Z",
                "label":{"name":"bug","color":"d73a4a"}}"#,
        )
        .unwrap();
        assert_eq!(e.actor, "rohan");
        assert_eq!(e.at, at("2026-09-15T08:00:00Z"));
        let PullEventKind::Labeled { label } = e.kind else {
            panic!("{e:?}")
        };
        assert_eq!(
            (label.name.as_str(), label.color.as_str()),
            ("bug", "d73a4a")
        );

        let e = event(
            r#"{"event":"unlabeled","created_at":"2026-09-15T08:00:00Z","label":{"name":"bug","color":null}}"#,
        )
        .unwrap();
        assert_eq!(
            e.kind,
            PullEventKind::Unlabeled {
                label: Label {
                    name: "bug".into(),
                    color: String::new()
                }
            }
        );
        assert_eq!(e.actor, "", "a deleted account is not an error");
    }

    #[test]
    fn a_commit_is_dated_and_signed_by_git_rather_than_by_an_account() {
        let e = event(
            r#"{"event":"committed","sha":"abc1234","message":"Fence the manifest\n\nwhy",
                "author":{"name":"Chris","email":"c@example.com","date":"2026-09-15T09:00:00Z"}}"#,
        )
        .unwrap();
        assert_eq!(
            e.at,
            at("2026-09-15T09:00:00Z"),
            "the timeline sends no created_at"
        );
        assert_eq!(e.actor, "", "a commit has no actor");
        assert_eq!(
            e.kind,
            PullEventKind::Committed {
                sha: "abc1234".into(),
                author: "Chris".into(),
                login: String::new(),
                subject: "Fence the manifest".into(),
            },
            "the body of the message is not a line"
        );
        assert!(
            event(r#"{"event":"committed","sha":"abc1234","author":{"name":"Chris"}}"#).is_none(),
            "a commit with no date has no place in the order"
        );
    }

    #[test]
    fn a_noreply_email_says_whose_commit_it_is() {
        let login = |email: &str| {
            let json = format!(
                r#"{{"event":"committed","sha":"abc1234","message":"m",
                    "author":{{"name":"Chris","email":"{email}","date":"2026-09-15T09:00:00Z"}}}}"#
            );
            match event(&json).unwrap().kind {
                PullEventKind::Committed { login, .. } => login,
                kind => panic!("{kind:?}"),
            }
        };
        assert_eq!(login("criccomini@users.noreply.github.com"), "criccomini");
        assert_eq!(login("12345+CRiccomini@users.noreply.github.com"), "CRiccomini");
        assert_eq!(login("c@example.com"), "", "any other email names no account");
        assert_eq!(login("@users.noreply.github.com"), "");
    }

    #[test]
    fn a_review_is_requested_of_a_person_or_of_a_team() {
        let of = |json: &str| event(json).map(|e| e.kind);
        assert_eq!(
            of(
                r#"{"event":"review_requested","created_at":"2026-09-15T10:00:00Z","requested_reviewer":{"login":"rohan"}}"#
            ),
            Some(PullEventKind::ReviewRequested {
                who: "rohan".into()
            })
        );
        assert_eq!(
            of(
                r#"{"event":"review_requested","created_at":"2026-09-15T10:00:00Z","requested_team":{"name":"storage"}}"#
            ),
            Some(PullEventKind::ReviewRequested {
                who: "storage".into()
            })
        );
        assert_eq!(
            of(
                r#"{"event":"review_request_removed","created_at":"2026-09-15T10:00:00Z","requested_reviewer":{"login":"rohan"}}"#
            ),
            Some(PullEventKind::ReviewRequestRemoved {
                who: "rohan".into()
            })
        );
        assert_eq!(
            of(r#"{"event":"review_requested","created_at":"2026-09-15T10:00:00Z"}"#),
            None,
            "a request of nobody is not a line"
        );
    }

    #[test]
    fn a_dismissal_names_the_review_and_the_caller_names_its_author() {
        let json = r#"{"event":"review_dismissed","actor":{"login":"chris"},
            "created_at":"2026-09-16T12:00:00Z","dismissed_review":{"state":"changes_requested","review_id":9}}"#;
        let e = read(json, &|id| (id == 9).then(|| "rohan".to_string())).unwrap();
        assert_eq!(
            e.kind,
            PullEventKind::ReviewDismissed {
                who: "rohan".into()
            }
        );
        let e = event(json).unwrap();
        assert_eq!(
            e.kind,
            PullEventKind::ReviewDismissed { who: String::new() },
            "a review this build never read leaves the line without a name"
        );
    }

    #[test]
    fn a_cross_reference_carries_the_item_on_the_other_end() {
        let e = event(
            r#"{"event":"cross-referenced","actor":{"login":"mdwaud"},"created_at":"2026-09-18T12:59:22Z",
                "source":{"type":"issue","issue":{"number":2098,"title":"Use remote_seq","html_url":"https://github.com/slatedb/slatedb/pull/2098",
                "pull_request":{"url":"x"},"repository":{"full_name":"slatedb/slatedb"}}}}"#,
        )
        .unwrap();
        let PullEventKind::CrossReferenced { reference } = e.kind else {
            panic!("{e:?}")
        };
        assert_eq!(reference.slug(), "slatedb/slatedb#2098");
        assert_eq!(reference.title, "Use remote_seq");
        assert!(reference.pull);

        assert!(
            event(
                r#"{"event":"cross-referenced","created_at":"2026-09-18T12:59:22Z",
                    "source":{"type":"issue","issue":{"number":7,"title":"t"}}}"#
            )
            .is_none(),
            "a reference nobody can follow is not a line"
        );
    }

    #[test]
    fn the_rest_of_the_vocabulary_reads_and_the_unknown_is_dropped() {
        let of = |json: &str| event(json).map(|e| e.kind);
        let when = r#""created_at":"2026-09-16T12:00:00Z""#;
        assert_eq!(
            of(&format!(
                r#"{{"event":"assigned",{when},"assignee":{{"login":"rohan"}}}}"#
            )),
            Some(PullEventKind::Assigned {
                who: "rohan".into()
            })
        );
        assert_eq!(
            of(&format!(
                r#"{{"event":"unassigned",{when},"assignee":{{"login":"rohan"}}}}"#
            )),
            Some(PullEventKind::Unassigned {
                who: "rohan".into()
            })
        );
        assert_eq!(
            of(&format!(
                r#"{{"event":"renamed",{when},"rename":{{"from":"a","to":"b"}}}}"#
            )),
            Some(PullEventKind::Renamed {
                from: "a".into(),
                to: "b".into()
            })
        );
        assert_eq!(
            of(&format!(
                r#"{{"event":"milestoned",{when},"milestone":{{"title":"0.2"}}}}"#
            )),
            Some(PullEventKind::Milestoned {
                title: "0.2".into()
            })
        );
        assert_eq!(
            of(&format!(
                r#"{{"event":"demilestoned",{when},"milestone":{{"title":"0.2"}}}}"#
            )),
            Some(PullEventKind::Demilestoned {
                title: "0.2".into()
            })
        );
        assert_eq!(
            of(&format!(
                r#"{{"event":"head_ref_force_pushed",{when},"commit_id":"abc"}}"#
            )),
            Some(PullEventKind::ForcePushed { sha: "abc".into() })
        );
        assert_eq!(
            of(&format!(
                r#"{{"event":"referenced",{when},"commit_id":"abc"}}"#
            )),
            Some(PullEventKind::Referenced { sha: "abc".into() })
        );
        assert_eq!(
            of(&format!(
                r#"{{"event":"referenced",{when},"commit_id":null}}"#
            )),
            None,
            "a reference to no commit is not a line"
        );
        assert_eq!(
            of(&format!(
                r#"{{"event":"assigned",{when},"assignee":{{"login":""}}}}"#
            )),
            None,
            "a deleted account is not a line"
        );
        assert_eq!(
            of(&format!(
                r#"{{"event":"milestoned",{when},"milestone":{{"title":""}}}}"#
            )),
            None,
            "a milestone with no name is not a line"
        );
        assert_eq!(
            of(&format!(r#"{{"event":"convert_to_draft",{when}}}"#)),
            Some(PullEventKind::Draft)
        );
        assert_eq!(
            of(&format!(r#"{{"event":"ready_for_review",{when}}}"#)),
            Some(PullEventKind::Ready)
        );
        assert_eq!(
            of(&format!(r#"{{"event":"auto_merge_enabled",{when}}}"#)),
            Some(PullEventKind::AutoMergeEnabled)
        );
        assert_eq!(
            of(&format!(r#"{{"event":"added_to_merge_queue",{when}}}"#)),
            Some(PullEventKind::MergeQueued)
        );

        // Already a card of its own, kept to itself by GitHub, or newer
        // than this build.
        for event_name in [
            "commented",
            "reviewed",
            "line-commented",
            "mentioned",
            "subscribed",
            "something_new",
        ] {
            assert_eq!(
                of(&format!(r#"{{"event":"{event_name}",{when}}}"#)),
                None,
                "{event_name}"
            );
        }
        assert_eq!(
            of(r#"{"event":"labeled","label":{"name":"bug"}}"#),
            None,
            "an undated event has no place in the order"
        );
    }

    #[test]
    fn tidying_orders_by_time_and_says_a_merge_once() {
        let e = |kind, when| PullEvent::new(kind, at(when), "rohan");
        let events = tidy(vec![
            e(PullEventKind::Closed, "2026-09-17T15:00:00Z"),
            e(PullEventKind::Merged, "2026-09-17T15:00:00Z"),
            e(
                PullEventKind::Labeled {
                    label: Label {
                        name: "bug".into(),
                        color: String::new(),
                    },
                },
                "2026-09-15T08:00:00Z",
            ),
            e(PullEventKind::Closed, "2026-09-16T08:00:00Z"),
        ]);
        let kinds: Vec<_> = events.iter().map(|e| e.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                PullEventKind::Labeled {
                    label: Label {
                        name: "bug".into(),
                        color: String::new()
                    }
                },
                PullEventKind::Closed,
                PullEventKind::Merged,
            ],
            "the close beside the merge goes; the one before it, that was reopened, stays"
        );
    }
}
