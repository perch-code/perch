//! The summary at the top of Conversation, and the details in the bar.
//!
//! The summary is one line: at its left what the item is, its state, its
//! repository and number, who opened it and its churn; at its right
//! whether it can go in, as three glyphs each in a place of its own: the
//! checks, the review, and whether it merges cleanly. Each is green,
//! yellow or red, and says the rest under the pointer. The checks' glyph
//! lists every check in its menu, and the bar's Merge has the rest.
//! The notices, and the way back from a retained snapshot, sit over the
//! line; on Code the notice is in the tab bar and the way back in the
//! rail, since the summary is Conversation's alone.
//!
//! Everything else is `details`, rows of a name and a value in the
//! popover the bar's details button opens, as Codex has them: Branch,
//! Worktree, Reviewers, then what GitHub says about the item. A row is
//! absent when it has nothing.

use std::rc::Rc;
use std::sync::Arc;

use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::views::{dyn_container, dyn_stack, Container, Decorators, Empty, Label, Stack};
use perch_github::{
    CheckState, Checks, Item, ItemKind, ItemSubject, MentionableUser, SubjectState,
};

use crate::backend::{Backend, StateChange};
use crate::icons::{self, SidebarIcon};
use crate::review::cards::{avatar_url_for, tip, tip_with, verdict_glyph};
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::timefmt;
use crate::ui::menu::{Menu, MenuExt};
use crate::ui::widgets::{
    avatar, badge_box, badge_text, clickable, name_label, open_in_browser, sidebar_glyph,
    worktree_menu, Name,
};

/// Whether the line is the description card's strip rather than the
/// summary's: a pull request and an issue have a description, and a
/// worktree of the reader's own has none.
fn line_in_card(state: ReviewState) -> bool {
    state.pull_request.with(Option::is_some) || state.issue.with(Option::is_some)
}

/// The block at the top of the page: the notices, then the line, unless
/// the description card has it. Hidden rather than empty when there is
/// nothing to say, so what follows it starts at the top.
pub fn summary(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let author = Memo::new(move |_| github_data(app, state).author);
    let in_card = Memo::new(move |_| line_in_card(state));
    let shown = Memo::new(move |_| {
        state.historical_review.with(Option::is_some)
            || notice_text(state).is_some()
            || (!in_card.get()
                && (subject_state(state).is_some()
                    || !repo_of(state).is_empty()
                    || churn_of(state).is_some()
                    || author.with(Option::is_some)))
    });
    // Asked as the item opens, so the details have them when they open.
    ask(app, state, &backend);
    let line_backend = backend.clone();
    Stack::vertical((
        notices(app, state, backend),
        dyn_container(
            move || in_card.get(),
            move |in_card| {
                if in_card {
                    Empty::new().into_any()
                } else {
                    line(app, state, line_backend.clone(), author, None).into_any()
                }
            },
        )
        .style(move |s| s.width_full().apply_if(in_card.get(), |s| s.hide())),
    ))
    .debug_name("item summary")
    .style(move |s| {
        // Over a card, a notice is a card's gap from it; the line alone
        // heads the page, and keeps a title's room under it.
        let under = if in_card.get() {
            theme::CARD_GAP
        } else {
            theme::CONVERSATION_TITLE_GAP
        };
        s.flex_col()
            .width_full()
            .padding_bottom(under)
            .apply_if(!shown.get(), |s| s.hide())
    })
}

/// The line as the description card's strip, in place of a heading: the
/// card's `actions`, drawn under the pointer, come before the status
/// glyphs, which then stay where they are at the line's end.
pub fn card_line(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    actions: floem::AnyView,
) -> impl IntoView {
    let author = Memo::new(move |_| github_data(app, state).author);
    line(app, state, backend, author, Some(actions))
}

/// What the details ask the backend for as an item opens.
#[derive(Clone, Copy)]
struct Asked {
    /// The repository's grants, on an item that takes Work runs.
    grants: Memo<Vec<(String, String)>>,
    /// Who the item can be assigned to, when the reader may assign.
    assignable: Memo<Option<Arc<Vec<MentionableUser>>>>,
}

fn ask(app: AppState, state: ReviewState, backend: &Rc<Backend>) -> Asked {
    // The repository's grants, on an item that takes Work runs: asked for
    // as the item opens, and kept as the backend sends them.
    let work_repo = Memo::new(move |_| {
        let viewer = app
            .viewer
            .with(|v| v.as_ref().map(|(login, _)| login.clone()));
        state.item.with(|item| {
            item.as_ref()
                .filter(|item| crate::items::work_allowed(item, viewer.as_deref()))
                .map(|item| item.repo_full_name.clone())
        })
    });
    {
        let backend = backend.clone();
        floem::reactive::Effect::new(move |_| {
            if let Some(repo) = work_repo.get() {
                backend.list_grants(&repo);
            }
        });
    }
    let grants = Memo::new(move |_| {
        work_repo
            .get()
            .map(|repo| {
                state
                    .grants
                    .with(|all| all.get(&repo.to_lowercase()).cloned().unwrap_or_default())
            })
            .unwrap_or_default()
            .into_iter()
            .map(|grant| (grant.id, grant.label))
            .collect::<Vec<_>>()
    });
    // Who the item can be assigned to, when the reader may assign: asked
    // as an issue or a pull request opens, since whoever took part in it
    // can be as well as the repository's assignable users.
    let assign_item = Memo::new(move |_| {
        state.item.with(|item| {
            item.as_ref().and_then(|item| {
                let number = item.subject.as_ref()?.number();
                Some((item.repo_full_name.clone(), number))
            })
        })
    });
    {
        let backend = backend.clone();
        floem::reactive::Effect::new(move |_| {
            if let Some((repo, number)) = assign_item.get() {
                backend.assignable_users(&repo, number);
            }
        });
    }
    let assignable = Memo::new(move |_| {
        assign_item.get().and_then(|(repo, number)| {
            state.assignable.with(|all| {
                all.get(&crate::review::model::assignable_key(&repo, number))
                    .cloned()
                    .flatten()
            })
        })
    });
    Asked { grants, assignable }
}

/// The rest of what there is to say about the item, in rows, at the top
/// of the bar's details: its branches and worktree, then what GitHub says
/// about it. A row is absent when it has nothing, and the block with
/// them.
pub(crate) fn details(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let data = Memo::new(move |_| github_data(app, state));
    let Asked { grants, assignable } = ask(app, state, &backend);
    let shown = Memo::new(move |_| {
        !branches_text(state).is_empty()
            || data.with(|d| d.has_details())
            || grants.with(|g| !g.is_empty())
            || assignable.with(Option::is_some)
    });
    let grants_backend = backend.clone();
    let assign_backend = backend.clone();
    let closes_backend = backend.clone();
    let open_backend = backend.clone();
    Stack::vertical((
        row(
            SidebarIcon::Branch,
            "Branch",
            move || !branches_text(state).is_empty(),
            branches(state),
        ),
        row(
            SidebarIcon::Folder,
            "Worktree",
            move || checkout_on_disk(state).is_some(),
            Stack::horizontal((
                directory(app, state, backend.clone()),
                divergence(state),
                push_button(app, state, backend.clone()),
                update_button(state, backend.clone()),
                rebase_button(state, backend.clone()),
            ))
            .style(|s| {
                s.items_center()
                    .min_width(0.0)
                    .flex_wrap(floem::taffy::FlexWrap::Wrap)
                    .col_gap(theme::SUMMARY_GAP)
                    .row_gap(4.0)
            }),
        ),
        dyn_row(
            SidebarIcon::Person,
            "Reviewers",
            move || data.with(|d| d.reviewers.clone()),
            |r| !r.is_empty(),
            move |reviewers| reviewers_view(app, reviewers),
        ),
        dyn_row(
            SidebarIcon::Check,
            "Work may",
            move || grants.get(),
            |g| !g.is_empty(),
            move |grants| {
                let backend = grants_backend.clone();
                chips(grants, move |(id, label)| {
                    grant_chip(backend.clone(), id, label)
                })
            },
        ),
        dyn_row(
            SidebarIcon::Tag,
            "Labels",
            move || data.with(|d| d.labels.clone()),
            |l| !l.is_empty(),
            |labels| chips(labels, |l| label_chip(l.name, l.color)),
        ),
        // Shown with nobody in it too when the reader may assign, for
        // the Assign menu.
        dyn_row(
            SidebarIcon::Person,
            "Assignees",
            move || {
                (
                    data.with(|d| d.assignees.clone()),
                    assignable.with(Option::is_some),
                )
            },
            |(logins, may_assign)| !logins.is_empty() || *may_assign,
            move |(logins, may_assign)| {
                let nobody = logins.is_empty();
                Stack::horizontal((
                    assignees(app, state, logins, assignable)
                        .style(move |s| s.min_width(0.0).apply_if(nobody, |s| s.hide())),
                    assign_button(app, state, assign_backend.clone(), assignable)
                        .style(move |s| s.apply_if(!may_assign, |s| s.hide())),
                ))
                .style(|s| {
                    s.items_center()
                        .flex_wrap(floem::taffy::FlexWrap::Wrap)
                        .gap(10.0)
                        .min_width(0.0)
                })
            },
        ),
        dyn_row(
            SidebarIcon::Milestone,
            "Milestone",
            move || data.with(|d| d.milestone.clone()),
            Option::is_some,
            |name| milestone(name.unwrap_or_default()),
        ),
        dyn_row(
            SidebarIcon::Issue,
            "Closes",
            move || data.with(|d| d.closes.clone()),
            |c| !c.is_empty(),
            move |closes| {
                let backend = closes_backend.clone();
                let repo = state
                    .item
                    .with(|i| i.as_ref().map(|i| i.repo_full_name.clone()))
                    .unwrap_or_default();
                chips(closes, move |n| {
                    let (backend, repo) = (backend.clone(), repo.clone());
                    let open = move || {
                        crate::ui::agents::dropdown::close(state, false);
                        // By number rather than by row: the issue may
                        // have no item yet, and the backend makes one.
                        let navigation_id = backend.open_subject(&repo, ItemKind::Issue, n);
                        app.begin_open("Reading the issue", navigation_id);
                    };
                    chip(
                        format!("#{n}"),
                        theme::badge_bg,
                        theme::fg_muted,
                        Some(Rc::new(open)),
                    )
                })
            },
        ),
        dyn_row(
            SidebarIcon::Link,
            "In Perch",
            move || data.with(|d| d.in_perch.clone()),
            |i| !i.is_empty(),
            move |items| {
                let backend = open_backend.clone();
                chips(items, move |item| {
                    let backend = backend.clone();
                    let name = item.title().to_string();
                    let open = move || {
                        crate::ui::agents::dropdown::close(state, false);
                        crate::ui::open_item(app, state, &backend, &item);
                    };
                    chip(name, theme::badge_bg, theme::fg_muted, Some(Rc::new(open)))
                })
            },
        ),
    ))
    .debug_name("item details")
    .style(move |s| {
        s.flex_col()
            .width_full()
            .padding_horiz(14.0)
            .padding_vert(6.0)
            .border_bottom(1.0)
            .border_color(theme::border_muted())
            .apply_if(!shown.get(), |s| s.hide())
    })
}

// ---------------------------------------------------------------------
// The rows
// ---------------------------------------------------------------------

/// One row: a glyph and its name in a column of their own, then the
/// value beside them. A value that wraps, reviewers or labels past the
/// column's edge, makes its row taller, and the name stays on the first
/// line. Hidden rather than empty when there is nothing, so the rows
/// above and below close up.
fn row(
    glyph: SidebarIcon,
    name: &'static str,
    present: impl Fn() -> bool + 'static,
    value: impl IntoView + 'static,
) -> impl IntoView {
    Stack::horizontal((
        Stack::horizontal((
            sidebar_glyph(glyph, theme::fg_subtle).style(|s| {
                s.size(theme::SUMMARY_GLYPH, theme::SUMMARY_GLYPH)
                    .margin_right(10.0)
                    .flex_shrink(0.0_f32)
            }),
            Label::derived(move || name.to_string()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_muted())
                    .selectable(false)
            }),
        ))
        .debug_name(format!("summary name {name}"))
        .style(|s| {
            s.width(theme::SUMMARY_NAME_WIDTH)
                .height(theme::SUMMARY_ROW)
                .items_center()
                .flex_shrink(0.0_f32)
        }),
        Container::new(value).style(|s| {
            s.min_height(theme::SUMMARY_ROW)
                .items_center()
                .flex_grow(1.0_f32)
                .min_width(0.0)
        }),
    ))
    .debug_name(format!("summary row {name}"))
    .style(move |s| {
        s.items_start()
            .width_full()
            .apply_if(!present(), |s| s.hide())
    })
}

/// A row whose value is built again when what it shows changes: the
/// reviewers, the labels, and the rest of what GitHub says.
fn dyn_row<T: Clone + PartialEq + 'static, V: IntoView + 'static>(
    glyph: SidebarIcon,
    name: &'static str,
    data: impl Fn() -> T + Copy + 'static,
    present: impl Fn(&T) -> bool + Copy + 'static,
    value: impl Fn(T) -> V + 'static,
) -> impl IntoView {
    row(
        glyph,
        name,
        move || present(&data()),
        dyn_container(data, move |data| value(data).into_any()).style(|s| s.min_width(0.0)),
    )
}

/// The one line under the title. At its left, what the item is: Open,
/// Draft, Merged, Closed as the pill, the repository and number as the
/// sidebar's rows say them, who opened it, and the lines added and
/// removed. At its right, whether it can go in: the status glyphs.
/// Merging is the bar's, and the rest of what there is to say is in the
/// bar's details. A worktree of the reader's own has no state, and starts
/// at its repository; one with no GitHub remote has its churn alone.
fn line(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    author: Memo<Option<(String, String, Option<jiff::Timestamp>)>>,
    actions: Option<floem::AnyView>,
) -> impl IntoView {
    let present = move || {
        subject_state(state).is_some()
            || !repo_of(state).is_empty()
            || churn_of(state).is_some()
            || author.with(Option::is_some)
    };
    Stack::horizontal((
        state_pill(app, state, backend),
        Label::derived(move || repo_of(state))
            .debug_name("repository line")
            .on_event_stop(el::Click, move |_, _| {
                if let Some(url) = url_of(state) {
                    open_in_browser(&url);
                }
            })
            .style(move |s| {
                let opens = url_of(state).is_some();
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_muted())
                    .selectable(false)
                    .apply_if(repo_of(state).is_empty(), |s| s.hide())
                    // Underlined under the pointer, as a name is.
                    .apply_if(opens, crate::ui::widgets::underlined_link)
            }),
        dyn_container(
            move || author.get(),
            move |author| match author {
                Some(author) => author_view(app, author).into_any(),
                None => Empty::new().into_any(),
            },
        )
        .style(move |s| {
            s.min_width(0.0)
                .apply_if(author.with(Option::is_none), |s| s.hide())
        }),
        churn(state),
        // What is left of the line, so the glyphs end it.
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        actions.unwrap_or_else(|| Empty::new().into_any()),
        status_glyphs(state),
    ))
    .debug_name("summary line")
    .style(move |s| {
        s.items_center()
            .width_full()
            .min_width(0.0)
            .min_height(theme::SUMMARY_ROW)
            .flex_wrap(floem::taffy::FlexWrap::Wrap)
            .col_gap(theme::SUMMARY_GAP)
            .apply_if(!present(), |s| s.hide())
    })
}

/// The repository and number, as the sidebar's rows say them. A worktree of
/// the reader's own is its repository alone, and one with no GitHub
/// remote has no slug worth a word: the sidebar's heading already says
/// which repository it is.
fn repo_line(item: &Item) -> String {
    match &item.subject {
        Some(ItemSubject::PullRequest(pr)) => format!("{} #{}", pr.repo_full_name, pr.number),
        Some(ItemSubject::Issue(issue)) => format!("{} #{}", issue.repo_full_name, issue.number),
        None if perch_github::is_local(&item.repo_full_name) => String::new(),
        None => item.repo_full_name.clone(),
    }
}

fn repo_of(state: ReviewState) -> String {
    state
        .item
        .with(|i| i.as_deref().map(repo_line).unwrap_or_default())
}

/// The pull request's or the issue's page, which the repository line
/// opens. A branch has none.
fn url_of(state: ReviewState) -> Option<String> {
    state
        .item
        .with(|i| i.as_deref().and_then(crate::ui::items::github_url))
}

/// A commit as a base reads as its short hash; a branch as its name.
fn short_ref(base: &str) -> String {
    if base.len() == 40 && base.chars().all(|c| c.is_ascii_hexdigit()) {
        base[..7].to_string()
    } else {
        base.to_string()
    }
}

/// The state of what is open, as the sidebar's glyphs draw it.
pub(crate) fn subject_state(state: ReviewState) -> Option<SubjectState> {
    state
        .pull_request
        .with(|p| p.as_ref().map(|p| p.subject_state()))
        .or_else(|| state.issue.with(|i| i.as_ref().map(|i| i.subject_state())))
}

/// The word in the state pill.
fn state_word(state: SubjectState) -> &'static str {
    match state {
        SubjectState::PrOpen | SubjectState::IssueOpen => "Open",
        SubjectState::PrDraft => "Draft",
        SubjectState::PrMerged => "Merged",
        SubjectState::PrClosed | SubjectState::IssueClosed => "Closed",
        SubjectState::IssueNotPlanned => "Not planned",
    }
}

/// Whether the reader may convert the pull request to a draft or mark it
/// ready for review: GitHub lets its author, and anyone who may write to
/// the repository. Not offline, and not from an old review's snapshot.
pub(crate) fn may_set_draft(app: AppState, state: ReviewState) -> bool {
    if state.offline.get() || state.historical_review.with(Option::is_some) {
        return false;
    }
    let viewer = app
        .viewer
        .with(|v| v.as_ref().map(|(login, _)| login.clone()));
    let writes = state.pull_request_overview.with(|overview| {
        overview.as_ref().is_some_and(|overview| {
            matches!(
                overview.viewer_permission,
                Some(
                    perch_github::RepositoryPermission::Admin
                        | perch_github::RepositoryPermission::Maintain
                        | perch_github::RepositoryPermission::Write
                )
            )
        })
    });
    state.pull_request.with(|pull| {
        pull.as_ref().is_some_and(|pull| {
            let authored = viewer
                .as_deref()
                .is_some_and(|viewer| viewer.eq_ignore_ascii_case(&pull.author_login));
            pull.is_open() && !pull.merged && (authored || writes)
        })
    })
}

/// Whether the reader may close the pull request or the issue, or reopen
/// it: GitHub lets a pull request's author, and anyone with triage access
/// to its repository. For an issue it says, and until it does only an
/// open issue's author is offered the close. Not offline, and not from
/// an old review's snapshot.
pub(crate) fn may_change_state(app: AppState, state: ReviewState) -> bool {
    use perch_github::RepositoryPermission;

    if state.offline.get() || state.historical_review.with(Option::is_some) {
        return false;
    }
    let viewer = app
        .viewer
        .with(|v| v.as_ref().map(|(login, _)| login.clone()));
    let authored = |login: &str| {
        viewer
            .as_deref()
            .is_some_and(|v| v.eq_ignore_ascii_case(login))
    };
    if state.pull_request.with(Option::is_some) {
        let triages = state.pull_request_overview.with(|overview| {
            overview.as_ref().is_some_and(|overview| {
                matches!(
                    overview.viewer_permission,
                    Some(
                        RepositoryPermission::Admin
                            | RepositoryPermission::Maintain
                            | RepositoryPermission::Write
                            | RepositoryPermission::Triage
                    )
                )
            })
        });
        return state.pull_request.with(|pull| {
            pull.as_ref()
                .is_some_and(|pull| !pull.merged && (triages || authored(&pull.author_login)))
        });
    }
    let (closable, reopenable) = (state.issue_closable.get(), state.issue_reopenable.get());
    state.issue.with(|issue| {
        issue.as_ref().is_some_and(|issue| {
            if issue.is_open() {
                closable.unwrap_or_else(|| authored(&issue.author_login))
            } else {
                reopenable.unwrap_or(false)
            }
        })
    })
}

/// Convert the pull request to a draft, or mark it ready for review, as
/// the state pill's menu and the merge box do.
pub(crate) fn set_draft(state: ReviewState, backend: &Backend, draft: bool) {
    let Some(item_id) = state
        .item
        .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
    else {
        return;
    };
    state.draft_pending.set(Some(draft));
    backend.set_draft(&item_id, draft);
}

/// Close or reopen the pull request or the issue, as the state pill's
/// menu and the duplicate search under it do.
pub(crate) fn change_state(state: ReviewState, backend: &Backend, change: StateChange) {
    let Some(item_id) = state
        .item
        .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
    else {
        return;
    };
    state.state_pending.set(Some(change));
    backend.change_state(&item_id, change);
}

/// The state pill's menu: what the state can become. An open pull
/// request converts to a draft and a draft is marked ready, when the
/// reader may; either closes, and a closed one reopens. An open issue
/// closes in GitHub's three ways, the last by a search for the issue it
/// repeats, and a closed one reopens. A change under way is the one row,
/// greyed.
fn state_menu(
    state: ReviewState,
    backend: Rc<Backend>,
    subject: SubjectState,
    may: (bool, bool),
    pill: floem::ViewId,
) -> Menu {
    let (may_draft, may_state) = may;
    let pending = match (
        state.draft_pending.get_untracked(),
        state.state_pending.get_untracked(),
    ) {
        (Some(false), _) => Some("Marking ready for review\u{2026}"),
        (Some(true), _) => Some("Converting to draft\u{2026}"),
        (_, Some(StateChange::Reopen)) => Some("Reopening\u{2026}"),
        (_, Some(_)) => Some("Closing\u{2026}"),
        (None, None) => None,
    };
    if let Some(label) = pending {
        return Menu::new().item(label, |item| item.enabled(false));
    }
    let change = |label: &'static str, change: StateChange| {
        let backend = backend.clone();
        move |menu: Menu| {
            menu.item(label, move |item| {
                item.action(move || change_state(state, &backend, change))
            })
        }
    };
    let mut menu = Menu::new();
    match subject {
        SubjectState::PrOpen | SubjectState::PrDraft => {
            if may_draft {
                let draft = subject == SubjectState::PrOpen;
                let label = if draft {
                    "Convert to draft"
                } else {
                    "Ready for review"
                };
                let backend = backend.clone();
                menu = menu.item(label, move |item| {
                    item.action(move || set_draft(state, &backend, draft))
                });
            }
            if may_state {
                if may_draft {
                    menu = menu.separator();
                }
                menu = change("Close pull request", StateChange::ClosePullRequest)(menu);
            }
        }
        SubjectState::PrClosed if may_state => {
            menu = change("Reopen pull request", StateChange::Reopen)(menu);
        }
        SubjectState::IssueOpen if may_state => {
            use perch_github::IssueCloseReason;
            menu = change(
                "Close as completed",
                StateChange::CloseIssue(IssueCloseReason::Completed),
            )(menu);
            menu = change(
                "Close as not planned",
                StateChange::CloseIssue(IssueCloseReason::NotPlanned),
            )(menu);
            menu = menu.item("Close as duplicate\u{2026}", move |item| {
                item.action(move || {
                    crate::review::close_menu::search_duplicate(
                        state,
                        pill.get_visual_rect_no_clip(),
                    )
                })
            });
        }
        SubjectState::IssueClosed | SubjectState::IssueNotPlanned if may_state => {
            menu = change("Reopen issue", StateChange::Reopen)(menu);
        }
        _ => {}
    }
    menu
}

/// Open, Draft, Merged, Closed: the state's icon and its word, in the
/// state's colour on a wash of it. A branch has no state, and no pill.
/// It opens a menu of what the state can become, when the reader may
/// change it: see `state_menu`.
fn state_pill(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // The overview, and the write access it says the reader has, is read
    // again on each refresh. What it said is kept meanwhile, so the menu
    // does not go for the moment of each one.
    type Changeable = (Option<(String, u64)>, (bool, bool));
    let changeable = Memo::new(move |prev: Option<&Changeable>| {
        let pull = state
            .pull_request
            .with(|p| p.as_ref().map(|p| (p.repo_full_name.clone(), p.number)));
        let reading = state.pull_request_overview_loading.get()
            && state.pull_request_overview.with(Option::is_none);
        let kept = |pick: fn(&(bool, bool)) -> bool| {
            prev.is_some_and(|(was, may)| pick(may) && reading && *was == pull)
        };
        let may_draft = kept(|may| may.0) || may_set_draft(app, state);
        let may_state = kept(|may| may.1) || may_change_state(app, state);
        (pull, (may_draft, may_state))
    });
    let key = Memo::new(move |_| {
        let subject = subject_state(state);
        let (may_draft, may_state) = changeable.with(|(_, may)| *may);
        let may_draft =
            may_draft && matches!(subject, Some(SubjectState::PrOpen | SubjectState::PrDraft));
        let may_state = may_state && !matches!(subject, Some(SubjectState::PrMerged) | None);
        (subject, (may_draft, may_state))
    });
    dyn_container(
        move || key.get(),
        move |(s, may)| match s {
            Some(subject) => {
                let changes = may.0 || may.1;
                let color = move || icons::state_color(subject);
                let pill = Stack::horizontal((
                    floem::views::svg(move || icons::state_icon(subject)).style(move |s| {
                        s.size(14.0, 14.0)
                            .color(color())
                            .margin_right(5.0)
                            .pointer_events_none()
                    }),
                    Label::derived(move || state_word(subject).to_string()).style(move |s| {
                        s.font_size(theme::TEXT_SECTION)
                            .font_weight(theme::weight_emphasis())
                            .color(color())
                            .selectable(false)
                            .pointer_events_none()
                    }),
                    sidebar_glyph(SidebarIcon::ChevronDown, color).style(move |s| {
                        s.size(10.0, 10.0)
                            .margin_left(5.0)
                            .flex_shrink(0.0_f32)
                            .pointer_events_none()
                            .apply_if(!changes, |s| s.hide())
                    }),
                ))
                .debug_name("state pill")
                .style(move |s| {
                    s.items_center()
                        .height(theme::STATE_PILL_HEIGHT)
                        .padding_left(7.0)
                        .padding_right(if changes { 8.0 } else { 9.0 })
                        .background(color().with_alpha(theme::STATE_WASH))
                        .border_radius(theme::STATE_PILL_HEIGHT / 2.0)
                        .flex_shrink(0.0_f32)
                        .apply_if(changes, |s| {
                            s.hover(|s| {
                                s.background(color().with_alpha(theme::STATE_WASH * 2.0))
                                    .cursor(CursorStyle::Pointer)
                            })
                        })
                });
                if changes {
                    let backend = backend.clone();
                    let id = pill.view_id();
                    pill.dropdown(move || state_menu(state, backend.clone(), subject, may, id))
                        .into_any()
                } else {
                    pill.into_any()
                }
            }
            None => Empty::new().into_any(),
        },
    )
    // Hidden rather than empty without a state, so the number is not a
    // gap in from the column.
    .style(move |s| {
        s.flex_shrink(0.0_f32)
            .apply_if(subject_state(state).is_none(), |s| s.hide())
    })
}

/// How the head's checks stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Passing,
    Failing,
    Running,
}

/// What the checks' glyph says under the pointer: the verdict, its
/// words, and how many passed beside a failure.
#[derive(Debug, Clone, PartialEq)]
struct ChecksLine {
    verdict: Verdict,
    text: String,
    passed: Option<usize>,
}

impl ChecksLine {
    /// The words whole, as `clippy failed · 13 passed`.
    fn words(&self) -> String {
        match self.passed {
            Some(n) => format!("{} · {n} passed", self.text),
            None => self.text.clone(),
        }
    }
}

/// The checks' line for `checks`: the check that failed by name, or
/// how many did, with how many passed beside it; how many passed when all
/// did; running as it is. Each check is counted when GitHub has listed
/// them, as the live overview does; the cached rollup has only its
/// verdict and the run that failed. None when there is no verdict to
/// give.
fn checks_line_of(checks: &Checks) -> Option<ChecksLine> {
    let mut failed: Vec<&str> = Vec::new();
    for check in checks.runs.iter().filter(|c| c.state.is_failing()) {
        // A workflow run on both push and pull_request fails twice under
        // one name, and is one name here.
        if !failed.contains(&check.name.as_str()) {
            failed.push(&check.name);
        }
    }
    let passed = checks.runs.iter().filter(|c| c.state.is_passing()).count();
    let running = checks.runs.iter().any(|c| c.state.is_pending());
    let line = |verdict, text: String, passed| {
        Some(ChecksLine {
            verdict,
            text,
            passed,
        })
    };
    let beside = (passed > 0).then_some(passed);
    match failed.as_slice() {
        [] => {}
        [one] => return line(Verdict::Failing, format!("{one} failed"), beside),
        many => return line(Verdict::Failing, format!("{} failed", many.len()), beside),
    }
    if let CheckState::Failing { workflow } = &checks.state {
        let text = if workflow.is_empty() {
            "Checks failed".to_string()
        } else {
            format!("{workflow} failed")
        };
        return line(Verdict::Failing, text, beside);
    }
    if running || checks.state == CheckState::Running {
        return line(Verdict::Running, "Checks running".to_string(), None);
    }
    if checks.state == CheckState::Passing || passed > 0 {
        let text = if passed > 0 {
            format!("{passed} passed")
        } else {
            "Checks passed".to_string()
        };
        return line(Verdict::Passing, text, None);
    }
    None
}

/// The checks' line for the open item, from the live overview's
/// checks once they have come, else the cached rollup. Tracked.
fn checks_line_now(state: ReviewState) -> Option<ChecksLine> {
    crate::review::merge::current_checks(state).and_then(|checks| checks_line_of(&checks))
}

/// The tallest the checks' menu grows before its rows scroll: some
/// fifteen checks, which most heads have fewer than.
const CHECKS_MENU_HEIGHT: f64 = 440.0;

/// How one of the line's status glyphs stands: green, yellow or red.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Tone {
    Good,
    Waiting,
    Bad,
}

impl Tone {
    fn ink(self) -> fn() -> floem::peniko::Color {
        match self {
            Tone::Good => theme::open,
            Tone::Waiting => theme::draft_yellow,
            Tone::Bad => theme::closed,
        }
    }
}

/// What a status glyph shows: its colour, and its words under the
/// pointer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Standing {
    tone: Tone,
    says: String,
}

impl Standing {
    fn new(tone: Tone, says: impl Into<String>) -> Self {
        Standing {
            tone,
            says: says.into(),
        }
    }
}

/// The checks' glyph and how it stands: a check, a cross, or the turning
/// arrow the checks' menu gives a run in progress.
fn checks_standing(line: &ChecksLine) -> (SidebarIcon, Standing) {
    let (icon, tone) = match line.verdict {
        Verdict::Passing => (SidebarIcon::Check, Tone::Good),
        Verdict::Failing => (SidebarIcon::Close, Tone::Bad),
        Verdict::Running => (SidebarIcon::Refresh, Tone::Waiting),
    };
    (icon, Standing::new(tone, line.words()))
}

/// GitHub's overview of the pull request while it is open: once it is
/// merged or closed there is nothing left to be ready for. None until the
/// live overview has come.
fn open_overview(state: ReviewState) -> Option<Arc<perch_github::PullRequestOverview>> {
    let pull = state.pull_request.get()?;
    if !pull.is_open() || pull.merged {
        return None;
    }
    state.pull_request_overview.get()
}

/// How the review stands on an open pull request: approved, still
/// needed, or changes asked for. None where GitHub gives no decision, a
/// repository that asks for no review.
fn review_standing(state: ReviewState) -> Option<Standing> {
    use perch_github::ReviewDecision;

    Some(match open_overview(state)?.review_decision? {
        ReviewDecision::Approved => Standing::new(Tone::Good, "Approved"),
        ReviewDecision::ReviewRequested => Standing::new(Tone::Waiting, "Review required"),
        ReviewDecision::ChangesRequested => Standing::new(Tone::Bad, "Changes requested"),
    })
}

/// Whether an open pull request merges cleanly into its base. None while
/// GitHub is still working it out.
fn merge_standing(state: ReviewState) -> Option<Standing> {
    use perch_github::{MergeState, MergeableState};

    let overview = open_overview(state)?;
    let base = state
        .pull_request
        .with(|pull| pull.as_ref().map(|pull| pull.base_ref.clone()))
        .unwrap_or_default();
    // As the merge box decides it: either of GitHub's words for conflicts.
    let conflicting = overview.mergeable == MergeableState::Conflicting
        || overview.merge_state == MergeState::Dirty;
    if conflicting {
        Some(Standing::new(Tone::Bad, format!("Conflicts with {base}")))
    } else if overview.mergeable == MergeableState::Mergeable {
        Some(Standing::new(
            Tone::Good,
            format!("No conflicts with {base}"),
        ))
    } else {
        None
    }
}

/// The side of a status glyph's box, and of the glyph in it.
const STATUS_BOX: f64 = 22.0;
const STATUS_GLYPH: f64 = 14.0;

/// One status glyph in its box, in its tone's colour. One that takes a
/// press lights under the pointer and takes the keyboard.
fn status_box(icon: SidebarIcon, tone: Tone, pressable: bool) -> Container {
    Container::new(
        sidebar_glyph(icon, tone.ink())
            .style(|s| s.size(STATUS_GLYPH, STATUS_GLYPH).pointer_events_none()),
    )
    .style(move |s| {
        s.size(STATUS_BOX, STATUS_BOX)
            .items_center()
            .justify_center()
            .flex_shrink(0.0_f32)
            .border_radius(theme::RADIUS)
            .apply_if(pressable, |s| {
                clickable(s)
                    .keyboard_navigable()
                    .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
                    .hover(|s| s.background(theme::canvas_hover()))
            })
    })
}

/// A place on the line for one status glyph: built again when how it
/// stands changes, and gone, with its place, when there is nothing to
/// say.
fn status_slot<T: Clone + PartialEq + 'static>(
    standing: Memo<Option<T>>,
    glyph: impl Fn(T) -> floem::AnyView + 'static,
) -> impl IntoView {
    dyn_container(
        move || standing.get(),
        move |standing| match standing {
            Some(standing) => glyph(standing),
            None => Empty::new().into_any(),
        },
    )
    .style(move |s| {
        s.flex_shrink(0.0_f32)
            .apply_if(standing.with(Option::is_none), |s| s.hide())
    })
}

/// Whether the item can go in, as three glyphs at the line's end, each
/// always in the same place: the checks, the review, and whether it
/// merges cleanly. Green is well, yellow is waiting, red blocks; the
/// words are under the pointer. A press on the checks opens the menu of
/// every check. Conflicts, for a reader the bar offers Merge, open the
/// merge box, where they are resolved. The review and the merge are an
/// open pull request's, once the live overview has come; the checks are
/// there as soon as something has been fetched.
fn status_glyphs(state: ReviewState) -> impl IntoView {
    // Memos, so a glyph is built again when what it says changes and not
    // on each refresh of the overview.
    let checks = Memo::new(move |_| checks_line_now(state).map(|line| checks_standing(&line)));
    let review = kept_while_reading(state, move || review_standing(state));
    let merge = kept_while_reading(state, move || {
        let resolves = crate::review::merge::may_merge(state)
            && !state
                .pull_request
                .with(|pull| pull.as_ref().is_some_and(|pull| pull.draft));
        merge_standing(state).map(|standing| (standing, resolves))
    });
    Stack::horizontal((
        status_slot(checks, move |(icon, standing): (SidebarIcon, Standing)| {
            let glyph = status_box(icon, standing.tone, true)
                .dropdown(move || checks_menu(state))
                .debug_name("checks status");
            tip(glyph, standing.says).into_any()
        }),
        status_slot(review, |standing: Standing| {
            let glyph =
                status_box(SidebarIcon::Unread, standing.tone, false).debug_name("review status");
            tip(glyph, standing.says).into_any()
        }),
        status_slot(merge, move |(standing, resolves): (Standing, bool)| {
            let opens = resolves && standing.tone == Tone::Bad;
            let glyph = status_box(SidebarIcon::Merge, standing.tone, opens)
                .debug_name("merge status")
                .on_event_stop(el::Click, move |_, _| {
                    if opens {
                        crate::review::merge_button::open(state);
                    }
                });
            tip(glyph, standing.says).into_any()
        }),
    ))
    .debug_name("status glyphs")
    .style(|s| s.items_center().gap(2.0).flex_shrink(0.0_f32))
}

/// Where a check comes in the menu: what failed first, then what is still
/// running or waiting, then what passed, then the rest, skipped or of no
/// verdict.
fn check_rank(state: perch_github::CheckRunState) -> u8 {
    if state.is_failing() {
        0
    } else if state.is_pending() {
        1
    } else if state.is_passing() {
        2
    } else {
        3
    }
}

/// The page on github.com with every check of the head: the rollup's own
/// link, else the pull request's Checks tab.
fn checks_page(state: ReviewState, checks: Option<&Checks>) -> Option<String> {
    checks
        .and_then(|checks| checks.url.clone())
        .filter(|url| !url.is_empty())
        .or_else(|| {
            state.pull_request.with_untracked(|pull| {
                pull.as_ref()
                    .map(|pull| pull.html_url.trim_end_matches('/'))
                    .filter(|url| !url.is_empty())
                    .map(|url| format!("{url}/checks"))
            })
        })
}

/// The menu the checks' glyph opens: every check, failed first, then running, then
/// passed, each with its glyph, its name and how it stands or how long it
/// took; picking one opens its run on github.com. A long list scrolls in
/// the menu. Last, the checks page on GitHub. Before GitHub has listed
/// the checks, the cached rollup has only its verdict, and the menu says
/// so over the way to GitHub.
fn checks_menu(state: ReviewState) -> Menu {
    let checks = floem::reactive::Effect::untrack(|| crate::review::merge::current_checks(state));
    let page = checks_page(state, checks.as_deref());
    let mut menu = Menu::new().max_height(CHECKS_MENU_HEIGHT);
    match checks.as_deref() {
        Some(checks) if !checks.runs.is_empty() => {
            let mut runs = checks.runs.clone();
            runs.sort_by_key(|check| check_rank(check.state));
            for check in runs {
                let (icon, ink) = crate::review::merge::check_icon(check.state);
                let status = crate::review::merge::check_status_label(&check);
                let url = check
                    .url
                    .clone()
                    .filter(|url| !url.is_empty())
                    .or_else(|| page.clone());
                menu = menu.item(crate::review::merge::check_label(&check), move |item| {
                    item.icon(icon)
                        .icon_tint(ink)
                        .detail(status)
                        .enabled(url.is_some())
                        .action(move || {
                            if let Some(url) = &url {
                                open_in_browser(url);
                            }
                        })
                });
            }
        }
        _ => {
            let loading = state.pull_request_overview_loading.get_untracked();
            let says = if loading {
                "Loading the list of checks…"
            } else {
                "The list of checks hasn't loaded"
            };
            menu = menu.item(says, |item| item.enabled(false));
        }
    }
    if let Some(page) = page {
        // Under the list, in sight however long it runs.
        menu = menu.footer().item("Open checks on GitHub", move |item| {
            item.icon(SidebarIcon::External)
                .action(move || open_in_browser(&page))
        });
    }
    menu
}

/// Each reviewer's standing verdict on the pull request, in the order
/// they first gave one: their last, except that a comment after an
/// approval or a request for changes leaves that standing, as on GitHub.
/// The author's own comments are not reviews of it, and a pending review
/// is not given yet. The Reviewers row draws these.
fn verdicts<'a>(reviews: &'a [perch_github::Review], author: &str) -> Vec<(&'a str, &'a str)> {
    let mut reviews: Vec<_> = reviews
        .iter()
        .filter(|r| r.state != "PENDING" && r.user != author)
        .collect();
    reviews.sort_by_key(|r| r.submitted_at);
    let mut verdicts: Vec<(&str, &str)> = Vec::new();
    for r in reviews {
        match verdicts.iter_mut().find(|(user, _)| *user == r.user) {
            Some(mark) if r.state == "COMMENTED" && mark.1 != "COMMENTED" => {}
            Some(mark) => mark.1 = &r.state,
            None => verdicts.push((&r.user, &r.state)),
        }
    }
    verdicts
}

/// `read`, kept as it was for the same pull request while the overview
/// is read again: the overview is None meanwhile, and nothing should go
/// for the moment of each refresh. The state pill keeps its menu so, and
/// the bar its Merge.
pub(crate) fn kept_while_reading<T: Clone + PartialEq + 'static>(
    state: ReviewState,
    read: impl Fn() -> T + 'static,
) -> Memo<T> {
    let kept = Memo::new(move |prev: Option<&(Option<(String, u64)>, T)>| {
        let pull = state
            .pull_request
            .with(|p| p.as_ref().map(|p| (p.repo_full_name.clone(), p.number)));
        // Only while it is open: one merged or closed meanwhile has
        // nothing left to keep.
        let open = state
            .pull_request
            .with(|p| p.as_ref().is_some_and(|p| p.is_open() && !p.merged));
        let reading = state.pull_request_overview_loading.get()
            && state.pull_request_overview.with(Option::is_none);
        let value = read();
        match prev {
            Some((was, kept)) if reading && open && *was == pull => (pull, kept.clone()),
            _ => (pull, value),
        }
    });
    Memo::new(move |_| kept.with(|(_, value)| value.clone()))
}

/// `base  ←  head`, for a pull request or a checkout.
fn branches_text(state: ReviewState) -> String {
    state
        .pull_request
        .with(|p| {
            p.as_ref()
                .map(|p| format!("{}  \u{2190}  {}", p.base_ref, p.head_ref))
        })
        .or_else(|| {
            state.item.with(|i| {
                i.as_ref()
                    .and_then(|i| i.checkout.as_ref())
                    .map(|c| format!("{}  \u{2190}  {}", short_ref(&c.base_ref), c.branch))
            })
        })
        .unwrap_or_default()
}

fn branches(state: ReviewState) -> impl IntoView {
    Label::derived(move || branches_text(state))
        .debug_name("branches")
        .style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .font_family(theme::CODE_FONT_STACK.to_string())
                .color(theme::fg_muted())
                .background(theme::canvas())
                .border(1.0)
                .border_color(theme::border_muted())
                .border_radius(theme::RADIUS)
                .padding_horiz(8.0)
                .padding_vert(2.0)
                .text_ellipsis()
                .min_width(0.0)
                .max_width(420.0)
                .selectable(false)
        })
}

/// The checkout on disk, if the item has one.
fn checkout_on_disk(state: ReviewState) -> Option<perch_github::ItemCheckout> {
    state.item.with(|item| {
        item.as_ref()
            .and_then(|i| i.checkout.clone())
            .filter(|c| c.on_disk)
    })
}

/// The tip on the worktree's warning: what waits for the reader to trust
/// the code, said once for all of them.
fn held_tip(kinds: &[perch_lsp::ServerKind]) -> String {
    match kinds {
        [] => String::new(),
        [kind] => perch_lsp::Status::Held.describe(*kind),
        _ => {
            let names = kinds
                .iter()
                .map(|kind| kind.name())
                .collect::<Vec<_>>()
                .join(" and ");
            format!("{names} wait for you to trust this code")
        }
    }
}

/// The worktree menu's rows about trusting `item`'s code, with `held` the
/// servers waiting for it: Trust and Start for the item, then, for each
/// server waiting, Always Trust it on everything in the item's repository,
/// and for each one trusted there already, Stop Trusting.
fn trust_rows(
    app: AppState,
    backend: &Rc<Backend>,
    item: &Item,
    held: &[perch_lsp::ServerKind],
) -> Vec<crate::ui::widgets::TrustRow> {
    let mut rows = Vec::new();
    if !held.is_empty() {
        let names = held
            .iter()
            .map(|kind| kind.name())
            .collect::<Vec<_>>()
            .join(" and ");
        let (backend, id) = (backend.clone(), item.id.clone());
        let start =
            Rc::new(move || crate::ui::toast::trust_and_start(app, &backend, &id, Vec::new()));
        rows.push((
            format!("Trust and Start {names}"),
            SidebarIcon::Check,
            start as Rc<dyn Fn()>,
        ));
    }
    let repo = &item.repo_full_name;
    for kind in perch_lsp::ServerKind::ALL {
        let trusted = app.trusts_everywhere(repo, kind);
        if !trusted && !held.contains(&kind) {
            continue;
        }
        let (label, icon) = match trusted {
            true => (
                format!("Stop Trusting {} in {repo}", kind.name()),
                SidebarIcon::Stop,
            ),
            false => (
                format!("Always Trust {} in {repo}", kind.name()),
                SidebarIcon::Repository,
            ),
        };
        let (backend, repo) = (backend.clone(), repo.clone());
        let flip = Rc::new(move || backend.trust_server_in_repo(&repo, kind, !trusted));
        rows.push((label, icon, flip as Rc<dyn Fn()>));
    }
    rows
}

/// The worktree's directory name, and a menu of ways to get there.
///
/// This is what makes the checkout a place rather than a cache: the name
/// on screen is the name in the Finder and in a shell.
///
/// While a language server waits for the reader to trust the code, a
/// warning sits by the name and the menu starts with Trust and Start, and
/// Always Trust for each server waiting. The toast that asked goes once it
/// is closed; this stays while they wait. A server trusted on everything
/// in the repository has Stop Trusting there instead.
fn directory(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // Read by the warning's style, not the container's key: a key on the
    // statuses would rebuild the chip with every one a server sends.
    let held = Memo::new(move |_| state.lsp_status.with(|s| s.held()));
    dyn_container(
        move || checkout_on_disk(state),
        move |checkout| {
            let Some(checkout) = checkout else {
                return Empty::new().into_any();
            };
            let name = checkout
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let path = checkout.path.clone();
            let id = state
                .item
                .with(|i| i.as_ref().map(|i| i.id.clone()).unwrap_or_default());
            // Only a worktree Perch made can be removed from here; one
            // under a root is the reader's. The backend closes the review
            // itself when the removal goes through, and the Items event
            // that follows closes the view. A refusal leaves everything as
            // it was, with the reason in the popup.
            let archive = checkout.owned.then(|| {
                let backend = backend.clone();
                Rc::new(move || {
                    backend.archive_item(&id);
                }) as Rc<dyn Fn()>
            });
            // Read as the menu opens, for the item open then.
            let menu_backend = backend.clone();
            let trust = move || match state.item.get_untracked() {
                Some(item) => trust_rows(
                    app,
                    &menu_backend,
                    &item,
                    &state.lsp_status.with_untracked(|s| s.held()),
                ),
                None => Vec::new(),
            };
            // The notice's caution colour, as the pill over the rows has it.
            let warning = tip_with(
                sidebar_glyph(SidebarIcon::Warning, theme::notice_fg)
                    .style(|s| s.size(12.0, 12.0).pointer_events_none()),
                move || held.with_untracked(|kinds| held_tip(kinds)),
            );

            Stack::horizontal((
                sidebar_glyph(SidebarIcon::Folder, theme::fg_muted).style(|s| {
                    s.size(13.0, 13.0)
                        .margin_right(6.0)
                        .flex_shrink(0.0_f32)
                        .pointer_events_none()
                }),
                Label::derived(move || name.clone()).style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .font_family(theme::CODE_FONT_STACK.to_string())
                        .color(theme::fg_muted())
                        .text_ellipsis()
                        .min_width(0.0)
                        .max_width(320.0)
                        .pointer_events_none()
                }),
                Container::new(warning)
                    .debug_name("untrusted")
                    .style(move |s| {
                        s.margin_left(6.0)
                            .flex_shrink(0.0_f32)
                            .apply_if(held.with(Vec::is_empty), |s| s.hide())
                    }),
                sidebar_glyph(SidebarIcon::ChevronDown, theme::fg_subtle).style(|s| {
                    s.size(10.0, 10.0)
                        .margin_left(5.0)
                        .flex_shrink(0.0_f32)
                        .pointer_events_none()
                }),
            ))
            .dropdown(move || worktree_menu(path.clone(), archive.clone(), trust()))
            .debug_name("worktree menu")
            .style(|s| {
                s.items_center()
                    .height(22.0)
                    .padding_horiz(8.0)
                    .background(theme::canvas())
                    .border(1.0)
                    .border_color(theme::border_muted())
                    .border_radius(theme::RADIUS)
                    .min_width(0.0)
                    .selectable(false)
                    .hover(|s| {
                        s.background(theme::canvas_hover())
                            .cursor(CursorStyle::Pointer)
                    })
            })
            .into_any()
        },
    )
    .style(|s| s.min_width(0.0))
}

/// Lines added and removed on the branch, from the pull request or the
/// checkout.
fn churn_of(state: ReviewState) -> Option<(u32, u32)> {
    state
        .pull_request
        .with(|p| p.as_ref().map(|p| (p.additions, p.deletions)))
        .or_else(|| {
            state.item.with(|i| {
                i.as_ref()
                    .and_then(|i| i.checkout.as_ref())
                    .and_then(|c| c.churn)
            })
        })
}

fn churn(state: ReviewState) -> impl IntoView {
    let has_churn = Memo::new(move |_| churn_of(state).is_some());
    dyn_container(
        move || has_churn.get(),
        move |has| {
            if has {
                churn_labels(
                    move || churn_of(state).map(|c| c.0).unwrap_or(0),
                    move || churn_of(state).map(|c| c.1).unwrap_or(0),
                )
                .into_any()
            } else {
                Empty::new().into_any()
            }
        },
    )
    .style(move |s| {
        s.flex_shrink(0.0_f32)
            .apply_if(!has_churn.get(), |s| s.hide())
    })
}

/// How the worktree stands against the pull request when they're at
/// different commits: `3 unpushed`, `2 behind PR`, both, or that the
/// pull request's head isn't here. Absent when they match.
fn divergence_of(state: ReviewState) -> Option<String> {
    state
        .item
        .with(|i| i.as_ref().and_then(|i| i.divergence()).map(|d| d.label()))
}

fn divergence(state: ReviewState) -> impl IntoView {
    Label::derived(move || divergence_of(state).unwrap_or_default())
        .debug_name("divergence")
        .style(move |s| {
            s.font_size(theme::TEXT_BADGE)
                .font_weight(theme::weight_emphasis())
                .color(theme::draft_yellow())
                .flex_shrink(0.0_f32)
                .selectable(false)
                .apply_if(divergence_of(state).is_none(), |s| s.hide())
        })
}

/// Whether the worktree has commits a pull request the reader may push to
/// lacks, and nothing the pull request has that the worktree doesn't: a
/// push that needs no force.
pub(crate) fn can_push(app: AppState, state: ReviewState) -> bool {
    let viewer = app
        .viewer
        .with(|v| v.as_ref().map(|(login, _)| login.clone()));
    state.item.with(|item| {
        item.as_ref().is_some_and(|item| {
            state.pull_request_overview.with(|overview| {
                crate::items::may_push_pull(item, viewer.as_deref(), overview.as_deref())
            }) && item
                .divergence()
                .is_some_and(|d| d.ahead.is_some_and(|a| a > 0) && d.behind == Some(0))
        })
    })
}

/// Push the item's branch to its pull request, as Push does here and in
/// the merge box.
pub(crate) fn push(state: ReviewState, backend: &Backend) {
    let Some(item_id) = state
        .item
        .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
    else {
        return;
    };
    state.push_pending.set(true);
    state.push_error.set(None);
    backend.push_branch(&item_id);
}

/// Whether Update branch can bring the worktree to its pull request: the
/// worktree is on the pull request's branch and has nothing the pull
/// request lacks, or the pull request's head isn't here to tell.
pub(crate) fn can_update(state: ReviewState) -> bool {
    state.item.with(|item| {
        item.as_ref().is_some_and(|item| {
            on_pull_branch(item)
                && item
                    .divergence()
                    .is_some_and(|d| d.ahead.is_none_or(|a| a == 0))
        })
    })
}

/// Whether Rebase can bring the worktree and its pull request together:
/// the worktree is on the pull request's branch and each has commits the
/// other lacks, so neither Push nor Update branch can.
pub(crate) fn can_rebase(state: ReviewState) -> bool {
    state.item.with(|item| {
        item.as_ref().is_some_and(|item| {
            on_pull_branch(item)
                && item.divergence().is_some_and(|d| {
                    d.ahead.is_some_and(|a| a > 0) && d.behind.is_some_and(|b| b > 0)
                })
        })
    })
}

/// Whether the item's worktree is on disk and on its pull request's branch.
fn on_pull_branch(item: &Item) -> bool {
    item.checkout
        .as_ref()
        .zip(item.pull_request())
        .is_some_and(|(c, pr)| c.on_disk && c.branch == pr.head_ref)
}

/// Fetch the pull request's head into the worktree and fast-forward to
/// it, as Update branch does here and in the merge box.
pub(crate) fn update(state: ReviewState, backend: &Backend) {
    let Some(item_id) = state
        .item
        .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
    else {
        return;
    };
    state.update_pending.set(true);
    state.update_error.set(None);
    backend.update_branch(&item_id);
}

/// Replay the worktree's own commits on the pull request's head, as
/// Rebase does here, in the notice and in the merge box. Nothing is
/// pushed: Push does that after.
pub(crate) fn rebase(state: ReviewState, backend: &Backend) {
    let Some(item_id) = state
        .item
        .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
    else {
        return;
    };
    state.rebase_pending.set(true);
    state.update_error.set(None);
    backend.rebase_branch(&item_id);
}

/// Push, beside `3 unpushed`, and what stopped the last one.
fn push_button(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    branch_button(
        state,
        ("Push", "Pushing…", "push-branch"),
        state.push_pending,
        state.push_error,
        move || can_push(app, state),
        move || push(state, &backend),
    )
}

/// Update branch, beside `PR head not fetched` or `2 behind PR`, and what
/// stopped the last one.
fn update_button(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    branch_button(
        state,
        ("Update branch", "Updating…", "update-branch"),
        state.update_pending,
        state.update_error,
        move || can_update(state),
        move || update(state, &backend),
    )
}

/// Rebase, beside `1 unpushed · 12 behind PR`, and what stopped the last
/// one.
fn rebase_button(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    branch_button(
        state,
        ("Rebase", "Rebasing…", "rebase-branch"),
        state.rebase_pending,
        state.update_error,
        move || can_rebase(state),
        move || rebase(state, &backend),
    )
}

/// A button on the Branch row that brings the worktree and the pull
/// request together, shown while it can or while it's at it, and the
/// error it last ended in.
fn branch_button(
    state: ReviewState,
    (label, pending_label, id): (&'static str, &'static str, &'static str),
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    can: impl Fn() -> bool + 'static,
    act: impl Fn() + 'static,
) -> impl IntoView {
    let shown = Memo::new(move |_| can() || pending.get());
    Stack::horizontal((
        crate::ui::widgets::action_button(
            move || {
                if pending.get() {
                    pending_label.to_string()
                } else {
                    label.to_string()
                }
            },
            id,
            false,
            move || !pending.get() && !state.offline.get(),
            act,
        )
        .style(|s| s.height(22.0).padding_horiz(8.0)),
        Label::derived(move || error.get().unwrap_or_default())
            .debug_name(format!("{id}-error"))
            .style(move |s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::closed())
                    .min_width(0.0)
                    .text_ellipsis()
                    .apply_if(error.with(Option::is_none), |s| s.hide())
            }),
    ))
    .style(move |s| {
        s.items_center()
            .gap(6.0)
            .min_width(0.0)
            .apply_if(!shown.get(), |s| s.hide())
    })
}

fn churn_labels(
    additions: impl Fn() -> u32 + 'static,
    deletions: impl Fn() -> u32 + 'static,
) -> impl IntoView {
    Stack::horizontal((
        Label::derived(move || format!("+{}", additions())).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .font_weight(theme::weight_emphasis())
                .color(theme::status_added())
        }),
        Label::derived(move || format!("-{}", deletions())).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .font_weight(theme::weight_emphasis())
                .color(theme::status_deleted())
                .margin_left(6.0)
        }),
    ))
    .debug_name("churn")
    .style(|s| s.items_center().flex_shrink(0.0_f32).selectable(false))
}

// ---------------------------------------------------------------------
// The notices over the rows
// ---------------------------------------------------------------------

/// The way back from a retained snapshot, and the notice, on one line
/// over the rows. Hidden when there is neither, so the rows start where
/// the title's gap ends.
fn notices(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let present =
        move || state.historical_review.with(Option::is_some) || notice_text(state).is_some();
    Stack::horizontal((
        snapshot_banner(app, state, backend.clone()),
        notice(app, state, backend),
    ))
    .debug_name("summary notices")
    .style(move |s| {
        s.items_center()
            .width_full()
            .min_width(0.0)
            .col_gap(theme::SUMMARY_GAP)
            .height(theme::SUMMARY_ROW)
            .apply_if(!present(), |s| s.hide())
    })
}

/// `Review #N snapshot · read-only`, and Back to current beside it. Over
/// the rows on Conversation, and in the tab bar on Code.
pub fn snapshot_banner(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    dyn_container(
        move || state.historical_review.get(),
        move |review| match review {
            Some(review) => Stack::horizontal((
                Label::derived(move || {
                    format!("Review #{} snapshot · read-only", review.group.number)
                })
                .style(|s| {
                    s.font_size(theme::TEXT_META)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::fg_muted())
                        .selectable(false)
                }),
                back_to_current(app, state, backend.clone()),
            ))
            .debug_name("snapshot banner")
            .style(|s| {
                s.height(24.0)
                    .items_center()
                    .gap(7.0)
                    .padding_horiz(8.0)
                    .background(theme::badge_bg())
                    .border_radius(theme::RADIUS)
            })
            .into_any(),
            None => Empty::new().into_any(),
        },
    )
    .style(move |s| {
        s.flex_shrink(0.0_f32)
            .apply_if(state.historical_review.with(Option::is_none), |s| s.hide())
    })
}

/// Back to current: the item as it is now, in place of the snapshot.
pub fn back_to_current(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    Label::derived(|| "Back to current".to_string())
        .on_event_stop(el::Click, move |_, _| {
            if let Some(item_id) = state
                .item
                .with_untracked(|item| item.as_ref().map(|item| item.id.clone()))
            {
                let navigation_id = backend.open_item(&item_id);
                app.begin_item_review(state, "Reading the current files", &item_id, navigation_id);
            }
        })
        .debug_name("back to current")
        .style(|s| {
            clickable(s)
                .height(20.0)
                .items_center()
                .padding_horiz(6.0)
                .font_size(theme::TEXT_BADGE)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
                .background(theme::canvas())
                .border_radius(theme::RADIUS)
                .flex_shrink(0.0_f32)
                .selectable(false)
        })
}

/// Why the files on screen may not be the pull request's latest.
///
/// Three cases, one shape: the pull request moved while the worktree had
/// work in it, it moved while the worktree had commits of its own (the
/// two diverged), or it could not be fetched at all. Diverged offers
/// Rebase; the others offer Check again, which opens the item as a click
/// on its row does: once the work is pushed or discarded, or GitHub
/// answers, that catches up.
///
/// A commit, the index or a run on Code is no case: the language servers
/// are given each file as it shows.
fn notice_text(state: ReviewState) -> Option<String> {
    notice_lines(state).map(|(text, _, _)| text)
}

/// What the notice's button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoticeAction {
    CheckAgain,
    Rebase,
}

/// The notice, the longer account its tip gives on hover, and its
/// button. The notice is cut from the end, so it leads with what
/// happened.
fn notice_lines(state: ReviewState) -> Option<(String, String, NoticeAction)> {
    if let Some(head) = state.stale_head.get() {
        let sha = crate::review::timeline::short_sha(&head);
        let counts = state.item.with(|i| {
            i.as_ref()
                .and_then(|i| i.divergence())
                .and_then(|d| d.ahead.zip(d.behind))
        });
        if let Some((ahead, behind)) = counts.filter(|_| can_rebase(state)) {
            let commits = |n: u32| {
                if n == 1 {
                    "1 commit".to_string()
                } else {
                    format!("{n} commits")
                }
            };
            return Some((
                format!("Not updated to {sha}: branch has diverged"),
                format!(
                    "The pull request has {} this worktree lacks, up to {sha}, \
                     and the worktree has {} the pull request lacks. Neither \
                     can simply move to the other. Rebase puts the worktree's \
                     {} on top of {sha}, here in the worktree; Push then \
                     sends {}.",
                    commits(behind),
                    commits(ahead),
                    if ahead == 1 { "commit" } else { "commits" },
                    if ahead == 1 { "it" } else { "them" },
                ),
                NoticeAction::Rebase,
            ));
        }
        return Some((
            format!("Not updated to {sha}: worktree has local work"),
            format!(
                "New commits were pushed to this pull request, up to {sha}. \
                 Perch left them out because the worktree has uncommitted \
                 changes or commits of its own, and checking them out would \
                 lose that work. The files here are the worktree as it is. \
                 To get the new commits, push or discard that work, then \
                 click Check again."
            ),
            NoticeAction::CheckAgain,
        ));
    }
    if state.offline.get() {
        return Some((
            "Offline; showing the checkout as it was last left".to_string(),
            "GitHub could not be reached, so the pull request was not \
             fetched. The files here are the worktree as it was last left, \
             and may be behind the pull request. Check again asks GitHub \
             again."
                .to_string(),
            NoticeAction::CheckAgain,
        ));
    }
    None
}

/// The notice as a pill, over the rows on Conversation and in the tab
/// bar on Code. It gives way, as the branches and the worktree name do:
/// it is a sentence, and its start says what it is about. Check again
/// does not give way. Hidden when there is nothing to say.
pub fn notice(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // It reads the item, which is written far more often than it changes.
    let lines = Memo::new(move |_| notice_lines(state));
    dyn_container(
        move || lines.get(),
        move |lines| {
            let Some((text, why, action)) = lines else {
                return Empty::new().into_any();
            };
            let words = Stack::horizontal((
                sidebar_glyph(SidebarIcon::Warning, theme::notice_fg)
                    .style(|s| s.size(13.0, 13.0).margin_right(6.0).flex_shrink(0.0_f32)),
                Label::derived(move || text.clone()).style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::notice_fg())
                        .text_ellipsis()
                        .min_width(0.0)
                }),
            ))
            .style(|s| s.items_center().min_width(0.0));
            // The tip wraps the words, so it gives way too.
            let words = tip(words, why).style(|s| s.min_width(0.0).flex_shrink(1.0_f32));
            let button = match action {
                NoticeAction::CheckAgain => check_again(app, state, backend.clone()).into_any(),
                NoticeAction::Rebase => rebase_in_notice(state, backend.clone()).into_any(),
            };
            Stack::horizontal((words, button))
                .style(|s| {
                    s.items_center()
                        .height(22.0)
                        .padding_left(9.0)
                        .padding_right(2.0)
                        .max_width(460.0)
                        .min_width(0.0)
                        .background(theme::notice_bg())
                        .border(1.0)
                        .border_color(theme::notice_border())
                        .border_radius(theme::RADIUS)
                        .selectable(false)
                })
                .into_any()
        },
    )
    .debug_name("item notice")
    .style(move |s| {
        s.min_width(0.0)
            .flex_shrink(1.0_f32)
            .apply_if(notice_text(state).is_none(), |s| s.hide())
    })
}

/// Rebase, in the notice of a diverged branch: as Rebase on the Branch
/// row, which shows what stopped it.
fn rebase_in_notice(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    notice_button(
        move || {
            if state.rebase_pending.get() {
                "Rebasing…"
            } else {
                "Rebase"
            }
            .to_string()
        },
        move || {
            if !state.rebase_pending.get_untracked() && !state.offline.get_untracked() {
                rebase(state, &backend);
            }
        },
    )
    .debug_name("rebase in notice")
}

/// Check again: the item opened again, as a click on its row does, which
/// fetches the pull request and moves a clean worktree to its head.
fn check_again(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    notice_button(
        || "Check again".to_string(),
        move || {
            if let Some(item_id) = state
                .item
                .with_untracked(|item| item.as_ref().map(|item| item.id.clone()))
            {
                let navigation_id = backend.open_item(&item_id);
                app.begin_item_review(state, "Reading the pull request", &item_id, navigation_id);
            }
        },
    )
    .debug_name("check again")
}

/// The notice's one button, at the end of the pill.
fn notice_button(label: impl Fn() -> String + 'static, act: impl Fn() + 'static) -> Label {
    Label::derived(label)
        .on_event_stop(el::Click, move |_, _| act())
        .style(|s| {
            clickable(s)
                .height(18.0)
                .items_center()
                .margin_left(8.0)
                .padding_horiz(6.0)
                .font_size(theme::TEXT_BADGE)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
                .background(theme::canvas())
                .border_radius(theme::RADIUS)
                .flex_shrink(0.0_f32)
                .selectable(false)
        })
}

// ---------------------------------------------------------------------
// What GitHub says
// ---------------------------------------------------------------------

/// What GitHub says about the item, read once from the signals so the
/// rows rebuild when any of it changes and not when the signals are
/// merely written.
#[derive(Debug, Clone, PartialEq, Default)]
struct GitHubData {
    /// Who opened it, their avatar, and when, once the discussion says.
    author: Option<(String, String, Option<jiff::Timestamp>)>,
    /// Login, avatar, and the state of their latest review if they have
    /// given one.
    reviewers: Vec<(String, String, Option<String>)>,
    labels: Vec<perch_github::Label>,
    assignees: Vec<String>,
    milestone: Option<String>,
    /// The issues a pull request's description says it closes.
    closes: Vec<u64>,
    /// The branches and pull requests Perch started from this item.
    in_perch: Vec<Item>,
}

impl GitHubData {
    /// Whether GitHub says anything the details have a row for.
    fn has_details(&self) -> bool {
        !(self.reviewers.is_empty()
            && self.labels.is_empty()
            && self.assignees.is_empty()
            && self.milestone.is_none()
            && self.closes.is_empty()
            && self.in_perch.is_empty())
    }
}

fn github_data(app: AppState, state: ReviewState) -> GitHubData {
    let mut data = GitHubData::default();
    if let Some(pr) = state.pull_request.get() {
        let opened = state
            .discussion
            .with(|d| d.as_ref().and_then(|d| d.opened_at));
        data.author = Some((
            pr.author_login.clone(),
            pr.author_avatar_url.clone(),
            opened,
        ))
        .filter(|(login, ..)| !login.is_empty());
        for r in &pr.requested_reviewers {
            data.reviewers
                .push((r.login.clone(), r.avatar_url.clone(), None));
        }
        state.discussion.with(|d| {
            let Some(d) = d else { return };
            for (user, verdict) in verdicts(&d.reviews, &pr.author_login) {
                match data.reviewers.iter_mut().find(|(login, ..)| login == user) {
                    Some(mark) => mark.2 = Some(verdict.to_string()),
                    None => data.reviewers.push((
                        user.to_string(),
                        avatar_url_for(state, user),
                        Some(verdict.to_string()),
                    )),
                }
            }
            data.closes = closes(&d.body);
        });
        data.milestone = pr.milestone.clone();
        data.labels = pr.labels.clone();
        data.assignees = pr.assignees.clone();
    }
    if let Some(issue) = state.issue.get() {
        data.author = Some((
            issue.author_login.clone(),
            issue.author_avatar_url.clone(),
            issue.created_at,
        ))
        .filter(|(login, ..)| !login.is_empty());
        data.labels = issue.labels.clone();
        data.assignees = issue.assignees.clone();
    }
    let id = state.item.with(|i| i.as_ref().map(|i| i.id.clone()));
    data.in_perch = app.items.with(|all| {
        all.iter()
            .filter(|i| i.from_item_id.is_some() && i.from_item_id == id)
            .cloned()
            .collect()
    });
    data
}

/// The issue numbers a description says it closes: `closes #12`, `Fixes
/// #3`, `resolved #7`, in any case, anywhere in the text. Only this
/// repository's; `owner/repo#N` names another and is left alone.
pub(crate) fn closes(body: &str) -> Vec<u64> {
    const WORDS: [&str; 9] = [
        "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
    ];
    let mut out = Vec::new();
    let mut words = body.split_whitespace().peekable();
    while let Some(word) = words.next() {
        let word = word.trim_matches(|c: char| !c.is_alphanumeric());
        if !WORDS.iter().any(|w| word.eq_ignore_ascii_case(w)) {
            continue;
        }
        let Some(next) = words.peek() else { break };
        let Some(number) = next.strip_prefix('#') else {
            continue;
        };
        let digits: String = number.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse::<u64>() {
            if !out.contains(&n) {
                out.push(n);
            }
            words.next();
        }
    }
    out
}

/// Avatar, login, and when it was opened, as the reviewers are drawn.
fn author_view(
    app: AppState,
    (login, url, opened): (String, String, Option<jiff::Timestamp>),
) -> impl IntoView {
    let seed = login.clone();
    // A memo, so the label is set again only when its text changes, not
    // on every tick of the clock.
    let opened_text = Memo::new(move |_| {
        opened
            .map(|at| format!("opened {}", timefmt::relative(at, app.now.get())))
            .unwrap_or_default()
    });
    Stack::horizontal((
        avatar(app, move || url.clone(), theme::SUMMARY_AVATAR, seed)
            .style(|s| s.margin_right(5.0)),
        name_label(
            move || Name::Login(login.clone()),
            |s| s.font_size(theme::TEXT_BADGE).color(theme::fg_muted()),
        ),
        Label::derived(move || opened_text.get())
            .debug_name("opened")
            .style(move |s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .margin_left(6.0)
                    .selectable(false)
                    .apply_if(opened.is_none(), |s| s.hide())
            }),
    ))
    .debug_name("author")
    .style(|s| s.items_center().min_width(0.0))
}

/// Avatar, login, and the verdict's glyph after it for a review given.
fn reviewers_view(
    app: AppState,
    reviewers: Vec<(String, String, Option<String>)>,
) -> impl IntoView {
    dyn_stack(
        move || reviewers.clone(),
        |(login, ..)| login.clone(),
        move |(login, url, verdict)| {
            let seed = login.clone();
            Stack::horizontal((
                avatar(app, move || url.clone(), theme::SUMMARY_AVATAR, seed)
                    .style(|s| s.margin_right(5.0)),
                name_label(
                    move || Name::Login(login.clone()),
                    |s| s.font_size(theme::TEXT_BADGE).color(theme::fg_muted()),
                ),
                match verdict {
                    Some(state) => {
                        let review = perch_github::Review {
                            viewer_can_update: false,
                            node_id: String::new(),
                            reactions: Default::default(),
                            id: 0,
                            user: String::new(),
                            user_avatar_url: String::new(),
                            body: String::new(),
                            state,
                            submitted_at: None,
                            html_url: String::new(),
                        };
                        let (icon, color) = verdict_glyph(&review);
                        sidebar_glyph(icon, color)
                            .style(|s| s.size(12.0, 12.0).margin_left(4.0))
                            .into_any()
                    }
                    None => Empty::new().into_any(),
                },
            ))
            .style(|s| s.items_center())
        },
    )
    .style(|s| {
        s.items_center()
            .flex_wrap(floem::taffy::FlexWrap::Wrap)
            .gap(10.0)
    })
}

/// Avatar and login of each assignee. GitHub names an assignee by login
/// alone, so the face is whichever one the item already has for them: as
/// its author, a reviewer or a commenter, or in the Assign menu.
fn assignees(
    app: AppState,
    state: ReviewState,
    logins: Vec<String>,
    assignable: Memo<Option<Arc<Vec<MentionableUser>>>>,
) -> impl IntoView {
    dyn_stack(
        move || logins.clone(),
        |l| l.clone(),
        move |login| {
            let seed = login.clone();
            let face = login.clone();
            let url = move || {
                // Read untracked there; a comment's face may come later.
                state.discussion.track();
                state.issue_comments.track();
                Some(avatar_url_for(state, &face))
                    .filter(|u| !u.is_empty())
                    .or_else(|| {
                        assignable.with(|users| {
                            users
                                .as_ref()?
                                .iter()
                                .find(|u| u.login == face)
                                .map(|u| u.avatar_url.clone())
                        })
                    })
                    .unwrap_or_default()
            };
            Stack::horizontal((
                avatar(app, url, theme::SUMMARY_AVATAR, seed).style(|s| s.margin_right(5.0)),
                name_label(
                    move || Name::Login(login.clone()),
                    |s| s.font_size(theme::TEXT_BADGE).color(theme::fg_muted()),
                ),
            ))
            .style(|s| s.items_center())
        },
    )
    .style(|s| {
        s.items_center()
            .flex_wrap(floem::taffy::FlexWrap::Wrap)
            .gap(10.0)
    })
}

/// Who the open issue or pull request is assigned to.
fn assignees_of(state: ReviewState) -> Vec<String> {
    state
        .pull_request
        .with_untracked(|p| p.as_ref().map(|p| p.assignees.clone()))
        .or_else(|| {
            state
                .issue
                .with_untracked(|i| i.as_ref().map(|i| i.assignees.clone()))
        })
        .unwrap_or_default()
}

/// Assign, after the assignees: a menu of who can be.
fn assign_button(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    assignable: Memo<Option<Arc<Vec<MentionableUser>>>>,
) -> impl IntoView {
    Stack::horizontal((
        Label::derived(|| "Assign".to_string()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg_muted())
                .pointer_events_none()
        }),
        sidebar_glyph(SidebarIcon::ChevronDown, theme::fg_subtle).style(|s| {
            s.size(10.0, 10.0)
                .margin_left(5.0)
                .flex_shrink(0.0_f32)
                .pointer_events_none()
        }),
    ))
    .dropdown(move || {
        let users = assignable.get_untracked().unwrap_or_default();
        let viewer = app
            .viewer
            .with_untracked(|v| v.as_ref().map(|(login, _)| login.clone()));
        let item_id = state
            .item
            .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
            .unwrap_or_default();
        let backend = backend.clone();
        assign_menu(
            viewer.as_deref(),
            &assignees_of(state),
            &users,
            Rc::new(move |login: String, assign: bool| {
                backend.set_assigned(&item_id, &login, assign);
            }),
        )
    })
    .debug_name("assign menu")
    .style(|s| {
        s.items_center()
            .height(22.0)
            .padding_horiz(8.0)
            .background(theme::canvas())
            .border(1.0)
            .border_color(theme::border_muted())
            .border_radius(theme::RADIUS)
            .flex_shrink(0.0_f32)
            .selectable(false)
            .hover(|s| {
                s.background(theme::canvas_hover())
                    .cursor(CursorStyle::Pointer)
            })
    })
}

/// The Assign menu: the reader first, then whoever is assigned, then the
/// rest of who may be, in GitHub's order (whoever took part in the item,
/// then the repository's assignable users), each
/// checked while assigned and named after their login. Picking one
/// assigns them or takes them off and leaves the menu up for the next,
/// and the box over them narrows them by login or name. GitHub assigns
/// ten at most, so with ten the rest are greyed.
fn assign_menu(
    viewer: Option<&str>,
    assigned: &[String],
    users: &[MentionableUser],
    set: Rc<dyn Fn(String, bool)>,
) -> Menu {
    let name_of = |login: &str| {
        users
            .iter()
            .find(|u| u.login.eq_ignore_ascii_case(login))
            .and_then(|u| u.name.clone())
    };
    let mut people: Vec<(String, Option<String>)> = Vec::new();
    let candidates = viewer
        .into_iter()
        .chain(assigned.iter().map(String::as_str))
        .chain(users.iter().map(|u| u.login.as_str()));
    for login in candidates {
        if !people.iter().any(|(l, _)| l.eq_ignore_ascii_case(login)) {
            people.push((login.to_string(), name_of(login)));
        }
    }
    let menu = Menu::new()
        .filter("Filter assignees")
        .most_checked(perch_github::MAX_ASSIGNEES);
    people.into_iter().fold(menu, |menu, (login, name)| {
        let on = assigned.iter().any(|a| a.eq_ignore_ascii_case(&login));
        let set = set.clone();
        menu.item(login.clone(), move |item| {
            let item = item
                .checked(on)
                .toggle(move |assign| set(login.clone(), assign));
            match name {
                Some(name) => item.detail(name),
                None => item,
            }
        })
    })
}

fn milestone(name: String) -> impl IntoView {
    Label::derived(move || name.clone()).style(|s| {
        s.font_size(theme::TEXT_BADGE)
            .color(theme::fg_muted())
            .text_ellipsis()
            .max_width(320.0)
    })
}

/// A row of chips, one per item.
fn chips<T: Clone + std::hash::Hash + Eq + 'static, V: IntoView + 'static>(
    items: Vec<T>,
    chip: impl Fn(T) -> V + 'static,
) -> impl IntoView {
    dyn_stack(move || items.clone(), |i| i.clone(), chip).style(|s| {
        s.items_center()
            .flex_wrap(floem::taffy::FlexWrap::Wrap)
            .gap(4.0)
    })
}

/// A small pill, clickable when it leads somewhere.
fn chip(
    text: String,
    tint: impl theme::Ink,
    ink: impl theme::Ink,
    on_click: Option<Rc<dyn Fn()>>,
) -> impl IntoView {
    let clicks = on_click.is_some();
    Container::new(
        Label::derived(move || text.clone())
            .style(move |s| badge_text(s, ink()).text_ellipsis().max_width(200.0)),
    )
    .on_event_stop(el::Click, move |_, _| {
        if let Some(on_click) = &on_click {
            on_click();
        }
    })
    .style(move |s| {
        let base = badge_box(s, tint(), None);
        if clicks {
            clickable(base).hover(|s| s.background(theme::border_muted()))
        } else {
            base.selectable(false)
        }
    })
}

/// A grant: what it allows, and a × that takes it back.
fn grant_chip(backend: Rc<Backend>, id: String, label: String) -> impl IntoView {
    Stack::horizontal((
        Label::derived(move || label.clone()).style(|s| {
            badge_text(s, theme::fg_muted())
                .text_ellipsis()
                .max_width(220.0)
        }),
        sidebar_glyph(SidebarIcon::Close, theme::fg_subtle)
            .on_event_stop(el::Click, move |_, _| backend.revoke_grant(&id))
            .debug_name("revoke-grant")
            .style(|s| {
                clickable(s)
                    .size(12.0, 12.0)
                    .margin_left(5.0)
                    .flex_shrink(0.0_f32)
                    .hover(|s| s.color(theme::fg()))
            }),
    ))
    .debug_name("grant-chip")
    .style(|s| badge_box(s, theme::badge_bg(), None))
}

/// A label as GitHub draws it: its colour as a tint behind darker text.
/// Shared with the timeline's label lines, which draw the same pill.
pub(crate) fn label_chip(name: String, color: String) -> impl IntoView {
    let rgb = label_rgb(&color);
    chip(
        name,
        move || label_colors(rgb).0,
        move || label_colors(rgb).1,
        None,
    )
}

/// A label's colour from GitHub's hex for it, or None when it is not one.
fn label_rgb(hex: &str) -> Option<[u8; 3]> {
    if hex.len() != 6 {
        return None;
    }
    let parse = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some([parse(0)?, parse(2)?, parse(4)?])
}

/// GitHub's label colour: a wash of it behind, and a cut of it for the
/// text, so every label reads on the canvas. On light, a light wash and a
/// darker cut; on dark, a dark wash and a lighter one, as GitHub's dark
/// theme draws them.
fn label_colors(rgb: Option<[u8; 3]>) -> (floem::peniko::Color, floem::peniko::Color) {
    let Some([r, g, b]) = rgb else {
        return (theme::badge_bg(), theme::fg_muted());
    };
    let [r, g, b] = [r, g, b].map(f32::from);
    let dark = theme::appearance() == theme::Appearance::Dark;
    // Toward the canvas for the wash, away from it for the text.
    let wash = |c: f32| {
        let canvas = if dark { 24.0 } else { 255.0 };
        (canvas + (c - canvas) * 0.18) as u8
    };
    let ink = |c: f32| {
        if dark {
            (c + (255.0 - c) * 0.45) as u8
        } else {
            (c * 0.55) as u8
        }
    };
    (
        floem::peniko::Color::from_rgb8(wash(r), wash(g), wash(b)),
        floem::peniko::Color::from_rgb8(ink(r), ink(g), ink(b)),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use std::sync::Arc;

    use floem::kurbo::Rect;

    use crate::review::header::tests::{
        issue_harness, named, open_details, pull_request, pull_request_harness,
        pull_request_harness_with, rect, within,
    };
    use crate::review::model::ItemTab;

    fn branch_item(repo: &str) -> Item {
        let mut item = Item::new(repo, "2026-09-18T09:59:00Z".parse().unwrap());
        item.checkout = Some(perch_github::ItemCheckout {
            branch: "fence".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: "/w/fence".into(),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
            divergence: None,
            source: None,
        });
        item
    }

    /// The rows on screen, top to bottom, by name.
    fn rows_shown(harness: &floem::headless::HeadlessHarness) -> Vec<String> {
        fn walk(id: floem::ViewId, out: &mut Vec<(f64, String)>) {
            let name = id.debug_name();
            if let Some(row) = name.split("summary row ").nth(1) {
                let r = id.get_visual_rect_no_clip();
                if r.height() > 0.0 {
                    out.push((r.y0, row.split(" - ").next().unwrap_or(row).to_string()));
                }
            }
            for child in id.children() {
                walk(child, out);
            }
        }
        let mut rows = Vec::new();
        walk(harness.root_id(), &mut rows);
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));
        rows.into_iter().map(|(_, name)| name).collect()
    }

    #[test]
    fn the_summary_tops_the_page_in_a_line_and_only_on_conversation() {
        let (mut harness, _, state) = pull_request_harness();
        // With no notice over it, the description heads the page, and
        // the line is its strip.
        let column = rect(&harness, "conversation column");
        let summary = rect(&harness, "card:description");
        assert_eq!(
            (summary.x0, summary.y0),
            (column.x0, column.y0),
            "the description at {summary:?} is not at the top of the page {column:?}"
        );
        assert_eq!(rect(&harness, "item summary").height(), 0.0);
        // A pull request with a checkout and passing checks: the pill,
        // the number, who opened it, the churn and the checks, on one
        // line. The rest is in the bar's details.
        assert!(rows_shown(&harness).is_empty());
        let line = rect(&harness, "summary line");
        assert_eq!(line.height(), theme::SUMMARY_ROW, "the line at {line:?}");
        assert!(within(line, summary));
        let mut last = line.x0;
        for part in [
            "state pill",
            "repository line",
            "author",
            "churn",
            "checks status",
        ] {
            let r = rect(&harness, part);
            assert!(
                within(r, line),
                "{part} at {r:?} is not on the line {line:?}"
            );
            assert!(
                r.x0 >= last,
                "{part} at {r:?} is not after what is before it"
            );
            assert!(
                (r.center().y - line.center().y).abs() < 0.01,
                "{part} at {r:?} is not centred on the line {line:?}"
            );
            last = r.x1;
        }
        assert_eq!(rect(&harness, "state pill").x0, line.x0);

        // The details have the branches and the worktree, each a row a
        // line tall, its name in the column at the left and its value
        // after the column.
        open_details(&mut harness, state);
        assert_eq!(rows_shown(&harness), ["Branch", "Worktree"]);
        let details = rect(&harness, "item details");
        let mut last: Option<Rect> = None;
        for (row, value) in [("Branch", "branches"), ("Worktree", "worktree menu")] {
            let r = rect(&harness, &format!("summary row {row}"));
            assert_eq!(r.height(), theme::SUMMARY_ROW, "{row} at {r:?}");
            assert!(within(r, details), "{row} at {r:?} is outside {details:?}");
            if let Some(last) = last {
                assert_eq!(r.y0, last.y1, "{row} at {r:?} does not follow {last:?}");
            }
            last = Some(r);
            let v = rect(&harness, value);
            assert!(within(v, r), "{value} at {v:?} is not in its row {r:?}");
            assert_eq!(
                v.x0,
                r.x0 + theme::SUMMARY_NAME_WIDTH,
                "{value} at {v:?} is not after the name column"
            );
        }
        crate::ui::agents::dropdown::close(state, false);

        // Files has none of it: the page is Conversation's alone.
        state.tab.set(ItemTab::Code);
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "item summary").is_none());
        assert!(named(harness.root_id(), "summary line").is_none());
        assert!(named(harness.root_id(), "branches").is_none());
    }

    #[test]
    fn what_github_says_is_rows_of_its_own_and_a_long_value_wraps_under_its_name() {
        let mut pull = pull_request(812);
        // Letters, so no name is found inside another's.
        pull.requested_reviewers = ('a'..='l')
            .map(|letter| perch_github::Reviewer {
                login: format!("reviewer-with-a-long-name-{letter}"),
                avatar_url: String::new(),
            })
            .collect();
        pull.labels = ["enhancement", "tracing", "needs-review"]
            .into_iter()
            .map(|name| perch_github::Label {
                name: name.into(),
                color: "0e8a16".into(),
            })
            .collect();
        pull.assignees = vec!["cadonna".into()];
        pull.milestone = Some("v1.0".into());
        let (mut harness, _, state) = pull_request_harness_with(pull);
        open_details(&mut harness, state);
        assert_eq!(
            rows_shown(&harness),
            [
                "Branch",
                "Worktree",
                "Reviewers",
                "Labels",
                "Assignees",
                "Milestone"
            ]
        );
        // Twelve reviewers do not fit on one line of the details: the row
        // grows to hold them, none is pushed past the edge, and the name
        // stays on the first line.
        let row = rect(&harness, "summary row Reviewers");
        let details = rect(&harness, "item details");
        assert!(
            row.height() > theme::SUMMARY_ROW,
            "the row at {row:?} did not wrap"
        );
        let first = rect(&harness, "reviewer-with-a-long-name-a");
        let last = rect(&harness, "reviewer-with-a-long-name-l");
        assert!(
            last.y0 > first.y1,
            "the last reviewer at {last:?} is on the first line"
        );
        assert!(
            last.x1 <= details.x1,
            "the last reviewer at {last:?} is past the edge"
        );
        assert!(
            last.x0 >= row.x0 + theme::SUMMARY_NAME_WIDTH,
            "the last reviewer at {last:?} wrapped under the name"
        );
        let name = rect(&harness, "summary name Reviewers");
        assert_eq!((name.y0, name.height()), (row.y0, theme::SUMMARY_ROW));
        // The rows under it follow it whole.
        let labels = rect(&harness, "summary row Labels");
        assert_eq!(labels.y0, row.y1);
    }

    #[test]
    fn an_issue_has_its_status_and_number_and_nothing_an_issue_cannot_have() {
        let (mut harness, _, state) = issue_harness();
        let line = rect(&harness, "summary line");
        assert!(within(rect(&harness, "repository line"), line));
        assert!(within(rect(&harness, "author"), line));
        assert!(named(harness.root_id(), "checks status").is_none());
        assert!(named(harness.root_id(), "churn").is_none());
        // Nothing for the details to say: no branch, and no worktree.
        open_details(&mut harness, state);
        assert!(rows_shown(&harness).is_empty());
        assert_eq!(rect(&harness, "item details").height(), 0.0);
        assert!(named(harness.root_id(), "worktree menu").is_none());
    }

    #[test]
    fn the_notice_is_over_the_rows_on_conversation_and_in_the_tab_bar_on_files() {
        let (mut harness, _, state) = pull_request_harness();
        assert!(named(harness.root_id(), "item notice").is_some());
        assert_eq!(
            rect(&harness, "item notice").height(),
            0.0,
            "nothing to say yet"
        );

        state.offline.set(true);
        harness.rebuild();
        harness.paint();
        let notice = rect(&harness, "item notice");
        let summary = rect(&harness, "item summary");
        let branch = rect(&harness, "summary line");
        assert!(
            within(notice, summary),
            "{notice:?} is not in the summary {summary:?}"
        );
        assert!(notice.y1 <= branch.y0, "{notice:?} is not over the rows");
        assert_eq!(
            notice.x0, summary.x0,
            "{notice:?} does not start where the rows do"
        );

        state.tab.set(ItemTab::Code);
        harness.rebuild();
        harness.paint();
        let notice = rect(&harness, "item notice");
        let tabs = rect(&harness, "tab strip");
        assert!(
            within(notice, tabs),
            "{notice:?} is not in the tab bar {tabs:?}"
        );
        assert!(notice.width() > 0.0);
        assert_eq!(
            tabs.y0,
            theme::REVIEW_HEADER_HEIGHT,
            "the tabs are right under the bar"
        );

        // Open files take the room the notice leaves, not the notice's.
        let alone = notice.width();
        for i in 0..20 {
            state.open_tab(&format!("src/file-{i}.rs"));
        }
        harness.rebuild();
        harness.paint();
        let notice = rect(&harness, "item notice");
        assert_eq!(
            notice.width(),
            alone,
            "twenty tabs squeezed the notice to {notice:?}"
        );
        assert!(within(notice, rect(&harness, "tab strip")));
    }

    #[test]
    fn check_again_on_a_stale_head_opens_the_pull_request_again() {
        let (mut harness, _, state) = pull_request_harness();
        state.stale_head.set(Some("7e30b27c0ffee".into()));
        harness.rebuild();
        harness.paint();
        let notice = rect(&harness, "item notice");
        let again = rect(&harness, "check again");
        assert!(again.width() > 0.0);
        assert!(within(again, notice), "{again:?} is not in {notice:?}");

        // Twenty tabs squeeze the words, not the button.
        state.tab.set(ItemTab::Code);
        for i in 0..20 {
            state.open_tab(&format!("src/file-{i}.rs"));
        }
        harness.rebuild();
        harness.paint();
        let again_on_code = rect(&harness, "check again");
        assert_eq!(again_on_code.width(), again.width());
        assert!(within(again_on_code, rect(&harness, "item notice")));

        harness.click(again_on_code.center().x, again_on_code.center().y);
        assert!(
            matches!(
                state.preparing.get(),
                crate::review::model::Preparing::Working { ref message, .. }
                    if message == "Reading the pull request"
            ),
            "{:?}",
            state.preparing.get()
        );
    }

    /// A worktree with a commit of its own, left behind a pull request
    /// that moved on: the notice says they diverged and offers Rebase.
    #[test]
    fn a_diverged_branch_is_offered_rebase_not_check_again() {
        let (mut harness, _, state) = pull_request_harness();
        state.item.update(|slot| {
            let mut item = (**slot.as_ref().unwrap()).clone();
            let (pr_ref, pr_head) = item
                .pull_request()
                .map(|pr| (pr.head_ref.clone(), pr.head_sha.clone()))
                .unwrap();
            let c = item.checkout.as_mut().unwrap();
            c.branch = pr_ref;
            c.on_disk = true;
            c.head_sha = "mine".into();
            c.divergence = Some(perch_github::Divergence {
                head: "mine".into(),
                pr_head,
                ahead: 1,
                behind: 12,
            });
            *slot = Some(Arc::new(item));
        });
        state.stale_head.set(Some("2070f3d1c0ffee".into()));
        harness.rebuild();
        harness.paint();

        assert_eq!(
            notice_text(state).as_deref(),
            Some("Not updated to 2070f3d: branch has diverged")
        );
        assert!(named(harness.root_id(), "check again").is_none());
        let rebase = rect(&harness, "rebase in notice");
        assert!(within(rebase, rect(&harness, "item notice")));

        harness.click(rebase.center().x, rebase.center().y);
        assert!(state.rebase_pending.get());
        // The worktree's row in the details has it too.
        open_details(&mut harness, state);
        assert!(rect(&harness, "rebase-branch").width() > 0.0);
    }

    pub(crate) fn snapshot() -> crate::review::model::ReviewGroupSummary {
        let created_at = "2026-09-19T12:00:00Z".parse().unwrap();
        crate::review::model::ReviewGroupSummary {
            group: perch_github::ReviewGroup {
                id: "g1".into(),
                item_id: "pr".into(),
                snapshot_id: "snapshot".into(),
                number: 2,
                status: perch_github::ReviewGroupStatus::Completed,
                new_findings: 0,
                still_present: 0,
                verified_fixed: 0,
                not_verified: 0,
                created_at,
                started_at: Some(created_at),
                ended_at: Some(created_at),
                instructions: None,
            },
            snapshot: perch_github::ReviewSnapshot {
                id: "snapshot".into(),
                item_id: "pr".into(),
                repository: "slatedb/slatedb".into(),
                branch_label: Some("feature".into()),
                requested_base_ref: "main".into(),
                base_tip_sha: "base".into(),
                merge_base_sha: "merge-base".into(),
                head_sha: "head".into(),
                dirty_edits_excluded: false,
                retention_key: "snapshot".into(),
                retained_ref: None,
                manifest_path: None,
                diff_path: None,
                created_at,
                worktree_parent_sha: None,
            },
            attempts: Vec::new(),
        }
    }

    #[test]
    fn a_retained_snapshot_has_its_way_back_on_both_tabs() {
        let (mut harness, _, state) = pull_request_harness();
        state.historical_review.set(Some(snapshot()));
        harness.rebuild();
        harness.paint();
        let banner = rect(&harness, "snapshot banner");
        let back = rect(&harness, "back to current");
        assert!(within(banner, rect(&harness, "item summary")));
        assert!(within(back, banner));

        state.tab.set(ItemTab::Code);
        harness.rebuild();
        harness.paint();
        let back = rect(&harness, "back to current");
        assert!(back.width() > 0.0);
        assert!(
            within(back, rect(&harness, "tab strip")),
            "{back:?} is not in the tab bar"
        );
        // The click asks for the item as it is now, and the view waits on
        // it.
        harness.click(back.center().x, back.center().y);
        assert!(
            matches!(
                state.preparing.get(),
                crate::review::model::Preparing::Working { ref message, .. }
                    if message == "Reading the current files"
            ),
            "{:?}",
            state.preparing.get()
        );
    }

    fn user(login: &str, name: Option<&str>) -> MentionableUser {
        MentionableUser {
            login: login.into(),
            name: name.map(Into::into),
            avatar_url: String::new(),
        }
    }

    #[test]
    fn the_assign_menu_has_the_reader_then_the_assigned_then_the_rest_and_picking_toggles() {
        let picked = Rc::new(std::cell::RefCell::new(Vec::new()));
        let set: Rc<dyn Fn(String, bool)> = {
            let picked = picked.clone();
            Rc::new(move |login, assign| picked.borrow_mut().push((login, assign)))
        };
        let users = [
            user("agavra", Some("Almog")),
            user("cadonna", None),
            user("criccomini", Some("Chris")),
            user("rohan", None),
        ];
        // Rohan is assigned; Fivo has lost access but stays, to be taken off.
        let assigned = ["rohan".to_string(), "FiV0".to_string()];
        let menu = assign_menu(Some("criccomini"), &assigned, &users, set.clone());
        assert_eq!(
            menu.outline(),
            ["criccomini", "✓ rohan", "✓ FiV0", "agavra", "cadonna"]
        );
        // Each pick flips one, with the menu still up for the next.
        menu.choose("criccomini");
        menu.choose("rohan");
        menu.choose("FiV0");
        menu.choose("rohan");
        assert_eq!(
            *picked.borrow(),
            [
                ("criccomini".to_string(), true),
                ("rohan".to_string(), false),
                ("FiV0".to_string(), false),
                ("rohan".to_string(), true),
            ]
        );
        assert_eq!(
            menu.outline(),
            ["✓ criccomini", "✓ rohan", "FiV0", "agavra", "cadonna"]
        );

        // Ten assigned is GitHub's most: the rest are greyed, and the ten
        // can still be taken off, which frees the rest.
        let ten: Vec<String> = (0..10).map(|n| format!("u{n}")).collect();
        let menu = assign_menu(Some("criccomini"), &ten, &users, set);
        let outline = menu.outline();
        assert_eq!(outline[0], "(criccomini)");
        assert_eq!(outline[1], "✓ u0");
        assert_eq!(outline.last().unwrap(), "(rohan)");
        menu.choose("u0");
        assert_eq!(menu.outline()[0], "criccomini");
    }

    #[test]
    fn the_assignees_row_offers_assign_once_github_says_the_reader_may() {
        let (mut harness, _, state) = issue_harness();
        open_details(&mut harness, state);
        assert!(
            !rows_shown(&harness).contains(&"Assignees".to_string()),
            "nobody assigned and no word from GitHub: no row"
        );
        // Read access: still no row.
        state.assignable.update(|all| {
            all.insert("slatedb/slatedb#400".into(), None);
        });
        harness.rebuild();
        harness.paint();
        assert!(!rows_shown(&harness).contains(&"Assignees".to_string()));

        state.assignable.update(|all| {
            all.insert(
                "slatedb/slatedb#400".into(),
                Some(Arc::new(vec![user("criccomini", None)])),
            );
        });
        harness.rebuild();
        harness.paint();
        assert!(rows_shown(&harness).contains(&"Assignees".to_string()));
        let row = rect(&harness, "summary row Assignees");
        let button = rect(&harness, "assign menu");
        assert!(
            within(button, row),
            "Assign at {button:?} is outside {row:?}"
        );
        assert_eq!(
            button.x0,
            row.x0 + theme::SUMMARY_NAME_WIDTH,
            "with nobody assigned, Assign is where the values start"
        );
    }

    #[test]
    fn an_assignee_has_the_face_the_item_already_has_for_them() {
        let mut pull = pull_request(812);
        pull.author_login = "sgbalogh".into();
        pull.author_avatar_url = "https://avatars/sgbalogh".into();
        pull.assignees = vec!["sgbalogh".into(), "rohan".into(), "nobody".into()];
        let (mut harness, app, state) = pull_request_harness_with(pull);
        let mut png = Vec::new();
        image::RgbaImage::new(2, 2)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let png = Arc::new(png);
        app.avatars.update(|m| {
            m.insert("https://avatars/sgbalogh".into(), png.clone());
            m.insert("https://avatars/rohan".into(), png.clone());
        });
        // Rohan's face is known only from the Assign menu.
        let mut rohan = user("rohan", None);
        rohan.avatar_url = "https://avatars/rohan".into();
        state.assignable.update(|all| {
            all.insert("slatedb/slatedb#812".into(), Some(Arc::new(vec![rohan])));
        });
        open_details(&mut harness, state);
        let row = named(harness.root_id(), "summary row Assignees").unwrap();
        let initial = |c: &str| named(row, &format!("Label: {c:?}")).is_some();
        assert!(!initial("S"), "the author's face, as in the Author row");
        assert!(!initial("R"), "the face from the Assign menu");
        assert!(initial("N"), "nobody's face is known: their initial");
    }

    #[test]
    fn who_may_be_assigned_is_asked_as_an_issue_opens() {
        let app = AppState::new();
        let state = ReviewState::new();
        let now = "2026-09-20T12:00:00Z".parse().unwrap();
        let mut item = Item::new("slatedb/slatedb", now);
        item.subject = Some(ItemSubject::Issue(perch_github::Issue {
            viewer_can_update: false,
            node_id: String::new(),
            reactions: Default::default(),
            number: 2144,
            title: "Linear seek".into(),
            body: String::new(),
            state: "open".into(),
            state_reason: None,
            author_login: "FiV0".into(),
            author_avatar_url: String::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            comments: 0,
            created_at: None,
            updated_at: now,
            repo_full_name: "slatedb/slatedb".into(),
        }));
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state.phase.set(crate::review::model::Phase::Issue);
        state.composer_autofocus.set(false);
        let (backend, mut commands) = Backend::recording();
        let root = floem::headless::TestRoot::new();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness =
            floem::headless::HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        let mut asked = false;
        while let Ok(command) = commands.try_recv() {
            asked |= matches!(
                command,
                crate::backend::Command::AssignableUsers { ref repo_full_name, number }
                    if repo_full_name == "slatedb/slatedb" && number == 2144
            );
        }
        assert!(asked, "who may be assigned is asked as the issue opens");
    }

    #[test]
    fn a_reviewer_or_an_assignee_is_a_link_to_their_page_on_github() {
        let mut pull = pull_request(812);
        pull.requested_reviewers = vec![
            perch_github::Reviewer {
                login: "agavra".into(),
                avatar_url: String::new(),
            },
            perch_github::Reviewer {
                login: "dependabot[bot]".into(),
                avatar_url: String::new(),
            },
        ];
        pull.assignees = vec!["cadonna".into()];
        let (mut harness, _, state) = pull_request_harness_with(pull);
        open_details(&mut harness, state);
        let opened = crate::ui::widgets::opened;
        opened();
        for (login, url) in [
            ("agavra", "https://github.com/agavra"),
            ("dependabot[bot]", "https://github.com/apps/dependabot"),
            ("cadonna", "https://github.com/cadonna"),
        ] {
            let c = rect(&harness, &format!("name label - Label: {login:?}")).center();
            harness.click(c.x, c.y);
            assert_eq!(opened(), vec![url.to_string()], "{login}");
        }
        // Under the pointer a name shows the underline and the pointer
        // that say it is a link; off it, neither.
        use floem::peniko::Color;
        use floem::style::{BorderBottomColor, Cursor, CursorStyle};
        let id = named(harness.root_id(), "name label - Label: \"agavra\"").unwrap();
        harness.pointer_move(0.0, 0.0);
        harness.recompute_styles();
        let away = harness.get_computed_style(id);
        assert_eq!(away.get(BorderBottomColor), Some(Color::TRANSPARENT.into()));
        assert_eq!(away.get(Cursor), None);
        let c = id.get_visual_rect_no_clip().center();
        harness.pointer_move(c.x, c.y);
        harness.recompute_styles();
        let under = harness.get_computed_style(id);
        assert_eq!(under.get(BorderBottomColor), Some(theme::fg_muted().into()));
        assert_eq!(under.get(Cursor), Some(CursorStyle::Pointer));
    }

    #[test]
    fn what_perch_started_from_the_item_is_a_row_of_chips() {
        let (mut harness, app, state) = pull_request_harness();
        open_details(&mut harness, state);
        assert!(!rows_shown(&harness).contains(&"In Perch".to_string()));
        let now = "2026-09-20T12:00:00Z".parse().unwrap();
        let mut child = Item::new("slatedb/slatedb", now);
        child.id = "child".into();
        child.name = Some("Child branch".into());
        child.from_item_id = Some("pr".into());
        app.items.set(vec![child]);
        harness.rebuild();
        harness.paint();
        // The sidebar names the child too; the chip is the row's.
        let row_id = named(harness.root_id(), "summary row In Perch").unwrap();
        let row = row_id.get_visual_rect_no_clip();
        // The name is the label in the chip; the chip is around it.
        let chip = named(row_id, "Child branch")
            .and_then(|label| label.parent())
            .expect("no chip in the row")
            .get_visual_rect_no_clip();
        assert!(
            within(chip, row),
            "the chip at {chip:?} is not in the row {row:?}"
        );
        assert_eq!(chip.x0, row.x0 + theme::SUMMARY_NAME_WIDTH);
    }

    #[test]
    fn the_repository_line_names_the_slug_and_number_unless_the_repository_is_local() {
        assert_eq!(
            repo_line(&branch_item("slatedb/slatedb")),
            "slatedb/slatedb"
        );
        assert_eq!(repo_line(&branch_item("local/notes")), "");
        assert_eq!(
            repo_line(&branch_item("Local/notes")),
            "",
            "the owner in any case"
        );

        let mut pull = branch_item("slatedb/slatedb");
        pull.subject = Some(ItemSubject::PullRequest(pull_request(812)));
        assert_eq!(repo_line(&pull), "slatedb/slatedb #812");
    }

    #[test]
    fn the_repository_s_grants_are_listed_on_a_work_item_and_can_be_revoked() {
        let app = AppState::new();
        let state = ReviewState::new();
        let mut item = branch_item("slatedb/slatedb");
        item.id = "branch".into();
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state.phase.set(crate::review::model::Phase::Ready);
        state.composer_autofocus.set(false);
        let grant = |id: &str, label: &str| perch_github::WorkGrantRecord {
            id: id.into(),
            repo: "slatedb/slatedb".into(),
            key: label.into(),
            label: label.into(),
            created_at: "2026-09-22T10:00:00Z".parse().unwrap(),
        };
        state.grants.update(|all| {
            all.insert(
                "slatedb/slatedb".into(),
                vec![grant("g1", "example.com"), grant("g2", "commit")],
            );
        });
        let (backend, mut commands) = Backend::recording();
        let root = floem::headless::TestRoot::new();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness =
            floem::headless::HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        open_details(&mut harness, state);
        assert_eq!(rows_shown(&harness), ["Branch", "Worktree", "Work may"]);
        let mut asked = false;
        while let Ok(command) = commands.try_recv() {
            asked |= matches!(command, crate::backend::Command::ListGrants { ref repo } if repo == "slatedb/slatedb");
        }
        assert!(asked, "the grants are asked for as the item opens");

        let revoke = named(harness.root_id(), "revoke-grant").expect("a × on the grant");
        let center = revoke.get_visual_rect_no_clip().center();
        harness.click(center.x, center.y);
        let mut revoked = Vec::new();
        while let Ok(command) = commands.try_recv() {
            if let crate::backend::Command::RevokeGrant { id } = command {
                revoked.push(id);
            }
        }
        assert_eq!(revoked, ["g1"]);
    }

    /// The worktree menu offers Always Trust for each server held, and
    /// Stop Trusting for each one trusted on the whole repository, held
    /// or not; each sends the repository and the server.
    #[test]
    fn the_worktree_menu_trusts_a_server_on_the_repository_and_stops() {
        use crate::backend::Command;
        let _root = floem::headless::TestRoot::new();
        let app = AppState::new();
        let (backend, mut commands) = Backend::recording();
        let backend = Rc::new(backend);
        let mut item = branch_item("slatedb/slatedb");
        item.id = "branch".into();
        let rust = perch_lsp::ServerKind::RustAnalyzer;
        let menu = |held: &[perch_lsp::ServerKind]| {
            crate::ui::widgets::worktree_menu(
                "/w".into(),
                None,
                trust_rows(app, &backend, &item, held),
            )
        };
        let mut sent = || {
            std::iter::from_fn(|| commands.try_recv().ok())
                .filter_map(|command| match command {
                    Command::TrustServerInRepo {
                        repo,
                        kind,
                        trusted,
                    } => Some((repo, kind, trusted)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        // A line first is dropped as the menu is drawn.
        let reveal = format!("Reveal in {}", crate::ui::widgets::FILE_MANAGER);
        assert_eq!(
            menu(&[]).outline()[..2],
            ["—", reveal.as_str()],
            "nothing held or trusted"
        );
        let held = menu(&[rust]);
        assert_eq!(
            held.outline()[..3],
            [
                "Trust and Start rust-analyzer",
                "Always Trust rust-analyzer in slatedb/slatedb",
                "—"
            ]
        );
        held.choose("Always Trust rust-analyzer in slatedb/slatedb");
        assert_eq!(sent(), [("slatedb/slatedb".to_string(), rust, true)]);

        app.apply(crate::backend::Event::TrustedServers(vec![(
            "SlateDB/SlateDB".into(),
            rust,
        )]));
        let trusted = menu(&[]);
        assert_eq!(
            trusted.outline()[0],
            "Stop Trusting rust-analyzer in slatedb/slatedb"
        );
        trusted.choose("Stop Trusting rust-analyzer in slatedb/slatedb");
        assert_eq!(sent(), [("slatedb/slatedb".to_string(), rust, false)]);
    }

    #[test]
    fn the_worktree_warning_says_once_what_waits() {
        use perch_lsp::ServerKind;
        assert_eq!(
            held_tip(&[ServerKind::RustAnalyzer]),
            "rust-analyzer waits for you to trust this code"
        );
        assert_eq!(
            held_tip(&[ServerKind::RustAnalyzer, ServerKind::Jdtls]),
            "rust-analyzer and jdtls wait for you to trust this code"
        );
    }

    /// While a server waits for the reader to trust the code, the worktree
    /// has a warning and its menu starts with Trust and Start, which does
    /// what the toast's button does, however long ago the toast went.
    #[test]
    fn a_held_server_is_trusted_from_the_worktree_menu() {
        use crate::backend::Command;
        let app = AppState::new();
        let state = ReviewState::new();
        let mut item = branch_item("slatedb/slatedb");
        item.id = "branch".into();
        state.item.set(Some(Arc::new(item.clone())));
        state.ready.set(true);
        state.phase.set(crate::review::model::Phase::Ready);
        state.composer_autofocus.set(false);
        app.open_item.set(Some("branch".into()));
        let (backend, mut commands) = Backend::recording();
        let root = floem::headless::TestRoot::new();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness =
            floem::headless::HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        let settle = |harness: &mut floem::headless::HeadlessHarness| {
            harness.rebuild();
            harness.paint();
        };
        settle(&mut harness);
        open_details(&mut harness, state);
        let open_menu = |harness: &mut floem::headless::HeadlessHarness| {
            let chip = rect(harness, "worktree menu").center();
            harness.pointer_down(chip.x, chip.y);
            settle(harness);
            assert!(crate::ui::menu::is_open());
        };
        let trusted = |commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>| {
            std::iter::from_fn(|| commands.try_recv().ok())
                .filter_map(|command| match command {
                    Command::TrustLanguageServers { item_id, .. } => Some(item_id),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        // Nothing held: no warning, and nothing to trust in the menu.
        assert_eq!(rect(&harness, "untrusted").width(), 0.0);
        open_menu(&mut harness);
        let reveal = format!("menu-row:Reveal in {}", crate::ui::widgets::FILE_MANAGER);
        assert!(named(harness.root_id(), &reveal).is_some());
        assert!(named(harness.root_id(), "menu-row:Trust").is_none());
        crate::ui::menu::close();
        settle(&mut harness);

        let rust = perch_lsp::ServerKind::RustAnalyzer;
        state.set_server_status(rust, perch_lsp::Status::Held);
        state.set_server_status(perch_lsp::ServerKind::Jdtls, perch_lsp::Status::Held);
        app.offer_server(
            crate::state::ServerOffer::held(&item, &[rust, perch_lsp::ServerKind::Jdtls]),
            false,
        );
        settle(&mut harness);
        assert!(
            rect(&harness, "untrusted").width() > 0.0,
            "a warning by the name"
        );
        assert!(trusted(&mut commands).is_empty());
        open_menu(&mut harness);
        let row = rect(&harness, "menu-row:Trust and Start rust-analyzer and jdtls");
        let first = rect(&harness, "menu-row:");
        assert_eq!(row, first, "Trust and Start comes first");
        harness.click(row.center().x, row.center().y);
        settle(&mut harness);
        assert!(!crate::ui::menu::is_open());
        assert_eq!(trusted(&mut commands), ["branch"]);
        assert!(
            app.current_toast().is_none(),
            "the toast's ask goes with it"
        );

        // In dark, the warning goes dark too.
        let left = crate::theme::dark_check::light_left(&mut harness);
        assert!(
            left.is_empty(),
            "light colours left in dark:\n{}",
            left.join("\n")
        );
    }

    #[test]
    fn a_branch_of_the_readers_own_has_its_repository_where_a_pull_request_has_its_status() {
        let app = AppState::new();
        let state = ReviewState::new();
        let mut item = branch_item("slatedb/slatedb");
        item.id = "branch".into();
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state.phase.set(crate::review::model::Phase::Ready);
        state.composer_autofocus.set(false);
        let root = floem::headless::TestRoot::new();
        let view = crate::ui::app_view(app, state, Rc::new(Backend::detached()));
        let mut harness =
            floem::headless::HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "state pill").is_none());
        // There is nothing to merge, and the line starts at the
        // repository.
        assert_eq!(rect(&harness, "merge-control").width(), 0.0);
        let line = rect(&harness, "summary line");
        let repo = rect(&harness, "repository line");
        assert_eq!(
            repo.x0, line.x0,
            "the repository at {repo:?} is not where the line starts"
        );
        // A branch has no page, and its repository leads nowhere.
        crate::ui::widgets::opened();
        harness.click(repo.center().x, repo.center().y);
        assert_eq!(crate::ui::widgets::opened(), Vec::<String>::new());
        // Its worktree is in the details, as a pull request's is.
        open_details(&mut harness, state);
        assert_eq!(rows_shown(&harness), ["Branch", "Worktree"]);
        assert!(within(
            rect(&harness, "worktree menu"),
            rect(&harness, "summary row Worktree")
        ));
    }

    /// The repository and number after the pill open the pull request on
    /// github.com.
    #[test]
    fn the_status_rows_number_opens_the_pull_request() {
        let (mut harness, ..) = pull_request_harness();
        let repo = rect(&harness, "repository line");
        crate::ui::widgets::opened();
        harness.click(repo.center().x, repo.center().y);
        assert_eq!(
            crate::ui::widgets::opened(),
            ["https://github.com/slatedb/slatedb/pull/812"]
        );
    }

    #[test]
    fn a_short_ref_is_a_hash_cut_to_seven_or_a_branch_whole() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(short_ref(sha), "0123456");
        assert_eq!(short_ref("main"), "main");
    }

    #[test]
    fn the_issues_a_description_closes_are_read_in_any_case_and_once_each() {
        let body = "Closes #12 and fixes #3.\n\nResolved #12 again; see #99 and other/repo#7. fix #x fixed #";
        assert_eq!(closes(body), [12, 3]);
        assert_eq!(closes("nothing here"), Vec::<u64>::new());
        assert_eq!(
            closes("close\n#5"),
            [5],
            "a line break between the word and the number is fine"
        );
    }

    #[test]
    fn every_state_the_pill_can_show_has_a_word() {
        for s in [
            SubjectState::PrOpen,
            SubjectState::PrDraft,
            SubjectState::PrMerged,
            SubjectState::PrClosed,
            SubjectState::IssueOpen,
            SubjectState::IssueClosed,
            SubjectState::IssueNotPlanned,
        ] {
            assert!(!state_word(s).is_empty(), "{s:?}");
        }
    }

    #[test]
    fn the_author_or_a_writer_may_change_an_open_pull_request_to_or_from_a_draft() {
        let _root = floem::headless::TestRoot::new();
        let app = AppState::new();
        app.viewer.set(Some(("U".into(), String::new())));
        let state = ReviewState::new();
        let with = |author: &str, change: &dyn Fn(&mut perch_github::PullRequest)| {
            let mut pr = pull_request(7);
            pr.author_login = author.into();
            change(&mut pr);
            state.pull_request.set(Some(Arc::new(pr)));
        };
        let permission = |permission: Option<perch_github::RepositoryPermission>| {
            state
                .pull_request_overview
                .set(permission.map(|permission| {
                    Arc::new(perch_github::PullRequestOverview {
                        head_sha: "h".into(),
                        mergeable: perch_github::MergeableState::Mergeable,
                        merge_state: perch_github::MergeState::Clean,
                        review_decision: None,
                        viewer_can_merge_as_admin: false,
                        viewer_permission: Some(permission),
                        allowed_merge_methods: Default::default(),
                        checks: perch_github::Checks {
                            state: CheckState::Passing,
                            url: None,
                            runs: Vec::new(),
                        },
                        squash_message: Default::default(),
                        merge_message: Default::default(),
                        cross_repository: false,
                        maintainer_can_modify: false,
                    })
                }));
        };
        with("u", &|_| {});
        assert!(may_set_draft(app, state), "its author, in any case");
        with("u", &|pr| pr.draft = true);
        assert!(may_set_draft(app, state));
        with("someone", &|_| {});
        assert!(
            !may_set_draft(app, state),
            "someone else's, with no word on access"
        );
        permission(Some(perch_github::RepositoryPermission::Triage));
        assert!(!may_set_draft(app, state));
        permission(Some(perch_github::RepositoryPermission::Write));
        assert!(
            may_set_draft(app, state),
            "someone who may write to the repository"
        );
        with("u", &|pr| pr.merged = true);
        assert!(!may_set_draft(app, state));
        with("u", &|pr| pr.state = "closed".into());
        assert!(!may_set_draft(app, state));
        with("u", &|_| {});
        state.offline.set(true);
        assert!(!may_set_draft(app, state));
    }

    #[test]
    fn the_state_pill_s_menu_changes_a_pull_request_to_or_from_a_draft() {
        use crate::backend::Command;

        let _root = floem::headless::TestRoot::new();
        let state = ReviewState::new();
        let mut item = Item::new("slatedb/slatedb", "2026-09-20T09:00:00Z".parse().unwrap());
        item.id = "pr".into();
        state.item.set(Some(Arc::new(item)));
        let (backend, mut commands) = Backend::recording();
        let backend = Rc::new(backend);
        let pill = floem::ViewId::new();
        let menu = |subject| state_menu(state, backend.clone(), subject, (true, false), pill);

        assert_eq!(menu(SubjectState::PrDraft).outline(), ["Ready for review"]);
        menu(SubjectState::PrDraft).choose("Ready for review");
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::SetDraft { item_id, draft: false }) if item_id == "pr"
        ));
        assert_eq!(state.draft_pending.get_untracked(), Some(false));
        // Under way: said, and not to be pressed again.
        assert_eq!(
            menu(SubjectState::PrDraft).outline(),
            ["(Marking ready for review…)"]
        );

        state.draft_pending.set(None);
        assert_eq!(menu(SubjectState::PrOpen).outline(), ["Convert to draft"]);
        menu(SubjectState::PrOpen).choose("Convert to draft");
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::SetDraft { item_id, draft: true }) if item_id == "pr"
        ));

        state.draft_pending.set(None);
        assert!(menu(SubjectState::PrMerged).is_empty());
    }

    #[test]
    fn the_state_pill_s_menu_closes_and_reopens_a_pull_request_or_an_issue() {
        use crate::backend::Command;
        use perch_github::IssueCloseReason;

        let _root = floem::headless::TestRoot::new();
        let state = ReviewState::new();
        let mut item = Item::new("slatedb/slatedb", "2026-09-20T09:00:00Z".parse().unwrap());
        item.id = "it".into();
        state.item.set(Some(Arc::new(item)));
        let (backend, mut commands) = Backend::recording();
        let backend = Rc::new(backend);
        let pill = floem::ViewId::new();
        let menu = |subject, may| state_menu(state, backend.clone(), subject, may, pill);
        let mut sent = move || match commands.try_recv() {
            Ok(Command::ChangeState { item_id, change }) if item_id == "it" => change,
            other => panic!("{other:?}"),
        };

        assert_eq!(
            menu(SubjectState::PrOpen, (true, true)).outline(),
            ["Convert to draft", "—", "Close pull request"]
        );
        assert_eq!(
            menu(SubjectState::PrDraft, (false, true)).outline(),
            ["Close pull request"]
        );
        menu(SubjectState::PrOpen, (false, true)).choose("Close pull request");
        assert_eq!(sent(), StateChange::ClosePullRequest);
        assert_eq!(
            menu(SubjectState::PrOpen, (true, true)).outline(),
            ["(Closing…)"]
        );

        state.state_pending.set(None);
        assert_eq!(
            menu(SubjectState::PrClosed, (false, true)).outline(),
            ["Reopen pull request"]
        );
        menu(SubjectState::PrClosed, (false, true)).choose("Reopen pull request");
        assert_eq!(sent(), StateChange::Reopen);
        assert_eq!(
            menu(SubjectState::PrClosed, (false, true)).outline(),
            ["(Reopening…)"]
        );

        state.state_pending.set(None);
        assert_eq!(
            menu(SubjectState::IssueOpen, (false, true)).outline(),
            [
                "Close as completed",
                "Close as not planned",
                "Close as duplicate…"
            ]
        );
        menu(SubjectState::IssueOpen, (false, true)).choose("Close as not planned");
        assert_eq!(
            sent(),
            StateChange::CloseIssue(IssueCloseReason::NotPlanned)
        );

        // A duplicate is searched for first, under the pill, and closes at
        // a pick; Escape there leaves the search rather than going to the
        // composer's ways.
        state.state_pending.set(None);
        menu(SubjectState::IssueOpen, (false, true)).choose("Close as duplicate…");
        assert!(state
            .close_menu
            .get_untracked()
            .is_some_and(|m| m.duplicate && m.direct));
        crate::review::close_menu::back(state);
        assert_eq!(state.close_menu.get_untracked(), None);

        for closed in [SubjectState::IssueClosed, SubjectState::IssueNotPlanned] {
            assert_eq!(menu(closed, (false, true)).outline(), ["Reopen issue"]);
        }
        menu(SubjectState::IssueClosed, (false, true)).choose("Reopen issue");
        assert_eq!(sent(), StateChange::Reopen);

        state.state_pending.set(None);
        assert!(menu(SubjectState::IssueOpen, (false, false)).is_empty());
        assert!(menu(SubjectState::PrClosed, (true, false)).is_empty());
    }

    #[test]
    fn who_may_close_or_reopen_is_the_author_a_triager_or_whom_github_says() {
        let (_harness, app, state) = issue_harness();
        app.viewer.set(Some(("U".into(), String::new())));
        assert!(
            may_change_state(app, state),
            "the issue's author, until GitHub says"
        );
        state.issue_closable.set(Some(false));
        assert!(!may_change_state(app, state));
        state.issue_closable.set(Some(true));
        assert!(may_change_state(app, state));
        state.offline.set(true);
        assert!(!may_change_state(app, state));
        state.offline.set(false);

        let mut closed = (*state.issue.get_untracked().unwrap()).clone();
        closed.state = "closed".into();
        state.issue.set(Some(Arc::new(closed)));
        assert!(
            !may_change_state(app, state),
            "a closed issue waits for GitHub"
        );
        state.issue_reopenable.set(Some(true));
        assert!(may_change_state(app, state));

        state.issue.set(None);
        let mut pr = pull_request(7);
        pr.author_login = "someone".into();
        state.pull_request.set(Some(Arc::new(pr.clone())));
        assert!(!may_change_state(app, state));
        pr.author_login = "u".into();
        state.pull_request.set(Some(Arc::new(pr.clone())));
        assert!(may_change_state(app, state));
        pr.state = "closed".into();
        state.pull_request.set(Some(Arc::new(pr.clone())));
        assert!(may_change_state(app, state), "to reopen it");
        pr.merged = true;
        state.pull_request.set(Some(Arc::new(pr)));
        assert!(!may_change_state(app, state));
    }

    #[test]
    fn push_is_offered_for_ones_own_pull_request_ahead_and_not_behind() {
        let _root = floem::headless::TestRoot::new();
        let app = AppState::new();
        app.viewer.set(Some(("u".into(), String::new())));
        let state = ReviewState::new();
        let with = |ahead: u32, behind: u32, author: &str| {
            let mut item = Item::new("slatedb/slatedb", "2026-09-20T09:00:00Z".parse().unwrap());
            let mut pr = pull_request(7);
            pr.author_login = author.into();
            item.subject = Some(ItemSubject::PullRequest(pr));
            item.checkout = Some(perch_github::ItemCheckout {
                branch: "feature".into(),
                base_ref: "main".into(),
                head_sha: "w".into(),
                path: "/tmp/perch-push-test".into(),
                owned: false,
                on_disk: true,
                churn: None,
                last_seen_head: None,
                divergence: Some(perch_github::Divergence {
                    head: "w".into(),
                    pr_head: "h".into(),
                    ahead,
                    behind,
                }),
                source: None,
            });
            state.item.set(Some(Arc::new(item)));
        };
        with(2, 0, "u");
        assert!(can_push(app, state));
        with(2, 1, "u");
        assert!(!can_push(app, state), "a push there would need force");
        with(0, 1, "u");
        assert!(!can_push(app, state));
        with(2, 0, "someone");
        assert!(!can_push(app, state), "not the reader's pull request");
    }

    fn check(name: &str, state: perch_github::CheckRunState) -> perch_github::Check {
        perch_github::Check {
            name: name.into(),
            workflow: None,
            event: None,
            state,
            description: None,
            url: None,
            started_at: None,
            completed_at: None,
            kind: perch_github::CheckKind::CheckRun,
        }
    }

    fn checks(runs: Vec<perch_github::Check>) -> Checks {
        let state = if runs.iter().any(|c| c.state.is_failing()) {
            CheckState::Failing {
                workflow: "CI".into(),
            }
        } else if runs.iter().any(|c| c.state.is_pending()) {
            CheckState::Running
        } else {
            CheckState::Passing
        };
        Checks {
            state,
            url: None,
            runs,
        }
    }

    /// `failing` checks failed and `passing` more passed.
    fn checks_of(failing: &[&str], passing: usize) -> Checks {
        use perch_github::CheckRunState;
        let mut runs: Vec<_> = failing
            .iter()
            .map(|name| check(name, CheckRunState::Failure))
            .collect();
        runs.extend((0..passing).map(|i| check(&format!("job-{i}"), CheckRunState::Success)));
        checks(runs)
    }

    /// GitHub's overview of pull request 812 at its head `h`: the reader
    /// may write, nothing conflicts, and it is approved.
    fn overview(checks: Checks) -> perch_github::PullRequestOverview {
        perch_github::PullRequestOverview {
            head_sha: "h".into(),
            mergeable: perch_github::MergeableState::Mergeable,
            merge_state: perch_github::MergeState::Clean,
            review_decision: Some(perch_github::ReviewDecision::Approved),
            viewer_can_merge_as_admin: false,
            viewer_permission: Some(perch_github::RepositoryPermission::Write),
            allowed_merge_methods: perch_github::MergeMethods {
                merge: true,
                squash: true,
                rebase: true,
            },
            checks,
            squash_message: Default::default(),
            merge_message: Default::default(),
            cross_repository: false,
            maintainer_can_modify: false,
        }
    }

    /// What the checks' glyph says under the pointer.
    fn checks_say(state: ReviewState) -> Option<(Tone, String)> {
        checks_line_now(state).map(|line| {
            let (_, standing) = checks_standing(&line);
            (standing.tone, standing.says)
        })
    }

    fn standing(tone: Tone, says: &str) -> Option<Standing> {
        Some(Standing::new(tone, says))
    }

    #[test]
    fn the_checks_name_what_failed_and_count_what_passed() {
        let line =
            |checks: &Checks| checks_line_of(checks).map(|line| (line.verdict, line.words()));
        assert_eq!(
            line(&checks_of(&["clippy"], 13)),
            Some((Verdict::Failing, "clippy failed · 13 passed".into()))
        );
        assert_eq!(
            line(&checks_of(&["clippy", "fmt"], 12)),
            Some((Verdict::Failing, "2 failed · 12 passed".into()))
        );
        assert_eq!(
            line(&checks_of(&["clippy", "fmt", "test"], 0)),
            Some((Verdict::Failing, "3 failed".into()))
        );
        // One check failing on push and on pull_request is one name.
        assert_eq!(
            line(&checks_of(&["clippy", "clippy"], 1)),
            Some((Verdict::Failing, "clippy failed · 1 passed".into()))
        );
        assert_eq!(
            line(&checks_of(&[], 14)),
            Some((Verdict::Passing, "14 passed".into()))
        );
        let mut running = checks_of(&[], 3);
        running
            .runs
            .push(check("test", perch_github::CheckRunState::Pending));
        running.state = CheckState::Running;
        assert_eq!(
            line(&running),
            Some((Verdict::Running, "Checks running".into()))
        );

        // The cached rollup has no list: its verdict, and the run that
        // failed by name.
        let rollup = |state| Checks {
            state,
            url: None,
            runs: Vec::new(),
        };
        assert_eq!(
            line(&rollup(CheckState::Passing)),
            Some((Verdict::Passing, "Checks passed".into()))
        );
        assert_eq!(
            line(&rollup(CheckState::Failing {
                workflow: "CI".into()
            })),
            Some((Verdict::Failing, "CI failed".into()))
        );
        assert_eq!(line(&rollup(CheckState::Unknown)), None);
    }

    /// The checks are one glyph at the line's end, with no row of their
    /// own, reading the live overview's checks once they come over the
    /// cached rollup, and nothing of merging in the summary: that is the
    /// bar's.
    #[test]
    fn the_checks_are_one_glyph_on_the_line_and_the_summary_has_no_merge() {
        let (mut harness, _, state) = pull_request_harness();
        assert!(named(harness.root_id(), "summary row Checks").is_none());
        assert_eq!(
            checks_say(state),
            Some((Tone::Good, "Checks passed".into()))
        );
        let line = rect(&harness, "summary line");
        let glyph = rect(&harness, "checks status");
        assert_eq!((glyph.width(), glyph.height()), (STATUS_BOX, STATUS_BOX));
        assert_eq!(
            glyph.x1, line.x1,
            "{glyph:?} does not end the line {line:?}"
        );

        // The overview brings the review and the merge after it.
        state
            .pull_request_overview
            .set(Some(Arc::new(overview(checks_of(&[], 14)))));
        harness.rebuild();
        harness.paint();
        assert_eq!(checks_say(state), Some((Tone::Good, "14 passed".into())));
        let glyph = rect(&harness, "checks status");

        state
            .pull_request_overview
            .set(Some(Arc::new(overview(checks_of(&["clippy"], 13)))));
        harness.rebuild();
        harness.paint();
        assert_eq!(
            checks_say(state),
            Some((Tone::Bad, "clippy failed · 13 passed".into()))
        );
        // In the same place, whatever it says.
        assert_eq!(rect(&harness, "checks status"), glyph);

        let mut running = checks_of(&[], 2);
        running
            .runs
            .push(check("test", perch_github::CheckRunState::Pending));
        running.state = CheckState::Running;
        state
            .pull_request_overview
            .set(Some(Arc::new(overview(running))));
        harness.rebuild();
        harness.paint();
        assert_eq!(
            checks_say(state),
            Some((Tone::Waiting, "Checks running".into()))
        );

        // Merge is the bar's, and the checks' detail the glyph's menu:
        // none of Merge's controls under the summary.
        let summary = named(harness.root_id(), "item summary").expect("no summary");
        for gone in [
            "merge-split",
            "merge-button",
            "merge-popover",
            "merge-action",
        ] {
            assert!(named(summary, gone).is_none(), "{gone} is in the summary");
        }
    }

    /// The glyph's menu lists every check, what failed first, then what
    /// runs, then what passed, and last the checks on GitHub; a press on
    /// the glyph opens it, and a long list scrolls in it.
    #[test]
    fn the_checks_menu_lists_what_failed_first_and_opens_from_the_glyph() {
        use perch_github::CheckRunState;

        let (mut harness, _, state) = pull_request_harness();
        // Before GitHub lists them, the rollup has only its verdict.
        assert_eq!(
            checks_menu(state).outline(),
            [
                "(The list of checks hasn't loaded)",
                "—",
                "Open checks on GitHub"
            ]
        );

        let mut listed = checks(vec![
            check("build", CheckRunState::Success),
            check("clippy", CheckRunState::Failure),
            check("test", CheckRunState::Pending),
            check("docs", CheckRunState::Success),
            check("fmt", CheckRunState::TimedOut),
        ]);
        listed.runs[1].url = Some("https://github.com/slatedb/slatedb/runs/1".into());
        state
            .pull_request_overview
            .set(Some(Arc::new(overview(listed))));
        harness.rebuild();
        harness.paint();
        assert_eq!(
            checks_menu(state).outline(),
            [
                "clippy",
                "fmt",
                "test",
                "build",
                "docs",
                "—",
                "Open checks on GitHub"
            ]
        );

        let pill = rect(&harness, "checks status").center();
        harness.click(pill.x, pill.y);
        for _ in 0..3 {
            harness.rebuild();
            harness.paint();
        }
        assert!(
            crate::ui::menu::is_open(),
            "the glyph did not open its menu"
        );
        crate::ui::menu::close();
        harness.rebuild();
        harness.paint();

        // Forty checks scroll in a menu no taller than its cap.
        state
            .pull_request_overview
            .set(Some(Arc::new(overview(checks_of(&["clippy"], 40)))));
        harness.rebuild();
        harness.paint();
        let pill = rect(&harness, "checks status").center();
        harness.click(pill.x, pill.y);
        for _ in 0..4 {
            harness.rebuild();
            harness.paint();
        }
        let panel = rect(&harness, "menu-panel:0");
        assert!(
            panel.height() > 0.0 && panel.height() <= CHECKS_MENU_HEIGHT,
            "the menu at {panel:?} is not capped"
        );
        crate::ui::menu::close();
    }

    /// The review and the merge each have a glyph of their own after the
    /// checks, once the overview has come: green when the pull request
    /// is approved and merges cleanly, yellow while a review is needed,
    /// red for changes asked for or conflicts.
    #[test]
    fn the_review_and_the_merge_each_have_a_glyph_after_the_checks() {
        let (mut harness, app, state) = pull_request_harness();
        let gone = |harness: &floem::headless::HeadlessHarness, name: &str| {
            named(harness.root_id(), name).is_none()
        };
        assert!(gone(&harness, "review status"), "nothing yet");
        assert!(gone(&harness, "merge status"), "nothing yet");

        // Approved and mergeable: both green, in order after the checks.
        state
            .pull_request_overview
            .set(Some(Arc::new(overview(checks_of(&[], 1)))));
        harness.rebuild();
        harness.paint();
        assert_eq!(review_standing(state), standing(Tone::Good, "Approved"));
        assert_eq!(
            merge_standing(state),
            standing(Tone::Good, "No conflicts with main")
        );
        let line = rect(&harness, "summary line");
        let mut last = rect(&harness, "churn").x1;
        for part in ["checks status", "review status", "merge status"] {
            let r = rect(&harness, part);
            assert!(within(r, line), "{part} at {r:?} is off the line {line:?}");
            assert!(r.x0 >= last, "{part} at {r:?} is not after what is before");
            last = r.x1;
        }
        assert_eq!(last, line.x1, "the glyphs do not end the line {line:?}");
        let places = (
            rect(&harness, "review status"),
            rect(&harness, "merge status"),
        );

        let mut waiting = overview(checks_of(&[], 1));
        waiting.review_decision = Some(perch_github::ReviewDecision::ReviewRequested);
        state.pull_request_overview.set(Some(Arc::new(waiting)));
        harness.rebuild();
        harness.paint();
        assert_eq!(
            review_standing(state),
            standing(Tone::Waiting, "Review required")
        );

        let mut blocked = overview(checks_of(&[], 1));
        blocked.review_decision = Some(perch_github::ReviewDecision::ChangesRequested);
        blocked.mergeable = perch_github::MergeableState::Conflicting;
        state
            .pull_request_overview
            .set(Some(Arc::new(blocked.clone())));
        harness.rebuild();
        harness.paint();
        assert_eq!(
            review_standing(state),
            standing(Tone::Bad, "Changes requested")
        );
        assert_eq!(
            merge_standing(state),
            standing(Tone::Bad, "Conflicts with main")
        );
        // Each in its place, whatever it says.
        assert_eq!(
            (
                rect(&harness, "review status"),
                rect(&harness, "merge status")
            ),
            places
        );

        // No decision, where the repository asks for no review, and a
        // merge GitHub is still working out: neither has a glyph.
        let mut unknown = overview(checks_of(&[], 1));
        unknown.review_decision = None;
        unknown.mergeable = perch_github::MergeableState::Unknown;
        unknown.merge_state = perch_github::MergeState::Unknown;
        state.pull_request_overview.set(Some(Arc::new(unknown)));
        harness.rebuild();
        harness.paint();
        assert!(gone(&harness, "review status"));
        assert!(gone(&harness, "merge status"));
        state.pull_request_overview.set(Some(Arc::new(blocked)));
        harness.rebuild();
        harness.paint();

        // Two reviewers approved, a comment after an approval leaving it
        // standing: the Reviewers row draws both as approving.
        let review = |id: u64, user: &str, verdict: &str, at: &str| perch_github::Review {
            id,
            node_id: String::new(),
            user: user.into(),
            user_avatar_url: String::new(),
            body: String::new(),
            state: verdict.into(),
            submitted_at: Some(at.parse().unwrap()),
            html_url: String::new(),
            reactions: Default::default(),
            viewer_can_update: false,
        };
        state
            .discussion
            .set(Some(Arc::new(perch_github::Discussion {
                reviews: vec![
                    review(1, "ann", "CHANGES_REQUESTED", "2026-09-20T09:00:00Z"),
                    review(2, "ann", "APPROVED", "2026-09-20T10:00:00Z"),
                    review(3, "bob", "APPROVED", "2026-09-20T10:30:00Z"),
                    review(4, "bob", "COMMENTED", "2026-09-20T11:00:00Z"),
                ],
                ..Default::default()
            })));
        let reviewers = github_data(app, state).reviewers;
        let verdict = |login: &str| {
            reviewers
                .iter()
                .find(|(user, ..)| user == login)
                .and_then(|(.., verdict)| verdict.clone())
        };
        assert_eq!(verdict("ann").as_deref(), Some("APPROVED"), "{reviewers:?}");
        assert_eq!(verdict("bob").as_deref(), Some("APPROVED"), "{reviewers:?}");

        // Read again: what was said stays meanwhile.
        state.pull_request_overview_loading.set(true);
        state.pull_request_overview.set(None);
        harness.rebuild();
        harness.paint();
        assert!(!gone(&harness, "merge status"), "the glyph went meanwhile");

        // Closed or merged, there is nothing left to be ready for, though
        // GitHub's overview still says approved and no conflicts.
        state.pull_request_overview_loading.set(false);
        for merged in [false, true] {
            let mut pull = crate::review::header::tests::pull_request(812);
            pull.state = "closed".into();
            pull.merged = merged;
            state.pull_request.set(Some(Arc::new(pull)));
            state
                .pull_request_overview
                .set(Some(Arc::new(overview(checks_of(&[], 1)))));
            harness.rebuild();
            harness.paint();
            for name in ["review status", "merge status"] {
                assert!(
                    gone(&harness, name),
                    "{name} on a closed pull request, merged {merged}"
                );
            }
        }
    }
}
