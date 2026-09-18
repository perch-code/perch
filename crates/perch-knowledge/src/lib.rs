//! Discussion, archived as files an agent can grep.
//!
//! One record type, `Message`, for every source. Raw records go to JSONL
//! under `<root>/<project>/raw/`, which is the truth; the markdown beside
//! it is rendered from the raw files and can be deleted and rebuilt. There
//! is no database and no search: `rg` over the markdown is the search.
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
    /// The project the container belongs to, such as `slatedb`.
    pub project: String,
    /// Where the message was said: `owner/repo#812` for a pull request,
    /// `owner/repo#issue-42` for an issue, later a channel.
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
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io { path: path.to_path_buf(), source }
}

/// The store: a root directory and nothing else.
#[derive(Debug, Clone)]
pub struct Knowledge {
    root: PathBuf,
}

/// Which raw file a message lands in.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct RawKey {
    project: String,
    source: Source,
    /// The container group: the repository for GitHub, the channel later.
    group: String,
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
    pub fn ingest(&self, messages: &[Message]) -> Result<Ingested, Error> {
        let mut by_file: BTreeMap<RawKey, Vec<&Message>> = BTreeMap::new();
        for m in messages {
            by_file.entry(raw_key(m)).or_default().push(m);
        }

        let mut result = Ingested::default();
        let mut touched_projects = std::collections::BTreeSet::new();

        for (key, incoming) in by_file {
            let path = self.raw_path(&key);
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
                self.render_container(&key.project, key.source, &container, &all)?;
            }
            touched_projects.insert(key.project.clone());
        }

        for project in touched_projects {
            self.render_project_index(&project)?;
        }
        if result.total() > 0 {
            self.render_root_index()?;
        }
        Ok(result)
    }

    /// Rebuild every rendered file and index from the raw files.
    pub fn rerender(&self) -> Result<(), Error> {
        for project in self.projects()? {
            for (key, path) in self.raw_files(&project)? {
                let all = sorted(read_raw(&path)?);
                let containers: std::collections::BTreeSet<&str> =
                    all.iter().map(|m| m.container.as_str()).collect();
                for container in containers {
                    self.render_container(&key.project, key.source, container, &all)?;
                }
            }
            self.render_project_index(&project)?;
        }
        self.render_root_index()
    }

    /// Write the skill that tells an agent where the files are.
    pub fn write_skill(&self, config: &SkillConfig) -> Result<(), Error> {
        let text = skill::render(&self.root, &config.projects);
        for dir in &config.dirs {
            std::fs::create_dir_all(dir).map_err(io(dir))?;
            write_atomic(&dir.join("SKILL.md"), &text)?;
        }
        Ok(())
    }

    /// Every project with a raw directory.
    pub fn projects(&self) -> Result<Vec<String>, Error> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Ok(Vec::new());
        };
        let mut out: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().join("raw").is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        out.sort();
        Ok(out)
    }

    fn raw_path(&self, key: &RawKey) -> PathBuf {
        self.root
            .join(&key.project)
            .join("raw")
            .join(key.source.dir_name())
            .join(format!("{}.jsonl", key.group))
    }

    fn raw_files(&self, project: &str) -> Result<Vec<(RawKey, PathBuf)>, Error> {
        let mut out = Vec::new();
        for source in Source::ALL {
            let dir = self.root.join(project).join("raw").join(source.dir_name());
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "jsonl") {
                    let group = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                    out.push((RawKey { project: project.to_string(), source, group }, path));
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// Where a container's markdown lives, relative to the project.
    pub fn rendered_relative(source: Source, container: &str) -> PathBuf {
        let mut path = PathBuf::from(source.dir_name());
        match numbered(container) {
            // `owner/repo#812` → `owner/repo/pulls/812.md`,
            // `owner/repo#issue-42` → `owner/repo/issues/42.md`
            Some(n) => {
                for part in n.repo.split('/') {
                    path.push(safe(part));
                }
                path.push(if n.issue { "issues" } else { "pulls" });
                path.push(format!("{}.md", safe(n.number)));
            }
            None => path.push(format!("{}.md", safe(container))),
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
        project: &str,
        source: Source,
        container: &str,
        all: &[Message],
    ) -> Result<(), Error> {
        let messages: Vec<&Message> = all.iter().filter(|m| m.container == container).collect();
        let text = render::container(&Self::container_label(container), &messages);
        let path = self.root.join(project).join(Self::rendered_relative(source, container));
        write_atomic(&path, &text)
    }

    fn render_project_index(&self, project: &str) -> Result<(), Error> {
        let mut sections = Vec::new();
        for (key, path) in self.raw_files(project)? {
            let all = read_raw(&path)?;
            sections.push((key.source, all));
        }
        let text = render::project_index(project, &sections);
        write_atomic(&self.root.join(project).join("index.md"), &text)
    }

    fn render_root_index(&self) -> Result<(), Error> {
        let mut projects = Vec::new();
        for project in self.projects()? {
            let mut all = Vec::new();
            for (_, path) in self.raw_files(&project)? {
                all.extend(read_raw(&path)?);
            }
            projects.push((project, all));
        }
        let text = render::root_index(&projects);
        write_atomic(&self.root.join("index.md"), &text)
    }
}

fn raw_key(m: &Message) -> RawKey {
    RawKey {
        project: m.project.clone(),
        source: m.source,
        group: container_group(&m.container),
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
    let (issue, number) = match rest.strip_prefix("issue-") {
        Some(number) => (true, number),
        None => (false, rest),
    };
    Some(Numbered { repo, number, issue })
}

/// The part of a container that names its raw file: the repository of
/// `owner/repo#812` or `owner/repo#issue-42`, the whole name otherwise.
/// Slashes become dashes so the group is one path segment. A pull request
/// and an issue in one repository share a file, and their record ids
/// (`pr-812`, `issue-42`) keep them apart in it.
fn container_group(container: &str) -> String {
    let group = numbered(container).map(|n| n.repo).unwrap_or(container);
    safe(&group.replace('/', "-"))
}

/// Keep a path segment to characters safe everywhere, and never `..`.
fn safe(segment: &str) -> String {
    let out: String = segment
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
        .collect();
    let trimmed = out.trim_matches('.');
    if trimmed.is_empty() { "-".to_string() } else { trimmed.to_string() }
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
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    std::fs::write(&tmp, text).map_err(io(&tmp))?;
    std::fs::rename(&tmp, path).map_err(io(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containers_map_to_raw_groups_and_rendered_paths() {
        assert_eq!(container_group("slatedb/slatedb#812"), "slatedb-slatedb");
        assert_eq!(container_group("general"), "general");
        assert_eq!(
            Knowledge::rendered_relative(Source::GitHub, "slatedb/slatedb#812"),
            PathBuf::from("github/slatedb/slatedb/pulls/812.md")
        );
        assert_eq!(
            Knowledge::rendered_relative(Source::Discord, "general"),
            PathBuf::from("discord/general.md")
        );
    }

    #[test]
    fn an_issue_shares_the_repository_raw_file_and_renders_beside_pulls() {
        assert_eq!(container_group("slatedb/slatedb#issue-42"), "slatedb-slatedb");
        assert_eq!(
            Knowledge::rendered_relative(Source::GitHub, "slatedb/slatedb#issue-42"),
            PathBuf::from("github/slatedb/slatedb/issues/42.md")
        );
        assert_eq!(Knowledge::container_label("slatedb/slatedb#issue-42"), "slatedb/slatedb#42");
        assert_eq!(Knowledge::container_label("slatedb/slatedb#812"), "slatedb/slatedb#812");
        assert_eq!(Knowledge::container_label("general"), "general");
    }

    #[test]
    fn hostile_names_cannot_leave_the_root() {
        // A `..` inside a segment is harmless; one that is the whole
        // segment would climb. Neither can come out as a component.
        let rel = Knowledge::rendered_relative(Source::GitHub, "../x#..");
        assert!(rel.components().all(|c| c.as_os_str() != ".."), "{rel:?}");
        assert_eq!(safe(".."), "-");
        assert!(!container_group("../../etc#1").contains('/'));
    }
}
