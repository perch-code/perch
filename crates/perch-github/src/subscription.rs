//! What the reader has done about an issue they didn't file: whether
//! they gave it a 👍 and whether they hear about it. Send Feedback reads
//! both for the issues that look like the report, and sets them on +1.

use serde::Deserialize;

/// An issue as +1 sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueViewerState {
    pub number: u64,
    pub node_id: String,
    pub thumbs_up: u64,
    pub viewer_thumbs_up: bool,
    /// `SUBSCRIBED`, `UNSUBSCRIBED` or `IGNORED`.
    pub viewer_subscription: String,
}

/// One query for several issues of one repository, each under an alias
/// of its number.
pub(crate) fn issues_query(numbers: &[u64]) -> String {
    let fields = numbers
        .iter()
        .map(|n| {
            format!(
                "i{n}: issue(number: {n}) {{ number id viewerSubscription \
                 reactionGroups {{ content viewerHasReacted reactors {{ totalCount }} }} }}"
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "query($owner: String!, $name: String!) {{ repository(owner: $owner, name: $name) {{ {fields} }} }}"
    )
}

pub(crate) const SUBSCRIPTION_MUTATION: &str = "mutation($id: ID!, $state: SubscriptionState!) { \
    updateSubscription(input: { subscribableId: $id, state: $state }) { \
    subscribable { viewerSubscription } } }";

#[derive(Debug, Deserialize)]
pub(crate) struct IssuesData {
    repository: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueNode {
    number: u64,
    id: String,
    #[serde(default)]
    viewer_subscription: Option<String>,
    #[serde(default)]
    reaction_groups: Vec<GroupNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupNode {
    content: String,
    #[serde(default)]
    viewer_has_reacted: bool,
    reactors: Option<Count>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Count {
    total_count: u64,
}

impl IssuesData {
    /// The issues found, in the order asked; one GitHub couldn't find is
    /// left out.
    pub(crate) fn into_states(self, numbers: &[u64]) -> Vec<IssueViewerState> {
        let Some(repository) = self.repository else {
            return Vec::new();
        };
        numbers
            .iter()
            .filter_map(|n| repository.get(&format!("i{n}")))
            .filter_map(|value| serde_json::from_value::<IssueNode>(value.clone()).ok())
            .map(|node| {
                let thumbs = node
                    .reaction_groups
                    .iter()
                    .find(|g| g.content == "THUMBS_UP");
                IssueViewerState {
                    number: node.number,
                    node_id: node.id,
                    thumbs_up: thumbs
                        .and_then(|g| g.reactors.as_ref())
                        .map_or(0, |c| c.total_count),
                    viewer_thumbs_up: thumbs.is_some_and(|g| g.viewer_has_reacted),
                    viewer_subscription: node
                        .viewer_subscription
                        .unwrap_or_else(|| "UNSUBSCRIBED".to_string()),
                }
            })
            .collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SubscriptionData {
    update_subscription: Option<SubscriptionPayload>,
}

#[derive(Debug, Deserialize)]
struct SubscriptionPayload {
    subscribable: Option<Subscribable>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Subscribable {
    viewer_subscription: Option<String>,
}

impl SubscriptionData {
    pub(crate) fn state(self) -> Option<String> {
        self.update_subscription?.subscribable?.viewer_subscription
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_aliases_each_number() {
        let query = issues_query(&[57, 9]);
        assert!(query.contains("i57: issue(number: 57)"));
        assert!(query.contains("i9: issue(number: 9)"));
        assert!(query.contains("viewerSubscription"));
    }

    #[test]
    fn states_parse_in_the_order_asked_and_skip_the_missing() {
        let data: IssuesData = serde_json::from_value(serde_json::json!({
            "repository": {
                "i9": {
                    "number": 9, "id": "I_9", "viewerSubscription": "SUBSCRIBED",
                    "reactionGroups": [
                        { "content": "THUMBS_UP", "viewerHasReacted": true, "reactors": { "totalCount": 4 } },
                        { "content": "HEART", "viewerHasReacted": false, "reactors": { "totalCount": 1 } }
                    ]
                },
                "i57": {
                    "number": 57, "id": "I_57", "viewerSubscription": "IGNORED",
                    "reactionGroups": []
                },
                "i3": null
            }
        }))
        .unwrap();
        let states = data.into_states(&[57, 3, 9]);
        assert_eq!(states.len(), 2);
        assert_eq!(states[0].number, 57);
        assert_eq!(states[0].thumbs_up, 0);
        assert!(!states[0].viewer_thumbs_up);
        assert_eq!(states[0].viewer_subscription, "IGNORED");
        assert_eq!(states[1].node_id, "I_9");
        assert_eq!(states[1].thumbs_up, 4);
        assert!(states[1].viewer_thumbs_up);
    }

    #[test]
    fn the_mutation_answer_parses() {
        let data: SubscriptionData = serde_json::from_value(serde_json::json!({
            "updateSubscription": { "subscribable": { "viewerSubscription": "SUBSCRIBED" } }
        }))
        .unwrap();
        assert_eq!(data.state().as_deref(), Some("SUBSCRIBED"));
    }
}
