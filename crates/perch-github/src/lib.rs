//! GitHub notifications: model, REST client, and local cache.
//!
//! This crate has no UI dependencies and never will. Data and IO stay
//! separable from the Floem layer so both can be tested on their own.

pub mod auth;
pub mod cache;
pub mod client;
pub mod comments;
pub mod config;
pub mod discussion;
pub mod items;
pub mod model;
pub mod pulls;
pub mod workflow_runs;

pub use auth::{resolve_token, Token, TokenSource};
pub use cache::{
    Cache, CacheError, CacheOpen, RunEventAppend, StoredReveals,
    NOTIFICATIONS_LAST_MODIFIED_KEY,
};
pub use comments::{
    Anchor, Author, CommentMessage, CommentThread, NewRunEvent, RevisionContext, RunEvent,
    RunEventRecord, RunHistoryCursor, RunHistoryFilter, RunLabels, RunOwner, RunPurpose, RunRecord,
    RunStatus, RunStep, RunUsage, Side, TerminalReason, ThreadOrigin,
};
pub use client::{Error, GitHub, ListResult, PullLookup};
pub use config::{
    is_local, AgentConfig, CodexConfig, Config, DeclaredRepo, RepoConfig, CLAUDE_CODE, CODEX,
    LOCAL_OWNER,
};
pub use discussion::{Discussion, IssueComment, PullEvent, PullEventKind, Review, ReviewComment};
pub use items::{
    Checkout as ItemCheckout, Issue, Item, Label, Subject as ItemSubject, SubjectKind as ItemKind,
};
pub use model::{Reason, Repository, Subject, SubjectKind, SubjectState, Thread};
pub use pulls::{CheckStatus, PullRequest, ReviewDecision, Reviewer};
pub use workflow_runs::{worst, CheckState, Checks, WorkflowRunPayload, WorkflowRunQuery};
