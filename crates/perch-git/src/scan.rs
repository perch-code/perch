//! Finding the branches a project's roots have checked out.
//!
//! A root is a directory the user declared in config: a clone of one of the
//! project's repositories, or a worktree of one. Asking it `git worktree
//! list` gives every worktree of that repository, the root itself included,
//! and each one on a branch other than the default is a branch of the
//! reader's own. Nothing here writes: the scan reads, and the caller decides
//! what the items are.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cmd::Git;
use crate::Error;

const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

/// One line of `git worktree list --porcelain`, as a checkout on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    pub head_sha: String,
    /// The repository's own working directory, the one `git clone` made,
    /// as opposed to a linked worktree.
    pub is_main: bool,
    /// Git still has a record of it but the directory is gone.
    pub prunable: bool,
}

/// What a root has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootScan {
    /// `owner/repo` from the `origin` remote, when it looks like one.
    pub repo_full_name: Option<String>,
    /// From `refs/remotes/origin/HEAD`, which `git clone` sets; `main` when
    /// nothing says otherwise.
    pub default_branch: String,
    pub entries: Vec<WorktreeEntry>,
}

/// Parse `git worktree list --porcelain`.
///
/// Entries are blank-line separated stanzas of `worktree <path>`, `HEAD
/// <sha>`, and one of `branch <ref>`, `detached` or `bare`; `locked` and
/// `prunable` lines may follow; only `prunable` matters. The first stanza is the
/// main worktree. A `bare` stanza is the repository itself and has no
/// working directory, so it is not an entry.
pub fn parse_worktree_list(porcelain: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut first = true;

    for stanza in porcelain.split("\n\n") {
        let mut path = None;
        let mut head = None;
        let mut branch = None;
        let mut bare = false;
        let mut prunable = false;

        for line in stanza.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(unquote(p));
            } else if let Some(sha) = line.strip_prefix("HEAD ") {
                head = Some(sha.trim().to_string());
            } else if let Some(r) = line.strip_prefix("branch ") {
                let r = r.trim();
                branch = Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_string());
            } else if line == "bare" {
                bare = true;
            } else if line == "prunable" || line.starts_with("prunable ") {
                prunable = true;
            }
        }

        let Some(path) = path else { continue };
        let is_main = first;
        first = false;
        if bare {
            continue;
        }
        entries.push(WorktreeEntry {
            path: PathBuf::from(path),
            branch,
            head_sha: head.unwrap_or_default(),
            is_main,
            prunable,
        });
    }

    entries
}

/// Undo the C-style quoting git applies to unusual paths when
/// `core.quotePath` is on: `"caf\303\251"` becomes `café`. A path that is
/// not wrapped in double quotes is taken as it is.
fn unquote(path: &str) -> String {
    let Some(inner) = path.strip_prefix('"').and_then(|p| p.strip_suffix('"')) else {
        return path.to_string();
    };
    let mut bytes = Vec::with_capacity(inner.len());
    let mut chars = inner.bytes().peekable();
    while let Some(b) = chars.next() {
        if b != b'\\' {
            bytes.push(b);
            continue;
        }
        match chars.next() {
            Some(d @ b'0'..=b'7') => {
                let mut value = u32::from(d - b'0');
                for _ in 0..2 {
                    match chars.peek() {
                        Some(&o @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(o - b'0');
                            chars.next();
                        }
                        _ => break,
                    }
                }
                bytes.push(value as u8);
            }
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(other) => bytes.push(other),
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `owner/repo` from a remote URL in any of the shapes GitHub hands out:
/// `https://github.com/o/r.git`, `git@github.com:o/r.git`,
/// `ssh://git@github.com/o/r`. The host is not checked, so a GitHub
/// Enterprise remote parses too; a local path does not.
pub fn repo_from_remote(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    let path = if let Some((scheme, rest)) = url.split_once("://") {
        if scheme == "file" {
            return None;
        }
        // Past the authority, which may carry `user@` and `:port`.
        let (_, path) = rest.split_once('/')?;
        path
    } else if url.starts_with('/') || url.starts_with('.') {
        return None;
    } else if let Some((_, path)) = url.split_once(':') {
        // scp-like: `git@github.com:o/r.git`
        path
    } else {
        return None;
    };

    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

/// Ask a root what it has checked out.
///
/// A root that is not inside a git checkout is an error from the first
/// command. A root with no `origin`, or one whose `origin` is not a
/// `owner/repo` URL, scans with `repo_full_name` unset and the caller says
/// whether that is a problem.
pub async fn scan_root(git: &Git, root: &Path) -> Result<RootScan, Error> {
    let listing = git
        .run(
            Some(root),
            ["worktree", "list", "--porcelain"],
            QUERY_TIMEOUT,
        )
        .await?;
    let entries = parse_worktree_list(&listing);

    let repo_full_name = git
        .run(Some(root), ["remote", "get-url", "origin"], QUERY_TIMEOUT)
        .await
        .ok()
        .and_then(|url| repo_from_remote(&url));

    let default_branch = git
        .run(
            Some(root),
            ["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
            QUERY_TIMEOUT,
        )
        .await
        .ok()
        .and_then(|target| {
            target
                .trim()
                .strip_prefix("refs/remotes/origin/")
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "main".to_string());

    Ok(RootScan {
        repo_full_name,
        default_branch,
        entries,
    })
}

/// Lines added and removed on a worktree's branch since it left `base_ref`:
/// `git diff base...HEAD`, summed over files. Binary files count for
/// nothing. The `+12 −3` a branch row shows.
pub async fn branch_churn(git: &Git, worktree: &Path, base_ref: &str) -> Result<(u32, u32), Error> {
    let out = git
        .run(
            Some(worktree),
            ["diff", "--numstat", &format!("{base_ref}...HEAD")],
            QUERY_TIMEOUT,
        )
        .await?;
    Ok(sum_numstat(&out))
}

fn sum_numstat(out: &str) -> (u32, u32) {
    out.lines().fold((0, 0), |(added, removed), line| {
        let mut fields = line.split('\t');
        let a = fields
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let r = fields
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        (added + a, removed + r)
    })
}

/// When the commit at a worktree's HEAD was made. The last activity of a
/// branch that has no pull request to carry an `updated_at`.
pub async fn head_commit_time(git: &Git, worktree: &Path) -> Result<jiff::Timestamp, Error> {
    let out = git
        .run(Some(worktree), ["log", "-1", "--format=%cI"], QUERY_TIMEOUT)
        .await?;
    let text = out.trim();
    text.parse::<jiff::Timestamp>()
        .map_err(|e| Error::Parse(format!("commit time {text:?}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from git 2.50 on a checkout with a linked worktree on a
    // branch and one detached, followed by a bare repository's own listing.
    const LISTING: &str = "\
worktree /Users/t/Code/widgets
HEAD f8f0e6743e95f05fec95943dacbe0cdf586d82dd
branch refs/heads/main

worktree /Users/t/Code/widgets-detached
HEAD f8f0e6743e95f05fec95943dacbe0cdf586d82dd
detached

worktree /Users/t/Code/widgets-feature
HEAD 004b0093ac55cb81de58e4859227dd657a01ca14
branch refs/heads/feature/x
locked

worktree /Users/t/Code/widgets-gone
HEAD 004b0093ac55cb81de58e4859227dd657a01ca14
branch refs/heads/feature/gone
prunable gitdir file points to non-existent location
";

    const BARE_LISTING: &str = "\
worktree /Users/t/store/widgets.git
bare

worktree /Users/t/store/worktrees/topic
HEAD f8f0e6743e95f05fec95943dacbe0cdf586d82dd
branch refs/heads/topic

";

    #[test]
    fn the_first_stanza_is_the_main_worktree() {
        let entries = parse_worktree_list(LISTING);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].path, PathBuf::from("/Users/t/Code/widgets"));
        assert!(entries[0].is_main);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert_eq!(
            entries[0].head_sha,
            "f8f0e6743e95f05fec95943dacbe0cdf586d82dd"
        );
        assert!(entries[1..].iter().all(|e| !e.is_main));
    }

    #[test]
    fn branches_lose_their_refs_heads_prefix_and_keep_their_slashes() {
        let entries = parse_worktree_list(LISTING);
        assert_eq!(entries[2].branch.as_deref(), Some("feature/x"));
        assert_eq!(
            entries[2].head_sha,
            "004b0093ac55cb81de58e4859227dd657a01ca14"
        );
    }

    #[test]
    fn a_prunable_entry_says_so() {
        let entries = parse_worktree_list(LISTING);
        assert!(entries[3].prunable);
        assert_eq!(entries[3].branch.as_deref(), Some("feature/gone"));
        assert!(entries[..3].iter().all(|e| !e.prunable));
    }

    #[test]
    fn a_detached_head_has_no_branch() {
        let entries = parse_worktree_list(LISTING);
        assert_eq!(entries[1].branch, None);
        assert_eq!(
            entries[1].path,
            PathBuf::from("/Users/t/Code/widgets-detached")
        );
    }

    #[test]
    fn a_bare_stanza_is_not_a_checkout() {
        let entries = parse_worktree_list(BARE_LISTING);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].branch.as_deref(), Some("topic"));
        assert!(
            !entries[0].is_main,
            "a bare repository has no main worktree"
        );
    }

    #[test]
    fn an_empty_listing_is_no_entries() {
        assert!(parse_worktree_list("").is_empty());
        assert!(parse_worktree_list("\n\n").is_empty());
    }

    #[test]
    fn quoted_paths_are_unescaped() {
        let listing = "worktree \"/Users/t/caf\\303\\251 \\\"x\\\"\"\nHEAD abc\ndetached\n";
        let entries = parse_worktree_list(listing);
        assert_eq!(entries[0].path, PathBuf::from("/Users/t/café \"x\""));
        assert_eq!(unquote("/plain/path with space"), "/plain/path with space");
    }

    #[test]
    fn github_remote_urls_in_every_shape_give_owner_and_repo() {
        for url in [
            "https://github.com/acme/widgets.git",
            "https://github.com/acme/widgets",
            "https://github.com/acme/widgets/",
            "https://x-access-token:abc@github.com/acme/widgets.git",
            "git@github.com:acme/widgets.git",
            "git@github.com:acme/widgets",
            "ssh://git@github.com/acme/widgets",
            "ssh://git@github.com:22/acme/widgets.git",
            "  https://github.com/acme/widgets.git\n",
        ] {
            assert_eq!(
                repo_from_remote(url).as_deref(),
                Some("acme/widgets"),
                "{url}"
            );
        }
    }

    #[test]
    fn dotted_repository_names_keep_their_dots() {
        assert_eq!(
            repo_from_remote("git@github.com:rust-lang/docs.rs.git").as_deref(),
            Some("rust-lang/docs.rs")
        );
    }

    #[test]
    fn remotes_that_are_not_a_repository_give_nothing() {
        for url in [
            "",
            "/Users/t/Code/widgets",
            "../widgets",
            "file:///Users/t/Code/widgets",
            "https://github.com/acme",
            "https://github.com/",
            "git@github.com:acme",
            "widgets",
        ] {
            assert_eq!(repo_from_remote(url), None, "{url:?}");
        }
    }

    #[test]
    fn numstat_sums_over_files_and_skips_binaries() {
        let out = "1\t0\ta.txt\n10\t3\tsrc/lib.rs\n-\t-\timage.png\n";
        assert_eq!(sum_numstat(out), (11, 3));
        assert_eq!(sum_numstat(""), (0, 0));
    }
}
