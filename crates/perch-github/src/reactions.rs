//! Emoji reactions: the eight GitHub offers, how many of each a comment
//! has, and which of them the reader left.
//!
//! GitHub's REST lists count reactions but do not say whose they are, and
//! count none on a review or on a pull request's own body. GraphQL says
//! both for any node in one `nodes(ids:)` query, a hundred ids at a time,
//! so a discussion's reactions are read there after its lists, and a
//! reaction is added or taken back there too, by the subject's node id.
//! The first few who left each kind come with it, for the pill's tip.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// The eight reactions GitHub offers, in the order its picker shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReactionKind {
    ThumbsUp,
    ThumbsDown,
    Laugh,
    Hooray,
    Confused,
    Heart,
    Rocket,
    Eyes,
}

impl ReactionKind {
    pub const ALL: [ReactionKind; 8] = [
        ReactionKind::ThumbsUp,
        ReactionKind::ThumbsDown,
        ReactionKind::Laugh,
        ReactionKind::Hooray,
        ReactionKind::Confused,
        ReactionKind::Heart,
        ReactionKind::Rocket,
        ReactionKind::Eyes,
    ];

    /// The emoji, as GitHub draws it.
    pub fn emoji(self) -> &'static str {
        match self {
            ReactionKind::ThumbsUp => "👍",
            ReactionKind::ThumbsDown => "👎",
            ReactionKind::Laugh => "😄",
            ReactionKind::Hooray => "🎉",
            ReactionKind::Confused => "😕",
            ReactionKind::Heart => "❤️",
            ReactionKind::Rocket => "🚀",
            ReactionKind::Eyes => "👀",
        }
    }

    /// The name in the tip of the picker's emoji.
    pub fn label(self) -> &'static str {
        match self {
            ReactionKind::ThumbsUp => "Thumbs up",
            ReactionKind::ThumbsDown => "Thumbs down",
            ReactionKind::Laugh => "Laugh",
            ReactionKind::Hooray => "Hooray",
            ReactionKind::Confused => "Confused",
            ReactionKind::Heart => "Heart",
            ReactionKind::Rocket => "Rocket",
            ReactionKind::Eyes => "Eyes",
        }
    }

    /// GraphQL's `ReactionContent` value.
    pub fn graphql(self) -> &'static str {
        match self {
            ReactionKind::ThumbsUp => "THUMBS_UP",
            ReactionKind::ThumbsDown => "THUMBS_DOWN",
            ReactionKind::Laugh => "LAUGH",
            ReactionKind::Hooray => "HOORAY",
            ReactionKind::Confused => "CONFUSED",
            ReactionKind::Heart => "HEART",
            ReactionKind::Rocket => "ROCKET",
            ReactionKind::Eyes => "EYES",
        }
    }

    fn from_graphql(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.graphql() == value)
    }
}

/// One reaction on a subject: how many left it, whether the reader is
/// among them, and the logins of the first of them, at most
/// `REACTORS_NAMED`, as GitHub orders them. None on a payload an older
/// build kept.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReactionGroup {
    pub kind: ReactionKind,
    pub count: u32,
    pub viewer_reacted: bool,
    #[serde(default)]
    pub reactors: Vec<String>,
}

/// How many of those who left a reaction GitHub is asked to name.
const REACTORS_NAMED: usize = 10;

/// The reactions on one subject: only the kinds anyone left, in picker
/// order. Empty on a payload an older build kept, which had none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Reactions {
    pub groups: Vec<ReactionGroup>,
}

impl Reactions {
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    pub fn count(&self, kind: ReactionKind) -> u32 {
        self.groups
            .iter()
            .find(|g| g.kind == kind)
            .map_or(0, |g| g.count)
    }

    pub fn viewer_reacted(&self, kind: ReactionKind) -> bool {
        self.groups
            .iter()
            .any(|g| g.kind == kind && g.viewer_reacted)
    }

    /// The reader's reaction added or taken back, as the row shows it
    /// before GitHub answers. Adding one already there, or taking back one
    /// that is not, changes nothing. The names stay as GitHub gave them;
    /// the tip puts the reader in or out by `viewer_reacted`.
    pub fn toggle(&mut self, kind: ReactionKind, add: bool) {
        if self.viewer_reacted(kind) == add {
            return;
        }
        match self.groups.iter_mut().find(|g| g.kind == kind) {
            Some(group) if add => {
                group.count += 1;
                group.viewer_reacted = true;
            }
            Some(group) => {
                group.count = group.count.saturating_sub(1);
                group.viewer_reacted = false;
            }
            None if add => {
                self.groups.push(ReactionGroup {
                    kind,
                    count: 1,
                    viewer_reacted: true,
                    reactors: Vec::new(),
                });
                self.groups.sort_by_key(|g| g.kind);
            }
            None => {}
        }
        self.groups.retain(|g| g.count > 0);
    }

    /// GitHub's groups, one per kind whether anyone left it or not, kept
    /// to the ones somebody did. GitHub sends them in picker order.
    fn from_groups(groups: Vec<ReactionGroupPayload>) -> Self {
        let mut groups: Vec<ReactionGroup> = groups
            .into_iter()
            .filter(|g| g.reactors.total_count > 0)
            .filter_map(|g| {
                Some(ReactionGroup {
                    kind: ReactionKind::from_graphql(&g.content)?,
                    count: g.reactors.total_count,
                    viewer_reacted: g.viewer_has_reacted,
                    reactors: g
                        .reactors
                        .nodes
                        .into_iter()
                        .flatten()
                        .map(|r| r.login)
                        .filter(|login| !login.is_empty())
                        .collect(),
                })
            })
            .collect();
        groups.sort_by_key(|g| g.kind);
        Reactions { groups }
    }
}

// ---------------------------------------------------------------------
// The wire
// ---------------------------------------------------------------------

/// `reactionGroups` on any `Reactable`, as every query here asks for it.
/// A reactor is a user, a bot, an organization or a mannequin, each an
/// `Actor` with a login.
fn reaction_groups() -> String {
    format!(
        "reactionGroups {{ content viewerHasReacted reactors(first: {REACTORS_NAMED}) {{ totalCount nodes {{ ... on Actor {{ login }} }} }} }}"
    )
}

/// The reactions on up to a hundred nodes at once, and whether the
/// reader may edit and delete each, which costs nothing more to ask in
/// passing.
pub(crate) fn reactions_query() -> String {
    format!(
        "query($ids: [ID!]!) {{ nodes(ids: $ids) {{ ... on Reactable {{ {} }} ... on Deletable {{ viewerCanDelete }} ... on Updatable {{ viewerCanUpdate }} }} }}",
        reaction_groups()
    )
}

/// Add the reader's reaction to a subject, or take it back, and read the
/// subject's reactions as they stand after.
pub(crate) fn reaction_mutation(add: bool) -> String {
    let field = if add { "addReaction" } else { "removeReaction" };
    format!(
        "mutation($subject: ID!, $content: ReactionContent!) {{ {field}(input: {{subjectId: $subject, content: $content}}) {{ subject {{ {} }} }} }}",
        reaction_groups()
    )
}

/// The most ids one `nodes(ids:)` query may name.
pub(crate) const NODES_PER_QUERY: usize = 100;

#[derive(Debug, Deserialize)]
pub(crate) struct ReactionGroupPayload {
    #[serde(default)]
    content: String,
    #[serde(default, rename = "viewerHasReacted")]
    viewer_has_reacted: bool,
    #[serde(default)]
    reactors: ReactorsPayload,
}

#[derive(Debug, Default, Deserialize)]
struct ReactorsPayload {
    #[serde(default, rename = "totalCount")]
    total_count: u32,
    #[serde(default)]
    nodes: Vec<Option<ReactorPayload>>,
}

#[derive(Debug, Default, Deserialize)]
struct ReactorPayload {
    #[serde(default)]
    login: String,
}

/// One node's answer. A node that is not reactable, or that GitHub could
/// not find, has no groups; one that cannot be deleted or edited, or that
/// the mutation's answer did not ask about, is not the reader's to delete
/// or edit.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct ReactableNode {
    #[serde(default, rename = "reactionGroups")]
    reaction_groups: Vec<ReactionGroupPayload>,
    #[serde(default, rename = "viewerCanDelete")]
    viewer_can_delete: bool,
    #[serde(default, rename = "viewerCanUpdate")]
    viewer_can_update: bool,
}

impl ReactableNode {
    pub(crate) fn into_reactions(self) -> Reactions {
        Reactions::from_groups(self.reaction_groups)
    }
}

/// What the node query reads of one node: its reactions, and whether the
/// reader may delete and edit it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NodeReading {
    pub reactions: Reactions,
    pub viewer_can_delete: bool,
    pub viewer_can_update: bool,
}

/// The `nodes` answer, one entry per id asked for, in order; `null` for a
/// node GitHub could not resolve.
#[derive(Debug, Deserialize)]
pub(crate) struct ReactionsData {
    #[serde(default)]
    nodes: Vec<Option<ReactableNode>>,
}

impl ReactionsData {
    /// What was read of each node, by id, for the ids the query named. A
    /// node GitHub did not answer for is left out.
    pub(crate) fn by_id(self, ids: &[String]) -> HashMap<String, NodeReading> {
        ids.iter()
            .zip(self.nodes)
            .filter_map(|(id, node)| {
                let node = node?;
                let (viewer_can_delete, viewer_can_update) =
                    (node.viewer_can_delete, node.viewer_can_update);
                Some((
                    id.clone(),
                    NodeReading {
                        reactions: node.into_reactions(),
                        viewer_can_delete,
                        viewer_can_update,
                    },
                ))
            })
            .collect()
    }
}

/// A mutation's answer: the subject with its reactions after the change.
#[derive(Debug, Deserialize)]
pub(crate) struct ReactionMutationData {
    #[serde(default, rename = "addReaction", alias = "removeReaction")]
    payload: Option<ReactionMutationPayload>,
}

#[derive(Debug, Deserialize)]
struct ReactionMutationPayload {
    #[serde(default)]
    subject: Option<ReactableNode>,
}

impl ReactionMutationData {
    pub(crate) fn into_reactions(self) -> Option<Reactions> {
        Some(self.payload?.subject?.into_reactions())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(content: &str, count: u32, mine: bool) -> ReactionGroupPayload {
        ReactionGroupPayload {
            content: content.into(),
            viewer_has_reacted: mine,
            reactors: ReactorsPayload {
                total_count: count,
                nodes: Vec::new(),
            },
        }
    }

    #[test]
    fn only_the_reactions_somebody_left_are_kept_in_picker_order() {
        let reactions = Reactions::from_groups(vec![
            group("EYES", 2, true),
            group("THUMBS_UP", 3, false),
            group("LAUGH", 0, false),
            group("SHRUG", 4, false),
        ]);
        assert_eq!(
            reactions.groups,
            vec![
                ReactionGroup {
                    kind: ReactionKind::ThumbsUp,
                    count: 3,
                    viewer_reacted: false,
                    reactors: vec![],
                },
                ReactionGroup {
                    kind: ReactionKind::Eyes,
                    count: 2,
                    viewer_reacted: true,
                    reactors: vec![],
                },
            ]
        );
        assert_eq!(reactions.count(ReactionKind::ThumbsUp), 3);
        assert_eq!(reactions.count(ReactionKind::Laugh), 0);
        assert!(reactions.viewer_reacted(ReactionKind::Eyes));
        assert!(!reactions.viewer_reacted(ReactionKind::ThumbsUp));
    }

    #[test]
    fn toggling_adds_and_takes_back_the_readers_own_reaction() {
        let mut reactions = Reactions::from_groups(vec![group("HEART", 1, false)]);
        reactions.toggle(ReactionKind::ThumbsUp, true);
        assert_eq!(reactions.count(ReactionKind::ThumbsUp), 1);
        assert!(reactions.viewer_reacted(ReactionKind::ThumbsUp));
        assert_eq!(
            reactions.groups.iter().map(|g| g.kind).collect::<Vec<_>>(),
            vec![ReactionKind::ThumbsUp, ReactionKind::Heart],
            "a new kind takes its place in picker order"
        );
        // Adding what is there already is nothing.
        reactions.toggle(ReactionKind::ThumbsUp, true);
        assert_eq!(reactions.count(ReactionKind::ThumbsUp), 1);

        reactions.toggle(ReactionKind::Heart, true);
        assert_eq!(reactions.count(ReactionKind::Heart), 2);
        reactions.toggle(ReactionKind::Heart, false);
        assert_eq!(reactions.count(ReactionKind::Heart), 1);
        assert!(!reactions.viewer_reacted(ReactionKind::Heart));
        // Taking back what is not there is nothing either.
        reactions.toggle(ReactionKind::Heart, false);
        assert_eq!(reactions.count(ReactionKind::Heart), 1);

        reactions.toggle(ReactionKind::ThumbsUp, false);
        assert_eq!(reactions.count(ReactionKind::ThumbsUp), 0);
        assert_eq!(
            reactions.groups.len(),
            1,
            "a kind nobody left any more goes"
        );
    }

    #[test]
    fn a_missing_node_is_left_out_of_the_answer() {
        let data: ReactionsData = serde_json::from_str(
            r#"{"nodes":[{"reactionGroups":[{"content":"ROCKET","viewerHasReacted":true,"reactors":{"totalCount":1}}]},null,{}]}"#,
        )
        .unwrap();
        let by_id = data.by_id(&["a".into(), "b".into(), "c".into()]);
        assert_eq!(by_id.len(), 2);
        assert!(by_id["a"].reactions.viewer_reacted(ReactionKind::Rocket));
        assert!(
            by_id["c"].reactions.is_empty(),
            "a node that is not reactable has none"
        );
        assert!(!by_id.contains_key("b"));
    }

    #[test]
    fn whether_the_reader_may_delete_a_node_is_read_beside_its_reactions() {
        assert!(reactions_query().contains("... on Deletable { viewerCanDelete }"));
        let data: ReactionsData = serde_json::from_str(
            r#"{"nodes":[{"reactionGroups":[],"viewerCanDelete":true},{"reactionGroups":[],"viewerCanDelete":false},{"reactionGroups":[]}]}"#,
        )
        .unwrap();
        let by_id = data.by_id(&["mine".into(), "theirs".into(), "review".into()]);
        assert!(by_id["mine"].viewer_can_delete);
        assert!(!by_id["theirs"].viewer_can_delete);
        assert!(
            !by_id["review"].viewer_can_delete,
            "a node GitHub does not call deletable is not"
        );
    }

    #[test]
    fn whether_the_reader_may_edit_a_node_is_read_beside_its_reactions() {
        assert!(reactions_query().contains("... on Updatable { viewerCanUpdate }"));
        let data: ReactionsData = serde_json::from_str(
            r#"{"nodes":[{"reactionGroups":[],"viewerCanDelete":true,"viewerCanUpdate":true},{"reactionGroups":[],"viewerCanDelete":true},{"reactionGroups":[]}]}"#,
        )
        .unwrap();
        let by_id = data.by_id(&["mine".into(), "theirs".into(), "review".into()]);
        assert!(by_id["mine"].viewer_can_update);
        assert!(
            !by_id["theirs"].viewer_can_update,
            "a maintainer may delete what they may not edit"
        );
        assert!(!by_id["review"].viewer_can_update);
    }

    #[test]
    fn the_first_reactors_are_named_by_login() {
        let data: ReactionsData = serde_json::from_str(
            r#"{"nodes":[{"reactionGroups":[{"content":"HEART","viewerHasReacted":false,"reactors":{"totalCount":12,"nodes":[{"login":"octocat"},null,{},{"login":"dependabot"}]}}]}]}"#,
        )
        .unwrap();
        let by_id = data.by_id(&["a".into()]);
        let heart = &by_id["a"].reactions.groups[0];
        assert_eq!(heart.count, 12);
        assert_eq!(heart.reactors, vec!["octocat", "dependabot"]);

        // A group an older build kept has no names.
        let kept: ReactionGroup =
            serde_json::from_str(r#"{"kind":"heart","count":1,"viewer_reacted":false}"#).unwrap();
        assert!(kept.reactors.is_empty());
    }

    #[test]
    fn either_mutation_answers_with_the_subjects_reactions() {
        let added: ReactionMutationData = serde_json::from_str(
            r#"{"addReaction":{"subject":{"reactionGroups":[{"content":"HOORAY","viewerHasReacted":true,"reactors":{"totalCount":2}}]}}}"#,
        )
        .unwrap();
        assert_eq!(
            added.into_reactions().unwrap().count(ReactionKind::Hooray),
            2
        );
        let removed: ReactionMutationData =
            serde_json::from_str(r#"{"removeReaction":{"subject":{"reactionGroups":[]}}}"#)
                .unwrap();
        assert!(removed.into_reactions().unwrap().is_empty());
        let nothing: ReactionMutationData =
            serde_json::from_str(r#"{"addReaction":null}"#).unwrap();
        assert!(nothing.into_reactions().is_none());
    }

    #[test]
    fn kinds_serialize_by_name_and_round_trip_through_graphql() {
        assert_eq!(
            serde_json::to_string(&ReactionKind::ThumbsUp).unwrap(),
            r#""thumbs_up""#
        );
        for kind in ReactionKind::ALL {
            assert_eq!(ReactionKind::from_graphql(kind.graphql()), Some(kind));
            assert!(!kind.emoji().is_empty());
            assert!(!kind.label().is_empty());
        }
        assert_eq!(ReactionKind::from_graphql("SHRUG"), None);
    }

    #[test]
    fn the_queries_ask_for_the_groups_on_a_reactable() {
        assert!(reactions_query().contains("nodes(ids: $ids)"));
        assert!(reactions_query().contains("... on Reactable"));
        assert!(reactions_query()
            .contains("reactors(first: 10) { totalCount nodes { ... on Actor { login } } }"));
        assert!(reaction_mutation(false).contains("reactors(first: 10)"));
        assert!(reaction_mutation(true).starts_with("mutation("));
        assert!(reaction_mutation(true)
            .contains("addReaction(input: {subjectId: $subject, content: $content})"));
        assert!(reaction_mutation(false).contains("removeReaction(input:"));
    }
}
