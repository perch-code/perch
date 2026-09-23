//! The people an `@` can name in a repository: GitHub's mentionable
//! users, whose logins and names its own comment box suggests.

use serde::Deserialize;

/// Someone who can be mentioned, as the suggestion list shows them: the
/// login, the name on their profile when they gave one, and the avatar.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MentionableUser {
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: String,
}

/// The repository's mentionable users whose login or name matches
/// `query`, as GitHub's comment box asks for them.
pub(crate) fn mentionable_users_query() -> &'static str {
    "query($owner: String!, $name: String!, $query: String, $first: Int!) {
  repository(owner: $owner, name: $name) {
    mentionableUsers(query: $query, first: $first) {
      nodes { login name avatarUrl }
    }
  }
}"
}

#[derive(Debug, Deserialize)]
pub(crate) struct MentionableUsersData {
    repository: Option<RepositoryNode>,
}

#[derive(Debug, Deserialize)]
struct RepositoryNode {
    #[serde(rename = "mentionableUsers")]
    mentionable_users: Option<UserConnection>,
}

#[derive(Debug, Deserialize)]
struct UserConnection {
    #[serde(default)]
    nodes: Vec<Option<UserNode>>,
}

#[derive(Debug, Deserialize)]
struct UserNode {
    #[serde(default)]
    login: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "avatarUrl")]
    avatar_url: String,
}

impl MentionableUsersData {
    /// The users in GitHub's order. A node GitHub could not resolve, or
    /// one without a login, is left out, and so is a blank name.
    pub(crate) fn into_users(self) -> Vec<MentionableUser> {
        self.repository
            .and_then(|r| r.mentionable_users)
            .map(|c| c.nodes)
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .filter(|n| !n.login.is_empty())
            .map(|n| MentionableUser {
                login: n.login,
                name: n.name.filter(|name| !name.trim().is_empty()),
                avatar_url: n.avatar_url,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn users_come_in_githubs_order_without_blank_names_or_missing_nodes() {
        let data: MentionableUsersData = serde_json::from_str(
            r#"{"repository":{"mentionableUsers":{"nodes":[
                {"login":"cadonna","name":"Bruno Cadonna","avatarUrl":"https://avatars/1"},
                null,
                {"login":"agavra","name":""},
                {"login":"","name":"Nobody"},
                {"login":"calavera","name":null}
            ]}}}"#,
        )
        .unwrap();
        assert_eq!(
            data.into_users(),
            vec![
                MentionableUser {
                    login: "cadonna".into(),
                    name: Some("Bruno Cadonna".into()),
                    avatar_url: "https://avatars/1".into(),
                },
                MentionableUser {
                    login: "agavra".into(),
                    name: None,
                    avatar_url: String::new(),
                },
                MentionableUser {
                    login: "calavera".into(),
                    name: None,
                    avatar_url: String::new(),
                },
            ]
        );
    }

    #[test]
    fn a_repository_github_cannot_see_has_nobody() {
        let data: MentionableUsersData = serde_json::from_str(r#"{"repository":null}"#).unwrap();
        assert!(data.into_users().is_empty());
    }
}
