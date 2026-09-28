//! What is an issue's alone: Start work and the state its glyph is drawn
//! in. The body is Conversation, drawn from `review::cards`, and the header
//! over it is the same bar every item has.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::SignalWith;
use floem::views::{dyn_container, Decorators, Empty};

use perch_github::Issue;

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::ui::widgets::text_button;

/// Cut a branch from the default branch and open it. When one exists
/// already, opens that. The issue's primary action, where a pull
/// request's is Submit review.
pub fn start_work_button(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    dyn_container(
        move || state.item.with(|i| i.as_ref().map(|i| i.id.clone())),
        move |id| {
            let Some(id) = id else {
                return Empty::new().into_any();
            };
            let backend = backend.clone();
            text_button(
                || "Start work".to_string(),
                SidebarIcon::Branch,
                move || start_work(app, state, &backend, &id),
            )
            .debug_name("start work button")
            .into_any()
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

/// Cut the issue's branch and open it, with a prompt to work on the issue
/// waiting in its composer.
pub fn start_work(app: AppState, state: ReviewState, backend: &Backend, issue_item_id: &str) {
    let navigation_id = backend.start_work(issue_item_id);
    app.begin_open("Cutting a branch", navigation_id);
    let prompt = state.issue.with_untracked(|i| i.as_deref().map(work_prompt));
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
}
