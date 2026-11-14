//! Stars against a scripted local server.

use perch_github::GitHub;

use crate::support::graphql::{spawn, Response};

#[tokio::test]
async fn a_star_is_read_and_then_added_by_the_repository_id() {
    let (base, requests) = spawn(vec![
        Response::ok(r#"{"data":{"repository":{"id":"R_1","viewerHasStarred":false}}}"#),
        Response::ok(r#"{"data":{"addStar":{"starrable":{"viewerHasStarred":true}}}}"#),
    ]);
    let github = GitHub::with_base_uri("token", &base).unwrap();

    let (id, starred) = github.fetch_star("perch-code/perch").await.unwrap();
    assert_eq!((id.as_str(), starred), ("R_1", false));
    github.add_star(&id).await.unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests[0].body["variables"]["owner"], "perch-code");
    assert_eq!(requests[0].body["variables"]["name"], "perch");
    let query = requests[1].body["query"].as_str().unwrap();
    assert!(
        query.contains("addStar(input: {starrableId: $id})"),
        "{query}"
    );
    assert_eq!(requests[1].body["variables"]["id"], "R_1");
}

#[tokio::test]
async fn a_star_github_did_not_add_is_an_error() {
    let (base, _) = spawn(vec![Response::ok(
        r#"{"data":{"addStar":{"starrable":{"viewerHasStarred":false}}}}"#,
    )]);
    let github = GitHub::with_base_uri("token", &base).unwrap();
    assert!(github.add_star("R_1").await.is_err());
}
