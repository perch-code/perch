//! `perch --items ["status=done;reason=mention;direction=oldest;query=fence"]`
//!
//! Prints the sidebar's flat list from the cache the app uses, with pinned
//! items first. The keys are the filter menu's, in `ItemFilters::to_meta`'s
//! form: `scope` (`all`, `notifications` or `authored`), `repository`,
//! `status`, `kinds`, `sort`, `show_pr_status`, `saved`, `reason` and
//! `direction`, plus `query` for the search box. It is how
//! `items::visible_items` is checked against real data without a window.

use perch_github::{Cache, CheckStatus, Item, ItemSubject};

use crate::items::{self, ItemFilters, RowKind};

pub fn run(args: &[String]) {
    let filters = match args {
        [] => ItemFilters::default(),
        [spec] => match ItemFilters::parse(spec) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("bad filters: {e}");
                eprintln!(
                    "keys: scope (all, notifications, authored), repository, \
                     status (active, closed, done, archived, all), kinds, sort, show_pr_status, saved, \
                     reason (any, review_requested, mention, assign, comment, ci_failed, manual, author), \
                     direction (newest, oldest), query"
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

fn print(filters: &ItemFilters) -> Result<(), String> {
    let path = perch_github::config::db_path().ok_or("no data dir")?;
    if !path.exists() {
        return Err(format!("no cache at {}; run the app once", path.display()));
    }
    // The same open the app does, reset included: a stale database is not
    // something this harness can read either way.
    let (cache, reset) = Cache::open_checked(&path, None).map_err(|e| e.to_string())?;
    if reset {
        eprintln!("note: the database was older than this build and has been recreated");
    }
    let items = cache.load_items().map_err(|e| e.to_string())?;
    let viewer = cache.meta("viewer.login").map_err(|e| e.to_string())?;
    let visible = items::visible_items(&items, filters, viewer.as_deref());

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
    if item.needs_attention() {
        let reason = item
            .attention_reason
            .as_deref()
            .map(items::attention_label)
            .unwrap_or_default();
        line.push_str("  *");
        if !reason.is_empty() {
            line.push(' ');
            line.push_str(&reason);
        }
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

    /// A pull request that wants a review and a pinned branch.
    fn fixture() -> Vec<Item> {
        let mut wanted = Item::new("criccomini/perch", at("2026-09-10T10:00:00Z"));
        wanted.id = "a".into();
        wanted.subject = Some(ItemSubject::PullRequest(PullRequest {
            number: 57,
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
        wanted.attention_at = Some(at("2026-09-18T09:00:00Z"));
        wanted.attention_reason = Some("review_requested".into());

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
        });
        branch.pinned_at = Some(at("2026-09-18T10:00:00Z"));

        vec![wanted, branch]
    }

    fn text(filters: &ItemFilters) -> String {
        let visible = items::visible_items(&fixture(), filters, Some("a"));
        let mut out = String::new();
        render(&mut out, &visible, filters);
        out
    }

    #[test]
    fn the_sidebar_prints_flat_rows_with_pins_first() {
        let got = text(&ItemFilters::default());
        let want = [
            "  B  perch/issue-42  [criccomini/perch · perch/issue-42]  pinned",
            "  P  Fence the manifest  [criccomini/perch #57 · fence]  ci:failure review requested  * review requested",
            "",
        ]
        .join("\n");
        assert_eq!(got, want);
    }

    #[test]
    fn chips_go_when_pr_status_is_off_and_the_row_still_names_its_repository() {
        let f = ItemFilters {
            show_pr_status: false,
            query: "fence".into(),
            ..Default::default()
        };
        let got = text(&f);
        assert_eq!(
            got,
            "  P  Fence the manifest  [criccomini/perch #57 · fence]  * review requested\n"
        );
    }

    #[test]
    fn repository_filter_limits_the_printed_list() {
        let got = text(&ItemFilters::parse("repository=criccomini/perch").unwrap());
        assert_eq!(got, text(&ItemFilters::default()));
        let got = text(&ItemFilters::parse("repository=slatedb/slatedb").unwrap());
        assert_eq!(got, "");
    }

    #[test]
    fn a_pinned_row_that_needs_attention_says_both() {
        let mut items = fixture();
        items[0].pinned_at = Some(at("2026-09-18T10:00:00Z"));
        let visible = items::visible_items(&items, &ItemFilters::default(), Some("a"));
        let mut out = String::new();
        render(&mut out, &visible, &ItemFilters::default());
        assert!(out.contains("  pinned  * review requested\n"), "{out}");
    }

    #[test]
    fn searching_for_a_pin_prints_only_its_row() {
        let f = ItemFilters {
            query: "issue-42".into(),
            ..Default::default()
        };
        let got = text(&f);
        assert_eq!(
            got,
            "  B  perch/issue-42  [criccomini/perch · perch/issue-42]  pinned\n"
        );
    }

    #[test]
    fn the_new_keys_narrow_the_printed_list_the_way_the_window_does() {
        // The wanted pull request is a review request and the pinned branch
        // has no reason at all; a mention filter leaves nothing anywhere.
        let got = text(&ItemFilters::parse("reason=mention").unwrap());
        assert_eq!(got, "");

        // Notifications cuts the pinned branch, which needs no attention.
        let got = text(
            &ItemFilters::parse("scope=notifications;reason=review_requested;direction=oldest")
                .unwrap(),
        );
        assert_eq!(
            got,
            "  P  Fence the manifest  [criccomini/perch #57 · fence]  ci:failure review requested  * review requested\n"
        );
        assert_eq!(
            got,
            text(
                &ItemFilters::parse("attention=1;reason=review_requested;direction=oldest")
                    .unwrap()
            )
        );

        // Nothing is saved.
        let got = text(&ItemFilters::parse("saved=1").unwrap());
        assert_eq!(got, "");
    }

    #[test]
    fn authored_scope_uses_the_viewer_and_excludes_local_branches() {
        let filters = ItemFilters::parse("scope=authored").unwrap();
        assert_eq!(
            text(&filters),
            "  P  Fence the manifest  [criccomini/perch #57 · fence]  ci:failure review requested  * review requested\n"
        );

        let visible = items::visible_items(&fixture(), &filters, None);
        let mut out = String::new();
        render(&mut out, &visible, &filters);
        assert_eq!(out, "");
    }
}
