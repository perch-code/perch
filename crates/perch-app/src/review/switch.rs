//! The switch in the middle of the header: Conversation, and Files when
//! the item has a checkout. The same switch on every item, so the bar
//! keeps its shape when Start work gives an issue a checkout.
//!
//! Drawn as the quiet segmented switch in Codex's toolbar: this one picks
//! what the item view is about, while the file tabs still pick a file.

use floem::prelude::*;
use floem::reactive::{SignalGet, SignalWith};
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::{dyn_container, Decorators, Empty, Label, Stack};

use crate::review::model::{ItemTab, ReviewState};
use crate::theme;

pub fn switch(state: ReviewState) -> impl IntoView {
    let has_files = move || {
        state
            .item
            .with(|i| i.as_ref().is_some_and(|i| i.checkout.is_some()))
    };
    // The cards Conversation shows, less the description and the events:
    // the same interleave, so a review GitHub minted for a reply and the
    // timeline drops is not counted either.
    let conversation_count = move || {
        let pull = state.pull_request.get();
        let discussion = state.discussion.get();
        let issue = state.issue.get();
        let comments = state.issue_comments.get();
        let threads = state.comment_threads.with(|threads| {
            threads
                .iter()
                .filter(|thread| state.thread_visible(&thread.id))
                .cloned()
                .collect::<Vec<_>>()
        });
        crate::review::timeline::timeline(
            pull.as_deref(),
            discussion.as_deref(),
            issue.as_deref().map(|i| (i, comments.as_slice())),
            &threads,
        )
        .iter()
        .filter(|c| !matches!(c.kind(), "description" | "event"))
        .count()
    };
    let files_count = move || state.changed_files.with(|f| f.len());

    Stack::horizontal((
        tab(
            state,
            ItemTab::Conversation,
            "Conversation",
            conversation_count,
        ),
        dyn_container(has_files, move |has| {
            if has {
                tab(state, ItemTab::Files, "Files", files_count).into_any()
            } else {
                Empty::new().into_any()
            }
        }),
    ))
    .debug_name("item switch")
    // The bar it sits in is the window's title bar, and lets the pointer
    // through everywhere but its controls; this is one.
    .style(|s| {
        s.items_center()
            .gap(2.0)
            .flex_shrink(0.0_f32)
            .pointer_events_auto()
    })
}

/// One segment: its name and a quiet count. Selection is a soft neutral
/// fill, not an accent line, so the control stays out of the document's way.
fn tab(
    state: ReviewState,
    which: ItemTab,
    name: &'static str,
    count: impl Fn() -> usize + 'static,
) -> impl IntoView {
    let active = move || state.tab.get() == which;
    let active_text = active;
    let active_count = active;
    Stack::horizontal((
        Label::derived(move || name.to_string()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .font_weight(if active_text() {
                    FontWeight::SEMI_BOLD
                } else {
                    FontWeight::NORMAL
                })
                .color(if active_text() {
                    theme::FG
                } else {
                    theme::FG_MUTED
                })
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
                        .color(if active_count() {
                            theme::FG_MUTED
                        } else {
                            theme::FG_SUBTLE
                        })
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
            .height(30.0)
            .padding_horiz(12.0)
            .flex_shrink(0.0_f32)
            .border_radius(9.0)
            .hover(|s| s.cursor(CursorStyle::Pointer));
        if active() {
            base.background(theme::CANVAS_SUBTLE)
        } else {
            base.hover(|s| s.background(theme::CANVAS_HOVER))
        }
    })
}
