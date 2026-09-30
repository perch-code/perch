//! `perch --timeline <item-id>`
//!
//! Prints an item's Conversation as text: one line per card with its kind,
//! time and first line, and a thread's anchor and the messages after its
//! first indented under it. From the cache the app uses, so the interleave
//! can be read without a window. An issue's comments are fetched on open and
//! not kept, so an issue prints without them and says so. A bare branch's
//! commits are read from git on open, not cached, and are left out too.

use perch_github::Cache;

use crate::review::timeline::{first_line, timeline, Card};

pub fn run(args: &[String]) {
    let [id] = args else {
        eprintln!("usage: perch --timeline <item-id>");
        std::process::exit(2);
    };
    match print(id) {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("no item {id}; `perch --items` lists them");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

/// Whether the item was found.
fn print(id: &str) -> Result<bool, String> {
    let path = perch_github::config::db_path().ok_or("no data dir")?;
    if !path.exists() {
        return Err(format!("no cache at {}; run the app once", path.display()));
    }
    let (cache, _) = Cache::open_with_outcome(&path, None).map_err(|e| e.to_string())?;
    let Some(item) = cache.load_item(id).map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    let discussion = cache.load_discussion(id).map_err(|e| e.to_string())?.map(|(_, d)| d);
    let threads = cache.load_comment_threads(id).map_err(|e| e.to_string())?;

    // A note on what the list below is missing, on stderr so stdout stays
    // the cards alone. An issue's comments are kept in the same row a pull
    // request's discussion is.
    if discussion.is_none() && item.subject.is_some() {
        eprintln!("note: nothing is cached for this item yet; open it in the app once");
    }
    let (comments, events): (Vec<perch_github::IssueComment>, Vec<perch_github::PullEvent>) =
        match (&item.issue(), &discussion) {
            (Some(_), Some(d)) => (d.comments.clone(), d.events.clone()),
            _ => Default::default(),
        };
    let issue = item
        .issue()
        .map(|i| (i, comments.as_slice(), events.as_slice()));
    let pull = item.pull_request();
    let cards = timeline(pull, if pull.is_some() { discussion.as_ref() } else { None }, issue, &[], &threads);
    let mut out = String::new();
    render(&mut out, &cards);
    print!("{out}");
    Ok(true)
}

/// `<kind>  <time or -->  <first line>`, with a thread's anchor as
/// `  <path:line>` and each message after its first as
/// `    reply <first line>`.
fn render(out: &mut String, cards: &[Card]) {
    for card in cards {
        let at = card.at().map(|t| t.to_string()).unwrap_or_else(|| "--".into());
        out.push_str(&format!("{:<11}  {at}  {}\n", card.kind(), card.first_line()));
        if let Card::Thread(thread) = card {
            if let Some(anchor) = &thread.anchor {
                out.push_str(&format!("  {}\n", anchor.label()));
            }
            for reply in thread.messages.iter().skip(1) {
                out.push_str(&format!("    reply {}\n", first_line(&reply.body)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::timeline::tests::{at, event, review};
    use perch_github::{Anchor, Author, CommentMessage, CommentThread, PullEventKind, Side, ThreadOrigin};

    fn message(id: u64, when: &str, body: &str) -> CommentMessage {
        CommentMessage {
            id: format!("gh-{id}"),
            thread_id: "gh-31".into(),
            origin: ThreadOrigin::GitHub,
            author: Author::GitHubUser("rohan".into()),
            body: body.into(),
            created_at: at(when),
            run: None,
            github_id: Some(id),
            node_id: None,
            html_url: None,
            diff_hunk: None,
            reactions: Default::default(),
        }
    }

    #[test]
    fn a_thread_prints_its_anchor_and_the_replies_under_it() {
        let cards = vec![
            Card::Event(event(PullEventKind::Opened, "2026-09-14T14:02:00Z")),
            Card::Review { review: review(9, "APPROVED", "Looks right.\n\nMore below.", Some("2026-09-16T11:40:00Z")) },
            Card::Thread(CommentThread {
                id: "gh-31".into(),
                worktree_id: "w".into(),
                origin: ThreadOrigin::GitHub,
                github_id: Some(31),
                anchor: Some(Anchor { path: "src/manifest.rs".into(), start_line: 41, line: 41, side: Side::Right, commit_sha: "h".into() }),
                outdated: false,
                resolved: false,
                node_id: None,
                can_resolve: false,
                created_at: at("2026-09-16T11:38:00Z"),
                messages: vec![
                    message(31, "2026-09-16T11:38:00Z", "epoch\nsecond line"),
                    message(32, "2026-09-16T12:00:00Z", "  the reply "),
                ],
            }),
            Card::Description { at: None, body: String::new(), node_id: String::new() },
        ];
        let mut out = String::new();
        render(&mut out, &cards);
        let want = [
            "event        2026-09-14T14:02:00Z  opened",
            "review       2026-09-16T11:40:00Z  approved  Looks right.",
            "thread       2026-09-16T11:38:00Z  epoch",
            "  src/manifest.rs:41",
            "    reply the reply",
            "description  --  No description.",
            "",
        ]
        .join("\n");
        assert_eq!(out, want);
    }
}
