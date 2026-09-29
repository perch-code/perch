//! Start work: cut a worktree of the reader's own from the item, and open
//! it. An issue's branch starts from the default branch; a pull request's
//! from its head, with the whole change to show against its base; a
//! worktree's from its head. A pull request opened from the new branch
//! closes the issue or the pull request it came from.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalWith};
use floem::views::{dyn_container, Decorators, Empty};

use perch_github::{Issue, Item, ItemSubject, PullRequest};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::cards::tip;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::icon_button;

/// Whether there is anything to cut from: an issue or a pull request on
/// GitHub that isn't merged, or a worktree with a checkout.
pub fn can_start_work(item: &Item) -> bool {
    if item.is_archived() {
        return false;
    }
    match &item.subject {
        Some(ItemSubject::Issue(_)) => !perch_github::is_local(&item.repo_full_name),
        Some(ItemSubject::PullRequest(pr)) => !pr.merged && !perch_github::is_local(&item.repo_full_name),
        None => item.checkout.is_some(),
    }
}

/// The branch glyph in the bar, on anything Start work can cut from.
pub fn button(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let id = Memo::new(move |_| {
        state.item.with(|i| {
            i.as_deref()
                .filter(|item| can_start_work(item))
                .map(|item| item.id.clone())
        })
    });
    dyn_container(
        move || id.get(),
        move |id| {
            let Some(id) = id else {
                return Empty::new().into_any();
            };
            let backend = backend.clone();
            tip(
                icon_button(
                    || SidebarIcon::Branch,
                    || theme::FG_MUTED,
                    move || start_work(app, state, &backend, &id),
                ),
                "Start work".to_string(),
            )
            .debug_name("start work button")
            .into_any()
        },
    )
    .style(move |s| {
        s.flex_shrink(0.0_f32)
            .pointer_events_auto()
            .apply_if(id.with(Option::is_none), |s| s.hide())
    })
}

/// Cut the item's branch and open it. From an issue or a pull request, a
/// prompt to work on it waits in the new branch's composer.
pub fn start_work(app: AppState, state: ReviewState, backend: &Backend, item_id: &str) {
    let navigation_id = backend.start_work(item_id);
    app.begin_open("Cutting a branch", navigation_id);
    let prompt = state
        .issue
        .with_untracked(|i| i.as_deref().map(work_prompt))
        .or_else(|| state.pull_request.with_untracked(|p| p.as_deref().map(pull_prompt)));
    app.start_work_draft.set(prompt.map(|p| (navigation_id, p)));
}

/// What to ask an agent on a branch cut for `issue`: the issue by number,
/// title and link, its body quoted, and what to do with it.
pub fn work_prompt(issue: &Issue) -> String {
    let mut prompt = format!(
        "Work on #{}: {}\nhttps://github.com/{}/issues/{}\n\n",
        issue.number, issue.title, issue.repo_full_name, issue.number
    );
    let body = perch_knowledge::sanitize::clean(&issue.body);
    if !body.trim().is_empty() {
        prompt.push_str(&crate::ask::quote_reply(body.trim()));
    }
    prompt.push_str(
        "Read the issue and its comments, make the change on this branch, and add tests where they fit.",
    );
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
        let pr = crate::review::header::tests::pull_request(812);
        assert_eq!(
            pull_prompt(&pr),
            "Continue #812: pull 812\n\
             https://github.com/slatedb/slatedb/pull/812\n\n\
             This branch has the pull request's commits. Read its description and review comments, \
             finish the change on this branch, and add tests where they fit."
        );
    }

    #[test]
    fn work_starts_from_anything_with_something_to_cut_from() {
        let now = "2026-09-20T12:00:00Z".parse().unwrap();
        let mut pr = crate::review::header::tests::pull_request(812);
        let mut item = Item::new("slatedb/slatedb", now);
        item.subject = Some(ItemSubject::PullRequest(pr.clone()));
        assert!(can_start_work(&item), "an open pull request");
        pr.merged = true;
        item.subject = Some(ItemSubject::PullRequest(pr));
        assert!(!can_start_work(&item), "a merged one has nothing left to do");

        item.subject = Some(ItemSubject::Issue(issue("")));
        assert!(can_start_work(&item), "an issue");
        item.archived_at = Some(now);
        assert!(!can_start_work(&item), "nothing archived");

        let branch = Item::new("slatedb/slatedb", now);
        assert!(!can_start_work(&branch), "a branch row with no checkout");
    }
}
