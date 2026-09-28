//! Cloning, checking out, and diffing pull requests.
//!
//! No UI dependencies. Network and store-mutating git operations shell out to
//! the `git` binary; everything else is in-process.

pub mod cmd;
pub mod diff;
pub mod display;
pub mod prepare;
pub mod review_snapshot;
pub mod scan;
pub mod scope;
pub mod store;
pub mod tree;

pub use diff::{file_diff, hunk_context, ChangeStatus, ChangedFile, DeletedRun, FileDiff, Hunk, LineStatus};
pub use display::{DisplayBuffer, Row};
pub use prepare::{home_ref, BaseBranch, Checkout, Cut, Preparer, PushError, Step};
pub use review_snapshot::{
    map_anchor_between, AnchorMapping, CaptureOutcome, DiffSide, MappedAnchor, MappingCandidate,
    MappingConfidence, MappingDocument, MappingEvidence, MappingMethod, MappingSnapshot, RangeDiff,
    ResolvedReviewRange, ReviewSnapshot, ReviewSnapshotStore, SnapshotAnchor, SnapshotCapture,
    SnapshotError, SnapshotFile, SnapshotManifest, UnmappedReason, BASE_TIP_REF, HEAD_REF,
    MERGE_BASE_REF,
};
pub use scan::{
    branch_churn, branch_readiness, github_remote, head_commit_time, parse_remotes, parse_worktree_list, remotes,
    repo_from_remote, scan_root, BranchReadiness, RootScan, WorktreeEntry,
};
pub use scope::{alignment, changed_files, dirt, log, read_side, Commit, Dirt, Rev, Scope};
pub use store::{RepoId, Store};
pub use tree::{compare_entries, compare_paths, list_tree, TreeEntry};

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
    #[error("unexpected git output: {0}")]
    Parse(String),
    #[error(
        "no base branch for {}: no remote HEAD, no base in config.toml, and neither main nor master",
        root.display()
    )]
    NoBase { root: std::path::PathBuf },
}
