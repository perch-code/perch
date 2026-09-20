//! Deterministic, local Markdown exports of Perch review history.
//!
//! This crate intentionally knows nothing about SQLite or GitHub. The app
//! converts its durable records into these plain data structures, and this
//! writer atomically replaces one item document. Stable entry IDs make a
//! repeated export idempotent without hiding later observations.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not create the knowledge directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not write the knowledge export {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[derive(Debug, Clone)]
pub struct Knowledge {
    root: PathBuf,
}

impl Knowledge {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, Error> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|source| Error::CreateDirectory {
            path: root.clone(),
            source,
        })?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Export the complete known history for one item. The destination is
    /// stable and the content is rebuilt from stable IDs, so replaying an
    /// unchanged export cannot duplicate a message.
    pub fn export_review(&self, review: &ReviewArchive) -> Result<PathBuf, Error> {
        let path = self.review_path(&review.repository, &review.item_id);
        let parent = path.parent().expect("a review path always has a parent");
        fs::create_dir_all(parent).map_err(|source| Error::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
        atomic_write(&path, render_review(review).as_bytes())?;
        Ok(path)
    }

    pub fn review_path(&self, repository: &str, item_id: &str) -> PathBuf {
        let mut path = self.root.join("reviews");
        let mut wrote_repository = false;
        for component in repository.split(['/', '\\']) {
            if component.trim().is_empty() {
                continue;
            }
            path.push(safe_component(component));
            wrote_repository = true;
        }
        if !wrote_repository {
            path.push("local");
        }
        path.join(format!("{}.md", safe_component(item_id)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewArchive {
    pub item_id: String,
    pub repository: String,
    pub title: String,
    pub groups: Vec<GroupArchive>,
    pub findings: Vec<FindingArchive>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupArchive {
    pub id: String,
    pub sequence: u64,
    pub status: String,
    pub summary: String,
    pub snapshot: SnapshotArchive,
    pub attempts: Vec<AttemptArchive>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotArchive {
    pub id: String,
    pub base_label: String,
    pub base_sha: String,
    pub merge_base_sha: String,
    pub head_sha: String,
    /// A retained diff or snapshot path. It is emitted as a Markdown link,
    /// not opened or interpreted by this crate.
    pub link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptArchive {
    pub id: String,
    pub backend: String,
    pub model: String,
    pub effort: String,
    pub status: String,
    pub retry_of: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingArchive {
    pub id: String,
    pub thread_id: Option<String>,
    pub severity: String,
    pub status: String,
    pub title: String,
    pub possible_duplicate_of: Option<String>,
    pub entries: Vec<FindingEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingEntryKind {
    Observation,
    Verification,
    Disposition,
    FollowUp,
    DuplicateAssociation,
}

impl FindingEntryKind {
    fn label(self) -> &'static str {
        match self {
            Self::Observation => "Observation",
            Self::Verification => "Verification",
            Self::Disposition => "Disposition",
            Self::FollowUp => "Follow-up",
            Self::DuplicateAssociation => "Duplicate association",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingEntry {
    pub id: String,
    pub kind: FindingEntryKind,
    pub created_at: String,
    pub backend: Option<String>,
    pub model: Option<String>,
    pub snapshot_id: Option<String>,
    pub body: String,
    pub evidence: Option<String>,
    pub location: Option<LocationArchive>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationArchive {
    pub path: String,
    pub side: String,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
}

pub fn render_review(review: &ReviewArchive) -> String {
    let mut output = String::new();
    writeln!(output, "# {}", one_line(&review.title)).unwrap();
    writeln!(output).unwrap();
    writeln!(output, "- Item ID: `{}`", inline_code(&review.item_id)).unwrap();
    writeln!(
        output,
        "- Repository: `{}`",
        inline_code(&review.repository)
    )
    .unwrap();

    writeln!(output, "\n## Review history\n").unwrap();
    let groups = unique_by_id(&review.groups, |group| &group.id);
    if groups.is_empty() {
        writeln!(output, "No reviews recorded.").unwrap();
    }
    for group in groups.values() {
        writeln!(
            output,
            "### Review #{} · `{}` · {}\n",
            group.sequence,
            short_sha(&group.snapshot.head_sha),
            one_line(&group.status)
        )
        .unwrap();
        writeln!(
            output,
            "Against `{}` at `{}` (merge base `{}`)",
            inline_code(&group.snapshot.base_label),
            short_sha(&group.snapshot.base_sha),
            short_sha(&group.snapshot.merge_base_sha)
        )
        .unwrap();
        if let Some(link) = group
            .snapshot
            .link
            .as_deref()
            .filter(|link| !link.is_empty())
        {
            writeln!(
                output,
                "\n[Retained snapshot {}]({})",
                inline_text(&group.snapshot.id),
                markdown_link(link)
            )
            .unwrap();
        } else {
            writeln!(output, "\nSnapshot: `{}`", inline_code(&group.snapshot.id)).unwrap();
        }
        if !group.summary.trim().is_empty() {
            writeln!(output, "\n{}", group.summary.trim()).unwrap();
        }
        let attempts = unique_by_id(&group.attempts, |attempt| &attempt.id);
        if !attempts.is_empty() {
            writeln!(output, "\n| Reviewer | Model | Effort | Status | Attempt |").unwrap();
            writeln!(output, "| --- | --- | --- | --- | --- |").unwrap();
            for attempt in attempts.values() {
                let retry = attempt
                    .retry_of
                    .as_deref()
                    .map(|id| format!(" (retry of `{}`)", inline_code(id)))
                    .unwrap_or_default();
                writeln!(
                    output,
                    "| {} | `{}` | {} | {} | `{}`{} |",
                    table_text(&attempt.backend),
                    inline_code(&attempt.model),
                    table_text(&attempt.effort),
                    table_text(&attempt.status),
                    inline_code(&attempt.id),
                    retry
                )
                .unwrap();
            }
        }
        writeln!(output).unwrap();
    }

    writeln!(output, "## Findings\n").unwrap();
    let findings = unique_by_id(&review.findings, |finding| &finding.id);
    if findings.is_empty() {
        writeln!(output, "No findings recorded.").unwrap();
    }
    for finding in findings.values() {
        writeln!(
            output,
            "### [{}] {}\n",
            one_line(&finding.severity),
            one_line(&finding.title)
        )
        .unwrap();
        writeln!(output, "- Finding ID: `{}`", inline_code(&finding.id)).unwrap();
        writeln!(output, "- Status: {}", one_line(&finding.status)).unwrap();
        if let Some(thread_id) = &finding.thread_id {
            writeln!(output, "- Thread ID: `{}`", inline_code(thread_id)).unwrap();
        }
        if let Some(candidate) = &finding.possible_duplicate_of {
            writeln!(
                output,
                "- Possible duplicate of: `{}`",
                inline_code(candidate)
            )
            .unwrap();
        }
        let entries = unique_by_id(&finding.entries, |entry| &entry.id);
        for entry in entries.values() {
            writeln!(
                output,
                "\n#### {} · {}\n",
                entry.kind.label(),
                one_line(&entry.created_at)
            )
            .unwrap();
            let attribution = match (&entry.backend, &entry.model) {
                (Some(backend), Some(model)) => Some(format!("{backend} · {model}")),
                (Some(backend), None) => Some(backend.clone()),
                (None, Some(model)) => Some(model.clone()),
                (None, None) => None,
            };
            if let Some(attribution) = attribution {
                writeln!(output, "Reviewer: {}", one_line(&attribution)).unwrap();
            }
            if let Some(snapshot_id) = &entry.snapshot_id {
                writeln!(output, "Snapshot: `{}`", inline_code(snapshot_id)).unwrap();
            }
            if let Some(location) = &entry.location {
                let range = match (location.start_line, location.end_line) {
                    (Some(start), Some(end)) if end != start => format!(":{start}-{end}"),
                    (Some(start), _) => format!(":{start}"),
                    _ => String::new(),
                };
                writeln!(
                    output,
                    "Location: `{}` {}{}",
                    inline_code(&location.path),
                    one_line(&location.side),
                    range
                )
                .unwrap();
            }
            writeln!(output, "\n{}", entry.body.trim()).unwrap();
            if let Some(evidence) = entry
                .evidence
                .as_deref()
                .filter(|text| !text.trim().is_empty())
            {
                writeln!(output, "\n<details><summary>Original evidence</summary>\n").unwrap();
                writeln!(output, "```text\n{}\n```", safe_fence(evidence.trim())).unwrap();
                writeln!(output, "\n</details>").unwrap();
            }
        }
        writeln!(output).unwrap();
    }
    output
}

fn unique_by_id<'a, T>(values: &'a [T], id: impl Fn(&'a T) -> &'a str) -> BTreeMap<&'a str, &'a T> {
    values.iter().map(|value| (id(value), value)).collect()
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), Error> {
    let mut attempt = 0_u32;
    let (temp, mut file) = loop {
        let candidate = path.with_extension(format!("md.tmp-{}-{attempt}", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempt < 100 => {
                attempt += 1;
            }
            Err(source) => {
                return Err(Error::Write {
                    path: candidate,
                    source,
                });
            }
        }
    };
    let result = (|| {
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|source| Error::Write {
        path: path.to_path_buf(),
        source,
    })
}

fn safe_component(value: &str) -> String {
    let value = value.trim();
    let cleaned = value
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => character,
            _ => '-',
        })
        .collect::<String>();
    let cleaned = cleaned.trim_matches(['.', '-']);
    if cleaned.is_empty() || matches!(cleaned, "." | "..") {
        "unknown".into()
    } else {
        cleaned.chars().take(120).collect()
    }
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn inline_text(value: &str) -> String {
    one_line(value).replace(['[', ']'], "")
}

fn inline_code(value: &str) -> String {
    one_line(value).replace('`', "'")
}

fn table_text(value: &str) -> String {
    one_line(value).replace('|', "\\|")
}

fn markdown_link(value: &str) -> String {
    value.replace(' ', "%20").replace(['(', ')'], "")
}

fn short_sha(value: &str) -> String {
    value.chars().take(12).collect()
}

fn safe_fence(value: &str) -> String {
    value.replace("```", "``\u{200b}`")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive() -> ReviewArchive {
        ReviewArchive {
            item_id: "item:one".into(),
            repository: "owner/repo".into(),
            title: "Feature review".into(),
            groups: vec![GroupArchive {
                id: "group-1".into(),
                sequence: 1,
                status: "completed".into(),
                summary: "1 new · full coverage".into(),
                snapshot: SnapshotArchive {
                    id: "snapshot-1".into(),
                    base_label: "main".into(),
                    base_sha: "1111111111111111".into(),
                    merge_base_sha: "2222222222222222".into(),
                    head_sha: "3333333333333333".into(),
                    link: Some("/tmp/reviews/snapshot-1/diff.patch".into()),
                },
                attempts: vec![AttemptArchive {
                    id: "attempt-1".into(),
                    backend: "codex".into(),
                    model: "gpt-review".into(),
                    effort: "high".into(),
                    status: "completed".into(),
                    retry_of: None,
                }],
            }],
            findings: vec![FindingArchive {
                id: "finding-1".into(),
                thread_id: Some("thread-1".into()),
                severity: "high".into(),
                status: "open".into(),
                title: "Bounds panic".into(),
                possible_duplicate_of: None,
                entries: vec![FindingEntry {
                    id: "observation-1".into(),
                    kind: FindingEntryKind::Observation,
                    created_at: "2026-09-19T00:00:00Z".into(),
                    backend: Some("codex".into()),
                    model: Some("gpt-review".into()),
                    snapshot_id: Some("snapshot-1".into()),
                    body: "The unchecked index can exceed the vector.".into(),
                    evidence: Some("review evidence".into()),
                    location: Some(LocationArchive {
                        path: "src/lib.rs".into(),
                        side: "right".into(),
                        start_line: Some(4),
                        end_line: Some(5),
                    }),
                }],
            }],
        }
    }

    #[test]
    fn repeated_export_replaces_one_stable_document() {
        let directory = tempfile::tempdir().unwrap();
        let knowledge = Knowledge::open(directory.path()).unwrap();
        let first = knowledge.export_review(&archive()).unwrap();
        let before = fs::read_to_string(&first).unwrap();
        let second = knowledge.export_review(&archive()).unwrap();
        let after = fs::read_to_string(&second).unwrap();
        assert_eq!(first, second);
        assert_eq!(before, after);
        assert_eq!(after.matches("#### Observation").count(), 1);
        assert!(after.contains("Retained snapshot snapshot-1"));
    }

    #[test]
    fn duplicate_stable_ids_render_once_and_kinds_remain_distinct() {
        let mut value = archive();
        let mut duplicate_finding = value.findings[0].clone();
        let original_entry = duplicate_finding.entries[0].clone();
        duplicate_finding.entries.push(original_entry.clone());
        duplicate_finding.entries.push(FindingEntry {
            id: "disposition-1".into(),
            kind: FindingEntryKind::Disposition,
            body: "Dismissed as intentional.".into(),
            ..original_entry
        });
        value.findings.push(duplicate_finding);
        let rendered = render_review(&value);
        assert_eq!(rendered.matches("### [high] Bounds panic").count(), 1);
        assert_eq!(rendered.matches("#### Observation").count(), 1);
        assert_eq!(rendered.matches("#### Disposition").count(), 1);
    }

    #[test]
    fn repository_and_item_cannot_escape_the_root() {
        let directory = tempfile::tempdir().unwrap();
        let knowledge = Knowledge::open(directory.path()).unwrap();
        let path = knowledge.review_path("../owner/../../repo", "../item");
        assert!(path.starts_with(directory.path()));
        assert!(!path.to_string_lossy().contains("/../"));
    }
}
