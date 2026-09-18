//! The prompt a question becomes, and the harness that runs one without a
//! window.
//!
//! The builder is a pure function so the exact text the agent sees can be
//! pinned by tests and printed by `perch --ask`. The framing goes in the
//! appended system prompt on a thread's first run; everything the reader
//! can see, the pull request, the lines, the question, is the user message.

use perch_git::{DisplayBuffer, LineStatus, Row};
use perch_github::{Anchor, Author, PullRequest};

/// Fold one event from the agent into its run record.
///
/// The backend applies this to the row it persists and the UI applies it to
/// the copy it draws, so the two cannot drift. Returns the text the agent
/// said, if the event carried any, for the caller to collect.
pub fn apply_run_event(run: &mut perch_github::RunRecord, event: &perch_agent::RunEvent) -> Option<String> {
    use perch_agent::RunEvent;
    use perch_github::{RunStatus, RunStep};

    match event {
        RunEvent::Started { session_id, .. } => {
            run.session_id = Some(session_id.clone());
        }
        RunEvent::Text(text) => return Some(text.clone()),
        RunEvent::ToolCall { summary, .. } => {
            run.steps.push(RunStep { summary: summary.clone(), ok: true });
        }
        RunEvent::ToolResult { ok, summary, .. } => {
            // Results come back in order, so the last step still marked good
            // is the one this answers.
            if !ok {
                if let Some(step) = run.steps.iter_mut().rev().find(|s| s.ok) {
                    step.ok = false;
                    if !summary.is_empty() {
                        step.summary = format!("{}: {summary}", step.summary);
                    }
                }
            }
        }
        RunEvent::Finished { session_id, cost_usd, duration_ms } => {
            run.session_id = Some(session_id.clone());
            run.status = RunStatus::Finished;
            run.cost_usd = *cost_usd;
            run.duration_ms = Some(*duration_ms);
            run.ended_at = Some(jiff::Timestamp::now());
        }
        RunEvent::Failed { message } => {
            run.status = if message == "Stopped" { RunStatus::Stopped } else { RunStatus::Failed };
            run.error = Some(message.clone());
            run.ended_at = Some(jiff::Timestamp::now());
        }
    }
    None
}

/// Which backend runs a model, as config decides it. Config speaks in
/// names so it need not depend on `perch-agent`; a name it does not know
/// is Claude Code, the same fallback config itself makes.
pub fn backend_for(agent: &perch_github::AgentConfig, model: &str) -> perch_agent::Backend {
    perch_agent::Backend::parse(agent.backend_of(model)).unwrap_or(perch_agent::Backend::ClaudeCode)
}

/// The config key to point at when a backend's binary cannot be started.
pub fn binary_hint(backend: perch_agent::Backend) -> &'static str {
    match backend {
        perch_agent::Backend::ClaudeCode => "Set agent.binary in config.toml to where Claude Code is installed.",
        perch_agent::Backend::Codex => "Set agent.codex.binary in config.toml to where Codex is installed.",
    }
}

/// The spec a run takes, from config, the prompt, and where to run it.
///
/// The tool list goes along whatever the backend; Claude Code reads it
/// and Codex ignores it in favour of its sandbox.
pub fn run_spec(
    agent: &perch_github::AgentConfig,
    backend: perch_agent::Backend,
    model: &str,
    effort: perch_agent::Effort,
    cwd: std::path::PathBuf,
    prompt: &Prompt,
    resume: Option<String>,
) -> perch_agent::RunSpec {
    perch_agent::RunSpec {
        backend,
        binary: agent.binary_for(backend.name()).into(),
        model: model.to_string(),
        effort,
        cwd,
        prompt: prompt.user.clone(),
        system: prompt.system.clone(),
        resume,
        allowed_tools: perch_agent::READ_ONLY_TOOLS.iter().map(|t| t.to_string()).collect(),
    }
}

/// `perch --ask owner/repo#N [path:start-end] "question"`.
///
/// Opens the worktree, builds the prompt exactly as the app would, runs it,
/// and prints each event on its own line, then the answer. How the prompt
/// and the parser get checked without a window.
pub fn run_cli(args: &[String]) {
    let (target, anchor, question) = match args {
        [target, anchor, question] => (target, Some(anchor.as_str()), question),
        [target, question] => (target, None, question),
        _ => {
            eprintln!("usage: perch --ask owner/repo#number [path:start-end] \"question\"");
            std::process::exit(2);
        }
    };
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("runtime");
    if let Err(e) = runtime.block_on(ask(target, anchor, question)) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn ask(target: &str, anchor: Option<&str>, question: &str) -> Result<(), String> {
    use perch_git::{Preparer, RepoId, Store};
    use perch_github::{resolve_token, Config, GitHub, Side};

    let (full_name, number) = target.split_once('#').ok_or("expected owner/repo#number")?;
    let number: u64 = number.parse().map_err(|_| "bad number")?;
    let id = RepoId::parse(full_name).ok_or("bad repo")?;

    let config = Config::load();
    let token = resolve_token(&config).ok_or("no token")?;
    let gh = GitHub::new(&token.value).map_err(|e| e.to_string())?;
    let root = perch_github::config::data_dir().ok_or("no data dir")?;
    let preparer = Preparer::new(Store::new(root), Some(token.value.clone()));

    // A number is a pull request or an issue; GitHub says which. An issue
    // is asked about from the repository's home checkout, with no anchor.
    let (pr, issue, checkout) = match gh.fetch_pull_request(full_name, number).await {
        Ok(pr) => {
            let checkout = preparer
                .prepare(&id, number, &pr.head_sha, &pr.base_ref, &format!("perch/pr-{number}"), |_| {})
                .await
                .map_err(|e| e.to_string())?;
            (Some(pr), None, checkout)
        }
        Err(_) => {
            let issue = gh.fetch_issue(full_name, number).await.map_err(|e| e.to_string())?;
            let comments = gh.fetch_issue_comments(full_name, number).await.unwrap_or_default();
            let bare = preparer.ensure_bare(&id, |_| {}).await.map_err(|e| e.to_string())?;
            let default = preparer.default_branch(&id).await.map_err(|e| e.to_string())?;
            let home = preparer.ensure_home(&id, &default).await.map_err(|e| e.to_string())?;
            let head = preparer
                .git()
                .run(Some(&home), ["rev-parse", "HEAD"], std::time::Duration::from_secs(30))
                .await
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let checkout = perch_git::Checkout {
                worktree: home,
                bare,
                branch: default,
                merge_base: head.clone(),
                head_sha: head,
                stale: None,
            };
            (None, Some((issue, comments)), checkout)
        }
    };
    let body = match &pr {
        Some(_) => gh.fetch_discussion(full_name, number).await.map(|d| d.body).unwrap_or_default(),
        None => String::new(),
    };

    // The anchor, and the file as the editor would show it.
    let (anchor, file) = match anchor {
        Some(spec) if pr.is_some() => {
            let (path, range) = spec.rsplit_once(':').ok_or("anchor is path:start-end")?;
            let (start, end) = match range.split_once('-') {
                Some((a, b)) => (a.parse::<u32>().map_err(|_| "bad line")?, b.parse::<u32>().map_err(|_| "bad line")?),
                None => {
                    let n = range.parse::<u32>().map_err(|_| "bad line")?;
                    (n, n)
                }
            };
            let anchor = Anchor {
                path: path.to_string(),
                start_line: start,
                line: end,
                side: Side::Right,
                commit_sha: checkout.head_sha.clone(),
            };
            let file = load_display(&preparer, &checkout, path).await?;
            (Some(anchor), Some(file))
        }
        Some(_) => return Err("an anchor needs a pull request".into()),
        None => (None, None),
    };

    let prompt = first_prompt(&PromptInput {
        pull_request: pr.as_ref(),
        issue: issue.as_ref().map(|(i, c)| (i, c.as_slice())),
        body: Some(&body),
        anchor: anchor.as_ref(),
        file: file.as_ref(),
        outdated: false,
        question,
        history: &[],
    });
    println!("--- system ---\n{}\n--- user ---\n{}\n--- run ---", prompt.system.as_deref().unwrap_or(""), prompt.user);

    let effort = perch_agent::Effort::parse(&config.agent.effort).unwrap_or_default();
    let backend = backend_for(&config.agent, &config.agent.model);
    let spec = run_spec(&config.agent, backend, &config.agent.model, effort, checkout.worktree.clone(), &prompt, None);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    perch_agent::Run::spawn(spec, tx).map_err(|e| e.to_string())?;

    let mut answer = Vec::new();
    while let Some(event) = rx.recv().await {
        println!("{event:?}");
        match event {
            perch_agent::RunEvent::Text(t) => answer.push(t),
            perch_agent::RunEvent::Failed { message } => return Err(message),
            _ => {}
        }
    }
    println!("--- answer ---\n{}", answer.join("\n\n"));
    Ok(())
}

/// The file the way the editor lays it out, diff spliced in.
pub(crate) async fn load_display(
    preparer: &perch_git::Preparer,
    checkout: &perch_git::Checkout,
    path: &str,
) -> Result<DisplayBuffer, String> {
    let head = std::fs::read_to_string(checkout.worktree.join(path)).map_err(|e| e.to_string())?;
    let base = preparer
        .show_blob(&checkout.bare, &checkout.merge_base, path)
        .await
        .unwrap_or_default();
    Ok(if base.is_empty() {
        DisplayBuffer::plain(&head)
    } else {
        DisplayBuffer::unified(&head, &base, &perch_git::diff::file_diff(&base, &head))
    })
}

/// What the agent is told about the tool it is inside.
pub const FRAMING: &str = "You are answering a question inside a code review tool. The reader is \
looking at a pull request checked out in the current directory. Answer \
the question about the lines given; read whatever else you need. Be \
brief. Do not change any file.";

/// How much of a pull request body goes into the prompt.
const MAX_BODY_LINES: usize = 40;
/// How many of an issue's comments, and how much of each.
const MAX_COMMENTS: usize = 12;
const MAX_COMMENT_LINES: usize = 12;

/// Everything a prompt is built from.
pub struct PromptInput<'a> {
    pub pull_request: Option<&'a PullRequest>,
    /// An issue and its comments, in place of the pull request block, for
    /// a question asked from the issue view.
    pub issue: Option<(&'a perch_github::Issue, &'a [perch_github::IssueComment])>,
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
        } else if let Some((issue, comments)) = input.issue {
            out.push_str(&format!("Issue: {}#{}: {}\n", issue.repo_full_name, issue.number, issue.title));
            let body = issue.body.trim();
            if !body.is_empty() {
                let lines: Vec<&str> = body.lines().collect();
                let shown = &lines[..lines.len().min(MAX_BODY_LINES)];
                out.push_str(&shown.join("\n"));
                if lines.len() > MAX_BODY_LINES {
                    out.push_str("\n[…]");
                }
                out.push('\n');
            }
            // The last few comments, each cut short: what was said is the
            // context, not every word of it.
            let recent: Vec<&perch_github::IssueComment> = comments.iter().rev().take(MAX_COMMENTS).collect();
            if !recent.is_empty() {
                out.push_str("\nComments:\n");
                for c in recent.into_iter().rev() {
                    let lines: Vec<&str> = c.body.trim().lines().collect();
                    let shown = &lines[..lines.len().min(MAX_COMMENT_LINES)];
                    out.push_str(&format!("{}: {}\n", c.user, shown.join("\n")));
                    if lines.len() > MAX_COMMENT_LINES {
                        out.push_str("[…]\n");
                    }
                }
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

/// A GitHub comment quoted above a question, as "Ask about this" starts
/// the draft: the author and a permalink on the first line, then the body
/// line by line, then a blank line for the reader to type under. The quote
/// is the only link the thread keeps to the comment; nothing else points
/// back at it.
pub fn quote_for(author: &str, url: &str, body: &str) -> String {
    let mut out = format!("> **{author}** [commented]({url}):\n");
    for line in body.trim_end().lines() {
        if line.is_empty() {
            out.push_str(">\n");
        } else {
            out.push_str("> ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push('\n');
    out
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
            head_repo_full_name: None,
            updated_at: None,
            review_requested: false,
            checks: None,
            review_decision: None,
            merged_at: None,
            merged_by: None,
            closed_at: None,
            requested_reviewers: vec![],
            milestone: None,
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
        PromptInput { pull_request: None, issue: None, body: None, anchor, file, outdated: false, question, history: &[] }
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
    fn run_events_fold_into_the_record() {
        use perch_agent::RunEvent;
        use perch_github::{RunRecord, RunStatus};
        let mut run = RunRecord {
            id: "r".into(),
            thread_id: "t".into(),
            backend: "claude-code".into(),
            model: "sonnet".into(),
            effort: "medium".into(),
            session_id: None,
            status: RunStatus::Running,
            cost_usd: None,
            duration_ms: None,
            steps: vec![],
            error: None,
            started_at: jiff::Timestamp::now(),
            ended_at: None,
        };
        apply_run_event(&mut run, &RunEvent::Started { session_id: "s".into(), model: "m".into() });
        assert_eq!(run.session_id.as_deref(), Some("s"));
        apply_run_event(&mut run, &RunEvent::ToolCall { id: "1".into(), name: "Read".into(), summary: "Read a.rs".into() });
        apply_run_event(&mut run, &RunEvent::ToolCall { id: "2".into(), name: "Write".into(), summary: "Asked to edit a.rs".into() });
        apply_run_event(&mut run, &RunEvent::ToolResult { id: "2".into(), ok: false, summary: "not allowed".into() });
        assert_eq!(run.steps[1].summary, "Asked to edit a.rs: not allowed");
        assert!(!run.steps[1].ok);
        assert!(run.steps[0].ok);
        assert_eq!(apply_run_event(&mut run, &RunEvent::Text("hi".into())), Some("hi".into()));
        apply_run_event(&mut run, &RunEvent::Finished { session_id: "s".into(), cost_usd: Some(0.1), duration_ms: 900 });
        assert_eq!(run.status, RunStatus::Finished);
        assert_eq!(run.duration_ms, Some(900));

        let mut stopped = run.clone();
        apply_run_event(&mut stopped, &RunEvent::Failed { message: "Stopped".into() });
        assert_eq!(stopped.status, RunStatus::Stopped);
        let mut failed = run.clone();
        apply_run_event(&mut failed, &RunEvent::Failed { message: "boom".into() });
        assert_eq!(failed.status, RunStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("boom"));
    }

    fn agent_config() -> perch_github::AgentConfig {
        perch_github::AgentConfig {
            binary: "/opt/claude".into(),
            codex: perch_github::CodexConfig { binary: "/opt/codex".into(), models: vec!["gpt-5.6-sol".into()] },
            ..perch_github::AgentConfig::default()
        }
    }

    #[test]
    fn the_backend_follows_the_model_and_the_binary_follows_the_backend() {
        use perch_agent::Backend;
        let agent = agent_config();
        assert_eq!(backend_for(&agent, "sonnet"), Backend::ClaudeCode);
        assert_eq!(backend_for(&agent, "gpt-5.6-sol"), Backend::Codex);
        assert_eq!(backend_for(&agent, "nobody-lists-this"), Backend::ClaudeCode);

        let prompt = first_prompt(&input(None, None, "why?"));
        let claude = run_spec(&agent, Backend::ClaudeCode, "sonnet", perch_agent::Effort::High, "/tmp".into(), &prompt, None);
        assert_eq!(claude.backend, Backend::ClaudeCode);
        assert_eq!(claude.binary, std::path::PathBuf::from("/opt/claude"));
        assert!(claude.args().windows(2).any(|w| w == ["--effort", "high"]));

        let codex = run_spec(&agent, Backend::Codex, "gpt-5.6-sol", perch_agent::Effort::Ultra, "/tmp".into(), &prompt, Some("t-1".into()));
        assert_eq!(codex.backend, Backend::Codex);
        assert_eq!(codex.binary, std::path::PathBuf::from("/opt/codex"));
        assert_eq!(codex.system.as_deref(), Some(FRAMING), "the framing goes as developer instructions");
        let args = codex.args();
        assert_eq!(&args[..4], ["exec", "--json", "resume", "t-1"]);
        assert!(args.windows(2).any(|w| w == ["-c", "model_reasoning_effort=ultra"]));
    }

    #[test]
    fn a_spawn_failure_names_the_key_for_its_backend() {
        assert!(binary_hint(perch_agent::Backend::ClaudeCode).starts_with("Set agent.binary "));
        assert!(binary_hint(perch_agent::Backend::Codex).starts_with("Set agent.codex.binary "));
    }

    #[test]
    fn an_issue_takes_the_pull_request_block_s_place() {
        let issue = perch_github::Issue {
            number: 42,
            title: "Fence the manifest".into(),
            body: "It races.".into(),
            state: "open".into(),
            state_reason: None,
            author_login: "rohan".into(),
            author_avatar_url: String::new(),
            labels: vec![],
            assignees: vec![],
            comments: 1,
            created_at: None,
            updated_at: "2026-09-18T10:00:00Z".parse().unwrap(),
            repo_full_name: "slatedb/slatedb".into(),
        };
        let comments = vec![perch_github::IssueComment {
            id: 1,
            user: "chris".into(),
            user_avatar_url: String::new(),
            body: "Only on compaction.".into(),
            created_at: "2026-09-18T11:00:00Z".parse().unwrap(),
            updated_at: None,
            html_url: String::new(),
        }];
        let p = first_prompt(&PromptInput { issue: Some((&issue, &comments)), ..input(None, None, "where?") });
        assert!(p.user.starts_with("Issue: slatedb/slatedb#42: Fence the manifest\nIt races.\n\nComments:\nchris: Only on compaction.\n"), "{}", p.user);
        assert!(!p.user.contains("Pull request:"));
        assert!(p.user.ends_with("Question: where?"));
    }

    #[test]
    fn a_follow_up_is_only_the_question() {
        let a = anchor(2, 3);
        let p = follow_up_prompt(&input(Some(&a), None, "and then?"));
        assert_eq!(p.system, None);
        assert_eq!(p.user, "(About src/manifest.rs:2–3)\n\nQuestion: and then?");
    }

    #[test]
    fn a_quoted_comment_prefixes_every_line_and_leaves_room_below() {
        let q = quote_for("rohan", "https://github.com/a/b/pull/1#discussion_r9", "Why the fence?\n\nIt was fine before.\n");
        assert_eq!(
            q,
            "> **rohan** [commented](https://github.com/a/b/pull/1#discussion_r9):\n> Why the fence?\n>\n> It was fine before.\n\n"
        );
        assert_eq!(quote_for("a", "u", ""), "> **a** [commented](u):\n\n", "an empty body is only the header");
    }

    #[test]
    fn a_thread_with_nothing_to_resume_folds_its_history_in() {
        let history = vec![(Author::User, "first?".to_string()), (Author::Agent { backend: "c".into(), model: "m".into() }, "partial".to_string())];
        let p = follow_up_prompt(&PromptInput { history: &history, ..input(None, None, "again?") });
        assert!(p.system.is_some());
        assert!(p.user.contains("Earlier in this thread:\n\nReader: first?\n\nYou: partial\n\nQuestion: again?"), "{}", p.user);
    }
}
