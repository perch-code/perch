//! Finding the branches a repository's roots have checked out.
//!
//! A root is a directory the user declared in config under a repository: a
//! clone of it, or a worktree of one. Asking it `git worktree
//! list` gives every worktree of that repository, the root itself included,
//! and each one on a branch other than the base is a worktree of the
//! reader's own. Its remotes say which repository it is: a GitHub remote,
//! `upstream` before `origin` before any other, names it `owner/repo`, and a
//! root with none is local to this machine. Nothing here writes or fetches:
//! the scan reads, and the caller decides what the items are.

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
    /// `owner/repo` from a GitHub remote, when the root has one.
    pub github: Option<String>,
    /// The remote that named it: `upstream` over `origin` over any other.
    /// None when `github` is.
    pub remote: Option<String>,
    /// The base branch: the remote's HEAD, else the `base` the caller gave,
    /// else `main` or `master`, whichever exists.
    pub base: String,
    /// What a branch is diffed against: `<remote>/<base>` for a GitHub
    /// root whose remote has the base, since the branch left the base as
    /// the remote had it; `<base>` itself for a local root, and for a
    /// GitHub root whose remote was added but never fetched, which has no
    /// copy of the base yet.
    pub base_ref: String,
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

/// `(name, url)` for each `(fetch)` line of `git remote -v`, in order.
///
/// Each remote prints twice, once for fetching and once for pushing, with
/// the name and URL separated by a tab and the URL and its role by a space.
/// The push URL can differ from the fetch one, but the fetch URL is where
/// the repository comes from, so that is the one that names it. A partial
/// clone's fetch line ends in its filter, `(fetch) [blob:none]`, and is a
/// fetch line all the same.
pub fn parse_remotes(remote_v: &str) -> Vec<(String, String)> {
    remote_v
        .lines()
        .filter_map(|line| {
            let (name, rest) = line.trim_end().split_once('\t')?;
            let url = rest
                .strip_suffix(" (fetch)")
                .or_else(|| rest.rsplit_once(" (fetch) [").map(|(url, _)| url))?;
            Some((name.to_string(), url.to_string()))
        })
        .collect()
}

/// A root's remotes, `(name, url)` for each, as `git remote -v` lists
/// them. An error when the root is not inside a git checkout, which is
/// the first thing a caller with a directory from config finds out.
pub async fn remotes(git: &Git, root: &Path) -> Result<Vec<(String, String)>, Error> {
    let listing = git.run(Some(root), ["remote", "-v"], QUERY_TIMEOUT).await?;
    Ok(parse_remotes(&listing))
}

/// `owner/repo` from a remote URL on one of `hosts`, in any of the shapes
/// GitHub hands out: `https://github.com/o/r.git`, `git@github.com:o/r.git`,
/// `ssh://git@github.com/o/r`. The host is compared without regard to case,
/// and a `user@` before it or a `:port` after it does not count. A remote on
/// any other host, or a local path, is not a GitHub repository.
pub fn repo_from_remote(url: &str, hosts: &[&str]) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    let (authority, path) = if let Some((_, rest)) = url.split_once("://") {
        rest.split_once('/')?
    } else if url.starts_with('/') || url.starts_with('.') {
        return None;
    } else {
        // scp-like: `git@github.com:o/r.git`
        url.split_once(':')?
    };

    let host = host_of(authority);
    if !hosts.iter().any(|h| h.eq_ignore_ascii_case(host)) {
        return None;
    }

    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

/// The host in a URL's authority, without the `user@` before it or the
/// `:port` after it.
fn host_of(authority: &str) -> &str {
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    host.split_once(':').map_or(host, |(host, _)| host)
}

/// The remote that decides which repository a root is: `upstream` if it is
/// on GitHub, else `origin` if it is, else the first other GitHub remote.
/// `(remote name, owner/repo)`. A fork's `origin` is the reader's own copy;
/// its `upstream` is the repository the pull requests are on.
pub fn github_remote(remotes: &[(String, String)], hosts: &[&str]) -> Option<(String, String)> {
    const PREFERRED: [&str; 2] = ["upstream", "origin"];
    let named = PREFERRED
        .iter()
        .filter_map(|preferred| remotes.iter().find(|(name, _)| name == preferred));
    let others = remotes
        .iter()
        .filter(|(name, _)| !PREFERRED.contains(&name.as_str()));
    named
        .chain(others)
        .find_map(|(name, url)| repo_from_remote(url, hosts).map(|slug| (name.clone(), slug)))
}

/// Ask a root what it has checked out, and which repository it is.
///
/// A root that is not inside a git checkout is an error from the first
/// command. Its remotes name the repository: the GitHub one among them,
/// `upstream` before `origin` before any other, fills `github` and `remote`,
/// and a root with no GitHub remote scans with both unset, the caller saying
/// whether that is a problem. The base branch is the remote's HEAD when a
/// clone or fetch recorded one, else the `base` given, else `main` or
/// `master`, whichever the root has; a root with none of those is an error,
/// since there would be nothing to diff a branch against. A branch is
/// diffed against the remote's copy of the base when the remote has one,
/// and against the base itself when it does not: a fork's `upstream`,
/// added by hand and not yet fetched, names the repository without having
/// a branch to diff against. Nothing here touches the network: remotes
/// come from the root's config and a remote HEAD from a ref already on
/// disk.
pub async fn scan_root(git: &Git, root: &Path, base: Option<&str>, hosts: &[&str]) -> Result<RootScan, Error> {
    let listing = git
        .run(
            Some(root),
            ["worktree", "list", "--porcelain"],
            QUERY_TIMEOUT,
        )
        .await?;
    let entries = parse_worktree_list(&listing);

    let (remote, github) = match github_remote(&remotes(git, root).await?, hosts) {
        Some((remote, github)) => (Some(remote), Some(github)),
        None => (None, None),
    };

    // The remote's HEAD is a branch the remote has by construction; a base
    // from anywhere else is checked against the remote before the diff is
    // pointed at it.
    let mut resolved = None;
    let mut remote_has_base = false;
    if let Some(remote) = &remote {
        resolved = remote_head(git, root, remote).await;
        remote_has_base = resolved.is_some();
    }
    if resolved.is_none() {
        resolved = base.map(str::to_string);
    }
    if resolved.is_none() {
        for candidate in ["main", "master"] {
            if has_ref(git, root, &format!("refs/heads/{candidate}")).await {
                resolved = Some(candidate.to_string());
                break;
            }
        }
    }
    let base = resolved.ok_or_else(|| Error::NoBase {
        root: root.to_path_buf(),
    })?;
    if let (Some(remote), false) = (&remote, remote_has_base) {
        remote_has_base = has_ref(git, root, &format!("refs/remotes/{remote}/{base}")).await;
    }
    let base_ref = match &remote {
        Some(remote) if remote_has_base => format!("{remote}/{base}"),
        _ => base.clone(),
    };

    Ok(RootScan {
        github,
        remote,
        base,
        base_ref,
        entries,
    })
}

/// The branch `refs/remotes/<remote>/HEAD` points at, which `git clone` sets
/// and `git remote set-head` can. None when the remote has never been
/// fetched, or its HEAD was never recorded.
async fn remote_head(git: &Git, root: &Path, remote: &str) -> Option<String> {
    let prefix = format!("refs/remotes/{remote}/");
    git.run(
        Some(root),
        ["symbolic-ref", "--quiet", &format!("{prefix}HEAD")],
        QUERY_TIMEOUT,
    )
    .await
    .ok()
    .and_then(|target| {
        target
            .trim()
            .strip_prefix(prefix.as_str())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    })
}

/// Whether a ref, spelled in full, exists in the root's repository.
async fn has_ref(git: &Git, root: &Path, full_ref: &str) -> bool {
    git.run(
        Some(root),
        ["rev-parse", "--verify", "--quiet", full_ref],
        QUERY_TIMEOUT,
    )
    .await
    .is_ok()
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

/// How a worktree's branch stands for a pull request against `base`, a
/// ref the worktree has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchReadiness {
    /// Commits on HEAD that `base` doesn't have.
    pub ahead: u32,
    /// Files `git status` lists: changed, staged or untracked. A pull
    /// request made now would leave them out.
    pub dirty_files: u32,
    /// The branch HEAD is on; `None` when it is detached.
    pub head_branch: Option<String>,
    /// Each commit's subject and body, oldest first, the newest fifty.
    pub commits: Vec<(String, String)>,
}

/// Read a worktree's [`BranchReadiness`]. Reads only.
pub async fn branch_readiness(git: &Git, worktree: &Path, base: &str) -> Result<BranchReadiness, Error> {
    let range = format!("{base}..HEAD");
    let ahead = git
        .run(Some(worktree), ["rev-list", "--count", &range], QUERY_TIMEOUT)
        .await?;
    let ahead = ahead
        .trim()
        .parse::<u32>()
        .map_err(|e| Error::Parse(format!("commit count {ahead:?}: {e}")))?;
    let status = git
        .run(
            Some(worktree),
            ["status", "--porcelain", "--untracked-files=normal"],
            QUERY_TIMEOUT,
        )
        .await?;
    let dirty_files = status
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with("!!"))
        .count() as u32;
    let head_branch = git
        .run(Some(worktree), ["symbolic-ref", "--quiet", "--short", "HEAD"], QUERY_TIMEOUT)
        .await
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let log = git
        .run(
            Some(worktree),
            ["log", "-n", "50", "--format=%s%x1f%b%x1e", &range],
            QUERY_TIMEOUT,
        )
        .await?;
    Ok(BranchReadiness {
        ahead,
        dirty_files,
        head_branch,
        commits: parse_subjects_and_bodies(&log),
    })
}

/// `git log --format=%s%x1f%b%x1e`, newest first, as (subject, body)
/// pairs oldest first.
fn parse_subjects_and_bodies(log: &str) -> Vec<(String, String)> {
    let mut commits: Vec<(String, String)> = log
        .split('\x1e')
        .filter_map(|record| {
            let record = record.trim_start_matches('\n');
            let (subject, body) = record.split_once('\x1f')?;
            Some((subject.trim().to_string(), body.trim().to_string()))
        })
        .collect();
    commits.reverse();
    commits
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

    const GITHUB: &[&str] = &["github.com"];

    // Captured from git 2.50 on a fork: the reader's copy as origin, the
    // repository it was forked from as upstream, a push URL that differs
    // from its fetch URL, and a remote from a `--filter=blob:none` clone,
    // whose fetch line carries the filter.
    const REMOTE_V: &str = "\
origin\thttps://github.com/me/widgets.git (fetch)
origin\tgit@github.com:me/widgets.git (push)
upstream\thttps://github.com/acme/widgets.git (fetch)
upstream\thttps://github.com/acme/widgets.git (push)
mirror\thttps://gitlab.com/acme/widgets.git (fetch)
mirror\thttps://gitlab.com/acme/widgets.git (push)
partial\thttps://github.com/acme/partial.git (fetch) [blob:none]
partial\thttps://github.com/acme/partial.git (push)
";

    fn remotes(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, url)| (name.to_string(), url.to_string()))
            .collect()
    }

    #[test]
    fn parse_remotes_takes_the_fetch_lines_and_keeps_their_order() {
        assert_eq!(
            parse_remotes(REMOTE_V),
            remotes(&[
                ("origin", "https://github.com/me/widgets.git"),
                ("upstream", "https://github.com/acme/widgets.git"),
                ("mirror", "https://gitlab.com/acme/widgets.git"),
                ("partial", "https://github.com/acme/partial.git"),
            ])
        );
        assert!(parse_remotes("").is_empty());
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
                repo_from_remote(url, GITHUB).as_deref(),
                Some("acme/widgets"),
                "{url}"
            );
        }
    }

    #[test]
    fn dotted_repository_names_keep_their_dots() {
        assert_eq!(
            repo_from_remote("git@github.com:rust-lang/docs.rs.git", GITHUB).as_deref(),
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
            assert_eq!(repo_from_remote(url, GITHUB), None, "{url:?}");
        }
    }

    #[test]
    fn a_remote_on_another_host_is_not_a_github_repository() {
        for url in [
            "https://gitlab.com/acme/widgets.git",
            "git@gitlab.com:acme/widgets.git",
            "ssh://git@gitlab.com/acme/widgets",
            "/Users/t/Code/acme/widgets",
        ] {
            assert_eq!(repo_from_remote(url, GITHUB), None, "{url}");
        }
    }

    #[test]
    fn a_configured_enterprise_host_counts_whatever_its_case() {
        let hosts = &["github.com", "github.acme.example"];
        assert_eq!(
            repo_from_remote("https://github.acme.example/acme/widgets.git", hosts).as_deref(),
            Some("acme/widgets")
        );
        assert_eq!(
            repo_from_remote("git@GitHub.Acme.Example:acme/widgets.git", hosts).as_deref(),
            Some("acme/widgets")
        );
        assert_eq!(
            repo_from_remote("https://GITHUB.COM/acme/widgets", hosts).as_deref(),
            Some("acme/widgets")
        );
        assert_eq!(
            repo_from_remote("https://github.acme.example/acme/widgets.git", GITHUB),
            None,
            "an enterprise host counts only when it is configured"
        );
    }

    #[test]
    fn a_user_and_a_port_do_not_count_when_matching_the_host() {
        for url in [
            "https://me@github.com/acme/widgets.git",
            "https://me:s3cret@github.com:443/acme/widgets.git",
            "ssh://git@github.com:22/acme/widgets",
            "ssh://github.com:22/acme/widgets",
            "me@github.com:acme/widgets.git",
        ] {
            assert_eq!(
                repo_from_remote(url, GITHUB).as_deref(),
                Some("acme/widgets"),
                "{url}"
            );
        }
        assert_eq!(
            repo_from_remote("https://github.com@evil.example/acme/widgets", GITHUB),
            None,
            "the user part is not the host"
        );
    }

    #[test]
    fn github_remote_prefers_upstream_then_origin_then_any_other() {
        let all = parse_remotes(REMOTE_V);
        assert_eq!(
            github_remote(&all, GITHUB),
            Some(("upstream".to_string(), "acme/widgets".to_string()))
        );

        let no_upstream = remotes(&[
            ("mirror", "https://github.com/mirror/widgets.git"),
            ("origin", "https://github.com/me/widgets.git"),
        ]);
        assert_eq!(
            github_remote(&no_upstream, GITHUB),
            Some(("origin".to_string(), "me/widgets".to_string())),
            "origin wins over another remote listed before it"
        );

        let upstream_elsewhere = remotes(&[
            ("upstream", "https://gitlab.com/acme/widgets.git"),
            ("origin", "/Users/t/Code/widgets"),
            ("fork", "https://github.com/them/widgets.git"),
            ("other", "https://github.com/other/widgets.git"),
        ]);
        assert_eq!(
            github_remote(&upstream_elsewhere, GITHUB),
            Some(("fork".to_string(), "them/widgets".to_string())),
            "a preferred name that is not on GitHub does not win"
        );
    }

    #[test]
    fn github_remote_is_nothing_when_no_remote_is_on_github() {
        let none = remotes(&[
            ("origin", "https://gitlab.com/acme/widgets.git"),
            ("backup", "/Volumes/backup/widgets.git"),
        ]);
        assert_eq!(github_remote(&none, GITHUB), None);
        assert_eq!(github_remote(&[], GITHUB), None);
    }

    #[test]
    fn numstat_sums_over_files_and_skips_binaries() {
        let out = "1\t0\ta.txt\n10\t3\tsrc/lib.rs\n-\t-\timage.png\n";
        assert_eq!(sum_numstat(out), (11, 3));
        assert_eq!(sum_numstat(""), (0, 0));
    }
}
