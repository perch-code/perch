//! New work cut from one of the reader's own clones rather than the
//! store's bare clone: a repository with no remote at all, and branches
//! checked out as they are.

use std::path::{Path, PathBuf};

use perch_git::{Preparer, RepoId, Store};

use crate::support::git;

fn commit(dir: &Path, name: &str) -> String {
    std::fs::write(dir.join(name), name).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", name]);
    git(dir, &["rev-parse", "HEAD"])
}

/// A repository with no remote, as `git init` leaves one, and a store
/// that has never cloned anything.
struct Local {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    store: Store,
    id: RepoId,
    preparer: Preparer,
}

impl Local {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("notes");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        commit(&root, "one");
        let root = root.canonicalize().unwrap();
        let store = Store::new(tmp.path().join("store"));
        let preparer = Preparer::new(store.clone(), None);
        Self {
            _tmp: tmp,
            root,
            store,
            id: RepoId::new("local", "notes"),
            preparer,
        }
    }

    fn worktrees(&self) -> String {
        git(&self.root, &["worktree", "list", "--porcelain"])
    }
}

#[tokio::test]
async fn a_branch_is_cut_in_the_root_and_checked_out_under_the_store() {
    let l = Local::new();
    let main = git(&l.root, &["rev-parse", "main"]);

    let cut = l
        .preparer
        .cut_branch(&l.root, &l.id, "chris/readme", "main", &[])
        .await
        .unwrap();

    assert_eq!(cut.branch, "chris/readme");
    assert!(
        cut.worktree.starts_with(l.store.worktrees_dir()),
        "{}",
        cut.worktree.display()
    );
    assert_eq!(
        cut.worktree.file_name().unwrap().to_string_lossy(),
        "local__notes__chris-readme"
    );
    assert_eq!(git(&cut.worktree, &["rev-parse", "HEAD"]), main);
    assert_eq!(
        git(&cut.worktree, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "chris/readme"
    );
    assert_eq!(
        git(&l.root, &["rev-parse", "refs/heads/chris/readme"]),
        main,
        "the branch lives in the root"
    );
    assert!(
        l.worktrees().contains("branch refs/heads/chris/readme"),
        "{}",
        l.worktrees()
    );
    assert!(!l.store.root().join("repos").exists(), "nothing was cloned");
    assert_eq!(
        l.preparer.repository_of(&cut.worktree).await.unwrap(),
        l.root,
        "the worktree knows its repository"
    );
}

#[tokio::test]
async fn a_taken_name_in_the_root_gets_the_next_one() {
    let l = Local::new();
    git(&l.root, &["branch", "fix"]);

    let cut = l
        .preparer
        .cut_branch(&l.root, &l.id, "fix", "main", &[])
        .await
        .unwrap();

    assert_eq!(cut.branch, "fix-2");
}

#[tokio::test]
async fn one_branch_is_fetched_into_its_remote_tracking_ref() {
    let l = Local::new();
    let clone = l.root.parent().unwrap().join("clone");
    git(
        l.root.parent().unwrap(),
        &[
            "clone",
            "-q",
            l.root.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    let moved = commit(&l.root, "two");
    git(&l.root, &["branch", "other"]);

    l.preparer
        .fetch_branch(&clone, "origin", "main")
        .await
        .unwrap();

    assert_eq!(git(&clone, &["rev-parse", "origin/main"]), moved);
    assert!(
        git(&clone, &["branch", "-r", "--list", "origin/other"]).is_empty(),
        "only the one branch"
    );
    let bases = l
        .preparer
        .local_bases(&clone, Some("origin"), "main")
        .await
        .unwrap();
    assert_eq!(
        bases.iter().map(|b| b.label()).collect::<Vec<_>>(),
        ["origin/main", "main"]
    );
}

#[tokio::test]
async fn a_fetch_that_wants_a_password_fails_rather_than_asking() {
    use std::io::{Read, Write};
    // A server that asks every request to sign in.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(8) {
            let Ok(mut stream) = stream else { break };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"x\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    let l = Local::new();
    git(
        &l.root,
        &[
            "remote",
            "add",
            "origin",
            &format!("http://127.0.0.1:{port}/acme/widgets.git"),
        ],
    );

    let started = std::time::Instant::now();
    let err = l
        .preparer
        .fetch_branch(&l.root, "origin", "main")
        .await
        .unwrap_err();

    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "it failed at once: {err}"
    );
}

#[tokio::test]
async fn a_worktree_is_recorded_as_a_commit_without_being_touched() {
    let l = Local::new();
    commit(&l.root, "gone");
    let cut = l
        .preparer
        .cut_branch(&l.root, &l.id, "work", "main", &[])
        .await
        .unwrap();
    let wt = &cut.worktree;
    std::fs::write(wt.join("staged"), "staged").unwrap();
    git(wt, &["add", "staged"]);
    let head = git(wt, &["rev-parse", "HEAD"]);
    let index_before = git(wt, &["diff", "--cached", "--name-status"]);

    let before = l
        .preparer
        .record_worktree(wt, "refs/perch/runs/r1/before", "before r1")
        .await
        .unwrap();
    std::fs::write(wt.join("one"), "one\nchanged\n").unwrap();
    std::fs::write(wt.join("new.txt"), "new\n").unwrap();
    std::fs::remove_file(wt.join("gone")).unwrap();
    std::fs::write(wt.join(".gitignore"), "target/\n").unwrap();
    std::fs::create_dir_all(wt.join("target")).unwrap();
    std::fs::write(wt.join("target/big"), "ignored").unwrap();
    let after = l
        .preparer
        .record_worktree(wt, "refs/perch/runs/r1/after", "after r1")
        .await
        .unwrap();

    assert_eq!(
        git(wt, &["rev-parse", "HEAD"]),
        head,
        "HEAD is where it was"
    );
    assert_eq!(
        git(wt, &["diff", "--cached", "--name-status"]),
        index_before,
        "the worktree's own index is as it was"
    );
    assert_eq!(
        git(&l.root, &["rev-parse", "refs/perch/runs/r1/before"]),
        before
    );
    assert_eq!(git(&l.root, &["rev-parse", &format!("{before}^")]), head);
    let changed = git(
        &l.root,
        &["diff", "--name-status", "--no-renames", &before, &after],
    );
    let mut lines: Vec<&str> = changed.lines().collect();
    lines.sort();
    assert_eq!(lines, ["A\t.gitignore", "A\tnew.txt", "D\tgone", "M\tone"]);
    // `one` had no newline at its end, so its one line is replaced.
    assert_eq!(
        l.preparer
            .change_count(&l.root, &before, &after)
            .await
            .unwrap(),
        (4, 4, 2)
    );

    l.preparer
        .drop_refs(
            &l.root,
            &[
                "refs/perch/runs/r1/before".into(),
                "refs/perch/runs/r1/after".into(),
            ],
        )
        .await;
    assert!(git(&l.root, &["for-each-ref", "refs/perch/"]).is_empty());
}
