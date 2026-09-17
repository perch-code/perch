//! Cloning, checking out, and diffing pull requests.
//!
//! No UI dependencies. Network and store-mutating git operations shell out to
//! the `git` binary; everything else is in-process.

pub mod cmd;
pub mod diff;
pub mod display;
pub mod prepare;
pub mod store;
pub mod tree;

pub use diff::{
    file_diff, ChangeStatus, ChangedFile, DeletedRun, FileDiff, Hunk, LineStatus,
};
pub use display::{DisplayBuffer, Row};
pub use prepare::{Checkout, Preparer, Step};
pub use store::{RepoId, Store};
pub use tree::{list_tree, TreeEntry};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("git exited with {status}: {stderr}")]
    Git { status: i32, stderr: String },
    #[error("could not run git: {0}")]
    Spawn(String),
    #[error("git took too long")]
    Timeout,
    #[error("filesystem error: {0}")]
    Io(String),
    #[error("the pull request and its base branch share no history")]
    NoMergeBase,
}
