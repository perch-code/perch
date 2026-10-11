//! Every scope the menu offers, against a real git repository in a temp dir.
//!
//! The repository has a base on `main` and a `topic` branch three commits
//! over it, then a staged edit, an unstaged edit on top of it and an
//! untracked file, so that `HEAD`, the index and the working tree all
//! differ and each scope has something of its own to list.

use std::path::{Path, PathBuf};

use perch_git::cmd::Git;
use perch_git::{changed_files, dirt, log, read_side, ChangeStatus, ChangedFile, Rev, Scope};

use crate::support::git;

fn write(dir: &Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    git: Git,
    /// `main`'s tip, which is the merge base.
    base: String,
    /// The three commits on `topic`, oldest first.
    commits: [String; 3],
    /// `topic`'s tip.
    head: String,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);

        write(&repo, "a.txt", "one\ntwo\nthree\n");
        write(&repo, "b.txt", "b\n");
        write(&repo, "gone.txt", "gone\n");
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        let base = git(&repo, &["rev-parse", "HEAD"]);

        git(&repo, &["checkout", "-q", "-b", "topic"]);
        // first: edit a line, add a file.
        write(&repo, "a.txt", "one\nTWO\nthree\n");
        write(&repo, "new.txt", "new\n");
        git(&repo, &["add", "."]);
        git(
            &repo,
            &[
                "commit",
                "-q",
                "-m",
                "first",
                "-m",
                "Why the first.\n\nAnd more.",
            ],
        );
        let first = git(&repo, &["rev-parse", "HEAD"]);
        // second: delete a file.
        git(&repo, &["rm", "-q", "gone.txt"]);
        git(&repo, &["commit", "-q", "-m", "second"]);
        let second = git(&repo, &["rev-parse", "HEAD"]);
        // third: rename a file, append a line.
        git(&repo, &["mv", "b.txt", "c.txt"]);
        write(&repo, "a.txt", "one\nTWO\nthree\nfour\n");
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "third"]);
        let third = git(&repo, &["rev-parse", "HEAD"]);

        // A staged edit, an unstaged edit over it, an unstaged edit to
        // another file, and an untracked file.
        write(&repo, "new.txt", "new\nstaged\n");
        git(&repo, &["add", "new.txt"]);
        write(&repo, "new.txt", "new\nstaged\nunstaged\n");
        write(&repo, "a.txt", "one\nTWO\nthree\nfour\nfive\n");
        write(&repo, "scratch.txt", "x\ny\n");

        Self {
            _tmp: tmp,
            repo,
            git: Git::new(None),
            base,
            commits: [first, second, third.clone()],
            head: third,
        }
    }

    async fn files(&self, scope: Scope) -> Vec<ChangedFile> {
        changed_files(&self.git, &self.repo, &scope, &self.base, &self.head)
            .await
            .unwrap_or_else(|e| panic!("{scope}: {e}"))
    }

    /// The blob a tree or the index has at `path`.
    fn blob(&self, spec: &str) -> String {
        git(&self.repo, &["rev-parse", spec])
    }

    /// The blob the file on disk would be added as.
    fn disk_blob(&self, path: &str) -> String {
        git(&self.repo, &["hash-object", path])
    }
}

/// `(path, status, old path, additions, deletions)`, the rail's columns.
fn rows(files: &[ChangedFile]) -> Vec<(&str, ChangeStatus, Option<&str>, u32, u32)> {
    files
        .iter()
        .map(|f| {
            (
                f.path.as_str(),
                f.status,
                f.old_path.as_deref(),
                f.additions,
                f.deletions,
            )
        })
        .collect()
}

fn blob_of<'a>(files: &'a [ChangedFile], path: &str) -> Option<&'a str> {
    files
        .iter()
        .find(|f| f.path == path)
        .expect(path)
        .head_blob
        .as_deref()
}

#[tokio::test]
async fn the_branch_scope_lists_everything_since_the_merge_base() {
    let f = Fixture::new();
    let files = f.files(Scope::Branch).await;

    assert_eq!(
        rows(&files),
        vec![
            ("a.txt", ChangeStatus::Modified, None, 2, 1),
            ("c.txt", ChangeStatus::Renamed, Some("b.txt"), 0, 0),
            ("gone.txt", ChangeStatus::Deleted, None, 0, 1),
            ("new.txt", ChangeStatus::Added, None, 1, 0),
        ]
    );
    assert_eq!(
        blob_of(&files, "new.txt"),
        Some(f.blob("HEAD:new.txt").as_str())
    );
    assert_eq!(
        blob_of(&files, "a.txt"),
        Some(f.blob("HEAD:a.txt").as_str())
    );
    assert_eq!(
        blob_of(&files, "c.txt"),
        Some(f.blob("HEAD:c.txt").as_str())
    );
    assert_eq!(
        blob_of(&files, "gone.txt"),
        None,
        "a deletion has no head side"
    );
    assert!(files.iter().all(|f| !f.binary));
}

#[tokio::test]
async fn since_last_looked_lists_only_what_the_later_commits_changed() {
    let f = Fixture::new();
    let files = f
        .files(Scope::SinceLastLooked {
            from: f.commits[0].clone(),
        })
        .await;

    assert_eq!(
        rows(&files),
        vec![
            ("a.txt", ChangeStatus::Modified, None, 1, 0),
            ("c.txt", ChangeStatus::Renamed, Some("b.txt"), 0, 0),
            ("gone.txt", ChangeStatus::Deleted, None, 0, 1),
        ],
        "new.txt was added before the reader last looked"
    );

    let none = f
        .files(Scope::SinceLastLooked {
            from: f.head.clone(),
        })
        .await;
    assert!(none.is_empty(), "the head has not moved");
}

#[tokio::test]
async fn a_commit_scope_lists_that_commit_against_its_parent() {
    let f = Fixture::new();

    let first = f
        .files(Scope::Commit {
            sha: f.commits[0].clone(),
        })
        .await;
    assert_eq!(
        rows(&first),
        vec![
            ("a.txt", ChangeStatus::Modified, None, 1, 1),
            ("new.txt", ChangeStatus::Added, None, 1, 0),
        ]
    );
    assert_eq!(
        blob_of(&first, "a.txt"),
        Some(f.blob(&format!("{}:a.txt", f.commits[0])).as_str()),
        "the blob is the commit's, not HEAD's"
    );
    assert_ne!(
        blob_of(&first, "a.txt"),
        Some(f.blob("HEAD:a.txt").as_str())
    );

    let second = f
        .files(Scope::Commit {
            sha: f.commits[1].clone(),
        })
        .await;
    assert_eq!(
        rows(&second),
        vec![("gone.txt", ChangeStatus::Deleted, None, 0, 1)]
    );

    let third = f
        .files(Scope::Commit {
            sha: f.commits[2].clone(),
        })
        .await;
    assert_eq!(
        rows(&third),
        vec![
            ("a.txt", ChangeStatus::Modified, None, 1, 0),
            ("c.txt", ChangeStatus::Renamed, Some("b.txt"), 0, 0),
        ]
    );
}

#[tokio::test]
async fn uncommitted_is_the_working_tree_against_head_with_untracked_files_added() {
    let f = Fixture::new();
    let files = f.files(Scope::Uncommitted).await;

    assert_eq!(
        rows(&files),
        vec![
            ("a.txt", ChangeStatus::Modified, None, 1, 0),
            ("new.txt", ChangeStatus::Modified, None, 2, 0),
            ("scratch.txt", ChangeStatus::Added, None, 2, 0),
        ]
    );
    // The head side is the disk, so the blobs are what the files would be
    // added as, not what HEAD or the index has.
    for path in ["a.txt", "new.txt", "scratch.txt"] {
        assert_eq!(
            blob_of(&files, path),
            Some(f.disk_blob(path).as_str()),
            "{path}"
        );
    }
    assert_ne!(
        blob_of(&files, "new.txt"),
        Some(f.blob("HEAD:new.txt").as_str())
    );
    assert_ne!(
        blob_of(&files, "new.txt"),
        Some(f.blob(":new.txt").as_str())
    );
}

#[tokio::test]
async fn unstaged_is_the_working_tree_against_the_index() {
    let f = Fixture::new();
    let files = f.files(Scope::Unstaged).await;

    assert_eq!(
        rows(&files),
        vec![
            ("a.txt", ChangeStatus::Modified, None, 1, 0),
            ("new.txt", ChangeStatus::Modified, None, 1, 0),
            ("scratch.txt", ChangeStatus::Added, None, 2, 0),
        ],
        "only the line past the staged one counts for new.txt"
    );
    assert_eq!(
        blob_of(&files, "new.txt"),
        Some(f.disk_blob("new.txt").as_str())
    );
}

#[tokio::test]
async fn staged_is_the_index_against_head() {
    let f = Fixture::new();
    let files = f.files(Scope::Staged).await;

    assert_eq!(
        rows(&files),
        vec![("new.txt", ChangeStatus::Modified, None, 1, 0)]
    );
    assert_eq!(
        blob_of(&files, "new.txt"),
        Some(f.blob(":new.txt").as_str()),
        "the head side is the index"
    );
}

#[tokio::test]
async fn a_file_unchanged_on_disk_keeps_the_blob_its_commit_has() {
    // Viewed is keyed by blob: a file the reader has not touched must
    // hash the same under Uncommitted as under Branch, or a check on it
    // would come and go with the scope.
    let f = Fixture::new();
    write(&f.repo, "c.txt", "b\nedited\n");

    let branch = f.files(Scope::Branch).await;
    let uncommitted = f.files(Scope::Uncommitted).await;

    assert_ne!(blob_of(&uncommitted, "c.txt"), blob_of(&branch, "c.txt"));
    git(&f.repo, &["checkout", "--", "c.txt"]);
    let uncommitted = f.files(Scope::Uncommitted).await;
    assert!(
        !uncommitted.iter().any(|f| f.path == "c.txt"),
        "undone, so no longer listed"
    );
    // And the same content, via the disk, hashes to the same blob.
    assert_eq!(f.disk_blob("c.txt"), f.blob("HEAD:c.txt"));
}

#[tokio::test]
async fn a_clean_checkout_has_nothing_uncommitted() {
    let f = Fixture::new();
    git(&f.repo, &["add", "-A"]);
    git(&f.repo, &["commit", "-q", "-m", "tidy"]);

    assert!(f.files(Scope::Uncommitted).await.is_empty());
    assert!(f.files(Scope::Unstaged).await.is_empty());
    assert!(f.files(Scope::Staged).await.is_empty());
}

#[tokio::test]
async fn an_untracked_binary_file_is_marked_and_not_counted() {
    let f = Fixture::new();
    std::fs::write(f.repo.join("logo.png"), b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();

    let files = f.files(Scope::Uncommitted).await;
    let logo = files.iter().find(|f| f.path == "logo.png").expect("listed");
    assert_eq!(logo.status, ChangeStatus::Added);
    assert!(logo.binary);
    assert_eq!(logo.additions, 0);
    assert_eq!(
        logo.head_blob.as_deref(),
        Some(f.disk_blob("logo.png").as_str())
    );
}

#[tokio::test]
async fn sha_scopes_read_the_same_from_a_linked_worktree() {
    let f = Fixture::new();
    let linked = f._tmp.path().join("linked");
    git(
        &f.repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            linked.to_str().unwrap(),
            &f.head,
        ],
    );

    let from_repo = f.files(Scope::Branch).await;
    let from_linked = changed_files(&f.git, &linked, &Scope::Branch, &f.base, &f.head)
        .await
        .unwrap();
    assert_eq!(from_linked, from_repo, "the objects are shared");

    let uncommitted = changed_files(&f.git, &linked, &Scope::Uncommitted, &f.base, &f.head)
        .await
        .unwrap();
    assert!(
        uncommitted.is_empty(),
        "the linked worktree's own tree is clean"
    );
}

#[tokio::test]
async fn a_scope_over_a_revision_that_does_not_exist_is_an_error() {
    let f = Fixture::new();
    let err = changed_files(
        &f.git,
        &f.repo,
        &Scope::Commit {
            sha: "0badf00d".into(),
        },
        &f.base,
        &f.head,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, perch_git::Error::Git { .. }), "{err}");
}

#[tokio::test]
async fn log_lists_the_commits_over_the_base_newest_first() {
    let f = Fixture::new();
    let commits = log(&f.git, &f.repo, &f.base, &f.head).await.unwrap();

    let shas: Vec<&str> = commits.iter().map(|c| c.sha.as_str()).collect();
    assert_eq!(
        shas,
        vec![
            f.commits[2].as_str(),
            f.commits[1].as_str(),
            f.commits[0].as_str()
        ]
    );
    let subjects: Vec<&str> = commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, vec!["third", "second", "first"]);
    assert!(commits.iter().all(|c| c.author == "t"));
    assert_eq!(commits[0].body, "");
    assert_eq!(commits[2].body, "Why the first.\n\nAnd more.");
    assert!(commits[0].authored_at >= commits[2].authored_at);

    assert!(
        log(&f.git, &f.repo, &f.head, &f.head)
            .await
            .unwrap()
            .is_empty(),
        "nothing over itself"
    );
    assert!(log(&f.git, &f.repo, "0badf00d", &f.head).await.is_err());
}

#[tokio::test]
async fn dirt_tells_staged_from_unstaged_and_counts_untracked_as_unstaged() {
    let f = Fixture::new();
    let d = dirt(&f.git, &f.repo).await.unwrap();
    assert!(d.staged, "new.txt has a staged edit");
    assert!(d.unstaged, "a.txt has an unstaged edit");

    git(&f.repo, &["add", "-A"]);
    let d = dirt(&f.git, &f.repo).await.unwrap();
    assert!(d.staged);
    assert!(
        !d.unstaged,
        "everything, the untracked file included, is staged now"
    );

    git(&f.repo, &["commit", "-q", "-m", "tidy"]);
    let d = dirt(&f.git, &f.repo).await.unwrap();
    assert!(!d.any(), "a clean checkout");

    write(&f.repo, "another.txt", "x\n");
    let d = dirt(&f.git, &f.repo).await.unwrap();
    assert!(d.unstaged, "an untracked file is unstaged work");
    assert!(!d.staged);
}

#[tokio::test]
async fn read_side_reads_head_the_index_and_the_disk_apart() {
    let f = Fixture::new();
    let read = |rev: Rev, path: &'static str| {
        let git = &f.git;
        let repo = &f.repo;
        async move { read_side(git, repo, &rev, path).await.unwrap() }
    };

    assert_eq!(
        read(Rev::Sha("HEAD".into()), "new.txt").await.as_deref(),
        Some("new\n")
    );
    assert_eq!(
        read(Rev::Index, "new.txt").await.as_deref(),
        Some("new\nstaged\n")
    );
    assert_eq!(
        read(Rev::Worktree, "new.txt").await.as_deref(),
        Some("new\nstaged\nunstaged\n")
    );

    // A commit's parent, as a Commit scope spells it.
    assert_eq!(
        read(Rev::Sha(format!("{}^", f.commits[0])), "a.txt")
            .await
            .as_deref(),
        Some("one\ntwo\nthree\n")
    );
    assert_eq!(
        read(Rev::Sha(f.base.clone()), "gone.txt").await.as_deref(),
        Some("gone\n")
    );
}

#[tokio::test]
async fn a_side_without_the_file_reads_as_none() {
    let f = Fixture::new();

    // Deleted on the branch: in the base, not in HEAD, the index or on disk.
    assert_eq!(
        read_side(&f.git, &f.repo, &Rev::Sha("HEAD".into()), "gone.txt")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        read_side(&f.git, &f.repo, &Rev::Index, "gone.txt")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        read_side(&f.git, &f.repo, &Rev::Worktree, "gone.txt")
            .await
            .unwrap(),
        None
    );
    // Untracked: on disk, and nowhere else.
    assert_eq!(
        read_side(&f.git, &f.repo, &Rev::Sha("HEAD".into()), "scratch.txt")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        read_side(&f.git, &f.repo, &Rev::Index, "scratch.txt")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        read_side(&f.git, &f.repo, &Rev::Worktree, "scratch.txt")
            .await
            .unwrap()
            .as_deref(),
        Some("x\ny\n")
    );
    // Never anywhere.
    assert_eq!(
        read_side(
            &f.git,
            &f.repo,
            &Rev::Sha(f.base.clone()),
            "nope/deeper.txt"
        )
        .await
        .unwrap(),
        None
    );

    // A bad revision is an error, not a missing file.
    let err = read_side(&f.git, &f.repo, &Rev::Sha("0badf00d".into()), "a.txt")
        .await
        .unwrap_err();
    assert!(matches!(err, perch_git::Error::Git { .. }), "{err}");
}
