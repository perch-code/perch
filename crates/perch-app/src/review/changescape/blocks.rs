//! A view on the Changescape tab: its title, its caption, and its blocks.
//! A block of a type Perch does not draw is one quiet line that offers the
//! browser page, which draws every type.

use std::rc::Rc;
use std::sync::Arc;

use floem::prelude::*;
use floem::views::{Container, Decorators, Label, Stack};
use floem::AnyView;
use perch_changescape::page::{Block, BlockBody, View};
use perch_changescape::Page;

use crate::backend::Backend;
use crate::review::changescape::shown;
use crate::review::model::ReviewState;
use crate::theme;
use crate::ui::widgets::{clickable, underlined_link};

pub fn view(
    state: ReviewState,
    backend: Rc<Backend>,
    page: Arc<Page>,
    view: View,
) -> impl IntoView {
    let mut parts: Vec<AnyView> = Vec::new();
    parts.push(
        Stack::horizontal((
            Label::new(view.t.clone()).style(|s| {
                s.font_size(theme::TEXT_SECTION_TITLE)
                    .font_weight(theme::weight_strong())
                    .color(theme::fg())
            }),
            Label::new(view.metric.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_muted())
                    .margin_left(10.0)
            }),
        ))
        .style(|s| s.items_baseline())
        .into_any(),
    );
    if !view.s.is_empty() {
        parts.push(
            Label::new(view.s.clone())
                .style(|s| s.font_size(theme::TEXT_META).color(theme::fg_muted()))
                .into_any(),
        );
    }
    if let Some(caption) = view.narration.clone().filter(|c| !c.is_empty()) {
        parts.push(
            Label::new(caption)
                .style(|s| {
                    s.font_size(theme::TEXT_BODY)
                        .color(theme::fg())
                        .line_height(theme::PROSE_LINE_HEIGHT)
                        .margin_top(6.0)
                })
                .into_any(),
        );
    }
    for block in view.blocks {
        parts.push(block_view(state, backend.clone(), page.clone(), block).into_any());
    }
    Stack::vertical_from_iter(parts)
        .debug_name(format!("Changescape view {}", view.id))
        .style(|s| s.flex_col().gap(8.0).width_full())
}

fn block_view(
    state: ReviewState,
    backend: Rc<Backend>,
    page: Arc<Page>,
    block: Block,
) -> impl IntoView {
    let _ = (backend, page);
    let kind = block.kind.clone();
    let body = match &block.body {
        BlockBody::Other | _ => fallback(state).into_any(),
    };
    let mut parts: Vec<AnyView> = Vec::new();
    if let Some(title) = block.title.clone() {
        parts.push(
            Label::new(title)
                .style(|s| {
                    s.font_size(theme::TEXT_SECTION)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::fg_muted())
                })
                .into_any(),
        );
    }
    parts.push(body);
    if let Some(note) = block.note.clone() {
        parts.push(
            Label::new(note)
                .style(|s| s.font_size(theme::TEXT_META).color(theme::fg_subtle()))
                .into_any(),
        );
    }
    Stack::vertical_from_iter(parts)
        .debug_name(format!("Changescape block {kind}"))
        .style(|s| s.flex_col().gap(6.0).width_full().margin_top(8.0))
}

/// A block Perch does not draw: one line, and the browser page.
fn fallback(state: ReviewState) -> impl IntoView {
    Stack::horizontal((
        Label::new("This view is in the browser page.").style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_subtle())
                .selectable(false)
        }),
        Container::new(Label::new("Open it").style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::link())
                .selectable(false)
        }))
        .on_event_stop(el::Click, move |_, _| open_html(state))
        .style(|s| clickable(underlined_link(s))),
    ))
    .style(|s| s.items_center().gap(8.0))
}

/// Open the page Changescape wrote for a browser.
fn open_html(state: ReviewState) {
    let Some(path) =
        shown(state).and_then(|shown| shown.lookup().drawable().map(|build| build.html_path()))
    else {
        return;
    };
    if let Err(e) = std::process::Command::new("open").arg(&path).spawn() {
        tracing::warn!(path = %path.display(), error = %e, "could not open the changescape page");
    }
}
