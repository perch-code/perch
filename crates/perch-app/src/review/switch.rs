//! The switch in the middle of the bar: Conversation and Code, as Codex
//! has Summary and Code, and Changescape between them for a pull request
//! Changescape has mapped. Drawn only while the item has a checkout, since
//! without one there is no code to switch to; Start work gives an issue
//! one, and the switch comes.
//!
//! Drawn as the quiet segmented switch in Codex's toolbar: this one picks
//! what the item view is about, while Code's tabs pick a file.

use floem::prelude::*;
use floem::reactive::{Memo, SignalGet};
use floem::style::CursorStyle;
use floem::views::{dyn_container, Container, Decorators, Label, Stack};
use floem::AnyView;

use crate::review::model::{ItemTab, ReviewState};
use crate::theme;

pub fn switch(state: ReviewState) -> impl IntoView {
    // Built again when Changescape's pill comes or goes, so without it the
    // switch is the two segments it always was.
    let changescape = Memo::new(move |_| crate::review::changescape::available(state));
    dyn_container(
        move || changescape.get(),
        move |changescape| segments(state, changescape),
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

fn segments(state: ReviewState, changescape: bool) -> AnyView {
    let pills = if changescape {
        Stack::horizontal((
            segment(state, ItemTab::Conversation, "Conversation"),
            segment(state, ItemTab::Changescape, "Changescape"),
            segment(state, ItemTab::Code, "Code"),
        ))
        .into_any()
    } else {
        Stack::horizontal((
            segment(state, ItemTab::Conversation, "Conversation"),
            segment(state, ItemTab::Code, "Code"),
        ))
        .into_any()
    };
    pills
        .debug_name("item switch")
        // The bar it sits in is the window's title bar, and lets the pointer
        // through everywhere but its controls; this is one.
        .style(move |s| {
            s.items_center()
                .gap(2.0)
                .flex_shrink(0.0_f32)
                .pointer_events_auto()
                .apply_if(!state.has_files(), |s| s.hide())
        })
        .into_any()
}

/// One segment, its name alone. Selection is a soft neutral fill, not an
/// accent line, so the control stays out of the document's way.
fn segment(state: ReviewState, which: ItemTab, name: &'static str) -> impl IntoView {
    let active = move || state.tab.get() == which;
    Container::new(Label::derived(move || name.to_string()).style(move |s| {
        s.font_size(theme::TEXT_META)
            .font_weight(if active() {
                theme::weight_emphasis()
            } else {
                theme::weight_body()
            })
            .color(if active() {
                theme::fg()
            } else {
                theme::fg_muted()
            })
            .selectable(false)
    }))
    .debug_name(format!("{name} pill"))
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
            base.background(theme::canvas_subtle())
        } else {
            base.hover(|s| s.background(theme::canvas_hover()))
        }
    })
}
