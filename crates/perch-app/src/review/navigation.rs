//! The editor made navigable: the pointer tracked to the symbol under it,
//! the tooltip that says what the symbol is, and the hand while Cmd is
//! held over one.
//!
//! The editor's own hit-testing maps the pointer to a byte offset, and
//! `review::hover` maps the offset to a symbol. The symbol under the
//! pointer is a signal on the review state; when it changes, and the
//! pointer stays on it for a moment, the server is asked about it, so the
//! answer is usually there by the time floem's tooltip timer fires. The
//! tooltip's content reads the answer through a signal, so an answer that
//! lands after the tip opened fills it in place.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use floem::kurbo::Point;
use floem::prelude::*;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::views::editor::Editor;
use floem::views::{dyn_container, tooltip, Decorators, Delay, Empty, TextEditor};

use crate::backend::Backend;
use crate::review::doc::Doc;
use crate::review::hover;
use crate::review::model::{LoadedFile, ReviewState, Symbol};

/// Whether the platform's command key is held: Cmd on a Mac, Ctrl
/// elsewhere, as VS Code reads a click that goes to a definition.
pub fn command_held(modifiers: &Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.meta()
    } else {
        modifiers.ctrl()
    }
}

/// The editor with the pointer tracked over it and the tooltip around it.
///
/// `doc` is the document the editor lays out, read live: a row asked
/// about after an edit is the edited one.
pub fn navigable(
    editor: TextEditor,
    state: ReviewState,
    backend: Rc<Backend>,
    file: Arc<LoadedFile>,
    doc: RwSignal<Arc<Doc>>,
) -> impl IntoView {
    let ed = editor.editor().clone();
    let path = file.path.clone();
    #[cfg(test)]
    LAST_EDITOR.with(|e| *e.borrow_mut() = Some(ed.clone()));

    let track_ed = ed.clone();
    let track_path = path.clone();
    let track_backend = backend.clone();
    let editor = editor
        .on_event_cont(el::PointerMove, move |cx, update| {
            let window_point = cx.world_transform.inverse() * update.current.logical_point();
            let symbol = symbol_under(&track_ed, &doc, &track_path, window_point);
            let hand = symbol.is_some() && command_held(&update.current.modifiers);
            set_hand(cx, &track_ed, hand);
            track(state, &track_backend, symbol);
        })
        .on_event_cont(el::PointerLeave, {
            let ed = ed.clone();
            move |cx, _| {
                set_hand(cx, &ed, false);
                state.pointer_symbol.set(None);
            }
        });

    tooltip(editor, move || {
        // Read each time the tip is drawn, and again when the answer lands:
        // nothing, an empty view with no chrome, until there is an answer
        // about the symbol the pointer is on.
        dyn_container(
            move || state.hover_for_pointer(),
            |markdown| match markdown {
                None => Empty::new().into_any(),
                Some(markdown) => hover::card(&markdown).into_any(),
            },
        )
    })
    .style(|s| {
        s.set(Delay, Duration::from_millis(hover::TOOLTIP_DELAY_MILLIS))
            .size_full()
            .min_width(0.0)
    })
}

/// The symbol under a window point, through the editor's hit-testing and
/// the document's rows. `None` past the end of a line's text as well as
/// on a row that is no head line, so the pointer resting in the space
/// after a line asks nothing.
///
/// The editor hit-tests to a caret, the nearest boundary between two
/// characters; the pointer is on whichever of the two it is over, which
/// its x against the caret's says.
pub fn symbol_under(ed: &Editor, doc: &RwSignal<Arc<Doc>>, path: &str, window_point: Point) -> Option<Symbol> {
    let content = ed.editor_view_id.get_untracked()?;
    let origin = content.get_visual_rect_no_clip().origin();
    let point = window_point - origin.to_vec2();
    let mode = ed.cursor.with_untracked(|c| c.get_mode());
    let (caret, inside, affinity) = ed.offset_of_point(mode, point);
    if !inside {
        return None;
    }
    let (caret_point, _) = ed.points_of_offset(caret, affinity);
    doc.with_untracked(|d| {
        let offset = if point.x >= caret_point.x {
            caret
        } else {
            let before = d.text()[..caret.min(d.text().len())].chars().next_back()?;
            caret - before.len_utf8()
        };
        hover::symbol_at(d, path, offset)
    })
}

/// Record the symbol under the pointer, and ask about it once the pointer
/// has rested on it for a moment. The timer's closure checks that the
/// pointer is still on the same symbol when it fires, so a pointer that
/// crossed a word on its way somewhere asks nothing.
fn track(state: ReviewState, backend: &Rc<Backend>, symbol: Option<Symbol>) {
    if state.pointer_symbol.with_untracked(|s| *s == symbol) {
        return;
    }
    state.pointer_symbol.set(symbol.clone());
    let Some(symbol) = symbol else { return };
    if !state.lsp_status.with_untracked(|s| s.is_usable()) {
        return;
    }
    let backend = backend.clone();
    floem::action::exec_after(Duration::from_millis(hover::ASK_DELAY_MILLIS), move |_| {
        if state.pointer_symbol.with_untracked(|s| s.as_ref() == Some(&symbol)) {
            state.ask_hover(&backend, symbol);
        }
    });
}

/// The pointer as a hand over a symbol while Cmd is held, and the text
/// beam otherwise. The editor's content view sets the beam on itself and
/// sits deepest under the pointer, so the hand goes on that element,
/// where it wins.
fn set_hand(cx: &mut floem::context::EventCx, ed: &Editor, hand: bool) {
    let Some(content) = ed.editor_view_id.get_untracked() else { return };
    if hand {
        cx.window_state.set_cursor(content, CursorStyle::Pointer);
    } else {
        cx.window_state.clear_cursor(content);
    }
}

#[cfg(test)]
thread_local! {
    /// The editor last made navigable, so a test can place the pointer
    /// from an offset the way the tracking maps the pointer back.
    static LAST_EDITOR: std::cell::RefCell<Option<Editor>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::reactive::SignalGet;
    use floem::views::editor::core::cursor::CursorAffinity;
    use perch_github::{Item, ItemCheckout};

    use super::*;
    use crate::review::model::{ItemTab, Phase};
    use crate::state::AppState;

    const HEAD: &str = "fn alpha() {}\nfn beta() {\n    alpha();\n}\n";
    const BASE: &str = "fn alpha() {}\nfn old() {\n    alpha();\n}\n";

    /// The whole window on Files with one changed file open, so the
    /// editor is where it lives and the pointer can be moved over it.
    fn harness() -> (HeadlessHarness, ReviewState) {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let now = "2026-09-18T09:59:00Z".parse().unwrap();
        let mut item = Item::new("slatedb/slatedb", now);
        item.id = "wt".into();
        item.checkout = Some(ItemCheckout {
            branch: "perch/pr-812".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: std::path::PathBuf::from("/tmp/wt"),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
        });
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state.set_phase(Phase::Ready);
        state.tab.set(ItemTab::Files);
        state.composer_autofocus.set(false);
        state.lsp_status.set(perch_lsp::Status::Ready);

        let file = Arc::new(crate::backend::build_loaded_file(
            "src/x.rs".into(),
            HEAD.into(),
            BASE.into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        ));
        state.open_tab("src/x.rs");
        state.finish_load("src/x.rs", file);

        let backend = crate::backend::Backend::detached();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        (harness, state)
    }

    /// The window point of a character of the editor's text: a little
    /// right of its caret, in the middle of its row.
    fn point_of(offset: usize) -> (f64, f64) {
        let ed = LAST_EDITOR.with(|e| e.borrow().clone()).expect("an editor was made navigable");
        let content = ed.editor_view_id.get_untracked().expect("the editor has a content view");
        let origin = content.get_visual_rect_no_clip().origin();
        let (top, bottom) = ed.points_of_offset(offset, CursorAffinity::Forward);
        (origin.x + top.x + 2.0, origin.y + (top.y + bottom.y) / 2.0)
    }

    #[test]
    fn the_pointer_over_a_name_records_the_symbol_and_over_nothing_clears_it() {
        let (mut harness, state) = harness();
        let doc = match state.docs.get().map(|d| d.layout) {
            Some(crate::review::model::LiveLayout::Stacked(live)) => live.doc.get(),
            _ => panic!("a stacked editor was built"),
        };
        // Rows: alpha, the ghost of `old`, beta, the call, the brace.
        assert_eq!(doc.head_line(1), None, "row 1 is the ghost");

        // On the `b` of `beta`, row 2.
        let (x, y) = point_of(doc.offset_of_row(2) + 3);
        harness.pointer_move(x, y);
        harness.rebuild();
        assert_eq!(
            state.pointer_symbol.get(),
            Some(Symbol {
                path: "src/x.rs".into(),
                line: 1,
                column: 3,
                end: 7,
                text: "beta".into(),
            }),
            "pointer at ({x}, {y})"
        );

        // On the `o` of the ghost's `old`: a removed line is nowhere.
        let (x, y) = point_of(doc.offset_of_row(1) + 3);
        harness.pointer_move(x, y);
        harness.rebuild();
        assert_eq!(state.pointer_symbol.get(), None, "pointer at ({x}, {y})");

        // On the `(` after `alpha` on row 0: punctuation.
        let (x, y) = point_of(doc.offset_of_row(0) + 8);
        harness.pointer_move(x, y);
        harness.rebuild();
        assert_eq!(state.pointer_symbol.get(), None, "pointer at ({x}, {y})");

        // Well past the end of row 0's text: nothing.
        let (x, y) = point_of(doc.offset_of_row(0) + 13);
        harness.pointer_move(x + 200.0, y);
        harness.rebuild();
        assert_eq!(state.pointer_symbol.get(), None);

        // On `alpha` in the call, then out of the editor altogether.
        let (x, y) = point_of(doc.offset_of_row(3) + 5);
        harness.pointer_move(x, y);
        harness.rebuild();
        assert_eq!(state.pointer_symbol.get().map(|s| (s.text, s.line, s.column)), Some(("alpha".into(), 2, 4)));
        harness.pointer_move(5.0, 5.0);
        harness.rebuild();
        assert_eq!(state.pointer_symbol.get(), None, "leaving the editor clears the symbol");
    }

    #[test]
    fn the_tip_draws_only_an_answer_about_the_symbol_under_the_pointer() {
        let state = ReviewState::new();
        let beta = Symbol { path: "src/x.rs".into(), line: 1, column: 3, end: 7, text: "beta".into() };
        let alpha = Symbol { path: "src/x.rs".into(), line: 0, column: 3, end: 8, text: "alpha".into() };
        let backend = crate::backend::Backend::detached();

        state.pointer_symbol.set(Some(beta.clone()));
        state.ask_hover(&backend, beta.clone());
        assert_eq!(state.hover_for_pointer(), None, "asked, not answered");
        let token = state.hover.get().unwrap().token;

        // A second ask about the same symbol is no ask.
        state.ask_hover(&backend, beta.clone());
        assert_eq!(state.hover.get().unwrap().token, token);

        // The answer lands with its token.
        state.hover.update(|h| h.as_mut().unwrap().markdown = "```rust\nfn beta()\n```".into());
        assert_eq!(state.hover_for_pointer().as_deref(), Some("```rust\nfn beta()\n```"));

        // The pointer moves to another symbol: the answer is not its.
        state.pointer_symbol.set(Some(alpha));
        assert_eq!(state.hover_for_pointer(), None);
        state.pointer_symbol.set(None);
        assert_eq!(state.hover_for_pointer(), None);
        state.pointer_symbol.set(Some(beta));
        assert!(state.hover_for_pointer().is_some(), "and back, the answer is there again");
    }
}
