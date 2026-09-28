//! What a pull request changed: the file list, and per-line status.

use imara_diff::intern::InternedInput;
use imara_diff::sources::lines;
use imara_diff::{diff, Algorithm};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
}

impl ChangeStatus {
    /// The single letter git uses, and that the rail shows.
    pub fn letter(self) -> &'static str {
        match self {
            ChangeStatus::Added => "A",
            ChangeStatus::Modified => "M",
            ChangeStatus::Deleted => "D",
            ChangeStatus::Renamed => "R",
            ChangeStatus::Copied => "C",
            ChangeStatus::TypeChanged => "T",
        }
    }

    pub(crate) fn from_letter(c: char) -> Option<Self> {
        Some(match c {
            'A' => ChangeStatus::Added,
            'M' => ChangeStatus::Modified,
            'D' => ChangeStatus::Deleted,
            'R' => ChangeStatus::Renamed,
            'C' => ChangeStatus::Copied,
            'T' => ChangeStatus::TypeChanged,
            _ => return None,
        })
    }
}

/// One entry in the changed-files rail.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChangedFile {
    /// Path in the head tree. For a deletion, the path in the base tree.
    pub path: String,
    /// Where a rename or copy came from.
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
    /// The blob sha of the head side; `None` for a deletion. Viewed is
    /// keyed by it, so a file is viewed by its content and not by which
    /// scope or commit showed it.
    pub head_blob: Option<String>,
}

impl ChangedFile {
    /// The directory portion, which the rail draws muted.
    pub fn directory(&self) -> &str {
        match self.path.rfind('/') {
            Some(i) => &self.path[..=i],
            None => "",
        }
    }

    pub fn file_name(&self) -> &str {
        match self.path.rfind('/') {
            Some(i) => &self.path[i + 1..],
            None => &self.path,
        }
    }
}

/// Status of one line of the head file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineStatus {
    Unchanged,
    Added,
    /// Part of a change that both removed and added lines.
    Modified,
}

/// Lines the pull request removed, anchored to where they used to be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeletedRun {
    /// The head line these lines sit above. Equal to the head line count when
    /// the removal was at the end of the file.
    pub before_line: usize,
    pub lines: Vec<String>,
}

/// A contiguous changed region of the head file, for next/previous change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FileDiff {
    /// One entry per line of the head file.
    pub line_status: Vec<LineStatus>,
    pub deleted: Vec<DeletedRun>,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    pub fn is_empty(&self) -> bool {
        self.hunks.is_empty() && self.deleted.is_empty()
    }

    pub fn status_at(&self, line: usize) -> LineStatus {
        self.line_status.get(line).copied().unwrap_or(LineStatus::Unchanged)
    }

    /// The first hunk starting after `line`, for the next-change key.
    pub fn next_hunk(&self, line: usize) -> Option<Hunk> {
        self.hunks.iter().find(|h| h.start > line).copied()
    }

    pub fn previous_hunk(&self, line: usize) -> Option<Hunk> {
        self.hunks.iter().rev().find(|h| h.start < line).copied()
    }
}

/// Every line of the head file marked against the base.
pub fn file_diff(base: &str, head: &str) -> FileDiff {
    let base_lines: Vec<&str> = base.lines().collect();
    let head_line_count = head.lines().count();

    let input = InternedInput::new(lines(base), lines(head));

    let mut line_status = vec![LineStatus::Unchanged; head_line_count];
    let mut deleted = Vec::new();
    let mut hunks: Vec<Hunk> = Vec::new();

    diff(Algorithm::Histogram, &input, |before: std::ops::Range<u32>, after: std::ops::Range<u32>| {
        let removed = before.len();
        let added = after.len();
        let start = after.start as usize;
        let end = after.end as usize;

        // A run that both removes and adds is a modification; one that only
        // adds is an addition. Colouring them differently is what lets a
        // reviewer tell rewritten code from new code at a glance.
        let status = if removed > 0 && added > 0 {
            LineStatus::Modified
        } else {
            LineStatus::Added
        };
        for entry in line_status.iter_mut().take(end.min(head_line_count)).skip(start) {
            *entry = status;
        }

        if removed > 0 {
            deleted.push(DeletedRun {
                before_line: start.min(head_line_count),
                lines: base_lines
                    [(before.start as usize).min(base_lines.len())..(before.end as usize).min(base_lines.len())]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            });
        }

        if added > 0 {
            hunks.push(Hunk { start, end: end.min(head_line_count) });
        } else {
            // A pure deletion has no head lines of its own, but the reviewer
            // still needs to be able to jump to it.
            hunks.push(Hunk { start, end: start });
        }
    });

    FileDiff { line_status, deleted, hunks }
}

/// How many bytes of a line git keeps after a hunk header's `@@`: the
/// size of xdiff's `func_line` buffer.
const HUNK_CONTEXT_BYTES: usize = 80;

/// What git writes after a hunk header's closing `@@` for a hunk whose
/// first line, context included, is line `first` of `lines`, zero based:
/// the nearest line above it that begins with a letter, `_` or `$`, cut to
/// eighty bytes and trimmed of trailing whitespace. `None` when no line
/// above qualifies.
///
/// This is xdiff's default funcname rule (`def_ff`), the one git applies
/// to a file whose attributes name no diff driver, which is how git's own
/// headers read `impl Manifest {` above a hunk inside the impl. git looks
/// in the old file, from the hunk up, so `lines` are the base's. The diff
/// here is computed in process and has no headers of its own, so the rule
/// is applied to the hunks the code pane shows.
pub fn hunk_context<'a>(lines: &[&'a str], first: usize) -> Option<&'a str> {
    let line = lines[..first.min(lines.len())]
        .iter()
        .rev()
        .find(|line| line.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_' || c == '$'))?;
    let mut end = line.len().min(HUNK_CONTEXT_BYTES);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    Some(line[..end].trim_end())
}

/// Parse `git diff -M --name-status -z`.
///
/// Records are NUL separated. A plain change is `STATUS\0path\0`; a rename or
/// copy is `R<score>\0old\0new\0`.
pub fn parse_name_status(raw: &str) -> Vec<(ChangeStatus, String, Option<String>)> {
    let mut out = Vec::new();
    let mut fields = raw.split('\0').filter(|f| !f.is_empty());

    while let Some(code) = fields.next() {
        let Some(status) = code.chars().next().and_then(ChangeStatus::from_letter) else {
            continue;
        };
        let Some(first) = fields.next() else { break };

        match status {
            ChangeStatus::Renamed | ChangeStatus::Copied => {
                let Some(second) = fields.next() else { break };
                out.push((status, second.to_string(), Some(first.to_string())));
            }
            _ => out.push((status, first.to_string(), None)),
        }
    }

    out
}

/// Parse `git diff -M --numstat -z`.
///
/// A plain change is `adds\tdels\tpath\0`. A rename is `adds\tdels\t\0old\0new\0`,
/// where the trailing tab leaves the path field empty and the two paths
/// follow as their own records. `-` in place of a count means binary.
pub fn parse_numstat(raw: &str) -> Vec<(u32, u32, bool, String)> {
    let mut out = Vec::new();
    let mut fields = raw.split('\0').filter(|f| !f.is_empty() || true).peekable();

    while let Some(record) = fields.next() {
        if record.is_empty() {
            continue;
        }
        let mut parts = record.splitn(3, '\t');
        let (Some(adds), Some(dels), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };

        let binary = adds == "-" || dels == "-";
        let additions = adds.parse().unwrap_or(0);
        let deletions = dels.parse().unwrap_or(0);

        if path.is_empty() {
            // Rename: the old and new paths are the next two records.
            let _old = fields.next();
            let Some(new) = fields.next() else { break };
            out.push((additions, deletions, binary, new.to_string()));
        } else {
            out.push((additions, deletions, binary, path.to_string()));
        }
    }

    out
}

/// Join the two git outputs into the rail's file list.
pub fn merge_file_lists(
    name_status: Vec<(ChangeStatus, String, Option<String>)>,
    numstat: Vec<(u32, u32, bool, String)>,
) -> Vec<ChangedFile> {
    let mut counts = std::collections::HashMap::new();
    for (adds, dels, binary, path) in numstat {
        counts.insert(path, (adds, dels, binary));
    }

    let mut files: Vec<ChangedFile> = name_status
        .into_iter()
        .map(|(status, path, old_path)| {
            let (additions, deletions, binary) =
                counts.get(&path).copied().unwrap_or((0, 0, false));
            ChangedFile { path, old_path, status, additions, deletions, binary, head_blob: None }
        })
        .collect();

    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_status_reads_plain_changes() {
        let raw = "M\0src/a.rs\0A\0src/b.rs\0D\0src/c.rs\0";
        let parsed = parse_name_status(raw);
        assert_eq!(
            parsed,
            vec![
                (ChangeStatus::Modified, "src/a.rs".into(), None),
                (ChangeStatus::Added, "src/b.rs".into(), None),
                (ChangeStatus::Deleted, "src/c.rs".into(), None),
            ]
        );
    }

    #[test]
    fn name_status_reads_renames_as_old_and_new() {
        let raw = "R100\0src/old.rs\0src/new.rs\0M\0src/a.rs\0";
        let parsed = parse_name_status(raw);
        assert_eq!(
            parsed[0],
            (ChangeStatus::Renamed, "src/new.rs".into(), Some("src/old.rs".into()))
        );
        // The record after a rename must still line up.
        assert_eq!(parsed[1], (ChangeStatus::Modified, "src/a.rs".into(), None));
    }

    #[test]
    fn numstat_reads_counts() {
        let raw = "12\t3\tsrc/a.rs\u{0}100\t0\tsrc/b.rs\u{0}";
        assert_eq!(
            parse_numstat(raw),
            vec![(12, 3, false, "src/a.rs".into()), (100, 0, false, "src/b.rs".into())]
        );
    }

    #[test]
    fn numstat_marks_binary_files() {
        let raw = "-\t-\tlogo.png\0";
        assert_eq!(parse_numstat(raw), vec![(0, 0, true, "logo.png".into())]);
    }

    #[test]
    fn numstat_reads_the_new_path_of_a_rename() {
        // The empty path field signals that two path records follow.
        let raw = "1\t1\t\0src/old.rs\0src/new.rs\0";
        let parsed = parse_numstat(raw);
        assert_eq!(parsed, vec![(1, 1, false, "src/new.rs".into())]);
    }

    #[test]
    fn merging_pairs_statuses_with_counts() {
        let ns = vec![
            (ChangeStatus::Modified, "src/b.rs".to_string(), None),
            (ChangeStatus::Added, "src/a.rs".to_string(), None),
        ];
        let numstat = vec![
            (5, 2, false, "src/b.rs".to_string()),
            (40, 0, false, "src/a.rs".to_string()),
        ];
        let merged = merge_file_lists(ns, numstat);

        // Sorted by path, so the rail is stable.
        assert_eq!(merged[0].path, "src/a.rs");
        assert_eq!(merged[0].additions, 40);
        assert_eq!(merged[1].path, "src/b.rs");
        assert_eq!(merged[1].deletions, 2);
    }

    #[test]
    fn a_file_missing_from_numstat_still_appears() {
        let ns = vec![(ChangeStatus::TypeChanged, "link".to_string(), None)];
        let merged = merge_file_lists(ns, vec![]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].additions, 0);
    }

    #[test]
    fn path_components_split_for_display() {
        let f = ChangedFile {
            path: "crates/perch-app/src/main.rs".into(),
            old_path: None,
            status: ChangeStatus::Modified,
            additions: 0,
            deletions: 0,
            binary: false,
            head_blob: None,
        };
        assert_eq!(f.directory(), "crates/perch-app/src/");
        assert_eq!(f.file_name(), "main.rs");

        let top = ChangedFile { path: "README.md".into(), ..f };
        assert_eq!(top.directory(), "");
        assert_eq!(top.file_name(), "README.md");
    }

    #[test]
    fn an_unchanged_file_has_no_hunks() {
        let text = "a\nb\nc\n";
        let d = file_diff(text, text);
        assert!(d.is_empty());
        assert!(d.line_status.iter().all(|s| *s == LineStatus::Unchanged));
    }

    #[test]
    fn appended_lines_are_marked_added() {
        let d = file_diff("a\nb\n", "a\nb\nc\nd\n");
        assert_eq!(d.line_status[0], LineStatus::Unchanged);
        assert_eq!(d.line_status[2], LineStatus::Added);
        assert_eq!(d.line_status[3], LineStatus::Added);
        assert!(d.deleted.is_empty(), "nothing was removed");
    }

    #[test]
    fn a_replaced_line_is_modified_not_added() {
        let d = file_diff("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(d.line_status[1], LineStatus::Modified);
        assert_eq!(d.deleted.len(), 1);
        assert_eq!(d.deleted[0].lines, vec!["b".to_string()]);
        assert_eq!(d.deleted[0].before_line, 1);
    }

    #[test]
    fn removed_lines_are_anchored_to_the_line_that_follows() {
        let d = file_diff("a\nb\nc\nd\n", "a\nd\n");
        assert_eq!(d.deleted.len(), 1);
        assert_eq!(d.deleted[0].lines, vec!["b".to_string(), "c".to_string()]);
        assert_eq!(d.deleted[0].before_line, 1, "ghost lines sit above head line 1");
    }

    #[test]
    fn a_removal_at_end_of_file_anchors_past_the_last_line() {
        let d = file_diff("a\nb\nc\n", "a\n");
        assert_eq!(d.deleted[0].lines, vec!["b".to_string(), "c".to_string()]);
        assert_eq!(d.deleted[0].before_line, 1);
    }

    #[test]
    fn line_status_covers_every_head_line_exactly() {
        let head = "a\nb\nc\nd\ne\n";
        let d = file_diff("a\nx\nc\n", head);
        assert_eq!(d.line_status.len(), head.lines().count());
    }

    #[test]
    fn hunks_let_the_reader_jump_forward_and_back() {
        let d = file_diff("a\nb\nc\nd\ne\n", "a\nB\nc\nD\ne\n");
        assert_eq!(d.hunks.len(), 2);

        assert_eq!(d.hunks[0].start, 1);
        assert_eq!(d.hunks[1].start, 3);
        assert_eq!(d.next_hunk(1).map(|h| h.start), Some(3));
        assert_eq!(d.previous_hunk(4).map(|h| h.start), Some(3));
        assert_eq!(d.previous_hunk(0), None);
    }

    #[test]
    fn a_new_file_is_entirely_added() {
        let d = file_diff("", "a\nb\n");
        assert!(d.line_status.iter().all(|s| *s == LineStatus::Added));
        assert!(d.deleted.is_empty());
    }

    #[test]
    fn a_deleted_file_has_no_head_lines_but_keeps_its_ghosts() {
        let d = file_diff("a\nb\n", "");
        assert!(d.line_status.is_empty());
        assert_eq!(d.deleted[0].lines, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn a_hunks_context_is_the_nearest_line_above_it_that_starts_an_identifier() {
        let base: Vec<&str> = "use std::fmt;\n\nimpl Manifest {\n    fn load() {\n        let a = 1;\n    }\n}\n".lines().collect();
        // A hunk from line five, `let a`: the indented `fn` does not count,
        // the `impl` does, as git's default rule has it.
        assert_eq!(hunk_context(&base, 4), Some("impl Manifest {"));
        // The hunk's own first line is not above it.
        assert_eq!(hunk_context(&base, 2), Some("use std::fmt;"));
        assert_eq!(hunk_context(&base, 0), None, "nothing is above the first line");
        assert_eq!(hunk_context(&base, 100), Some("impl Manifest {"), "past the end is below every line");
        assert_eq!(hunk_context(&["  a", "{"], 2), None, "no line starts an identifier");
        assert_eq!(hunk_context(&["_private", "$var", "x"], 2), Some("$var"));
    }

    #[test]
    fn a_hunks_context_is_cut_to_eighty_bytes_and_trimmed() {
        let long = format!("fn {}() {{   ", "a".repeat(100));
        let context = hunk_context(&[&long, "body"], 1).unwrap();
        assert_eq!(context.len(), 80);
        assert!(context.starts_with("fn aaa"));
        assert_eq!(hunk_context(&["struct S {   \t", "x"], 1), Some("struct S {"));
        // A cut never splits a character.
        let wide = format!("s{}", "é".repeat(60));
        assert_eq!(hunk_context(&[&wide, "x"], 1).unwrap().len(), 79);
    }

    #[test]
    fn files_without_a_trailing_newline_are_handled() {
        let d = file_diff("a\nb", "a\nb\nc");
        assert_eq!(d.line_status.len(), 3);
        assert_eq!(d.line_status[2], LineStatus::Added);
    }
}
