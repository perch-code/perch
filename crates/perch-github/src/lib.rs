//! GitHub notifications: model, REST client, and local cache.
//!
//! This crate has no UI dependencies and never will. Data and IO stay
//! separable from the Floem layer so both can be tested on their own.

pub mod auth;
pub mod cache;
pub mod comments;
pub mod client;
pub mod config;
pub mod discussion;
pub mod model;
pub mod pulls;
pub mod workflow_runs;

pub use auth::{resolve_token, Token, TokenSource};
pub use cache::{Cache, CacheError, Mark, Worktree};
pub use comments::{
    Anchor, Author, CommentMessage, CommentThread, RunRecord, RunStatus, RunStep, Side,
    ThreadOrigin,
};
pub use client::{Error, GitHub, ListResult};
pub use config::{AgentConfig, Config, KnowledgeConfig};
pub use discussion::{Discussion, IssueComment, Review, ReviewComment};
pub use pulls::PullRequest;
pub use workflow_runs::WorkflowRunQuery;
pub use model::{
    DetailSource, Reason, Repository, Subject, SubjectDetail, SubjectKind, SubjectState, Thread,
};
