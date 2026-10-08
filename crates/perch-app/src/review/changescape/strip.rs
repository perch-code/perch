//! Code's way back to the Changescape tab: while the caret is in the head
//! lines of an element the page names, one line over the editor says
//! which, with its open threads, and links to the first section that
//! shows it. Only under the Branch scope, since the page's lines are the
//! whole branch's.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalWith};
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};
use perch_changescape::DiffSide;
use perch_git::Scope;

use crate::backend::Backend;
use crate::review::changescape::{show_section, shown, threads};
use crate::review::model::ReviewState;
use crate::theme;
use crate::ui::widgets::{clickable, underlined_link};

/// The element under the caret: its id, title and kind. Tracked.
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
    let el = shown.index.innermost(DiffSide::Head, &path, line)?;
    let element = shown.page()?.elements.get(el)?;
    Some((el.to_string(), element.t.clone(), element.k.clone()))
}

pub fn code_strip(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let under = Memo::new(move |_| under_caret(state));
    let counts = threads::counts(state);
    dyn_container(
        move || under.get(),
        move |under| {
            let Some((el, title, kind)) = under else {
                return Empty::new().into_any();
            };
            let back = el.clone();
            Stack::horizontal((
                Label::new(title).style(|s| {
                    s.font_size(theme::TEXT_META)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::fg())
                        .font_family(theme::CODE_FONT_STACK.to_string())
                }),
                Label::new(kind).style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::fg_muted())
                        .margin_left(8.0)
                        .min_width(0.0)
                        .text_ellipsis()
                }),
                threads::badge(state, backend.clone(), counts, el).style(|s| s.margin_left(8.0)),
                Empty::new().style(|s| s.flex_grow(1.0_f32)),
                Container::new(Label::new("On the changescape").style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::link())
                        .selectable(false)
                }))
                .debug_name("Changescape strip link")
                .on_event_stop(el::Click, move |_, _| {
                    let Some(shown) = shown(state) else {
                        return;
                    };
                    let section = shown
                        .page()
                        .and_then(|page| page.first_section_showing(&back))
                        .map(|section| section.id.clone());
                    if let Some(section) = section {
                        show_section(state, &section);
                    }
                })
                .style(|s| clickable(underlined_link(s))),
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
