//! A view a flag or a Watch line points at, as a popover over
//! the tour: the view's title, its diagrams, then a row for each element
//! its tables name, with the element's title and description. Perch draws
//! no table, fold or facet of the view: the rows are the elements, and a
//! click on one, or on a diagram's node, opens it.
//!
//! What an element opens depends on its kind. A pull request or an issue
//! opens in Perch, a commit on Code as its own changes, and code at its
//! lines. Anything else with a page on GitHub opens there.

use floem::prelude::*;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};
use floem::AnyView;
use perch_changescape::v4::{Element, ElementKind, ViewId};
use perch_github::ItemKind;

use crate::review::changescape::navigate;
use crate::review::changescape::prose::{self, hover_row, meta, MUTED, STRONG, TITLE};
use crate::review::changescape::tour::{self, Ctx};
use crate::theme;
use crate::ui::widgets::{clickable, open_in_browser};

/// Where a click on an element goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opens {
    /// The pull request of this number, in Perch.
    Pull(u64),
    /// The issue of this number, in Perch.
    Issue(u64),
    /// The commit of this SHA, whole or cut short, on Code.
    Commit(String),
    /// The element's lines, in Code.
    Code,
    /// Its page on GitHub.
    Url(String),
    Nothing,
}

/// Where a click on the element goes. Its kind says what it is: a pull
/// request, an issue or a commit, named by its number or its SHA; an
/// element with lines is code; anything else opens its page, if it has one.
pub fn opens(element: &Element) -> Opens {
    match element.kind {
        ElementKind::Pr => {
            if let Ok(n) = element.name.parse() {
                return Opens::Pull(n);
            }
        }
        ElementKind::Issue => {
            if let Ok(n) = element.name.parse() {
                return Opens::Issue(n);
            }
        }
        ElementKind::Commit if !element.name.is_empty() => {
            return Opens::Commit(element.name.clone())
        }
        _ => {}
    }
    let lines = element
        .refs
        .iter()
        .any(|r| r.head.is_some() || r.base.is_some() || r.whole_file);
    if lines {
        return Opens::Code;
    }
    match element.url.as_deref().filter(|url| !url.is_empty()) {
        Some(url) => Opens::Url(url.to_string()),
        None => Opens::Nothing,
    }
}

/// Open what an element id names, and put away the popover it was in.
pub fn open_element(cx: &Ctx, id: &str) {
    let Some(element) = cx.page.elements.get(id) else {
        return;
    };
    cx.state.changescape_view.set(None);
    let reference = |number: u64, kind: ItemKind, message: &str| {
        let navigation_id = cx
            .backend
            .open_reference(&cx.page.meta.repo, number, Some(kind));
        cx.app.begin_open(message, navigation_id);
    };
    match opens(element) {
        Opens::Pull(n) => reference(n, ItemKind::PullRequest, "Reading the pull request"),
        Opens::Issue(n) => reference(n, ItemKind::Issue, "Reading the issue"),
        // A commit the checkout lacks opens on GitHub.
        Opens::Commit(sha) => {
            if !cx.state.show_commit(&sha) {
                if let Some(url) = element.url.as_deref().filter(|url| !url.is_empty()) {
                    open_in_browser(url);
                }
            }
        }
        Opens::Code => navigate::open_element(cx.state, &cx.backend, &cx.page, id),
        Opens::Url(url) => open_in_browser(&url),
        Opens::Nothing => {}
    }
}

/// The popover of the view the tour's state names, over the tour. A click
/// outside it, Close, or Escape puts it away.
pub fn popover(cx: Ctx) -> impl IntoView {
    let state = cx.state;
    // Expand on a diagram here shows it over the tour, in place of this.
    floem::reactive::Effect::new(move |_| {
        if state.changescape_expanded.with(Option::is_some) {
            state.changescape_view.set(None);
        }
    });
    dyn_container(
        move || state.changescape_view.get(),
        move |id| match id {
            Some(id) => card(&cx, id),
            None => Empty::new().into_any(),
        },
    )
    .debug_name("Changescape view backdrop")
    .on_event_stop(el::Click, move |_, _| state.changescape_view.set(None))
    .style(move |s| {
        s.position(floem::style::Position::Absolute)
            .inset(0.0)
            .z_index(2)
            .justify_center()
            .items_start()
            .padding_top(48.0)
            .apply_if(state.changescape_view.with(Option::is_none), |s| s.hide())
    })
}

fn card(cx: &Ctx, id: ViewId) -> AnyView {
    let Some(view) = cx.page.view(id) else {
        return Empty::new().into_any();
    };
    let state = cx.state;
    let rows: Vec<AnyView> = cx
        .page
        .view_rows(id)
        .into_iter()
        .filter_map(|el| Some(row(cx, el, cx.page.elements.get(el)?)))
        .collect();
    let pictures: Vec<AnyView> = cx
        .page
        .view_pictures(id)
        .into_iter()
        .enumerate()
        .map(|(i, picture)| {
            let key = tour::view_picture_key(id, i);
            tour::diagram(
                cx,
                key,
                picture.caption.as_deref().or(picture.title.as_deref()),
                picture.note.as_deref(),
                picture.figure,
            )
            .into_any()
        })
        .collect();
    // A block that did not parse is named where it was, one line each.
    let dropped: Vec<AnyView> = cx
        .page
        .dropped_blocks(id)
        .map(|block| {
            let text = if block.known {
                "Perch cannot read this block."
            } else {
                "This block is in the browser page."
            };
            Label::new(text)
                .style(|s| s.font_size(theme::TEXT_META).color(theme::fg_subtle()))
                .into_any()
        })
        .collect();
    let empty = rows.is_empty() && pictures.is_empty();
    let header = Stack::horizontal((
        Stack::vertical((prose::paragraph(&view.title, TITLE), {
            let none = view.metric.is_empty();
            meta(&view.metric).style(move |s| s.selectable(true).apply_if(none, |s| s.hide()))
        }))
        .style(|s| {
            s.flex_col()
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
                .min_width(0.0)
        }),
        Container::new(meta("Close"))
            .debug_name("Changescape view close")
            .on_event_stop(el::Click, move |_, _| state.changescape_view.set(None))
            .style(|s| clickable(s.margin_left(12.0).flex_shrink(0.0_f32))),
    ))
    .style(|s| s.items_start().width_full().padding_bottom(8.0));
    let body = if empty {
        Label::new("This view names nothing Perch can open.")
            .style(|s| s.font_size(theme::TEXT_META).color(theme::fg_subtle()))
            .into_any()
    } else if rows.is_empty() {
        Empty::new().into_any()
    } else {
        Stack::vertical_from_iter(rows)
            .style(|s| s.flex_col().width_full())
            .scroll()
            .style(|s| s.max_height(420.0).width_full())
            .into_any()
    };
    let drawn = !pictures.is_empty();
    let pictures = Stack::vertical_from_iter(pictures).style(move |s| {
        s.flex_col()
            .gap(8.0)
            .width_full()
            .apply_if(drawn, |s| s.padding_bottom(8.0))
    });
    Stack::vertical((
        header,
        pictures,
        Stack::vertical_from_iter(dropped).style(|s| s.flex_col().gap(4.0).width_full()),
        body,
    ))
    .debug_name(format!("Changescape view {id}"))
    // A click inside is not a click outside.
    .on_event_stop(el::Click, |_, _| {})
    .style(|s| {
        s.flex_col()
            .width(560.0)
            .max_width_pct(90.0)
            .padding(theme::CARD_PADDING)
            .background(theme::menu_bg())
            .border(1.0)
            .border_color(theme::menu_ring())
            .border_radius(theme::CARD_RADIUS)
            .box_shadow_blur(16.0)
            .box_shadow_color(theme::menu_shadow())
    })
    .into_any()
}

/// One element of the view: its title, and its description's first line.
fn row(cx: &Ctx, id: &str, element: &Element) -> AnyView {
    let description = element
        .detail
        .as_deref()
        .and_then(|d| d.lines().next())
        .filter(|d| !d.is_empty())
        .map_or_else(
            || Empty::new().into_any(),
            |d| prose::line(d, MUTED).into_any(),
        );
    let (cx, el) = (cx.clone(), id.to_string());
    Container::new(
        Stack::vertical((prose::line(&element.title, STRONG), description))
            .style(|s| s.flex_col().width_full().min_width(0.0)),
    )
    .debug_name(format!("Changescape view row {id}"))
    .on_event_stop(el::Click, move |_, _| cx.open(&el))
    .style(|s| hover_row(s.padding_vert(6.0).padding_horiz(8.0)))
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An element of `kind` and `name`, with the given refs and url.
    fn element(kind: &str, name: &str, refs: &str, url: Option<&str>) -> Element {
        serde_json::from_value(serde_json::json!({
            "appears": [], "detail": null, "hash": null, "kind": kind, "label": "x",
            "name": name, "refs": serde_json::from_str::<serde_json::Value>(refs).unwrap(),
            "status": "context", "title": "x", "url": url }))
        .unwrap()
    }

    #[test]
    fn an_element_opens_by_its_kind() {
        let url = Some("https://github.com/a/b/pull/7");
        assert_eq!(opens(&element("pr", "7", "[]", url)), Opens::Pull(7));
        assert_eq!(opens(&element("issue", "9", "[]", None)), Opens::Issue(9));
        assert_eq!(
            opens(&element("commit", "92396a9", "[]", None)),
            Opens::Commit("92396a9".into())
        );
        let code = element(
            "function",
            "a::b",
            r#"[{ "base": null, "base_path": "a.rs", "head": { "start": 1, "end": 2 },
                  "head_path": "a.rs", "rows": [], "whole_file": false }]"#,
            None,
        );
        assert_eq!(opens(&code), Opens::Code);
        let person = element(
            "person",
            "someone",
            "[]",
            Some("https://github.com/someone"),
        );
        assert_eq!(
            opens(&person),
            Opens::Url("https://github.com/someone".into())
        );
        assert_eq!(opens(&element("package", "x", "[]", None)), Opens::Nothing);
        // A number that is not one is not a pull request.
        assert_eq!(opens(&element("pr", "x", "[]", None)), Opens::Nothing);
    }
}
