//! Show Call Hierarchy: the panel under the editor that lists who calls a
//! function, or what it calls, as a tree the reader opens a row at a time.
//!
//! The tree is flat with depths, the way the sidebar's lists are, so
//! `dyn_stack` draws it. The root row is the function, with its path and
//! line; under it, one row per caller with the file and line of the call
//! and the call's line of text, the callee emphasised the way the
//! references rows emphasise the symbol. A chevron expands a row, which
//! asks the server for that row's calls: one request per expansion, and
//! the tree goes as deep as the reader takes it. A row that repeats a
//! function open above it, a recursive call, expands to nothing and says
//! so. A toggle in the header turns the tree the other way.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Effect, Memo, SignalGet, SignalUpdate, SignalWith};
use floem::views::{
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, ScrollExt, Stack,
};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::{Direction, Hierarchy, HierarchyAsk, Located, ReviewState, Symbol};
use crate::review::references::{self, close_button, line_text, panel_frame};
use crate::theme;
use crate::ui::widgets::{clickable, sidebar_glyph};

// ---------------------------------------------------------------------
// Asking
// ---------------------------------------------------------------------

/// Ask what a symbol is for a call hierarchy. The panel opens when the
/// server names a function, as its callers; on a struct, a type or a
/// local the server names nothing and nothing opens. Nothing while the
/// server is not there.
pub fn show(state: ReviewState, backend: &Backend, symbol: &Symbol) {
    if !state
        .lsp_status
        .with_untracked(|s| s.usable_for(&symbol.path))
    {
        return;
    }
    let token = state.next_token();
    state.hierarchy_ask.set(Some(HierarchyAsk {
        token,
        symbol: symbol.text.clone(),
    }));
    backend.prepare_call_hierarchy(
        token,
        &symbol.path,
        symbol.line as u32,
        symbol.column as u32,
    );
}

/// `Shift-Alt-H`: the call hierarchy of the symbol at the caret.
pub fn show_at_cursor(state: ReviewState, backend: &Backend) {
    if let Some(symbol) = references::symbol_at_cursor(state) {
        show(state, backend, &symbol);
    }
}

// ---------------------------------------------------------------------
// The tree, shaped for the panel
// ---------------------------------------------------------------------

/// One row of the panel's list.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Row {
    /// A function: the root, or a caller or callee under one.
    Item {
        at: usize,
        depth: usize,
        name: String,
        /// `path:line` of the call, or of the function for the root.
        place: String,
        /// The call's line of text, empty for the root.
        text: String,
        emphasis: String,
        expanded: bool,
        pending: bool,
        repeats: bool,
    },
    /// A line under an expanded row with nothing under it: no callers,
    /// or a recursive call, or the calls still on their way.
    Notice {
        at: usize,
        depth: usize,
        text: String,
    },
}

/// Where a row's place reads: a worktree file by its path, an outside
/// file by its name and where it is from.
fn place_label(site: &Located) -> String {
    match &site.outside {
        Some(label) => {
            let name = site.path.rsplit('/').next().unwrap_or(&site.path);
            format!("{name}:{} \u{00b7} {label}", site.line)
        }
        None => format!("{}:{}", site.path, site.line),
    }
}

pub(crate) fn rows(hierarchy: &Hierarchy) -> Vec<Row> {
    let mut out = Vec::with_capacity(hierarchy.rows.len() * 2);
    for (at, row) in hierarchy.rows.iter().enumerate() {
        let site = row
            .site
            .clone()
            .unwrap_or_else(|| hierarchy.root_place.clone());
        out.push(Row::Item {
            at,
            depth: row.depth,
            name: row.item.name.clone(),
            place: place_label(&site),
            text: row
                .site
                .as_ref()
                .map(|s| s.text.clone())
                .unwrap_or_default(),
            emphasis: row.emphasis.clone(),
            expanded: row.expanded,
            pending: row.pending.is_some(),
            repeats: row.repeats,
        });
        // What an open row with nothing under it says.
        let has_children = hierarchy
            .rows
            .get(at + 1)
            .is_some_and(|next| next.depth > row.depth);
        let notice = if row.pending.is_some() {
            Some("Looking\u{2026}".to_string())
        } else if row.expanded && !has_children {
            Some(if row.repeats {
                "Already open above.".to_string()
            } else {
                hierarchy.direction.none(row.failed.as_deref())
            })
        } else {
            None
        };
        if let Some(text) = notice {
            out.push(Row::Notice {
                at,
                depth: row.depth + 1,
                text,
            });
        }
    }
    out
}

/// How far a row is set in for its depth.
fn indent(depth: usize) -> f64 {
    10.0 + depth as f64 * 16.0
}

// ---------------------------------------------------------------------
// The panel
// ---------------------------------------------------------------------

/// The panel under the editor, while there is a tree to show. It also
/// sends the asks the tree queued, since it holds the backend and the
/// events that queue them do not.
pub fn panel(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    {
        let backend = backend.clone();
        Effect::new(move |_| {
            let asks = state.pending_calls.get();
            if asks.is_empty() {
                return;
            }
            state.pending_calls.set(Vec::new());
            for (token, item, direction) in asks {
                backend.calls(token, item, direction);
            }
        });
    }
    // Keyed on whether there is a panel, not on what is in it: the answer
    // landing must not rebuild the handle under a drag.
    let open = Memo::new(move |_| state.hierarchy.with(Option::is_some));
    dyn_container(
        move || open.get(),
        move |open| {
            if open {
                panel_frame(state, header(state), tree(state)).into_any()
            } else {
                Empty::new().into_any()
            }
        },
    )
    .style(|s| s.width_full().flex_shrink(0.0_f32))
}

/// "Callers of `run`", the direction toggle, and the close glyph.
fn header(state: ReviewState) -> impl IntoView {
    let heading = move || {
        state.hierarchy.with(|h| {
            h.as_ref()
                .map(|h| h.direction.heading().to_string())
                .unwrap_or_default()
        })
    };
    let symbol = move || {
        state
            .hierarchy
            .with(|h| h.as_ref().map(|h| h.root.name.clone()).unwrap_or_default())
    };

    Stack::horizontal((
        Label::derived(heading).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .selectable(false)
                .flex_shrink(0.0_f32)
        }),
        Label::derived(symbol).style(|s| {
            s.font_family(theme::CODE_FONT_STACK.to_string())
                .font_size(theme::TEXT_BADGE)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
                .margin_left(5.0)
                .text_ellipsis()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .selectable(false)
        }),
        toggle(state),
        close_button(move || state.close_panel()).style(|s| s.margin_left(8.0)),
    ))
}

/// Callers | Callees: the direction, as two pills with the chosen one
/// filled, the way the layout tiles read.
fn toggle(state: ReviewState) -> impl IntoView {
    Stack::horizontal((
        toggle_pill(state, Direction::Callers, "Callers"),
        toggle_pill(state, Direction::Callees, "Callees"),
    ))
    .style(|s| {
        s.items_center()
            .padding(2.0)
            .gap(2.0)
            .border_radius(theme::RADIUS)
            .background(theme::badge_bg())
            .flex_shrink(0.0_f32)
    })
}

fn toggle_pill(state: ReviewState, direction: Direction, text: &'static str) -> impl IntoView {
    let active = move || {
        state
            .hierarchy
            .with(|h| h.as_ref().is_some_and(|h| h.direction == direction))
    };
    Label::derived(move || text.to_string())
        .on_event_stop(el::Click, move |_, _| state.turn_hierarchy(direction))
        .style(move |s| {
            let s = clickable(s)
                .font_size(theme::TEXT_SECTION)
                .font_weight(theme::weight_emphasis())
                .padding_horiz(8.0)
                .height(theme::ORIGIN_CHIP_HEIGHT + 2.0)
                .items_center()
                .border_radius(theme::RADIUS - 1.0);
            if active() {
                s.background(theme::canvas()).color(theme::fg())
            } else {
                s.color(theme::fg_muted()).hover(|s| s.color(theme::fg()))
            }
        })
}

fn tree(state: ReviewState) -> impl IntoView {
    dyn_stack(
        move || {
            state
                .hierarchy
                .with(|h| h.as_ref().map(|h| rows(h)).unwrap_or_default())
        },
        |row| row.clone(),
        move |row| match row {
            Row::Notice { depth, text, .. } => notice(depth, text).into_any(),
            Row::Item {
                at,
                depth,
                name,
                place,
                text,
                emphasis,
                expanded,
                pending,
                repeats,
            } => item_row(
                state, at, depth, name, place, text, emphasis, expanded, pending, repeats,
            )
            .into_any(),
        },
    )
    .style(|s| s.flex_col().width_full().padding_vert(4.0))
    .scroll()
    .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0))
}

fn notice(depth: usize, text: String) -> impl IntoView {
    Label::derived(move || text.clone()).style(move |s| {
        s.font_size(theme::TEXT_META)
            .color(theme::fg_subtle())
            .height(theme::REFERENCES_ROW_HEIGHT)
            .items_center()
            .padding_left(indent(depth) + theme::CHEVRON_SLOT + 6.0)
            .padding_right(12.0)
            .selectable(false)
    })
}

/// One function: the chevron that opens it, its name, where the call is,
/// and the call's line with the callee emphasised. Click the row and the
/// file opens at the call; click the chevron and the row opens.
#[allow(clippy::too_many_arguments)]
fn item_row(
    state: ReviewState,
    at: usize,
    depth: usize,
    name: String,
    place: String,
    text: String,
    emphasis: String,
    expanded: bool,
    pending: bool,
    repeats: bool,
) -> impl IntoView {
    let is_root = depth == 0;
    Stack::horizontal((
        chevron(state, at, expanded, pending),
        Label::derived(move || name.clone()).style(move |s| {
            s.font_family(theme::CODE_FONT_STACK.to_string())
                .font_size(theme::code_font_size())
                .font_weight(if is_root {
                    theme::weight_strong()
                } else {
                    theme::weight_emphasis()
                })
                .color(if repeats {
                    theme::fg_muted()
                } else {
                    theme::fg()
                })
                .flex_shrink(0.0_f32)
                .selectable(false)
        }),
        Label::derived(move || place.clone()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg_subtle())
                .margin_left(8.0)
                .flex_shrink(0.0_f32)
                .selectable(false)
        }),
        if text.is_empty() {
            Empty::new().into_any()
        } else {
            line_text(text, emphasis)
                .style(|s| s.margin_left(14.0))
                .into_any()
        },
    ))
    .on_event_stop(el::Click, move |_, _| {
        let place = state.hierarchy.with_untracked(|h| {
            h.as_ref().and_then(|h| {
                h.rows
                    .get(at)
                    .map(|row| row.site.clone().unwrap_or_else(|| h.root_place.clone()))
            })
        });
        if let Some(place) = place {
            state.open_located(&place);
        }
    })
    .style(move |s| {
        clickable(s)
            .height(theme::REFERENCES_ROW_HEIGHT)
            .width_full()
            .items_center()
            .padding_left(indent(depth))
            .padding_right(12.0)
            .hover(|s| s.background(theme::canvas_hover()))
    })
}

/// The chevron that opens a row: right while it is shut, down while it
/// is open, dimmed while its calls are on their way. Click opens or
/// shuts; the row's own click, which opens the file, does not fire.
fn chevron(state: ReviewState, at: usize, expanded: bool, pending: bool) -> impl IntoView {
    let glyph = if expanded {
        SidebarIcon::ChevronDown
    } else {
        SidebarIcon::ChevronRight
    };
    let color = if pending {
        theme::fg_subtle
    } else {
        theme::fg_muted
    };
    Container::new(
        sidebar_glyph(glyph, color).style(|s| s.size(theme::CHEVRON_GLYPH, theme::CHEVRON_GLYPH)),
    )
    .on_event_stop(el::Click, move |_, _| {
        if expanded {
            state.collapse_hierarchy_row(at);
        } else {
            state.expand_hierarchy_row(at);
        }
    })
    .style(|s| {
        s.size(theme::CHEVRON_SLOT, theme::CHEVRON_SLOT)
            .items_center()
            .justify_center()
            .margin_right(6.0)
            .border_radius(3.0)
            .flex_shrink(0.0_f32)
            .hover(|s| s.background(theme::badge_bg()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::Call;
    use perch_lsp::lsp_types::{CallHierarchyItem, Position, Range, SymbolKind, Uri};

    fn item(name: &str, line: u32) -> CallHierarchyItem {
        let uri: Uri = format!("file:///wt/src/{name}.rs").parse().unwrap();
        let range = Range::new(
            Position::new(line, 3),
            Position::new(line, 3 + name.len() as u32),
        );
        CallHierarchyItem {
            name: name.into(),
            kind: SymbolKind::FUNCTION,
            tags: None,
            detail: None,
            uri,
            range,
            selection_range: range,
            data: None,
        }
    }

    fn call(name: &str, line: u32, text: &str) -> Call {
        Call {
            item: item(name, line),
            site: Located {
                path: format!("src/{name}.rs"),
                outside: None,
                line: line as usize + 4,
                character: 8,
                text: text.into(),
            },
        }
    }

    fn root_place() -> Located {
        Located {
            path: "src/run.rs".into(),
            outside: None,
            line: 1,
            character: 3,
            text: "fn run()".into(),
        }
    }

    #[test]
    fn the_tree_flattens_with_depths_and_a_repeated_ancestor_says_so() {
        let mut hierarchy = Hierarchy::new(item("run", 0), root_place(), Direction::Callers, 1);
        assert_eq!(rows(&hierarchy).len(), 2, "the root, and Looking under it");
        assert!(
            matches!(&rows(&hierarchy)[1], Row::Notice { text, depth: 1, .. } if text == "Looking\u{2026}")
        );

        // Two callers of run: main, and step.
        assert_eq!(
            hierarchy.fill(
                1,
                vec![call("main", 10, "run();"), call("step", 20, "run().await")]
            ),
            Some(2)
        );
        assert_eq!(hierarchy.fill(1, vec![]), None, "answered once");
        let listed = rows(&hierarchy);
        assert_eq!(listed.len(), 3);
        assert!(
            matches!(&listed[0], Row::Item { depth: 0, name, expanded: true, place, text, .. } if name == "run" && place == "src/run.rs:1" && text.is_empty())
        );
        assert!(
            matches!(&listed[1], Row::Item { depth: 1, name, place, text, emphasis, .. }
            if name == "main" && place == "src/main.rs:14" && text == "run();" && emphasis == "run")
        );
        assert!(matches!(&listed[2], Row::Item { depth: 1, name, .. } if name == "step"));

        // step's callers: run itself, a recursive call, and loop.
        hierarchy.expanding(2, 7);
        assert!(matches!(
            &rows(&hierarchy)[2],
            Row::Item { pending: true, .. }
        ));
        assert!(
            matches!(&rows(&hierarchy)[3], Row::Notice { text, depth: 2, .. } if text == "Looking\u{2026}")
        );
        hierarchy.fill(
            7,
            vec![call("run", 0, "step();"), call("loop", 30, "step()")],
        );
        let listed = rows(&hierarchy);
        assert_eq!(listed.len(), 5);
        assert!(
            matches!(&listed[3], Row::Item { depth: 2, name, repeats: true, .. } if name == "run")
        );
        assert!(
            matches!(&listed[4], Row::Item { depth: 2, name, repeats: false, .. } if name == "loop")
        );
        assert!(hierarchy.rows[3].repeats && !hierarchy.rows[4].repeats);

        // Open the recursive row: it has nothing under it, and says so.
        hierarchy.rows[3].expanded = true;
        let listed = rows(&hierarchy);
        assert!(
            matches!(&listed[4], Row::Notice { text, depth: 3, .. } if text == "Already open above.")
        );

        // Fold step: its subtree goes, and run's other caller stays.
        hierarchy.collapse(2);
        assert_eq!(hierarchy.rows.len(), 3);
        assert!(!hierarchy.rows[2].expanded);
        assert_eq!(
            hierarchy
                .rows
                .iter()
                .map(|r| r.item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["run", "main", "step"]
        );

        // A function with no callers reads so under it.
        hierarchy.expanding(1, 9);
        hierarchy.fill(9, vec![]);
        assert!(
            matches!(&rows(&hierarchy)[2], Row::Notice { text, depth: 2, .. } if text == "No callers.")
        );

        // Turned the other way, the tree starts again from the root.
        let turned = hierarchy.turned(Direction::Callees, 11);
        assert_eq!(turned.direction, Direction::Callees);
        assert_eq!(turned.rows.len(), 1);
        assert_eq!(turned.rows[0].pending, Some(11));
        assert_eq!(turned.root.name, "run");
    }

    #[test]
    fn a_callees_row_emphasises_its_own_name() {
        let mut hierarchy = Hierarchy::new(item("run", 0), root_place(), Direction::Callees, 1);
        hierarchy.fill(1, vec![call("flush", 40, "self.flush().await")]);
        assert!(matches!(&rows(&hierarchy)[1], Row::Item { emphasis, .. } if emphasis == "flush"));
        hierarchy.expanding(1, 2);
        hierarchy.fill(2, vec![]);
        assert!(matches!(&rows(&hierarchy)[2], Row::Notice { text, .. } if text == "No callees."));
    }

    #[test]
    fn an_outside_caller_reads_by_its_name_and_where_it_is_from() {
        let site = Located {
            path: "/deps/tokio-1.40.0/src/task.rs".into(),
            outside: Some("tokio 1.40.0".into()),
            line: 9,
            character: 0,
            text: "run()".into(),
        };
        assert_eq!(place_label(&site), "task.rs:9 \u{00b7} tokio 1.40.0");
    }

    #[test]
    fn the_panel_opens_when_the_server_names_a_function_and_not_otherwise() {
        let state = ReviewState::new();
        state.set_server_status(
            perch_lsp::ServerKind::RustAnalyzer,
            perch_lsp::Status::Ready,
        );
        let (backend, mut commands) = crate::backend::Backend::recording();
        let symbol = Symbol {
            path: "src/x.rs".into(),
            line: 4,
            column: 8,
            end: 11,
            text: "run".into(),
        };

        show(state, &backend, &symbol);
        let token = state.hierarchy_ask.get().expect("asked").token;
        assert!(matches!(
            commands.try_recv(),
            Ok(crate::backend::Command::PrepareCallHierarchy { path, line: 4, character: 8, .. }) if path == "src/x.rs"
        ));

        // A struct: nothing named, nothing opens.
        state.finish_prepare_hierarchy(token, Ok(vec![]), None);
        assert!(state.hierarchy.get().is_none());
        assert!(state.hierarchy_ask.get().is_none());

        // A function: the panel opens as its callers, with the calls asked
        // for through the queue the panel drains.
        show(state, &backend, &symbol);
        let token = state.hierarchy_ask.get().unwrap().token;
        state.references.set(Some(std::sync::Arc::new(
            crate::review::model::References {
                kind: crate::review::model::Locations::References,
                symbol: "x".into(),
                locations: vec![],
                pending: false,
                token: 0,
                failed: None,
                asked: None,
            },
        )));
        state.finish_prepare_hierarchy(token, Ok(vec![item("run", 0)]), Some(root_place()));
        let open = state.hierarchy.get().expect("the panel opened");
        assert_eq!(open.direction, Direction::Callers);
        assert_eq!(open.rows.len(), 1);
        assert!(state.references.get().is_none(), "one panel at a time");
        let queued = state.pending_calls.get();
        assert_eq!(queued.len(), 1);
        assert_eq!(
            (queued[0].0, queued[0].2),
            (open.rows[0].pending.unwrap(), Direction::Callers)
        );

        // The calls land, one row is expanded, and the direction turns.
        state.finish_calls(queued[0].0, Ok(vec![call("main", 10, "run();")]));
        assert_eq!(state.hierarchy.get().unwrap().rows.len(), 2);
        state.expand_hierarchy_row(1);
        let asked = state.pending_calls.get();
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[1].1.name, "main");
        state.turn_hierarchy(Direction::Callees);
        let turned = state.hierarchy.get().unwrap();
        assert_eq!(turned.direction, Direction::Callees);
        assert_eq!(turned.rows.len(), 1);
        assert_eq!(
            state.pending_calls.get().last().map(|a| a.2),
            Some(Direction::Callees)
        );

        // Escape closes whichever panel is open.
        assert!(state.panel_open());
        state.close_panel();
        assert!(!state.panel_open());
    }

    /// Asked while the server loads: the prepare says so in a notice, as a
    /// goto does, and a row's calls say so under it, and are asked again
    /// once the server is ready.
    #[test]
    fn a_hierarchy_asked_while_the_server_loads_says_so_and_asks_again() {
        let state = ReviewState::new();
        state.set_server_status(
            perch_lsp::ServerKind::RustAnalyzer,
            perch_lsp::Status::Indexing { percent: None },
        );
        let (backend, _commands) = crate::backend::Backend::recording();
        let symbol = Symbol {
            path: "src/x.rs".into(),
            line: 4,
            column: 8,
            end: 11,
            text: "run".into(),
        };
        let loading = "rust-analyzer is still loading";

        show(state, &backend, &symbol);
        let token = state.hierarchy_ask.get().unwrap().token;
        state.finish_prepare_hierarchy(token, Err(loading.into()), None);
        assert!(state.hierarchy.get().is_none());
        assert_eq!(
            state.pending_notice.get().as_deref(),
            Some("No call hierarchy for `run` (rust-analyzer is still loading)")
        );

        show(state, &backend, &symbol);
        let token = state.hierarchy_ask.get().unwrap().token;
        state.finish_prepare_hierarchy(token, Ok(vec![item("run", 0)]), Some(root_place()));
        let first = state.pending_calls.get()[0].0;
        state.finish_calls(first, Err(loading.into()));
        let listed = rows(&state.hierarchy.get().unwrap());
        assert!(matches!(
            &listed[1],
            Row::Notice { text, depth: 1, .. }
                if text == "No callers (rust-analyzer is still loading)."
        ));

        state.ask_again(&backend, perch_lsp::ServerKind::Gopls);
        assert_eq!(state.pending_calls.get().len(), 1, "not its server");

        state.set_server_status(
            perch_lsp::ServerKind::RustAnalyzer,
            perch_lsp::Status::Ready,
        );
        state.ask_again(&backend, perch_lsp::ServerKind::RustAnalyzer);
        let queued = state.pending_calls.get();
        assert_eq!(queued.len(), 2);
        assert_eq!(
            (queued[1].1.name.as_str(), queued[1].2),
            ("run", Direction::Callers)
        );
        let open = state.hierarchy.get().unwrap();
        assert_eq!(open.rows[0].pending, Some(queued[1].0));
        assert!(matches!(&rows(&open)[1], Row::Notice { text, .. } if text == "Looking\u{2026}"));

        state.finish_calls(queued[1].0, Ok(vec![call("main", 10, "run();")]));
        let open = state.hierarchy.get().unwrap();
        assert_eq!(open.rows.len(), 2);
        assert_eq!(open.rows[0].failed, None);
        state.ask_again(&backend, perch_lsp::ServerKind::RustAnalyzer);
        assert_eq!(
            state.pending_calls.get().len(),
            2,
            "answered: nothing to ask again"
        );
    }
}
