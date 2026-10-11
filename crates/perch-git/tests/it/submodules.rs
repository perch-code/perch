//! A submodule someone else populated, against a real git repository.
//!
//! A Work run can move a directory holding a `.git` onto a gitlink path,
//! and plain `git status` in the superproject then runs git in there under
//! that repository's config. The nested repository here has a
//! `core.fsmonitor` that leaves a mark, and the superproject's
//! `.gitmodules` asks for the submodule's dirt to be checked, which
//! outranks `diff.ignoreSubmodules`.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use perch_git::cmd::Git;
use perch_git::{branch_readiness, changed_files, dirt, Preparer, Scope, Store};

use crate::support::git;

struct Fixture {
    tmp: tempfile::TempDir,
    repo: PathBuf,
    head: String,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let hook = tmp.path().join("hook.sh");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\necho ran >> {}\nexit 1\n",
                tmp.path().join("ran").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

        let nested = tmp.path().join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        git(&nested, &["init", "-q", "-b", "main"]);
        std::fs::write(nested.join("f.txt"), "f\n").unwrap();
        git(&nested, &["add", "."]);
        git(&nested, &["commit", "-q", "-m", "nested"]);
        let pinned = git(&nested, &["rev-parse", "HEAD"]);

        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        std::fs::write(
            repo.join(".gitmodules"),
            "[submodule \"sub\"]\n\tpath = sub\n\turl = ../nested\n\tignore = none\n",
        )
        .unwrap();
        git(&repo, &["add", "."]);
        git(
            &repo,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{pinned},sub"),
            ],
        );
        git(&repo, &["commit", "-q", "-m", "one"]);
        let head = git(&repo, &["rev-parse", "HEAD"]);

        // Populated at the pinned commit, with an edit in it.
        git(&repo, &["clone", "-q", nested.to_str().unwrap(), "sub"]);
        git(
            &repo.join("sub"),
            &["config", "core.fsmonitor", hook.to_str().unwrap()],
        );
        std::fs::write(repo.join("sub/f.txt"), "edited\n").unwrap();

        Self { tmp, repo, head }
    }

    /// Whether the nested repository's hook has run since the last call.
    fn ran(&self) -> bool {
        std::fs::remove_file(self.tmp.path().join("ran")).is_ok()
    }

    fn preparer(&self) -> Preparer {
        Preparer::new(Store::new(self.tmp.path().join("store")), None)
    }
}

#[tokio::test]
async fn perch_never_runs_git_inside_a_populated_submodule() {
    let f = Fixture::new();
    git(&f.repo, &["status", "--porcelain"]);
    assert!(
        f.ran(),
        "plain git status runs the nested repository's fsmonitor"
    );

    let git = Git::plain();
    dirt(&git, &f.repo).await.unwrap();
    changed_files(&git, &f.repo, &Scope::Uncommitted, &f.head, &f.head)
        .await
        .unwrap();
    branch_readiness(&git, &f.repo, "HEAD").await.unwrap();
    let preparer = f.preparer();
    preparer.is_dirty(&f.repo).await.unwrap();
    git.run(
        Some(&f.repo),
        ["--no-optional-locks", "status", "--porcelain"],
        std::time::Duration::from_secs(30),
    )
    .await
    .unwrap();
    git.run(
        Some(&f.repo),
        ["-c", "core.quotePath=false", "diff", "HEAD"],
        std::time::Duration::from_secs(30),
    )
    .await
    .unwrap();
    assert!(!f.ran(), "some call ran git in the submodule");
}

#[tokio::test]
async fn edits_inside_a_submodule_go_unreported_but_still_keep_the_worktree() {
    let f = Fixture::new();
    let git = Git::plain();
    assert!(
        !dirt(&git, &f.repo).await.unwrap().any(),
        "git does not look inside the submodule"
    );
    assert!(
        f.preparer().is_dirty(&f.repo).await.unwrap(),
        "a populated submodule may hold work, so nothing resets or removes it"
    );

    // Unpopulated, as a worktree Perch adds leaves it, the gitlink is clean.
    std::fs::remove_dir_all(f.repo.join("sub")).unwrap();
    std::fs::create_dir(f.repo.join("sub")).unwrap();
    assert!(!f.preparer().is_dirty(&f.repo).await.unwrap());

    // A new commit in there is still a change.
    git_submodule_ahead(&f);
    assert!(
        dirt(&git, &f.repo).await.unwrap().unstaged,
        "the submodule's HEAD moved"
    );
}

/// Populate the submodule again, one commit past the pinned one.
fn git_submodule_ahead(f: &Fixture) {
    std::fs::remove_dir(f.repo.join("sub")).unwrap();
    git(
        &f.repo,
        &[
            "clone",
            "-q",
            f.tmp.path().join("nested").to_str().unwrap(),
            "sub",
        ],
    );
    git(
        &f.repo.join("sub"),
        &["commit", "-q", "--allow-empty", "-m", "ahead"],
    );
}
