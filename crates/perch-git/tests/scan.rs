//! Root scans, home checkouts and branch cuts, against real git repositories
//! in a temp dir.
//!
//! The "remote" is a local repository. A root is a clone of it with two
//! linked worktrees, which is what a reader's working directory looks like.
//! The store's bare clone is made from the same remote, as in
//! `worktree_branches.rs`, so `ensure_home` and `cut_branch` fetch from a
//! path rather than GitHub. Where a root's remote is given a GitHub URL it
//! is only ever read back as text; nothing fetches from it.

use std::path::{Path, PathBuf};
use std::process::Command;

use perch_git::cmd::Git;
use perch_git::{branch_churn, head_commit_time, home_ref, scan_root, Preparer, RepoId, Store};

const GITHUB: &[&str] = &["github.com"];

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, name: &str) -> String {
    std::fs::write(dir.join(format!("{name}.txt")), format!("{name}\n")).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", name]);
    git(dir, &["rev-parse", "HEAD"])
}

/// A remote on `default`, with one commit.
fn make_remote(tmp: &Path, default: &str) -> PathBuf {
    let remote = tmp.join("remote");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "-q", "-b", default]);
    commit(&remote, "one");
    remote
}

// ---- the root scan ------------------------------------------------------

struct RootFixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    one: PathBuf,
    two: PathBuf,
    git: Git,
}

impl RootFixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let remote = make_remote(tmp.path(), "main");

        let root = tmp.path().join("root");
        git(
            tmp.path(),
            &[
                "clone",
                "-q",
                remote.to_str().unwrap(),
                root.to_str().unwrap(),
            ],
        );
        // The remote is a path; make `origin` look like the GitHub remote a
        // real root has, so the scan has a name to parse. Nothing fetches
        // from it after this.
        git(
            &root,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
        );

        let one = tmp.path().join("root-one");
        let two = tmp.path().join("root-two");
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature/one",
                one.to_str().unwrap(),
            ],
        );
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature/two",
                two.to_str().unwrap(),
            ],
        );

        Self {
            _tmp: tmp,
            root,
            one,
            two,
            git: Git::new(None),
        }
    }
}

/// A repository made with `git init -b <branch>` and one commit, with a
/// linked worktree on `feature/local`: the reader's own project, never
/// pushed anywhere. It has no remote at all.
struct LocalFixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    feature: PathBuf,
    git: Git,
}

impl LocalFixture {
    fn new(branch: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("local");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", branch]);
        commit(&root, "one");

        let feature = tmp.path().join("local-feature");
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature/local",
                feature.to_str().unwrap(),
            ],
        );

        Self {
            _tmp: tmp,
            root,
            feature,
            git: Git::new(None),
        }
    }
}

#[tokio::test]
async fn a_root_lists_itself_and_its_worktrees_with_their_branches() {
    let f = RootFixture::new();

    let scan = scan_root(&f.git, &f.root, None, GITHUB).await.expect("scan");

    assert_eq!(scan.github.as_deref(), Some("acme/widgets"));
    assert_eq!(scan.remote.as_deref(), Some("origin"));
    assert_eq!(scan.base, "main", "set by git clone in origin/HEAD");
    assert_eq!(scan.base_ref, "origin/main");

    let branches: Vec<_> = scan.entries.iter().map(|e| e.branch.as_deref()).collect();
    assert_eq!(
        branches,
        vec![Some("main"), Some("feature/one"), Some("feature/two")]
    );

    let main = &scan.entries[0];
    assert!(main.is_main);
    assert_eq!(
        main.path.canonicalize().unwrap(),
        f.root.canonicalize().unwrap()
    );
    assert_eq!(main.head_sha, git(&f.root, &["rev-parse", "HEAD"]));
    assert!(scan.entries[1..].iter().all(|e| !e.is_main));
    assert_eq!(
        scan.entries[1].path.canonicalize().unwrap(),
        f.one.canonicalize().unwrap()
    );
    assert_eq!(
        scan.entries[2].path.canonicalize().unwrap(),
        f.two.canonicalize().unwrap()
    );
}

#[tokio::test]
async fn scanning_from_a_linked_worktree_finds_the_same_set() {
    let f = RootFixture::new();

    let from_root = scan_root(&f.git, &f.root, None, GITHUB).await.unwrap();
    let from_worktree = scan_root(&f.git, &f.one, None, GITHUB).await.unwrap();

    assert_eq!(
        from_root, from_worktree,
        "a root may be a worktree, not the clone"
    );
}

#[tokio::test]
async fn a_detached_worktree_has_no_branch() {
    let f = RootFixture::new();
    let detached = f._tmp.path().join("root-detached");
    git(
        &f.root,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            detached.to_str().unwrap(),
            "HEAD",
        ],
    );

    let scan = scan_root(&f.git, &f.root, None, GITHUB).await.unwrap();
    let entry = scan
        .entries
        .iter()
        .find(|e| e.path.canonicalize().unwrap() == detached.canonicalize().unwrap())
        .expect("the detached worktree is listed");
    assert_eq!(entry.branch, None);
    assert_eq!(entry.head_sha, git(&f.root, &["rev-parse", "HEAD"]));
}

#[tokio::test]
async fn a_directory_that_is_not_a_checkout_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();

    let err = scan_root(&Git::new(None), &plain, None, GITHUB).await.unwrap_err();
    assert!(matches!(err, perch_git::Error::Git { .. }), "{err}");

    let missing = tmp.path().join("missing");
    assert!(scan_root(&Git::new(None), &missing, None, GITHUB).await.is_err());
}

#[tokio::test]
async fn a_root_without_an_origin_scans_as_local() {
    let f = RootFixture::new();
    git(&f.root, &["remote", "remove", "origin"]);

    let scan = scan_root(&f.git, &f.root, None, GITHUB).await.unwrap();
    assert_eq!(scan.github, None);
    assert_eq!(scan.remote, None);
    assert_eq!(scan.base, "main", "the local branch, with origin/HEAD gone");
    assert_eq!(scan.base_ref, "main");
    assert_eq!(scan.entries.len(), 3);
}

#[tokio::test]
async fn a_repository_with_no_remote_scans_as_local_on_main() {
    // The design's "no GitHub call is made" case. There is no remote to
    // name a repository and no token in `Git::new(None)`; scan_root only
    // reads the worktree list, the remote config and refs already on disk,
    // so nothing here can reach the network.
    let f = LocalFixture::new("main");

    let scan = scan_root(&f.git, &f.root, None, GITHUB).await.expect("scan");

    assert_eq!(scan.github, None);
    assert_eq!(scan.remote, None);
    assert_eq!(scan.base, "main");
    assert_eq!(scan.base_ref, "main");
    let branches: Vec<_> = scan.entries.iter().map(|e| e.branch.as_deref()).collect();
    assert_eq!(branches, vec![Some("main"), Some("feature/local")]);
    assert_eq!(
        scan.entries[1].path.canonicalize().unwrap(),
        f.feature.canonicalize().unwrap()
    );
}

#[tokio::test]
async fn a_repository_with_no_remote_on_master_has_master_as_its_base() {
    let f = LocalFixture::new("master");

    let scan = scan_root(&f.git, &f.root, None, GITHUB).await.expect("scan");

    assert_eq!(scan.github, None);
    assert_eq!(scan.remote, None);
    assert_eq!(scan.base, "master");
}

#[tokio::test]
async fn a_base_from_config_names_a_branch_that_is_neither_main_nor_master() {
    let f = LocalFixture::new("trunk");

    let scan = scan_root(&f.git, &f.root, Some("trunk"), GITHUB).await.expect("scan");

    assert_eq!(scan.base, "trunk");
    assert_eq!(scan.base_ref, "trunk");
}

#[tokio::test]
async fn a_repository_with_no_remote_head_no_base_and_no_main_or_master_is_an_error() {
    let f = LocalFixture::new("trunk");

    let err = scan_root(&f.git, &f.root, None, GITHUB).await.unwrap_err();

    assert!(matches!(err, perch_git::Error::NoBase { .. }), "{err}");
    let message = err.to_string();
    assert!(message.contains(&f.root.display().to_string()), "{message}");
    assert!(message.contains("neither main nor master"), "{message}");
}

#[tokio::test]
async fn a_fork_is_named_by_upstream_and_its_base_falls_back_to_the_local_branch() {
    let f = RootFixture::new();
    std::fs::write(f.one.join("b.txt"), "x\ny\n").unwrap();
    git(&f.one, &["add", "."]);
    git(&f.one, &["commit", "-q", "-m", "work"]);
    // The reader's copy is origin; the repository it came from is upstream,
    // added but never fetched, so neither refs/remotes/upstream/HEAD nor
    // refs/remotes/upstream/main exists.
    git(
        &f.root,
        &["remote", "set-url", "origin", "https://github.com/me/widgets.git"],
    );
    git(
        &f.root,
        &["remote", "add", "upstream", "https://github.com/acme/widgets.git"],
    );

    let scan = scan_root(&f.git, &f.root, None, GITHUB).await.expect("scan");

    assert_eq!(scan.github.as_deref(), Some("acme/widgets"));
    assert_eq!(scan.remote.as_deref(), Some("upstream"));
    assert_eq!(scan.base, "main", "past the missing upstream/HEAD to refs/heads/main");
    assert_eq!(scan.base_ref, "main", "upstream has no main to diff against yet");
    assert_eq!(branch_churn(&f.git, &f.one, &scan.base_ref).await.unwrap(), (2, 0));

    // The first fetch of upstream brings its main, without an upstream/HEAD
    // on older gits, and the diff moves there.
    let main = git(&f.root, &["rev-parse", "refs/heads/main"]);
    git(&f.root, &["update-ref", "refs/remotes/upstream/main", &main]);

    let scan = scan_root(&f.git, &f.root, None, GITHUB).await.expect("scan");

    assert_eq!(scan.base, "main");
    assert_eq!(scan.base_ref, "upstream/main");
    assert_eq!(branch_churn(&f.git, &f.one, &scan.base_ref).await.unwrap(), (2, 0));
}

#[tokio::test]
async fn a_root_whose_origin_is_elsewhere_scans_as_local() {
    let f = RootFixture::new();
    git(
        &f.root,
        &["remote", "set-url", "origin", "https://gitlab.com/acme/widgets.git"],
    );

    let scan = scan_root(&f.git, &f.root, None, GITHUB).await.expect("scan");

    assert_eq!(scan.github, None);
    assert_eq!(scan.remote, None);
    assert_eq!(scan.base, "main", "origin is not GitHub, so its HEAD does not count");
    assert_eq!(scan.entries.len(), 3);
}

#[tokio::test]
async fn churn_runs_against_the_scan_s_base_ref_for_github_and_local_roots() {
    let f = RootFixture::new();
    std::fs::write(f.one.join("b.txt"), "x\ny\n").unwrap();
    git(&f.one, &["add", "."]);
    git(&f.one, &["commit", "-q", "-m", "work"]);
    let scan = scan_root(&f.git, &f.root, None, GITHUB).await.unwrap();
    assert_eq!(scan.base_ref, "origin/main");
    assert_eq!(branch_churn(&f.git, &f.one, &scan.base_ref).await.unwrap(), (2, 0));

    let l = LocalFixture::new("main");
    std::fs::write(l.feature.join("c.txt"), "x\n").unwrap();
    git(&l.feature, &["add", "."]);
    git(&l.feature, &["commit", "-q", "-m", "work"]);
    let scan = scan_root(&l.git, &l.root, None, GITHUB).await.unwrap();
    assert_eq!(scan.base_ref, "main");
    assert_eq!(branch_churn(&l.git, &l.feature, &scan.base_ref).await.unwrap(), (1, 0));
}

#[tokio::test]
async fn churn_counts_the_branch_s_own_lines_against_its_base() {
    let f = RootFixture::new();
    // Two lines added and one removed on the branch.
    std::fs::write(f.one.join("one.txt"), "").unwrap();
    std::fs::write(f.one.join("b.txt"), "x\ny\n").unwrap();
    git(&f.one, &["add", "."]);
    git(&f.one, &["commit", "-q", "-m", "work"]);
    // A commit on main after the branch left it must not count against it.
    commit(&f.root, "later");

    assert_eq!(branch_churn(&f.git, &f.one, "main").await.unwrap(), (2, 1));
    assert_eq!(branch_churn(&f.git, &f.two, "main").await.unwrap(), (0, 0));
    assert!(
        branch_churn(&f.git, &f.two, "no-such-branch")
            .await
            .is_err(),
        "a base that does not exist is an error, not zero"
    );
}

#[tokio::test]
async fn head_commit_time_is_the_committer_date() {
    let f = RootFixture::new();
    let when = "2024-05-06T07:08:09+02:00";
    let out = Command::new("git")
        .args(["commit", "-q", "--allow-empty", "-m", "dated"])
        .current_dir(&f.two)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env("GIT_COMMITTER_DATE", when)
        .output()
        .unwrap();
    assert!(out.status.success());

    let time = head_commit_time(&f.git, &f.two).await.unwrap();
    assert_eq!(time, when.parse::<jiff::Timestamp>().unwrap());
}

// ---- home checkouts and branch cuts -------------------------------------

struct StoreFixture {
    _tmp: tempfile::TempDir,
    remote: PathBuf,
    store: Store,
    id: RepoId,
    preparer: Preparer,
}

impl StoreFixture {
    /// A remote whose default branch is `default`, and a bare clone of it
    /// in the store.
    fn new(default: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let remote = make_remote(tmp.path(), default);

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

    fn bare(&self) -> PathBuf {
        self.store.bare_path(&self.id)
    }
}

#[tokio::test]
async fn the_default_branch_is_what_the_remote_s_head_points_at() {
    let f = StoreFixture::new("trunk");
    assert_eq!(f.preparer.default_branch(&f.id).await.unwrap(), "trunk");
}

#[tokio::test]
async fn origin_head_in_the_clone_answers_before_the_remote_is_asked() {
    let f = StoreFixture::new("trunk");
    git(
        &f.bare(),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/develop",
        ],
    );
    assert_eq!(f.preparer.default_branch(&f.id).await.unwrap(), "develop");
}

/// Every issue open asked the remote, a second of network each time.
/// The clone keeps the first answer, and forgets it when a home fetch of
/// that branch fails, as it does once the branch is renamed.
#[tokio::test]
async fn the_clone_remembers_the_default_branch_until_its_fetch_fails() {
    let f = StoreFixture::new("trunk");
    assert_eq!(f.preparer.default_branch(&f.id).await.unwrap(), "trunk");

    git(&f.remote, &["branch", "-m", "trunk", "develop"]);
    assert_eq!(
        f.preparer.default_branch(&f.id).await.unwrap(),
        "trunk",
        "answered from the clone, without asking"
    );

    assert!(f.preparer.ensure_home(&f.id, "trunk").await.is_err());
    assert_eq!(f.preparer.default_branch(&f.id).await.unwrap(), "develop");
}

#[tokio::test]
async fn a_remote_with_a_detached_head_gets_main() {
    let f = StoreFixture::new("trunk");
    git(&f.remote, &["checkout", "-q", "--detach"]);
    assert_eq!(f.preparer.default_branch(&f.id).await.unwrap(), "main");
}

#[tokio::test]
async fn a_home_is_a_worktree_on_the_default_branch_under_homes() {
    let f = StoreFixture::new("main");

    let home = f
        .preparer
        .ensure_home(&f.id, "main")
        .await
        .expect("ensure_home");

    assert_eq!(home, f.store.home_path(&f.id));
    assert!(home.starts_with(f.store.homes_dir()));
    assert!(!home.starts_with(f.store.worktrees_dir()));
    assert!(home.join("one.txt").is_file());
    assert_eq!(git(&home, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    assert_eq!(
        git(&home, &["rev-parse", "HEAD"]),
        git(&f.remote, &["rev-parse", "HEAD"])
    );
    assert_eq!(
        git(&f.bare(), &["rev-parse", &home_ref("main")]),
        git(&f.remote, &["rev-parse", "HEAD"]),
        "the fetched tip is kept under refs/perch/home"
    );
    assert!(
        f.preparer.worktree_dirs().is_empty(),
        "a home is not one of the worktrees the fortnight rule sees"
    );
}

#[tokio::test]
async fn a_home_follows_the_default_branch_with_a_fast_forward() {
    let f = StoreFixture::new("main");
    let home = f.preparer.ensure_home(&f.id, "main").await.unwrap();
    let marker = home.join(".perch-marker");
    std::fs::write(&marker, "here").unwrap();

    let two = commit(&f.remote, "two");
    let again = f.preparer.ensure_home(&f.id, "main").await.unwrap();

    assert_eq!(again, home);
    assert!(marker.exists(), "the directory was recreated, not reused");
    assert_eq!(git(&home, &["rev-parse", "HEAD"]), two);
    assert!(home.join("two.txt").is_file());
    assert_eq!(git(&home, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
}

#[tokio::test]
async fn a_home_with_local_changes_in_the_way_is_left_alone() {
    let f = StoreFixture::new("main");
    let home = f.preparer.ensure_home(&f.id, "main").await.unwrap();
    let before = git(&home, &["rev-parse", "HEAD"]);
    // Edit a file the remote's next commit also changes, so the
    // fast-forward would have to overwrite it.
    std::fs::write(home.join("one.txt"), "mine\n").unwrap();
    std::fs::write(f.remote.join("one.txt"), "theirs\n").unwrap();
    git(&f.remote, &["commit", "-q", "-am", "theirs"]);

    let again = f
        .preparer
        .ensure_home(&f.id, "main")
        .await
        .expect("not an error");

    assert_eq!(again, home);
    assert_eq!(
        git(&home, &["rev-parse", "HEAD"]),
        before,
        "not fast-forwarded"
    );
    assert_eq!(
        std::fs::read_to_string(home.join("one.txt")).unwrap(),
        "mine\n"
    );
}

#[tokio::test]
async fn a_home_whose_directory_vanished_is_made_again() {
    let f = StoreFixture::new("main");
    let home = f.preparer.ensure_home(&f.id, "main").await.unwrap();
    std::fs::remove_dir_all(&home).unwrap();

    let again = f.preparer.ensure_home(&f.id, "main").await.unwrap();
    assert_eq!(again, home);
    assert!(home.join("one.txt").is_file());
}

#[tokio::test]
async fn a_cut_branch_starts_at_the_commit_asked_for_in_its_own_worktree() {
    let f = StoreFixture::new("main");
    f.preparer.ensure_home(&f.id, "main").await.unwrap();
    let tip = git(&f.bare(), &["rev-parse", &home_ref("main")]);

    let cut = f
        .preparer
        .cut_branch(&f.bare(), &f.id, "perch/issue-42", &tip, &[])
        .await
        .expect("cut");

    assert_eq!(cut.branch, "perch/issue-42");
    assert_eq!(cut.worktree, f.store.worktree_path(&f.id, "perch/issue-42"));
    assert_eq!(git(&cut.worktree, &["rev-parse", "HEAD"]), tip);
    assert_eq!(
        git(&cut.worktree, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "perch/issue-42"
    );
    assert!(cut.worktree.join("one.txt").is_file());
    assert_eq!(
        git(&f.bare(), &["rev-parse", "refs/heads/perch/issue-42"]),
        tip
    );
}

#[tokio::test]
async fn cutting_the_same_name_twice_numbers_the_second() {
    let f = StoreFixture::new("main");
    f.preparer.ensure_home(&f.id, "main").await.unwrap();
    let from = home_ref("main");

    let first = f
        .preparer
        .cut_branch(&f.bare(), &f.id, "perch/issue-42", &from, &[])
        .await
        .unwrap();
    let second = f
        .preparer
        .cut_branch(&f.bare(), &f.id, "perch/issue-42", &from, &[])
        .await
        .unwrap();
    let third = f
        .preparer
        .cut_branch(&f.bare(), &f.id, "perch/issue-42", &from, &[])
        .await
        .unwrap();

    assert_eq!(first.branch, "perch/issue-42");
    assert_eq!(second.branch, "perch/issue-42-2");
    assert_eq!(third.branch, "perch/issue-42-3");
    assert_ne!(first.worktree, second.worktree);
    assert!(second.worktree.join("one.txt").is_file());
    assert_eq!(
        git(&second.worktree, &["rev-parse", "HEAD"]),
        git(&first.worktree, &["rev-parse", "HEAD"]),
        "a ref name works as the starting point as well as a sha"
    );
}

#[tokio::test]
async fn a_cut_never_removes_a_checkout_at_its_path() {
    // Two branch names map to one directory once slashes become dashes.
    // A worktree already there, with work in it, is someone's; the cut
    // takes the next name rather than clearing the path.
    let f = StoreFixture::new("main");
    f.preparer.ensure_home(&f.id, "main").await.unwrap();
    let from = home_ref("main");

    let first = f.preparer.cut_branch(&f.bare(), &f.id, "perch/issue-42", &from, &[]).await.unwrap();
    std::fs::write(first.worktree.join("draft.txt"), "keep me").unwrap();
    assert_eq!(first.worktree, f.store.worktree_path(&f.id, "perch-issue-42"));

    let second = f.preparer.cut_branch(&f.bare(), &f.id, "perch-issue-42", &from, &[]).await.unwrap();

    assert_eq!(second.branch, "perch-issue-42-2");
    assert_ne!(second.worktree, first.worktree);
    assert_eq!(std::fs::read_to_string(first.worktree.join("draft.txt")).unwrap(), "keep me");
    assert_eq!(git(&first.worktree, &["rev-parse", "--abbrev-ref", "HEAD"]), "perch/issue-42");
}

#[tokio::test]
async fn a_name_the_caller_reserves_is_skipped_like_one_the_clone_has() {
    // A root's branch is a row without being in the store's clone. A
    // fork of it must not take the same name.
    let f = StoreFixture::new("main");
    f.preparer.ensure_home(&f.id, "main").await.unwrap();
    let from = home_ref("main");

    let cut = f
        .preparer
        .cut_branch(&f.bare(), &f.id, "feature", &from, &["feature".to_string(), "feature-2".to_string()])
        .await
        .unwrap();

    assert_eq!(cut.branch, "feature-3");
    assert_eq!(git(&cut.worktree, &["rev-parse", "--abbrev-ref", "HEAD"]), "feature-3");
}

#[tokio::test]
async fn a_fork_can_be_cut_from_a_pull_request_s_head() {
    let f = StoreFixture::new("main");
    // A pull request head the way `prepare` fetches it.
    let head = commit(&f.remote, "pr");
    git(&f.remote, &["update-ref", "refs/pull/7/head", &head]);
    let checkout = f
        .preparer
        .prepare(&f.id, 7, &head, "main", "perch/pr-7", |_| {})
        .await
        .unwrap();

    let fork = f
        .preparer
        .cut_branch(&f.bare(), &f.id, &checkout.branch, &checkout.head_sha, &[])
        .await
        .unwrap();

    assert_eq!(
        fork.branch, "perch/pr-7-2",
        "the source's own branch is taken"
    );
    assert_eq!(git(&fork.worktree, &["rev-parse", "HEAD"]), head);
    assert!(fork.worktree.join("pr.txt").is_file());
}

#[tokio::test]
async fn cutting_without_a_clone_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("store"));
    let id = RepoId::new("acme", "widgets");
    let bare = store.bare_path(&id);
    let preparer = Preparer::new(store, None);
    let err = preparer
        .cut_branch(&bare, &id, "x", "HEAD", &[])
        .await
        .unwrap_err();
    assert!(matches!(err, perch_git::Error::Io(_)), "{err}");
}
