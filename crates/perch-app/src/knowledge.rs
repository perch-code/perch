//! From a pull request or an issue and its discussion to knowledge records.
//!
//! `perch-knowledge` does not know GitHub, so the shape of a pull request
//! or an issue is turned into plain messages here. Ids carry their kind so
//! a review and a comment with the same number cannot collide, and so an
//! issue and a pull request with the same number stay apart in the raw
//! file they share.

use std::path::PathBuf;

use perch_github::{Discussion, Issue, IssueComment, PullRequest};
use perch_knowledge::{Message, Source};

/// Every message on one pull request.
pub fn to_messages(pr: &PullRequest, discussion: &Discussion, project: &str) -> Vec<Message> {
    let container = format!("{}#{}", pr.repo_full_name, pr.number);
    let pr_url = format!("https://github.com/{}/pull/{}", pr.repo_full_name, pr.number);
    let base = |id: String, author: &str, at, body: String, url: String| Message {
        source: Source::GitHub,
        project: project.to_string(),
        container: container.clone(),
        thread: None,
        id,
        reply_to: None,
        author: author.to_string(),
        at,
        edited_at: None,
        body,
        url,
        location: None,
    };

    let mut out = Vec::new();

    // The description opens the container. Its first line is the title, as
    // a heading, which is the convention the renderer lifts into the file's
    // own heading. Without an opening time, anything no later than the
    // first comment keeps it first.
    let opened_at = discussion.opened_at.unwrap_or_else(|| {
        discussion
            .comments
            .iter()
            .map(|c| c.created_at)
            .chain(discussion.reviews.iter().filter_map(|r| r.submitted_at))
            .chain(discussion.review_comments.iter().map(|c| c.created_at))
            .min()
            .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
    });
    out.push(base(
        format!("pr-{}", pr.number),
        &pr.author_login,
        opened_at,
        format!("# {}\n\n{}", pr.title, discussion.body),
        pr_url.clone(),
    ));

    for c in &discussion.comments {
        let mut m = base(
            format!("comment-{}", c.id),
            &c.user,
            c.created_at,
            c.body.clone(),
            c.html_url.clone(),
        );
        m.edited_at = c.updated_at.filter(|u| *u > c.created_at);
        out.push(m);
    }

    for r in &discussion.reviews {
        // A pending review is a draft nobody else can see.
        let Some(at) = r.submitted_at else { continue };
        let verdict = match r.state.as_str() {
            "APPROVED" => "approved",
            "CHANGES_REQUESTED" => "requested changes",
            "DISMISSED" => "dismissed",
            _ => "commented",
        };
        let mut m = base(
            format!("review-{}", r.id),
            &r.user,
            at,
            format!("{verdict}\n\n{}", r.body),
            r.html_url.clone(),
        );
        m.thread = Some(format!("review-{}", r.id));
        out.push(m);
    }

    for c in &discussion.review_comments {
        let mut m = base(
            format!("review-comment-{}", c.id),
            &c.user,
            c.created_at,
            c.body.clone(),
            c.html_url.clone(),
        );
        m.thread = c.pull_request_review_id.map(|id| format!("review-{id}"));
        m.reply_to = c.in_reply_to_id.map(|id| format!("review-comment-{id}"));
        m.location = Some(c.location());
        m.edited_at = c.updated_at.filter(|u| *u > c.created_at);
        out.push(m);
    }

    out
}

/// Every message on one issue: the issue itself, then its comments.
///
/// The container is `owner/repo#issue-<n>`, which the knowledge crate
/// files under `issues/` beside the repository's pull requests.
// Nothing opens an issue yet; the backend's OpenItem for an issue item is
// the caller, and the allow goes when it lands.
pub fn issue_to_messages(issue: &Issue, comments: &[IssueComment], project: &str) -> Vec<Message> {
    let container = format!("{}#issue-{}", issue.repo_full_name, issue.number);
    let issue_url = format!("https://github.com/{}/issues/{}", issue.repo_full_name, issue.number);
    let base = |id: String, author: &str, at, body: String, url: String| Message {
        source: Source::GitHub,
        project: project.to_string(),
        container: container.clone(),
        thread: None,
        id,
        reply_to: None,
        author: author.to_string(),
        at,
        edited_at: None,
        body,
        url,
        location: None,
    };

    let mut out = Vec::new();

    // A snapshot from before the field existed has no opening time; the
    // first comment's is the latest it could have been.
    let opened_at = issue.created_at.unwrap_or_else(|| {
        comments
            .iter()
            .map(|c| c.created_at)
            .min()
            .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
    });
    out.push(base(
        format!("issue-{}", issue.number),
        &issue.author_login,
        opened_at,
        format!("# {}\n\n{}", issue.title, issue.body),
        issue_url,
    ));

    for c in comments {
        let mut m = base(
            format!("comment-{}", c.id),
            &c.user,
            c.created_at,
            c.body.clone(),
            c.html_url.clone(),
        );
        m.edited_at = c.updated_at.filter(|u| *u > c.created_at);
        out.push(m);
    }

    out
}

/// Where the two agents look for skills.
pub fn skill_dirs() -> Vec<PathBuf> {
    let home = perch_github::config::home_dir();
    vec![
        home.join(".claude").join("skills").join("perch"),
        home.join(".agents").join("skills").join("perch"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_github::{IssueComment, Review, ReviewComment};

    fn at(s: &str) -> jiff::Timestamp {
        s.parse().unwrap()
    }

    fn pr() -> PullRequest {
        PullRequest {
            number: 812,
            title: "Fence the manifest".into(),
            author_login: "rohan".into(),
            author_avatar_url: String::new(),
            head_sha: "h".into(),
            head_ref: "f".into(),
            base_sha: "b".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            repo_full_name: "slatedb/slatedb".into(),
            updated_at: None,
            review_requested: false,
            checks: None,
            review_decision: None,
        }
    }

    fn discussion() -> Discussion {
        Discussion {
            opened_at: Some(at("2026-09-14T14:02:00Z")),
            body: "The description.".into(),
            comments: vec![IssueComment {
                id: 1,
                user: "chris".into(),
                body: "q".into(),
                created_at: at("2026-09-15T09:14:00Z"),
                updated_at: Some(at("2026-09-15T09:30:00Z")),
                html_url: "https://github.com/slatedb/slatedb/pull/812#issuecomment-1".into(),
            }],
            reviews: vec![
                Review {
                    id: 9,
                    user: "chris".into(),
                    body: "nice".into(),
                    state: "APPROVED".into(),
                    submitted_at: Some(at("2026-09-16T11:40:00Z")),
                    html_url: "https://github.com/slatedb/slatedb/pull/812#pullrequestreview-9".into(),
                },
                Review {
                    id: 10,
                    user: "chris".into(),
                    body: "draft".into(),
                    state: "PENDING".into(),
                    submitted_at: None,
                    html_url: String::new(),
                },
            ],
            review_comments: vec![
                ReviewComment {
                    id: 3,
                    pull_request_review_id: Some(9),
                    in_reply_to_id: None,
                    user: "chris".into(),
                    body: "epoch".into(),
                    path: "src/manifest.rs".into(),
                    line: Some(41),
                    original_line: None,
                    created_at: at("2026-09-16T11:38:00Z"),
                    updated_at: Some(at("2026-09-16T11:38:00Z")),
                    html_url: "https://github.com/slatedb/slatedb/pull/812#discussion_r3".into(),
                },
                ReviewComment {
                    id: 4,
                    pull_request_review_id: Some(9),
                    in_reply_to_id: Some(3),
                    user: "rohan".into(),
                    body: "fixed".into(),
                    path: "src/manifest.rs".into(),
                    line: Some(41),
                    original_line: None,
                    created_at: at("2026-09-16T11:45:00Z"),
                    updated_at: None,
                    html_url: "https://github.com/slatedb/slatedb/pull/812#discussion_r4".into(),
                },
            ],
        }
    }

    #[test]
    fn ids_carry_their_kind_and_the_container_is_the_pull_request() {
        let ms = to_messages(&pr(), &discussion(), "slatedb");
        let ids: Vec<&str> = ms.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["pr-812", "comment-1", "review-9", "review-comment-3", "review-comment-4"]);
        assert!(ms.iter().all(|m| m.container == "slatedb/slatedb#812"));
        assert!(ms.iter().all(|m| m.project == "slatedb"));
        assert!(ms.iter().all(|m| m.source == Source::GitHub));
    }

    #[test]
    fn the_description_opens_with_the_title_as_a_heading_and_sorts_first() {
        let ms = to_messages(&pr(), &discussion(), "slatedb");
        assert_eq!(ms[0].body, "# Fence the manifest\n\nThe description.");
        assert_eq!(ms[0].author, "rohan");
        assert_eq!(ms[0].at, at("2026-09-14T14:02:00Z"));
        assert!(ms[0].at <= ms[1].at);
        assert_eq!(ms[0].url, "https://github.com/slatedb/slatedb/pull/812");
    }

    #[test]
    fn reviews_are_threads_and_line_comments_hang_under_them() {
        let ms = to_messages(&pr(), &discussion(), "slatedb");
        let review = ms.iter().find(|m| m.id == "review-9").unwrap();
        assert_eq!(review.thread.as_deref(), Some("review-9"));
        assert_eq!(review.body, "approved\n\nnice");

        let line = ms.iter().find(|m| m.id == "review-comment-3").unwrap();
        assert_eq!(line.thread.as_deref(), Some("review-9"));
        assert_eq!(line.location.as_deref(), Some("src/manifest.rs:41"));
        assert_eq!(line.reply_to, None);

        let reply = ms.iter().find(|m| m.id == "review-comment-4").unwrap();
        assert_eq!(reply.reply_to.as_deref(), Some("review-comment-3"));
    }

    #[test]
    fn a_pending_review_is_left_out() {
        let ms = to_messages(&pr(), &discussion(), "slatedb");
        assert!(!ms.iter().any(|m| m.id == "review-10"));
    }

    #[test]
    fn edited_means_updated_after_created() {
        let ms = to_messages(&pr(), &discussion(), "slatedb");
        assert!(ms.iter().find(|m| m.id == "comment-1").unwrap().edited_at.is_some());
        assert!(ms.iter().find(|m| m.id == "review-comment-3").unwrap().edited_at.is_none());
    }

    fn issue() -> Issue {
        Issue {
            number: 42,
            title: "Manifest races with compaction".into(),
            body: "Seen twice.".into(),
            state: "open".into(),
            state_reason: None,
            author_login: "rohan".into(),
            author_avatar_url: String::new(),
            labels: vec![],
            assignees: vec![],
            comments: 2,
            created_at: Some(at("2026-09-13T08:00:00Z")),
            updated_at: at("2026-09-13T09:30:00Z"),
            repo_full_name: "slatedb/slatedb".into(),
        }
    }

    fn issue_comments() -> Vec<IssueComment> {
        vec![
            IssueComment {
                id: 7,
                user: "chris".into(),
                body: "Repro attached.".into(),
                created_at: at("2026-09-13T09:00:00Z"),
                updated_at: Some(at("2026-09-13T09:30:00Z")),
                html_url: "https://github.com/slatedb/slatedb/issues/42#issuecomment-7".into(),
            },
            IssueComment {
                id: 8,
                user: "rohan".into(),
                body: "Thanks.".into(),
                created_at: at("2026-09-13T09:10:00Z"),
                updated_at: None,
                html_url: "https://github.com/slatedb/slatedb/issues/42#issuecomment-8".into(),
            },
        ]
    }

    #[test]
    fn an_issue_is_its_own_container_and_its_ids_say_so() {
        let ms = issue_to_messages(&issue(), &issue_comments(), "slatedb");
        let ids: Vec<&str> = ms.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["issue-42", "comment-7", "comment-8"]);
        assert!(ms.iter().all(|m| m.container == "slatedb/slatedb#issue-42"));
        assert!(ms.iter().all(|m| m.project == "slatedb"));
        assert!(ms.iter().all(|m| m.source == Source::GitHub));
        assert!(ms.iter().all(|m| m.thread.is_none() && m.location.is_none()));
    }

    #[test]
    fn the_issue_opens_with_its_title_at_its_creation_time() {
        let ms = issue_to_messages(&issue(), &issue_comments(), "slatedb");
        assert_eq!(ms[0].body, "# Manifest races with compaction\n\nSeen twice.");
        assert_eq!(ms[0].author, "rohan");
        assert_eq!(ms[0].at, at("2026-09-13T08:00:00Z"));
        assert_eq!(ms[0].url, "https://github.com/slatedb/slatedb/issues/42");
        assert!(ms.iter().find(|m| m.id == "comment-7").unwrap().edited_at.is_some());
        assert!(ms.iter().find(|m| m.id == "comment-8").unwrap().edited_at.is_none());
    }

    #[test]
    fn without_a_creation_time_the_first_comment_dates_the_opener() {
        let mut i = issue();
        i.created_at = None;
        let ms = issue_to_messages(&i, &issue_comments(), "slatedb");
        assert_eq!(ms[0].at, at("2026-09-13T09:00:00Z"));
        assert!(ms[0].at <= ms[1].at);
    }
}
