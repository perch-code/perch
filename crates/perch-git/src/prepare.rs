//! Turning a pull request into a checkout on disk.

use std::path::PathBuf;
use std::time::Duration;

use crate::cmd::{Git, Progress};
use crate::store::{RepoId, Store};
use crate::Error;

const CLONE_TIMEOUT: Duration = Duration::from_secs(600);
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
const WORKTREE_TIMEOUT: Duration = Duration::from_secs(30);
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

/// What the caller is told while a checkout is being prepared.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Cloning { phase: String, percent: Option<u8> },
    Fetching,
    CreatingWorktree,
    ComputingDiff,
}

/// A prepared pull request checkout.
#[derive(Debug, Clone, PartialEq)]
pub struct Checkout {
    pub worktree: PathBuf,
    /// The bare clone the worktree belongs to. Blob reads go here.
    pub bare: PathBuf,
    pub head_sha: String,
    /// The commit the diff is taken against.
    ///
    /// This is the merge base of the base branch and the head, not the base
    /// branch tip. It is what GitHub's Files changed tab shows, and it keeps
    /// commits that landed on the base branch after the pull request opened
    /// out of the diff.
    pub merge_base: String,
}

pub struct Preparer {
    git: Git,
    store: Store,
}

impl Preparer {
    pub fn new(store: Store, token: Option<String>) -> Self {
        Self { git: Git::new(token), store }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Clone if needed, fetch the pull request, and lay down a worktree.
    pub async fn prepare(
        &self,
        id: &RepoId,
        number: u64,
        head_sha: &str,
        base_ref: &str,
        mut on_step: impl FnMut(Step) + Send,
    ) -> Result<Checkout, Error> {
        let bare = self.store.bare_path(id);

        if !self.store.has_bare_clone(id) {
            if let Some(parent) = bare.parent() {
                std::fs::create_dir_all(parent).map_err(|e| Error::Io(e.to_string()))?;
            }
            // A partial clone would be gone if it failed halfway, so build it
            // under a temporary name and move it into place only on success.
            let staging = bare.with_extension("git.partial");
            let _ = std::fs::remove_dir_all(&staging);

            self.git
                .run_with_progress(
                    None,
                    [
                        "clone".to_string(),
                        "--bare".to_string(),
                        "--progress".to_string(),
                        id.clone_url(),
                        staging.to_string_lossy().into_owned(),
                    ],
                    CLONE_TIMEOUT,
                    |p: Progress| {
                        on_step(Step::Cloning { phase: p.phase, percent: p.percent })
                    },
                )
                .await?;

            std::fs::rename(&staging, &bare).map_err(|e| Error::Io(e.to_string()))?;
        }

        on_step(Step::Fetching);

        // The pull request head and the base branch, in a namespace of our
        // own so these refs can never collide with the user's.
        let head_ref = format!("refs/perch/pull/{number}/head");
        let base_local = format!("refs/perch/base/{number}");
        self.git
            .run(
                Some(&bare),
                [
                    "fetch".to_string(),
                    "--no-tags".to_string(),
                    "origin".to_string(),
                    format!("+refs/pull/{number}/head:{head_ref}"),
                    format!("+refs/heads/{base_ref}:{base_local}"),
                ],
                FETCH_TIMEOUT,
            )
            .await?;

        let merge_base = self
            .git
            .run(Some(&bare), ["merge-base", &base_local, head_sha], QUERY_TIMEOUT)
            .await?
            .trim()
            .to_string();

        if merge_base.is_empty() {
            return Err(Error::NoMergeBase);
        }

        on_step(Step::CreatingWorktree);
        let worktree = self.store.worktree_path(id, head_sha);

        if !worktree.join(".git").exists() {
            std::fs::create_dir_all(self.store.worktrees_dir())
                .map_err(|e| Error::Io(e.to_string()))?;
            // A directory left behind by an interrupted run would make
            // `worktree add` fail; clear it and let git recreate it.
            let _ = std::fs::remove_dir_all(&worktree);
            self.git
                .run(
                    Some(&bare),
                    [
                        "worktree".to_string(),
                        "add".to_string(),
                        "--detach".to_string(),
                        "--force".to_string(),
                        worktree.to_string_lossy().into_owned(),
                        head_sha.to_string(),
                    ],
                    WORKTREE_TIMEOUT,
                )
                .await?;
        }

        Ok(Checkout {
            worktree,
            bare,
            head_sha: head_sha.to_string(),
            merge_base,
        })
    }

    /// Read a blob at a commit. Used for the base side of a diff and for
    /// files the pull request deleted.
    pub async fn show_blob(
        &self,
        bare: &std::path::Path,
        commit: &str,
        path: &str,
    ) -> Result<String, Error> {
        self.git
            .run(Some(bare), ["show", &format!("{commit}:{path}")], QUERY_TIMEOUT)
            .await
    }

    pub fn git(&self) -> &Git {
        &self.git
    }

    /// Remove worktrees untouched for `max_age`, and prune git's records.
    ///
    /// Worktrees survive leaving a pull request so that going back is
    /// instant; this is what stops them accumulating forever.
    pub async fn sweep_worktrees(&self, max_age: Duration) -> usize {
        let dir = self.store.worktrees_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return 0;
        };

        let mut removed = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let stale = entry
                .metadata()
                .and_then(|m| m.accessed().or_else(|_| m.modified()))
                .map(|t| t.elapsed().map(|e| e > max_age).unwrap_or(false))
                .unwrap_or(false);
            if stale && std::fs::remove_dir_all(&path).is_ok() {
                removed += 1;
            }
        }

        if removed > 0 {
            // git still lists the directories it no longer has; prune them so
            // a later `worktree add` at the same path succeeds.
            if let Ok(repos) = std::fs::read_dir(self.store.root().join("repos/github.com")) {
                for owner in repos.flatten() {
                    if let Ok(bares) = std::fs::read_dir(owner.path()) {
                        for bare in bares.flatten() {
                            let _ = self
                                .git
                                .run(Some(&bare.path()), ["worktree", "prune"], WORKTREE_TIMEOUT)
                                .await;
                        }
                    }
                }
            }
        }

        removed
    }
}
