//! The prompt a question becomes, and the harness that runs one without a
//! window.
//!
//! The builder is a pure function so the exact text the agent sees can be
//! pinned by tests and printed by `perch --ask`. The framing goes in the
//! appended system prompt on a thread's first run; everything the reader
//! can see, the pull request, the lines, the question, is the user message.

use perch_git::{DisplayBuffer, LineStatus, Row};
use perch_github::{Anchor, Author, PullRequest};

/// What the agent is told about the tool it is inside.
pub const FRAMING: &str = "You are answering a question inside a code review tool. The reader is \
looking at a pull request checked out in the current directory. Answer \
the question about the lines given; read whatever else you need. Be \
brief. Do not change any file.";

/// How much of a pull request body goes into the prompt.
const MAX_BODY_LINES: usize = 40;

/// Everything a prompt is built from.
pub struct PromptInput<'a> {
    pub pull_request: Option<&'a PullRequest>,
    /// The description, kept separately because `PullRequest` does not
    /// carry it. Trimmed to `MAX_BODY_LINES`.
    pub body: Option<&'a str>,
    pub anchor: Option<&'a Anchor>,
    /// The anchored file as the editor shows it, so the selected lines and
    /// the lines the pull request removed beside them can be quoted.
    pub file: Option<&'a DisplayBuffer>,
    pub outdated: bool,
    pub question: &'a str,
    /// Earlier messages, only for a thread whose runs all failed and so
    /// has no session to resume. Otherwise empty: `--resume` is the memory.
    pub history: &'a [(Author, String)],
}

/// The two strings a run takes.
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    pub system: Option<String>,
    pub user: String,
}

/// A thread's first question: framing plus the whole context.
pub fn first_prompt(input: &PromptInput<'_>) -> Prompt {
    Prompt { system: Some(FRAMING.to_string()), user: user_message(input, true) }
}

/// A follow-up in a resumed session: just the question. The anchor is
/// repeated in one line so the agent knows which lines are meant when the
/// reader has scrolled on since.
pub fn follow_up_prompt(input: &PromptInput<'_>) -> Prompt {
    if input.history.is_empty() {
        let mut user = String::new();
        if let Some(anchor) = input.anchor {
            user.push_str(&format!("(About {})\n\n", anchor.label()));
        }
        user.push_str(&format!("Question: {}", input.question.trim()));
        Prompt { system: None, user }
    } else {
        // Nothing to resume: start over with the earlier exchange folded in.
        Prompt { system: Some(FRAMING.to_string()), user: user_message(input, true) }
    }
}

fn user_message(input: &PromptInput<'_>, with_context: bool) -> String {
    let mut out = String::new();

    if with_context {
        if let Some(pr) = input.pull_request {
            out.push_str(&format!("Pull request: {}#{}: {}\n", pr.repo_full_name, pr.number, pr.title));
            if let Some(body) = input.body.map(str::trim).filter(|b| !b.is_empty()) {
                let lines: Vec<&str> = body.lines().collect();
                let shown = &lines[..lines.len().min(MAX_BODY_LINES)];
                out.push_str(&shown.join("\n"));
                if lines.len() > MAX_BODY_LINES {
                    out.push_str("\n[…]");
                }
                out.push('\n');
            }
            out.push('\n');
        }

        if let Some(anchor) = input.anchor {
            let quoted = input.file.map(|f| quote(f, anchor));
            let changed = quoted.as_ref().is_some_and(|q| q.changed);
            out.push_str(&format!(
                "File: {}, lines {}{}\n",
                anchor.path,
                if anchor.start_line == anchor.line {
                    anchor.line.to_string()
                } else {
                    format!("{}\u{2013}{}", anchor.start_line, anchor.line)
                },
                if changed { " (changed in this pull request)" } else { "" }
            ));
            if input.outdated {
                out.push_str("The file has changed since this thread began, so the line numbers may be off.\n");
            }
            if let Some(q) = quoted {
                out.push_str(&fenced(&anchor.path, &q.selected));
                if !q.removed.is_empty() {
                    out.push_str("Before this pull request:\n");
                    out.push_str(&fenced(&anchor.path, &q.removed));
                }
            }
            out.push('\n');
        }
    }

    if !input.history.is_empty() {
        out.push_str("Earlier in this thread:\n\n");
        for (author, body) in input.history {
            let who = match author {
                Author::User => "Reader",
                Author::Agent { .. } => "You",
                Author::GitHubUser(login) => login.as_str(),
            };
            out.push_str(&format!("{who}: {}\n\n", body.trim()));
        }
    }

    out.push_str(&format!("Question: {}", input.question.trim()));
    out
}

struct Quoted {
    selected: String,
    removed: String,
    changed: bool,
}

/// The selected head lines, whether any of them is part of a hunk, and the
/// removed lines the diff shows beside them.
fn quote(file: &DisplayBuffer, anchor: &Anchor) -> Quoted {
    let first = anchor.start_line.saturating_sub(1) as usize;
    let last = anchor.line.saturating_sub(1) as usize;
    let lines: Vec<&str> = file.text.lines().collect();

    let mut selected = Vec::new();
    let mut removed = Vec::new();
    let mut changed = false;
    let mut first_display = None;

    for (display, row) in file.rows.iter().enumerate() {
        match row {
            Row::Head { head_line, status } if (first..=last).contains(head_line) => {
                if first_display.is_none() {
                    first_display = Some(display);
                }
                selected.push(lines.get(display).copied().unwrap_or(""));
                changed |= *status != LineStatus::Unchanged;
            }
            // Removed lines inside the range belong to the change too.
            Row::Deleted { .. } if first_display.is_some() && display_in_range(file, display, last) => {
                removed.push(lines.get(display).copied().unwrap_or(""));
            }
            _ => {}
        }
    }

    // Removed lines are spliced above the line that replaced them, so the
    // run just before the selection's first line belongs to it.
    if let Some(start) = first_display {
        let mut above = Vec::new();
        let mut d = start;
        while d > 0 {
            d -= 1;
            match file.rows[d] {
                Row::Deleted { .. } => above.push(lines.get(d).copied().unwrap_or("")),
                _ => break,
            }
        }
        above.reverse();
        above.extend(removed);
        removed = above;
    }
    if !removed.is_empty() {
        changed = true;
    }

    Quoted { selected: selected.join("\n"), removed: removed.join("\n"), changed }
}

/// Whether a deleted row sits before the last selected head line.
fn display_in_range(file: &DisplayBuffer, display: usize, last_head: usize) -> bool {
    file.rows[display..]
        .iter()
        .find_map(|r| r.head_line())
        .is_some_and(|next_head| next_head <= last_head)
}

fn fenced(path: &str, text: &str) -> String {
    let lang = path.rsplit('.').next().filter(|e| !e.contains('/')).unwrap_or("");
    format!("```{lang}\n{text}\n```\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_github::Side;

    fn pr() -> PullRequest {
        PullRequest {
            number: 812,
            title: "Fence the manifest".into(),
            author_login: "rohan".into(),
            author_avatar_url: String::new(),
            head_sha: "h".into(),
            head_ref: "f".into(),
            base_sha: "b".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            repo_full_name: "slatedb/slatedb".into(),
        }
    }

    fn anchor(start: u32, line: u32) -> Anchor {
        Anchor { path: "src/manifest.rs".into(), start_line: start, line, side: Side::Right, commit_sha: "h".into() }
    }

    /// Base line 2 was replaced by two lines; line 5 was added.
    fn buffer() -> DisplayBuffer {
        let base = "a\nold b\nc\nd\n";
        let head = "a\nnew b1\nnew b2\nc\nd\nadded\n";
        DisplayBuffer::unified(head, base, &perch_git::diff::file_diff(base, head))
    }

    fn input<'a>(anchor: Option<&'a Anchor>, file: Option<&'a DisplayBuffer>, question: &'a str) -> PromptInput<'a> {
        PromptInput { pull_request: None, body: None, anchor, file, outdated: false, question, history: &[] }
    }

    #[test]
    fn a_selection_inside_a_hunk_carries_the_removed_lines() {
        let buf = buffer();
        let a = anchor(2, 3);
        let p = first_prompt(&input(Some(&a), Some(&buf), "why?"));
        assert_eq!(p.system.as_deref(), Some(FRAMING));
        assert!(p.user.contains("File: src/manifest.rs, lines 2–3 (changed in this pull request)"), "{}", p.user);
        assert!(p.user.contains("```rs\nnew b1\nnew b2\n```"), "{}", p.user);
        assert!(p.user.contains("Before this pull request:\n```rs\nold b\n```"), "{}", p.user);
        assert!(p.user.ends_with("Question: why?"));
    }

    #[test]
    fn an_unchanged_selection_says_nothing_about_the_diff() {
        let buf = buffer();
        let a = anchor(4, 4);
        let p = first_prompt(&input(Some(&a), Some(&buf), "what?"));
        assert!(p.user.contains("File: src/manifest.rs, lines 4\n"), "{}", p.user);
        assert!(!p.user.contains("changed in this pull request"));
        assert!(!p.user.contains("Before this pull request"));
        assert!(p.user.contains("```rs\nc\n```"));
    }

    #[test]
    fn no_anchor_omits_the_file_block() {
        let p = first_prompt(&PromptInput { pull_request: Some(&pr()), body: Some("Body."), ..input(None, None, "overall?") });
        assert!(p.user.starts_with("Pull request: slatedb/slatedb#812: Fence the manifest\nBody.\n\n"), "{}", p.user);
        assert!(!p.user.contains("File:"));
        assert!(p.user.ends_with("Question: overall?"));
    }

    #[test]
    fn outdated_adds_its_line() {
        let buf = buffer();
        let a = anchor(1, 1);
        let p = first_prompt(&PromptInput { outdated: true, ..input(Some(&a), Some(&buf), "q") });
        assert!(p.user.contains("The file has changed since this thread began"));
    }

    #[test]
    fn a_long_body_is_trimmed() {
        let body: String = (1..=200).map(|i| format!("line {i}\n")).collect();
        let p = first_prompt(&PromptInput { pull_request: Some(&pr()), body: Some(&body), ..input(None, None, "q") });
        assert!(p.user.contains("line 40\n[…]\n"), "{}", p.user);
        assert!(!p.user.contains("line 41\n"));
    }

    #[test]
    fn a_follow_up_is_only_the_question() {
        let a = anchor(2, 3);
        let p = follow_up_prompt(&input(Some(&a), None, "and then?"));
        assert_eq!(p.system, None);
        assert_eq!(p.user, "(About src/manifest.rs:2–3)\n\nQuestion: and then?");
    }

    #[test]
    fn a_thread_with_nothing_to_resume_folds_its_history_in() {
        let history = vec![(Author::User, "first?".to_string()), (Author::Agent { backend: "c".into(), model: "m".into() }, "partial".to_string())];
        let p = follow_up_prompt(&PromptInput { history: &history, ..input(None, None, "again?") });
        assert!(p.system.is_some());
        assert!(p.user.contains("Earlier in this thread:\n\nReader: first?\n\nYou: partial\n\nQuestion: again?"), "{}", p.user);
    }
}
