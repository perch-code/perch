//! Discussion, archived as files an agent can grep.
//!
//! One record type, `Message`, for every source. Raw records go to JSONL
//! under `<root>/<owner>/<repo>/raw/<source>.jsonl`, which is the truth;
//! the markdown beside it is rendered from the raw files and can be
//! deleted and rebuilt. There is no database and no search: `rg` over the
//! markdown is the search.
//!
//! This crate knows about records and files. It does not know where a
//! record came from beyond the `Source` tag, and it never touches the
//! network.

mod render;
mod skill;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

pub use skill::SkillConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    GitHub,
    Discord,
    /// The reader's own threads with an agent, on this machine and nowhere
    /// else. Kept apart from GitHub's files so nothing local can be taken
    /// for something said in public.
    Perch,
}

impl Source {
    pub const ALL: [Source; 3] = [Source::GitHub, Source::Discord, Source::Perch];

    /// The directory the source's rendered files live under.
    pub fn dir_name(self) -> &'static str {
        match self {
            Source::GitHub => "github",
            Source::Discord => "discord",
            Source::Perch => "perch",
        }
    }
}

/// One thing somebody said, wherever they said it.
///
/// What a source has to say beyond these fields goes in `body` or goes
/// nowhere. Two conventions carry the little structure the renderer needs:
/// the first line of the earliest top-level message in a container is the
/// container's title when it is a markdown heading, and the first line of a
/// thread's head message is that thread's headline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub source: Source,
    /// The repository the container belongs to, `owner/repo`.
    pub repo: String,
    /// Where the message was said: `owner/repo#812` for a pull request,
    /// `owner/repo#issue-42` for an issue, `owner/repo@branch` for a
    /// branch with neither, later a channel.
    pub container: String,
    /// A review, later a Discord thread. `None` for the main conversation.
    pub thread: Option<String>,
    /// The source's own id, prefixed by kind so kinds cannot collide.
    /// Ingesting the same id again replaces the record.
    pub id: String,
    pub reply_to: Option<String>,
    pub author: String,
    pub at: Timestamp,
    pub edited_at: Option<Timestamp>,
    pub body: String,
    /// A permalink back to the source.
    pub url: String,
    /// `src/manifest.rs:41` on a review comment.
    pub location: Option<String>,
}

/// What an ingest did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ingested {
    pub added: usize,
    pub changed: usize,
}

impl Ingested {
    pub fn total(self) -> usize {
        self.added + self.changed
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path} line {line}: {source}")]
    Decode { path: PathBuf, line: usize, source: serde_json::Error },
    #[error("could not encode a record: {0}")]
    Encode(#[from] serde_json::Error),
    /// A message's `repo` is not `owner/repo`, so there is no directory it
    /// could go under.
    #[error("{0:?} is not owner/repo")]
    Repo(String),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io { path: path.to_path_buf(), source }
}

/// The store: a root directory and nothing else.
#[derive(Debug, Clone)]
pub struct Knowledge {
    root: PathBuf,
}

/// Which raw file a message lands in: one per repository and source. A
/// pull request and an issue in one repository share a file, and their
/// record ids (`pr-812`, `issue-42`) keep them apart in it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct RawKey {
    repo: String,
    source: Source,
}

impl Knowledge {
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Append what is new, replace what changed, and re-render the files
    /// those records land in.
    ///
    /// A message whose `repo` is not `owner/repo` fails the whole call
    /// before anything is written, since nothing could be filed under it.
    pub fn ingest(&self, messages: &[Message]) -> Result<Ingested, Error> {
        let mut by_file: BTreeMap<RawKey, Vec<&Message>> = BTreeMap::new();
        for m in messages {
            split_repo(&m.repo)?;
            by_file.entry(raw_key(m)).or_default().push(m);
        }

        let mut result = Ingested::default();
        let mut touched_repos = std::collections::BTreeSet::new();

        for (key, incoming) in by_file {
            let path = self.raw_path(&key)?;
            let mut records: BTreeMap<String, Message> = read_raw(&path)?
                .into_iter()
                .map(|m| (m.id.clone(), m))
                .collect();

            let mut containers = std::collections::BTreeSet::new();
            for m in incoming {
                match records.get(&m.id) {
                    Some(existing) if existing == m => continue,
                    Some(_) => result.changed += 1,
                    None => result.added += 1,
                }
                containers.insert(m.container.clone());
                records.insert(m.id.clone(), m.clone());
            }
            if containers.is_empty() {
                continue;
            }

            let all: Vec<Message> = sorted(records.into_values().collect());
            write_raw(&path, &all)?;
            for container in containers {
                self.render_container(&key.repo, key.source, &container, &all)?;
            }
            touched_repos.insert(key.repo.clone());
        }

        for repo in touched_repos {
            self.render_repo_index(&repo)?;
        }
        if result.total() > 0 {
            self.render_root_index()?;
        }
        Ok(result)
    }

    /// Rebuild every rendered file and index from the raw files.
    pub fn rerender(&self) -> Result<(), Error> {
        for repo in self.repos()? {
            for (key, path) in self.raw_files(&repo)? {
                let all = sorted(read_raw(&path)?);
                let containers: std::collections::BTreeSet<&str> =
                    all.iter().map(|m| m.container.as_str()).collect();
                for container in containers {
                    self.render_container(&key.repo, key.source, container, &all)?;
                }
            }
            self.render_repo_index(&repo)?;
        }
        self.render_root_index()
    }

    /// Write the skill that tells an agent where the files are.
    pub fn write_skill(&self, config: &SkillConfig) -> Result<(), Error> {
        let text = skill::render(&self.root, &config.repos);
        for dir in &config.dirs {
            std::fs::create_dir_all(dir).map_err(io(dir))?;
            write_atomic(&dir.join("SKILL.md"), &text)?;
        }
        Ok(())
    }

    /// Every repository with a raw directory, as `owner/repo`.
    ///
    /// Only `<owner>/<repo>/raw` counts, two levels down, so a directory
    /// left from the layout before this one, which had `raw/` one level
    /// down, is never taken for a repository.
    pub fn repos(&self) -> Result<Vec<String>, Error> {
        let Ok(owners) = std::fs::read_dir(&self.root) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for owner in owners.flatten().filter(|e| e.path().is_dir()) {
            let Ok(repos) = std::fs::read_dir(owner.path()) else { continue };
            for repo in repos.flatten().filter(|e| e.path().join("raw").is_dir()) {
                out.push(format!(
                    "{}/{}",
                    owner.file_name().to_string_lossy(),
                    repo.file_name().to_string_lossy()
                ));
            }
        }
        out.sort();
        Ok(out)
    }

    /// The directory a repository's files live under: `<root>/<owner>/<repo>`,
    /// each segment made safe, so nothing in a name can climb out of the root.
    fn repo_dir(&self, repo: &str) -> Result<PathBuf, Error> {
        let (owner, name) = split_repo(repo)?;
        Ok(self.root.join(owner).join(name))
    }

    fn raw_path(&self, key: &RawKey) -> Result<PathBuf, Error> {
        Ok(self
            .repo_dir(&key.repo)?
            .join("raw")
            .join(format!("{}.jsonl", key.source.dir_name())))
    }

    fn raw_files(&self, repo: &str) -> Result<Vec<(RawKey, PathBuf)>, Error> {
        let mut out = Vec::new();
        for source in Source::ALL {
            let key = RawKey { repo: repo.to_string(), source };
            let path = self.raw_path(&key)?;
            if path.is_file() {
                out.push((key, path));
            }
        }
        out.sort();
        Ok(out)
    }

    /// Where a container's markdown lives, relative to the repository's
    /// directory.
    pub fn rendered_relative(source: Source, container: &str) -> PathBuf {
        let mut path = PathBuf::from(source.dir_name());
        if let Some(n) = numbered(container) {
            // `owner/repo#812` → `pulls/812.md`,
            // `owner/repo#issue-42` → `issues/42.md`
            path.push(if n.issue { "issues" } else { "pulls" });
            path.push(format!("{}.md", safe(n.number)));
        } else if let Some(branch) = branched(container) {
            // `owner/repo@chris/fence` → `branches/chris-fence.md`
            path.push("branches");
            path.push(format!("{}.md", safe(branch)));
        } else {
            path.push(format!("{}.md", safe(container)));
        }
        path
    }

    /// How a container is named in a heading or an index.
    ///
    /// An issue's container carries `issue-` so its records cannot be
    /// mistaken for a pull request's in the shared raw file. The reader
    /// sees `owner/repo#42` for both, which is how GitHub itself refers to
    /// them; the rendered file's path says which kind it is.
    pub fn container_label(container: &str) -> String {
        match numbered(container) {
            Some(n) => format!("{}#{}", n.repo, n.number),
            None => container.to_string(),
        }
    }

    fn render_container(
        &self,
        repo: &str,
        source: Source,
        container: &str,
        all: &[Message],
    ) -> Result<(), Error> {
        let messages: Vec<&Message> = all.iter().filter(|m| m.container == container).collect();
        let text = render::container(&Self::container_label(container), &messages);
        let path = self.repo_dir(repo)?.join(Self::rendered_relative(source, container));
        write_atomic(&path, &text)
    }

    fn render_repo_index(&self, repo: &str) -> Result<(), Error> {
        let mut sections = Vec::new();
        for (key, path) in self.raw_files(repo)? {
            let all = read_raw(&path)?;
            sections.push((key.source, all));
        }
        let text = render::repo_index(repo, &sections);
        write_atomic(&self.repo_dir(repo)?.join("index.md"), &text)
    }

    fn render_root_index(&self) -> Result<(), Error> {
        let mut repos = Vec::new();
        for repo in self.repos()? {
            let mut all = Vec::new();
            for (_, path) in self.raw_files(&repo)? {
                all.extend(read_raw(&path)?);
            }
            repos.push((repo, all));
        }
        let text = render::root_index(&repos);
        write_atomic(&self.root.join("index.md"), &text)
    }
}

fn raw_key(m: &Message) -> RawKey {
    RawKey { repo: m.repo.clone(), source: m.source }
}

/// `owner/repo` as two path segments, each made safe. Anything else, from
/// a bare name to `a/b/c`, is refused rather than guessed at, since the two
/// segments are what keeps a name inside the root.
fn split_repo(repo: &str) -> Result<(String, String), Error> {
    match repo.split_once('/') {
        Some((owner, name)) if !owner.is_empty() && !name.is_empty() && !name.contains('/') => {
            Ok((safe(owner), safe(name)))
        }
        _ => Err(Error::Repo(repo.to_string())),
    }
}

/// A container that names a numbered thing in a repository: a pull request
/// as `owner/repo#812`, an issue as `owner/repo#issue-42`.
struct Numbered<'a> {
    repo: &'a str,
    number: &'a str,
    issue: bool,
}

fn numbered(container: &str) -> Option<Numbered<'_>> {
    let (repo, rest) = container.split_once('#')?;
    // A `#` after the `@` of a branch container is part of the branch.
    if repo.contains('@') {
        return None;
    }
    let (issue, number) = match rest.strip_prefix("issue-") {
        Some(number) => (true, number),
        None => (false, rest),
    };
    Some(Numbered { repo, number, issue })
}

/// The branch of a container that names one, `owner/repo@branch`: a
/// checkout with no pull request or issue on GitHub.
fn branched(container: &str) -> Option<&str> {
    container.split_once('@').map(|(_, branch)| branch)
}

/// Keep a path segment to characters safe everywhere, and never `..`.
///
/// Only the segments that would climb or vanish are replaced: `.`, `..`
/// and the empty string. A leading dot is kept, since `.github` is a
/// repository name and must come back as one from `repos()`.
fn safe(segment: &str) -> String {
    let out: String = segment
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
        .collect();
    if out.is_empty() || out == "." || out == ".." { "-".to_string() } else { out }
}

fn sorted(mut messages: Vec<Message>) -> Vec<Message> {
    messages.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
    messages
}

fn read_raw(path: &Path) -> Result<Vec<Message>, Error> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::Io { path: path.to_path_buf(), source: e }),
    };
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let m: Message = serde_json::from_str(line)
            .map_err(|source| Error::Decode { path: path.to_path_buf(), line: i + 1, source })?;
        out.push(m);
    }
    Ok(out)
}

fn write_raw(path: &Path, messages: &[Message]) -> Result<(), Error> {
    let mut text = String::new();
    for m in messages {
        text.push_str(&serde_json::to_string(m)?);
        text.push('\n');
    }
    write_atomic(path, &text)
}

/// Whole file to a temporary name, then rename: a reader mid-write sees
/// the old file or the new one, never half of each.
fn write_atomic(path: &Path, text: &str) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    // A name of this write's own, so two writers of one file, which can
    // happen when a discussion is archived twice at once, do not rename
    // each other's temporary away.
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!(
        "{}.{}.{n}.tmp",
        path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default(),
        std::process::id()
    ));
    std::fs::write(&tmp, text).map_err(io(&tmp))?;
    std::fs::rename(&tmp, path).map_err(io(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containers_map_to_rendered_paths_under_the_repository() {
        assert_eq!(
            Knowledge::rendered_relative(Source::GitHub, "slatedb/slatedb#812"),
            PathBuf::from("github/pulls/812.md")
        );
        assert_eq!(
            Knowledge::rendered_relative(Source::Perch, "slatedb/slatedb@chris/fence"),
            PathBuf::from("perch/branches/chris-fence.md")
        );
        assert_eq!(
            Knowledge::rendered_relative(Source::Discord, "general"),
            PathBuf::from("discord/general.md")
        );
        let k = Knowledge::open("/k");
        assert_eq!(k.repo_dir("slatedb/slatedb").unwrap(), PathBuf::from("/k/slatedb/slatedb"));
        assert_eq!(
            k.raw_path(&RawKey { repo: "slatedb/slatedb".into(), source: Source::GitHub }).unwrap(),
            PathBuf::from("/k/slatedb/slatedb/raw/github.jsonl")
        );
    }

    #[test]
    fn an_issue_shares_the_repository_raw_file_and_renders_beside_pulls() {
        assert_eq!(
            Knowledge::rendered_relative(Source::GitHub, "slatedb/slatedb#issue-42"),
            PathBuf::from("github/issues/42.md")
        );
        assert_eq!(Knowledge::container_label("slatedb/slatedb#issue-42"), "slatedb/slatedb#42");
        assert_eq!(Knowledge::container_label("slatedb/slatedb#812"), "slatedb/slatedb#812");
        assert_eq!(Knowledge::container_label("general"), "general");
    }

    #[test]
    fn a_hash_in_a_branch_name_does_not_make_it_a_pull_request() {
        assert_eq!(
            Knowledge::rendered_relative(Source::Perch, "slatedb/slatedb@fix#12"),
            PathBuf::from("perch/branches/fix-12.md")
        );
        assert_eq!(Knowledge::container_label("slatedb/slatedb@fix#12"), "slatedb/slatedb@fix#12");
    }

    #[test]
    fn hostile_names_cannot_leave_the_root() {
        // A `..` inside a segment is harmless; one that is the whole
        // segment would climb. Neither can come out as a component.
        let rel = Knowledge::rendered_relative(Source::GitHub, "../x#..");
        assert!(rel.components().all(|c| c.as_os_str() != ".."), "{rel:?}");
        let rel = Knowledge::rendered_relative(Source::Perch, "../x@../..");
        assert!(rel.components().all(|c| c.as_os_str() != ".."), "{rel:?}");
        assert_eq!(safe(".."), "-");
        assert_eq!(safe("."), "-");
        assert_eq!(safe(""), "-");
        assert_eq!(safe("../.."), "..-..");

        // A repository is two segments and no more; each is made safe, and
        // a third segment is refused rather than joined.
        let k = Knowledge::open("/k");
        let dir = k.repo_dir("../x").unwrap();
        assert!(dir.starts_with("/k"), "{dir:?}");
        assert!(dir.components().all(|c| c.as_os_str() != ".."), "{dir:?}");
        assert_eq!(dir, PathBuf::from("/k/-/x"));
        for repo in ["../x/../y", "../../etc", "slatedb", "/slatedb", "slatedb/", ""] {
            assert!(matches!(k.repo_dir(repo), Err(Error::Repo(_))), "{repo:?}");
        }
    }

    #[test]
    fn a_repository_named_with_a_leading_dot_keeps_it() {
        // `.github` is a real repository name. Only `.` and `..` climb.
        let k = Knowledge::open("/k");
        assert_eq!(k.repo_dir("acme/.github").unwrap(), PathBuf::from("/k/acme/.github"));
        assert_eq!(k.repo_dir("acme/.").unwrap(), PathBuf::from("/k/acme/-"));
        assert_eq!(k.repo_dir("./x").unwrap(), PathBuf::from("/k/-/x"));
    }
}
