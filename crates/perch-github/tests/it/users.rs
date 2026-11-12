//! An account's profile against a local server, as the card under a
//! login asks for it.

use perch_github::GitHub;

use crate::support::rest::{spawn, Response};

#[tokio::test]
async fn a_profile_is_read_from_the_users_endpoint() {
    let (base, requests) = spawn(vec![Response::new(
        "200 OK",
        r#"{"login":"nomiero","id":1,"avatar_url":"https://avatars/u/1?v=4","type":"User",
            "name":"Nomi Ero","company":"@slatedb","location":"Lisbon","bio":null}"#,
    )]);
    let gh = GitHub::with_base_uri("t", &base).unwrap();

    let profile = gh.profile("nomiero").await.unwrap().unwrap();
    assert_eq!(profile.login, "nomiero");
    assert_eq!(profile.avatar_url, "https://avatars/u/1?v=4");
    assert_eq!(profile.name(), Some("Nomi Ero"));
    assert_eq!(profile.company(), Some("@slatedb"));
    assert_eq!(profile.location(), Some("Lisbon"));
    assert_eq!(requests.lock().unwrap()[0].path, "/users/nomiero");
}

#[tokio::test]
async fn a_login_with_no_account_has_no_profile() {
    let (base, _) = spawn(vec![Response::new(
        "404 Not Found",
        r#"{"message":"Not Found"}"#,
    )]);
    let gh = GitHub::with_base_uri("t", &base).unwrap();

    assert_eq!(gh.profile("ghost").await.unwrap(), None);
}
