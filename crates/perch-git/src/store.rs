//! Where clones and worktrees live on disk.
//!
//! One bare clone per repository, one worktree per pull request head. Objects
//! are shared between worktrees, so a second pull request from the same
//! repository costs a fetch rather than a clone, and one worktree can stay
//! indexed by a language server while another is prepared.

use std::path::{Path, PathBuf};

/// A repository coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoId {
    pub owner: String,
    pub repo: String,
}

impl RepoId {
    pub fn new(owner: impl Into<String>, repo: impl Into<String>) -> Self {
        Self { owner: owner.into(), repo: repo.into() }
    }

    /// Parse `owner/repo`, as it appears in a notification.
    pub fn parse(full_name: &str) -> Option<Self> {
        let (owner, repo) = full_name.split_once('/')?;
        if owner.is_empty() || repo.is_empty() || repo.contains('/') {
            return None;
        }
        Some(Self::new(owner, repo))
    }

    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    pub fn clone_url(&self) -> String {
        format!("https://github.com/{}/{}.git", self.owner, self.repo)
    }
}

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Bare clone directory for a repository.
    pub fn bare_path(&self, id: &RepoId) -> PathBuf {
        self.root
            .join("repos")
            .join("github.com")
            .join(sanitize(&id.owner))
            .join(format!("{}.git", sanitize(&id.repo)))
    }

    /// Worktree directory for one pull request head.
    ///
    /// Keyed by commit rather than pull request number: a force-push produces
    /// a new directory instead of a stale checkout wearing the right name.
    pub fn worktree_path(&self, id: &RepoId, head_sha: &str) -> PathBuf {
        let short: String = head_sha.chars().take(12).collect();
        self.root.join("worktrees").join(format!(
            "{}__{}__{}",
            sanitize(&id.owner),
            sanitize(&id.repo),
            sanitize(&short)
        ))
    }

    pub fn worktrees_dir(&self) -> PathBuf {
        self.root.join("worktrees")
    }

    pub fn has_bare_clone(&self, id: &RepoId) -> bool {
        self.bare_path(id).join("HEAD").is_file()
    }
}

/// Keep a path segment to characters that are safe on every filesystem.
///
/// GitHub names are already restricted, but a hostile or malformed value must
/// never be able to escape the store with `..` or a separator.
fn sanitize(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    let mut previous_dot = false;

    for c in segment.chars() {
        let mapped = if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
            c
        } else {
            '-'
        };
        // A single dot is kept, because repository names contain them
        // (`docs.rs`). A run of two is the traversal sequence, so the run
        // collapses to a separator instead.
        if mapped == '.' && previous_dot {
            out.pop();
            out.push('-');
            previous_dot = false;
            continue;
        }
        previous_dot = mapped == '.';
        out.push(mapped);
    }

    let trimmed = out.trim_matches('.');
    if trimmed.is_empty() {
        "-".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_names_round_trip() {
        let id = RepoId::parse("slatedb/slatedb").unwrap();
        assert_eq!(id.owner, "slatedb");
        assert_eq!(id.repo, "slatedb");
        assert_eq!(id.full_name(), "slatedb/slatedb");
    }

    #[test]
    fn malformed_full_names_are_rejected() {
        assert!(RepoId::parse("noslash").is_none());
        assert!(RepoId::parse("/repo").is_none());
        assert!(RepoId::parse("owner/").is_none());
        assert!(RepoId::parse("a/b/c").is_none());
    }

    #[test]
    fn paths_are_nested_under_the_store_root() {
        let store = Store::new("/tmp/perch");
        let id = RepoId::new("slatedb", "slatedb");
        assert_eq!(
            store.bare_path(&id),
            PathBuf::from("/tmp/perch/repos/github.com/slatedb/slatedb.git")
        );
    }

    #[test]
    fn worktrees_are_keyed_by_commit_not_pull_request() {
        let store = Store::new("/tmp/perch");
        let id = RepoId::new("slatedb", "slatedb");
        let a = store.worktree_path(&id, "abcdef0123456789abcdef");
        let b = store.worktree_path(&id, "999999999999999999");
        assert_ne!(a, b, "a force-push must land in its own directory");
        assert!(a.to_string_lossy().contains("abcdef012345"));
    }

    #[test]
    fn traversal_cannot_escape_the_store() {
        let store = Store::new("/tmp/perch");
        let id = RepoId::new("../../etc", "passwd");
        let path = store.bare_path(&id);
        assert!(!path.to_string_lossy().contains(".."));
        assert!(path.starts_with("/tmp/perch"));
    }

    #[test]
    fn separators_in_a_sha_cannot_escape_the_worktree_directory() {
        let store = Store::new("/tmp/perch");
        let id = RepoId::new("a", "b");
        let path = store.worktree_path(&id, "../../../etc");
        assert!(path.starts_with("/tmp/perch/worktrees"));
        assert!(!path.to_string_lossy().contains(".."));
    }

    #[test]
    fn dotted_repository_names_survive_sanitising() {
        let store = Store::new("/tmp/perch");
        let id = RepoId::new("rust-lang", "docs.rs");
        assert!(store.bare_path(&id).to_string_lossy().ends_with("docs.rs.git"));
    }

    #[test]
    fn clone_urls_carry_no_credentials() {
        let id = RepoId::new("slatedb", "slatedb");
        let url = id.clone_url();
        assert_eq!(url, "https://github.com/slatedb/slatedb.git");
        assert!(!url.contains('@'), "a token must never ride in the URL");
    }
}
