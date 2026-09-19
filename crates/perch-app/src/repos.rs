//! Giving each `[[repo]]` its identity.
//!
//! An entry with `github` knows what it is from the file. One with only
//! roots is whatever its first root turns out to be: a checkout with a
//! GitHub remote is that repository, and one with none lives on this
//! machine alone and is `local/<name>`. What decides is the root's
//! remotes, one git command over one directory, so it runs once, at
//! startup before the sidebar is told its repositories, and again in every
//! harness command that reads the file. Until it has run, a roots-only
//! entry has no identity and nothing lists it, which is why every reader
//! of `[[repo]]` loads through here rather than through `Config::load`
//! alone.

use perch_git::cmd::Git;
use perch_github::Config;

/// Give each `[[repo]]` with roots and no `github` its identity from its
/// first root's remotes: a GitHub remote makes it that repository, none
/// makes it `local/<name>`. The remotes alone, not the whole scan: which
/// repository a root is does not depend on whether a base branch can be
/// found in it, and a root missing its base is the scan's notice, every
/// time it runs, not a reason to call a GitHub checkout local. A root
/// whose remotes cannot be read is a warning and the entry is local, so
/// the entry is still there to be named in the sidebar and the reason is
/// in the status bar. Then repeated identities are dropped: a roots-only
/// entry may have turned out to be a repository declared above it.
pub async fn resolve(config: &mut Config, git: &Git) {
    let hosts = config.github_hosts();
    let hosts: Vec<&str> = hosts.iter().map(String::as_str).collect();
    for entry in config.repo.iter_mut().filter(|r| !r.is_resolved()) {
        // `parse` keeps no entry with neither `github` nor roots, so an
        // unresolved entry has a first root; were one to get here anyway,
        // local is the answer that asks nothing of the network.
        let Some(root) = entry.root_paths().into_iter().next() else {
            entry.resolve(None);
            continue;
        };
        match perch_git::remotes(git, &root).await {
            Ok(remotes) => entry.resolve(perch_git::github_remote(&remotes, &hosts).map(|(_, slug)| slug)),
            Err(e) => {
                config.warnings.push(format!(
                    "config.toml: {} could not be read ({e}); {} is taken as a local repository",
                    root.display(),
                    entry.display_name()
                ));
                entry.resolve(None);
            }
        }
    }
    config.drop_repeated_identities();
}

/// `Config::load()` with every entry resolved, for a caller already on a
/// runtime.
pub async fn load_resolved_async() -> Config {
    let mut config = Config::load();
    resolve(&mut config, &Git::new(None)).await;
    config
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use perch_github::RepoConfig;

    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// A repository on `main` with one commit, and no remote unless the
    /// test adds one. Nothing here fetches: a remote is config the scan
    /// reads back as text.
    fn init_repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("one.txt"), "one\n").unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "one"]);
    }

    /// Built field by field: the entry's identity is a private field, so
    /// the struct cannot be spelled out from here.
    fn roots_only(root: &Path) -> RepoConfig {
        let mut repo = RepoConfig::default();
        repo.roots = vec![root.to_string_lossy().into_owned()];
        repo
    }

    fn github(slug: &str) -> RepoConfig {
        let mut repo = RepoConfig::default();
        repo.github = Some(slug.to_string());
        repo
    }

    #[tokio::test]
    async fn a_root_with_no_remote_is_local_and_one_with_a_github_remote_is_that_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let plain = tmp.path().join("notes");
        init_repo(&plain);
        let forked = tmp.path().join("fork");
        init_repo(&forked);
        git(&forked, &["remote", "add", "origin", "git@github.com:me/fork.git"]);
        git(&forked, &["remote", "add", "upstream", "https://github.com/them/project.git"]);

        let mut config = Config::default();
        config.repo = vec![roots_only(&plain), roots_only(&forked)];
        resolve(&mut config, &Git::new(None)).await;

        assert_eq!(config.declared(), ["local/notes", "them/project"]);
        assert!(config.is_declared("LOCAL/notes"));
        assert_eq!(config.display_name("them/project"), "fork", "the name is still the root's, as it was before the scan");
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    }

    #[tokio::test]
    async fn a_root_that_cannot_be_read_is_a_warning_and_the_entry_stays_as_local() {
        let tmp = tempfile::tempdir().unwrap();
        let not_git = tmp.path().join("plain-dir");
        std::fs::create_dir_all(&not_git).unwrap();

        let mut config = Config::default();
        config.repo = vec![roots_only(&not_git)];
        resolve(&mut config, &Git::new(None)).await;

        assert_eq!(config.declared(), ["local/plain-dir"], "still there, so the sidebar can name it");
        assert_eq!(config.warnings.len(), 1, "{:?}", config.warnings);
        let warning = &config.warnings[0];
        assert!(warning.starts_with("config.toml: "), "{warning}");
        assert!(warning.contains("plain-dir"), "names the root: {warning}");
        assert!(warning.contains("local"), "{warning}");
    }

    #[tokio::test]
    async fn a_root_with_a_github_remote_but_no_base_is_still_that_repository() {
        // `git init -b develop` with a remote added and never fetched: no
        // remote HEAD, no `base` in config, and neither main nor master.
        // The scan will say so every time it runs; the identity is the
        // remote's regardless, so the rows and the knowledge do not move
        // when `base` is added.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("widgets");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "develop"]);
        git(&repo, &["remote", "add", "origin", "https://github.com/acme/widgets.git"]);

        let mut config = Config::default();
        config.repo = vec![roots_only(&repo)];
        resolve(&mut config, &Git::new(None)).await;

        assert_eq!(config.declared(), ["acme/widgets"]);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    }

    #[tokio::test]
    async fn a_roots_only_entry_that_turns_out_to_be_a_declared_repository_is_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let clone = tmp.path().join("project");
        init_repo(&clone);
        git(&clone, &["remote", "add", "origin", "https://github.com/them/project.git"]);

        let mut config = Config::default();
        config.repo = vec![github("them/project"), roots_only(&clone)];
        resolve(&mut config, &Git::new(None)).await;

        assert_eq!(config.declared(), ["them/project"]);
        assert_eq!(config.warnings.len(), 1, "{:?}", config.warnings);
        assert!(config.warnings[0].contains("already declares"), "{}", config.warnings[0]);
    }
}
