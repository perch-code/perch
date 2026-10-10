//! Deleting and editing a comment against a scripted local server, and
//! the line a deletion leaves in the timeline.

use perch_github::{CommentPlace, Error, GitHub, PullEventKind};

use crate::support::graphql::{spawn, Response};

#[tokio::test]
async fn each_place_is_deleted_by_its_own_mutation() {
    let (base, requests) = spawn(vec![
        Response::ok(r#"{"data":{"deleteIssueComment":{"clientMutationId":null}}}"#),
        Response::ok(r#"{"data":{"deletePullRequestReviewComment":{"clientMutationId":null}}}"#),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    github
        .delete_comment("IC_1", CommentPlace::Conversation)
        .await
        .unwrap();
    github
        .delete_comment("PRRC_2", CommentPlace::Line)
        .await
        .unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let query = |i: usize| requests[i].body["query"].as_str().unwrap().to_string();
    assert!(query(0).contains("deleteIssueComment(input: {id: $id})"));
    assert_eq!(requests[0].body["variables"]["id"], "IC_1");
    assert!(query(1).contains("deletePullRequestReviewComment(input: {id: $id})"));
    assert_eq!(requests[1].body["variables"]["id"], "PRRC_2");
    assert_eq!(requests[0].path, "/graphql");
}

#[tokio::test]
async fn each_place_is_edited_by_its_own_mutation_and_answers_the_kept_body() {
    let (base, requests) = spawn(vec![
        Response::ok(r#"{"data":{"updateIssueComment":{"issueComment":{"body":"fixed"}}}}"#),
        Response::ok(
            r#"{"data":{"updatePullRequestReviewComment":{"pullRequestReviewComment":{"body":"also fixed"}}}}"#,
        ),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let kept = github
        .update_comment("IC_1", CommentPlace::Conversation, "fixed")
        .await
        .unwrap();
    assert_eq!(kept.body, "fixed");
    assert_eq!(kept.updated_at, None);
    let kept = github
        .update_comment("PRRC_2", CommentPlace::Line, "also fixed")
        .await
        .unwrap();
    assert_eq!(kept.body, "also fixed");

    let requests = requests.lock().unwrap();
    let query = |i: usize| requests[i].body["query"].as_str().unwrap().to_string();
    assert!(query(0).contains("updateIssueComment(input: {id: $id, body: $body})"));
    assert_eq!(requests[0].body["variables"]["id"], "IC_1");
    assert_eq!(requests[0].body["variables"]["body"], "fixed");
    assert!(query(1).contains(
        "updatePullRequestReviewComment(input: {pullRequestReviewCommentId: $id, body: $body})"
    ));
    assert_eq!(requests[1].body["variables"]["id"], "PRRC_2");
    assert_eq!(requests[1].body["variables"]["body"], "also fixed");
}

#[tokio::test]
async fn a_description_and_a_review_summary_are_edited_by_their_own_mutations() {
    let (base, requests) = spawn(vec![
        Response::ok(r#"{"data":{"updatePullRequest":{"pullRequest":{"body":"pr"}}}}"#),
        Response::ok(r#"{"data":{"updateIssue":{"issue":{"body":"issue"}}}}"#),
        Response::ok(
            r#"{"data":{"updatePullRequestReview":{"pullRequestReview":{"body":"lgtm"}}}}"#,
        ),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let places = [
        ("PR_1", CommentPlace::PullRequestBody, "pr"),
        ("I_1", CommentPlace::IssueBody, "issue"),
        ("PRR_1", CommentPlace::Review, "lgtm"),
    ];
    for (id, place, body) in places {
        let kept = github.update_comment(id, place, body).await.unwrap();
        assert_eq!(kept.body, body);
        assert_eq!(
            kept.updated_at, None,
            "an answer without its time still reads"
        );
    }

    let requests = requests.lock().unwrap();
    let query = |i: usize| requests[i].body["query"].as_str().unwrap().to_string();
    assert!(query(0).contains("updatePullRequest(input: {pullRequestId: $id, body: $body})"));
    assert!(query(1).contains("updateIssue(input: {id: $id, body: $body})"));
    assert!(query(2)
        .contains("updatePullRequestReview(input: {pullRequestReviewId: $id, body: $body})"));
    assert_eq!(requests[2].body["variables"]["id"], "PRR_1");
}

#[tokio::test]
async fn a_description_or_a_review_is_never_deleted() {
    let (base, requests) = spawn(Vec::new());
    let github = GitHub::with_base_uri("token", &base).unwrap();
    for place in [
        CommentPlace::PullRequestBody,
        CommentPlace::IssueBody,
        CommentPlace::Review,
    ] {
        let refused = github.delete_comment("X_1", place).await.unwrap_err();
        assert!(matches!(refused, Error::Decode(_)), "{refused:?}");
    }
    assert!(requests.lock().unwrap().is_empty(), "nothing was asked");
}

#[tokio::test]
async fn a_refused_edit_is_an_error_and_no_id_asks_nothing() {
    let (base, requests) = spawn(vec![Response::ok(
        r#"{"data":{"updateIssueComment":null},"errors":[{"message":"Resource not accessible by integration"}]}"#,
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let refused = github
        .update_comment("IC_1", CommentPlace::Conversation, "no")
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, Error::Graphql(message) if message.contains("not accessible")),
        "{refused:?}"
    );
    let none = github
        .update_comment("", CommentPlace::Line, "no")
        .await
        .unwrap_err();
    assert!(matches!(none, Error::Decode(_)), "{none:?}");
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_refused_deletion_is_an_error_and_no_id_asks_nothing() {
    let (base, requests) = spawn(vec![Response::ok(
        r#"{"data":{"deleteIssueComment":null},"errors":[{"message":"Resource not accessible by integration"}]}"#,
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let refused = github
        .delete_comment("IC_1", CommentPlace::Conversation)
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, Error::Graphql(message) if message.contains("not accessible")),
        "{refused:?}"
    );
    let none = github
        .delete_comment("", CommentPlace::Line)
        .await
        .unwrap_err();
    assert!(matches!(none, Error::Decode(_)), "{none:?}");
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_deleted_comment_is_a_timeline_line_naming_whose_it_was() {
    let (base, requests) = spawn(vec![
        Response::ok(
            r#"[{"event":"comment_deleted","node_id":"CDE_1","actor":{"login":"criccomini"},"created_at":"2026-10-03T22:15:58Z"},
                {"event":"comment_deleted","node_id":"CDE_2","actor":{"login":"criccomini"},"created_at":"2026-10-03T22:16:30Z"}]"#,
        ),
        Response::ok(
            r#"{"data":{"nodes":[{"deletedCommentAuthor":{"login":"kvnloo"}},{"deletedCommentAuthor":null}]}}"#,
        ),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let events = github.fetch_timeline("a/b", 7, &[]).await.unwrap();
    let kinds: Vec<_> = events
        .iter()
        .map(|e| (e.actor.as_str(), e.kind.clone()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (
                "criccomini",
                PullEventKind::CommentDeleted {
                    author: "kvnloo".into()
                }
            ),
            (
                "criccomini",
                PullEventKind::CommentDeleted {
                    author: String::new()
                }
            ),
        ],
        "an author GitHub no longer has leaves the line unnamed"
    );
    let requests = requests.lock().unwrap();
    assert_eq!(requests[1].path, "/graphql");
    assert_eq!(
        requests[1].body["variables"]["ids"],
        serde_json::json!(["CDE_1", "CDE_2"])
    );
}

#[tokio::test]
async fn a_timeline_without_deletions_asks_graphql_nothing() {
    let (base, requests) = spawn(vec![Response::ok(
        r#"[{"event":"closed","actor":{"login":"chris"},"created_at":"2026-10-03T22:15:58Z"}]"#,
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();
    let events = github.fetch_timeline("a/b", 7, &[]).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(requests.lock().unwrap().len(), 1);
}
