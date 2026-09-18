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
use crate::ui::widgets::{avatar, sidebar_glyph, text_button};

pub fn header(
    app: AppState,
    state: ReviewState,
    on_back: impl Fn() + 'static,
    on_done: impl Fn(String) + 'static,
) -> impl IntoView {
    Stack::horizontal((
        back_button(on_back),
        icon_and_title(app, state),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        branches(state),
        churn(state),
        done_button(state, on_done),
    ))
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
    .style(|s| s.items_center().gap(9.0).min_width(0.0).flex_shrink(1.0_f32))
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
    })
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
    .style(|s| s.items_center().flex_shrink(0.0_f32))
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
