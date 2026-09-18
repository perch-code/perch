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

/// A branch cut by `cut_branch`, checked out and ready to be opened.
#[derive(Debug, Clone, PartialEq)]
pub struct Cut {
    /// The name that was free: the one asked for, or it with `-2`, `-3`…
    pub branch: String,
    pub worktree: PathBuf,
}

/// Where `ensure_home` fetches a default branch's tip to in the bare clone.
pub fn home_ref(branch: &str) -> String {
    format!("refs/perch/home/{branch}")
}

/// The branch `git ls-remote --symref <remote> HEAD` says HEAD points at.
///
/// The line is `ref: refs/heads/<name>\tHEAD`; the sha line after it is not
/// needed. A detached remote HEAD prints no `ref:` line at all.
fn parse_symref_head(out: &str) -> Option<String> {
    out.lines().find_map(|line| {
        let rest = line.strip_prefix("ref: ")?;
        let (target, _) = rest.split_once('\t')?;
        let name = target.strip_prefix("refs/heads/")?;
        (!name.is_empty()).then(|| name.to_string())
    })
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
        let bare = self.ensure_bare(id, &mut on_step).await?;

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

    /// The bare clone, made if it is missing. Every checkout starts here.
    pub async fn ensure_bare(
        &self,
        id: &RepoId,
        mut on_step: impl FnMut(Step) + Send,
    ) -> Result<PathBuf, Error> {
        let bare = self.store.bare_path(id);
        if self.store.has_bare_clone(id) {
            return Ok(bare);
        }

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
                |p: Progress| on_step(Step::Cloning { phase: p.phase, percent: p.percent }),
            )
            .await?;

        std::fs::rename(&staging, &bare).map_err(|e| Error::Io(e.to_string()))?;
        Ok(bare)
    }

    /// The repository's default branch, as the remote reports it.
    ///
    /// `refs/remotes/origin/HEAD` in the bare clone answers without the
    /// network when something has set it; a bare clone does not get one
    /// from `git clone`, so the usual path is one `ls-remote` round trip.
    /// A remote whose HEAD is not a branch, or an empty repository, gets
    /// `main`. A remote that cannot be reached is an error, because the
    /// fetch that follows would fail on it anyway and this says so sooner.
    pub async fn default_branch(&self, id: &RepoId) -> Result<String, Error> {
        let bare = self.store.bare_path(id);
        let has_bare = self.store.has_bare_clone(id);

        if has_bare {
            if let Ok(target) = self
                .git
                .run(Some(&bare), ["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"], QUERY_TIMEOUT)
                .await
            {
                if let Some(name) = target.trim().strip_prefix("refs/remotes/origin/") {
                    if !name.is_empty() {
                        return Ok(name.to_string());
                    }
                }
            }
        }

        // With a clone, ask its origin, which in a test may be a local path;
        // without one, ask GitHub directly rather than cloning to find out.
        let (cwd, remote) = if has_bare {
            (Some(bare.as_path()), "origin".to_string())
        } else {
            (None, id.clone_url())
        };
        let out = self
            .git
            .run(cwd, ["ls-remote", "--symref", &remote, "HEAD"], FETCH_TIMEOUT)
            .await?;

        Ok(parse_symref_head(&out).unwrap_or_else(|| "main".to_string()))
    }

    /// Bring the home checkout up to date with the default branch, making
    /// it if it does not exist yet.
    ///
    /// The branch tip is fetched into `refs/perch/home/<branch>`, a
    /// namespace of Perch's own so it can never collide with a branch the
    /// user has, and one keyed by branch rather than the pull-request-keyed
    /// `refs/perch/base/<n>` because a home has no pull request. The
    /// worktree sits on a local branch of the default branch's name, and on
    /// each call it is fast-forwarded to the fetched tip. A home that cannot
    /// be fast-forwarded, because someone left changes or commits in it, is
    /// left where it is with a log line; nothing here ever resets or removes
    /// a home.
    pub async fn ensure_home(&self, id: &RepoId, default_branch: &str) -> Result<PathBuf, Error> {
        let bare = self.ensure_bare(id, |_| {}).await?;
        let fetched = home_ref(default_branch);

        self.git
            .run(
                Some(&bare),
                [
                    "fetch".to_string(),
                    "--no-tags".to_string(),
                    "origin".to_string(),
                    format!("+refs/heads/{default_branch}:{fetched}"),
                ],
                FETCH_TIMEOUT,
            )
            .await?;

        let home = self.store.home_path(id);

        if !home.join(".git").exists() {
            std::fs::create_dir_all(self.store.homes_dir()).map_err(|e| Error::Io(e.to_string()))?;
            // As in `prepare`: a directory an interrupted run left behind,
            // or a registration of one that is gone, would make `worktree
            // add` refuse.
            let _ = std::fs::remove_dir_all(&home);
            let _ = self.git.run(Some(&bare), ["worktree", "prune"], WORKTREE_TIMEOUT).await;

            let path = home.to_string_lossy().into_owned();
            // `-B`: the bare clone's own copy of the branch is whatever the
            // clone saw, and the home is the only place it is checked out,
            // so it is simply moved to the tip that was just fetched.
            self.git
                .run(
                    Some(&bare),
                    ["worktree", "add", "-B", default_branch, &path, &fetched],
                    WORKTREE_TIMEOUT,
                )
                .await?;
            return Ok(home);
        }

        if let Err(e) = self
            .git
            .run(Some(&home), ["merge", "--ff-only", &fetched], WORKTREE_TIMEOUT)
            .await
        {
            tracing::warn!(
                home = %home.display(),
                branch = default_branch,
                error = %e,
                "home checkout could not be fast-forwarded; leaving it as it is"
            );
        }
        Ok(home)
    }

    /// Cut a new branch at `from` and check it out in a worktree of its own.
    ///
    /// Start work and Fork both come here. `wanted` is the name asked for;
    /// when the bare clone already has a branch of that name, `-2`, `-3`
    /// and so on are tried, so a second Start work on the same issue or a
    /// second Fork of the same item gets a fresh branch rather than an
    /// error. `from` is anything git resolves to a commit: a sha, or the
    /// `refs/perch/home/<branch>` ref a home was just fetched to.
    pub async fn cut_branch(&self, id: &RepoId, wanted: &str, from: &str) -> Result<Cut, Error> {
        if !self.store.has_bare_clone(id) {
            return Err(Error::Io(format!("no clone of {} in the store", id.full_name())));
        }
        let bare = self.store.bare_path(id);

        // A name is taken when the branch exists, or when a worktree
        // already sits at the path the name maps to: two branches can map
        // to one path once slashes become dashes, and a checkout there is
        // someone's work whichever branch it is on.
        let mut branch = wanted.to_string();
        let mut n = 1u32;
        loop {
            let taken = self.branch_exists(&bare, &branch).await
                || self.store.worktree_path(id, &branch).join(".git").exists();
            if !taken {
                break;
            }
            n += 1;
            branch = format!("{wanted}-{n}");
        }

        let worktree = self.store.worktree_path(id, &branch);
        std::fs::create_dir_all(self.store.worktrees_dir()).map_err(|e| Error::Io(e.to_string()))?;
        // Neither a branch nor a checkout, so anything at the path is a
        // leftover from an interrupted run, and empty of git's knowledge.
        if worktree.exists() {
            std::fs::remove_dir_all(&worktree).map_err(|e| Error::Io(e.to_string()))?;
        }
        let _ = self.git.run(Some(&bare), ["worktree", "prune"], WORKTREE_TIMEOUT).await;

        let path = worktree.to_string_lossy().into_owned();
        self.git
            .run(Some(&bare), ["worktree", "add", "-b", &branch, &path, from], WORKTREE_TIMEOUT)
            .await?;

        Ok(Cut { branch, worktree })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_symref_line_names_the_default_branch() {
        let out = "ref: refs/heads/develop\tHEAD\n0123abcd\tHEAD\n";
        assert_eq!(parse_symref_head(out).as_deref(), Some("develop"));
    }

    #[test]
    fn a_detached_remote_head_has_no_symref_line() {
        assert_eq!(parse_symref_head("0123abcd\tHEAD\n"), None);
        assert_eq!(parse_symref_head(""), None);
    }

    #[test]
    fn home_refs_sit_in_their_own_namespace() {
        assert_eq!(home_ref("main"), "refs/perch/home/main");
        assert!(!home_ref("main").starts_with("refs/heads/"));
    }
}
