//! Worktrees on branches, against a real git repository in a temp dir.
//!
//! The store expects to clone from GitHub, so these tests pre-create the
//! bare clone from a local "remote" instead. `prepare` then finds it, skips
//! cloning, and fetches from the local path the clone recorded as `origin`.
//! The pull request ref that GitHub would serve is written into the remote
//! by hand.

use std::path::PathBuf;

use perch_git::{Preparer, RepoId, Store};

use crate::support::git;

struct Fixture {
    _tmp: tempfile::TempDir,
    remote: PathBuf,
    store: Store,
    id: RepoId,
    preparer: Preparer,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote");
        std::fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "-q", "-b", "main"]);
        std::fs::write(remote.join("a.txt"), "one\n").unwrap();
        git(&remote, &["add", "."]);
        git(&remote, &["commit", "-q", "-m", "one"]);

        let store = Store::new(tmp.path().join("store"));
        let id = RepoId::new("acme", "widgets");
        let bare = store.bare_path(&id);
        std::fs::create_dir_all(bare.parent().unwrap()).unwrap();
        git(
            tmp.path(),
            &[
                "clone",
                "-q",
                "--bare",
                remote.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );

        let preparer = Preparer::new(store.clone(), None);
        Self {
            _tmp: tmp,
            remote,
            store,
            id,
            preparer,
        }
    }

    /// A new commit on the remote's main, published as pull request 7's head.
    fn push_commit(&self, name: &str) -> String {
        std::fs::write(self.remote.join(format!("{name}.txt")), name).unwrap();
        git(&self.remote, &["add", "."]);
        git(&self.remote, &["commit", "-q", "-m", name]);
        let sha = git(&self.remote, &["rev-parse", "HEAD"]);
        git(&self.remote, &["update-ref", "refs/pull/7/head", &sha]);
        sha
    }

    async fn prepare(&self, head: &str) -> perch_git::Checkout {
        self.preparer
            .prepare(&self.id, 7, head, "main", "perch/pr-7", |_| {})
            .await
            .expect("prepare")
    }

    /// Prepare against `base`, saying whether it fetched.
    async fn prepare_on(&self, head: &str, base: &str) -> (perch_git::Checkout, bool) {
        let mut fetched = false;
        let checkout = self
            .preparer
            .prepare(&self.id, 7, head, base, "perch/pr-7", |step| {
                fetched |= step == perch_git::Step::Fetching;
            })
            .await
            .expect("prepare");
        (checkout, fetched)
    }

    fn bare(&self) -> PathBuf {
        self.store.bare_path(&self.id)
    }

    fn branches(&self) -> String {
        git(&self.bare(), &["branch", "--list", "perch/*"])
    }
}

#[tokio::test]
async fn a_fresh_worktree_is_created_on_its_own_branch() {
    let f = Fixture::new();
    let head = f.push_commit("pr");

    let checkout = f.prepare(&head).await;

    assert_eq!(checkout.branch, "perch/pr-7");
    assert_eq!(checkout.head_sha, head);
    assert_eq!(checkout.stale, None);
    assert!(checkout.worktree.join("a.txt").is_file());
    assert_eq!(
        git(&checkout.worktree, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "perch/pr-7",
        "HEAD is attached to the branch, not detached"
    );
    assert!(f.branches().contains("perch/pr-7"), "{}", f.branches());
}

#[tokio::test]
async fn a_head_github_never_published_is_fetched_by_commit() {
    // GitHub sometimes serves only `refs/pull/N/merge` for a new pull
    // request; the head is still reachable on its branch.
    let f = Fixture::new();
    git(&f.remote, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(f.remote.join("pr.txt"), "pr").unwrap();
    git(&f.remote, &["add", "."]);
    git(&f.remote, &["commit", "-q", "-m", "pr"]);
    let head = git(&f.remote, &["rev-parse", "HEAD"]);
    git(&f.remote, &["checkout", "-q", "main"]);

    let checkout = f.prepare(&head).await;

    assert_eq!(checkout.head_sha, head);
    assert!(checkout.worktree.join("pr.txt").is_file());
    assert!(f.preparer.has_pull(&f.bare(), 7, &head, "main").await);
}

#[tokio::test]
async fn opening_again_reuses_the_directory() {
    let f = Fixture::new();
    let head = f.push_commit("pr");

    let first = f.prepare(&head).await;
    let marker = first.worktree.join(".perch-marker");
    // A file git does not know about, so the second prepare is asked to
    // leave the directory alone rather than recreate it.
    std::fs::write(&marker, "here").unwrap();

    let second = f.prepare(&head).await;
    assert_eq!(first.worktree, second.worktree);
    assert!(marker.exists(), "the directory was recreated, not reused");
}

#[tokio::test]
async fn opening_a_head_already_fetched_does_not_fetch_again() {
    let f = Fixture::new();
    let head = f.push_commit("pr");
    let (_, fetched) = f.prepare_on(&head, "main").await;
    assert!(fetched, "the first open fetches");

    // With the remote gone, a fetch would fail.
    std::fs::rename(&f.remote, f.remote.with_extension("gone")).unwrap();
    let (checkout, fetched) = f.prepare_on(&head, "main").await;
    assert!(!fetched);
    assert_eq!(checkout.head_sha, head);
}

#[tokio::test]
async fn a_retargeted_pull_request_fetches_its_new_base() {
    let f = Fixture::new();
    let first = git(&f.remote, &["rev-parse", "HEAD"]);
    git(&f.remote, &["branch", "old", &first]);
    let head = f.push_commit("pr");

    let (on_main, _) = f.prepare_on(&head, "main").await;
    assert_eq!(on_main.merge_base, head, "main has the head");

    let (on_old, fetched) = f.prepare_on(&head, "old").await;
    assert!(fetched, "a new base is fetched though the head is the same");
    assert_eq!(on_old.merge_base, first);

    let (back, fetched) = f.prepare_on(&head, "main").await;
    assert!(
        fetched,
        "the old base's ref is not what refs/perch/base holds now"
    );
    assert_eq!(back.merge_base, head);
}

#[tokio::test]
async fn a_clean_worktree_follows_a_moved_head() {
    let f = Fixture::new();
    let one = f.push_commit("pr");
    f.prepare(&one).await;

    let two = f.push_commit("more");
    let checkout = f.prepare(&two).await;

    assert_eq!(checkout.head_sha, two);
    assert_eq!(checkout.stale, None);
    assert_eq!(git(&checkout.worktree, &["rev-parse", "HEAD"]), two);
    assert_eq!(
        git(&f.bare(), &["rev-parse", "refs/heads/perch/pr-7"]),
        two,
        "the branch moved with the reset"
    );
}

#[tokio::test]
async fn a_dirty_worktree_is_left_alone_and_says_so() {
    let f = Fixture::new();
    let one = f.push_commit("pr");
    let checkout = f.prepare(&one).await;
    std::fs::write(checkout.worktree.join("scratch.txt"), "work in progress").unwrap();

    let two = f.push_commit("more");
    let kept = f.prepare(&two).await;

    assert_eq!(kept.head_sha, one, "the older head is what is on disk");
    assert_eq!(kept.stale.as_deref(), Some(two.as_str()));
    assert!(
        kept.worktree.join("scratch.txt").exists(),
        "the uncommitted file survived"
    );

    // Once the work is gone, the next open catches up.
    std::fs::remove_file(kept.worktree.join("scratch.txt")).unwrap();
    let caught_up = f.prepare(&two).await;
    assert_eq!(caught_up.head_sha, two);
    assert_eq!(caught_up.stale, None);
}

#[tokio::test]
async fn dirt_means_any_change_including_untracked_files() {
    let f = Fixture::new();
    let head = f.push_commit("pr");
    let checkout = f.prepare(&head).await;

    assert!(!f.preparer.is_dirty(&checkout.worktree).await.unwrap());
    std::fs::write(checkout.worktree.join("new.txt"), "x").unwrap();
    assert!(f.preparer.is_dirty(&checkout.worktree).await.unwrap());
    // Even where the reader's config hides untracked files from status.
    git(
        &checkout.worktree,
        &["config", "status.showUntrackedFiles", "no"],
    );
    assert!(f.preparer.is_dirty(&checkout.worktree).await.unwrap());
}

#[tokio::test]
async fn archiving_keeps_the_branch_and_restoring_needs_no_clone() {
    let f = Fixture::new();
    let head = f.push_commit("pr");
    let checkout = f.prepare(&head).await;

    f.preparer
        .remove_worktree(&f.bare(), &checkout.worktree)
        .await
        .unwrap();
    assert!(!checkout.worktree.exists());
    assert!(
        f.branches().contains("perch/pr-7"),
        "archiving must keep the branch: {}",
        f.branches()
    );
    assert!(
        !git(&f.bare(), &["worktree", "list"]).contains("perch-pr-7"),
        "git's record of the directory was pruned"
    );

    // Restore: the branch already exists, so it is checked out as-is.
    let restored = f.prepare(&head).await;
    assert_eq!(restored.worktree, checkout.worktree);
    assert!(restored.worktree.join("a.txt").is_file());
    assert_eq!(
        git(&restored.worktree, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "perch/pr-7"
    );
}

#[tokio::test]
async fn forgetting_deletes_the_branch() {
    let f = Fixture::new();
    let head = f.push_commit("pr");
    let checkout = f.prepare(&head).await;
    f.preparer
        .remove_worktree(&f.bare(), &checkout.worktree)
        .await
        .unwrap();

    f.preparer
        .delete_branch(&f.bare(), "perch/pr-7")
        .await
        .unwrap();
    assert!(!f.branches().contains("perch/pr-7"));
}

#[tokio::test]
async fn every_directory_is_listed_whether_or_not_it_is_known() {
    let f = Fixture::new();
    let head = f.push_commit("pr");
    let checkout = f.prepare(&head).await;
    let stray = f
        .store
        .worktrees_dir()
        .join("acme__widgets__old-commit-layout");
    std::fs::create_dir_all(&stray).unwrap();

    let mut dirs = f.preparer.worktree_dirs();
    dirs.sort();
    let mut expected = vec![checkout.worktree.clone(), stray];
    expected.sort();
    assert_eq!(dirs, expected);
}

#[tokio::test]
async fn a_clean_worktree_with_a_commit_of_its_own_is_not_reset() {
    let f = Fixture::new();
    let one = f.push_commit("pr");
    let checkout = f.prepare(&one).await;
    std::fs::write(checkout.worktree.join("mine.txt"), "mine").unwrap();
    git(&checkout.worktree, &["add", "."]);
    git(&checkout.worktree, &["commit", "-q", "-m", "mine"]);
    let mine = git(&checkout.worktree, &["rev-parse", "HEAD"]);

    // Nothing moved on GitHub: the reader's commit is still not the head.
    let same = f.prepare(&one).await;
    assert_eq!(same.head_sha, mine);
    assert_eq!(same.stale.as_deref(), Some(one.as_str()));

    let two = f.push_commit("more");
    let kept = f.prepare(&two).await;
    assert_eq!(
        kept.head_sha, mine,
        "the unpushed commit is still checked out"
    );
    assert_eq!(kept.stale.as_deref(), Some(two.as_str()));
    assert_eq!(git(&kept.worktree, &["rev-parse", "HEAD"]), mine);
}

#[tokio::test]
async fn a_clean_worktree_follows_a_force_push() {
    let f = Fixture::new();
    let base = git(&f.remote, &["rev-parse", "HEAD"]);
    let one = f.push_commit("pr");
    f.prepare(&one).await;

    // Rewrite the pull request: the old head is behind nothing new.
    git(&f.remote, &["reset", "-q", "--hard", &base]);
    let rewritten = f.push_commit("rewritten");
    let checkout = f.prepare(&rewritten).await;

    assert_eq!(checkout.head_sha, rewritten);
    assert_eq!(checkout.stale, None);
    assert_eq!(git(&checkout.worktree, &["rev-parse", "HEAD"]), rewritten);
}

#[tokio::test]
async fn commits_only_on_counts_what_deleting_the_branch_loses() {
    let f = Fixture::new();
    let main = git(&f.bare(), &["rev-parse", "refs/heads/main"]);
    let cut = f
        .preparer
        .cut_branch(&f.bare(), &f.id, "chris/work", &main, &[])
        .await
        .unwrap();
    assert_eq!(
        f.preparer
            .commits_only_on(&f.bare(), &cut.branch)
            .await
            .unwrap(),
        0
    );

    for name in ["one", "two"] {
        std::fs::write(cut.worktree.join(name), name).unwrap();
        git(&cut.worktree, &["add", "."]);
        git(&cut.worktree, &["commit", "-q", "-m", name]);
    }
    assert_eq!(
        f.preparer
            .commits_only_on(&f.bare(), &cut.branch)
            .await
            .unwrap(),
        2,
        "the worktree's own HEAD does not count as holding them"
    );
    // Nor does a Work run's record, a commit on top of HEAD that is
    // dropped with the item.
    f.preparer
        .record_worktree(&cut.worktree, "refs/perch/runs/r1/after", "after r1")
        .await
        .unwrap();
    assert_eq!(
        f.preparer
            .commits_only_on(&f.bare(), &cut.branch)
            .await
            .unwrap(),
        2,
        "a run's record does not count as holding them"
    );

    // Another branch at the first commit keeps it.
    git(&cut.worktree, &["branch", "keep", "HEAD~1"]);
    assert_eq!(
        f.preparer
            .commits_only_on(&f.bare(), &cut.branch)
            .await
            .unwrap(),
        1
    );
    // So does a ref of Perch's own, such as a pull request's head.
    let head = git(&cut.worktree, &["rev-parse", "HEAD"]);
    git(&f.bare(), &["update-ref", "refs/perch/pull/9/head", &head]);
    assert_eq!(
        f.preparer
            .commits_only_on(&f.bare(), &cut.branch)
            .await
            .unwrap(),
        0
    );
    assert!(f.preparer.has_branch(&f.bare(), &cut.branch).await.unwrap());
    assert!(!f.preparer.has_branch(&f.bare(), "nope").await.unwrap());
}
