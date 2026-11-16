//! What waits in the composer of a branch Start work cut: a prompt to
//! work on the issue it came from or to continue the pull request. The
//! cutting itself, and the dialog that asks, are perch-app's
//! `review::start_work`.

use perch_github::{Issue, Item, ItemSubject, PullRequest};

/// What waits in the composer of a branch cut from `source`: a prompt to
/// work on its issue or to continue its pull request. None from a worktree.
/// `instructions`, what the reader wrote in an issue's composer set to
/// Work, take the place of the issue prompt's own last line.
pub fn draft(source: &Item, instructions: Option<&str>) -> Option<String> {
    match source.subject.as_ref()? {
        ItemSubject::Issue(issue) => Some(match instructions {
            Some(instructions) if !instructions.trim().is_empty() => {
                format!("{}{}", issue_head(issue), instructions.trim())
            }
            _ => work_prompt(issue),
        }),
        ItemSubject::PullRequest(pr) => Some(pull_prompt(pr)),
    }
}

/// What to ask an agent on a branch cut for `issue`: the issue by number,
/// title and link, its body quoted, and what to do with it.
pub fn work_prompt(issue: &Issue) -> String {
    let mut prompt = issue_head(issue);
    prompt.push_str(
        "Read the issue and its comments, make the change on this branch, and add tests where they fit.",
    );
    prompt
}

/// The issue by number, title and link, and its body quoted: what every
/// prompt to work on it opens with.
fn issue_head(issue: &Issue) -> String {
    let mut prompt = format!(
        "Work on #{}: {}\nhttps://github.com/{}/issues/{}\n\n",
        issue.number, issue.title, issue.repo_full_name, issue.number
    );
    let body = perch_knowledge::sanitize::clean(&issue.body);
    if !body.trim().is_empty() {
        prompt.push_str(&crate::ask::quote_reply(body.trim()));
    }
    prompt
}

/// What to ask an agent on a branch cut from `pr`: the pull request by
/// number, title and link, and that its commits are already here.
pub fn pull_prompt(pr: &PullRequest) -> String {
    format!(
        "Continue #{}: {}\n{}\n\n\
         This branch has the pull request's commits. Read its description and review comments, \
         finish the change on this branch, and add tests where they fit.",
        pr.number, pr.title, pr.html_url
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(body: &str) -> Issue {
        serde_json::from_value(serde_json::json!({
            "number": 7,
            "title": "Fence the manifest",
            "body": body,
            "state": "open",
            "updated_at": "2026-09-18T09:59:00Z",
            "repo_full_name": "slatedb/slatedb",
        }))
        .unwrap()
    }

    fn pull_request(number: u64) -> perch_github::PullRequest {
        perch_github::PullRequest {
            number,
            title: format!("pull {number}"),
            html_url: format!("https://github.com/slatedb/slatedb/pull/{number}"),
            author_login: "u".into(),
            author_avatar_url: String::new(),
            head_sha: "h".into(),
            head_ref: "feature".into(),
            base_sha: "base".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            repo_full_name: "slatedb/slatedb".into(),
            head_repo_full_name: Some("slatedb/slatedb".into()),
            updated_at: None,
            review_requested: false,
            checks: None,
            review_decision: None,
            merged_at: None,
            merged_by: None,
            closed_at: None,
            requested_reviewers: Vec::new(),
            milestone: None,
            labels: Vec::new(),
            assignees: Vec::new(),
        }
    }

    #[test]
    fn the_work_prompt_names_the_issue_and_quotes_its_body() {
        assert_eq!(
            work_prompt(&issue("Two writers race.\n\nFence them.")),
            "Work on #7: Fence the manifest\n\
             https://github.com/slatedb/slatedb/issues/7\n\n\
             > Two writers race.\n>\n> Fence them.\n\n\
             Read the issue and its comments, make the change on this branch, and add tests where they fit."
        );
        assert_eq!(
            work_prompt(&issue("")),
            "Work on #7: Fence the manifest\n\
             https://github.com/slatedb/slatedb/issues/7\n\n\
             Read the issue and its comments, make the change on this branch, and add tests where they fit."
        );
    }

    #[test]
    fn the_pull_prompt_names_the_pull_request_and_says_its_commits_are_here() {
        let pr = pull_request(812);
        assert_eq!(
            pull_prompt(&pr),
            "Continue #812: pull 812\n\
             https://github.com/slatedb/slatedb/pull/812\n\n\
             This branch has the pull request's commits. Read its description and review comments, \
             finish the change on this branch, and add tests where they fit."
        );
    }

    /// The prompt comes from the row Start work cut from, not from what
    /// the main pane has open, so it is right from any row's menu.
    #[test]
    fn the_draft_is_the_sources_prompt() {
        let now = "2026-09-20T12:00:00Z".parse().unwrap();
        let mut item = Item::new("slatedb/slatedb", now);
        assert_eq!(
            draft(&item, None),
            None,
            "a worktree has nothing to ask about"
        );
        item.subject = Some(ItemSubject::Issue(issue("Fence them.")));
        assert_eq!(draft(&item, None), Some(work_prompt(&issue("Fence them."))));
        let pr = pull_request(812);
        item.subject = Some(ItemSubject::PullRequest(pr.clone()));
        assert_eq!(draft(&item, None), Some(pull_prompt(&pr)));
    }

    /// Work picked in an issue's composer: what the reader wrote is the
    /// ask, after the issue itself.
    #[test]
    fn the_readers_words_replace_the_issue_prompts_ask() {
        let now = "2026-09-20T12:00:00Z".parse().unwrap();
        let mut item = Item::new("slatedb/slatedb", now);
        item.subject = Some(ItemSubject::Issue(issue("Fence them.")));
        assert_eq!(
            draft(&item, Some("  Use a CAS on the manifest.\n")).as_deref(),
            Some(
                "Work on #7: Fence the manifest\n\
                 https://github.com/slatedb/slatedb/issues/7\n\n\
                 > Fence them.\n\n\
                 Use a CAS on the manifest."
            )
        );
        assert_eq!(
            draft(&item, Some(" ")),
            Some(work_prompt(&issue("Fence them."))),
            "nothing written is the usual prompt"
        );
    }
}
