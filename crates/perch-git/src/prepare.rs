//! Turning a pull request into a checkout on disk.

use std::path::{Path, PathBuf};
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

/// A prepared checkout, on a branch Perch owns.
#[derive(Debug, Clone, PartialEq)]
pub struct Checkout {
    pub worktree: PathBuf,
    /// The bare clone the worktree belongs to. Blob reads go here.
    pub bare: PathBuf,
    /// The local branch checked out in the worktree, such as `perch/pr-7`.
    pub branch: String,
    /// The commit actually on disk. Normally the head that was asked for;
    /// when `stale` is set, the older head the worktree was kept at.
    pub head_sha: String,
    /// Set when the pull request moved to this head but the worktree had
    /// uncommitted changes, so it was left where it was rather than reset
    /// over the top of them.
    pub stale: Option<String>,
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

    /// Clone if needed, fetch the pull request, and put its head on
    /// `branch` in a worktree.
    ///
    /// The worktree is keyed by the branch, so opening the same pull request
    /// again finds the same directory. When its head has moved since, a
    /// clean worktree is reset to the new head; a dirty one is left alone
    /// and the checkout reports the head it did not take, so nothing
    /// anyone wrote in there is ever reset away.
    pub async fn prepare(
        &self,
        id: &RepoId,
        number: u64,
        head_sha: &str,
        base_ref: &str,
        branch: &str,
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
        let worktree = self.store.worktree_path(id, branch);

        if !worktree.join(".git").exists() {
            std::fs::create_dir_all(self.store.worktrees_dir())
                .map_err(|e| Error::Io(e.to_string()))?;
            // A directory left behind by an interrupted run would make
            // `worktree add` fail; clear it and let git recreate it. So
            // would a stale registration of a directory that is gone.
            let _ = std::fs::remove_dir_all(&worktree);
            let _ = self
                .git
                .run(Some(&bare), ["worktree", "prune"], WORKTREE_TIMEOUT)
                .await;

            let path = worktree.to_string_lossy().into_owned();
            if self.branch_exists(&bare, branch).await {
                // An archived worktree being restored: the branch is still
                // here, so check it out as it is. The head check below
                // brings it up to date.
                self.git
                    .run(Some(&bare), ["worktree", "add", &path, branch], WORKTREE_TIMEOUT)
                    .await?;
            } else {
                self.git
                    .run(
                        Some(&bare),
                        ["worktree", "add", "-B", branch, &path, head_sha],
                        WORKTREE_TIMEOUT,
                    )
                    .await?;
            }
        }

        // Whether the directory was just made or already here, make sure it
        // is at the head that was asked for, unless someone has work in it.
        let on_disk = self
            .git
            .run(Some(&worktree), ["rev-parse", "HEAD"], QUERY_TIMEOUT)
            .await?
            .trim()
            .to_string();

        let (head_sha, stale) = if on_disk == head_sha {
            (head_sha.to_string(), None)
        } else if self.is_dirty(&worktree).await? {
            (on_disk, Some(head_sha.to_string()))
        } else {
            // HEAD is attached to the branch, so this moves the branch too.
            self.git
                .run(Some(&worktree), ["reset", "--hard", head_sha], WORKTREE_TIMEOUT)
                .await?;
            (head_sha.to_string(), None)
        };

        Ok(Checkout {
            worktree,
            bare,
            branch: branch.to_string(),
            head_sha,
            merge_base,
            stale,
        })
    }

    async fn branch_exists(&self, bare: &Path, branch: &str) -> bool {
        self.git
            .run(
                Some(bare),
                ["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")],
                QUERY_TIMEOUT,
            )
            .await
            .is_ok()
    }

    /// Whether a worktree has uncommitted changes, untracked files included.
    ///
    /// This is the check that makes it safe for anything to write into a
    /// worktree: nothing here resets or removes one that answers yes.
    pub async fn is_dirty(&self, worktree: &Path) -> Result<bool, Error> {
        let status = self
            .git
            .run(Some(worktree), ["status", "--porcelain"], QUERY_TIMEOUT)
            .await?;
        Ok(!status.trim().is_empty())
    }

    /// Remove a worktree's directory and git's record of it, keeping the
    /// branch and the fetched refs so it can be restored without a clone.
    ///
    /// Deliberately not `git worktree remove --force`, which would happily
    /// delete uncommitted work. Callers check for dirt first; this only
    /// removes the directory and then tells git it is gone.
    pub async fn remove_worktree(&self, bare: &Path, worktree: &Path) -> Result<(), Error> {
        if worktree.exists() {
            std::fs::remove_dir_all(worktree).map_err(|e| Error::Io(e.to_string()))?;
        }
        self.git
            .run(Some(bare), ["worktree", "prune"], WORKTREE_TIMEOUT)
            .await?;
        Ok(())
    }

    /// Delete a branch Perch owns. Only for a worktree being forgotten,
    /// after its directory is already gone.
    pub async fn delete_branch(&self, bare: &Path, branch: &str) -> Result<(), Error> {
        self.git
            .run(Some(bare), ["branch", "-D", branch], WORKTREE_TIMEOUT)
            .await?;
        Ok(())
    }

    /// Worktree directories not modified for `max_age`.
    ///
    /// Reports and does nothing else. Only the caller holds the records
    /// that say which of these to archive and which are unknown to it.
    pub fn stale_worktrees(&self, max_age: Duration) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(self.store.worktrees_dir()) else {
            return Vec::new();
        };
        entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .filter(|p| {
                std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .map(|age| age > max_age)
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Every directory under the worktrees root, whether or not a record
    /// claims it. Startup uses this to find directories no row knows.
    pub fn worktree_dirs(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(self.store.worktrees_dir()) else {
            return Vec::new();
        };
        entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()
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

}
