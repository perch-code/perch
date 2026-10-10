//! Code's way back to the tour: while the caret is on a head line that a
//! stop's code window holds, one line over the editor names the stop and
//! its chapter, and links to it on the tour. Only under the Branch scope,
//! since the page's lines are the whole branch's.

use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalWith};
use floem::views::{dyn_container, Decorators, Empty, Label, Stack};
use perch_git::Scope;

use crate::review::changescape::{prose, shown, tour};
use crate::review::model::{ItemTab, ReviewState};
use crate::theme;

/// The stop under the caret: its id, its title, and its chapter's title.
/// Tracked.
pub(super) fn under_caret(state: ReviewState) -> Option<(String, String, String)> {
    let shown = shown(state)?;
    if state.scope.get() != Scope::Branch {
        return None;
    }
    let path = state.active.get()?;
    let display = state.cursor_line.get();
    let head = state.files.with(|files| {
        files
            .get(&path)
            .and_then(|file| file.buffer.head_line(display))
    })?;
    let line = u32::try_from(head + 1).ok()?;
    let id = shown.index.at(&path, line)?;
    let (chapter, stop) = shown.page()?.stop(id)?;
    Some((
        stop.id.clone(),
        prose::plain(&stop.title),
        prose::plain(&chapter.title),
    ))
}

pub fn code_strip(state: ReviewState) -> impl IntoView {
    let under = Memo::new(move |_| under_caret(state));
    dyn_container(
        move || under.get(),
        move |under| {
            let Some((id, title, chapter)) = under else {
                return Empty::new().into_any();
            };
            Stack::horizontal((
                Label::new(title).style(|s| {
                    s.font_size(theme::TEXT_META)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::fg())
                        .min_width(0.0)
                        .text_ellipsis()
                }),
                Label::new(chapter).style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::fg_muted())
                        .margin_left(8.0)
                        .min_width(0.0)
                        .flex_shrink(1.0_f32)
                        .text_ellipsis()
                }),
                Empty::new().style(|s| s.flex_grow(1.0_f32)),
                prose::link("On the tour", move || {
                    state.show_tab(ItemTab::Changescape);
                    tour::go_to(state, &id);
                })
                .debug_name("Changescape strip link")
                .style(|s| s.flex_shrink(0.0_f32)),
            ))
            .style(|s| {
                s.items_center()
                    .width_full()
                    .height(28.0)
                    .padding_horiz(14.0)
                    .background(theme::canvas_subtle())
                    .border_bottom(1.0)
                    .border_color(theme::border_muted())
            })
            .into_any()
        },
    )
    .debug_name("Changescape code strip")
    .style(|s| s.width_full().flex_shrink(0.0_f32))
}
