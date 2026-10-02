//! Start work: cut a worktree of the reader's own from the item, and open
//! it. An issue's branch starts from the default branch; a pull request's
//! from its head, with the whole change to show against its base; a
//! worktree's from its head. A pull request opened from the new branch
//! closes the issue or the pull request it came from.

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};

use perch_github::{Issue, Item, ItemKind, ItemSubject, PullRequest, SubjectKind, SubjectState, Thread};

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
pub fn button(app: AppState, state: ReviewState) -> impl IntoView {
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
            tip(
                icon_button(
                    || SidebarIcon::Branch,
                    theme::fg_muted,
                    move || ask(app, Pending::Item(id.clone())),
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

/// Cut the item's branch and open it, from the bar or any row's menu.
/// From an issue or a pull request, a prompt to work on it waits in the
/// new branch's composer: see `draft`.
pub fn start_work(app: AppState, backend: &Backend, item_id: &str) {
    let navigation_id = backend.start_work(item_id);
    app.begin_open("Cutting a branch", navigation_id);
}

/// Start work from a notification's pull request or issue, which may have
/// no item until the backend makes one.
pub fn start_work_on_thread(app: AppState, backend: &Backend, thread: &Thread) {
    let Some((kind, number)) = thread_subject(thread) else {
        return;
    };
    let navigation_id = backend.start_work_on_subject(&thread.repository.full_name, kind, number);
    app.begin_open("Cutting a branch", navigation_id);
}

/// What Start work would cut from, while its confirmation is up.
#[derive(Clone, PartialEq)]
pub enum Pending {
    Item(String),
    Thread(Thread),
}

/// Ask before cutting: the bar's button and every row's menu come here,
/// and the confirmation's Create goes on to `start_work`.
pub fn ask(app: AppState, pending: Pending) {
    app.start_work_confirm.set(Some(pending));
}

/// Create: cut what the confirmation was about.
fn confirm(app: AppState, backend: &Backend) {
    let Some(pending) = app.start_work_confirm.try_update(Option::take).flatten() else {
        return;
    };
    match pending {
        Pending::Item(id) => start_work(app, backend, &id),
        Pending::Thread(thread) => start_work_on_thread(app, backend, &thread),
    }
}

fn cancel(app: AppState) {
    app.start_work_confirm.set(None);
}

/// The confirmation's keys, from the window while it is up: Enter
/// creates, Escape cancels, and nothing else gets through.
pub fn key(app: AppState, backend: &Backend, key: &KeyboardEvent) -> EventPropagation {
    if key.is_composing {
        return EventPropagation::Continue;
    }
    if key.modifiers.is_empty() {
        match key.code {
            Code::Escape => cancel(app),
            Code::Enter | Code::NumpadEnter => confirm(app, backend),
            _ => {}
        }
    }
    EventPropagation::Stop
}

/// The name of what the confirmation would cut from.
fn pending_title(app: AppState, pending: &Pending) -> String {
    match pending {
        Pending::Item(id) => app.item(id).map(|item| item.title().to_string()).unwrap_or_default(),
        Pending::Thread(thread) => thread.subject.title.clone(),
    }
}

/// The confirmation over the window, while it is up. A press outside it
/// cancels, as one outside Open… puts that away.
pub fn overlay(app: AppState, backend: Rc<Backend>) -> impl IntoView {
    let shown = Memo::new(move |_| app.start_work_confirm.with(Option::is_some));
    dyn_container(
        move || shown.get(),
        move |open| {
            if !open {
                return Empty::new().into_any();
            }
            Container::new(panel(app, backend.clone()))
                .on_event_stop(el::PointerDown, move |_, _| cancel(app))
                .style(|s| s.size_full())
                .into_any()
        },
    )
    .debug_name("start-work-confirm")
    .style(move |s| {
        s.position(Position::Absolute)
            .inset(0.0)
            .z_index(100)
            .apply_if(!shown.get(), |s| s.hide())
    })
}

const CONFIRM_WIDTH: f64 = 380.0;

fn panel(app: AppState, backend: Rc<Backend>) -> impl IntoView {
    let title = app
        .start_work_confirm
        .with_untracked(|p| p.as_ref().map(|p| pending_title(app, p)))
        .unwrap_or_default();
    let has_title = !title.is_empty();
    let cancel_button = crate::review::open_pull::action_button(
        || "Cancel".to_string(),
        "start-work-cancel",
        false,
        || true,
        move || cancel(app),
    );
    let create_button = crate::review::open_pull::action_button(
        || "Create worktree".to_string(),
        "start-work-create",
        true,
        || true,
        move || confirm(app, &backend),
    );
    // Create is the default button: it takes the keyboard once laid out,
    // so Enter and Escape reach the window whatever had it before.
    let create_id = create_button.id();
    let taken = floem::reactive::RwSignal::new(false);
    let create_button = create_button.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if !taken.get_untracked() {
            taken.set(true);
            create_id.request_focus();
        }
    });
    Stack::vertical((
        Label::new("Create a new worktree from this item?").style(|s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
                .selectable(false)
        }),
        Label::new(title).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .text_wrap()
                .width_full()
                .margin_top(4.0)
                .apply_if(!has_title, |s| s.hide())
        }),
        Stack::horizontal((Empty::new().style(|s| s.flex_grow(1.0_f32)), cancel_button, create_button))
            .style(|s| s.width_full().items_center().gap(8.0).margin_top(14.0)),
    ))
    .on_event_stop(el::PointerDown, |_, _| {})
    .debug_name("start-work-confirm-panel")
    .style(move |s| {
        let (left, width) =
            crate::review::go_to::placement(app.main_pane.get(), app.window_width.get());
        let panel = CONFIRM_WIDTH.min(width);
        s.position(Position::Absolute)
            .inset_top(theme::GO_TO_TOP)
            .inset_left(left + (width - panel) / 2.0)
            .width(panel)
            .flex_col()
            .padding(16.0)
            .apply(crate::ui::menu::panel_surface(floem::style::Style::new()))
    })
}

/// The pull request or the issue a notification is about: what Start work
/// from its row cuts from. Other kinds of notification have none.
pub fn thread_subject(thread: &Thread) -> Option<(ItemKind, u64)> {
    let kind = match thread.subject.kind {
        SubjectKind::Issue => ItemKind::Issue,
        SubjectKind::PullRequest => ItemKind::PullRequest,
        _ => return None,
    };
    let number = thread.number_hint()?;
    Some((kind, number))
}

/// Whether a notification's subject is still open to work on, as far as a
/// look has found: a merged pull request or a closed issue is done with.
/// One not looked at yet is left to the backend to judge.
pub fn thread_open_for_work(thread: &Thread) -> bool {
    !matches!(
        thread.subject_snapshot.as_ref().map(|s| s.state),
        Some(SubjectState::PrMerged | SubjectState::IssueClosed | SubjectState::IssueNotPlanned)
    )
}

/// What waits in the composer of a branch cut from `source`: a prompt to
/// work on its issue or to continue its pull request. None from a worktree.
pub fn draft(source: &Item) -> Option<String> {
    match source.subject.as_ref()? {
        ItemSubject::Issue(issue) => Some(work_prompt(issue)),
        ItemSubject::PullRequest(pr) => Some(pull_prompt(pr)),
    }
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

    /// The prompt comes from the row Start work cut from, not from what
    /// the main pane has open, so it is right from any row's menu.
    #[test]
    fn the_draft_is_the_sources_prompt() {
        let now = "2026-09-20T12:00:00Z".parse().unwrap();
        let mut item = Item::new("slatedb/slatedb", now);
        assert_eq!(draft(&item), None, "a worktree has nothing to ask about");
        item.subject = Some(ItemSubject::Issue(issue("Fence them.")));
        assert_eq!(draft(&item), Some(work_prompt(&issue("Fence them."))));
        let pr = crate::review::header::tests::pull_request(812);
        item.subject = Some(ItemSubject::PullRequest(pr.clone()));
        assert_eq!(draft(&item), Some(pull_prompt(&pr)));
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
