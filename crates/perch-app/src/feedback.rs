//! Send Feedback: a bug report or an idea, filed as an issue on Perch's
//! own repository with what Perch knows about itself attached. This is
//! the part with no window and no network: the draft, the redaction,
//! the issue's text and the agent's prompt. The backend collects and
//! files (`backend::feedback`); the dropdown is `ui::feedback`.

use std::collections::VecDeque;

use jiff::Timestamp;
use regex::Regex;

use crate::state::Tone;

/// Where every report goes.
pub const REPO: &str = "perch-code/perch";

/// GitHub refuses an issue body past this many characters.
pub const BODY_LIMIT: usize = 65_536;

/// How many of the session's errors and warnings a report carries.
pub const RECENT_PROBLEMS: usize = 20;

/// How many lines of the log a report carries.
pub const LOG_LINES: usize = 300;

/// What a redacted value reads as.
pub const REDACTED: &str = "‹redacted›";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackKind {
    Bug,
    Idea,
}

impl FeedbackKind {
    pub fn label(self) -> &'static str {
        match self {
            FeedbackKind::Bug => "Bug",
            FeedbackKind::Idea => "Idea",
        }
    }

    /// The label the issue asks for. GitHub drops it for someone without
    /// push access, which is fine.
    pub fn issue_label(self) -> &'static str {
        match self {
            FeedbackKind::Bug => "bug",
            FeedbackKind::Idea => "enhancement",
        }
    }

    pub fn placeholder(self) -> &'static str {
        match self {
            FeedbackKind::Bug => "What did you do, what did you expect, and what happened instead?",
            FeedbackKind::Idea => "What would you like Perch to do?",
        }
    }
}

/// An error or warning the session showed, kept after its toast has gone.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub at: Timestamp,
    pub text: String,
    pub tone: Tone,
}

/// The session's last errors and warnings, oldest first.
pub fn remember_problem(problems: &mut VecDeque<Problem>, problem: Problem) {
    if problem.tone == Tone::Success || problem.text.is_empty() {
        return;
    }
    problems.push_back(problem);
    while problems.len() > RECENT_PROBLEMS {
        problems.pop_front();
    }
}

/// Where the draft is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Editing,
    Collecting,
    Diagnosing,
    Searching,
    Preview,
    Filing,
}

/// One heading of Diagnostics: its lines, each a `Name: value`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Section {
    pub heading: String,
    pub lines: Vec<String>,
}

/// What Perch collected, before redaction.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FeedbackContext {
    /// `Perch 0.1.0 (a1b2c3d) · macOS 15.6 (24G84) arm64 · Claude Code
    /// 2.1.230 · Codex 0.160.0`.
    pub summary: String,
    pub sections: Vec<Section>,
    pub log: Vec<String>,
}

impl FeedbackContext {
    /// The sections as Markdown, for the issue and the agent's prompt.
    pub fn diagnostics_markdown(&self) -> String {
        let mut out = String::new();
        for (i, section) in self.sections.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&format!("**{}**\n\n", section.heading));
            if section.lines.is_empty() {
                out.push_str("- none\n");
            }
            for line in &section.lines {
                out.push_str(&format!("- {line}\n"));
            }
        }
        out
    }
}

/// The agent's notes, and who wrote them.
#[derive(Debug, Clone, PartialEq)]
pub struct Notes {
    pub by: String,
    pub text: String,
}

/// Whether the viewer hears about an issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subscription {
    Subscribed,
    Unsubscribed,
    Ignored,
}

impl Subscription {
    pub fn parse(value: &str) -> Self {
        match value {
            "SUBSCRIBED" => Subscription::Subscribed,
            "IGNORED" => Subscription::Ignored,
            _ => Subscription::Unsubscribed,
        }
    }
}

/// An open issue whose title looks like the report's.
#[derive(Debug, Clone, PartialEq)]
pub struct SimilarIssue {
    pub number: u64,
    pub title: String,
    pub updated_at: Option<Timestamp>,
    pub node_id: String,
    pub thumbs_up: u64,
    pub viewer_thumbs_up: bool,
    pub viewer_subscription: Subscription,
}

/// What +1 on a similar issue does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlusOne {
    /// Add the 👍, then subscribe unless ignored.
    React,
    /// The 👍 is there; subscribe.
    Subscribe,
    /// Nothing left: `You +1'd this`.
    Done,
}

impl SimilarIssue {
    pub fn plus_one(&self) -> PlusOne {
        let wants_subscription = self.viewer_subscription == Subscription::Unsubscribed;
        if !self.viewer_thumbs_up {
            PlusOne::React
        } else if wants_subscription {
            PlusOne::Subscribe
        } else {
            PlusOne::Done
        }
    }
}

/// What went wrong sending, and whether github.com is the way round it.
#[derive(Debug, Clone, PartialEq)]
pub struct SendError {
    pub message: String,
    /// The token can't write issues on [`REPO`]: offer Open on GitHub.
    pub forbidden: bool,
}

/// What the window knows that the backend doesn't, for Diagnostics.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WindowFacts {
    /// Each screen's size and backing scale, and the window's size.
    pub display: Vec<String>,
    /// The open item's language server, as its status line says it.
    pub language_server: Option<String>,
    /// The last poll's outcome and time.
    pub last_poll: Option<String>,
}

/// Everything Preview… sends the backend.
#[derive(Debug, Clone, PartialEq)]
pub struct PrepareRequest {
    /// Which Preview… this is: an answer to an older one is dropped.
    pub attempt: u64,
    pub kind: FeedbackKind,
    pub title: String,
    pub body: String,
    pub diagnose: bool,
    pub seed: Option<Problem>,
    pub problems: Vec<Problem>,
    pub item_id: Option<String>,
    /// The model menu's pick, when there's a CLI to run.
    pub choice: Option<(perch_agent::Backend, String, perch_agent::Effort)>,
    pub facts: WindowFacts,
}

/// What Preview… came back with.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub attempt: u64,
    pub context: FeedbackContext,
    pub notes: Option<Notes>,
    pub notes_error: Option<String>,
    pub preview: String,
    pub redacted: usize,
    pub similar: Vec<SimilarIssue>,
}

/// What a send did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    Filed,
    Commented,
    /// +1, and what became of the subscription.
    PlusOned {
        reacted: bool,
        subscription: Subscribed,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subscribed {
    /// Subscribed now, or already.
    Yes,
    /// The reader had ignored it, and it stays so.
    Ignored,
    /// GitHub refused the subscribe; the 👍 stands.
    Failed,
}

impl Sent {
    /// The toast's line, and whether it's a warning.
    pub fn toast(self, number: u64) -> (String, Tone) {
        let issue = format!("{REPO}#{number}");
        match self {
            Sent::Filed => (format!("Filed {issue}"), Tone::Success),
            Sent::Commented => (format!("Commented on {issue}"), Tone::Success),
            Sent::PlusOned {
                reacted: true,
                subscription: Subscribed::Yes,
            } => (format!("+1'd {issue} and subscribed you"), Tone::Success),
            Sent::PlusOned {
                reacted: true,
                subscription: Subscribed::Ignored,
            } => (format!("+1'd {issue}"), Tone::Success),
            Sent::PlusOned {
                reacted: true,
                subscription: Subscribed::Failed,
            } => (
                format!("+1'd {issue}, but couldn't subscribe you. Watch it on github.com."),
                Tone::Warning,
            ),
            Sent::PlusOned {
                reacted: false,
                subscription: Subscribed::Failed,
            } => (
                format!("Couldn't subscribe you to {issue}. Watch it on github.com."),
                Tone::Warning,
            ),
            Sent::PlusOned { reacted: false, .. } => {
                (format!("Subscribed you to {issue}"), Tone::Success)
            }
        }
    }
}

/// The report, from the first keystroke until it's sent or Perch quits.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedbackDraft {
    pub kind: FeedbackKind,
    pub title: String,
    pub body: String,
    pub diagnose: bool,
    /// The toast being reported.
    pub seed: Option<Problem>,
    /// Another toast's Report, waiting on Keep mine or Replace.
    pub offer: Option<Problem>,
    pub step: Step,
    pub context: Option<FeedbackContext>,
    pub notes: Option<Notes>,
    /// Why there are no notes: stopped, failed, timed out.
    pub notes_error: Option<String>,
    /// The issue's text as it will be sent.
    pub preview: String,
    /// How many values the redaction replaced.
    pub redacted: usize,
    /// Set when the pass before sending caught something new.
    pub caught_on_send: usize,
    pub similar: Vec<SimilarIssue>,
    pub error: Option<SendError>,
    /// What's being sent, while it is.
    pub pending: Option<Pending>,
    /// Which Preview… the draft waits on.
    pub attempt: u64,
    /// Rises when text is put in from outside the form, which rebuilds
    /// its fields.
    pub generation: u64,
}

/// A send in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    File,
    Comment(u64),
    PlusOne(u64),
}

impl FeedbackDraft {
    pub fn new(kind: FeedbackKind) -> Self {
        Self {
            kind,
            title: String::new(),
            body: String::new(),
            diagnose: kind == FeedbackKind::Bug,
            seed: None,
            offer: None,
            step: Step::Editing,
            context: None,
            notes: None,
            notes_error: None,
            preview: String::new(),
            redacted: 0,
            caught_on_send: 0,
            similar: Vec::new(),
            error: None,
            pending: None,
            attempt: 0,
            generation: 0,
        }
    }

    /// A draft about a toast: a Bug, titled with the toast's first line.
    pub fn from_problem(problem: Problem) -> Self {
        let mut draft = Self::new(FeedbackKind::Bug);
        draft.title = title_from(&problem.text);
        draft.seed = Some(problem);
        draft
    }

    /// Whether anything has been typed or made, so replacing it would
    /// lose something.
    pub fn has_content(&self) -> bool {
        !self.title.trim().is_empty()
            || !self.body.trim().is_empty()
            || self.context.is_some()
            || self.step != Step::Editing
    }

    /// Switch Bug and Idea. The diagnosis box follows the kind.
    pub fn set_kind(&mut self, kind: FeedbackKind) {
        if self.kind != kind {
            self.kind = kind;
            self.diagnose = kind == FeedbackKind::Bug;
        }
    }

    /// Back to the form from the preview: what was collected goes, since
    /// the form's text may change what's worth sending.
    pub fn back_to_form(&mut self) {
        self.step = Step::Editing;
        self.context = None;
        self.notes = None;
        self.notes_error = None;
        self.preview.clear();
        self.redacted = 0;
        self.caught_on_send = 0;
        self.similar.clear();
        self.error = None;
        self.pending = None;
        // An answer to the Preview… left behind is dropped.
        self.attempt += 1;
    }
}

/// A title from a toast's text: its first line, cut to 80 characters.
pub fn title_from(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= 80 {
        return line.to_string();
    }
    let cut: String = line.chars().take(79).collect();
    format!("{}…", cut.trim_end())
}

/// What the redaction takes out, beside the patterns it always looks for.
#[derive(Debug, Clone, Default)]
pub struct Secrets {
    /// Values never to publish: the token, and every secret-looking
    /// variable in Perch's environment.
    pub exact: Vec<String>,
    /// `$HOME`, written as `~`.
    pub home: Option<String>,
    /// The login name, wherever a path still has it.
    pub user: Option<String>,
    /// Every repository Perch knows other than [`REPO`], as `owner/repo`.
    pub repos: Vec<String>,
    /// Their checkouts' directories, as absolute paths.
    pub repo_paths: Vec<String>,
}

fn token_patterns() -> &'static [Regex] {
    static PATTERNS: std::sync::OnceLock<Vec<Regex>> = std::sync::OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"\bgh[opusr]_[A-Za-z0-9]{20,}",
            r"\bgithub_pat_[A-Za-z0-9_]{20,}",
            r"\bsk-ant-[A-Za-z0-9_\-]{10,}",
            r"\bsk-[A-Za-z0-9_\-]{20,}",
            r"\bxox[abp]-[A-Za-z0-9\-]{10,}",
            r"\bAKIA[0-9A-Z]{16}\b",
        ]
        .iter()
        .map(|p| Regex::new(p).expect("token pattern"))
        .collect()
    })
}

fn header_pattern() -> &'static Regex {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)\b(authorization:\s*(?:bearer\s+|token\s+|basic\s+)?|bearer\s+)([A-Za-z0-9._~+/=\-]{8,})")
            .expect("header pattern")
    })
}

fn email_pattern() -> &'static Regex {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,}")
            .expect("email pattern")
    })
}

/// Take out what shouldn't reach a public issue. Returns the text and
/// how many values were replaced; `$HOME` turning into `~` isn't counted,
/// since nothing is hidden by it.
pub fn redact(text: &str, secrets: &Secrets) -> (String, usize) {
    let mut out = text.to_string();
    let mut count = 0;

    let mut exact: Vec<&String> = secrets.exact.iter().filter(|v| v.len() >= 6).collect();
    // The longest first, so a value holding another goes whole.
    exact.sort_by_key(|v| std::cmp::Reverse(v.len()));
    for value in exact {
        count += replace_all(&mut out, value, REDACTED);
    }

    for pattern in token_patterns() {
        let found = pattern.find_iter(&out).count();
        if found > 0 {
            out = pattern.replace_all(&out, REDACTED).into_owned();
            count += found;
        }
    }
    let found = header_pattern().captures_iter(&out).count();
    if found > 0 {
        out = header_pattern()
            .replace_all(&out, |caps: &regex::Captures| {
                if caps[2].contains(REDACTED) {
                    caps[0].to_string()
                } else {
                    format!("{}{REDACTED}", &caps[1])
                }
            })
            .into_owned();
        count += found;
    }

    let mut paths: Vec<&String> = secrets.repo_paths.iter().filter(|p| p.len() > 1).collect();
    paths.sort_by_key(|p| std::cmp::Reverse(p.len()));
    for path in paths {
        count += replace_all(&mut out, path, "‹repo›");
    }

    if let Some(home) = secrets.home.as_deref().filter(|h| h.len() > 1) {
        let home = home.trim_end_matches('/');
        replace_all(&mut out, home, "~");
    }

    if let Some(user) = secrets.user.as_deref().filter(|u| u.len() >= 3) {
        let pattern =
            Regex::new(&format!(r"/{}(/|\b)", regex::escape(user))).expect("user pattern");
        let found = pattern.find_iter(&out).count();
        if found > 0 {
            out = pattern.replace_all(&out, "/‹user›$1").into_owned();
            count += found;
        }
    }

    let mut emails = 0;
    out = email_pattern()
        .replace_all(&out, |caps: &regex::Captures| {
            let email = &caps[0];
            let lower = email.to_ascii_lowercase();
            if lower.ends_with("noreply.github.com") || lower.starts_with("git@") {
                email.to_string()
            } else {
                emails += 1;
                "‹email›".to_string()
            }
        })
        .into_owned();
    count += emails;

    let mut repos: Vec<&String> = secrets
        .repos
        .iter()
        .filter(|r| r.contains('/') && !r.eq_ignore_ascii_case(REPO))
        .collect();
    repos.sort_by_key(|r| std::cmp::Reverse(r.len()));
    for repo in repos {
        let pattern =
            Regex::new(&format!(r"(?i)\b{}\b", regex::escape(repo))).expect("repo pattern");
        let found = pattern.find_iter(&out).count();
        if found > 0 {
            out = pattern.replace_all(&out, "‹repo›").into_owned();
            count += found;
        }
    }

    (out, count)
}

fn replace_all(text: &mut String, needle: &str, with: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let found = text.matches(needle).count();
    if found > 0 {
        *text = text.replace(needle, with);
    }
    found
}

/// Whether `text` still holds the token, which a send must never do.
pub fn holds(text: &str, value: &str) -> bool {
    !value.is_empty() && text.contains(value)
}

/// The issue's text: the user's words, the summary line, then the
/// agent's notes, Diagnostics and the log, each folded. An Idea sent
/// without a diagnosis leaves Diagnostics and the log out.
pub fn render(
    kind: FeedbackKind,
    body: &str,
    diagnose: bool,
    context: &FeedbackContext,
    notes: Option<&Notes>,
) -> String {
    let with_details = kind == FeedbackKind::Bug || diagnose;
    let head = {
        let mut out = String::new();
        let body = body.trim();
        if !body.is_empty() {
            out.push_str(body);
            out.push_str("\n\n");
        }
        out.push_str("---\n\n");
        out.push_str(&context.summary);
        out.push('\n');
        if let Some(notes) = notes.filter(|n| !n.text.trim().is_empty()) {
            out.push_str(&format!(
                "\n<details><summary>Agent notes ({}, unverified)</summary>\n\n{}\n\n</details>\n",
                notes.by,
                notes.text.trim()
            ));
        }
        if with_details && !context.sections.is_empty() {
            out.push_str(&format!(
                "\n<details><summary>Diagnostics</summary>\n\n{}\n</details>\n",
                context.diagnostics_markdown()
            ));
        }
        out
    };
    let foot = "\nFiled from Perch's Send Feedback.\n";
    if !with_details || context.log.is_empty() {
        return format!("{head}{foot}");
    }
    // The log is what gives way when the body is too long: its oldest
    // lines first.
    let mut skip = 0;
    loop {
        let log = &context.log[skip..];
        let text = format!("{head}{}{foot}", log_block(log, skip));
        if text.chars().count() <= BODY_LIMIT || log.is_empty() {
            return text;
        }
        let over = text.chars().count() - BODY_LIMIT;
        // Cut roughly as many lines as the overflow needs, at least one.
        let mut cut = 0;
        let mut chars = 0;
        for line in log {
            cut += 1;
            chars += line.chars().count() + 1;
            if chars >= over {
                break;
            }
        }
        skip += cut.max(1);
    }
}

fn log_block(log: &[String], cut: usize) -> String {
    if log.is_empty() {
        return format!(
            "\n<details><summary>Log</summary>\n\n{cut} lines cut to fit.\n\n</details>\n"
        );
    }
    let fence = if log.iter().any(|line| line.contains("```")) {
        "````"
    } else {
        "```"
    };
    let note = if cut > 0 {
        format!("{cut} earlier lines cut to fit.\n\n")
    } else {
        String::new()
    };
    format!(
        "\n<details><summary>Log (last {} lines)</summary>\n\n{note}{fence}text\n{}\n{fence}\n\n</details>\n",
        log.len(),
        log.join("\n")
    )
}

/// The words of a title worth searching on: the longest few, as GitHub's
/// search wants every word it's given.
pub fn search_query(title: &str) -> Option<String> {
    const STOP: &[&str] = &[
        "the", "and", "for", "not", "with", "when", "this", "that", "from", "into", "does", "did",
        "has", "have", "was", "are", "can", "can't", "cannot", "doesn't", "didn't", "isn't", "its",
        "after", "before", "while", "should", "would", "could", "perch",
    ];
    let mut words: Vec<String> = title
        .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '\''))
        .map(|w| w.trim_matches(|c| c == '-' || c == '\'').to_lowercase())
        .filter(|w| w.chars().count() >= 3 && !STOP.contains(&w.as_str()))
        .collect();
    let mut seen = std::collections::HashSet::new();
    words.retain(|w| seen.insert(w.clone()));
    if words.is_empty() {
        return None;
    }
    // Keep the title's order among the longest three.
    let mut by_length = words.clone();
    by_length.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
    by_length.truncate(3);
    let picked: Vec<String> = words
        .into_iter()
        .filter(|w| by_length.contains(w))
        .collect();
    Some(format!(
        "repo:{REPO} is:issue is:open in:title {}",
        picked.join(" ")
    ))
}

/// The question the agent is asked.
pub fn prompt(
    kind: FeedbackKind,
    title: &str,
    body: &str,
    context: &FeedbackContext,
    logs_dir: &str,
) -> String {
    let body = if body.trim().is_empty() {
        "(no description)"
    } else {
        body.trim()
    };
    format!(
        "Someone using Perch, a macOS app for GitHub pull requests with coding agents, is \
reporting a {kind} to Perch's maintainers. Find out why it happened on this machine.\n\n\
## Their report\n\nTitle: {title}\n\n{body}\n\n\
## What Perch collected\n\n{summary}\n\n{diagnostics}\n\
Perch's own logs are in `{logs_dir}` (`perch.<date>.log`, one a day). Its config is \
`~/.perch/config.toml`.\n\n\
## What to do\n\n\
- Look into the reported problem on this machine: read Perch's logs and config, and run \
read-only commands such as `ps`, `which`, `--version`, `rustup show`. A command the sandbox \
refuses is fine: note it and move on.\n\
- When they bear on it, read build and toolchain files in this directory \
(`rust-toolchain.toml`, `Cargo.toml`, `.cargo/config.toml`). Don't read the project's source.\n\
- Don't print tokens, keys or other secrets, even if you come across them.\n\n\
Answer in GitHub Markdown, under 400 words, in three parts with these headings:\n\n\
### Checked\nEach command or file you looked at and what it showed.\n\n\
### Likely cause\nWhat you think is wrong, and what would fix it.\n\n\
### Not sure\nWhat you couldn't confirm.\n",
        kind = kind.label().to_lowercase(),
        summary = context.summary,
        diagnostics = context.diagnostics_markdown(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> FeedbackContext {
        FeedbackContext {
            summary: "Perch 0.1.0 (a1b2c3d) · macOS 15.6 (24G84) arm64".into(),
            sections: vec![Section {
                heading: "Perch".into(),
                lines: vec!["Version: 0.1.0 (a1b2c3d)".into()],
            }],
            log: vec!["line one".into(), "line two".into()],
        }
    }

    #[test]
    fn recent_problems_hold_twenty_and_skip_successes() {
        let mut problems = VecDeque::new();
        for i in 0..25 {
            remember_problem(
                &mut problems,
                Problem {
                    at: Timestamp::UNIX_EPOCH,
                    text: format!("problem {i}"),
                    tone: Tone::Error,
                },
            );
        }
        remember_problem(
            &mut problems,
            Problem {
                at: Timestamp::UNIX_EPOCH,
                text: "it worked".into(),
                tone: Tone::Success,
            },
        );
        assert_eq!(problems.len(), RECENT_PROBLEMS);
        assert_eq!(problems.front().unwrap().text, "problem 5");
        assert_eq!(problems.back().unwrap().text, "problem 24");
    }

    #[test]
    fn a_title_from_a_toast_is_its_first_line_cut_to_eighty() {
        assert_eq!(
            title_from("rust-analyzer failed\nusage: …"),
            "rust-analyzer failed"
        );
        let long = "x".repeat(100);
        let title = title_from(&long);
        assert_eq!(title.chars().count(), 80);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn redaction_replaces_and_counts_each_kind() {
        let secrets = Secrets {
            exact: vec!["s3cret-value-123".into()],
            home: Some("/Users/alice".into()),
            user: Some("alice".into()),
            repos: vec!["acme/rocket".into(), REPO.into()],
            repo_paths: vec!["/Users/alice/Code/rocket".into()],
        };
        let text = "\
token s3cret-value-123
gh ghp_abcdefghijklmnopqrstuvwxyz0123456789
pat github_pat_11ABCDEFG0123456789_abcdefghijklmnop
claude sk-ant-api03-abcdefghijklmnop
Authorization: Bearer abcdef0123456789
path /Users/alice/.perch/logs/perch.log
root /Users/alice/Code/rocket/src
other /private/tmp/alice/x
mail alice@example.com and 123+alice@users.noreply.github.com
remote git@github.com:acme/rocket.git
repo ACME/rocket and perch-code/perch";
        let (out, count) = redact(text, &secrets);
        assert!(!out.contains("s3cret"), "{out}");
        assert!(!out.contains("ghp_"), "{out}");
        assert!(!out.contains("github_pat_"), "{out}");
        assert!(!out.contains("sk-ant-"), "{out}");
        assert!(out.contains("Authorization: Bearer ‹redacted›"), "{out}");
        assert!(out.contains("path ~/.perch/logs/perch.log"), "{out}");
        assert!(out.contains("root ‹repo›/src"), "{out}");
        assert!(out.contains("other /private/tmp/‹user›/x"), "{out}");
        assert!(
            out.contains("mail ‹email› and 123+alice@users.noreply.github.com"),
            "{out}"
        );
        assert!(out.contains("git@github.com:‹repo›.git"), "{out}");
        assert!(out.contains("repo ‹repo› and perch-code/perch"), "{out}");
        // token, ghp, pat, sk-ant, bearer, root path, user, email, two repos
        assert_eq!(count, 10, "{out}");
    }

    #[test]
    fn redacting_twice_finds_nothing_new() {
        let secrets = Secrets {
            exact: vec!["s3cret-value-123".into()],
            home: Some("/Users/alice".into()),
            user: Some("alice".into()),
            repos: vec!["acme/rocket".into()],
            repo_paths: vec![],
        };
        let (once, _) = redact(
            "a s3cret-value-123 b /Users/alice/x acme/rocket bob@x.io",
            &secrets,
        );
        let (twice, count) = redact(&once, &secrets);
        assert_eq!(once, twice);
        assert_eq!(count, 0, "{twice}");
    }

    #[test]
    fn the_issue_has_its_parts_in_order() {
        let notes = Notes {
            by: "Claude".into(),
            text: "### Checked\nps".into(),
        };
        let text = render(
            FeedbackKind::Bug,
            "It broke.",
            true,
            &context(),
            Some(&notes),
        );
        let body = text.find("It broke.").unwrap();
        let summary = text.find("Perch 0.1.0 (a1b2c3d)").unwrap();
        let notes_at = text.find("Agent notes (Claude, unverified)").unwrap();
        let diagnostics = text.find("<summary>Diagnostics</summary>").unwrap();
        let log = text.find("Log (last 2 lines)").unwrap();
        assert!(
            body < summary && summary < notes_at && notes_at < diagnostics && diagnostics < log
        );
        assert!(text.ends_with("Filed from Perch's Send Feedback.\n"));
    }

    #[test]
    fn notes_only_when_there_are_some() {
        let text = render(FeedbackKind::Bug, "x", true, &context(), None);
        assert!(!text.contains("Agent notes"));
    }

    #[test]
    fn an_idea_without_diagnosis_keeps_only_the_summary() {
        let text = render(FeedbackKind::Idea, "Add a thing.", false, &context(), None);
        assert!(text.contains("Perch 0.1.0"));
        assert!(!text.contains("Diagnostics"));
        assert!(!text.contains("<summary>Log"));
        let text = render(FeedbackKind::Idea, "Add a thing.", true, &context(), None);
        assert!(text.contains("Diagnostics"));
    }

    #[test]
    fn an_oversized_log_is_cut_from_its_oldest_end() {
        let mut context = context();
        context.log = (0..2000)
            .map(|i| format!("{i:05} {}", "x".repeat(60)))
            .collect();
        let text = render(FeedbackKind::Bug, "x", true, &context, None);
        assert!(text.chars().count() <= BODY_LIMIT);
        assert!(text.contains("earlier lines cut to fit"));
        assert!(text.contains("01999 "), "keeps the newest");
        assert!(!text.contains("\n00000 "), "drops the oldest");
    }

    #[test]
    fn a_log_with_a_fence_gets_a_longer_one() {
        let mut context = context();
        context.log = vec!["```".into()];
        let text = render(FeedbackKind::Bug, "x", true, &context, None);
        assert!(text.contains("````text"));
    }

    #[test]
    fn the_search_takes_the_titles_longest_words() {
        assert_eq!(
            search_query("rust-analyzer failed: the language server did not answer in time")
                .as_deref(),
            Some("repo:perch-code/perch is:issue is:open in:title rust-analyzer failed language")
        );
        assert_eq!(search_query("a b"), None);
    }

    #[test]
    fn plus_one_does_what_is_left() {
        let mut issue = SimilarIssue {
            number: 57,
            title: "x".into(),
            updated_at: None,
            node_id: "I_1".into(),
            thumbs_up: 2,
            viewer_thumbs_up: false,
            viewer_subscription: Subscription::Unsubscribed,
        };
        assert_eq!(issue.plus_one(), PlusOne::React);
        issue.viewer_thumbs_up = true;
        assert_eq!(issue.plus_one(), PlusOne::Subscribe);
        issue.viewer_subscription = Subscription::Subscribed;
        assert_eq!(issue.plus_one(), PlusOne::Done);
        issue.viewer_subscription = Subscription::Ignored;
        assert_eq!(issue.plus_one(), PlusOne::Done);
    }

    #[test]
    fn the_prompt_has_the_report_and_the_logs() {
        let prompt = prompt(
            FeedbackKind::Bug,
            "RA failed",
            "It hung.",
            &context(),
            "/x/logs",
        );
        assert!(prompt.contains("RA failed"));
        assert!(prompt.contains("It hung."));
        assert!(prompt.contains("Perch 0.1.0"));
        assert!(prompt.contains("`/x/logs`"));
        assert!(prompt.contains("### Likely cause"));
    }

    #[test]
    fn a_draft_from_a_toast_is_a_bug_with_its_title() {
        let draft = FeedbackDraft::from_problem(Problem {
            at: Timestamp::UNIX_EPOCH,
            text: "rust-analyzer failed: no".into(),
            tone: Tone::Error,
        });
        assert_eq!(draft.kind, FeedbackKind::Bug);
        assert!(draft.diagnose);
        assert_eq!(draft.title, "rust-analyzer failed: no");
        let mut idea = FeedbackDraft::new(FeedbackKind::Idea);
        assert!(!idea.diagnose);
        idea.set_kind(FeedbackKind::Bug);
        assert!(idea.diagnose);
    }
}
