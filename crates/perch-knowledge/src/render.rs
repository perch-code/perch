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
pub fn container(name: &str, messages: &[&Message]) -> String {
    let by_id: HashMap<&str, &Message> = messages.iter().map(|m| (m.id.as_str(), *m)).collect();

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
        out.push_str(&format!(
            "## {}: {} {} {} [↗]({})\n\n",
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

/// A project's index: each source, its containers, how many messages and
/// how recent.
pub fn project_index(project: &str, sections: &[(Source, Vec<Message>)]) -> String {
    let mut out = format!("# {project}\n\n");
    let repos = repos_of(sections.iter().flat_map(|(_, m)| m.iter()));
    if !repos.is_empty() {
        out.push_str("Repositories: ");
        out.push_str(&repos.join(", "));
        out.push_str("\n\n");
    }
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
                "| {c} | {} | {} | {newest} |\n",
                file.display(),
                of.len()
            ));
        }
        out.push('\n');
    }
    out.trim_end().to_string() + "\n"
}

/// The root index: every project, its repositories, and when it last moved,
/// so an agent can match a git remote to a project.
pub fn root_index(projects: &[(String, Vec<Message>)]) -> String {
    let mut out = String::from("# Perch knowledge\n\n");
    out.push_str("| project | repositories | messages | updated |\n|---|---|---|---|\n");
    for (name, messages) in projects {
        let repos = repos_of(messages.iter()).join(", ");
        let updated = messages.iter().map(|m| m.at).max().map(stamp).unwrap_or_default();
        out.push_str(&format!("| {name} | {repos} | {} | {updated} |\n", messages.len()));
    }
    out
}

fn repos_of<'a>(messages: impl Iterator<Item = &'a Message>) -> Vec<String> {
    let mut repos: Vec<String> = messages
        .filter(|m| m.source == Source::GitHub)
        .filter_map(|m| m.container.split_once('#').map(|(r, _)| r.to_string()))
        .collect();
    repos.sort();
    repos.dedup();
    repos
}
