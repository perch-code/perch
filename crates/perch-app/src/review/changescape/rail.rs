//! The rail on the Changescape tab's left: the page's sections, in the
//! page's order and with the page's labels. A section with no views is one
//! quiet line with the reason it has none. The `code` section is Perch's
//! own Code, so its entry switches there.

use std::sync::Arc;

use floem::prelude::*;
use floem::views::{Container, Decorators, Label, Stack};
use perch_changescape::page::PageBlock;
use perch_changescape::v3::{Block, FacetOption, Section};
use perch_changescape::LoadedPage;

use crate::review::changescape::{select, selected_section, with_facet};
use crate::review::model::{ItemTab, ReviewState};
use crate::theme;
use crate::ui::widgets::clickable;

/// The section Changescape keeps for the diff, which Perch shows in Code.
const CODE_SECTION: &str = "code";

const RAIL_WIDTH: f64 = 220.0;

pub fn rail(state: ReviewState, page: Arc<LoadedPage>) -> impl IntoView {
    let entries: Vec<_> = page
        .sections
        .iter()
        .map(|section| entry(state, page.clone(), section.clone()).into_any())
        .collect();
    Stack::vertical_from_iter(entries)
        .debug_name("Changescape rail")
        .style(|s| {
            s.flex_col()
                .gap(2.0)
                .width(RAIL_WIDTH)
                .flex_shrink(0.0_f32)
                .height_full()
                .padding(12.0)
                .background(theme::rail_bg())
                .border_right(1.0)
                .border_color(theme::border_muted())
        })
        .scroll()
        .style(|s| s.flex_shrink(0.0_f32).height_full())
}

fn entry(state: ReviewState, page: Arc<LoadedPage>, section: Section) -> impl IntoView {
    let id = section.id.clone();
    let debug = format!("Changescape rail {id}");
    if id == CODE_SECTION {
        return Container::new(Label::new(section.label.clone()).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::link())
                .selectable(false)
        }))
        .debug_name(debug)
        .on_event_stop(el::Click, move |_, _| state.show_tab(ItemTab::Code))
        .style(|s| clickable(row(s)).hover(|s| s.background(theme::canvas_hover())))
        .into_any();
    }
    if page.section_views(&section).is_empty() {
        let reason = section
            .reason
            .clone()
            .filter(|reason| !reason.is_empty())
            .unwrap_or_else(|| section.empty.clone());
        return Stack::vertical((
            Label::new(section.label.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_subtle())
                    .selectable(false)
            }),
            Label::new(reason).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .selectable(false)
                    .width_full()
                    .min_width(0.0)
            }),
        ))
        .debug_name(debug)
        .style(|s| row(s).flex_col().gap(2.0))
        .into_any();
    }
    let active = {
        let page = page.clone();
        let id = id.clone();
        move || selected_section(state, &page).as_deref() == Some(id.as_str())
    };
    let label_active = active.clone();
    let dimmed = {
        let (page, section) = (page.clone(), section.clone());
        move || {
            with_facet(state, &page, |option| {
                option.is_some_and(|o| !touches(&page, &section, o))
            })
        }
    };
    Container::new(Label::new(section.label.clone()).style(move |s| {
        let on = label_active();
        s.font_size(theme::TEXT_META)
            .font_weight(if on {
                theme::weight_emphasis()
            } else {
                theme::weight_body()
            })
            .color(if dimmed() {
                theme::fg_subtle()
            } else if on {
                theme::fg()
            } else {
                theme::fg_muted()
            })
            .selectable(false)
    }))
    .debug_name(debug)
    .on_event_stop(el::Click, move |_, _| select(state, &id))
    .style(move |s| {
        let s = clickable(row(s));
        if active() {
            s.background(theme::canvas_subtle())
        } else {
            s.hover(|s| s.background(theme::canvas_hover()))
        }
    })
    .into_any()
}

/// Whether a section's views have a table row or a diagram node in the
/// facet option.
fn touches(page: &LoadedPage, section: &Section, option: &FacetOption) -> bool {
    section.views.iter().any(|view| {
        page.blocks(view).iter().any(|block| match block {
            PageBlock::Known(Block::Table { rows, .. }) => rows
                .iter()
                .filter_map(|row| row.el.as_ref())
                .any(|el| option.els.contains(el)),
            PageBlock::Known(Block::Svg { hot, .. }) => {
                hot.values().any(|el| option.els.contains(el))
            }
            _ => false,
        })
    })
}

fn row(s: floem::style::Style) -> floem::style::Style {
    s.width_full()
        .padding_horiz(10.0)
        .padding_vert(6.0)
        .border_radius(theme::RADIUS)
}
