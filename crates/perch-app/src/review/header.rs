//! The bar above the item view: which pull request, issue or branch this
//! is, its state, its checks, and under the bar a strip of what GitHub
//! says about it: reviewers, labels, assignees, milestone, linked issues.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalWith};
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::{dyn_container, dyn_stack, Container, Decorators, Empty, Label, Stack};
use perch_github::{CheckState, Item, ItemKind, SubjectState};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::review::cards::{avatar_url_for, tip, verdict_glyph};
use crate::review::model::{ItemTab, ReviewState};
use crate::state::AppState;
use crate::theme;
use crate::ui::items::OpenItem;
use crate::ui::titlebar::title_bar;
use crate::ui::widgets::{avatar, clickable, open_in_browser, sidebar_glyph, worktree_menu};

pub fn header(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_item: OpenItem,
    on_back: impl Fn() + 'static,
    on_archive: impl Fn(String) + 'static,
) -> impl IntoView {
    // The bar is the window's title bar, so a press on anything in it that
    // is not a control drags the window. Those parts are `pointer_events_none`
    // so the press falls through to the bar.
    let bar = title_bar(Stack::horizontal((
        back_button(on_back),
        icon_and_title(app, state),
        Empty::new().style(|s| s.flex_grow(1.0_f32).pointer_events_none()),
        notice(state),
        historical_review(app, state, backend.clone()),
        crate::review::review_launch::control(app, state, backend.clone()),
        branches(state),
        checks_pill(state),
        directory(state, on_archive),
        churn(state),
        item_marks(state, backend.clone()),
    )))
    // Floem bounds z-index to sibling stacking contexts. The review launcher
    // lives inside this bar, so the bar itself must sit above the metadata
    // strip that follows it.
    .style(|s| bar_style(s).z_index(1));
    Stack::vertical((bar, metadata_strip(app, state, backend, open_item)))
        .style(|s| s.flex_col().width_full())
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
                    s.height(28.0)
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

/// The bar above an issue: back, the issue, and Save. The same bar
/// as a review's, so the window drags from it the same way.
pub fn issue_header(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_item: OpenItem,
) -> impl IntoView {
    let back_backend = backend.clone();
    let on_back = move || {
        let navigation_id = back_backend.close_review();
        app.begin_close(state, navigation_id);
    };
    let mark_backend = backend.clone();
    let bar = title_bar(Stack::horizontal((
        back_button(on_back),
        crate::review::issue::issue_header(app, state, backend.clone()),
        item_marks(state, mark_backend),
    )))
    .style(bar_style);
    Stack::vertical((bar, metadata_strip(app, state, backend, open_item)))
        .style(|s| s.flex_col().width_full())
}

fn bar_style(s: floem::style::Style) -> floem::style::Style {
    s.height(theme::REVIEW_HEADER_HEIGHT)
        .width_full()
        .flex_shrink(0.0_f32)
        .items_center()
        .padding_horiz(14.0)
        .gap(12.0)
        .background(theme::CANVAS)
        .border_bottom(1.0)
        .border_color(theme::BORDER_MUTED)
}

/// Save for the open item, as the sidebar rows have.
fn item_marks(state: ReviewState, backend: std::rc::Rc<crate::backend::Backend>) -> impl IntoView {
    dyn_container(
        move || {
            state
                .item
                .with(|i| i.as_ref().map(|i| (i.id.clone(), i.saved)))
        },
        move |item| {
            let Some((id, saved)) = item else {
                return Empty::new().into_any();
            };
            let backend = backend.clone();
            crate::ui::widgets::icon_button(
                move || {
                    if saved {
                        SidebarIcon::BookmarkFilled
                    } else {
                        SidebarIcon::Bookmark
                    }
                },
                move || {
                    if saved {
                        theme::ACCENT
                    } else {
                        theme::FG_MUTED
                    }
                },
                move || backend.mark_saved(&id, !saved),
            )
            .into_any()
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

fn back_button(on_back: impl Fn() + 'static) -> impl IntoView {
    // A drawn chevron rather than a typed one: a glyph is centred by its
    // line box, not by its own shape, so it never sits square in a button.
    Container::new(
        sidebar_glyph(SidebarIcon::ChevronLeft, theme::FG_MUTED).style(|s| s.size(14.0, 14.0)),
    )
    .on_event_stop(el::Click, move |_, _| on_back())
    .style(|s| {
        s.size(28.0, 28.0)
            .flex_shrink(0.0_f32)
            .items_center()
            .justify_center()
            .border_radius(theme::RADIUS)
            .border(1.0)
            .border_color(theme::BORDER)
            .background(theme::CANVAS)
            .hover(|s| {
                s.background(theme::CANVAS_SUBTLE)
                    .cursor(CursorStyle::Pointer)
            })
    })
}

fn icon_and_title(app: AppState, state: ReviewState) -> impl IntoView {
    // A branch of the reader's own has no title but its name, and the
    // subtitle says where it is.
    let title = move || {
        state
            .pull_request
            .with(|p| p.as_ref().map(|p| p.title.clone()))
            .or_else(|| {
                state
                    .item
                    .with(|i| i.as_ref().map(|i| i.title().to_string()))
            })
            .unwrap_or_default()
    };
    let subtitle = move || {
        state
            .pull_request
            .with(|p| {
                p.as_ref()
                    .map(|p| format!("{}  #{}", p.repo_full_name, p.number))
            })
            .or_else(|| state.item.with(|i| i.as_ref().map(|i| branch_subtitle(i))))
            .unwrap_or_default()
    };
    let author = move || {
        state
            .pull_request
            .with(|p| p.as_ref().map(|p| p.author_login.clone()))
            .unwrap_or_default()
    };
    let author_avatar = move || {
        state
            .pull_request
            .with(|p| p.as_ref().map(|p| p.author_avatar_url.clone()))
            .unwrap_or_default()
    };

    Stack::horizontal((
        state_pill(state),
        Stack::vertical((
            Label::derived(subtitle).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG_SUBTLE)
                    .text_ellipsis()
                    .max_width_full()
            }),
            Label::derived(title).style(|s| {
                s.font_size(theme::TEXT_TITLE)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(theme::FG)
                    .text_ellipsis()
                    .max_width_full()
            }),
        ))
        .style(|s| {
            s.flex_col()
                .min_width(0.0)
                .justify_center()
                .margin_right(10.0)
        }),
        dyn_container(author, move |login| {
            if login.is_empty() {
                Empty::new().into_any()
            } else {
                avatar(app, author_avatar, 20.0, login).into_any()
            }
        })
        .style(|s| s.flex_shrink(0.0_f32)),
    ))
    .style(|s| {
        s.items_center()
            .gap(9.0)
            .min_width(0.0)
            .flex_shrink(1.0_f32)
            .pointer_events_none()
    })
}

/// The subtitle of a branch item: the repository's slug and what the
/// branch is against. A repository with no GitHub remote has no slug worth
/// a line, so its subtitle is the base alone; the sidebar's heading
/// already says which repository it is.
fn branch_subtitle(item: &Item) -> String {
    let base = item
        .checkout
        .as_ref()
        .map(|c| format!("against {}", short_ref(&c.base_ref)));
    if perch_github::is_local(&item.repo_full_name) {
        return base.unwrap_or_default();
    }
    match base {
        Some(base) => format!("{}  \u{00b7}  {base}", item.repo_full_name),
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
/// state's colour on a wash of it. A branch has no state, and keeps its
/// glyph.
pub fn state_pill(state: ReviewState) -> impl IntoView {
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
                        .pointer_events_none()
                })
                .into_any()
            }
            None => sidebar_glyph(SidebarIcon::Branch, theme::FG_MUTED)
                .style(|s| s.size(18.0, 18.0).pointer_events_none())
                .into_any(),
        },
    )
    .style(|s| s.flex_shrink(0.0_f32).pointer_events_none())
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
                    .height(24.0)
                    .padding_horiz(8.0)
                    .background(theme::CANVAS_SUBTLE)
                    .border(1.0)
                    .border_color(theme::BORDER_MUTED)
                    .border_radius(theme::RADIUS)
                    .flex_shrink(0.0_f32)
                    .hover(|s| s.background(theme::CANVAS_HOVER))
            });
            match failing {
                Some(workflow) => tip(pill, format!("{workflow} failed")).into_any(),
                None => pill.into_any(),
            }
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

fn branches(state: ReviewState) -> impl IntoView {
    let text = move || {
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
    };

    Label::derived(text).style(|s| {
        s.font_size(theme::TEXT_BADGE)
            .font_family(theme::CODE_FONT_STACK.to_string())
            .color(theme::FG_MUTED)
            .background(theme::CANVAS_SUBTLE)
            .border(1.0)
            .border_color(theme::BORDER_MUTED)
            .border_radius(theme::RADIUS)
            .padding_horiz(8.0)
            .padding_vert(3.0)
            .flex_shrink(0.0_f32)
            .text_ellipsis()
            .max_width(320.0)
            .pointer_events_none()
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
                sidebar_glyph(SidebarIcon::Folder, theme::FG_MUTED)
                    .style(|s| s.size(13.0, 13.0).margin_right(6.0).pointer_events_none()),
                Label::derived(move || name.clone()).style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .font_family(theme::CODE_FONT_STACK.to_string())
                        .color(theme::FG_MUTED)
                        .text_ellipsis()
                        .max_width(240.0)
                        .pointer_events_none()
                }),
                sidebar_glyph(SidebarIcon::ChevronDown, theme::FG_SUBTLE)
                    .style(|s| s.size(10.0, 10.0).margin_left(5.0).pointer_events_none()),
            ))
            .popout_menu(move || {
                let archive = on_archive.clone().map(|on_archive| {
                    let id = id.clone();
                    std::rc::Rc::new(move || on_archive(id.clone())) as std::rc::Rc<dyn Fn()>
                });
                worktree_menu(path.clone(), archive)
            })
            .style(|s| {
                s.items_center()
                    .height(24.0)
                    .padding_horiz(8.0)
                    .background(theme::CANVAS_SUBTLE)
                    .border(1.0)
                    .border_color(theme::BORDER_MUTED)
                    .border_radius(theme::RADIUS)
                    .flex_shrink(0.0_f32)
                    .selectable(false)
                    .hover(|s| {
                        s.background(theme::CANVAS_HOVER)
                            .cursor(CursorStyle::Pointer)
                    })
            })
            .into_any()
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

/// Why the files on screen may not be the pull request's latest.
///
/// Two cases, one shape: the pull request moved while the worktree had work
/// in it, or the pull request could not be fetched at all. Neither offers a
/// button. Reading what is on disk is the right answer today.
fn notice(state: ReviewState) -> impl IntoView {
    let text = move || {
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
                    return Some(
                        "Showing the index; rust-analyzer sees the working tree".to_string(),
                    );
                }
                _ => {}
            }
        }
        None
    };

    dyn_container(text, move |text| {
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
                .height(24.0)
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
    })
    .style(|s| s.min_width(0.0).flex_shrink(1.0_f32).pointer_events_none())
}

/// Lines added and removed. On Files it is the changed list's, whatever
/// scope made it; on Conversation it is the branch's, from the pull
/// request or the checkout.
fn churn(state: ReviewState) -> impl IntoView {
    let churn = move || -> Option<(u32, u32)> {
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
    };
    let has_churn = Memo::new(move |_| churn().is_some());

    dyn_container(
        move || has_churn.get(),
        move |has| {
            if has {
                churn_labels(
                    move || churn().map(|c| c.0).unwrap_or(0),
                    move || churn().map(|c| c.1).unwrap_or(0),
                )
                .into_any()
            } else {
                Empty::new().into_any()
            }
        },
    )
}

fn churn_labels(
    additions: impl Fn() -> u32 + 'static,
    deletions: impl Fn() -> u32 + 'static,
) -> impl IntoView {
    Stack::horizontal((
        Label::derived(move || format!("+{}", additions())).style(|s| {
            s.font_size(theme::TEXT_META)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(theme::STATUS_ADDED)
        }),
        Label::derived(move || format!("-{}", deletions())).style(|s| {
            s.font_size(theme::TEXT_META)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(theme::STATUS_DELETED)
                .margin_left(6.0)
        }),
    ))
    .style(|s| s.items_center().flex_shrink(0.0_f32).pointer_events_none())
}

// ---------------------------------------------------------------------
// The strip under the bar
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

/// Reviewers with their verdicts, labels, assignees, the milestone, the
/// issues closed, and what Perch started from here. Each part is absent
/// when empty, and the strip with them.
fn metadata_strip(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_item: OpenItem,
) -> impl IntoView {
    let data = Memo::new(move |_| strip_data(app, state));
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
                            let navigation_id =
                                backend.open_subject(&repo, ItemKind::Issue, n);
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
            .style(|s| {
                s.items_center()
                    .width_full()
                    .min_width(0.0)
                    .height(theme::METADATA_HEIGHT)
                    .flex_shrink(0.0_f32)
                    .padding_horiz(14.0)
                    .gap(theme::METADATA_GAP)
                    .background(theme::CANVAS_SUBTLE)
                    .border_bottom(1.0)
                    .border_color(theme::BORDER_MUTED)
            })
            .into_any()
        },
    )
    .style(|s| s.width_full())
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
    .style(|s| s.items_center().flex_shrink(0.0_f32))
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

    fn branch_item(repo: &str, base_ref: &str) -> Item {
        let mut item = Item::new(repo, "2026-09-18T09:59:00Z".parse().unwrap());
        item.checkout = Some(perch_github::ItemCheckout {
            branch: "fence".into(),
            base_ref: base_ref.into(),
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
    fn a_branch_subtitle_names_the_slug_unless_the_repository_is_local() {
        assert_eq!(
            branch_subtitle(&branch_item("slatedb/slatedb", "main")),
            "slatedb/slatedb  \u{00b7}  against main"
        );
        assert_eq!(
            branch_subtitle(&branch_item("local/notes", "main")),
            "against main"
        );
        assert_eq!(
            branch_subtitle(&branch_item("Local/notes", "main")),
            "against main",
            "the owner in any case"
        );
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            branch_subtitle(&branch_item("local/notes", sha)),
            "against 0123456"
        );
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
