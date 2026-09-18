//! GitHub notifications: model, REST client, and local cache.
//!
//! This crate has no UI dependencies and never will. Data and IO stay
//! separable from the Floem layer so both can be tested on their own.

pub mod auth;
pub mod cache;
pub mod client;
pub mod config;
pub mod discussion;
pub mod model;
pub mod pulls;

pub use auth::{resolve_token, Token, TokenSource};
pub use cache::{Cache, CacheError, Mark, Worktree};
pub use client::{Error, GitHub, ListResult};
pub use config::Config;
pub use discussion::{Discussion, IssueComment, Review, ReviewComment};
pub use pulls::PullRequest;
pub use model::{
    Reason, Repository, Subject, SubjectDetail, SubjectKind, SubjectState, Thread,
};
