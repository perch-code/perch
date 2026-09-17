//! Data model mirroring the GitHub notifications REST API.
//!
//! Enums deserialize unknown strings into an `Other` variant rather than
//! failing. GitHub adds notification reasons and subject types without
//! notice, and a new one must never take down the inbox.

use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize};

/// A notification thread, as returned by `GET /notifications`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub unread: bool,
    pub reason: Reason,
    #[serde(with = "ts")]
    pub updated_at: Timestamp,
    #[serde(default, with = "ts_opt")]
    pub last_read_at: Option<Timestamp>,
    pub subject: Subject,
    pub repository: Repository,
}

impl Thread {
    /// The issue or PR number, parsed from the tail of the subject API URL.
    ///
    /// Lets a row render `#2097` on first paint, before the per-thread detail
    /// request lands.
    pub fn number_hint(&self) -> Option<u64> {
        let url = self.subject.url.as_deref()?;
        url.rsplit('/').next()?.parse().ok()
    }

    /// True when this thread's subject supports a detail fetch.
    pub fn has_detail(&self) -> bool {
        matches!(
            self.subject.kind,
            SubjectKind::Issue | SubjectKind::PullRequest
        )
    }
}

/// Why this thread is in the inbox.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Reason {
    Assign,
    Author,
    Comment,
    CiActivity,
    Invitation,
    Manual,
    Mention,
    ReviewRequested,
    SecurityAlert,
    StateChange,
    Subscribed,
    TeamMention,
    Other(String),
}

impl Reason {
    /// The string GitHub uses on the wire. Serializing this rather than the
    /// variant name keeps a cached thread lossless across a round trip.
    pub fn wire(&self) -> &str {
        match self {
            Reason::Assign => "assign",
            Reason::Author => "author",
            Reason::Comment => "comment",
            Reason::CiActivity => "ci_activity",
            Reason::Invitation => "invitation",
            Reason::Manual => "manual",
            Reason::Mention => "mention",
            Reason::ReviewRequested => "review_requested",
            Reason::SecurityAlert => "security_alert",
            Reason::StateChange => "state_change",
            Reason::Subscribed => "subscribed",
            Reason::TeamMention => "team_mention",
            Reason::Other(s) => s,
        }
    }

    /// The wording GitHub's web UI shows in the right-hand badge.
    pub fn label(&self) -> &str {
        match self {
            Reason::Assign => "assigned",
            Reason::Author => "author",
            Reason::Comment => "commented",
            Reason::CiActivity => "ci activity",
            Reason::Invitation => "invited",
            Reason::Manual => "subscribed",
            Reason::Mention => "mention",
            Reason::ReviewRequested => "review requested",
            Reason::SecurityAlert => "security alert",
            Reason::StateChange => "state change",
            Reason::Subscribed => "subscribed",
            Reason::TeamMention => "team mentioned",
            Reason::Other(s) => s,
        }
    }

    /// Reasons the web UI's "Participating" filter includes.
    pub fn is_participating(&self) -> bool {
        matches!(
            self,
            Reason::Author
                | Reason::Comment
                | Reason::Mention
                | Reason::TeamMention
                | Reason::ReviewRequested
                | Reason::Assign
        )
    }
}

impl Serialize for Reason {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.wire())
    }
}

impl<'de> Deserialize<'de> for Reason {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "assign" => Reason::Assign,
            "author" => Reason::Author,
            "comment" => Reason::Comment,
            "ci_activity" => Reason::CiActivity,
            "invitation" => Reason::Invitation,
            "manual" => Reason::Manual,
            "mention" => Reason::Mention,
            "review_requested" => Reason::ReviewRequested,
            "security_alert" => Reason::SecurityAlert,
            "state_change" => Reason::StateChange,
            "subscribed" => Reason::Subscribed,
            "team_mention" => Reason::TeamMention,
            _ => Reason::Other(s),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subject {
    pub title: String,
    #[serde(rename = "type")]
    pub kind: SubjectKind,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub latest_comment_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SubjectKind {
    Issue,
    PullRequest,
    Discussion,
    Commit,
    Release,
    CheckSuite,
    RepositoryVulnerabilityAlert,
    Other(String),
}

impl SubjectKind {
    pub fn wire(&self) -> &str {
        match self {
            SubjectKind::Issue => "Issue",
            SubjectKind::PullRequest => "PullRequest",
            SubjectKind::Discussion => "Discussion",
            SubjectKind::Commit => "Commit",
            SubjectKind::Release => "Release",
            SubjectKind::CheckSuite => "CheckSuite",
            SubjectKind::RepositoryVulnerabilityAlert => "RepositoryVulnerabilityAlert",
            SubjectKind::Other(s) => s,
        }
    }
}

impl Serialize for SubjectKind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.wire())
    }
}

impl<'de> Deserialize<'de> for SubjectKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "Issue" => SubjectKind::Issue,
            "PullRequest" => SubjectKind::PullRequest,
            "Discussion" => SubjectKind::Discussion,
            "Commit" => SubjectKind::Commit,
            "Release" => SubjectKind::Release,
            "CheckSuite" => SubjectKind::CheckSuite,
            "RepositoryVulnerabilityAlert" => SubjectKind::RepositoryVulnerabilityAlert,
            _ => SubjectKind::Other(s),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Repository {
    pub full_name: String,
    /// Flattened out of the API's nested `owner` object.
    ///
    /// Serialized back under the same `owner` key as a bare string, and the
    /// deserializer accepts either shape, so a cached thread round-trips
    /// through SQLite without a second wire format.
    #[serde(rename = "owner", default, deserialize_with = "owner_avatar")]
    pub owner_avatar_url: String,
}

fn owner_avatar<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Shape {
        /// What we write to the cache.
        Flat(String),
        /// What GitHub sends.
        Nested {
            #[serde(default)]
            avatar_url: String,
        },
    }
    Ok(match Shape::deserialize(d)? {
        Shape::Flat(url) => url,
        Shape::Nested { avatar_url } => avatar_url,
    })
}

/// Per-thread detail, fetched from `subject.url`. Issues and PRs only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubjectDetail {
    pub number: u64,
    pub state: SubjectState,
    pub author_login: String,
    pub author_avatar_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubjectState {
    IssueOpen,
    IssueClosed,
    IssueNotPlanned,
    PrOpen,
    PrDraft,
    PrMerged,
    PrClosed,
}

/// The raw shape of an issue or PR payload, reduced to what a row needs.
#[derive(Debug, Deserialize)]
pub(crate) struct SubjectPayload {
    pub number: u64,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub state_reason: Option<String>,
    #[serde(default)]
    pub draft: Option<bool>,
    #[serde(default)]
    pub merged_at: Option<String>,
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
    #[serde(default)]
    pub user: Option<UserPayload>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UserPayload {
    #[serde(default)]
    pub login: String,
    #[serde(default)]
    pub avatar_url: String,
}

impl SubjectPayload {
    pub(crate) fn into_detail(self, kind: &SubjectKind) -> SubjectDetail {
        let is_pr = matches!(kind, SubjectKind::PullRequest) || self.pull_request.is_some();
        let closed = self.state.eq_ignore_ascii_case("closed");

        let state = if is_pr {
            if self.merged_at.is_some() {
                SubjectState::PrMerged
            } else if closed {
                SubjectState::PrClosed
            } else if self.draft.unwrap_or(false) {
                SubjectState::PrDraft
            } else {
                SubjectState::PrOpen
            }
        } else if closed {
            match self.state_reason.as_deref() {
                Some("not_planned") => SubjectState::IssueNotPlanned,
                _ => SubjectState::IssueClosed,
            }
        } else {
            SubjectState::IssueOpen
        };

        let (author_login, author_avatar_url) = self
            .user
            .map(|u| (u.login, u.avatar_url))
            .unwrap_or_default();

        SubjectDetail {
            number: self.number,
            state,
            author_login,
            author_avatar_url,
        }
    }
}

/// GitHub emits RFC 3339 with a `Z` suffix, which `jiff::Timestamp` parses.
mod ts {
    use jiff::Timestamp;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(t: &Timestamp, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&t.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Timestamp, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

mod ts_opt {
    use jiff::Timestamp;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(t: &Option<Timestamp>, s: S) -> Result<S::Ok, S::Error> {
        match t {
            Some(t) => s.serialize_str(&t.to_string()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Timestamp>, D::Error> {
        let s = Option::<String>::deserialize(d)?;
        match s {
            Some(s) => s.parse().map(Some).map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_reasons_map_to_variants() {
        let cases = [
            (r#""review_requested""#, Reason::ReviewRequested),
            (r#""team_mention""#, Reason::TeamMention),
            (r#""ci_activity""#, Reason::CiActivity),
            (r#""subscribed""#, Reason::Subscribed),
        ];
        for (json, want) in cases {
            assert_eq!(serde_json::from_str::<Reason>(json).unwrap(), want);
        }
    }

    #[test]
    fn unknown_reason_falls_back_to_other() {
        let r: Reason = serde_json::from_str(r#""brand_new_reason""#).unwrap();
        assert_eq!(r, Reason::Other("brand_new_reason".into()));
        assert_eq!(r.label(), "brand_new_reason");
    }

    #[test]
    fn unknown_subject_kind_falls_back_to_other() {
        let k: SubjectKind = serde_json::from_str(r#""Sponsorship""#).unwrap();
        assert_eq!(k, SubjectKind::Other("Sponsorship".into()));
    }

    #[test]
    fn participating_covers_the_web_ui_set() {
        assert!(Reason::Author.is_participating());
        assert!(Reason::ReviewRequested.is_participating());
        assert!(!Reason::Subscribed.is_participating());
        assert!(!Reason::CiActivity.is_participating());
    }

    #[test]
    fn number_is_parsed_from_subject_url() {
        let t = thread_with_url(Some("https://api.github.com/repos/slatedb/slatedb/issues/2097"));
        assert_eq!(t.number_hint(), Some(2097));
    }

    #[test]
    fn number_hint_is_none_without_a_url() {
        assert_eq!(thread_with_url(None).number_hint(), None);
    }

    #[test]
    fn full_thread_payload_deserializes() {
        let json = r#"{
            "id": "123",
            "unread": true,
            "reason": "mention",
            "updated_at": "2026-09-17T18:00:00Z",
            "last_read_at": null,
            "subject": {
                "title": "RFC 0035 - incremental compaction",
                "type": "PullRequest",
                "url": "https://api.github.com/repos/slatedb/slatedb/pulls/2091",
                "latest_comment_url": null
            },
            "repository": {
                "full_name": "slatedb/slatedb",
                "owner": { "avatar_url": "https://avatars.githubusercontent.com/u/1" }
            }
        }"#;
        let t: Thread = serde_json::from_str(json).unwrap();
        assert_eq!(t.reason, Reason::Mention);
        assert_eq!(t.subject.kind, SubjectKind::PullRequest);
        assert_eq!(t.number_hint(), Some(2091));
        assert_eq!(t.repository.owner_avatar_url, "https://avatars.githubusercontent.com/u/1");
        assert!(t.has_detail());
    }

    #[test]
    fn enums_round_trip_through_their_wire_format() {
        for reason in [
            Reason::ReviewRequested,
            Reason::CiActivity,
            Reason::Subscribed,
            Reason::Other("something_new".into()),
        ] {
            let json = serde_json::to_string(&reason).unwrap();
            assert_eq!(serde_json::from_str::<Reason>(&json).unwrap(), reason);
        }

        for kind in [
            SubjectKind::PullRequest,
            SubjectKind::Issue,
            SubjectKind::Other("Sponsorship".into()),
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(serde_json::from_str::<SubjectKind>(&json).unwrap(), kind);
        }
    }

    #[test]
    fn repository_avatar_round_trips_through_both_shapes() {
        // GitHub's shape.
        let from_api: Repository = serde_json::from_str(
            r#"{"full_name":"a/b","owner":{"avatar_url":"https://avatars/9"}}"#,
        )
        .unwrap();
        assert_eq!(from_api.owner_avatar_url, "https://avatars/9");

        // Our own shape, as written to the cache.
        let json = serde_json::to_string(&from_api).unwrap();
        let round_tripped: Repository = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped, from_api);
    }

    #[test]
    fn repository_without_an_owner_is_not_an_error() {
        let r: Repository = serde_json::from_str(r#"{"full_name":"a/b"}"#).unwrap();
        assert_eq!(r.owner_avatar_url, "");
    }

    #[test]
    fn pr_states_derive_from_flags() {
        let merged = payload(r#"{"number":1,"state":"closed","merged_at":"2026-01-01T00:00:00Z"}"#);
        assert_eq!(
            merged.into_detail(&SubjectKind::PullRequest).state,
            SubjectState::PrMerged
        );

        let draft = payload(r#"{"number":2,"state":"open","draft":true}"#);
        assert_eq!(
            draft.into_detail(&SubjectKind::PullRequest).state,
            SubjectState::PrDraft
        );

        let closed = payload(r#"{"number":3,"state":"closed"}"#);
        assert_eq!(
            closed.into_detail(&SubjectKind::PullRequest).state,
            SubjectState::PrClosed
        );
    }

    #[test]
    fn issue_not_planned_is_distinct_from_closed() {
        let np = payload(r#"{"number":4,"state":"closed","state_reason":"not_planned"}"#);
        assert_eq!(
            np.into_detail(&SubjectKind::Issue).state,
            SubjectState::IssueNotPlanned
        );

        let done = payload(r#"{"number":5,"state":"closed","state_reason":"completed"}"#);
        assert_eq!(
            done.into_detail(&SubjectKind::Issue).state,
            SubjectState::IssueClosed
        );
    }

    fn payload(json: &str) -> SubjectPayload {
        serde_json::from_str(json).unwrap()
    }

    fn thread_with_url(url: Option<&str>) -> Thread {
        Thread {
            id: "1".into(),
            unread: true,
            reason: Reason::Subscribed,
            updated_at: "2026-09-17T18:00:00Z".parse().unwrap(),
            last_read_at: None,
            subject: Subject {
                title: "t".into(),
                kind: SubjectKind::Issue,
                url: url.map(String::from),
                latest_comment_url: None,
            },
            repository: Repository {
                full_name: "a/b".into(),
                owner_avatar_url: String::new(),
            },
        }
    }
}
