//! Suggested changes, as github.com has them on a review comment: a
//! fence named `suggestion` holds what the commented lines should read,
//! and the comment shows it as a diff of those lines against it.
//!
//! The composer's Suggest button starts one with the lines as they are,
//! to edit; the preview and the thread draw it against the lines it
//! replaces. Those come from the file as it is loaded, where the thread's
//! numbers count there, and otherwise from the hunk GitHub sent with the
//! comment. With neither, the suggestion is drawn on its own.

use perch_github::{Anchor, CommentThread, Side};

use crate::review::cards::{hunk_tail, Tint};
use crate::review::model::{LoadedFile, ReviewState};

/// The lines a suggestion replaces, and the number of the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Original {
    pub first: u32,
    pub lines: Vec<String>,
}

/// Whether `text` has a suggestion in it, so a body without one is drawn
/// as it always was.
pub fn has_suggestion(text: &str) -> bool {
    text.contains("```suggestion")
}

/// The head lines `anchor` is on, read from the loaded file.
pub fn from_file(file: &LoadedFile, anchor: &Anchor) -> Option<Original> {
    if anchor.side != Side::Right {
        return None;
    }
    let first = anchor.start_line.min(anchor.line);
    let last = anchor.start_line.max(anchor.line);
    let text: Vec<&str> = file.buffer.text.lines().collect();
    let lines = (first..=last)
        .map(|n| {
            let display = file.buffer.display_line(n.checked_sub(1)? as usize)?;
            Some(text.get(display).copied().unwrap_or_default().to_string())
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Original { first, lines })
}

/// The head lines `anchor` is on, read from the end of the hunk GitHub
/// sent with its comment, which runs down to the commented line. `None`
/// when the hunk does not have them all.
pub fn from_hunk(hunk: &str, anchor: &Anchor) -> Option<Original> {
    if anchor.side != Side::Right {
        return None;
    }
    let first = anchor.start_line.min(anchor.line);
    let last = anchor.start_line.max(anchor.line);
    let wanted = first as usize..=last as usize;
    let lines: Vec<String> = hunk_tail(hunk, usize::MAX)
        .into_iter()
        .filter(|l| l.tint != Tint::Removed && l.number.is_some_and(|n| wanted.contains(&n)))
        .map(|l| l.text)
        .collect();
    let count = wanted.end() - wanted.start() + 1;
    (lines.len() == count).then_some(Original { first, lines })
}

/// The lines a suggestion on `anchor` replaces: the loaded file's where
/// the anchor's numbers count in it, else the hunk's. Tracked, so a view
/// built on it follows the file as it loads.
pub fn original(
    state: ReviewState,
    anchor: &Anchor,
    outdated: bool,
    hunk: Option<&str>,
) -> Option<Original> {
    let from_loaded = || {
        let file = state.file(&anchor.path)?;
        // Under a commit's scope the file is that commit's, where the
        // numbers of a comment from GitHub may not count.
        if hunk.is_some() && file.scope != perch_git::Scope::Branch {
            return None;
        }
        from_file(&file, anchor)
    };
    let loaded = if outdated { None } else { from_loaded() };
    loaded.or_else(|| from_hunk(hunk?, anchor))
}

/// [`original`] for a thread: its anchor, and the hunk its first comment
/// from GitHub came with.
pub fn original_of(state: ReviewState, thread: &CommentThread) -> Option<Original> {
    let anchor = thread.anchor.as_ref()?;
    let hunk = thread
        .messages
        .first()
        .filter(|m| m.from_github())
        .and_then(|m| m.diff_hunk.as_deref());
    original(state, anchor, thread.outdated, hunk)
}

/// A suggestion of `lines` as they are, put in at the caret in `text`
/// with the fences on lines of their own, and where the caret goes
/// after: at the end of the last line, to edit from there, as github.com
/// leaves it.
pub fn insert(text: &str, (start, end): (usize, usize), lines: &[String]) -> (String, usize) {
    let (start, end) = (start.min(end), start.max(end).min(text.len()));
    let before = if start == 0 || text[..start].ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let after = if end == text.len() || text[end..].starts_with('\n') {
        ""
    } else {
        "\n"
    };
    let open = format!("{before}```suggestion\n");
    let body = lines.join("\n");
    let mut out = String::with_capacity(text.len() + open.len() + body.len() + 8);
    out.push_str(&text[..start]);
    out.push_str(&open);
    out.push_str(&body);
    out.push_str("\n```");
    out.push_str(after);
    out.push_str(&text[end..]);
    (out, start + open.len() + body.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(start: u32, line: u32) -> Anchor {
        Anchor {
            path: "src/x.rs".into(),
            start_line: start,
            line,
            side: Side::Right,
            commit_sha: "h".into(),
        }
    }

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_file_gives_the_head_lines_and_skips_the_removed_ones() {
        let file = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "a\nb2\nc\nd\n".into(),
            "a\nb\nc\nd\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        assert_eq!(
            from_file(&file, &anchor(1, 3)),
            Some(Original {
                first: 1,
                lines: lines(&["a", "b2", "c"])
            })
        );
        assert_eq!(
            from_file(&file, &anchor(4, 4)),
            Some(Original {
                first: 4,
                lines: lines(&["d"])
            })
        );
        assert_eq!(from_file(&file, &anchor(9, 9)), None, "past the end");
    }

    #[test]
    fn the_hunk_gives_the_commented_lines_from_its_end() {
        let hunk = "@@ -10,4 +10,5 @@ fn f() {\n a\n-b\n+b2\n+b3\n c";
        assert_eq!(
            from_hunk(hunk, &anchor(11, 13)),
            Some(Original {
                first: 11,
                lines: lines(&["b2", "b3", "c"])
            })
        );
        assert_eq!(
            from_hunk(hunk, &anchor(13, 13)),
            Some(Original {
                first: 13,
                lines: lines(&["c"])
            })
        );
        assert_eq!(from_hunk(hunk, &anchor(8, 13)), None, "not all in the hunk");
    }

    #[test]
    fn a_suggestion_goes_in_on_lines_of_its_own_with_the_caret_after_the_code() {
        let code = lines(&["    mut manifest,"]);
        let (out, at) = insert("", (0, 0), &code);
        assert_eq!(out, "```suggestion\n    mut manifest,\n```");
        assert_eq!(&out[..at], "```suggestion\n    mut manifest,");

        let (out, at) = insert("see this", (8, 8), &code);
        assert_eq!(out, "see this\n```suggestion\n    mut manifest,\n```");
        assert_eq!(&out[at..], "\n```");

        let (out, _) = insert("above\nbelow", (6, 6), &code);
        assert_eq!(out, "above\n```suggestion\n    mut manifest,\n```\nbelow");
    }

    #[test]
    fn an_empty_line_is_suggested_as_an_empty_line() {
        let (out, _) = insert("", (0, 0), &lines(&[""]));
        assert_eq!(out, "```suggestion\n\n```");
    }
}
