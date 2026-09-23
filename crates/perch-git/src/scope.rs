//! What range of a checkout a diff is read over.
//!
//! The scope menu offers the branch, the commits since the reader last
//! looked, one commit, and the uncommitted work in three cuts. Every one
//! of them comes down to two sides, a base and a head, each a commit, the
//! index or the working tree, and everything past this module reads a
//! changed list and a pair of sides without knowing which scope made them.
//!
//! Every command here runs in the checkout's own directory. A linked
//! worktree shares its clone's objects, so a diff between two commits
//! works there as well as in the clone, and the index and the working
//! tree are only reachable from there.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cmd::Git;
use crate::diff::{parse_numstat, ChangeStatus, ChangedFile, FileDiff};
use crate::Error;

const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const DIFF_TIMEOUT: Duration = Duration::from_secs(60);

/// One side of a diff.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Rev {
    /// Anything git resolves to a commit: a sha, `HEAD`, `<sha>^`.
    Sha(String),
    Index,
    Worktree,
}

/// What the scope menu offers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scope {
    /// The merge base to the head: what the diff has always shown.
    Branch,
    /// The head at the previous open to the head now.
    SinceLastLooked { from: String },
    /// One commit against its parent.
    Commit { sha: String },
    /// The working tree against `HEAD`.
    Uncommitted,
    /// The working tree against the index.
    Unstaged,
    /// The index against `HEAD`.
    Staged,
    /// What one Work run changed: the worktree as it was recorded when
    /// the run started, against it as it was when the run ended.
    Run { before: String, after: String },
}

impl Scope {
    /// The base and head sides, given the branch's merge base and head.
    ///
    /// A commit's parent is spelled `<sha>^` and left for git to resolve:
    /// every commit over a base has one, so the root-commit case does not
    /// arise here.
    pub fn sides(&self, merge_base: &str, head: &str) -> (Rev, Rev) {
        match self {
            Scope::Branch => (Rev::Sha(merge_base.to_string()), Rev::Sha(head.to_string())),
            Scope::SinceLastLooked { from } => (Rev::Sha(from.clone()), Rev::Sha(head.to_string())),
            Scope::Commit { sha } => (Rev::Sha(format!("{sha}^")), Rev::Sha(sha.clone())),
            Scope::Uncommitted => (Rev::Sha("HEAD".to_string()), Rev::Worktree),
            Scope::Unstaged => (Rev::Index, Rev::Worktree),
            Scope::Staged => (Rev::Sha("HEAD".to_string()), Rev::Index),
            Scope::Run { before, after } => (Rev::Sha(before.clone()), Rev::Sha(after.clone())),
        }
    }

    /// What the menu button says.
    pub fn label(&self) -> String {
        match self {
            Scope::Branch => "Branch".to_string(),
            Scope::SinceLastLooked { .. } => "Since you last looked".to_string(),
            Scope::Commit { sha } => short_sha(sha).to_string(),
            Scope::Uncommitted => "Uncommitted".to_string(),
            Scope::Unstaged => "Unstaged".to_string(),
            Scope::Staged => "Staged".to_string(),
            Scope::Run { .. } => "What the run changed".to_string(),
        }
    }

    /// The inverse of `Display`, for the command line: `branch`,
    /// `since:<sha>`, `commit:<sha>`, `uncommitted`, `unstaged`, `staged`,
    /// `run:<before>..<after>`.
    pub fn parse(s: &str) -> Option<Scope> {
        Some(match s.trim() {
            "branch" => Scope::Branch,
            "uncommitted" => Scope::Uncommitted,
            "unstaged" => Scope::Unstaged,
            "staged" => Scope::Staged,
            other => {
                let (kind, sha) = other.split_once(':')?;
                if sha.is_empty() {
                    return None;
                }
                match kind {
                    "since" => Scope::SinceLastLooked { from: sha.to_string() },
                    "commit" => Scope::Commit { sha: sha.to_string() },
                    "run" => {
                        let (before, after) = sha.split_once("..")?;
                        if before.is_empty() || after.is_empty() {
                            return None;
                        }
                        Scope::Run { before: before.to_string(), after: after.to_string() }
                    }
                    _ => return None,
                }
            }
        })
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Scope::Branch => f.write_str("branch"),
            Scope::SinceLastLooked { from } => write!(f, "since:{from}"),
            Scope::Commit { sha } => write!(f, "commit:{sha}"),
            Scope::Uncommitted => f.write_str("uncommitted"),
            Scope::Unstaged => f.write_str("unstaged"),
            Scope::Staged => f.write_str("staged"),
            Scope::Run { before, after } => write!(f, "run:{before}..{after}"),
        }
    }
}

/// The seven characters the menu shows.
fn short_sha(sha: &str) -> &str {
    let end = sha.char_indices().nth(7).map(|(i, _)| i).unwrap_or(sha.len());
    &sha[..end]
}

/// One row of the Committed submenu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    /// The message past the subject, without its trailing newline. Empty
    /// for a one-line message.
    pub body: String,
    pub author: String,
    pub authored_at: jiff::Timestamp,
}

impl Commit {
    pub fn short_sha(&self) -> &str {
        short_sha(&self.sha)
    }
}

/// Five NUL-separated fields per commit; `-z` ends each record with a NUL
/// as well, so the output is a flat run of fields, five to a commit.
const LOG_FORMAT: &str = "--format=%H%x00%s%x00%b%x00%an%x00%aI";

/// The commits in `base..head`, newest first.
pub async fn log(git: &Git, cwd: &Path, base: &str, head: &str) -> Result<Vec<Commit>, Error> {
    let out = git
        .run(Some(cwd), ["log", "-z", LOG_FORMAT, &format!("{base}..{head}")], QUERY_TIMEOUT)
        .await?;
    parse_log(&out)
}

/// Parse `git log -z` in `LOG_FORMAT`.
pub fn parse_log(raw: &str) -> Result<Vec<Commit>, Error> {
    let mut fields = raw.split('\0');
    let mut commits = Vec::new();

    while let Some(sha) = fields.next() {
        if sha.is_empty() {
            // The NUL after the last record leaves one empty field.
            break;
        }
        let (Some(subject), Some(body), Some(author), Some(date)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(Error::Parse(format!("log record for {sha} is cut short")));
        };
        let authored_at = date
            .parse::<jiff::Timestamp>()
            .map_err(|e| Error::Parse(format!("author date {date:?}: {e}")))?;
        commits.push(Commit {
            sha: sha.to_string(),
            subject: subject.to_string(),
            body: body.trim_end().to_string(),
            author: author.to_string(),
            authored_at,
        });
    }

    Ok(commits)
}

/// Whether a checkout has anything unstaged and anything staged. The menu
/// greys Uncommitted, Unstaged and Staged by it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dirt {
    /// Edits not yet added, and untracked files.
    pub unstaged: bool,
    pub staged: bool,
}

impl Dirt {
    pub fn any(self) -> bool {
        self.unstaged || self.staged
    }
}

/// What `git status` says a worktree has.
pub async fn dirt(git: &Git, worktree: &Path) -> Result<Dirt, Error> {
    // Named explicitly so a `status.showUntrackedFiles=no` in the reader's
    // config cannot hide what `changed_files` would list.
    let out = git
        .run(
            Some(worktree),
            ["status", "--porcelain", "--untracked-files=normal"],
            QUERY_TIMEOUT,
        )
        .await?;
    Ok(parse_status(&out))
}

/// Parse `git status --porcelain`.
///
/// Each line is `XY path`: `X` is the index against `HEAD`, `Y` the
/// working tree against the index, a space where a side is unchanged.
/// `??` is an untracked file, which counts as unstaged; `!!` is ignored.
pub fn parse_status(porcelain: &str) -> Dirt {
    let mut dirt = Dirt::default();
    for line in porcelain.lines() {
        let mut chars = line.chars();
        let (Some(x), Some(y)) = (chars.next(), chars.next()) else { continue };
        match x {
            '?' => dirt.unstaged = true,
            '!' => {}
            ' ' => dirt.unstaged |= y != ' ',
            _ => {
                dirt.staged = true;
                dirt.unstaged |= y != ' ';
            }
        }
    }
    dirt
}

/// The revision arguments that make `git diff` compare `base` to `head`.
///
/// git diffs between a commit, the index and the working tree, but its
/// options only spell each pair one way round; `-R` turns the others.
/// Two of the same side are nothing to diff.
fn diff_revs(base: &Rev, head: &Rev) -> Option<Vec<String>> {
    let revs = match (base, head) {
        (Rev::Sha(a), Rev::Sha(b)) => vec![a.clone(), b.clone()],
        (Rev::Sha(a), Rev::Index) => vec!["--cached".into(), a.clone()],
        (Rev::Index, Rev::Sha(b)) => vec!["-R".into(), "--cached".into(), b.clone()],
        (Rev::Sha(a), Rev::Worktree) => vec![a.clone()],
        (Rev::Worktree, Rev::Sha(b)) => vec!["-R".into(), b.clone()],
        (Rev::Index, Rev::Worktree) => vec![],
        (Rev::Worktree, Rev::Index) => vec!["-R".into()],
        (Rev::Index, Rev::Index) | (Rev::Worktree, Rev::Worktree) => return None,
    };
    Some(revs)
}

/// The changed list the rail reads, for any scope.
///
/// `worktree` is the checkout's root. `merge_base` and `head` are the
/// branch's, and only the scopes that span the branch use them.
///
/// When the head side is the working tree, the untracked files are in the
/// list as additions: git's diff does not look at them, and the working
/// tree has them.
pub async fn changed_files(
    git: &Git,
    worktree: &Path,
    scope: &Scope,
    merge_base: &str,
    head: &str,
) -> Result<Vec<ChangedFile>, Error> {
    let (base, head) = scope.sides(merge_base, head);
    let Some(revs) = diff_revs(&base, &head) else {
        return Ok(Vec::new());
    };

    // `--raw` carries what `--name-status` does and the blob on each side
    // besides, which is what Viewed keys on.
    let raw = diff(git, worktree, &["--raw", "--no-abbrev"], &revs).await?;
    let numstat = diff(git, worktree, &["--numstat"], &revs).await?;
    let raw = parse_raw(&raw);
    let irregular: std::collections::HashSet<String> =
        raw.iter().filter(|c| !c.regular).map(|c| c.path.clone()).collect();
    let mut files = assemble(raw, parse_numstat(&numstat));

    if head == Rev::Worktree {
        add_untracked(git, worktree, &mut files).await?;
        // git prints the null sha for a working tree file rather than
        // hashing it, so the head blobs are taken from disk.
        hash_on_disk(git, worktree, &mut files, &irregular).await?;
        files.sort_by(|a, b| a.path.cmp(&b.path));
    }

    Ok(files)
}

/// `git diff -M -z <flags> <revs>` in the worktree.
async fn diff(git: &Git, worktree: &Path, flags: &[&str], revs: &[String]) -> Result<String, Error> {
    let args = ["diff", "-M", "-z"]
        .iter()
        .chain(flags)
        .map(|s| s.to_string())
        .chain(revs.iter().cloned());
    git.run(Some(worktree), args, DIFF_TIMEOUT).await
}

/// Append the untracked files as additions.
///
/// A path the diff already lists is left to it: a file removed from the
/// index but still on disk shows as deleted against `HEAD` and untracked
/// at once, and one row is enough.
async fn add_untracked(git: &Git, worktree: &Path, files: &mut Vec<ChangedFile>) -> Result<(), Error> {
    let out = git
        .run(
            Some(worktree),
            ["ls-files", "--others", "--exclude-standard", "-z"],
            QUERY_TIMEOUT,
        )
        .await?;

    for path in out.split('\0').filter(|p| !p.is_empty()) {
        if files.iter().any(|f| f.path == path) {
            continue;
        }
        let (additions, binary) = match tokio::fs::read(worktree.join(path)).await {
            Ok(bytes) => count_lines(&bytes),
            Err(_) => (0, false),
        };
        files.push(ChangedFile {
            path: path.to_string(),
            old_path: None,
            status: ChangeStatus::Added,
            additions,
            deletions: 0,
            binary,
            head_blob: None,
        });
    }
    Ok(())
}

/// Lines in a file as `--numstat` would count them, and whether git would
/// call it binary: a NUL in the first 8000 bytes, which is git's own test.
fn count_lines(bytes: &[u8]) -> (u32, bool) {
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return (0, true);
    }
    let newlines = bytes.iter().filter(|b| **b == b'\n').count();
    let unterminated = usize::from(bytes.last().is_some_and(|b| *b != b'\n'));
    ((newlines + unterminated) as u32, false)
}

/// Fill `head_blob` from the files on disk, with one `git hash-object`.
///
/// The filters `git add` would apply are applied, so an unchanged file
/// hashes to the blob its commit has and stays viewed.
async fn hash_on_disk(
    git: &Git,
    worktree: &Path,
    files: &mut [ChangedFile],
    irregular: &std::collections::HashSet<String>,
) -> Result<(), Error> {
    // Only a regular file has a blob to hash: a submodule is a commit and
    // a symlink's blob is its target text, which hash-object would not
    // give. Those keep no blob and are never Viewed.
    let on_disk = |f: &ChangedFile| f.status != ChangeStatus::Deleted && !irregular.contains(&f.path);
    let paths: Vec<String> = files.iter().filter(|f| on_disk(f)).map(|f| f.path.clone()).collect();
    if paths.is_empty() {
        return Ok(());
    }

    let args = ["hash-object", "--"].iter().map(|s| s.to_string()).chain(paths.clone());
    match git.run(Some(worktree), args, DIFF_TIMEOUT).await {
        Ok(out) => {
            let mut shas = out.lines();
            for file in files.iter_mut().filter(|f| on_disk(f)) {
                file.head_blob = shas.next().map(str::to_string);
            }
        }
        // One unreadable file, a broken symlink say, fails the whole call;
        // hash the rest one at a time and leave that one without a blob.
        Err(_) => {
            for file in files.iter_mut().filter(|f| on_disk(f)) {
                let args = ["hash-object", "--", file.path.as_str()];
                file.head_blob = git.run(Some(worktree), args, DIFF_TIMEOUT).await.ok().map(|s| s.trim().to_string());
            }
        }
    }
    Ok(())
}

/// One record of `git diff --raw -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawChange {
    pub status: ChangeStatus,
    /// Path in the head tree. For a deletion, the path in the base tree.
    pub path: String,
    pub old_path: Option<String>,
    /// The head side's blob. `None` where git printed the null sha: a
    /// deletion, or a working tree file it did not hash.
    pub head_blob: Option<String>,
    /// Whether the head side is a regular file, as opposed to a submodule
    /// or a symlink, which have no blob worth keying Viewed on.
    pub regular: bool,
}

/// Parse `git diff -M --raw --no-abbrev -z`.
///
/// A record is `:<old mode> <new mode> <old sha> <new sha> <status>\0path\0`;
/// a rename or copy has `R<score>` for the status and two paths.
pub fn parse_raw(raw: &str) -> Vec<RawChange> {
    let mut out = Vec::new();
    let mut fields = raw.split('\0').filter(|f| !f.is_empty());

    while let Some(record) = fields.next() {
        let mut parts = record.strip_prefix(':').unwrap_or(record).split(' ');
        let (_, Some(new_mode), _, Some(new_sha), Some(code)) =
            (parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        // 100644 and 100755 are files; 120000 is a symlink, 160000 a
        // submodule, and 000000 a deletion, which is regular enough.
        let regular = new_mode.starts_with("100") || new_mode == "000000";
        let Some(status) = code.chars().next().and_then(ChangeStatus::from_letter) else {
            continue;
        };
        let Some(first) = fields.next() else { break };

        let head_blob = (!new_sha.bytes().all(|b| b == b'0')).then(|| new_sha.to_string());
        match status {
            ChangeStatus::Renamed | ChangeStatus::Copied => {
                let Some(second) = fields.next() else { break };
                out.push(RawChange {
                    status,
                    path: second.to_string(),
                    old_path: Some(first.to_string()),
                    head_blob,
                    regular,
                });
            }
            _ => out.push(RawChange { status, path: first.to_string(), old_path: None, head_blob, regular }),
        }
    }

    out
}

/// Join the raw records with their counts, sorted by path as the rail is.
fn assemble(raw: Vec<RawChange>, numstat: Vec<(u32, u32, bool, String)>) -> Vec<ChangedFile> {
    let counts: HashMap<String, (u32, u32, bool)> = numstat
        .into_iter()
        .map(|(adds, dels, binary, path)| (path, (adds, dels, binary)))
        .collect();

    let mut files: Vec<ChangedFile> = raw
        .into_iter()
        .map(|change| {
            let (additions, deletions, binary) =
                counts.get(&change.path).copied().unwrap_or((0, 0, false));
            ChangedFile {
                path: change.path,
                old_path: change.old_path,
                status: change.status,
                additions,
                deletions,
                binary,
                head_blob: change.head_blob,
            }
        })
        .collect();

    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// The content of `path` on one side, or `None` when that side has no such
/// file: not in the commit's tree, not in the index, not on disk.
///
/// Text comes back as git's or the disk's bytes read as UTF-8, lossily.
/// Neither size nor binary-ness is checked here: the caller has the
/// changed row's `binary` and its own size limit, as it does today.
pub async fn read_side(git: &Git, worktree: &Path, rev: &Rev, path: &str) -> Result<Option<String>, Error> {
    let spec = match rev {
        Rev::Sha(sha) => format!("{sha}:{path}"),
        Rev::Index => format!(":{path}"),
        Rev::Worktree => {
            return match tokio::fs::read(worktree.join(path)).await {
                Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(Error::Io(e.to_string())),
            };
        }
    };

    match git.run(Some(worktree), ["show", &spec], QUERY_TIMEOUT).await {
        Ok(text) => Ok(Some(text)),
        // `git show` has no exit code for a missing path apart from the
        // one for a bad revision; the message is the only tell. It says
        // "does not exist in", "does not exist (neither on disk nor in the
        // index)" or "exists on disk, but not in".
        Err(Error::Git { stderr, .. })
            if stderr.contains("does not exist") || stderr.contains("exists on disk, but not in") =>
        {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Rows of a split view: the base line and the head line each row shows,
/// 1-based, `None` on the side that has nothing there.
///
/// Computed from the same `FileDiff` the stacked view uses. Unchanged
/// lines are paired, an addition has no base line, a removal no head
/// line, and a modification is paired line for line as far as the shorter
/// side goes. `base` and `head` are only counted, so a diff that does not
/// account for every line still yields every line.
pub fn alignment(diff: &FileDiff, base: &str, head: &str) -> Vec<(Option<usize>, Option<usize>)> {
    let base_count = base.lines().count();
    let head_count = head.lines().count();

    // Removed lines by the head line they sit above. A modification's
    // removal and its hunk share that anchor.
    let mut removed_at = vec![0usize; head_count + 1];
    for run in &diff.deleted {
        removed_at[run.before_line.min(head_count)] += run.lines.len();
    }

    let mut rows = Vec::with_capacity(base_count.max(head_count));
    let (mut b, mut h) = (0usize, 0usize);

    for hunk in &diff.hunks {
        let start = hunk.start.min(head_count);
        while h < start {
            let base_line = (b < base_count).then(|| b + 1);
            rows.push((base_line, Some(h + 1)));
            b += usize::from(base_line.is_some());
            h += 1;
        }

        let removed = removed_at[start];
        let added = hunk.end.min(head_count).saturating_sub(start);
        let paired = removed.min(added);
        for _ in 0..paired {
            rows.push((Some(b + 1), Some(h + 1)));
            b += 1;
            h += 1;
        }
        for _ in paired..removed {
            rows.push((Some(b + 1), None));
            b += 1;
        }
        for _ in paired..added {
            rows.push((None, Some(h + 1)));
            h += 1;
        }
    }

    // The tail after the last change.
    while b < base_count || h < head_count {
        let base_line = (b < base_count).then(|| b + 1);
        let head_line = (h < head_count).then(|| h + 1);
        b += usize::from(base_line.is_some());
        h += usize::from(head_line.is_some());
        rows.push((base_line, head_line));
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::file_diff;

    #[test]
    fn every_scope_comes_down_to_two_sides() {
        let sha = |s: &str| Rev::Sha(s.to_string());
        assert_eq!(Scope::Branch.sides("mb", "hd"), (sha("mb"), sha("hd")));
        assert_eq!(
            Scope::SinceLastLooked { from: "old".into() }.sides("mb", "hd"),
            (sha("old"), sha("hd"))
        );
        assert_eq!(Scope::Commit { sha: "c1".into() }.sides("mb", "hd"), (sha("c1^"), sha("c1")));
        assert_eq!(Scope::Uncommitted.sides("mb", "hd"), (sha("HEAD"), Rev::Worktree));
        assert_eq!(Scope::Unstaged.sides("mb", "hd"), (Rev::Index, Rev::Worktree));
        assert_eq!(Scope::Staged.sides("mb", "hd"), (sha("HEAD"), Rev::Index));
    }

    #[test]
    fn scopes_print_and_parse_the_same() {
        let scopes = [
            Scope::Branch,
            Scope::SinceLastLooked { from: "abc123".into() },
            Scope::Commit { sha: "def456".into() },
            Scope::Uncommitted,
            Scope::Unstaged,
            Scope::Staged,
            Scope::Run { before: "aaa111".into(), after: "bbb222".into() },
        ];
        for scope in scopes {
            assert_eq!(Scope::parse(&scope.to_string()), Some(scope.clone()), "{scope}");
        }
        assert_eq!(Scope::parse("commit:def456").unwrap().to_string(), "commit:def456");
        assert_eq!(Scope::parse("since:abc").unwrap().to_string(), "since:abc");
        assert_eq!(Scope::parse(" staged\n"), Some(Scope::Staged));
    }

    #[test]
    fn a_scope_that_is_not_one_does_not_parse() {
        for s in ["", "Branch", "commit", "commit:", "since:", "tag:v1", "head", "run:a", "run:..b", "run:a.."] {
            assert_eq!(Scope::parse(s), None, "{s:?}");
        }
    }

    #[test]
    fn labels_name_the_entry_and_a_commit_by_its_short_sha() {
        assert_eq!(Scope::Branch.label(), "Branch");
        assert_eq!(Scope::SinceLastLooked { from: "x".into() }.label(), "Since you last looked");
        assert_eq!(Scope::Uncommitted.label(), "Uncommitted");
        assert_eq!(Scope::Unstaged.label(), "Unstaged");
        assert_eq!(Scope::Staged.label(), "Staged");
        assert_eq!(
            Scope::Commit { sha: "fa19d134cbfb5378935afdcec1326ae15620e461".into() }.label(),
            "fa19d13"
        );
        assert_eq!(Scope::Commit { sha: "abc".into() }.label(), "abc");
    }

    // Captured from git 2.50: three commits, the oldest with a two
    // paragraph body.
    const LOG: &str = "fa19d134cbfb5378935afdcec1326ae15620e461\0third\0\0t\u{0}2026-09-18T11:29:52-07:00\0\
115ac939242a3cc9398da7653822b9ac2fb88eae\0second\0\0Ann Other\u{0}2026-09-17T10:00:00+02:00\0\
c9e2a71baf7e8791602f0b113820970ff4b49134\0first\0A body line.\n\nSecond paragraph.\n\0t\u{0}2026-09-16T09:00:00Z\0";

    #[test]
    fn log_records_read_five_fields_each() {
        let commits = parse_log(LOG).unwrap();
        assert_eq!(commits.len(), 3);

        let subjects: Vec<&str> = commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, vec!["third", "second", "first"]);
        assert_eq!(commits[0].sha, "fa19d134cbfb5378935afdcec1326ae15620e461");
        assert_eq!(commits[0].short_sha(), "fa19d13");
        assert_eq!(commits[0].body, "");
        assert_eq!(commits[1].author, "Ann Other");
        assert_eq!(
            commits[1].authored_at,
            "2026-09-17T08:00:00Z".parse::<jiff::Timestamp>().unwrap(),
            "the offset is kept"
        );
        assert_eq!(commits[2].body, "A body line.\n\nSecond paragraph.");
    }

    #[test]
    fn an_empty_log_is_no_commits() {
        assert_eq!(parse_log("").unwrap(), vec![]);
    }

    #[test]
    fn a_cut_short_log_record_is_an_error() {
        let err = parse_log("abc\0subject\0body\0").unwrap_err();
        assert!(matches!(err, Error::Parse(_)), "{err}");
        let err = parse_log("abc\0s\0\0t\0not a date\0").unwrap_err();
        assert!(matches!(err, Error::Parse(_)), "{err}");
    }

    #[test]
    fn status_tells_staged_from_unstaged() {
        assert_eq!(parse_status(""), Dirt::default());
        assert!(!parse_status("").any());

        assert_eq!(parse_status(" M a.txt\n"), Dirt { unstaged: true, staged: false });
        assert_eq!(parse_status("M  a.txt\n"), Dirt { unstaged: false, staged: true });
        assert_eq!(parse_status("MM a.txt\n"), Dirt { unstaged: true, staged: true });
        assert_eq!(parse_status("A  new.txt\n D gone.txt\n"), Dirt { unstaged: true, staged: true });
        assert_eq!(parse_status("R  old.txt -> new.txt\n"), Dirt { unstaged: false, staged: true });
    }

    #[test]
    fn untracked_files_are_unstaged_and_ignored_ones_are_nothing() {
        assert_eq!(parse_status("?? scratch.txt\n"), Dirt { unstaged: true, staged: false });
        assert_eq!(parse_status("!! target/\n"), Dirt::default());
    }

    #[test]
    fn raw_records_carry_status_paths_and_the_head_blob() {
        let raw = ":100644 100644 4cb29ea38f70d7c61b2a3a25b02e3bdf44905402 6addb9b7c753aa12cdc6c0f22d7c7fe4b43ec8eb M\0a.txt\0\
:100644 100644 61780798228d17af2d34fce4cfbdf35556832472 61780798228d17af2d34fce4cfbdf35556832472 R100\0b.txt\0c.txt\0\
:100644 000000 286c5f5776916d7d7d5849988ca9d83e722cf9c2 0000000000000000000000000000000000000000 D\0gone.txt\0\
:000000 100644 0000000000000000000000000000000000000000 3e757656cf36eca53338e520d134963a44f793f8 A\0new.txt\0";
        let parsed = parse_raw(raw);
        assert_eq!(
            parsed,
            vec![
                RawChange {
                    status: ChangeStatus::Modified,
                    path: "a.txt".into(),
                    old_path: None,
                    head_blob: Some("6addb9b7c753aa12cdc6c0f22d7c7fe4b43ec8eb".into()),
                    regular: true,
                },
                RawChange {
                    status: ChangeStatus::Renamed,
                    path: "c.txt".into(),
                    old_path: Some("b.txt".into()),
                    head_blob: Some("61780798228d17af2d34fce4cfbdf35556832472".into()),
                    regular: true,
                },
                RawChange {
                    status: ChangeStatus::Deleted,
                    path: "gone.txt".into(),
                    old_path: None,
                    head_blob: None,
                    regular: true,
                },
                RawChange {
                    status: ChangeStatus::Added,
                    path: "new.txt".into(),
                    old_path: None,
                    head_blob: Some("3e757656cf36eca53338e520d134963a44f793f8".into()),
                    regular: true,
                },
            ]
        );
    }

    #[test]
    fn a_working_tree_file_git_did_not_hash_has_no_blob_yet() {
        let raw = ":100644 100644 6addb9b7c753aa12cdc6c0f22d7c7fe4b43ec8eb 0000000000000000000000000000000000000000 M\0a.txt\0";
        let parsed = parse_raw(raw);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].status, ChangeStatus::Modified);
        assert_eq!(parsed[0].head_blob, None);
        assert!(parse_raw("").is_empty());
    }

    #[test]
    fn assembling_pairs_records_with_counts_and_sorts_by_path() {
        let raw = vec![
            RawChange { status: ChangeStatus::Modified, path: "b.rs".into(), old_path: None, head_blob: Some("bb".into()), regular: true },
            RawChange { status: ChangeStatus::Added, path: "a.rs".into(), old_path: None, head_blob: Some("aa".into()), regular: true },
        ];
        let numstat = vec![(5, 2, false, "b.rs".to_string()), (40, 0, false, "a.rs".to_string())];
        let files = assemble(raw, numstat);
        assert_eq!(files[0].path, "a.rs");
        assert_eq!(files[0].additions, 40);
        assert_eq!(files[0].head_blob.as_deref(), Some("aa"));
        assert_eq!(files[1].path, "b.rs");
        assert_eq!(files[1].deletions, 2);
        assert_eq!(files[1].head_blob.as_deref(), Some("bb"));
    }

    #[test]
    fn line_counts_match_numstat() {
        assert_eq!(count_lines(b""), (0, false));
        assert_eq!(count_lines(b"a\nb\n"), (2, false));
        assert_eq!(count_lines(b"a\nb"), (2, false), "an unterminated last line counts");
        assert_eq!(count_lines(b"\x89PNG\0\0"), (0, true));
    }

    #[test]
    fn diff_revs_spell_each_pair_the_way_git_takes_it() {
        let sha = |s: &str| Rev::Sha(s.to_string());
        let strs = |v: Option<Vec<String>>| v.map(|v| v.join(" "));
        assert_eq!(strs(diff_revs(&sha("a"), &sha("b"))).as_deref(), Some("a b"));
        assert_eq!(strs(diff_revs(&sha("HEAD"), &Rev::Index)).as_deref(), Some("--cached HEAD"));
        assert_eq!(strs(diff_revs(&sha("HEAD"), &Rev::Worktree)).as_deref(), Some("HEAD"));
        assert_eq!(strs(diff_revs(&Rev::Index, &Rev::Worktree)).as_deref(), Some(""));
        assert_eq!(strs(diff_revs(&Rev::Worktree, &Rev::Index)).as_deref(), Some("-R"));
        assert_eq!(diff_revs(&Rev::Index, &Rev::Index), None);
    }

    fn rows(base: &str, head: &str) -> Vec<(Option<usize>, Option<usize>)> {
        alignment(&file_diff(base, head), base, head)
    }

    #[test]
    fn unchanged_lines_are_paired() {
        assert_eq!(rows("a\nb\n", "a\nb\n"), vec![(Some(1), Some(1)), (Some(2), Some(2))]);
    }

    #[test]
    fn an_addition_has_no_base_line() {
        assert_eq!(
            rows("a\nb\n", "a\nx\nb\n"),
            vec![(Some(1), Some(1)), (None, Some(2)), (Some(2), Some(3))]
        );
    }

    #[test]
    fn a_removal_has_no_head_line() {
        assert_eq!(
            rows("a\nb\nc\n", "a\nc\n"),
            vec![(Some(1), Some(1)), (Some(2), None), (Some(3), Some(2))]
        );
    }

    #[test]
    fn a_modification_is_paired_as_far_as_it_goes() {
        assert_eq!(
            rows("a\nb\nc\n", "a\nB\nc\n"),
            vec![(Some(1), Some(1)), (Some(2), Some(2)), (Some(3), Some(3))]
        );
        assert_eq!(
            rows("a\nb\nc\n", "a\nB\nB2\nc\n"),
            vec![(Some(1), Some(1)), (Some(2), Some(2)), (None, Some(3)), (Some(3), Some(4))]
        );
        assert_eq!(
            rows("a\nb\nc\nd\n", "a\nX\nd\n"),
            vec![(Some(1), Some(1)), (Some(2), Some(2)), (Some(3), None), (Some(4), Some(3))]
        );
    }

    #[test]
    fn a_removal_at_the_end_of_the_file_trails() {
        assert_eq!(
            rows("a\nb\nc\n", "a\n"),
            vec![(Some(1), Some(1)), (Some(2), None), (Some(3), None)]
        );
    }

    #[test]
    fn new_and_deleted_files_are_one_sided() {
        assert_eq!(rows("", "a\nb\n"), vec![(None, Some(1)), (None, Some(2))]);
        assert_eq!(rows("a\nb\n", ""), vec![(Some(1), None), (Some(2), None)]);
        assert!(rows("", "").is_empty());
    }

    #[test]
    fn a_plain_file_with_no_diff_pairs_every_line() {
        let text = "a\nb\nc\n";
        let rows = alignment(&FileDiff::default(), text, text);
        assert_eq!(rows, vec![(Some(1), Some(1)), (Some(2), Some(2)), (Some(3), Some(3))]);
    }

    #[test]
    fn every_line_of_both_sides_appears_once_in_order() {
        let cases = [
            ("a\nb\nc\nd\ne\n", "a\nB\nc\nD\ne\n"),
            ("one\ntwo\nthree\n", "one\ntwo prime\nthree\nfour\n"),
            ("a\nb\nc\nd\n", "d\nc\nb\na\n"),
            ("x\n", "y\ny\ny\n"),
            ("a\nb", "a\nb\nc"),
        ];
        for (base, head) in cases {
            let rows = rows(base, head);
            let bases: Vec<usize> = rows.iter().filter_map(|r| r.0).collect();
            let heads: Vec<usize> = rows.iter().filter_map(|r| r.1).collect();
            assert_eq!(bases, (1..=base.lines().count()).collect::<Vec<_>>(), "{base:?} -> {head:?}");
            assert_eq!(heads, (1..=head.lines().count()).collect::<Vec<_>>(), "{base:?} -> {head:?}");
            assert!(rows.iter().all(|r| r.0.is_some() || r.1.is_some()));
        }
    }
}
