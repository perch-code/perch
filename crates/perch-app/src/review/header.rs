//! The bar above the review view: which pull request this is.

use floem::prelude::*;
use floem::reactive::{SignalGet, SignalWith};
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::{Container, Decorators, Empty, Label, Stack, dyn_container};
use perch_github::SubjectState;

use crate::icons::{self, SidebarIcon};
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::titlebar::title_bar;
use crate::ui::widgets::{avatar, sidebar_glyph, text_button, worktree_menu};

pub fn header(
    app: AppState,
    state: ReviewState,
    on_back: impl Fn() + 'static,
    on_done: impl Fn(String) + 'static,
    on_archive: impl Fn(String) + 'static,
) -> impl IntoView {
    // The bar is the window's title bar, so a press on anything in it that
    // is not a control drags the window. Those parts are `pointer_events_none`
    // so the press falls through to the bar.
    title_bar(Stack::horizontal((
        back_button(on_back),
        icon_and_title(app, state),
        Empty::new().style(|s| s.flex_grow(1.0_f32).pointer_events_none()),
        notice(state),
        branches(state),
        directory(state, on_archive),
        churn(state),
        done_button(state, on_done),
    )))
    .style(|s| {
        s.height(theme::REVIEW_HEADER_HEIGHT)
            .width_full()
            .flex_shrink(0.0_f32)
            .items_center()
            .padding_horiz(14.0)
            .gap(12.0)
            .background(theme::CANVAS)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

fn back_button(on_back: impl Fn() + 'static) -> impl IntoView {
    // A drawn chevron rather than a typed one: a glyph is centred by its
    // line box, not by its own shape, so it never sits square in a button.
    Container::new(
        sidebar_glyph(SidebarIcon::ChevronLeft, theme::FG_MUTED)
            .style(|s| s.size(14.0, 14.0)),
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
            .hover(|s| s.background(theme::CANVAS_SUBTLE).cursor(CursorStyle::Pointer))
    })
}

/// Finish with a pull request and dismiss the notification behind it.
///
/// The whole point of the feature: read a review, be done with it, land on
/// the next one. Absent when the pull request was opened by some route that
/// had no notification, such as the launch hook.
fn done_button(state: ReviewState, on_done: impl Fn(String) + 'static) -> impl IntoView {
    // The container rebuilds the button whenever the thread changes, and each
    // rebuild needs its own handle on the callback.
    let on_done = std::rc::Rc::new(on_done);
    dyn_container(
        move || state.thread_id.get(),
        move |thread_id| {
            let Some(thread_id) = thread_id else {
                return Empty::new().into_any();
            };
            let on_done = on_done.clone();
            text_button(
                || "Done".to_string(),
                SidebarIcon::Check,
                move || on_done(thread_id.clone()),
            )
            .into_any()
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

fn icon_and_title(app: AppState, state: ReviewState) -> impl IntoView {
    let title = move || {
        state
            .pull_request
            .with(|p| p.as_ref().map(|p| p.title.clone()))
            .unwrap_or_default()
    };
    let subtitle = move || {
        state
            .pull_request
            .with(|p| {
                p.as_ref()
                    .map(|p| format!("{}  #{}", p.repo_full_name, p.number))
            })
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
        state_icon(state),
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
        .style(|s| s.flex_col().min_width(0.0).justify_center().margin_right(10.0)),
        dyn_container(author, move |login| {
            avatar(app, author_avatar, 20.0, login).into_any()
        })
        .style(|s| s.size(20.0, 20.0).flex_shrink(0.0_f32)),
    ))
    .style(|s| {
        s.items_center()
            .gap(9.0)
            .min_width(0.0)
            .flex_shrink(1.0_f32)
            .pointer_events_none()
    })
}

fn state_icon(state: ReviewState) -> impl IntoView {
    let subject_state = move || {
        state.pull_request.with(|p| {
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
    };

    dyn_container(subject_state, move |s| match s {
        Some(state) => floem::views::svg(move || icons::state_icon(state))
            .style(move |s| s.size(18.0, 18.0).color(icons::state_color(state)))
            .into_any(),
        None => Empty::new().into_any(),
    })
    .style(|s| s.size(18.0, 18.0).flex_shrink(0.0_f32))
}

fn branches(state: ReviewState) -> impl IntoView {
    let text = move || {
        state
            .pull_request
            .with(|p| {
                p.as_ref()
                    .map(|p| format!("{}  \u{2190}  {}", p.base_ref, p.head_ref))
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
            let Some(checkout) = item.as_ref().and_then(|i| i.checkout.clone()).filter(|c| c.on_disk) else {
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
                    .hover(|s| s.background(theme::CANVAS_HOVER).cursor(CursorStyle::Pointer))
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
            return Some(format!("Pull request moved to {short}; showing the worktree as it is"));
        }
        if state.offline.get() {
            return Some("Offline; showing the checkout as it was last left".to_string());
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

fn churn(state: ReviewState) -> impl IntoView {
    let additions = move || {
        state
            .pull_request
            .with(|p| p.as_ref().map(|p| p.additions).unwrap_or(0))
    };
    let deletions = move || {
        state
            .pull_request
            .with(|p| p.as_ref().map(|p| p.deletions).unwrap_or(0))
    };

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

/// Shown while a checkout is being prepared.
pub fn preparing_card(message: impl Fn() -> String + 'static, percent: impl Fn() -> Option<u8> + 'static) -> impl IntoView {
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
                s.font_size(theme::TEXT_BODY).color(theme::FG_MUTED).margin_bottom(14.0)
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
