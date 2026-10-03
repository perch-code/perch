//! Who an issue or pull request is assigned to, and who may change it:
//! GitHub lets anyone with triage access or more assign, and only the
//! repository's assignable users can be.

use serde::Deserialize;

use crate::mentions::MentionableUser;
use crate::pulls::RepositoryPermission;

/// The most people GitHub assigns one issue or pull request to.
pub const MAX_ASSIGNEES: usize = 10;

/// The assignees GitHub answered a change with, and when the issue last
/// changed by its clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assigned {
    pub assignees: Vec<String>,
    pub updated_at: jiff::Timestamp,
}

/// The reader's permission on the repository, and its assignable users,
/// at most `first` of them, in GitHub's order.
pub(crate) fn assignable_query() -> &'static str {
    "query($owner: String!, $name: String!, $first: Int!) {
  repository(owner: $owner, name: $name) {
    viewer_permission: viewerPermission
    assignable_users: assignableUsers(first: $first) {
      nodes { login name avatar_url: avatarUrl }
    }
  }
}"
}

#[derive(Debug, Deserialize)]
pub(crate) struct AssignableData {
    repository: Option<RepositoryNode>,
}

#[derive(Debug, Deserialize)]
struct RepositoryNode {
    #[serde(default)]
    viewer_permission: Option<String>,
    #[serde(default)]
    assignable_users: Option<UserConnection>,
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
    #[serde(default)]
    avatar_url: String,
}

impl AssignableData {
    /// The assignable users when the reader may assign them, none when
    /// they may not: triage access is the least that can.
    pub(crate) fn into_assignable(self) -> Option<Vec<MentionableUser>> {
        let repository = self.repository?;
        let permission = crate::pulls::repository_permission(
            repository.viewer_permission.as_deref().unwrap_or_default(),
        );
        let may_assign = matches!(
            permission,
            RepositoryPermission::Admin
                | RepositoryPermission::Maintain
                | RepositoryPermission::Write
                | RepositoryPermission::Triage
        );
        if !may_assign {
            return None;
        }
        Some(
            repository
                .assignable_users
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
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(permission: &str) -> AssignableData {
        serde_json::from_str(&format!(
            r#"{{"repository":{{"viewer_permission":"{permission}","assignable_users":{{"nodes":[
                {{"login":"criccomini","name":"Chris Riccomini","avatar_url":"https://avatars/1"}},
                null,
                {{"login":"","name":"Nobody"}},
                {{"login":"rohan","name":""}}
            ]}}}}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn triage_or_more_may_assign_the_repositorys_assignable_users() {
        for permission in ["ADMIN", "MAINTAIN", "WRITE", "TRIAGE"] {
            let users = data(permission).into_assignable().expect(permission);
            assert_eq!(
                users,
                vec![
                    MentionableUser {
                        login: "criccomini".into(),
                        name: Some("Chris Riccomini".into()),
                        avatar_url: "https://avatars/1".into(),
                    },
                    MentionableUser {
                        login: "rohan".into(),
                        name: None,
                        avatar_url: String::new(),
                    },
                ]
            );
        }
    }

    #[test]
    fn read_access_or_no_answer_may_not_assign() {
        assert_eq!(data("READ").into_assignable(), None);
        assert_eq!(data("").into_assignable(), None);
        let missing: AssignableData = serde_json::from_str(r#"{"repository":null}"#).unwrap();
        assert_eq!(missing.into_assignable(), None);
    }
}
