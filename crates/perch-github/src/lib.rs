//! GitHub notifications: model, REST client, and local cache.
//!
//! This crate has no UI dependencies and never will. Data and IO stay
//! separable from the Floem layer so both can be tested on their own.

pub mod activity;
pub mod assignees;
pub mod auth;
pub mod cache;
pub mod client;
pub mod comments;
pub mod commits;
pub mod config;
pub mod discussion;
pub mod items;
pub mod mentions;
pub mod model;
pub mod pulls;
pub mod reactions;
pub mod references;
pub mod releases;
mod responses;
pub mod reviews;
pub mod search;
mod subjects;
pub mod subscription;
pub mod timeline;
pub mod workflow_runs;

pub use activity::{ActivityEvent, ActivityOutcome, ItemActivity};
pub use assignees::{Assigned, MAX_ASSIGNEES};
pub use auth::{resolve_token, Token, TokenSource};
pub use cache::{
    Cache, CacheError, CacheOpen, DraftRow, Reconciled, RunContext, RunEventAppend, StoredReveals,
    WorkGrantRecord, NOTIFICATIONS_LAST_MODIFIED_KEY,
};
pub use client::{CheckProbe, CheckTags, EditedComment, Error, GitHub, ListResult, PullLookup};
pub use comments::{
    Anchor, Author, CommentMessage, CommentThread, NewRunEvent, RevisionContext, RunEvent,
    RunEventRecord, RunHistoryCursor, RunHistoryFilter, RunHistoryPage, RunLabels, RunMode,
    RunOwner, RunPurpose, RunRecord, RunStatus, RunStep, RunSummary, RunUsage, Side,
    TerminalReason, ThreadOrigin,
};
pub use commits::Unreadable;
pub use config::{
    append_repo, is_local, remove_repo, AgentConfig, Approvals, CodexConfig, Config, DeclaredRepo,
    RepoConfig, RepoLsp, RepoWork, WorkDefaults, CLAUDE_CODE, CODEX, CODEX_DEFAULT_MODEL,
    LOCAL_OWNER,
};
pub use discussion::{
    CommentPlace, Discussion, IssueComment, PullReviewEvent, Review, ReviewComment,
    ReviewCommentDraft, ReviewThreadState,
};
pub use items::{
    Checkout as ItemCheckout, Diverged, Divergence, Issue, IssueCloseReason, IssueRights, Item,
    Label, Subject as ItemSubject, SubjectKind as ItemKind,
};
pub use mentions::MentionableUser;
pub use model::{
    is_app_avatar, Participant, Reason, Repository, RunSnapshot, Subject, SubjectKind,
    SubjectSnapshot, SubjectState, Thread,
};
pub use pulls::{
    CheckStatus, CommitMessage, MergeMethod, MergeMethods, MergeResult, MergeState, MergeableState,
    PullRequest, PullRequestOverview, RepositoryPermission, ReviewDecision, Reviewer,
};
pub use reactions::{ReactionGroup, ReactionKind, Reactions};
pub use references::IssueReference;
pub use releases::{Release, ReleaseAsset};
pub use reviews::{
    AnchorMapping, AnchorMappingMethod, AnchorMappingStatus, DeletedReviewHistory, Finding,
    FindingAuditAction, FindingAuditEntry, FindingDisposition, FindingSeverity, FindingTarget,
    FindingVerification, NewFinding, NewObservation, NewReviewerAttempt, Observation,
    ObservationKind, ReconciledObservation, ReviewAdmission, ReviewAnchor, ReviewEvidence,
    ReviewEvidenceSpan, ReviewGroup, ReviewGroupStatus, ReviewIngestion, ReviewIngestionResult,
    ReviewIngestionState, ReviewPreference, ReviewResultKind, ReviewSnapshot, ReviewThreadMessage,
    ReviewVerificationInput, ReviewerAttempt, ReviewerOutputSave, ReviewerPhase,
    StoredReviewerOutput, MAX_REVIEW_VERIFICATION_FINDINGS,
};
pub use search::{SearchHit, SearchPage};
pub use subscription::IssueViewerState;
pub use timeline::{PullEvent, PullEventKind, Reference};
pub use workflow_runs::{
    announced_verdict, Check, CheckKind, CheckRunState, CheckState, Checks, WorkflowRunPayload,
    WorkflowRunQuery,
};
