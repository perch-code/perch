//! `perch --items ["status=closed;group=date;query=fence"]`
//!
//! Prints the sidebar as text: Priority, Pinned, then each section and its
//! rows, from the cache the app uses. It is how `items::sections` is checked
//! against real data without a window.

use jiff::Timestamp;
use perch_github::{Cache, CheckStatus, Item, ItemSubject, Project};

use crate::items::{self, ItemFilters, RowKind, Sections};

pub fn run(args: &[String]) {
    let filters = match args {
        [] => ItemFilters::default(),
        [spec] => match ItemFilters::parse(spec) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("bad filters: {e}");
                eprintln!("keys: status, kinds, group, sort, show_empty, show_pr_status, query");
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
    let projects = cache.load_projects().map_err(|e| e.to_string())?;
    let items = cache.load_items().map_err(|e| e.to_string())?;
    let sections = items::sections(&projects, &items, filters, Timestamp::now());

    // What the list below was made under, on stderr so stdout stays the
    // sidebar alone.
    let query = filters.query.trim();
    if query.is_empty() {
        eprintln!("filters: {}", filters.to_meta());
    } else {
        eprintln!("filters: {};query={query}", filters.to_meta());
    }

    let mut out = String::new();
    render(&mut out, &projects, &sections, filters);
    print!("{out}");
    Ok(())
}

fn render(out: &mut String, projects: &[Project], sections: &Sections, filters: &ItemFilters) {
    let project_of = |item: &Item| projects.iter().find(|p| p.id == item.project_id);

    out.push_str("Priority\n");
    if sections.priority.is_empty() {
        out.push_str("  Nothing needs attention\n");
    }
    for item in &sections.priority {
        row(out, item, project_of(item), filters, false);
    }

    if !sections.pinned.is_empty() {
        out.push_str("\nPinned\n");
        for item in &sections.pinned {
            row(out, item, project_of(item), filters, true);
        }
    }

    for section in &sections.groups {
        out.push_str(&format!("\n{} ({})\n", section.title, section.count));
        for item in &section.items {
            row(out, item, project_of(item), filters, false);
        }
    }
}

/// One row: the glyph letter, the title, the reference line, and whatever
/// the window would show at the right edge, in the order it shows them.
fn row(out: &mut String, item: &Item, project: Option<&Project>, filters: &ItemFilters, pinned: bool) {
    let mut line = format!(
        "  {}  {}  [{}]",
        RowKind::of(item).letter(),
        item.title(),
        items::reference(item, project, filters.group),
    );
    if filters.show_pr_status {
        if let Some(chips) = pr_status(item) {
            line.push_str("  ");
            line.push_str(&chips);
        }
    }
    if pinned {
        line.push_str("  pinned");
    }
    if item.is_archived() {
        line.push_str("  archived");
    }
    if item.needs_attention() {
        let reason = item.attention_reason.as_deref().map(items::attention_label).unwrap_or_default();
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
    use crate::items::ItemGroup;
    use perch_github::{ItemCheckout, PullRequest, ReviewDecision};

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    /// One project with a pull request that wants a review and a pinned
    /// branch.
    fn fixture() -> (Vec<Project>, Vec<Item>) {
        let perch = Project {
            id: "perch".into(),
            name: "perch".into(),
            repos: vec!["criccomini/perch".into()],
            roots: vec![],
            implicit: false,
        };

        let mut wanted = Item::new("perch", "criccomini/perch", at("2026-09-10T10:00:00Z"));
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

        let mut branch = Item::new("perch", "criccomini/perch", at("2026-09-14T10:00:00Z"));
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

        (vec![perch], vec![wanted, branch])
    }

    fn text(filters: &ItemFilters) -> String {
        let (projects, items) = fixture();
        // `now` is the attention's own instant, so the date bucket is Today
        // in any time zone.
        let sections = items::sections(&projects, &items, filters, at("2026-09-18T09:00:00Z"));
        let mut out = String::new();
        render(&mut out, &projects, &sections, filters);
        out
    }

    #[test]
    fn the_sidebar_prints_as_sections_of_rows() {
        let got = text(&ItemFilters::default());
        let want = [
            "Priority",
            "  P  Fence the manifest  [criccomini/perch #57 · fence]  ci:failure review requested  * review requested",
            "",
            "Pinned",
            "  B  perch/issue-42  [criccomini/perch · perch/issue-42]  pinned",
            "",
            "perch (2)",
            "  P  Fence the manifest  [criccomini/perch #57 · fence]  ci:failure review requested  * review requested",
            "",
        ]
        .join("\n");
        assert_eq!(got, want);
    }

    #[test]
    fn chips_go_when_pr_status_is_off_and_a_date_row_names_its_project() {
        let f = ItemFilters {
            show_pr_status: false,
            group: ItemGroup::Date,
            query: "fence".into(),
            ..Default::default()
        };
        let got = text(&f);
        assert!(got.contains("  P  Fence the manifest  [perch #57 · fence]  * review requested\n"), "{got}");
        assert!(got.contains("\nToday (1)\n"), "{got}");
        assert!(!got.contains("Pinned"), "{got}");
    }

    #[test]
    fn an_empty_priority_says_so() {
        let f = ItemFilters { query: "issue-42".into(), ..Default::default() };
        let got = text(&f);
        assert!(got.starts_with("Priority\n  Nothing needs attention\n"), "{got}");
    }
}
