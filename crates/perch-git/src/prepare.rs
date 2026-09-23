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

/// A branch new work can start from, as the base picker lists it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BaseBranch {
    /// The branch's name, without a remote: `main`, `chris/fix`.
    pub name: String,
    /// The remote it is on, for a remote-tracking branch, or one the
    /// store's clone only knows from `ls-remote`. `None` for a local one.
    pub remote: Option<String>,
}

impl BaseBranch {
    pub fn local(name: &str) -> Self {
        Self { name: name.to_string(), remote: None }
    }

    pub fn on(remote: &str, name: &str) -> Self {
        Self { name: name.to_string(), remote: Some(remote.to_string()) }
    }

    /// `origin/fix`, or `fix` for a local branch: what git calls it in a
    /// clone that has the remote.
    pub fn label(&self) -> String {
        match &self.remote {
            Some(remote) => format!("{remote}/{}", self.name),
            None => self.name.clone(),
        }
    }
}

/// Put the base first, and list each other name once: a local branch
/// before a remote one of the same name. `refs` are `for-each-ref` names,
/// newest first.
fn order_bases(refs: &[String], remote: Option<&str>, base: &str) -> Vec<BaseBranch> {
    let mut locals = Vec::new();
    let mut remotes = Vec::new();
    for name in refs {
        if let Some(branch) = name.strip_prefix("refs/heads/") {
            locals.push(BaseBranch::local(branch));
        } else if let Some(remote) = remote {
            if let Some(branch) = name.strip_prefix(&format!("refs/remotes/{remote}/")) {
                if branch != "HEAD" {
                    remotes.push(BaseBranch::on(remote, branch));
                }
            }
        }
    }
    let mut out = Vec::new();
    // The base as the remote has it, fetched just before the cut, then as
    // the clone has it, which may be behind or ahead.
    if let Some(b) = remotes.iter().find(|b| b.name == base) {
        out.push(b.clone());
    }
    if let Some(b) = locals.iter().find(|b| b.name == base) {
        out.push(b.clone());
    }
    for b in &locals {
        if b.name != base {
            out.push(b.clone());
        }
    }
    for b in &remotes {
        if b.name != base && !locals.iter().any(|l| l.name == b.name) {
            out.push(b.clone());
        }
    }
    out
}

/// A path for an index of Perch's own beside nothing of the reader's: in
/// the temporary directory, named for the worktree and the moment.
fn tempfile_path(worktree: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    worktree.hash(&mut hasher);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("perch-index-{:x}-{nanos}", hasher.finish()))
}

/// Where `ensure_home` fetches a default branch's tip to in the bare clone.
pub fn home_ref(branch: &str) -> String {
    format!("refs/perch/home/{branch}")
}

/// Where `fetch_pull` also puts a pull request's base, under a name that
/// says which branch it was fetched from. The branch name is hex so that
/// `release` and `release/1.0` never make a ref and a directory of one
/// path.
fn base_of_ref(number: u64, base_ref: &str) -> String {
    let hex: String = base_ref.bytes().map(|b| format!("{b:02x}")).collect();
    format!("refs/perch/base-of/{number}/{hex}")
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

    /// Use this token for every git command from now on.
    pub fn set_token(&self, token: Option<String>) {
        self.git.set_token(token);
    }

    /// Clone if needed, fetch the pull request, and put its head on
    /// `branch` in a worktree.
    ///
    /// The worktree is keyed by the branch, so opening the same pull request
    /// again finds the same directory. When its head has moved since, a
    /// clean worktree is reset to the new head; a dirty one is left alone
    /// and the checkout reports the head it did not take, so nothing
    /// anyone wrote in there is ever reset away. So is a clean one holding
    /// a commit the pull request never had: one the reader made there and
    /// has not pushed.
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
        // The head the last fetch saw, before this one moves the ref: after
        // a force push it is the only thing that says the commit on disk
        // came from the pull request rather than from the reader.
        let previous = self.resolve(&bare, &format!("refs/perch/pull/{number}/head")).await;

        // A fetch is a round trip to GitHub, most of the time an open takes.
        // When the last one already brought this head, of this base, there
        // is nothing new the diff needs: the base's tip may have moved on,
        // but with the head where it was, the merge base has not.
        if !self.has_pull(&bare, number, head_sha, base_ref).await {
            on_step(Step::Fetching);
            self.fetch_pull(&bare, "origin", number, base_ref).await?;
        }
        let base_local = format!("refs/perch/base/{number}");

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
        } else if self.is_dirty(&worktree).await?
            || !self.came_from_pull(&worktree, &on_disk, head_sha, previous.as_deref()).await
        {
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
    /// Start work, Fork and new work all come here. `repo` is where the
    /// branch is made: the store's bare clone of `id`, or one of the
    /// reader's roots. The worktree goes under the store either way, named
    /// for `id`. `wanted` is the name asked for; when `repo` already has a
    /// branch of that name, `-2`, `-3` and so on are tried, so a second
    /// Start work on the same issue or a second Fork of the same item gets
    /// a fresh branch rather than an error. `from` is anything git
    /// resolves to a commit: a sha, a branch, or the
    /// `refs/perch/home/<branch>` ref a home was just fetched to.
    pub async fn cut_branch(
        &self,
        repo: &Path,
        id: &RepoId,
        wanted: &str,
        from: &str,
        reserved: &[String],
    ) -> Result<Cut, Error> {
        if !repo.exists() {
            return Err(Error::Io(format!("no repository at {}", repo.display())));
        }

        // A name is taken when the branch exists, when a worktree already
        // sits at the path the name maps to (two branches can map to one
        // path once slashes become dashes, and a checkout there is
        // someone's work whichever branch it is on), or when the caller
        // has it spoken for: a root's branch is not in this clone, but it
        // is a row, and a second row on the name would be refused.
        let mut branch = wanted.to_string();
        let mut n = 1u32;
        loop {
            if !self.branch_taken(repo, id, &branch, reserved).await {
                break;
            }
            n += 1;
            branch = format!("{wanted}-{n}");
        }

        let worktree = self.add_worktree(repo, id, &branch, &["-b", &branch], from).await?;
        Ok(Cut { branch, worktree })
    }

    /// Whether a new branch can't have this name: `repo` has a branch of
    /// it, a worktree sits at the path it maps to, or the caller has it
    /// spoken for.
    pub async fn branch_taken(&self, repo: &Path, id: &RepoId, branch: &str, reserved: &[String]) -> bool {
        reserved.iter().any(|r| r == branch)
            || self.branch_exists(repo, branch).await
            || self.store.worktree_path(id, branch).join(".git").exists()
    }

    /// Check out `branch` in the store's clone at `commitish`, a ref just
    /// fetched from GitHub, moving the clone's own copy of the branch
    /// there. For a repository with no root, where the clone's branches
    /// are whatever it saw when it was made and nothing else of the
    /// reader's.
    pub async fn check_out_fetched(&self, bare: &Path, id: &RepoId, branch: &str, commitish: &str) -> Result<Cut, Error> {
        let worktree = self.add_worktree(bare, id, branch, &["-B", branch], commitish).await?;
        Ok(Cut { branch: branch.to_string(), worktree })
    }

    /// Check out a branch that already exists, rather than cutting a new
    /// one: `branch` itself when `repo` has it, else a new local branch of
    /// that name tracking `tracking`, a remote-tracking ref such as
    /// `origin/fix`. Git refuses a branch another worktree has checked
    /// out, and that refusal is the error.
    pub async fn check_out_branch(
        &self,
        repo: &Path,
        id: &RepoId,
        branch: &str,
        tracking: Option<&str>,
    ) -> Result<Cut, Error> {
        let worktree = if self.branch_exists(repo, branch).await {
            self.add_worktree(repo, id, branch, &[], branch).await?
        } else {
            let Some(tracking) = tracking else {
                return Err(Error::Io(format!("there is no branch {branch} to check out")));
            };
            self.add_worktree(repo, id, branch, &["--track", "-b", branch], tracking)
                .await?
        };
        Ok(Cut { branch: branch.to_string(), worktree })
    }

    /// The branches of one of the reader's clones that new work can start
    /// from: its local branches and `remote`'s, newest first, `base`
    /// first of all. No network.
    pub async fn local_bases(&self, repo: &Path, remote: Option<&str>, base: &str) -> Result<Vec<BaseBranch>, Error> {
        let mut args = vec![
            "for-each-ref".to_string(),
            "--sort=-committerdate".to_string(),
            "--format=%(refname)".to_string(),
            "refs/heads/".to_string(),
        ];
        if let Some(remote) = remote {
            args.push(format!("refs/remotes/{remote}/"));
        }
        let out = self.git.run(Some(repo), args, QUERY_TIMEOUT).await?;
        let refs: Vec<String> = out.lines().map(str::to_string).collect();
        Ok(order_bases(&refs, remote, base))
    }

    /// The branches GitHub has for a repository with no root, `base`
    /// first. One round trip, asked of the store's clone's `origin` when
    /// there is a clone and of GitHub directly when there is not, so
    /// listing them does not clone.
    pub async fn remote_bases(&self, id: &RepoId, base: &str) -> Result<Vec<BaseBranch>, Error> {
        let bare = self.store.bare_path(id);
        let (cwd, remote) = if self.store.has_bare_clone(id) {
            (Some(bare.as_path()), "origin".to_string())
        } else {
            (None, id.clone_url())
        };
        let out = self
            .git
            .run(cwd, ["ls-remote", "--heads", &remote], FETCH_TIMEOUT)
            .await?;
        let mut names: Vec<&str> = out
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .filter_map(|(_, name)| name.strip_prefix("refs/heads/"))
            .collect();
        names.sort();
        let mut out = vec![BaseBranch::on("origin", base)];
        out.extend(
            names
                .into_iter()
                .filter(|name| *name != base)
                .map(|name| BaseBranch::on("origin", name)),
        );
        Ok(out)
    }

    /// Fetch one branch of `remote` into its remote-tracking ref in one of
    /// the reader's clones, with the reader's own credentials and none of
    /// Perch's. Nothing can prompt: SSH runs in batch mode on top of any
    /// `core.sshCommand` the clone has, so a key that wants a passphrase
    /// or a host not yet known fails the fetch instead of waiting.
    pub async fn fetch_branch(&self, repo: &Path, remote: &str, branch: &str) -> Result<(), Error> {
        let plain = Git::plain();
        let ssh = std::env::var("GIT_SSH_COMMAND").ok().filter(|s| !s.is_empty());
        let ssh = match ssh {
            Some(ssh) => ssh,
            None => plain
                .run(Some(repo), ["config", "--get", "core.sshCommand"], QUERY_TIMEOUT)
                .await
                .map(|s| s.trim().to_string())
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "ssh".to_string()),
        };
        plain
            .run(
                Some(repo),
                [
                    "-c".to_string(),
                    format!("core.sshCommand={ssh} -o BatchMode=yes"),
                    "fetch".to_string(),
                    "--no-tags".to_string(),
                    remote.to_string(),
                    format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}"),
                ],
                FETCH_TIMEOUT,
            )
            .await?;
        Ok(())
    }

    /// How long ago a ref's commit was made, as git says it: `3 days ago`.
    /// For a notice about a fetch that failed.
    pub async fn age_of(&self, repo: &Path, rev: &str) -> Option<String> {
        self.git
            .run(Some(repo), ["log", "-1", "--format=%cr", rev, "--"], QUERY_TIMEOUT)
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Fetch a branch of the store's clone's `origin` into
    /// `refs/perch/home/<branch>`, where a cut starts from.
    pub async fn fetch_home_ref(&self, bare: &Path, branch: &str) -> Result<(), Error> {
        self.git
            .run(
                Some(bare),
                [
                    "fetch".to_string(),
                    "--no-tags".to_string(),
                    "origin".to_string(),
                    format!("+refs/heads/{branch}:{}", home_ref(branch)),
                ],
                FETCH_TIMEOUT,
            )
            .await?;
        Ok(())
    }

    /// The repository a checkout belongs to, as a path a branch can be
    /// cut from and that outlives the checkout: its main worktree, or the
    /// bare repository when it has none. `git worktree list` names that
    /// first, from any worktree of the repository.
    pub async fn repository_of(&self, checkout: &Path) -> Result<PathBuf, Error> {
        let out = self
            .git
            .run(Some(checkout), ["worktree", "list", "--porcelain"], QUERY_TIMEOUT)
            .await?;
        out.lines()
            .find_map(|line| line.strip_prefix("worktree "))
            .map(PathBuf::from)
            .ok_or_else(|| Error::Io(format!("git lists no worktree for {}", checkout.display())))
    }

    /// `git worktree add <flags> <path> <commitish>` in `repo`, at the
    /// store's path for `branch`, after clearing a leftover there.
    async fn add_worktree(
        &self,
        repo: &Path,
        id: &RepoId,
        branch: &str,
        flags: &[&str],
        commitish: &str,
    ) -> Result<PathBuf, Error> {
        let worktree = self.store.worktree_path(id, branch);
        if worktree.join(".git").exists() {
            return Err(Error::Io(format!("{} is already a checkout", worktree.display())));
        }
        std::fs::create_dir_all(self.store.worktrees_dir()).map_err(|e| Error::Io(e.to_string()))?;
        // Not a checkout, so anything at the path is a leftover from an
        // interrupted run, and empty of git's knowledge.
        if worktree.exists() {
            std::fs::remove_dir_all(&worktree).map_err(|e| Error::Io(e.to_string()))?;
        }
        let _ = self.git.run(Some(repo), ["worktree", "prune"], WORKTREE_TIMEOUT).await;

        let path = worktree.to_string_lossy().into_owned();
        let mut args = vec!["worktree", "add"];
        args.extend_from_slice(flags);
        args.push(&path);
        args.push(commitish);
        self.git.run(Some(repo), args, WORKTREE_TIMEOUT).await?;
        Ok(worktree)
    }

    /// Whether the commit on disk is one the pull request had: behind its
    /// new head, or behind the head the last fetch saw, which a force push
    /// has since replaced. Anything else was committed here, and resetting
    /// would leave it reachable from nothing but the reflog.
    async fn came_from_pull(&self, worktree: &Path, on_disk: &str, head: &str, previous: Option<&str>) -> bool {
        if self.is_ancestor(worktree, on_disk, head).await {
            return true;
        }
        match previous {
            Some(previous) => self.is_ancestor(worktree, on_disk, previous).await,
            None => false,
        }
    }

    /// `git merge-base --is-ancestor a b`: whether `a` is `b` or behind it.
    /// A commit git cannot find is not an ancestor of anything.
    pub async fn is_ancestor(&self, repo: &Path, a: &str, b: &str) -> bool {
        self.git
            .run(Some(repo), ["merge-base", "--is-ancestor", a, b], QUERY_TIMEOUT)
            .await
            .is_ok()
    }

    /// The commit a ref names, or `None` when there is no such ref.
    async fn resolve(&self, repo: &Path, name: &str) -> Option<String> {
        self.git
            .run(Some(repo), ["rev-parse", "--verify", "--quiet", &format!("{name}^{{commit}}")], QUERY_TIMEOUT)
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
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

    /// Fetch a pull request's head and its base branch into the bare clone,
    /// in a namespace of our own so these refs can never collide with the
    /// user's: `refs/perch/pull/N/head` and `refs/perch/base/N`. `from` is
    /// `origin`, or the URL of the repository the pull request is on when
    /// the clone is of a fork.
    ///
    /// The base is also written under a ref that names the branch it came
    /// from, which is how `has_pull` knows a retargeted pull request needs
    /// fetching again.
    pub async fn fetch_pull(&self, bare: &Path, from: &str, number: u64, base_ref: &str) -> Result<(), Error> {
        self.git
            .run(
                Some(bare),
                [
                    "fetch".to_string(),
                    "--no-tags".to_string(),
                    from.to_string(),
                    format!("+refs/pull/{number}/head:refs/perch/pull/{number}/head"),
                    format!("+refs/heads/{base_ref}:refs/perch/base/{number}"),
                    format!("+refs/heads/{base_ref}:{}", base_of_ref(number, base_ref)),
                ],
                FETCH_TIMEOUT,
            )
            .await?;
        Ok(())
    }

    /// Whether the last `fetch_pull` for this pull request saw `head_sha`
    /// as its head and fetched its base from `base_ref`. One local git
    /// call; anything missing or unreadable is a no.
    async fn has_pull(&self, bare: &Path, number: u64, head_sha: &str, base_ref: &str) -> bool {
        let Ok(out) = self
            .git
            .run(
                Some(bare),
                [
                    "rev-parse".to_string(),
                    format!("refs/perch/pull/{number}/head"),
                    format!("refs/perch/base/{number}"),
                    base_of_ref(number, base_ref),
                    "--".to_string(),
                ],
                QUERY_TIMEOUT,
            )
            .await
        else {
            return false;
        };
        let mut shas = out.lines().map(str::trim);
        match (shas.next(), shas.next(), shas.next()) {
            (Some(head), Some(base), Some(base_of)) => head == head_sha && base == base_of,
            _ => false,
        }
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

    /// How many commits on `branch` no other ref holds: no other branch,
    /// tag, remote-tracking branch, or ref of Perch's own such as a pull
    /// request's fetched head. These are what deleting the branch loses.
    /// Worktrees' `HEAD`s are left out on purpose, since the branch's own
    /// worktree would otherwise hold every one of them.
    pub async fn commits_only_on(&self, repo: &Path, branch: &str) -> Result<u64, Error> {
        let out = self
            .git
            .run(
                Some(repo),
                [
                    "rev-list".to_string(),
                    "--count".to_string(),
                    format!("refs/heads/{branch}"),
                    "--not".to_string(),
                    format!("--exclude={branch}"),
                    "--branches".to_string(),
                    "--tags".to_string(),
                    "--remotes".to_string(),
                    "--glob=refs/perch/*".to_string(),
                    "--".to_string(),
                ],
                QUERY_TIMEOUT,
            )
            .await?;
        out.trim()
            .parse()
            .map_err(|_| Error::Io(format!("git rev-list --count said {:?}", out.trim())))
    }

    /// What in a worktree git ignores, as `status` names it: a directory
    /// that is ignored whole is one entry ending in `/`. Removing the
    /// directory removes these too, and git keeps no copy of them.
    pub async fn ignored_entries(&self, worktree: &Path) -> Result<Vec<String>, Error> {
        let out = self
            .git
            .run(
                Some(worktree),
                ["status", "--porcelain", "-z", "--ignored=matching", "--untracked-files=normal"],
                QUERY_TIMEOUT,
            )
            .await?;
        let mut entries: Vec<String> = out
            .split('\0')
            .filter_map(|entry| entry.strip_prefix("!! "))
            .map(str::to_string)
            .collect();
        entries.sort();
        entries.dedup();
        Ok(entries)
    }

    /// Record a worktree as it is, as a commit, without touching it: an
    /// index of Perch's own is filled from `HEAD` and then with every
    /// tracked and untracked file that is not ignored, written as a tree,
    /// and committed with `HEAD` as its parent. The commit is kept at
    /// `reference`, a ref under `refs/perch/`, so `gc` keeps it; the
    /// worktree's own index, `HEAD` and files are as they were.
    pub async fn record_worktree(&self, worktree: &Path, reference: &str, message: &str) -> Result<String, Error> {
        let index = tempfile_path(worktree);
        let result = self.record_with_index(worktree, &index, reference, message).await;
        let _ = std::fs::remove_file(&index);
        result
    }

    async fn record_with_index(
        &self,
        worktree: &Path,
        index: &Path,
        reference: &str,
        message: &str,
    ) -> Result<String, Error> {
        let env: [(&str, &std::ffi::OsStr); 5] = [
            ("GIT_INDEX_FILE", index.as_os_str()),
            ("GIT_AUTHOR_NAME", "Perch".as_ref()),
            ("GIT_AUTHOR_EMAIL", "perch@localhost".as_ref()),
            ("GIT_COMMITTER_NAME", "Perch".as_ref()),
            ("GIT_COMMITTER_EMAIL", "perch@localhost".as_ref()),
        ];
        let git = &self.git;
        git.run_env(Some(worktree), ["read-tree", "HEAD"], &env, QUERY_TIMEOUT).await?;
        git.run_env(Some(worktree), ["add", "-A", "--", "."], &env, WORKTREE_TIMEOUT)
            .await?;
        let tree = git.run_env(Some(worktree), ["write-tree"], &env, QUERY_TIMEOUT).await?;
        let commit = git
            .run_env(
                Some(worktree),
                ["commit-tree", tree.trim(), "-p", "HEAD", "-m", message],
                &env,
                QUERY_TIMEOUT,
            )
            .await?;
        let commit = commit.trim().to_string();
        git.run(Some(worktree), ["update-ref", reference, &commit], QUERY_TIMEOUT)
            .await?;
        Ok(commit)
    }

    /// Drop refs a run recorded, where they are still there to drop.
    pub async fn drop_refs(&self, repo: &Path, references: &[String]) {
        for reference in references {
            let _ = self
                .git
                .run(Some(repo), ["update-ref", "-d", reference], QUERY_TIMEOUT)
                .await;
        }
    }

    /// `(files, added, removed)` between two commits, binary files
    /// counted as files with no lines.
    pub async fn change_count(&self, repo: &Path, from: &str, to: &str) -> Result<(u32, u32, u32), Error> {
        let out = self
            .git
            .run(Some(repo), ["diff", "--numstat", "--no-renames", from, to], QUERY_TIMEOUT)
            .await?;
        let mut counts = (0, 0, 0);
        for line in out.lines() {
            let mut parts = line.split('\t');
            let added = parts.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
            let removed = parts.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
            counts.0 += 1;
            counts.1 += added;
            counts.2 += removed;
        }
        Ok(counts)
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
    fn the_base_comes_first_and_each_other_name_once() {
        let refs: Vec<String> = [
            "refs/heads/feature",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/fix",
            "refs/heads/main",
            "refs/remotes/origin/feature",
            "refs/remotes/origin/main",
            "refs/remotes/fork/other",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let bases = order_bases(&refs, Some("origin"), "main");
        let labels: Vec<String> = bases.iter().map(BaseBranch::label).collect();
        assert_eq!(labels, ["origin/main", "main", "feature", "origin/fix"]);
    }

    #[test]
    fn a_clone_with_no_remote_lists_its_branches() {
        let refs: Vec<String> = ["refs/heads/b", "refs/heads/main", "refs/heads/a"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let labels: Vec<String> = order_bases(&refs, None, "main").iter().map(BaseBranch::label).collect();
        assert_eq!(labels, ["main", "b", "a"]);
    }

    #[test]
    fn home_refs_sit_in_their_own_namespace() {
        assert_eq!(home_ref("main"), "refs/perch/home/main");
        assert!(!home_ref("main").starts_with("refs/heads/"));
    }
}
