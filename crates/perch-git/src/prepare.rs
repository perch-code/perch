//! Turning a pull request into a checkout on disk.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cmd::{Git, Progress};
use crate::keyed::KeyedMutex;
use crate::store::{RepoId, Store};
use crate::Error;

/// How long a clone may go without reporting progress. A clone has no
/// limit on the whole: a large repository takes as long as it takes.
const CLONE_IDLE: Duration = Duration::from_secs(120);
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
const WORKTREE_TIMEOUT: Duration = Duration::from_secs(30);
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

/// What the caller is told while a checkout is being prepared.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Cloning {
        phase: String,
        percent: Option<u8>,
    },
    Fetching,
    /// A partial clone's files for the checkout, ahead of writing them.
    Downloading {
        phase: String,
        percent: Option<u8>,
    },
    CreatingWorktree,
    ComputingDiff,
}

/// A pull request fetched before its head was known, for `prepare_after`.
#[derive(Debug, Clone, PartialEq)]
pub struct Prefetched {
    /// The head the fetch before it saw.
    previous: Option<String>,
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
        Self {
            name: name.to_string(),
            remote: None,
        }
    }

    pub fn on(remote: &str, name: &str) -> Self {
        Self {
            name: name.to_string(),
            remote: Some(remote.to_string()),
        }
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

/// Commit a worktree as it is without touching it: an index of Perch's
/// own is filled from `HEAD` and then with every tracked and untracked
/// file that is not ignored, written as a tree, and committed with `HEAD`
/// as its parent. The worktree's own index, `HEAD` and files are as they
/// were. Nothing points at the commit, so a caller that keeps it past the
/// next `gc` gives it a ref or copies it out.
pub(crate) async fn commit_worktree(
    git: &Git,
    worktree: &Path,
    message: &str,
) -> Result<String, Error> {
    let index = tempfile_path(worktree);
    let result = commit_with_index(git, worktree, &index, message).await;
    let _ = std::fs::remove_file(&index);
    result
}

async fn commit_with_index(
    git: &Git,
    worktree: &Path,
    index: &Path,
    message: &str,
) -> Result<String, Error> {
    let env: [(&str, &std::ffi::OsStr); 5] = [
        ("GIT_INDEX_FILE", index.as_os_str()),
        ("GIT_AUTHOR_NAME", "Perch".as_ref()),
        ("GIT_AUTHOR_EMAIL", "perch@localhost".as_ref()),
        ("GIT_COMMITTER_NAME", "Perch".as_ref()),
        ("GIT_COMMITTER_EMAIL", "perch@localhost".as_ref()),
    ];
    git.run_env(Some(worktree), ["read-tree", "HEAD"], &env, QUERY_TIMEOUT)
        .await?;
    git.run_env(
        Some(worktree),
        ["add", "-A", "--", "."],
        &env,
        WORKTREE_TIMEOUT,
    )
    .await?;
    let tree = git
        .run_env(Some(worktree), ["write-tree"], &env, QUERY_TIMEOUT)
        .await?;
    // `commit.gpgSign` would otherwise ask for a key for a commit no one
    // else sees.
    let commit = git
        .run_env(
            Some(worktree),
            [
                "commit-tree",
                "--no-gpg-sign",
                tree.trim(),
                "-p",
                "HEAD",
                "-m",
                message,
            ],
            &env,
            QUERY_TIMEOUT,
        )
        .await?;
    Ok(commit.trim().to_string())
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

/// A directory removed when this is dropped, if it is still there.
struct RemoveOnDrop<'a>(&'a Path);

impl Drop for RemoveOnDrop<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0);
    }
}

pub struct Preparer {
    git: Git,
    store: Store,
    /// A repository's, held while its bare clone is made, so two callers
    /// that both find it missing do not clone into the same staging
    /// directory. Per repository: a large clone holds no other up.
    cloning: KeyedMutex,
    /// A repository's, held while its home is fetched and moved, so a home
    /// brought up to date behind an open issue and one Start work needs do
    /// not race on its ref and its directory.
    homes: KeyedMutex,
    /// Repositories to clone without their history's file contents: see
    /// `clone_partially`.
    partial: std::sync::Mutex<std::collections::HashSet<String>>,
}

/// Where the bare clone remembers its remote's default branch. A clone
/// made with `--bare` has no `refs/remotes/origin/HEAD` to say so.
const DEFAULT_BRANCH_KEY: &str = "perch.defaultBranch";

impl Preparer {
    pub fn new(store: Store, token: Option<String>) -> Self {
        Self {
            git: Git::new(token),
            store,
            cloning: KeyedMutex::new(),
            homes: KeyedMutex::new(),
            partial: Default::default(),
        }
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
        on_step: impl FnMut(Step) + Send,
    ) -> Result<Checkout, Error> {
        self.prepare_after(id, number, head_sha, base_ref, branch, None, on_step)
            .await
    }

    /// Clone if needed and fetch a pull request's head and base, before
    /// its head is known: the number and the base branch are all a fetch
    /// needs. What it returns goes to `prepare_after`, which then fetches
    /// again only if the head asked for is not the one fetched here.
    pub async fn prefetch(
        &self,
        id: &RepoId,
        number: u64,
        base_ref: &str,
        mut on_step: impl FnMut(Step) + Send,
    ) -> Result<Prefetched, Error> {
        let bare = self.ensure_bare(id, &mut on_step).await?;
        let previous = self
            .resolve(&bare, &format!("refs/perch/pull/{number}/head"))
            .await;
        on_step(Step::Fetching);
        self.fetch_pull(&bare, "origin", number, base_ref).await?;
        Ok(Prefetched { previous })
    }

    /// `prepare`, after a `prefetch` of the same pull request, or with
    /// `None` when there was none.
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_after(
        &self,
        id: &RepoId,
        number: u64,
        head_sha: &str,
        base_ref: &str,
        branch: &str,
        prefetched: Option<Prefetched>,
        mut on_step: impl FnMut(Step) + Send,
    ) -> Result<Checkout, Error> {
        let bare = self.ensure_bare(id, &mut on_step).await?;
        // The head the last fetch saw, before this one moves the ref: after
        // a force push it is the only thing that says the commit on disk
        // came from the pull request rather than from the reader. A
        // prefetch read it before it fetched.
        let previous = match prefetched {
            Some(prefetched) => prefetched.previous,
            None => {
                self.resolve(&bare, &format!("refs/perch/pull/{number}/head"))
                    .await
            }
        };

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
            .run(
                Some(&bare),
                ["merge-base", &base_local, head_sha],
                QUERY_TIMEOUT,
            )
            .await?
            .trim()
            .to_string();

        if merge_base.is_empty() {
            return Err(Error::NoMergeBase);
        }

        self.fill_blobs(&bare, head_sha, &mut on_step).await?;
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
                    .run(
                        Some(&bare),
                        ["worktree", "add", &path, branch],
                        WORKTREE_TIMEOUT,
                    )
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
            || !self
                .came_from_pull(&worktree, &on_disk, head_sha, previous.as_deref())
                .await
        {
            (on_disk, Some(head_sha.to_string()))
        } else {
            // HEAD is attached to the branch, so this moves the branch too.
            self.git
                .run(
                    Some(&worktree),
                    ["reset", "--hard", head_sha],
                    WORKTREE_TIMEOUT,
                )
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

    /// In a partial clone, download the files `rev` has that are not here
    /// yet, saying how it goes, before a checkout writes them out. git
    /// would fetch them itself as the checkout reads them, but silently,
    /// and under the checkout's short timeout, which a large tree outlasts.
    /// The request is the one git makes for them. A full clone has every
    /// file already, so this asks nothing of it.
    async fn fill_blobs(
        &self,
        repo: &Path,
        rev: &str,
        on_step: &mut (impl FnMut(Step) + Send),
    ) -> Result<(), Error> {
        let promisor = self
            .git
            .run(
                Some(repo),
                ["config", "--get", "remote.origin.promisor"],
                QUERY_TIMEOUT,
            )
            .await
            .is_ok_and(|value| value.trim() == "true");
        if !promisor {
            return Ok(());
        }
        // `--missing=print` lists what is not here rather than fetching it.
        let listed = self
            .git
            .run(
                Some(repo),
                [
                    "rev-list",
                    "--objects",
                    "--missing=print",
                    &format!("{rev}^{{tree}}"),
                ],
                QUERY_TIMEOUT,
            )
            .await?;
        let missing: String = listed
            .lines()
            .filter_map(|line| line.strip_prefix('?'))
            .map(|oid| format!("{oid}\n"))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        self.git
            .run_with_progress_input(
                Some(repo),
                [
                    "-c",
                    "fetch.negotiationAlgorithm=noop",
                    "fetch",
                    "--progress",
                    "origin",
                    "--no-tags",
                    "--no-write-fetch-head",
                    "--recurse-submodules=no",
                    "--filter=blob:none",
                    "--stdin",
                ],
                Some(missing.into_bytes()),
                CLONE_IDLE,
                |p: Progress| {
                    on_step(Step::Downloading {
                        phase: p.phase,
                        percent: p.percent,
                    })
                },
            )
            .await
    }

    /// Clone this repository, if it is cloned from now on, with its commits
    /// and trees but not the contents of every file it ever had
    /// (`--filter=blob:none`). git fetches a file's contents when a checkout
    /// or a diff reads them, so a review costs what its files weigh rather
    /// than what the whole history does. For a repository too large to
    /// download whole: a read deep in history becomes a round trip to
    /// GitHub, and a reviewer's snapshot of it cannot read what was never
    /// fetched.
    pub fn clone_partially(&self, id: &RepoId) {
        if let Ok(mut partial) = self.partial.lock() {
            partial.insert(id.full_name().to_lowercase());
        }
    }

    fn clones_partially(&self, id: &RepoId) -> bool {
        self.partial
            .lock()
            .is_ok_and(|partial| partial.contains(&id.full_name().to_lowercase()))
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
        let _cloning = self.cloning.lock(id.full_name().to_lowercase()).await;
        if self.store.has_bare_clone(id) {
            return Ok(bare);
        }

        if let Some(parent) = bare.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io(e.to_string()))?;
        }
        // A clone that failed halfway would be taken for a whole one, so
        // build it under a temporary name and move it into place only on
        // success.
        let staging = bare.with_extension("git.partial");
        let _ = std::fs::remove_dir_all(&staging);
        // A clone dropped before it ends, as one for an open the reader
        // left is, kills git, which cleans up after nothing; the partial
        // directory, gigabytes for a large repository, goes with it here.
        let _partial = RemoveOnDrop(&staging);

        let mut args = vec![
            "clone".to_string(),
            "--bare".to_string(),
            "--progress".to_string(),
        ];
        if self.clones_partially(id) {
            args.push("--filter=blob:none".to_string());
        }
        args.push(id.clone_url());
        args.push(staging.to_string_lossy().into_owned());
        self.git
            .run_with_progress(None, args, CLONE_IDLE, |p: Progress| {
                on_step(Step::Cloning {
                    phase: p.phase,
                    percent: p.percent,
                })
            })
            .await?;

        std::fs::rename(&staging, &bare).map_err(|e| Error::Io(e.to_string()))?;
        Ok(bare)
    }

    /// The repository's default branch, as the remote reports it.
    ///
    /// `refs/remotes/origin/HEAD` in the bare clone answers without the
    /// network when something has set it; a bare clone does not get one
    /// from `git clone`, so the first answer is one `ls-remote` round trip,
    /// which the clone then remembers. A home fetch that fails forgets it,
    /// so a renamed default branch is asked about again. A remote whose
    /// HEAD is not a branch, or an empty repository, gets `main`, which is
    /// not remembered. A remote that cannot be reached is an error, because
    /// the fetch that follows would fail on it anyway and this says so
    /// sooner.
    pub async fn default_branch(&self, id: &RepoId) -> Result<String, Error> {
        let bare = self.store.bare_path(id);
        let has_bare = self.store.has_bare_clone(id);

        if has_bare {
            if let Ok(target) = self
                .git
                .run(
                    Some(&bare),
                    ["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
                    QUERY_TIMEOUT,
                )
                .await
            {
                if let Some(name) = target.trim().strip_prefix("refs/remotes/origin/") {
                    if !name.is_empty() {
                        return Ok(name.to_string());
                    }
                }
            }
            if let Ok(name) = self
                .git
                .run(
                    Some(&bare),
                    ["config", "--get", DEFAULT_BRANCH_KEY],
                    QUERY_TIMEOUT,
                )
                .await
            {
                let name = name.trim();
                if !name.is_empty() {
                    return Ok(name.to_string());
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
            .run(
                cwd,
                ["ls-remote", "--symref", &remote, "HEAD"],
                FETCH_TIMEOUT,
            )
            .await?;

        let Some(name) = parse_symref_head(&out) else {
            return Ok("main".to_string());
        };
        if has_bare {
            if let Err(e) = self
                .git
                .run(
                    Some(&bare),
                    ["config", DEFAULT_BRANCH_KEY, &name],
                    QUERY_TIMEOUT,
                )
                .await
            {
                tracing::debug!(error = %e, "could not remember the default branch");
            }
        }
        Ok(name)
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
        let _homes = self.homes.lock(id.full_name().to_lowercase()).await;
        let fetched = home_ref(default_branch);

        let fetch = self
            .git
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
            .await;
        if let Err(e) = fetch {
            // The branch may be gone, renamed on GitHub: the next open asks
            // the remote again rather than repeating what it remembered.
            let _ = self
                .git
                .run(
                    Some(&bare),
                    ["config", "--unset", DEFAULT_BRANCH_KEY],
                    QUERY_TIMEOUT,
                )
                .await;
            return Err(e);
        }

        let home = self.store.home_path(id);
        self.fill_blobs(&bare, &fetched, &mut |_| {}).await?;

        if !home.join(".git").exists() {
            std::fs::create_dir_all(self.store.homes_dir())
                .map_err(|e| Error::Io(e.to_string()))?;
            // As in `prepare`: a directory an interrupted run left behind,
            // or a registration of one that is gone, would make `worktree
            // add` refuse.
            let _ = std::fs::remove_dir_all(&home);
            let _ = self
                .git
                .run(Some(&bare), ["worktree", "prune"], WORKTREE_TIMEOUT)
                .await;

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
            .run(
                Some(&home),
                ["merge", "--ff-only", &fetched],
                WORKTREE_TIMEOUT,
            )
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

        let worktree = self
            .add_worktree(repo, id, &branch, &["-b", &branch], from)
            .await?;
        Ok(Cut { branch, worktree })
    }

    /// Whether a new branch can't have this name: `repo` has a branch of
    /// it, a worktree sits at the path it maps to, or the caller has it
    /// spoken for.
    pub async fn branch_taken(
        &self,
        repo: &Path,
        id: &RepoId,
        branch: &str,
        reserved: &[String],
    ) -> bool {
        reserved.iter().any(|r| r == branch)
            || self.branch_exists(repo, branch).await
            || self.store.worktree_path(id, branch).join(".git").exists()
    }

    /// The branches of one of the reader's clones that new work can start
    /// from: its local branches and `remote`'s, newest first, `base`
    /// first of all. No network.
    pub async fn local_bases(
        &self,
        repo: &Path,
        remote: Option<&str>,
        base: &str,
    ) -> Result<Vec<BaseBranch>, Error> {
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
        let ssh = batch_ssh(&plain, repo).await;
        plain
            .run(
                Some(repo),
                [
                    "-c".to_string(),
                    ssh,
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

    /// Push the worktree's HEAD to `branch` on `remote`, never forcing.
    ///
    /// `own_credentials` is for a worktree of one of the reader's clones:
    /// their credential helpers and SSH key, never Perch's token, SSH in
    /// batch mode as `fetch_branch` has it, and the branch set to track
    /// what it was pushed to, so a `git push` in a terminal goes to the
    /// same place. Otherwise the worktree is of Perch's own clone, and the
    /// push goes with Perch's git and token, and tracks nothing. A branch
    /// the remote already has at HEAD pushes nothing and succeeds.
    pub async fn push_branch(
        &self,
        worktree: &Path,
        remote: &str,
        branch: &str,
        own_credentials: bool,
    ) -> Result<(), PushError> {
        let refspec = format!("HEAD:refs/heads/{branch}");
        let pushed = if own_credentials {
            let plain = Git::plain();
            let ssh = batch_ssh(&plain, worktree).await;
            plain
                .run(
                    Some(worktree),
                    ["-c", &ssh, "push", "--set-upstream", remote, &refspec],
                    FETCH_TIMEOUT,
                )
                .await
        } else {
            self.git
                .run(Some(worktree), ["push", remote, &refspec], FETCH_TIMEOUT)
                .await
        };
        pushed.map(|_| ()).map_err(PushError::from_git)
    }

    /// How long ago a ref's commit was made, as git says it: `3 days ago`.
    /// For a notice about a fetch that failed.
    pub async fn age_of(&self, repo: &Path, rev: &str) -> Option<String> {
        self.git
            .run(
                Some(repo),
                ["log", "-1", "--format=%cr", rev, "--"],
                QUERY_TIMEOUT,
            )
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
            .run(
                Some(checkout),
                ["worktree", "list", "--porcelain"],
                QUERY_TIMEOUT,
            )
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
            return Err(Error::Io(format!(
                "{} is already a checkout",
                worktree.display()
            )));
        }
        std::fs::create_dir_all(self.store.worktrees_dir())
            .map_err(|e| Error::Io(e.to_string()))?;
        // Not a checkout, so anything at the path is a leftover from an
        // interrupted run, and empty of git's knowledge.
        if worktree.exists() {
            std::fs::remove_dir_all(&worktree).map_err(|e| Error::Io(e.to_string()))?;
        }
        let _ = self
            .git
            .run(Some(repo), ["worktree", "prune"], WORKTREE_TIMEOUT)
            .await;

        self.fill_blobs(repo, commitish, &mut |_| {}).await?;
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
    async fn came_from_pull(
        &self,
        worktree: &Path,
        on_disk: &str,
        head: &str,
        previous: Option<&str>,
    ) -> bool {
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
            .run(
                Some(repo),
                ["merge-base", "--is-ancestor", a, b],
                QUERY_TIMEOUT,
            )
            .await
            .is_ok()
    }

    /// The commit a ref names, or `None` when there is no such ref.
    async fn resolve(&self, repo: &Path, name: &str) -> Option<String> {
        self.git
            .run(
                Some(repo),
                [
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{name}^{{commit}}"),
                ],
                QUERY_TIMEOUT,
            )
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    async fn branch_exists(&self, bare: &Path, branch: &str) -> bool {
        self.git
            .run(
                Some(bare),
                [
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}"),
                ],
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
    pub async fn fetch_pull(
        &self,
        bare: &Path,
        from: &str,
        number: u64,
        base_ref: &str,
    ) -> Result<(), Error> {
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
    pub async fn has_pull(&self, bare: &Path, number: u64, head_sha: &str, base_ref: &str) -> bool {
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
    ///
    /// `status` passes over edits inside a submodule (see
    /// `guard_submodules` in `cmd.rs`), so a submodule whose directory
    /// holds a `.git` counts as dirty whatever is in it.
    pub async fn is_dirty(&self, worktree: &Path) -> Result<bool, Error> {
        let status = self
            .git
            .run(Some(worktree), ["status", "--porcelain"], QUERY_TIMEOUT)
            .await?;
        if !status.trim().is_empty() {
            return Ok(true);
        }
        let staged = self
            .git
            .run(Some(worktree), ["ls-files", "--stage", "-z"], QUERY_TIMEOUT)
            .await?;
        Ok(staged
            .split('\0')
            .filter_map(|entry| entry.strip_prefix("160000 "))
            .filter_map(|entry| entry.split_once('\t'))
            .any(|(_, path)| worktree.join(path).join(".git").symlink_metadata().is_ok()))
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

    /// Record a worktree as it is, as a commit, without touching it, as
    /// [`commit_worktree`] does. The commit is kept at `reference`, a ref
    /// under `refs/perch/`, so `gc` keeps it.
    pub async fn record_worktree(
        &self,
        worktree: &Path,
        reference: &str,
        message: &str,
    ) -> Result<String, Error> {
        let commit = commit_worktree(&self.git, worktree, message).await?;
        self.git
            .run(
                Some(worktree),
                ["update-ref", reference, &commit],
                QUERY_TIMEOUT,
            )
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
    pub async fn change_count(
        &self,
        repo: &Path,
        from: &str,
        to: &str,
    ) -> Result<(u32, u32, u32), Error> {
        let out = self
            .git
            .run(
                Some(repo),
                ["diff", "--numstat", "--no-renames", from, to],
                QUERY_TIMEOUT,
            )
            .await?;
        let mut counts = (0, 0, 0);
        for line in out.lines() {
            let mut parts = line.split('\t');
            let added = parts
                .next()
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(0);
            let removed = parts
                .next()
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(0);
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
        entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect()
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
            .run(
                Some(bare),
                ["show", &format!("{commit}:{path}")],
                QUERY_TIMEOUT,
            )
            .await
    }

    pub fn git(&self) -> &Git {
        &self.git
    }
}

/// `core.sshCommand=…` for a command in one of the reader's clones: the
/// SSH they have set, in `GIT_SSH_COMMAND` or the clone's config, in batch
/// mode, so a key that wants a passphrase or a host not yet known fails the
/// command instead of waiting on a prompt nobody can see.
async fn batch_ssh(plain: &Git, repo: &Path) -> String {
    let ssh = std::env::var("GIT_SSH_COMMAND")
        .ok()
        .filter(|s| !s.is_empty());
    let ssh = match ssh {
        Some(ssh) => ssh,
        None => plain
            .run(
                Some(repo),
                ["config", "--get", "core.sshCommand"],
                QUERY_TIMEOUT,
            )
            .await
            .map(|s| s.trim().to_string())
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "ssh".to_string()),
    };
    format!("core.sshCommand={ssh} -o BatchMode=yes")
}

/// Why a push did not go.
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    /// The remote's branch has commits HEAD doesn't: a push would have to
    /// force, and Perch never does.
    #[error("the remote's branch has commits this one doesn't")]
    Rejected,
    /// The remote wouldn't take the credentials, or there were none that
    /// could be given without a prompt.
    #[error("could not authenticate: {0}")]
    Auth(String),
    #[error(transparent)]
    Other(Error),
}

impl PushError {
    /// Told apart by what git says, which is English whatever the
    /// reader's locale: `Git` runs it with `LC_ALL=C`.
    fn from_git(error: Error) -> Self {
        let Error::Git { stderr, .. } = &error else {
            return Self::Other(error);
        };
        let said = stderr.to_ascii_lowercase();
        if said.contains("[rejected]") || said.contains("non-fast-forward") {
            return Self::Rejected;
        }
        const AUTH: [&str; 8] = [
            "permission denied",
            "authentication failed",
            "could not read username",
            "could not read password",
            "terminal prompts disabled",
            "host key verification failed",
            "invalid username or password",
            "returned error: 403",
        ];
        if AUTH.iter().any(|needle| said.contains(needle)) {
            return Self::Auth(stderr.clone());
        }
        Self::Other(error)
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
        let labels: Vec<String> = order_bases(&refs, None, "main")
            .iter()
            .map(BaseBranch::label)
            .collect();
        assert_eq!(labels, ["main", "b", "a"]);
    }

    fn git(cwd: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// A bare remote with `main`, and a clone of it on `feature` one
    /// commit past `main`.
    fn clone_with_feature(dir: &Path) -> (PathBuf, PathBuf) {
        let remote = dir.join("remote.git");
        let seed = dir.join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        git(
            dir,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                remote.to_str().unwrap(),
            ],
        );
        git(&seed, &["init", "-q", "-b", "main"]);
        git(&seed, &["config", "user.name", "Perch Test"]);
        git(&seed, &["config", "user.email", "perch@example.com"]);
        std::fs::write(seed.join("a.txt"), "a\n").unwrap();
        git(&seed, &["add", "-A"]);
        git(&seed, &["commit", "-q", "-m", "first"]);
        git(&seed, &["push", "-q", remote.to_str().unwrap(), "main"]);
        let clone = dir.join("clone");
        git(
            dir,
            &[
                "clone",
                "-q",
                remote.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        git(&clone, &["config", "user.name", "Perch Test"]);
        git(&clone, &["config", "user.email", "perch@example.com"]);
        git(&clone, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(clone.join("b.txt"), "b\n").unwrap();
        git(&clone, &["add", "-A"]);
        git(&clone, &["commit", "-q", "-m", "Add b", "-m", "Because b."]);
        (remote, clone)
    }

    fn preparer(dir: &Path) -> Preparer {
        Preparer::new(Store::new(dir.join("store")), None)
    }

    /// The objects `rev`'s tree has that `repo` does not.
    fn missing(repo: &Path, rev: &str) -> usize {
        git(
            repo,
            &[
                "rev-list",
                "--objects",
                "--missing=print",
                &format!("{rev}^{{tree}}"),
            ],
        )
        .lines()
        .filter(|line| line.starts_with('?'))
        .count()
    }

    /// A partial clone gets a tree's files ahead of a checkout, saying how
    /// it goes; a full clone is asked for nothing.
    #[tokio::test]
    async fn a_partial_clone_downloads_a_trees_files_before_its_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let (remote, _clone) = clone_with_feature(dir.path());
        git(&remote, &["config", "uploadpack.allowFilter", "true"]);
        git(
            &remote,
            &["config", "uploadpack.allowAnySHA1InWant", "true"],
        );
        let url = format!("file://{}", remote.display());
        let partial = dir.path().join("partial.git");
        let full = dir.path().join("full.git");
        for (args, to) in [(vec!["--filter=blob:none"], &partial), (Vec::new(), &full)] {
            let mut clone = vec!["clone", "-q", "--bare"];
            clone.extend(args);
            clone.extend([url.as_str(), to.to_str().unwrap()]);
            git(dir.path(), &clone);
        }
        assert_eq!(missing(&partial, "main"), 1, "a.txt is not here yet");

        let preparer = preparer(dir.path());
        let mut steps = Vec::new();
        preparer
            .fill_blobs(&partial, "main", &mut |step| steps.push(step))
            .await
            .unwrap();
        assert_eq!(missing(&partial, "main"), 0);
        assert!(
            steps
                .iter()
                .any(|step| matches!(step, Step::Downloading { .. })),
            "{steps:?}"
        );

        let mut steps = Vec::new();
        preparer
            .fill_blobs(&full, "main", &mut |step| steps.push(step))
            .await
            .unwrap();
        assert!(steps.is_empty(), "{steps:?}");
    }

    #[tokio::test]
    async fn a_push_with_the_readers_git_sets_the_upstream_and_again_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let (remote, clone) = clone_with_feature(dir.path());
        let preparer = preparer(dir.path());

        preparer
            .push_branch(&clone, "origin", "feature", true)
            .await
            .unwrap();

        assert_eq!(
            git(&remote, &["rev-parse", "refs/heads/feature"]),
            git(&clone, &["rev-parse", "HEAD"])
        );
        assert_eq!(
            git(&clone, &["rev-parse", "--abbrev-ref", "feature@{upstream}"]),
            "origin/feature"
        );
        preparer
            .push_branch(&clone, "origin", "feature", true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_push_with_perchs_git_tracks_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (remote, clone) = clone_with_feature(dir.path());

        preparer(dir.path())
            .push_branch(&clone, "origin", "feature", false)
            .await
            .unwrap();

        assert_eq!(
            git(&remote, &["rev-parse", "refs/heads/feature"]),
            git(&clone, &["rev-parse", "HEAD"])
        );
        let upstream = std::process::Command::new("git")
            .current_dir(&clone)
            .args(["rev-parse", "--abbrev-ref", "feature@{upstream}"])
            .output()
            .unwrap();
        assert!(!upstream.status.success());
    }

    #[tokio::test]
    async fn a_push_the_remote_is_ahead_of_is_refused_and_not_forced() {
        let dir = tempfile::tempdir().unwrap();
        let (remote, clone) = clone_with_feature(dir.path());
        // Someone else's commit on the remote's feature, one this clone
        // doesn't have.
        let other = dir.path().join("other");
        git(
            dir.path(),
            &[
                "clone",
                "-q",
                remote.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        );
        git(&other, &["config", "user.name", "Other"]);
        git(&other, &["config", "user.email", "other@example.com"]);
        git(&other, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(other.join("c.txt"), "c\n").unwrap();
        git(&other, &["add", "-A"]);
        git(&other, &["commit", "-q", "-m", "Add c"]);
        git(&other, &["push", "-q", "origin", "feature"]);
        let theirs = git(&other, &["rev-parse", "HEAD"]);

        let pushed = preparer(dir.path())
            .push_branch(&clone, "origin", "feature", true)
            .await;

        assert!(matches!(pushed, Err(PushError::Rejected)), "{pushed:?}");
        assert_eq!(git(&remote, &["rev-parse", "refs/heads/feature"]), theirs);
    }

    #[test]
    fn what_git_says_tells_a_refusal_from_a_credential_from_the_rest() {
        let said = |stderr: &str| {
            PushError::from_git(Error::Git {
                status: 1,
                stderr: stderr.into(),
            })
        };
        assert!(matches!(
            said(" ! [rejected]        HEAD -> feature (fetch first)"),
            PushError::Rejected
        ));
        assert!(matches!(
            said("git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository."),
            PushError::Auth(_)
        ));
        assert!(matches!(
            said("fatal: could not read Username for 'https://github.com': terminal prompts disabled"),
            PushError::Auth(_)
        ));
        assert!(matches!(
            said(" ! [remote rejected] HEAD -> feature (protected branch hook declined)"),
            PushError::Other(_)
        ));
        assert!(matches!(
            PushError::from_git(Error::Timeout),
            PushError::Other(Error::Timeout)
        ));
    }

    #[tokio::test]
    async fn readiness_counts_the_commits_the_dirt_and_names_the_branch() {
        let dir = tempfile::tempdir().unwrap();
        let (_remote, clone) = clone_with_feature(dir.path());
        std::fs::write(clone.join("a.txt"), "changed\n").unwrap();
        std::fs::write(clone.join("new.txt"), "new\n").unwrap();
        git(&clone, &["commit", "-q", "--allow-empty", "-m", "Second"]);

        let ready = crate::branch_readiness(&Git::plain(), &clone, "origin/main")
            .await
            .unwrap();

        assert_eq!(ready.ahead, 2);
        assert_eq!(ready.dirty_files, 2);
        assert_eq!(ready.head_branch.as_deref(), Some("feature"));
        assert_eq!(
            ready.commits,
            [
                ("Add b".to_string(), "Because b.".to_string()),
                ("Second".to_string(), String::new()),
            ]
        );

        git(&clone, &["checkout", "-q", "--detach"]);
        let detached = crate::branch_readiness(&Git::plain(), &clone, "origin/main")
            .await
            .unwrap();
        assert_eq!(detached.head_branch, None);
    }

    #[test]
    fn home_refs_sit_in_their_own_namespace() {
        assert_eq!(home_ref("main"), "refs/perch/home/main");
        assert!(!home_ref("main").starts_with("refs/heads/"));
    }
}
