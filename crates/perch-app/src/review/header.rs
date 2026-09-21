//! The bar above the item view, and the strip under it.
//!
//! The bar is the window's title bar. It holds what is open, the switch
//! between Conversation and Files, and the three things a reader reaches
//! for from anywhere in an item: GitHub, the item's one action (Submit
//! review, or Start work on an issue), and the whole window. Everything
//! else about the item is in the strip: its state and number, branches,
//! checks and worktree, and what GitHub says about it: reviewers, labels,
//! assignees, milestone, linked issues.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::{dyn_container, dyn_stack, Container, Decorators, Empty, Label, Stack};
use perch_github::{CheckState, Item, ItemKind, ItemSubject, SubjectState};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::review::cards::{avatar_url_for, tip, tip_with, verdict_glyph};
use crate::review::model::{ItemTab, Phase, ReviewState};
use crate::state::AppState;
use crate::theme;
use crate::ui::items::{github_url, OpenItem};
use crate::ui::titlebar::title_bar;
use crate::ui::widgets::{
    avatar, clickable, icon_button, open_in_browser, sidebar_glyph, worktree_menu,
};

/// The bar and the strip, the same for every kind of item: a pull request,
/// an issue, or a branch of the reader's own.
pub fn header(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_item: OpenItem,
    on_archive: impl Fn(String) + 'static,
) -> impl IntoView {
    // The bar is the window's title bar, so a press on anything in it that
    // is not a control drags the window. Its three parts let the pointer
    // through, and each control in them takes it back.
    let bar = title_bar(
        Stack::horizontal((
            leading(app, state),
            crate::review::switch::switch(state),
            trailing(app, state, backend.clone()),
        ))
        .debug_name("item bar"),
    )
    // Floem bounds z-index to sibling stacking contexts. Submit review
    // opens from this bar and the review launcher from the strip, so the
    // bar sits above the strip, and the strip above the item.
    .style(|s| bar_style(s).z_index(2));
    Stack::vertical((
        bar,
        metadata_strip(app, state, backend, open_item, on_archive),
    ))
    .style(|s| s.flex_col().width_full())
}

fn bar_style(s: floem::style::Style) -> floem::style::Style {
    // The bottom border comes out of the bar's height, so a pixel of
    // padding at the top balances it: the bar's contents then centre on
    // the same line as the drawer's strip, which has no border, and the
    // drawer control stays put when the drawer shuts.
    s.height(theme::REVIEW_HEADER_HEIGHT)
        .width_full()
        .flex_shrink(0.0_f32)
        .items_center()
        .padding_top(1.0)
        .padding_horiz(theme::REVIEW_HEADER_PADDING)
        .gap(12.0)
        .background(theme::CANVAS)
        .border_bottom(1.0)
        .border_color(theme::BORDER_MUTED)
}

/// Either side of the switch. The two take the bar's room in equal
/// shares, so the switch sits at the bar's centre whatever is on either
/// side of it; the title on the left gives way first, since the controls
/// on the right hold their size.
fn side(s: floem::style::Style) -> floem::style::Style {
    s.items_center()
        .flex_grow(1.0_f32)
        .flex_basis(0.0)
        .pointer_events_none()
}

/// What is open: the lights and the drawer control while the drawer is
/// shut, then the item's state as a glyph, and its title.
fn leading(app: AppState, state: ReviewState) -> impl IntoView {
    Stack::horizontal((
        crate::ui::sidebar::lights_lane(app, theme::REVIEW_HEADER_PADDING)
            .style(move |s| s.apply_if(app.sidebar_open.get(), |s| s.hide())),
        state_glyph(state),
        Label::derived(move || title(state))
            .debug_name("item title")
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .font_weight(FontWeight::MEDIUM)
                    .color(theme::FG)
                    .text_ellipsis()
                    .min_width(0.0)
                    .margin_bottom(theme::OPTICAL_TEXT_LIFT)
            }),
    ))
    .style(|s| side(s).min_width(0.0).gap(8.0))
}

/// The pull request's or the issue's title. A branch of the reader's own
/// has no title but its name.
fn title(state: ReviewState) -> String {
    state
        .pull_request
        .with(|p| p.as_ref().map(|p| p.title.clone()))
        .or_else(|| state.issue.with(|i| i.as_ref().map(|i| i.title.clone())))
        .or_else(|| {
            state
                .item
                .with(|i| i.as_ref().map(|i| i.title().to_string()))
        })
        .unwrap_or_default()
}

/// The item's state as the sidebar's rows draw it: the pull request or
/// issue glyph in the state's colour. A branch has no state, and keeps its
/// own glyph.
fn state_glyph(state: ReviewState) -> impl IntoView {
    dyn_container(
        move || subject_state(state),
        move |s| match s {
            Some(state) => floem::views::svg(move || icons::state_icon(state))
                .style(move |s| s.size(16.0, 16.0).color(icons::state_color(state)))
                .into_any(),
            None => sidebar_glyph(SidebarIcon::Branch, theme::FG_MUTED)
                .style(|s| s.size(16.0, 16.0))
                .into_any(),
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

/// The three things reached for from anywhere in an item: GitHub, the
/// item's one action, and the whole window.
fn trailing(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    Stack::horizontal((
        github_button(state),
        primary_action(app, state, backend),
        expand_button(app),
    ))
    .style(|s| side(s).justify_end().gap(8.0))
}

/// Open the pull request or the issue on github.com. A branch of the
/// reader's own is nowhere on GitHub, and has no button.
fn github_button(state: ReviewState) -> impl IntoView {
    let url = move || state.item.with(|i| i.as_deref().and_then(github_url));
    dyn_container(url, move |url| {
        let Some(url) = url else {
            return Empty::new().into_any();
        };
        tip(
            icon_button(
                || SidebarIcon::External,
                || theme::FG_MUTED,
                move || open_in_browser(&url),
            ),
            "Open on GitHub".to_string(),
        )
        .debug_name("github button")
        .into_any()
    })
    .style(move |s| control(s).apply_if(url().is_none(), |s| s.hide()))
}

/// Submit review for a pull request, Start work for an issue. A branch of
/// the reader's own has neither, and neither does a pull request that is
/// no longer open.
fn primary_action(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let is_issue = move || state.phase.get() == Phase::Issue;
    dyn_container(is_issue, move |is_issue| {
        if is_issue {
            crate::review::issue::start_work_button(app, state, backend.clone()).into_any()
        } else {
            crate::review::submit_review::control(state, backend.clone()).into_any()
        }
    })
    .style(move |s| {
        let present = is_issue() || crate::review::submit_review::is_available(state);
        control(s).apply_if(!present, |s| s.hide())
    })
}

/// The pane to the whole window, and the drawer back.
fn expand_button(app: AppState) -> impl IntoView {
    let open = move || app.sidebar_open.get();
    tip_with(
        icon_button(
            move || {
                if open() {
                    SidebarIcon::Expand
                } else {
                    SidebarIcon::Contract
                }
            },
            || theme::FG_MUTED,
            move || app.sidebar_open.update(|open| *open = !*open),
        ),
        move || {
            if open() {
                "Hide the sidebar".to_string()
            } else {
                "Show the sidebar".to_string()
            }
        },
    )
    .debug_name("expand button")
    .style(control)
}

/// A control in the bar. The bar's sides let the pointer through so a
/// press on them drags the window; a control takes it back.
fn control(s: floem::style::Style) -> floem::style::Style {
    s.flex_shrink(0.0_f32).pointer_events_auto()
}

// ---------------------------------------------------------------------
// The strip under the bar: the item's own particulars
// ---------------------------------------------------------------------

/// How a part of the strip takes a row too narrow for all of it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Room {
    /// Keeps its size: a word or a number, no use cut short.
    Holds,
    /// Gives way, and is cut to an ellipsis: a name that is still a name
    /// with its end missing.
    GivesWay,
}

/// A part of the strip, hidden rather than empty when it has nothing, so
/// it takes no share of the row's gap.
fn slot(
    present: impl Fn() -> bool + 'static,
    room: Room,
    view: impl IntoView + 'static,
) -> impl IntoView {
    Container::new(view).style(move |s| {
        let s = line(s).apply_if(!present(), |s| s.hide());
        match room {
            Room::Holds => s.flex_shrink(0.0_f32),
            Room::GivesWay => s.flex_shrink(1.0_f32).min_width(0.0),
        }
    })
}

fn historical_review(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    dyn_container(
        move || state.historical_review.get(),
        move |review| match review {
            Some(review) => {
                let current_backend = backend.clone();
                Stack::horizontal((
                    Label::derived(move || {
                        format!("Review #{} snapshot · read-only", review.group.number)
                    })
                    .style(|s| {
                        s.font_size(theme::TEXT_META)
                            .font_weight(FontWeight::MEDIUM)
                            .color(theme::FG_MUTED)
                            .selectable(false)
                    }),
                    Label::derived(|| "Back to current".to_string())
                        .on_event_stop(el::Click, move |_, _| {
                            if let Some(item_id) = state
                                .item
                                .with_untracked(|item| item.as_ref().map(|item| item.id.clone()))
                            {
                                let navigation_id = current_backend.open_item(&item_id);
                                app.begin_item_review(
                                    state,
                                    "Reading the current files",
                                    &item_id,
                                    navigation_id,
                                );
                            }
                        })
                        .style(|s| {
                            clickable(s)
                                .height(20.0)
                                .items_center()
                                .padding_horiz(6.0)
                                .font_size(theme::TEXT_BADGE)
                                .font_weight(FontWeight::MEDIUM)
                                .color(theme::FG)
                                .background(theme::CANVAS)
                                .border_radius(theme::RADIUS)
                                .selectable(false)
                        }),
                ))
                .style(|s| {
                    s.height(24.0)
                        .items_center()
                        .gap(7.0)
                        .padding_horiz(8.0)
                        .background(theme::BADGE_BG)
                        .border_radius(theme::RADIUS)
                })
                .into_any()
            }
            None => Empty::new().into_any(),
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

/// The repository and number, as the sidebar's rows say them. A branch of
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

/// A commit as a base reads as its short hash; a branch as its name.
fn short_ref(base: &str) -> String {
    if base.len() == 40 && base.chars().all(|c| c.is_ascii_hexdigit()) {
        base[..7].to_string()
    } else {
        base.to_string()
    }
}

/// The state of what is open, as the sidebar's glyphs draw it.
fn subject_state(state: ReviewState) -> Option<SubjectState> {
    state
        .pull_request
        .with(|p| {
            p.as_ref().map(|p| {
                if p.merged {
                    SubjectState::PrMerged
                } else if !p.is_open() {
                    SubjectState::PrClosed
                } else if p.draft {
                    SubjectState::PrDraft
                } else {
                    SubjectState::PrOpen
                }
            })
        })
        .or_else(|| {
            state
                .issue
                .with(|i| i.as_ref().map(|i| crate::review::issue::state_of(i)))
        })
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

/// Open, Draft, Merged, Closed: the state's icon and its word, in the
/// state's colour on a wash of it. A branch has no state, and no pill.
fn state_pill(state: ReviewState) -> impl IntoView {
    dyn_container(
        move || subject_state(state),
        move |s| match s {
            Some(state) => {
                let color = icons::state_color(state);
                Stack::horizontal((
                    floem::views::svg(move || icons::state_icon(state))
                        .style(move |s| s.size(14.0, 14.0).color(color).margin_right(5.0)),
                    Label::derived(move || state_word(state).to_string()).style(move |s| {
                        s.font_size(theme::TEXT_SECTION)
                            .font_weight(FontWeight::SEMI_BOLD)
                            .color(color)
                            .selectable(false)
                            .margin_bottom(theme::OPTICAL_TEXT_LIFT)
                    }),
                ))
                .style(move |s| {
                    s.items_center()
                        .height(theme::STATE_PILL_HEIGHT)
                        .padding_left(7.0)
                        .padding_right(9.0)
                        .background(color.with_alpha(theme::STATE_WASH))
                        .border_radius(theme::STATE_PILL_HEIGHT / 2.0)
                        .flex_shrink(0.0_f32)
                })
                .into_any()
            }
            None => Empty::new().into_any(),
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

/// The workflow runs' verdict for the head: a green check, a red cross or
/// a muted dot. Hover names the run that failed; click opens it on
/// github.com. Absent until something has been fetched.
fn checks_pill(state: ReviewState) -> impl IntoView {
    dyn_container(
        move || state.checks.get(),
        move |checks| {
            let Some(checks) = checks else {
                return Empty::new().into_any();
            };
            let (glyph, text, failing): (floem::AnyView, &str, Option<String>) = match &checks.state
            {
                CheckState::Passing => (
                    sidebar_glyph(SidebarIcon::Check, theme::OPEN)
                        .style(|s| s.size(13.0, 13.0))
                        .into_any(),
                    "Checks passed",
                    None,
                ),
                CheckState::Failing { workflow } => (
                    sidebar_glyph(SidebarIcon::Close, theme::CLOSED)
                        .style(|s| s.size(13.0, 13.0))
                        .into_any(),
                    "Checks failed",
                    Some(workflow.clone()),
                ),
                CheckState::Running => (
                    Empty::new()
                        .style(|s| {
                            s.size(theme::EVENT_DOT, theme::EVENT_DOT)
                                .border_radius(theme::EVENT_DOT / 2.0)
                                .background(theme::FG_MUTED)
                        })
                        .into_any(),
                    "Checks running",
                    None,
                ),
                CheckState::Unknown => return Empty::new().into_any(),
            };
            let url = checks.url.clone();
            let pill = Stack::horizontal((
                Container::new(glyph).style(|s| {
                    s.size(13.0, 13.0)
                        .items_center()
                        .justify_center()
                        .margin_right(6.0)
                }),
                Label::derived(move || text.to_string()).style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::FG_MUTED)
                        .margin_bottom(theme::OPTICAL_TEXT_LIFT)
                }),
            ))
            .on_event_stop(el::Click, move |_, _| {
                if let Some(url) = &url {
                    open_in_browser(url);
                }
            })
            .style(|s| {
                clickable(s)
                    .items_center()
                    .height(22.0)
                    .padding_horiz(8.0)
                    .background(theme::CANVAS)
                    .border(1.0)
                    .border_color(theme::BORDER_MUTED)
                    .border_radius(theme::RADIUS)
                    .flex_shrink(0.0_f32)
                    .hover(|s| s.background(theme::CANVAS_HOVER))
            });
            let pill = pill.debug_name("checks pill");
            match failing {
                Some(workflow) => tip(pill, format!("{workflow} failed")).into_any(),
                None => pill.into_any(),
            }
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

/// Whether the checks pill has a verdict to show.
fn has_checks(state: ReviewState) -> bool {
    state.checks.with(|c| {
        c.as_ref()
            .is_some_and(|c| !matches!(c.state, CheckState::Unknown))
    })
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
                .color(theme::FG_MUTED)
                .background(theme::CANVAS)
                .border(1.0)
                .border_color(theme::BORDER_MUTED)
                .border_radius(theme::RADIUS)
                .padding_horiz(8.0)
                .padding_vert(2.0)
                .text_ellipsis()
                .min_width(0.0)
                .max_width(320.0)
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

/// The worktree's directory name, and a menu of ways to get there.
///
/// This is what makes the checkout a place rather than a cache: the name
/// on screen is the name in the Finder and in a shell.
fn directory(state: ReviewState, on_archive: impl Fn(String) + 'static) -> impl IntoView {
    let on_archive = std::rc::Rc::new(on_archive);
    dyn_container(
        move || state.item.get(),
        move |item| {
            let Some(checkout) = item
                .as_ref()
                .and_then(|i| i.checkout.clone())
                .filter(|c| c.on_disk)
            else {
                return Empty::new().into_any();
            };
            let name = checkout
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let path = checkout.path.clone();
            let id = item.as_ref().map(|i| i.id.clone()).unwrap_or_default();
            // Only a checkout Perch owns can be archived from here; one
            // under a root is the reader's.
            let on_archive = checkout.owned.then(|| on_archive.clone());

            Stack::horizontal((
                sidebar_glyph(SidebarIcon::Folder, theme::FG_MUTED).style(|s| {
                    s.size(13.0, 13.0)
                        .margin_right(6.0)
                        .flex_shrink(0.0_f32)
                        .pointer_events_none()
                }),
                Label::derived(move || name.clone()).style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .font_family(theme::CODE_FONT_STACK.to_string())
                        .color(theme::FG_MUTED)
                        .text_ellipsis()
                        .min_width(0.0)
                        .max_width(240.0)
                        .pointer_events_none()
                }),
                sidebar_glyph(SidebarIcon::ChevronDown, theme::FG_SUBTLE).style(|s| {
                    s.size(10.0, 10.0)
                        .margin_left(5.0)
                        .flex_shrink(0.0_f32)
                        .pointer_events_none()
                }),
            ))
            .popout_menu(move || {
                let archive = on_archive.clone().map(|on_archive| {
                    let id = id.clone();
                    std::rc::Rc::new(move || on_archive(id.clone())) as std::rc::Rc<dyn Fn()>
                });
                worktree_menu(path.clone(), archive)
            })
            .debug_name("worktree menu")
            .style(|s| {
                s.items_center()
                    .height(22.0)
                    .padding_horiz(8.0)
                    .background(theme::CANVAS)
                    .border(1.0)
                    .border_color(theme::BORDER_MUTED)
                    .border_radius(theme::RADIUS)
                    .min_width(0.0)
                    .selectable(false)
                    .hover(|s| {
                        s.background(theme::CANVAS_HOVER)
                            .cursor(CursorStyle::Pointer)
                    })
            })
            .into_any()
        },
    )
    .style(|s| s.min_width(0.0))
}

/// Why the files on screen may not be the pull request's latest.
///
/// Two cases, one shape: the pull request moved while the worktree had work
/// in it, or the pull request could not be fetched at all. Neither offers a
/// button. Reading what is on disk is the right answer today.
fn notice_text(state: ReviewState) -> Option<String> {
    if let Some(head) = state.stale_head.get() {
        let short: String = head.chars().take(7).collect();
        return Some(format!(
            "Pull request moved to {short}; showing the worktree as it is"
        ));
    }
    if state.offline.get() {
        return Some("Offline; showing the checkout as it was last left".to_string());
    }
    // Under a commit or the index the files on Files are not the
    // disk's, which is what rust-analyzer reads, so hover and
    // definition can be off by the lines that differ.
    if state.tab.get() == ItemTab::Files {
        match state.scope.get() {
            perch_git::Scope::Commit { sha } => {
                let short: String = sha.chars().take(7).collect();
                return Some(format!(
                    "Showing {short}; rust-analyzer sees the working tree"
                ));
            }
            perch_git::Scope::Staged => {
                return Some("Showing the index; rust-analyzer sees the working tree".to_string());
            }
            _ => {}
        }
    }
    None
}

fn notice(state: ReviewState) -> impl IntoView {
    dyn_container(
        move || notice_text(state),
        move |text| {
            let Some(text) = text else {
                return Empty::new().into_any();
            };
            Stack::horizontal((
                sidebar_glyph(SidebarIcon::Warning, theme::NOTICE_FG)
                    .style(|s| s.size(13.0, 13.0).margin_right(6.0).flex_shrink(0.0_f32)),
                Label::derived(move || text.clone()).style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::NOTICE_FG)
                        .text_ellipsis()
                        .min_width(0.0)
                        .margin_bottom(theme::OPTICAL_TEXT_LIFT)
                }),
            ))
            .style(|s| {
                s.items_center()
                    .height(22.0)
                    .padding_horiz(9.0)
                    .max_width(380.0)
                    .min_width(0.0)
                    .background(theme::NOTICE_BG)
                    .border(1.0)
                    .border_color(theme::NOTICE_BORDER)
                    .border_radius(theme::RADIUS)
                    .selectable(false)
            })
            .into_any()
        },
    )
    // Gives way, as the branches and the worktree name do: it is a
    // sentence, and its start says what it is about.
    .style(move |s| {
        line(s)
            .min_width(0.0)
            .flex_shrink(1.0_f32)
            .apply_if(notice_text(state).is_none(), |s| s.hide())
    })
}

/// Lines added and removed. On Files it is the changed list's, whatever
/// scope made it; on Conversation it is the branch's, from the pull
/// request or the checkout.
fn churn_of(state: ReviewState) -> Option<(u32, u32)> {
    match state.tab.get() {
        ItemTab::Files => Some(state.changed_files.with(|files| {
            files
                .iter()
                .fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions))
        })),
        ItemTab::Conversation => state
            .pull_request
            .with(|p| p.as_ref().map(|p| (p.additions, p.deletions)))
            .or_else(|| {
                state.item.with(|i| {
                    i.as_ref()
                        .and_then(|i| i.checkout.as_ref())
                        .and_then(|c| c.churn)
                })
            }),
    }
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
        line(s)
            .flex_shrink(0.0_f32)
            .apply_if(!has_churn.get(), |s| s.hide())
    })
}

fn churn_labels(
    additions: impl Fn() -> u32 + 'static,
    deletions: impl Fn() -> u32 + 'static,
) -> impl IntoView {
    Stack::horizontal((
        Label::derived(move || format!("+{}", additions())).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(theme::STATUS_ADDED)
        }),
        Label::derived(move || format!("-{}", deletions())).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(theme::STATUS_DELETED)
                .margin_left(6.0)
        }),
    ))
    .style(|s| s.items_center().flex_shrink(0.0_f32).selectable(false))
}

/// The agent review launcher, at the strip's right end. An issue has no
/// files to review until Start work gives it some, and then it is a
/// branch.
fn launcher(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let reviewable = move || state.phase.get() != Phase::Issue;
    dyn_container(reviewable, move |reviewable| {
        if reviewable {
            crate::review::review_launch::control(app, state, backend.clone()).into_any()
        } else {
            Empty::new().into_any()
        }
    })
    .style(move |s| {
        line(s)
            .flex_shrink(0.0_f32)
            .apply_if(!reviewable(), |s| s.hide())
    })
}

// ---------------------------------------------------------------------
// The strip under the bar: what GitHub says
// ---------------------------------------------------------------------

/// What the strip shows, read once from the signals so the strip rebuilds
/// when any of it changes and not when the signals are merely written.
#[derive(Debug, Clone, PartialEq, Default)]
struct Strip {
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

impl Strip {
    fn is_empty(&self) -> bool {
        self.reviewers.is_empty()
            && self.labels.is_empty()
            && self.assignees.is_empty()
            && self.milestone.is_none()
            && self.closes.is_empty()
            && self.in_perch.is_empty()
    }
}

fn strip_data(app: AppState, state: ReviewState) -> Strip {
    let mut strip = Strip::default();
    if let Some(pr) = state.pull_request.get() {
        for r in &pr.requested_reviewers {
            strip
                .reviewers
                .push((r.login.clone(), r.avatar_url.clone(), None));
        }
        state.discussion.with(|d| {
            let Some(d) = d else { return };
            // The last verdict each reviewer gave. The author's own
            // comments are not reviews of it, and a pending review is
            // not given yet.
            let mut reviews: Vec<_> = d
                .reviews
                .iter()
                .filter(|r| r.state != "PENDING" && r.user != pr.author_login)
                .collect();
            reviews.sort_by_key(|r| r.submitted_at);
            for r in reviews {
                match strip
                    .reviewers
                    .iter_mut()
                    .find(|(login, ..)| *login == r.user)
                {
                    Some(mark) => mark.2 = Some(r.state.clone()),
                    None => strip.reviewers.push((
                        r.user.clone(),
                        avatar_url_for(state, &r.user),
                        Some(r.state.clone()),
                    )),
                }
            }
            strip.closes = closes(&d.body);
        });
        strip.milestone = pr.milestone.clone();
        strip.labels = pr.labels.clone();
        strip.assignees = pr.assignees.clone();
    }
    if let Some(issue) = state.issue.get() {
        strip.labels = issue.labels.clone();
        strip.assignees = issue.assignees.clone();
    }
    let id = state.item.with(|i| i.as_ref().map(|i| i.id.clone()));
    strip.in_perch = app.items.with(|all| {
        all.iter()
            .filter(|i| i.from_item_id.is_some() && i.from_item_id == id)
            .cloned()
            .collect()
    });
    strip
}

/// The issue numbers a description says it closes: `closes #12`, `Fixes
/// #3`, `resolved #7`, in any case, anywhere in the text. Only this
/// repository's; `owner/repo#N` names another and is left alone.
pub fn closes(body: &str) -> Vec<u64> {
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

/// The item's state, repository, branches, churn, checks and worktree;
/// what GitHub says about it: reviewers with their verdicts, labels,
/// assignees, the milestone, the issues closed, and what Perch started
/// from here; and the review launcher at the right. Each part is absent
/// when it has nothing, and the strip with them; a pull request or an
/// issue always has its state and number to show, and a branch of the
/// reader's own its branches.
///
/// Two lines: the item's own particulars with the launcher at their
/// right, and what GitHub says about it under them, which wraps when a
/// line is too narrow for all of it, so nothing is pushed past the edge.
fn metadata_strip(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_item: OpenItem,
    on_archive: impl Fn(String) + 'static,
) -> impl IntoView {
    let data = Memo::new(move |_| strip_data(app, state));
    let repo = move || {
        state
            .item
            .with(|i| i.as_deref().map(repo_line).unwrap_or_default())
    };
    let shown = Memo::new(move |_| {
        subject_state(state).is_some()
            || !repo().is_empty()
            || !branches_text(state).is_empty()
            || state.historical_review.with(Option::is_some)
            || notice_text(state).is_some()
            || data.with(|d| !d.is_empty())
    });
    let particulars = Stack::horizontal((
        notice(state),
        slot(
            move || state.historical_review.with(Option::is_some),
            Room::Holds,
            historical_review(app, state, backend.clone()),
        ),
        slot(
            move || subject_state(state).is_some(),
            Room::Holds,
            state_pill(state),
        ),
        slot(
            move || !repo().is_empty(),
            Room::Holds,
            Label::derived(repo).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::FG_MUTED)
                    .selectable(false)
            }),
        ),
        slot(
            move || !branches_text(state).is_empty(),
            Room::GivesWay,
            branches(state),
        ),
        churn(state),
        slot(move || has_checks(state), Room::Holds, checks_pill(state)),
        slot(
            move || checkout_on_disk(state).is_some(),
            Room::GivesWay,
            directory(state, on_archive),
        ),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        launcher(app, state, backend.clone()),
    ))
    .debug_name("item particulars")
    // Above the line under it, for the launcher's popover.
    .style(|s| line(s).width_full().min_width(0.0).z_index(1));
    Stack::vertical((
        particulars,
        github_parts(app, state, backend, open_item, data),
    ))
    .debug_name("item strip")
    .style(move |s| {
        s.flex_col()
            .width_full()
            .flex_shrink(0.0_f32)
            .padding_horiz(theme::REVIEW_HEADER_PADDING)
            .background(theme::CANVAS_SUBTLE)
            .border_bottom(1.0)
            .border_color(theme::BORDER_MUTED)
            // Above the item, for the launcher's popover.
            .z_index(1)
            .apply_if(!shown.get(), |s| s.hide())
    })
}

/// A line of the strip, and each part on it: `METADATA_LINE` tall with
/// its content centred, the parts `METADATA_GAP` apart.
fn line(s: floem::style::Style) -> floem::style::Style {
    s.height(theme::METADATA_LINE)
        .items_center()
        .col_gap(theme::METADATA_GAP)
}

/// Reviewers with their verdicts, labels, assignees, the milestone, the
/// issues closed, and what Perch started from here.
fn github_parts(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_item: OpenItem,
    data: Memo<Strip>,
) -> impl IntoView {
    dyn_container(
        move || data.get(),
        move |strip| {
            if strip.is_empty() {
                return Empty::new().into_any();
            }
            let repo = state
                .item
                .with(|i| i.as_ref().map(|i| i.repo_full_name.clone()))
                .unwrap_or_default();
            let (backend, open_item) = (backend.clone(), open_item.clone());
            Stack::horizontal((
                part(
                    "Reviewers",
                    !strip.reviewers.is_empty(),
                    reviewers(app, strip.reviewers),
                ),
                part(
                    "",
                    !strip.labels.is_empty(),
                    chips(strip.labels, |l| label_chip(l.name, l.color)),
                ),
                part(
                    "Assignees",
                    !strip.assignees.is_empty(),
                    assignees(app, strip.assignees),
                ),
                part(
                    "Milestone",
                    strip.milestone.is_some(),
                    milestone(strip.milestone.unwrap_or_default()),
                ),
                part(
                    "Closes",
                    !strip.closes.is_empty(),
                    chips(strip.closes, move |n| {
                        let (backend, repo) = (backend.clone(), repo.clone());
                        let open = move || {
                            // By number rather than by row: the issue may
                            // have no item yet, and the backend makes one.
                            let navigation_id = backend.open_subject(&repo, ItemKind::Issue, n);
                            app.begin_open("Reading the issue", navigation_id);
                        };
                        chip(
                            format!("#{n}"),
                            theme::BADGE_BG,
                            theme::FG_MUTED,
                            Some(Rc::new(open)),
                        )
                    }),
                ),
                part(
                    "In Perch",
                    !strip.in_perch.is_empty(),
                    chips(strip.in_perch, move |item| {
                        let open = open_item.clone();
                        let target = item.clone();
                        let name = item.title().to_string();
                        chip(
                            name,
                            theme::BADGE_BG,
                            theme::FG_MUTED,
                            Some(Rc::new(move || open(&target))),
                        )
                    }),
                ),
            ))
            // Each part is a line tall and holds its size; when the line
            // is too narrow for all of them the rest wrap onto another.
            .style(|s| {
                s.flex_wrap(floem::taffy::FlexWrap::Wrap)
                    .items_start()
                    .col_gap(theme::METADATA_GAP)
                    .width_full()
                    .min_width(0.0)
            })
            .into_any()
        },
    )
    .style(move |s| {
        s.width_full()
            .apply_if(data.with(|d| d.is_empty()), |s| s.hide())
    })
}

/// One part of the strip: its name in the subtle colour, then its items.
/// Nothing at all when it has none: hidden rather than empty, so it takes
/// no share of the row's gap.
fn part(name: &'static str, present: bool, items: impl IntoView + 'static) -> impl IntoView {
    if !present {
        return Empty::new().style(|s| s.hide()).into_any();
    }
    Stack::horizontal((
        if name.is_empty() {
            Empty::new().into_any()
        } else {
            Label::derived(move || name.to_string())
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::FG_SUBTLE)
                        .margin_right(8.0)
                        .selectable(false)
                })
                .into_any()
        },
        items,
    ))
    .style(|s| line(s).flex_shrink(0.0_f32))
    .into_any()
}

/// Avatar, login, and the verdict's glyph after it for a review given.
fn reviewers(app: AppState, reviewers: Vec<(String, String, Option<String>)>) -> impl IntoView {
    dyn_stack(
        move || reviewers.clone(),
        |(login, ..)| login.clone(),
        move |(login, url, verdict)| {
            let seed = login.clone();
            Stack::horizontal((
                avatar(app, move || url.clone(), theme::METADATA_AVATAR, seed)
                    .style(|s| s.margin_right(5.0)),
                Label::derived(move || login.clone())
                    .style(|s| s.font_size(theme::TEXT_BADGE).color(theme::FG_MUTED)),
                match verdict {
                    Some(state) => {
                        let review = perch_github::Review {
                            id: 0,
                            user: String::new(),
                            body: String::new(),
                            state,
                            submitted_at: None,
                            html_url: String::new(),
                        };
                        let (icon, color) = verdict_glyph(Some(&review));
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
    .style(|s| s.items_center().gap(10.0))
}

fn assignees(app: AppState, logins: Vec<String>) -> impl IntoView {
    dyn_stack(
        move || logins.clone(),
        |l| l.clone(),
        move |login| {
            let seed = login.clone();
            Stack::horizontal((
                avatar(app, String::new, theme::METADATA_AVATAR, seed)
                    .style(|s| s.margin_right(5.0)),
                Label::derived(move || login.clone())
                    .style(|s| s.font_size(theme::TEXT_BADGE).color(theme::FG_MUTED)),
            ))
            .style(|s| s.items_center())
        },
    )
    .style(|s| s.items_center().gap(10.0))
}

fn milestone(name: String) -> impl IntoView {
    Stack::horizontal((
        sidebar_glyph(SidebarIcon::Milestone, theme::FG_SUBTLE)
            .style(|s| s.size(12.0, 12.0).margin_right(5.0)),
        Label::derived(move || name.clone()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::FG_MUTED)
                .text_ellipsis()
                .max_width(240.0)
        }),
    ))
    .style(|s| s.items_center())
}

/// A row of chips, one per item.
fn chips<T: Clone + std::hash::Hash + Eq + 'static, V: IntoView + 'static>(
    items: Vec<T>,
    chip: impl Fn(T) -> V + 'static,
) -> impl IntoView {
    dyn_stack(move || items.clone(), |i| i.clone(), chip).style(|s| s.items_center().gap(4.0))
}

/// A small pill, clickable when it leads somewhere.
fn chip(
    text: String,
    tint: floem::peniko::Color,
    ink: floem::peniko::Color,
    on_click: Option<Rc<dyn Fn()>>,
) -> impl IntoView {
    let clicks = on_click.is_some();
    Container::new(Label::derived(move || text.clone()).style(move |s| {
        s.font_size(theme::TEXT_SECTION)
            .font_weight(FontWeight::MEDIUM)
            .color(ink)
            .selectable(false)
            .text_ellipsis()
            .max_width(200.0)
            .margin_bottom(theme::OPTICAL_TEXT_LIFT)
    }))
    .on_event_stop(el::Click, move |_, _| {
        if let Some(on_click) = &on_click {
            on_click();
        }
    })
    .style(move |s| {
        let base = s
            .height(theme::CHIP_HEIGHT)
            .padding_horiz(7.0)
            .items_center()
            .justify_center()
            .background(tint)
            .border_radius(theme::CHIP_HEIGHT / 2.0)
            .flex_shrink(0.0_f32);
        if clicks {
            clickable(base).hover(|s| s.background(theme::BORDER_MUTED))
        } else {
            base.selectable(false)
        }
    })
}

/// A label as GitHub draws it: its colour as a tint behind darker text.
fn label_chip(name: String, color: String) -> impl IntoView {
    let (tint, ink) = label_colors(&color);
    chip(name, tint, ink, None)
}

/// GitHub's label colour: a light wash of it behind, and a darker cut of
/// it for the text, so every label reads on white.
fn label_colors(hex: &str) -> (floem::peniko::Color, floem::peniko::Color) {
    if hex.len() != 6 {
        return (theme::BADGE_BG, theme::FG_MUTED);
    }
    let parse = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0) as f32;
    let (r, g, b) = (parse(0), parse(2), parse(4));
    let wash = |c: f32| (255.0 - (255.0 - c) * 0.18) as u8;
    let ink = |c: f32| (c * 0.55) as u8;
    (
        floem::peniko::Color::from_rgb8(wash(r), wash(g), wash(b)),
        floem::peniko::Color::from_rgb8(ink(r), ink(g), ink(b)),
    )
}

/// Shown while a checkout is being prepared.
pub fn preparing_card(
    message: impl Fn() -> String + 'static,
    percent: impl Fn() -> Option<u8> + 'static,
) -> impl IntoView {
    Container::new(
        Stack::vertical((
            Container::new(sidebar_glyph(SidebarIcon::Refresh, theme::ACCENT)).style(|s| {
                s.size(44.0, 44.0)
                    .border_radius(22.0)
                    .background(theme::ACCENT_SUBTLE)
                    .items_center()
                    .justify_center()
                    .margin_bottom(14.0)
            }),
            Label::derived(|| "Preparing the checkout".to_string()).style(|s| {
                s.font_size(15.0)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(theme::FG)
                    .margin_bottom(6.0)
            }),
            Label::derived(message).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .color(theme::FG_MUTED)
                    .margin_bottom(14.0)
            }),
            progress_bar(percent),
        ))
        .style(|s| s.items_center().width(340.0)),
    )
    .style(|s| s.size_full().items_center().justify_center())
}

fn progress_bar(percent: impl Fn() -> Option<u8> + 'static) -> impl IntoView {
    Container::new(Empty::new().style(move |s| {
        // Without a percentage the bar sits at a third, which still reads as
        // motion rather than as a stalled zero.
        let fraction = percent().map(|p| p as f64 / 100.0).unwrap_or(0.33);
        s.height_full()
            .width_pct(fraction * 100.0)
            .background(theme::ACCENT)
            .border_radius(3.0)
    }))
    .style(|s| {
        s.width_full()
            .height(5.0)
            .background(theme::BADGE_BG)
            .border_radius(3.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::kurbo::Rect;

    /// The first view whose debug name has `name` in it, depth first.
    fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().contains(name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
    }

    fn rect(harness: &HeadlessHarness, name: &str) -> Rect {
        named(harness.root_id(), name)
            .unwrap_or_else(|| panic!("no view named {name}"))
            .get_visual_rect_no_clip()
    }

    fn checkout() -> perch_github::ItemCheckout {
        perch_github::ItemCheckout {
            branch: "perch/pr-812".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: "/tmp/slatedb__slatedb__perch-pr-812".into(),
            owned: true,
            on_disk: true,
            churn: Some((504, 135)),
            last_seen_head: None,
        }
    }

    /// The whole window, 1400 by 900 with the drawer open, on an open
    /// pull request with a checkout and passing checks.
    fn pull_request_harness() -> (HeadlessHarness, AppState, ReviewState) {
        pull_request_harness_with(pull_request(812))
    }

    fn pull_request_harness_with(
        pull: perch_github::PullRequest,
    ) -> (HeadlessHarness, AppState, ReviewState) {
        let app = AppState::new();
        let state = ReviewState::new();
        let now = "2026-09-20T12:00:00Z".parse().unwrap();
        let mut item = Item::new("slatedb/slatedb", now);
        item.id = "pr".into();
        item.subject = Some(ItemSubject::PullRequest(pull.clone()));
        item.checkout = Some(checkout());
        state.item.set(Some(Arc::new(item)));
        state.pull_request.set(Some(Arc::new(pull)));
        state.checks.set(Some(Arc::new(perch_github::Checks {
            state: CheckState::Passing,
            url: None,
            runs: Vec::new(),
        })));
        state.ready.set(true);
        state.phase.set(Phase::Ready);
        // A focused editor asks the window for IME, which the headless
        // window has none of.
        state.composer_autofocus.set(false);
        state.submit_review_autofocus.set(false);
        let root = TestRoot::new();
        let view = crate::ui::app_view(app, state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        (harness, app, state)
    }

    /// The same window on an open issue with nothing on it: no labels, no
    /// assignees, nothing started from it.
    fn issue_harness() -> (HeadlessHarness, AppState, ReviewState) {
        let app = AppState::new();
        let state = ReviewState::new();
        let now = "2026-09-20T12:00:00Z".parse().unwrap();
        let issue = perch_github::Issue {
            number: 400,
            title: "Trace reads".into(),
            body: String::new(),
            state: "open".into(),
            state_reason: None,
            author_login: "u".into(),
            author_avatar_url: String::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            comments: 0,
            created_at: None,
            updated_at: now,
            repo_full_name: "slatedb/slatedb".into(),
        };
        let mut item = Item::new("slatedb/slatedb", now);
        item.id = "issue".into();
        item.subject = Some(ItemSubject::Issue(issue.clone()));
        state.item.set(Some(Arc::new(item)));
        state.issue.set(Some(Arc::new(issue)));
        state.ready.set(true);
        state.phase.set(Phase::Issue);
        state.composer_autofocus.set(false);
        let root = TestRoot::new();
        let view = crate::ui::app_view(app, state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        (harness, app, state)
    }

    fn within(inner: Rect, outer: Rect) -> bool {
        inner.x0 >= outer.x0 && inner.x1 <= outer.x1 && inner.y0 >= outer.y0 && inner.y1 <= outer.y1
    }

    /// The drawer control moves from the drawer's strip to the item bar
    /// when the drawer shuts. The bar has a border along its bottom and
    /// the strip has none, and the control must not move up a pixel with
    /// the room the border takes.
    #[test]
    fn the_drawer_control_keeps_its_line_when_the_drawer_shuts() {
        let (mut harness, app, _) = pull_request_harness();
        let in_strip = rect(&harness, "drawer toggle");

        app.sidebar_open.set(false);
        harness.rebuild();
        harness.paint();
        let in_bar = rect(&harness, "drawer toggle");
        assert_eq!(
            (in_bar.y0, in_bar.y1),
            (in_strip.y0, in_strip.y1),
            "the control at {in_bar:?} is not on the strip's line {in_strip:?}"
        );
    }

    #[test]
    fn the_bar_is_the_title_the_switch_and_three_controls_in_a_row() {
        let (harness, ..) = pull_request_harness();
        let bar = rect(&harness, "item bar");
        assert_eq!(bar.height(), theme::REVIEW_HEADER_HEIGHT);
        assert_eq!(
            bar.x0,
            theme::SIDEBAR_WIDTH,
            "the bar starts at the drawer's edge"
        );
        assert_eq!(bar.y0, 0.0, "and at the window's top: it is the title bar");

        // Left to right: the title, the switch at the centre, then GitHub,
        // Submit review and the expand control against the right edge.
        let title = rect(&harness, "item title");
        let switch = rect(&harness, "item switch");
        let github = rect(&harness, "github button");
        let submit = rect(&harness, "submit-review-control");
        let expand = rect(&harness, "expand button");
        for (name, r) in [
            ("title", title),
            ("switch", switch),
            ("github", github),
            ("submit", submit),
            ("expand", expand),
        ] {
            assert!(within(r, bar), "{name} at {r:?} is not in the bar {bar:?}");
            assert!(r.width() > 0.0, "{name} has no width");
        }
        assert!(title.x1 <= switch.x0);
        assert!(switch.x1 <= github.x0);
        assert!(github.x1 <= submit.x0);
        assert!(submit.x1 <= expand.x0);
        assert!(
            (switch.center().x - bar.center().x).abs() <= 1.0,
            "the switch at {switch:?} is off the bar's centre {}",
            bar.center().x
        );
        assert_eq!(
            expand.x1,
            bar.x1 - theme::REVIEW_HEADER_PADDING,
            "expand sits against the bar's padding"
        );
        // With the drawer open there is no lane before the title: the
        // state glyph and its gap are all that is.
        assert_eq!(title.x0, bar.x0 + theme::REVIEW_HEADER_PADDING + 16.0 + 8.0);
    }

    #[test]
    fn a_long_title_gives_way_and_the_switch_stays_centred() {
        let mut pull = pull_request(812);
        pull.title = "Add tracing spans to every read path, ".repeat(8);
        let (harness, ..) = pull_request_harness_with(pull);
        let bar = rect(&harness, "item bar");
        let title = rect(&harness, "item title");
        let switch = rect(&harness, "item switch");
        assert!(
            title.width() < bar.width() / 2.0,
            "the title at {title:?} did not give way"
        );
        assert!(title.x1 <= switch.x0);
        assert!((switch.center().x - bar.center().x).abs() <= 1.0);
        assert_eq!(
            rect(&harness, "expand button").x1,
            bar.x1 - theme::REVIEW_HEADER_PADDING
        );
    }

    #[test]
    fn submit_review_opens_from_the_bar_over_the_strip_and_the_item() {
        let (mut harness, _, state) = pull_request_harness();
        let control = rect(&harness, "submit-review-control").center();
        harness.click(control.x, control.y);
        harness.rebuild();
        harness.paint();
        assert!(state.submit_review_open.get(), "the click did not open it");

        let popover = named(harness.root_id(), "submit-review-popover").unwrap();
        let bounds = popover.get_visual_rect_no_clip();
        let x = bounds.x0 + 2.0;
        let strip_y = theme::REVIEW_HEADER_HEIGHT + theme::METADATA_HEIGHT / 2.0;
        let content_y = theme::REVIEW_HEADER_HEIGHT + theme::METADATA_HEIGHT + 10.0;
        assert!(
            bounds.y0 < strip_y && bounds.y1 > content_y,
            "popover at {bounds:?}"
        );
        for (layer, y) in [("strip", strip_y), ("item", content_y)] {
            let target = harness
                .element_id_at(x, y)
                .unwrap_or_else(|| panic!("nothing under the {layer}"))
                .owning_id();
            let mut id = target;
            let inside = loop {
                if id == popover {
                    break true;
                }
                match id.parent() {
                    Some(parent) => id = parent,
                    None => break false,
                }
            };
            assert!(inside, "the {layer} painted over the Submit review popover");
        }
    }

    #[test]
    fn what_github_says_wraps_under_the_particulars_rather_than_push_the_launcher_out() {
        let mut pull = pull_request(812);
        pull.requested_reviewers = ["agavra", "criccomini"]
            .into_iter()
            .map(|login| perch_github::Reviewer {
                login: login.into(),
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
        let (harness, ..) = pull_request_harness_with(pull);
        let strip = rect(&harness, "item strip");
        assert_eq!(
            strip.height(),
            theme::METADATA_HEIGHT + theme::METADATA_LINE,
            "the strip at {strip:?} is not two lines"
        );
        // The particulars hold the first line, the launcher at its right
        // end and inside the padding, and nothing gave way to nothing.
        let launcher = rect(&harness, "review-launch-control");
        assert_eq!(launcher.x1, strip.x1 - theme::REVIEW_HEADER_PADDING);
        assert!(launcher.y1 <= strip.y0 + theme::METADATA_LINE);
        let (plain, ..) = pull_request_harness();
        for name in ["branches", "worktree menu", "checks pill"] {
            assert_eq!(
                rect(&harness, name).width(),
                rect(&plain, name).width(),
                "{name} gave way"
            );
        }
        // What GitHub says is the second line, in full.
        let second = strip.y0 + theme::METADATA_LINE;
        for login in ["agavra", "criccomini", "cadonna"] {
            let r = rect(&harness, login);
            assert!(r.y0 >= second && r.x1 <= strip.x1, "{login} at {r:?}");
        }
    }

    #[test]
    fn what_left_the_bar_is_in_the_strip_under_it() {
        let (harness, ..) = pull_request_harness();
        let strip = rect(&harness, "item strip");
        assert_eq!(strip.y0, theme::REVIEW_HEADER_HEIGHT);
        assert_eq!(strip.height(), theme::METADATA_HEIGHT);
        for name in [
            "branches",
            "checks pill",
            "worktree menu",
            "review-launch-control",
        ] {
            let r = rect(&harness, name);
            assert!(
                within(r, strip),
                "{name} at {r:?} is not in the strip {strip:?}"
            );
            assert!(r.width() > 0.0, "{name} has no width");
        }
        // The launcher is at the right end, where the bar had it, and
        // inside the padding: the row is a little narrow for everything
        // at this width, and the branches and the worktree name give way.
        let launcher = rect(&harness, "review-launch-control");
        assert_eq!(launcher.x1, strip.x1 - theme::REVIEW_HEADER_PADDING);
        assert!(rect(&harness, "branches").width() > 60.0);
        assert!(rect(&harness, "worktree menu").width() > 60.0);
    }

    #[test]
    fn the_bar_drags_beside_the_title_and_its_controls_keep_their_clicks() {
        let (mut harness, app, _) = pull_request_harness();
        let bar = named(harness.root_id(), "item bar").unwrap();
        let title = rect(&harness, "item title");
        let switch = rect(&harness, "item switch");

        // The room between the title and the switch is the bar's own, so a
        // press there is a drag and not a click on anything.
        let x = (title.x1 + switch.x0) / 2.0;
        let y = rect(&harness, "item bar").center().y;
        let hit = harness.element_id_at(x, y).expect("nothing under the bar");
        assert_eq!(hit.owning_id(), bar, "the bar's side took the press");

        // The expand control shuts the drawer: the bar then runs from the
        // window's edge, with the lights lane and the drawer control
        // before the title, and the control brings the drawer back.
        let expand = rect(&harness, "expand button").center();
        harness.click(expand.x, expand.y);
        harness.rebuild();
        harness.paint();
        assert!(!app.sidebar_open.get());
        let bar = rect(&harness, "item bar");
        assert_eq!(bar.x0, 0.0);
        let toggle = rect(&harness, "drawer toggle");
        assert!(
            within(toggle, bar),
            "the drawer toggle at {toggle:?} is not in the bar"
        );
        assert_eq!(toggle.x0, theme::ACTIVITY_RAIL_WIDTH, "after the lights");
        assert!(toggle.x1 <= rect(&harness, "item title").x0);
        // Beside the lights is the bar's own surface too.
        let hit = harness
            .element_id_at(theme::ACTIVITY_RAIL_WIDTH / 2.0, bar.center().y)
            .expect("nothing under the lane");
        assert_eq!(
            hit.owning_id(),
            named(harness.root_id(), "item bar").unwrap(),
            "the lane took the press"
        );
        harness.click(toggle.center().x, toggle.center().y);
        harness.rebuild();
        harness.paint();
        assert!(app.sidebar_open.get());
        assert_eq!(rect(&harness, "item bar").x0, theme::SIDEBAR_WIDTH);
    }

    #[test]
    fn an_issue_has_start_work_where_a_pull_request_has_submit_review() {
        let (harness, ..) = issue_harness();
        let bar = rect(&harness, "item bar");
        assert!(named(harness.root_id(), "submit-review-control").is_none());
        assert!(named(harness.root_id(), "review-launch-control").is_none());
        let github = rect(&harness, "github button");
        let start = rect(&harness, "start work button");
        let expand = rect(&harness, "expand button");
        for (name, r) in [
            ("github", github),
            ("start work", start),
            ("expand", expand),
        ] {
            assert!(within(r, bar), "{name} at {r:?} is not in the bar {bar:?}");
            assert!(r.width() > 0.0, "{name} has no width");
        }
        assert!(github.x1 <= start.x0 && start.x1 <= expand.x0);
        // With nothing else to say about it, the strip is its state and
        // its number, and nothing an issue cannot have.
        let strip = rect(&harness, "item strip");
        assert_eq!(
            strip.height(),
            theme::METADATA_HEIGHT,
            "the strip at {strip:?}"
        );
        // The branches label is built and hidden; the worktree menu is not
        // built at all without a checkout.
        assert_eq!(rect(&harness, "branches").width(), 0.0, "branches is drawn");
        assert!(named(harness.root_id(), "worktree menu").is_none());
    }

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
        });
        item
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
}
