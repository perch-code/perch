//! Durable review-domain records.
//!
//! A review run and a finding are deliberately different identities. Groups
//! and attempts describe work which ran against one immutable snapshot;
//! findings and their append-only observations survive repeated reviews.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::comments::Side;

macro_rules! storage_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $value:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }

            pub(crate) fn parse(value: &str) -> Option<Self> {
                match value {
                    $($value => Some(Self::$variant)),+,
                    _ => None,
                }
            }
        }
    };
}

storage_enum! {
    /// Aggregate lifecycle for one snapshot review.
    pub enum ReviewGroupStatus {
        Preparing => "preparing",
        Reviewing => "reviewing",
        Stopping => "stopping",
        Reconciling => "reconciling",
        Empty => "empty",
        Completed => "completed",
        Failed => "failed",
        Stopped => "stopped",
        Interrupted => "interrupted",
    }
}

impl ReviewGroupStatus {
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::Reviewing | Self::Stopping | Self::Reconciling
        )
    }

    pub fn is_terminal(self) -> bool {
        !self.is_active()
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        match self {
            Self::Preparing => matches!(
                next,
                Self::Reviewing | Self::Stopping | Self::Failed | Self::Stopped | Self::Interrupted
            ),
            Self::Reviewing => matches!(
                next,
                Self::Stopping
                    | Self::Reconciling
                    | Self::Completed
                    | Self::Failed
                    | Self::Stopped
                    | Self::Interrupted
            ),
            Self::Stopping => matches!(
                next,
                Self::Completed | Self::Failed | Self::Stopped | Self::Interrupted
            ),
            Self::Reconciling => matches!(
                next,
                Self::Stopping | Self::Completed | Self::Failed | Self::Stopped | Self::Interrupted
            ),
            Self::Empty | Self::Completed | Self::Failed | Self::Stopped | Self::Interrupted => {
                false
            }
        }
    }
}

storage_enum! {
    /// Review-specific work within a reviewer attempt. This is intentionally
    /// separate from the owning Milestone 12 process state.
    pub enum ReviewerPhase {
        Queued => "queued",
        Preparing => "preparing",
        Reviewing => "reviewing",
        Extracting => "extracting",
        Verifying => "verifying",
        Reconciling => "reconciling",
        Stopping => "stopping",
        Completed => "completed",
        Failed => "failed",
        Stopped => "stopped",
        Interrupted => "interrupted",
    }
}

impl ReviewerPhase {
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Queued
                | Self::Preparing
                | Self::Reviewing
                | Self::Extracting
                | Self::Verifying
                | Self::Reconciling
                | Self::Stopping
        )
    }

    pub fn is_terminal(self) -> bool {
        !self.is_active()
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        if self.is_terminal() {
            return false;
        }
        match next {
            Self::Failed | Self::Stopped | Self::Interrupted => true,
            Self::Stopping => !matches!(self, Self::Stopping),
            Self::Completed => matches!(self, Self::Reviewing | Self::Reconciling),
            Self::Preparing => self == Self::Queued,
            Self::Reviewing => matches!(self, Self::Queued | Self::Preparing),
            Self::Extracting => self == Self::Reviewing,
            Self::Verifying => matches!(self, Self::Reviewing | Self::Extracting),
            Self::Reconciling => {
                matches!(self, Self::Reviewing | Self::Extracting | Self::Verifying)
            }
            Self::Queued => false,
        }
    }
}

storage_enum! {
    pub enum ReviewIngestionState {
        Pending => "pending",
        RawSaved => "raw_saved",
        Complete => "complete",
        Incomplete => "incomplete",
        Failed => "failed",
    }
}

storage_enum! {
    /// A clean result is distinct from an incomplete parse with no accepted
    /// findings.
    pub enum ReviewResultKind {
        Findings => "findings",
        Clean => "clean",
        Incomplete => "incomplete",
    }
}

storage_enum! {
    pub enum FindingDisposition {
        Open => "open",
        VerifiedFixed => "verified_fixed",
        Dismissed => "dismissed",
    }
}

storage_enum! {
    pub enum FindingVerification {
        Unverified => "unverified",
        Present => "present",
        Fixed => "fixed",
        NotVerified => "not_verified",
        Disputed => "disputed",
    }
}

storage_enum! {
    pub enum ObservationKind {
        Reported => "reported",
        VerifiedPresent => "verified_present",
        VerifiedFixed => "verified_fixed",
        NotVerified => "not_verified",
        Disagreement => "disagreement",
    }
}

storage_enum! {
    pub enum FindingSeverity {
        Unknown => "unknown",
        Note => "note",
        Low => "low",
        Medium => "medium",
        High => "high",
        Critical => "critical",
    }
}

storage_enum! {
    pub enum AnchorMappingStatus {
        Mapped => "mapped",
        Unmapped => "unmapped",
    }
}

storage_enum! {
    pub enum AnchorMappingMethod {
        Exact => "exact",
        Diff => "diff",
        Rename => "rename",
        Symbol => "symbol",
        Semantic => "semantic",
        Manual => "manual",
        Unavailable => "unavailable",
    }
}

storage_enum! {
    pub enum FindingAuditAction {
        Dismissed => "dismissed",
        Reopened => "reopened",
        VerifiedFixed => "verified_fixed",
        NotVerified => "not_verified",
        Recurrence => "recurrence",
        PossibleDuplicateSet => "possible_duplicate_set",
        PossibleDuplicateCleared => "possible_duplicate_cleared",
        DuplicateLinked => "duplicate_linked",
        DuplicateUnlinked => "duplicate_unlinked",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewSnapshot {
    pub id: String,
    pub item_id: String,
    pub repository: String,
    pub branch_label: Option<String>,
    pub requested_base_ref: String,
    pub base_tip_sha: String,
    pub merge_base_sha: String,
    pub head_sha: String,
    pub dirty_edits_excluded: bool,
    /// Shared retained-object identity used to release storage only after its
    /// final snapshot reference is removed.
    pub retention_key: String,
    pub retained_ref: Option<String>,
    pub manifest_path: Option<String>,
    pub diff_path: Option<String>,
    pub created_at: Timestamp,
    /// For a review of uncommitted work, the `HEAD` it was captured over;
    /// `head_sha` is then a commit Perch made of the work, found only in
    /// the retained snapshot.
    pub worktree_parent_sha: Option<String>,
}

impl ReviewSnapshot {
    /// What a review's title names as what it read: the head's short sha,
    /// or for uncommitted work the commit it sits on and that it is more.
    pub fn head_label(&self) -> String {
        match self.worktree_parent_sha.as_deref() {
            Some(parent) => format!("{} + uncommitted", short(parent)),
            None => short(&self.head_sha).to_string(),
        }
    }
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewGroup {
    pub id: String,
    pub item_id: String,
    pub snapshot_id: String,
    pub number: u64,
    pub status: ReviewGroupStatus,
    pub new_findings: u64,
    pub still_present: u64,
    pub verified_fixed: u64,
    pub not_verified: u64,
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    /// What the requester asked the reviewers to focus on, kept so a retry
    /// reviews the same thing.
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewerAttempt {
    pub id: String,
    pub group_id: String,
    pub run_id: Option<String>,
    pub backend: String,
    pub executable: Option<String>,
    pub executable_version: Option<String>,
    pub model: String,
    pub effort: String,
    pub adapter_version: String,
    pub output_format_version: String,
    pub attempt_number: u64,
    pub retry_of: Option<String>,
    pub phase: ReviewerPhase,
    pub ingestion_state: ReviewIngestionState,
    pub result_kind: Option<ReviewResultKind>,
    pub output_id: Option<String>,
    pub raw_result: Option<String>,
    pub diagnostics: Option<String>,
    /// The exact bounded set supplied to the separate prior-finding
    /// verifier. Recovery must replay this set, never a fresh database query.
    pub verification_input: ReviewVerificationInput,
    pub output_saved_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
}

pub const MAX_REVIEW_VERIFICATION_FINDINGS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReviewVerificationInput {
    pub finding_ids: Vec<String>,
    /// More prior findings existed, but were deliberately outside the
    /// bounded verifier input. They must not gain verification observations.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub id: String,
    pub item_id: String,
    /// A reconciler-issued semantic identity. It is not a line/hash
    /// fingerprint and must only be reused after a validated association.
    pub stable_key: String,
    pub thread_id: String,
    pub created_group_id: Option<String>,
    pub disposition: FindingDisposition,
    pub verification: FindingVerification,
    pub dismissal_reason: Option<String>,
    pub possible_duplicate_of: Option<String>,
    pub duplicate_of: Option<String>,
    pub last_verified_snapshot_id: Option<String>,
    pub last_verified_head_sha: Option<String>,
    pub last_checked_snapshot_id: Option<String>,
    pub last_checked_head_sha: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// A location is bound to immutable snapshot/blob identity. Numeric lines
/// alone are never sufficient to project a historical finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewAnchor {
    pub snapshot_id: String,
    pub path: String,
    pub side: Side,
    pub start_line: u32,
    pub end_line: u32,
    pub commit_sha: String,
    pub blob_sha: String,
}

impl ReviewAnchor {
    pub fn is_valid(&self) -> bool {
        !self.snapshot_id.is_empty()
            && !self.path.is_empty()
            && self.start_line > 0
            && self.end_line >= self.start_line
            && !self.commit_sha.is_empty()
            && !self.blob_sha.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewEvidenceSpan {
    /// Byte offsets in the preserved native result or extraction input.
    pub start: u64,
    pub end: u64,
    pub excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReviewEvidence {
    pub spans: Vec<ReviewEvidenceSpan>,
    pub symbol: Option<String>,
    /// Candidate narrowing only; this is never semantic identity by itself.
    pub fingerprint: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub id: String,
    pub finding_id: String,
    pub attempt_id: String,
    pub snapshot_id: String,
    pub source_output_id: String,
    pub kind: ObservationKind,
    pub title: String,
    pub body: String,
    pub severity: FindingSeverity,
    pub anchor: Option<ReviewAnchor>,
    pub evidence: ReviewEvidence,
    pub message_id: Option<String>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnchorMapping {
    pub id: String,
    pub finding_id: String,
    pub source_snapshot_id: String,
    pub target_snapshot_id: String,
    pub status: AnchorMappingStatus,
    pub method: AnchorMappingMethod,
    pub confidence: Option<f64>,
    pub source_anchor: ReviewAnchor,
    pub target_anchor: Option<ReviewAnchor>,
    pub evidence: ReviewEvidence,
    pub created_by_attempt_id: Option<String>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingAuditEntry {
    pub id: String,
    pub finding_id: String,
    pub action: FindingAuditAction,
    pub from_disposition: FindingDisposition,
    pub to_disposition: FindingDisposition,
    pub related_finding_id: Option<String>,
    pub snapshot_id: Option<String>,
    pub reason: Option<String>,
    pub evidence: ReviewEvidence,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPreference {
    pub backend: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub enabled: bool,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewReviewerAttempt {
    pub id: String,
    pub run_id: Option<String>,
    pub backend: String,
    pub executable: Option<String>,
    pub executable_version: Option<String>,
    pub model: String,
    pub effort: String,
    pub adapter_version: String,
    pub output_format_version: String,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewAdmission {
    pub group_id: String,
    pub snapshot: ReviewSnapshot,
    pub attempts: Vec<NewReviewerAttempt>,
    pub admitted_at: Timestamp,
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredReviewerOutput {
    pub attempt_id: String,
    pub output_id: String,
    pub raw_result: String,
    pub diagnostics: Option<String>,
    pub verification_input: ReviewVerificationInput,
    pub saved_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewerOutputSave {
    Stored,
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewThreadMessage {
    pub id: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFinding {
    pub id: String,
    pub stable_key: String,
    pub thread_id: String,
    pub initial_message: ReviewThreadMessage,
    /// The initial message is the reader's draft for the pull request, on
    /// the finding's lines, rather than the reviewer's message to them.
    /// It posts with their review, and deleting it dismisses the finding.
    pub draft: bool,
    pub possible_duplicate_of: Option<String>,
    pub possible_duplicate_evidence: ReviewEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindingTarget {
    Existing { finding_id: String },
    New(Box<NewFinding>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewObservation {
    pub id: String,
    pub source_output_id: String,
    pub kind: ObservationKind,
    pub title: String,
    pub body: String,
    pub severity: FindingSeverity,
    pub anchor: Option<ReviewAnchor>,
    pub evidence: ReviewEvidence,
    /// A materially different explanation to append to an existing finding
    /// thread. The initial message for a new finding lives on `NewFinding`.
    pub additional_message: Option<ReviewThreadMessage>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciledObservation {
    pub finding: FindingTarget,
    pub observation: NewObservation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReviewIngestion {
    pub attempt_id: String,
    pub output_id: String,
    pub result_kind: ReviewResultKind,
    pub observations: Vec<ReconciledObservation>,
    /// Snapshot-to-snapshot projections produced while reconciling this
    /// output. They commit with the observations they informed.
    pub anchor_mappings: Vec<AnchorMapping>,
    pub completed_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewIngestionResult {
    pub duplicate: bool,
    pub finding_ids: Vec<String>,
    pub observation_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedReviewHistory {
    pub snapshots: Vec<ReviewSnapshot>,
    pub thread_ids: Vec<String>,
    pub run_ids: Vec<String>,
}
