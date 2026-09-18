//! The tab strip under the header: Conversation, and Files when the item
//! has a checkout. The same strip on every item, so the view keeps its
//! shape when Start work gives an issue a checkout.
//!
//! Drawn the way the file tabs draw theirs, an accent line on the active
//! one, so the two strips read as related: this one picks what the view
//! is about, that one picks a file within it.

use floem::prelude::*;
use floem::reactive::{SignalGet, SignalWith};
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::{Decorators, Empty, Label, Stack, dyn_container};

use crate::review::model::{ItemTab, ReviewState};
use crate::theme;

pub fn strip(state: ReviewState) -> impl IntoView {
    let has_files = move || state.item.with(|i| i.as_ref().is_some_and(|i| i.checkout.is_some()));
    let conversation_count = move || {
        let said = state.discussion.with(|d| {
            d.as_ref().map(|d| d.comments.len() + d.reviews.len()).unwrap_or(0)
        }) + state.issue_comments.with(|c| c.len());
        said + state.comment_threads.with(|t| t.len())
    };
    let files_count = move || state.changed_files.with(|f| f.len());

    Stack::horizontal((
        tab(state, ItemTab::Conversation, "Conversation", conversation_count),
        dyn_container(has_files, move |has| {
            if has {
                tab(state, ItemTab::Files, "Files", files_count).into_any()
            } else {
                Empty::new().into_any()
            }
        }),
    ))
    .style(|s| {
        s.height(theme::TAB_HEIGHT)
            .width_full()
            .flex_shrink(0.0_f32)
            .items_end()
            .padding_left(14.0)
            .gap(2.0)
            .background(theme::CANVAS_SUBTLE)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

/// One tab: its name, its count in the muted colour, and the accent line
/// under the active one, sitting on the strip's bottom border.
fn tab(state: ReviewState, which: ItemTab, name: &'static str, count: impl Fn() -> usize + 'static) -> impl IntoView {
    let active = move || state.tab.get() == which;
    let active_text = active;
    let active_count = active;
    Stack::horizontal((
        Label::derived(move || name.to_string()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .font_weight(if active_text() { FontWeight::SEMI_BOLD } else { FontWeight::NORMAL })
                .color(if active_text() { theme::FG } else { theme::FG_MUTED })
                .selectable(false)
        }),
        dyn_container(count, move |n| {
            if n == 0 {
                return Empty::new().into_any();
            }
            Label::derived(move || n.to_string())
                .style(move |s| {
                    s.font_size(theme::TEXT_SECTION)
                        .font_weight(FontWeight::MEDIUM)
                        .color(if active_count() { theme::FG_MUTED } else { theme::FG_SUBTLE })
                        .background(theme::BADGE_BG)
                        .border_radius(9.0)
                        .height(18.0)
                        .padding_horiz(6.0)
                        .items_center()
                        .justify_center()
                        .margin_left(6.0)
                        .selectable(false)
                })
                .into_any()
        }),
    ))
    .on_event_stop(el::Click, move |_, _| state.show_tab(which))
    .style(move |s| {
        let base = s
            .items_center()
            .height(theme::TAB_HEIGHT)
            .padding_horiz(10.0)
            .flex_shrink(0.0_f32)
            .border_bottom(2.0)
            .hover(|s| s.cursor(CursorStyle::Pointer));
        if active() {
            base.border_color(theme::ACCENT)
        } else {
            base.border_color(floem::peniko::Color::TRANSPARENT).hover(|s| s.border_color(theme::BORDER))
        }
    })
}
