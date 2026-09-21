//! The editor made navigable: the pointer tracked to the symbol under it,
//! the tooltip that says what the symbol is, the hand while Cmd is held
//! over one, and the Cmd-click that goes to its definition.
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

use floem::event::EventPropagation;
use floem::kurbo::Point;
use floem::prelude::*;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::views::editor::core::cursor::CursorAffinity;
use floem::views::editor::Editor;
use floem::views::{dyn_container, tooltip, Decorators, Delay, Empty, TextEditor};

use crate::backend::Backend;
use crate::review::doc::Doc;
use crate::review::hover;
use crate::review::model::{GotoAsk, LoadedFile, Locations, ReviewState, Symbol};
use crate::review::references;

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

    // Cmd-click goes to the definition of the symbol under the pointer,
    // with the caret placed where the click was. Caught on the way down
    // and kept: floem has already given the editor the keyboard, as it
    // does for any press, and the editor's own handler would only start a
    // drag selection for a click that was a jump. A Cmd-click on nothing
    // is a click, and the editor gets it.
    let click_ed = ed.clone();
    let click_path = path.clone();
    let click_backend = backend.clone();
    let editor = editor.on_event_with_config(
        el::PointerDown,
        floem::context::EventCallbackConfig {
            phases: floem::context::Phases::CAPTURE,
        },
        move |cx, e: &PointerButtonEvent| {
            if !e.pointer.is_primary_pointer() || !command_held(&e.state.modifiers) {
                return EventPropagation::Continue;
            }
            let window_point = cx.world_transform.inverse() * e.state.logical_point();
            if cmd_click(state, &click_backend, &click_ed, &doc, &click_path, window_point) {
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        },
    );

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

/// Where a window point lands in the editor's text: the caret nearest it,
/// and which side of the caret the point is on.
///
/// The editor hit-tests to a caret, the nearest boundary between two
/// characters; the pointer is on whichever of the two it is over, which
/// its x against the caret's says.
#[derive(Debug, Clone, Copy)]
pub struct Hit {
    pub caret: usize,
    pub affinity: CursorAffinity,
    /// True when the point is right of the caret, on the character that
    /// follows it; false when it is on the one before.
    after: bool,
}

impl Hit {
    /// The symbol the character under the point belongs to.
    pub fn symbol(&self, doc: &Doc, path: &str) -> Option<Symbol> {
        let text = doc.text();
        let offset = if self.after {
            self.caret
        } else {
            let before = text[..self.caret.min(text.len())].chars().next_back()?;
            self.caret - before.len_utf8()
        };
        hover::symbol_at(doc, path, offset)
    }
}

/// The caret nearest a window point, through the editor's hit-testing.
/// `None` past the end of a line's text, so the pointer resting in the
/// space after a line is on nothing.
pub fn hit(ed: &Editor, window_point: Point) -> Option<Hit> {
    let content = ed.editor_view_id.get_untracked()?;
    let origin = content.get_visual_rect_no_clip().origin();
    let point = window_point - origin.to_vec2();
    let mode = ed.cursor.with_untracked(|c| c.get_mode());
    let (caret, inside, affinity) = ed.offset_of_point(mode, point);
    if !inside {
        return None;
    }
    let (caret_point, _) = ed.points_of_offset(caret, affinity);
    Some(Hit {
        caret,
        affinity,
        after: point.x >= caret_point.x,
    })
}

/// The symbol under a window point, through the document's rows. `None`
/// on a row that is no head line as well as past the text.
pub fn symbol_under(ed: &Editor, doc: &RwSignal<Arc<Doc>>, path: &str, window_point: Point) -> Option<Symbol> {
    let hit = hit(ed, window_point)?;
    doc.with_untracked(|d| hit.symbol(d, path))
}

/// A Cmd-click at a window point: the caret goes where the click was and
/// the definition of the symbol there is asked for. False when the point
/// is on no symbol, and the click is the editor's.
pub fn cmd_click(
    state: ReviewState,
    backend: &Backend,
    ed: &Editor,
    doc: &RwSignal<Arc<Doc>>,
    path: &str,
    window_point: Point,
) -> bool {
    let Some(hit) = hit(ed, window_point) else {
        return false;
    };
    let Some(symbol) = doc.with_untracked(|d| hit.symbol(d, path)) else {
        return false;
    };
    ed.cursor.update(|c| c.set_offset(hit.caret, hit.affinity, false, false));
    go_to_definition(state, backend, &symbol);
    true
}

/// Ask where a symbol is defined. The answer opens one location, lists
/// several as "Definitions of", and does nothing for none. Nothing while
/// the server is not there: the status line already says so.
pub fn go_to_definition(state: ReviewState, backend: &Backend, symbol: &Symbol) {
    if !state.lsp_status.with_untracked(|s| s.is_usable()) {
        return;
    }
    let token = state.next_token();
    state.goto.set(Some(GotoAsk {
        token,
        symbol: symbol.text.clone(),
        kind: Locations::Definitions,
    }));
    backend.definition(token, &symbol.path, symbol.line as u32, symbol.column as u32);
}

/// `F12`: the definition of the symbol at the caret.
pub fn definition_at_cursor(state: ReviewState, backend: &Backend) {
    if let Some(symbol) = references::symbol_at_cursor(state) {
        go_to_definition(state, backend, &symbol);
    }
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
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::*;
    use crate::backend::Command;
    use crate::review::model::{ItemTab, Phase};
    use crate::state::AppState;

    const HEAD: &str = "fn alpha() {}\nfn beta() {\n    alpha();\n}\n";
    const BASE: &str = "fn alpha() {}\nfn old() {\n    alpha();\n}\n";

    /// The whole window on Files with one changed file open, so the
    /// editor is where it lives and the pointer can be moved over it.
    fn harness() -> (HeadlessHarness, ReviewState) {
        let (harness, state, _) = recording_harness();
        (harness, state)
    }

    /// The same, with the commands the window sends kept for the test.
    fn recording_harness() -> (HeadlessHarness, ReviewState, UnboundedReceiver<Command>) {
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

        let (backend, commands) = crate::backend::Backend::recording();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        (harness, state, commands)
    }

    fn editor() -> Editor {
        LAST_EDITOR.with(|e| e.borrow().clone()).expect("an editor was made navigable")
    }

    fn stacked_doc(state: ReviewState) -> Arc<Doc> {
        match state.docs.get().map(|d| d.layout) {
            Some(crate::review::model::LiveLayout::Stacked(live)) => live.doc.get(),
            _ => panic!("a stacked editor was built"),
        }
    }

    /// The window point of a character of the editor's text: a little
    /// right of its caret, in the middle of its row.
    fn point_of(offset: usize) -> (f64, f64) {
        let ed = editor();
        let content = ed.editor_view_id.get_untracked().expect("the editor has a content view");
        let origin = content.get_visual_rect_no_clip().origin();
        let (top, bottom) = ed.points_of_offset(offset, CursorAffinity::Forward);
        (origin.x + top.x + 2.0, origin.y + (top.y + bottom.y) / 2.0)
    }

    fn key(code: Code, key: Key, modifiers: Modifiers) -> floem::event::Event {
        floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key,
            code,
            location: Default::default(),
            modifiers,
            repeat: false,
            is_composing: false,
        })
    }

    /// The definition asks the window sent.
    fn definitions_asked(commands: &mut UnboundedReceiver<Command>) -> Vec<(String, u32, u32)> {
        let mut out = Vec::new();
        while let Ok(command) = commands.try_recv() {
            if let Command::Definition { path, line, character, .. } = command {
                out.push((path, line, character));
            }
        }
        out
    }

    #[test]
    fn the_pointer_over_a_name_records_the_symbol_and_over_nothing_clears_it() {
        let (mut harness, state) = harness();
        let doc = stacked_doc(state);
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

    /// A Cmd-click, as the editor's capture handler makes it from the
    /// event. Not dispatched as an event: floem gives the pressed view the
    /// keyboard before any handler runs, and a focused editor asks the
    /// window for IME, which the headless window cannot give.
    fn cmd_click_at(state: ReviewState, backend: &Backend, doc: &RwSignal<Arc<Doc>>, x: f64, y: f64) -> bool {
        cmd_click(state, backend, &editor(), doc, "src/x.rs", Point::new(x, y))
    }

    #[test]
    fn cmd_click_on_a_name_places_the_caret_and_asks_for_its_definition() {
        let (mut harness, state, mut commands) = recording_harness();
        let (backend, mut asked) = crate::backend::Backend::recording();
        let live = match state.docs.get().map(|d| d.layout) {
            Some(crate::review::model::LiveLayout::Stacked(live)) => live,
            _ => panic!("a stacked editor was built"),
        };
        let doc = live.doc.get();
        let ed = editor();

        // On the `t` of `beta`, row 2, head line 1.
        let offset = doc.offset_of_row(2) + 5;
        let (x, y) = point_of(offset);
        assert!(cmd_click_at(state, &backend, &live.doc, x, y), "a name takes the click");
        harness.rebuild();
        assert_eq!(definitions_asked(&mut asked), vec![("src/x.rs".into(), 1, 3)]);
        let ask = state.goto.get().expect("a definition is on its way");
        assert_eq!(ask.symbol, "beta");
        assert_eq!(ask.kind, Locations::Definitions);
        assert_eq!(ed.cursor.get().offset(), offset, "the caret is where the click was");
        assert_eq!(state.last_position.get(), Some((1, 5)));

        // On the ghost, and on the `(` after `alpha`, the click is the
        // editor's: nothing is asked and the caret stays.
        let (x, y) = point_of(doc.offset_of_row(1) + 3);
        assert!(!cmd_click_at(state, &backend, &live.doc, x, y));
        let (x, y) = point_of(doc.offset_of_row(0) + 8);
        assert!(!cmd_click_at(state, &backend, &live.doc, x, y));
        harness.rebuild();
        assert!(definitions_asked(&mut asked).is_empty());
        assert_eq!(ed.cursor.get().offset(), offset);

        // With the server gone, the caret is placed and nothing is asked.
        state.goto.set(None);
        state.lsp_status.set(perch_lsp::Status::Failed { reason: "gone".into() });
        let offset = doc.offset_of_row(0) + 4;
        let (x, y) = point_of(offset);
        assert!(cmd_click_at(state, &backend, &live.doc, x, y));
        harness.rebuild();
        assert!(definitions_asked(&mut asked).is_empty());
        assert!(state.goto.get().is_none());
        assert_eq!(ed.cursor.get().offset(), offset);
        assert!(definitions_asked(&mut commands).is_empty(), "the window's own backend saw nothing");
    }

    #[test]
    fn f12_asks_for_the_definition_of_the_symbol_at_the_caret() {
        let (mut harness, state, mut commands) = recording_harness();
        let doc = stacked_doc(state);
        let ed = editor();

        // The caret on `alpha` in the call, row 3, head line 2, as a click
        // would place it.
        let offset = doc.offset_of_row(3) + 6;
        ed.cursor.update(|c| c.set_offset(offset, CursorAffinity::Backward, false, false));
        harness.rebuild();
        assert_eq!(state.last_position.get(), Some((2, 6)));

        harness.dispatch_event(key(Code::F12, Key::Named(NamedKey::F12), Modifiers::empty()));
        harness.rebuild();
        assert_eq!(definitions_asked(&mut commands), vec![("src/x.rs".into(), 2, 4)]);
        assert_eq!(state.goto.get().map(|g| g.symbol), Some("alpha".into()));

        // Shift-F12 still asks for references, and not for a definition.
        harness.dispatch_event(key(Code::F12, Key::Named(NamedKey::F12), Modifiers::SHIFT));
        harness.rebuild();
        assert!(definitions_asked(&mut commands).is_empty());
        assert_eq!(state.references.get().map(|r| r.symbol.clone()), Some("alpha".into()));
    }

    #[test]
    fn the_answer_jumps_and_the_history_keys_walk_back_and_forward() {
        let (mut harness, state, _) = recording_harness();
        let doc = stacked_doc(state);
        let ed = editor();

        // The caret on `alpha` in the call, head line 2.
        ed.cursor.update(|c| c.set_offset(doc.offset_of_row(3) + 6, CursorAffinity::Backward, false, false));
        harness.rebuild();
        assert_eq!(state.last_position.get(), Some((2, 6)));

        // The definition answers with the one place `alpha` is defined.
        state.goto.set(Some(GotoAsk { token: 1, symbol: "alpha".into(), kind: Locations::Definitions }));
        state.finish_goto(
            1,
            vec![crate::review::model::Located {
                path: "src/x.rs".into(),
                outside: None,
                line: 1,
                character: 3,
                text: "fn alpha() {}".into(),
            }],
        );
        harness.rebuild();
        harness.paint();
        assert_eq!(state.pending_jump.get(), None, "the view took the jump");
        assert_eq!(state.jump_to_line.get(), None, "and the editor took it");
        assert_eq!(state.last_position.get(), Some((0, 3)), "the caret landed on the definition");
        assert_eq!(ed.cursor.get().offset(), doc.offset_of_row(0) + 3);

        // Ctrl-- goes back to where the jump left.
        harness.dispatch_event(key(Code::Minus, Key::Character("-".into()), Modifiers::CONTROL));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.last_position.get(), Some((2, 0)));
        assert_eq!(ed.cursor.get().offset(), doc.offset_of_row(3));

        // Ctrl-Shift-- goes forward again.
        harness.dispatch_event(key(Code::Minus, Key::Character("_".into()), Modifiers::CONTROL | Modifiers::SHIFT));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.last_position.get(), Some((0, 0)));

        // And once more back, twice: the second has nowhere to go.
        harness.dispatch_event(key(Code::Minus, Key::Character("-".into()), Modifiers::CONTROL));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.last_position.get(), Some((2, 0)));
        harness.dispatch_event(key(Code::Minus, Key::Character("-".into()), Modifiers::CONTROL));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.last_position.get(), Some((2, 0)));
    }

    #[test]
    fn the_keys_type_nothing_and_open_nothing_while_a_composer_has_the_keyboard() {
        let (mut harness, state, mut commands) = recording_harness();
        let doc = stacked_doc(state);
        editor().cursor.update(|c| c.set_offset(doc.offset_of_row(3) + 6, CursorAffinity::Backward, false, false));
        harness.rebuild();
        state.composer_focused.set(true);

        harness.dispatch_event(key(Code::F12, Key::Named(NamedKey::F12), Modifiers::empty()));
        harness.dispatch_event(key(Code::F12, Key::Named(NamedKey::F12), Modifiers::SHIFT));
        harness.rebuild();
        assert!(definitions_asked(&mut commands).is_empty());
        assert!(state.references.get().is_none());
    }
}
