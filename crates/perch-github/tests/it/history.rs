use perch_github::{Error, GitHub, HistoryStream, HistorySubject, ItemKind, ItemSubject};

use crate::support::page;
use crate::support::rest::{spawn, Response};

#[tokio::test]
async fn history_lists_both_item_kinds_and_finds_comments_edited_since_the_checkpoint() {
    let full_page = page((1..=100).map(|n| format!(r#"{{"number":{n}}}"#)).collect());
    let (base, requests) = spawn(vec![
        Response::new("200 OK", full_page).with_link(
            r#"<https://api.github.com/repositories/123/issues?per_page=100&after=a%2Bb&page=2>; rel="next""#,
        ),
        Response::new("200 OK", r#"[{"number":101,"pull_request":{}}]"#),
        Response::new(
            "200 OK",
            r#"[{"html_url":"https://github.com/a/b/issues/8#issuecomment-1"},{"html_url":"https://github.com/a/b/pull/101#issuecomment-2"}]"#,
        ),
        Response::new(
            "200 OK",
            r#"[{"pull_request_url":"https://api.github.com/repos/a/b/pulls/80"}]"#,
        ),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();
    let since = Some("2026-09-16T12:00:00.25Z".parse().unwrap());

    let first = github
        .history_page("a/b", HistoryStream::Items, None, None)
        .await
        .unwrap();
    assert_eq!(first.subjects.len(), 100);
    let second = github
        .history_page(
            "a/b",
            HistoryStream::Items,
            None,
            Some(first.next_path.as_deref().unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(
        second.subjects,
        [HistorySubject {
            kind: ItemKind::PullRequest,
            number: 101
        }]
    );
    assert_eq!(second.next_path, None);

    let comments = github
        .history_page("a/b", HistoryStream::IssueComments, since, None)
        .await
        .unwrap();
    assert_eq!(
        comments.subjects,
        [
            HistorySubject {
                kind: ItemKind::Issue,
                number: 8
            },
            HistorySubject {
                kind: ItemKind::PullRequest,
                number: 101
            },
        ]
    );
    let comments = github
        .history_page("a/b", HistoryStream::ReviewComments, since, None)
        .await
        .unwrap();
    assert_eq!(
        comments.subjects,
        [HistorySubject {
            kind: ItemKind::PullRequest,
            number: 80
        }]
    );

    let requests = requests.lock().unwrap();
    assert_eq!(
        requests[0].path,
        "/repos/a/b/issues?state=all&sort=created&direction=asc&per_page=100"
    );
    assert_eq!(
        requests[1].path,
        "/repositories/123/issues?per_page=100&after=a%2Bb&page=2"
    );
    assert_eq!(requests[2].path, "/repos/a/b/issues/comments?sort=created&direction=asc&per_page=100&since=2026-09-16T12:00:00Z");
    assert_eq!(requests[3].path, "/repos/a/b/pulls/comments?sort=created&direction=asc&per_page=100&since=2026-09-16T12:00:00Z");
}

#[tokio::test]
async fn history_keeps_submitted_reviews_and_inline_threads() {
    let (base, requests) = spawn(vec![
        Response::new(
            "200 OK",
            r#"{"number":5,"repository_url":"https://api.github.com/repos/new/b","pull_request":{},"updated_at":"2026-09-16T12:00:00Z"}"#,
        ),
        Response::new(
            "200 OK",
            r#"{"number":5,"title":"Manifest","body":"Description","state":"closed","merged":true,"head":{},"base":{"repo":{"full_name":"new/b"}}}"#,
        ),
        Response::new(
            "200 OK",
            r#"[{"id":9,"body":"Conversation","created_at":"2026-09-16T12:00:00Z"}]"#,
        ),
        Response::new(
            "200 OK",
            r#"[{"id":1,"state":"APPROVED","body":"Reviewed"},{"id":2,"state":"PENDING","body":"Draft review"}]"#,
        ),
        Response::new(
            "200 OK",
            r#"[{"id":3,"pull_request_review_id":1,"body":"Inline comment","path":"manifest.rs","created_at":"2026-09-16T12:00:00Z"},{"id":4,"pull_request_review_id":2,"body":"Draft comment","created_at":"2026-09-16T12:00:00Z"}]"#,
        ),
        Response::new(
            "200 OK",
            r#"{"data":{"repository":{"pull_request":{"review_threads":{"nodes":[{"id":"thread","is_resolved":true,"comments":{"nodes":[{"database_id":3}]}}]}}}}}"#,
        ),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();
    let (subject, discussion) = github
        .fetch_history_subject(
            "a/b",
            "new/b",
            HistorySubject {
                kind: ItemKind::Issue,
                number: 5,
            },
        )
        .await
        .unwrap()
        .unwrap();

    assert!(
        matches!(subject, ItemSubject::PullRequest(pull) if pull.merged && pull.repo_full_name == "a/b")
    );
    assert_eq!(discussion.body, "Description");
    assert_eq!(discussion.comments[0].body, "Conversation");
    assert_eq!(discussion.reviews.len(), 1);
    assert_eq!(discussion.reviews[0].body, "Reviewed");
    assert_eq!(discussion.review_comments.len(), 1);
    assert_eq!(discussion.review_comments[0].body, "Inline comment");
    assert!(discussion.review_threads[0].resolved);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 6);
    assert_eq!(requests[0].path, "/repos/a/b/issues/5");
    assert_eq!(requests[1].path, "/repos/a/b/pulls/5");
    assert_eq!(requests[5].path, "/graphql");
}

#[tokio::test]
async fn a_missing_conversation_does_not_mark_the_subject_missing() {
    let (base, _) = spawn(vec![
        Response::new("410 Gone", "{}"),
        Response::new(
            "200 OK",
            r#"{"number":2,"updated_at":"2026-09-16T12:00:00Z"}"#,
        ),
        Response::new("404 Not Found", "{}"),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();
    let missing = HistorySubject {
        kind: ItemKind::Issue,
        number: 1,
    };
    assert!(github
        .fetch_history_subject("a/b", "a/b", missing)
        .await
        .unwrap()
        .is_none());
    let present = HistorySubject {
        number: 2,
        ..missing
    };
    assert!(matches!(
        github.fetch_history_subject("a/b", "a/b", present).await,
        Err(Error::HistorySubjectUnavailable)
    ));
}

#[tokio::test]
async fn ambiguous_404s_never_confirm_subject_deletion() {
    let (base, requests) = spawn(vec![
        Response::new("404 Not Found", "{}"),
        Response::new("404 Not Found", "{}"),
        Response::new("404 Not Found", "{}"),
        Response::new("200 OK", r#"{"full_name":"a/b"}"#),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();
    let subject = HistorySubject {
        kind: ItemKind::Issue,
        number: 1,
    };
    assert!(matches!(
        github.fetch_history_subject("a/b", "a/b", subject).await,
        Err(Error::Status { status: 404, .. })
    ));
    assert!(matches!(
        github.fetch_history_subject("a/b", "a/b", subject).await,
        Err(Error::HistorySubjectUnavailable)
    ));
    assert_eq!(requests.lock().unwrap()[1].path, "/repos/a/b");
}

#[tokio::test]
async fn a_transferred_issue_cannot_replace_history_in_its_old_repository() {
    let (base, requests) = spawn(vec![
        Response::new(
            "200 OK",
            r#"{"number":7,"repository_url":"https://api.github.com/repos/other/repo","updated_at":"2026-09-16T12:00:00Z"}"#,
        ),
        Response::new("200 OK", r#"{"full_name":"renamed/b"}"#),
        Response::new(
            "200 OK",
            r#"{"number":9,"repository_url":"https://api.github.com/repos/renamed/b","updated_at":"2026-09-16T12:00:00Z"}"#,
        ),
        Response::new("200 OK", "[]"),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();
    let issue = HistorySubject {
        kind: ItemKind::Issue,
        number: 7,
    };
    assert!(github
        .fetch_history_subject("a/b", "renamed/b", issue)
        .await
        .unwrap()
        .is_none());
    let (renamed, _) = github
        .fetch_history_subject("a/b", "renamed/b", HistorySubject { number: 9, ..issue })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(renamed.repo_full_name(), "a/b");
    assert_eq!(requests.lock().unwrap().len(), 4);
}
