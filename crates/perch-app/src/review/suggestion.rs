//! Suggested changes, as github.com has them on a review comment: a
//! fence named `suggestion` holds what the commented lines should read,
//! and the comment shows it as a diff of those lines against it.
//!
//! The composer's Suggest button starts one with the lines as they are,
//! to edit; the preview and the thread draw it against the lines it
//! replaces. Those come from the file as it is loaded, where the thread's
//! numbers count there, and otherwise from the hunk GitHub sent with the
//! comment. With neither, the suggestion is drawn on its own. Against
//! them, on an open pull request, it has Apply suggestion, which commits
//! it on the head branch as github.com does.

use perch_github::{Anchor, CommentThread, Side};

use crate::review::cards::{hunk_tail, Tint};
use crate::review::model::{LoadedFile, ReviewState};

/// The lines a suggestion replaces, and the number of the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Original {
    pub first: u32,
    pub lines: Vec<String>,
}

/// A suggestion to commit on a pull request's head: the thread it is
/// in, the lines it replaces and what they become, and the commit's
/// message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub thread_id: String,
    pub path: String,
    pub original: Original,
    pub lines: Vec<String>,
    pub headline: String,
    pub body: String,
}

/// The commit message github.com starts Apply suggestion with.
pub fn headline_for(path: &str) -> String {
    format!("Update {path}")
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

/// The lines a message suggests when it is one suggestion and nothing
/// else, as the Suggest button leaves it. An empty fence suggests none,
/// taking the lines out; a fence holding one blank line suggests that.
pub fn only_suggestion(text: &str) -> Option<Vec<String>> {
    use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
    let mut events = Parser::new(text);
    match events.next()? {
        Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))
            if info.split_whitespace().next() == Some("suggestion") => {}
        _ => return None,
    }
    let mut body = String::new();
    for event in events.by_ref() {
        match event {
            Event::Text(t) => body.push_str(&t),
            Event::End(TagEnd::CodeBlock) => break,
            _ => return None,
        }
    }
    if events.next().is_some() {
        return None;
    }
    if body.is_empty() {
        return Some(Vec::new());
    }
    let body = body.strip_suffix('\n').unwrap_or(&body);
    Some(body.split('\n').map(str::to_string).collect())
}

/// `file` with `original`'s lines made `lines`, as Commit suggestion
/// does on github.com. The file must still read `original` there, so a
/// change made since the suggestion was written is not overwritten. The
/// new lines end as the old ones did, `\r\n` or `\n`.
pub fn apply(file: &str, original: &Original, lines: &[String]) -> Result<String, String> {
    let changed = || "The lines this suggestion replaces have changed".to_string();
    let first = (original.first as usize).checked_sub(1).ok_or_else(changed)?;
    let last = first + original.lines.len();
    let mut rows: Vec<&str> = file.split_inclusive('\n').collect();
    if last > rows.len() || original.lines.is_empty() {
        return Err(changed());
    }
    let unended = |row: &str| row.trim_end_matches('\n').trim_end_matches('\r').to_string();
    if rows[first..last].iter().map(|r| unended(r)).ne(original.lines.iter().cloned()) {
        return Err(changed());
    }
    let ending = if rows[first].ends_with("\r\n") { "\r\n" } else { "\n" };
    // The file's last line may have no ending; the new last line has none
    // either then.
    let open_end = last == rows.len() && !rows[last - 1].ends_with('\n');
    let mut new = String::new();
    for (i, line) in lines.iter().enumerate() {
        new.push_str(line);
        if !(open_end && i + 1 == lines.len()) {
            new.push_str(ending);
        }
    }
    if open_end && lines.is_empty() && first > 0 {
        // The last lines taken out: the one above them ends the file.
        let above = rows[first - 1];
        rows[first - 1] = above.trim_end_matches('\n').trim_end_matches('\r');
    }
    let mut out = String::with_capacity(file.len() + new.len());
    rows[..first].iter().for_each(|r| out.push_str(r));
    out.push_str(&new);
    rows[last..].iter().for_each(|r| out.push_str(r));
    Ok(out)
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

    #[test]
    fn a_message_of_one_suggestion_gives_its_lines() {
        assert_eq!(
            only_suggestion("```suggestion\n    a,\n    b,\n```"),
            Some(lines(&["    a,", "    b,"]))
        );
        assert_eq!(only_suggestion("```suggestion\n```"), Some(vec![]), "taken out");
        assert_eq!(only_suggestion("```suggestion\n\n```"), Some(lines(&[""])), "one blank line");
        assert_eq!(only_suggestion("\n```suggestion\nx\n```\n\n"), Some(lines(&["x"])));
        assert_eq!(only_suggestion("why:\n```suggestion\nx\n```"), None, "words beside it");
        assert_eq!(only_suggestion("```suggestion\nx\n```\nsee?"), None);
        assert_eq!(only_suggestion("```rust\nx\n```"), None);
        assert_eq!(only_suggestion("```suggestion\nx\n```\n```suggestion\ny\n```"), None);
    }

    #[test]
    fn a_suggestion_applies_over_the_lines_it_was_written_against() {
        let original = Original { first: 2, lines: lines(&["b", "c"]) };
        assert_eq!(apply("a\nb\nc\nd\n", &original, &lines(&["B"])), Ok("a\nB\nd\n".into()));
        assert_eq!(apply("a\nb\nc\nd\n", &original, &[]), Ok("a\nd\n".into()));
        assert_eq!(
            apply("a\r\nb\r\nc\r\n", &original, &lines(&["x", "y", "z"])),
            Ok("a\r\nx\r\ny\r\nz\r\n".into())
        );
        assert_eq!(
            apply("a\nb\nc", &original, &lines(&["x", "y"])),
            Ok("a\nx\ny".into()),
            "no ending at the end"
        );
        assert_eq!(apply("a\nb\nc", &original, &[]), Ok("a".into()));
        assert!(apply("a\nb2\nc\n", &original, &[]).is_err(), "changed since");
        assert!(apply("a\nb\n", &original, &[]).is_err(), "shorter now");
    }
}
