//! `perch --items ["worktrees=recent;query=fence"]`
//!
//! Prints Worktrees' list from the cache the app uses, with pinned items
//! first: the worktrees on disk, or with `worktrees=recent` everything
//! opened. The keys are the filter menu's, in `ItemFilters::to_meta`'s
//! form: `worktrees` (`worktrees` or `recent`), `repository`,
//! `worktrees_state` (`any`, `open`, `closed`), `worktrees_unread`, `sort`
//! and `direction` for Worktrees, `recent_sort` and `recent_direction` for
//! Recent, and `show_pr_status`, plus `query` for the search box. It is
//! how `items::local_items` is checked against real data without a
//! window.

use perch_github::{Cache, CheckStatus, Item, ItemSubject};

use crate::items::{self, ItemFilters, RowKind};

pub fn run(args: &[String]) {
    let filters = match args {
        [] => worktrees(""),
        [spec] => match ItemFilters::parse(&worktrees_unless_said(spec)) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("bad filters: {e}");
                eprintln!(
                    "keys: worktrees (worktrees, recent), repository, worktrees_state (any, open, closed), \
                     worktrees_unread, sort, direction (newest, oldest), recent_sort, recent_direction, \
                     show_pr_status, query"
                );
                std::process::exit(2);
            }
        },
        _ => {
            eprintln!("usage: perch --items [\"key=value;...\"]");
            std::process::exit(2);
        }
    };
    if let Err(e) = print(&filters) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// The filters, on Worktrees.
fn worktrees(spec: &str) -> ItemFilters {
    ItemFilters::parse(&worktrees_unless_said(spec)).unwrap_or_default()
}

/// `spec` with `tab=worktrees` in front when it names no tab: the
/// window opens on Inbox, which lists no items.
fn worktrees_unless_said(spec: &str) -> String {
    let said = spec
        .split(';')
        .filter_map(|pair| pair.split_once('='))
        .any(|(key, _)| matches!(key.trim(), "tab" | "scope"));
    if said {
        spec.to_string()
    } else {
        format!("tab=worktrees;{spec}")
    }
}

fn print(filters: &ItemFilters) -> Result<(), String> {
    let path = perch_github::config::db_path().ok_or("no data dir")?;
    if !path.exists() {
        return Err(format!("no cache at {}; run the app once", path.display()));
    }
    let (cache, _) = Cache::open_with_outcome(&path, None).map_err(|e| e.to_string())?;
    let items = cache.load_items().map_err(|e| e.to_string())?;
    // What Unread Only goes by: an unread notification about the item, as
    // the window's dot has it.
    let threads = cache.load_threads().map_err(|e| e.to_string())?;
    let unread = |item: &Item| {
        item.subject.as_ref().is_some_and(|subject| {
            threads.iter().any(|thread| {
                thread.unread
                    && thread.is_about(subject.repo_full_name(), subject.kind(), subject.number())
            })
        })
    };
    let visible = items::local_items(&items, filters, &unread);

    // What the list below was made under, on stderr so stdout stays the
    // sidebar alone.
    let query = filters.query.trim();
    if query.is_empty() {
        eprintln!("filters: {}", filters.to_meta());
    } else {
        eprintln!("filters: {};query={query}", filters.to_meta());
    }

    let mut out = String::new();
    render(&mut out, &visible, filters);
    print!("{out}");
    Ok(())
}

/// Rows have no section headings; each reference names its repository.
fn render(out: &mut String, visible: &[Item], filters: &ItemFilters) {
    for item in visible {
        row(out, item, &items::full_reference(item), filters);
    }
}

/// One row: the glyph letter, the title, the reference line, and whatever
/// the window would show at the right edge, in the order it shows them.
fn row(out: &mut String, item: &Item, reference: &str, filters: &ItemFilters) {
    let mut line = format!(
        "  {}  {}  [{reference}]",
        RowKind::of(item).letter(),
        item.title()
    );
    if filters.show_pr_status {
        if let Some(chips) = pr_status(item) {
            line.push_str("  ");
            line.push_str(&chips);
        }
    }
    if item.is_pinned() {
        line.push_str("  pinned");
    }
    if item.is_archived() {
        line.push_str("  archived");
    }
    line.push('\n');
    out.push_str(&line);
}

/// The CI and review chips, as words.
fn pr_status(item: &Item) -> Option<String> {
    let ItemSubject::PullRequest(pr) = item.subject.as_ref()? else {
        return None;
    };
    let mut chips = Vec::new();
    if let Some(checks) = pr.checks {
        chips.push(match checks {
            CheckStatus::Pending => "ci:pending".to_string(),
            CheckStatus::Success => "ci:success".to_string(),
            CheckStatus::Failure => "ci:failure".to_string(),
        });
    }
    if let Some(decision) = pr.review_decision {
        chips.push(decision.label().to_string());
    }
    if chips.is_empty() {
        None
    } else {
        Some(chips.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::Timestamp;
    use perch_github::{ItemCheckout, PullRequest, ReviewDecision};

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    /// An open pull request and a pinned branch.
    fn fixture() -> Vec<Item> {
        let mut wanted = Item::new("criccomini/perch", at("2026-09-10T10:00:00Z"));
        wanted.id = "a".into();
        wanted.subject = Some(ItemSubject::PullRequest(PullRequest {
            number: 57,
            html_url: "https://github.com/criccomini/perch/pull/57".into(),
            title: "Fence the manifest".into(),
            author_login: "a".into(),
            author_avatar_url: String::new(),
            head_sha: "h".into(),
            head_ref: "fence".into(),
            base_sha: "b".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            repo_full_name: "criccomini/perch".into(),
            head_repo_full_name: None,
            updated_at: Some(at("2026-09-17T10:00:00Z")),
            review_requested: true,
            checks: Some(CheckStatus::Failure),
            review_decision: Some(ReviewDecision::ReviewRequested),
            merged_at: None,
            merged_by: None,
            closed_at: None,
            requested_reviewers: vec![],
            milestone: None,
            labels: vec![],
            assignees: vec![],
        }));
        let mut branch = Item::new("criccomini/perch", at("2026-09-14T10:00:00Z"));
        branch.id = "b".into();
        branch.checkout = Some(ItemCheckout {
            branch: "perch/issue-42".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: "/w/issue-42".into(),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
            divergence: None,
            source: None,
        });
        branch.pinned_at = Some(at("2026-09-18T10:00:00Z"));

        vec![wanted, branch]
    }

    fn text(filters: &ItemFilters) -> String {
        let visible = items::local_items(&fixture(), filters, &|_| false);
        let mut out = String::new();
        render(&mut out, &visible, filters);
        out
    }

    #[test]
    fn worktrees_print_the_checkouts_and_recent_everything_opened() {
        assert_eq!(
            text(&worktrees("")),
            "  B  perch/issue-42  [criccomini/perch · perch/issue-42]  pinned\n"
        );
        assert_eq!(
            text(&worktrees("worktrees=recent")),
            "  B  perch/issue-42  [criccomini/perch · perch/issue-42]  pinned\n  P  Fence the manifest  [criccomini/perch #57 · fence]  ci:failure review requested\n"
        );
        assert_eq!(
            text(&worktrees("tab=browse;browse=recent")),
            text(&worktrees("worktrees=recent")),
            "as saved while Browse showed Recent"
        );
        assert_eq!(
            text(&worktrees("tab=inbox")),
            "",
            "the inbox lists no items"
        );
    }

    #[test]
    fn chips_go_when_pr_status_is_off_and_the_row_still_names_its_repository() {
        let got = text(&worktrees("worktrees=recent;show_pr_status=0;query=fence"));
        assert_eq!(
            got,
            "  P  Fence the manifest  [criccomini/perch #57 · fence]\n"
        );
    }

    #[test]
    fn repository_filter_limits_the_printed_list() {
        assert_eq!(
            text(&worktrees("repository=criccomini/perch")),
            text(&worktrees(""))
        );
        assert_eq!(text(&worktrees("repository=slatedb/slatedb")), "");
    }

    #[test]
    fn a_search_that_misses_prints_nothing() {
        assert_eq!(text(&worktrees("query=nothing")), "");
    }
}
