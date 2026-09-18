//! Markdown from records. Boring on purpose: a grep hit should carry its
//! own context on the line and in the heading above it.

use std::collections::HashMap;

use jiff::Timestamp;

use crate::{Message, Source};

fn stamp(at: Timestamp) -> String {
    at.strftime("%Y-%m-%d %H:%M").to_string()
}

fn clock(at: Timestamp) -> String {
    at.strftime("%H:%M").to_string()
}

/// Split a body into its first line and the rest, both trimmed.
fn headline(body: &str) -> (&str, &str) {
    let body = body.trim();
    match body.split_once('\n') {
        Some((first, rest)) => (first.trim(), rest.trim()),
        None => (body, ""),
    }
}

/// A body with any fenced code block that was left open closed, so a stray
/// fence cannot swallow the rest of the file.
pub fn close_fences(body: &str) -> String {
    let open = body.lines().filter(|l| l.trim_start().starts_with("```")).count() % 2 == 1;
    if open {
        format!("{}\n```", body.trim_end())
    } else {
        body.to_string()
    }
}

/// One container: the opener, the conversation, then each thread.
///
/// A file of the reader's own threads says so in front matter before
/// anything else, so an agent that reads it knows these were questions
/// asked here, not comments on GitHub.
pub fn container(name: &str, messages: &[&Message]) -> String {
    let by_id: HashMap<&str, &Message> = messages.iter().map(|m| (m.id.as_str(), *m)).collect();
    let local = messages.iter().all(|m| m.source == Source::Perch) && !messages.is_empty();

    // Everything arrives sorted by time; the opener is the first message
    // that belongs to no thread and answers nobody.
    let top: Vec<&Message> = messages.iter().copied().filter(|m| m.thread.is_none()).collect();
    let opener = top.iter().copied().find(|m| m.reply_to.is_none());

    let mut out = String::new();

    // The opener's first line is the title when it is a heading.
    let (title, opener_rest) = match opener {
        Some(m) => {
            let (first, rest) = headline(&m.body);
            match first.strip_prefix("# ") {
                Some(title) => (Some(title.trim()), rest),
                None => (None, m.body.trim()),
            }
        }
        None => (None, ""),
    };
    if local {
        out.push_str(
            "---\nsource: perch\nkind: local threads\nnote: Questions the reader asked an agent about this on their own machine, with the answers. Not on GitHub; nobody else has seen them.\n---\n\n",
        );
    }
    match title {
        Some(title) => out.push_str(&format!("# {name}: {title}\n\n")),
        None => out.push_str(&format!("# {name}\n\n")),
    }

    if let Some(m) = opener {
        out.push_str(&format!("**{}** opened {} [↗]({})\n\n", m.author, stamp(m.at), m.url));
        if !opener_rest.is_empty() {
            out.push_str(&close_fences(opener_rest));
            out.push_str("\n\n");
        }
    }

    let conversation: Vec<&Message> =
        top.iter().copied().filter(|m| Some(m.id.as_str()) != opener.map(|o| o.id.as_str())).collect();
    if !conversation.is_empty() {
        out.push_str("---\n\n");
        for m in conversation {
            push_message(&mut out, m, &by_id);
        }
    }

    // Threads, in the order their heads were written.
    let mut threads: Vec<&str> = Vec::new();
    for m in messages {
        if let Some(t) = m.thread.as_deref() {
            if !threads.contains(&t) {
                threads.push(t);
            }
        }
    }
    for thread in threads {
        let members: Vec<&Message> =
            messages.iter().copied().filter(|m| m.thread.as_deref() == Some(thread)).collect();
        let head = members.iter().copied().find(|m| m.reply_to.is_none()).or(members.first().copied());
        out.push_str("---\n\n");
        let Some(h) = head else { continue };
        let (first, rest) = headline(&h.body);
        // A question asked about lines names them in its heading, where a
        // grep for the path finds it.
        let location = h.location.as_deref().map(|l| format!(" on `{l}`")).unwrap_or_default();
        out.push_str(&format!(
            "## {}: {} {} {}{location} [↗]({})\n\n",
            h.source.thread_noun(),
            h.author,
            first,
            stamp(h.at),
            h.url
        ));
        if !rest.is_empty() {
            out.push_str(&close_fences(rest));
            out.push_str("\n\n");
        }
        for m in members.iter().copied().filter(|m| m.id != h.id) {
            push_message(&mut out, m, &by_id);
        }
    }

    out.trim_end().to_string() + "\n"
}

impl Source {
    fn thread_noun(self) -> &'static str {
        match self {
            Source::GitHub => "Review",
            Source::Discord => "Thread",
            Source::Perch => "Asked",
        }
    }
}

fn push_message(out: &mut String, m: &Message, by_id: &HashMap<&str, &Message>) {
    out.push_str(&format!("**{}** {} [↗]({})", m.author, stamp(m.at), m.url));
    if let Some(loc) = &m.location {
        out.push_str(&format!(" on `{loc}`"));
    }
    if let Some(parent) = m.reply_to.as_deref().and_then(|id| by_id.get(id)) {
        out.push_str(&format!(" replying to {} {}", parent.author, clock(parent.at)));
    }
    if m.edited_at.is_some() {
        out.push_str(" (edited)");
    }
    out.push_str("\n\n");
    let body = m.body.trim();
    if !body.is_empty() {
        out.push_str(&close_fences(body));
        out.push_str("\n\n");
    }
}

/// A repository's index: each source, its containers, how many messages
/// and how recent.
pub fn repo_index(repo: &str, sections: &[(Source, Vec<Message>)]) -> String {
    let mut out = format!("# {repo}\n\n");
    for (source, messages) in sections {
        out.push_str(&format!("## {}\n\n", source.dir_name()));
        let mut containers: Vec<&str> = messages.iter().map(|m| m.container.as_str()).collect();
        containers.sort();
        containers.dedup();
        out.push_str("| container | file | messages | newest |\n|---|---|---|---|\n");
        for c in containers {
            let of: Vec<&Message> = messages.iter().filter(|m| m.container == c).collect();
            let newest = of.iter().map(|m| m.at).max().map(stamp).unwrap_or_default();
            let file = crate::Knowledge::rendered_relative(*source, c);
            out.push_str(&format!(
                "| {} | {} | {} | {newest} |\n",
                crate::Knowledge::container_label(c),
                file.display(),
                of.len()
            ));
        }
        out.push('\n');
    }
    out.trim_end().to_string() + "\n"
}

/// The root index: every repository, the directory it is under, and when
/// it last moved, so an agent can match `git remote get-url origin` to a
/// directory.
pub fn root_index(repos: &[(String, Vec<Message>)]) -> String {
    let mut out = String::from("# Perch knowledge\n\n");
    out.push_str("| repository | directory | messages | updated |\n|---|---|---|---|\n");
    for (repo, messages) in repos {
        let updated = messages.iter().map(|m| m.at).max().map(stamp).unwrap_or_default();
        out.push_str(&format!("| {repo} | {repo}/ | {} | {updated} |\n", messages.len()));
    }
    out
}
