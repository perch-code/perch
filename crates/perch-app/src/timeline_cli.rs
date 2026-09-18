//! `perch --timeline <item-id>`
//!
//! Prints an item's Conversation as text: one line per card with its kind,
//! time and first line, and a review's line comments and their replies
//! indented under it. From the cache the app uses, so the interleave can be
//! read without a window. An issue's comments are fetched on open and not
//! kept, so an issue prints without them and says so.

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
    // The same open the app does, reset included, as `--items` does.
    let (cache, reset) = Cache::open_checked(&path, None).map_err(|e| e.to_string())?;
    if reset {
        eprintln!("note: the database was older than this build and has been recreated");
    }
    let Some(item) = cache.load_item(id).map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    let discussion = cache.load_discussion(id).map_err(|e| e.to_string())?.map(|(_, d)| d);
    let threads = cache.load_comment_threads(id).map_err(|e| e.to_string())?;

    // Notes on what the list below is missing, on stderr so stdout stays
    // the cards alone.
    if item.pull_request().is_some() && discussion.is_none() {
        eprintln!("note: no discussion is cached for this item; open it in the app once");
    }
    let issue = item.issue().map(|i| (i, &[][..]));
    if issue.is_some() {
        eprintln!("note: an issue's comments are not cached; this is the description and the threads");
    }

    let cards = timeline(item.pull_request(), discussion.as_ref(), issue, &threads);
    let mut out = String::new();
    render(&mut out, &cards);
    print!("{out}");
    Ok(true)
}

/// `<kind>  <time or -->  <first line>`, with a review's line comments as
/// `  <path:line> <first line>` and each reply as `    reply <first line>`.
fn render(out: &mut String, cards: &[Card]) {
    for card in cards {
        let at = card.at().map(|t| t.to_string()).unwrap_or_else(|| "--".into());
        out.push_str(&format!("{:<11}  {at}  {}\n", card.kind(), card.first_line()));
        if let Card::Review { comments, .. } = card {
            for thread in comments {
                out.push_str(&format!("  {} {}\n", thread.comment.location(), first_line(&thread.comment.body)));
                for reply in &thread.replies {
                    out.push_str(&format!("    reply {}\n", first_line(&reply.body)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::timeline::tests::{event, line, review};
    use crate::review::timeline::LineThread;
    use perch_github::PullEventKind;

    #[test]
    fn a_review_prints_its_line_comments_and_their_replies_under_it() {
        let cards = vec![
            Card::Event(event(PullEventKind::Opened, "2026-09-14T14:02:00Z")),
            Card::Review {
                review: Some(review(9, "APPROVED", "Looks right.\n\nMore below.", Some("2026-09-16T11:40:00Z"))),
                comments: vec![LineThread {
                    comment: line(31, Some(9), None, "2026-09-16T11:38:00Z", "epoch\nsecond line"),
                    replies: vec![line(32, Some(10), Some(31), "2026-09-16T12:00:00Z", "  the reply ")],
                }],
            },
            Card::Description { author: "rohan".into(), avatar_url: String::new(), at: None, body: String::new() },
        ];
        let mut out = String::new();
        render(&mut out, &cards);
        let want = [
            "event        2026-09-14T14:02:00Z  opened",
            "review       2026-09-16T11:40:00Z  approved  Looks right.",
            "  src/manifest.rs:41 epoch",
            "    reply the reply",
            "description  --  No description.",
            "",
        ]
        .join("\n");
        assert_eq!(out, want);
    }
}
