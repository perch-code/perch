//! The editor made navigable: the pointer tracked to the symbol under it,
//! the tooltip that says what the symbol is, the hand while Cmd is held
//! over one, the Cmd-click that goes to its definition, and the menu on
//! a right-click that offers the rest.
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
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::core::cursor::CursorAffinity;
use floem::views::editor::Editor;
use floem::views::{dyn_container, tooltip, Decorators, Delay, Empty, TextEditor, TooltipClass};
use floem::HasViewId;

use crate::backend::Backend;
use crate::review::doc::Doc;
use crate::review::hierarchy;
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
            if cmd_click(
                state,
                &click_backend,
                &click_ed,
                &doc,
                &click_path,
                window_point,
            ) {
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        },
    );

    // Where the pointer last was, in window coordinates, for fitting the
    // tip beside it: floem opens the tip there and clamps nothing.
    let pointer_at = Rc::new(std::cell::Cell::new(Point::ZERO));
    let editor_id = editor.view_id();

    let track_ed = ed.clone();
    let track_path = path.clone();
    let track_backend = backend.clone();
    let track_pointer = pointer_at.clone();
    let editor = editor
        .on_event_cont(el::PointerMove, move |cx, update| {
            let window_point = cx.world_transform.inverse() * update.current.logical_point();
            track_pointer.set(window_point);
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

    // The menu is on the editor's content view: floem looks for one on
    // the view under the pointer alone. Its closure gets no position, so
    // it reads the symbol the pointer tracking last saw, which at the
    // moment of the click is the one clicked.
    if let Some(content) = ed.editor_view_id.get_untracked() {
        let menu_backend = backend.clone();
        let menu_ed = ed.clone();
        content.update_context_menu(move || {
            symbol_menu(
                state,
                &menu_backend,
                &menu_ed,
                state.pointer_symbol.get_untracked(),
            )
        });
    }

    tooltip(editor, move || {
        // Read each time the tip is drawn, and again when the answer lands:
        // nothing, an empty view with no chrome, until there is an answer
        // about the symbol the pointer is on. The card fits the room
        // between the pointer and the pane's right edge.
        let room = editor_id.get_visual_rect_no_clip().x1 - pointer_at.get().x - 12.0;
        let fit = hover::Fit::in_room(room);
        dyn_container(
            move || state.hover_for_pointer(),
            move |markdown| match markdown {
                None => Empty::new().into_any(),
                Some(markdown) => hover::card(&markdown, fit).into_any(),
            },
        )
    })
    .style(|s| {
        s.set(Delay, Duration::from_millis(hover::TOOLTIP_DELAY_MILLIS))
            .size_full()
            .min_width(0.0)
            // floem's theme dresses a tooltip with padding, a background
            // and shadows. The chrome is the card's, so that a tip with
            // nothing to say is nothing and not an empty box.
            .class(TooltipClass, |s| {
                s.padding(0.0)
                    .border(0.0)
                    .background(Color::TRANSPARENT)
                    .apply_box_shadows(Vec::new())
            })
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
pub fn symbol_under(
    ed: &Editor,
    doc: &RwSignal<Arc<Doc>>,
    path: &str,
    window_point: Point,
) -> Option<Symbol> {
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
    ed.cursor
        .update(|c| c.set_offset(hit.caret, hit.affinity, false, false));
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
    backend.definition(
        token,
        &symbol.path,
        symbol.line as u32,
        symbol.column as u32,
    );
}

/// `F12`: the definition of the symbol at the caret.
pub fn definition_at_cursor(state: ReviewState, backend: &Backend) {
    if let Some(symbol) = references::symbol_at_cursor(state) {
        go_to_definition(state, backend, &symbol);
    }
}

/// Ask for the type of the thing a symbol is: a binding's type, the
/// struct behind a `Self`. It nearly always has one answer, which opens;
/// several list as definitions.
pub fn go_to_type_definition(state: ReviewState, backend: &Backend, symbol: &Symbol) {
    if !state.lsp_status.with_untracked(|s| s.is_usable()) {
        return;
    }
    let token = state.next_token();
    state.goto.set(Some(GotoAsk {
        token,
        symbol: symbol.text.clone(),
        kind: Locations::Definitions,
    }));
    backend.type_definition(
        token,
        &symbol.path,
        symbol.line as u32,
        symbol.column as u32,
    );
}

/// Ask for the impls of a trait, or of a trait method. One opens; several
/// open the panel as "Implementations of".
pub fn go_to_implementations(state: ReviewState, backend: &Backend, symbol: &Symbol) {
    if !state.lsp_status.with_untracked(|s| s.is_usable()) {
        return;
    }
    let token = state.next_token();
    state.goto.set(Some(GotoAsk {
        token,
        symbol: symbol.text.clone(),
        kind: Locations::Implementations,
    }));
    backend.implementation(
        token,
        &symbol.path,
        symbol.line as u32,
        symbol.column as u32,
    );
}

/// `Cmd-F12`: the implementations of the symbol at the caret.
pub fn implementations_at_cursor(state: ReviewState, backend: &Backend) {
    if let Some(symbol) = references::symbol_at_cursor(state) {
        go_to_implementations(state, backend, &symbol);
    }
}

/// The entries of the right-click menu, in order, for the symbol under
/// the pointer: each names what it acts on. `None` on nothing, whitespace
/// or a removed line, where the menu is Copy alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Definition,
    TypeDefinition,
    Implementations,
    References,
    CallHierarchy,
    Copy,
}

impl Entry {
    /// Which entries a menu has: the navigation for a symbol, then Copy.
    pub fn for_symbol(symbol: Option<&Symbol>) -> Vec<Entry> {
        match symbol {
            Some(_) => vec![
                Entry::Definition,
                Entry::TypeDefinition,
                Entry::Implementations,
                Entry::References,
                Entry::CallHierarchy,
                Entry::Copy,
            ],
            None => vec![Entry::Copy],
        }
    }

    /// "Go to Definition of `parse`": the entry with the symbol it acts on.
    pub fn label(self, symbol: Option<&Symbol>) -> String {
        let name = symbol
            .map(|s| format!(" of `{}`", s.text))
            .unwrap_or_default();
        match self {
            Entry::Definition => format!("Go to Definition{name}"),
            Entry::TypeDefinition => format!("Go to Type Definition{name}"),
            Entry::Implementations => format!("Go to Implementations{name}"),
            Entry::References => format!("Find All References{}", name.replacen(" of ", " to ", 1)),
            Entry::CallHierarchy => {
                format!("Show Call Hierarchy{}", name.replacen(" of ", " for ", 1))
            }
            Entry::Copy => "Copy".to_string(),
        }
    }

    /// The key that does the same, as a menu accelerator: VS Code's keys.
    pub fn accelerator(self) -> Option<&'static str> {
        match self {
            Entry::Definition => Some("F12"),
            Entry::TypeDefinition => None,
            Entry::Implementations => Some("super+F12"),
            Entry::References => Some("shift+F12"),
            Entry::CallHierarchy => Some("shift+alt+KeyH"),
            Entry::Copy => Some("super+KeyC"),
        }
    }
}

/// The menu for a right-click in the code. The navigation entries each
/// act on the symbol; Copy copies the editor's selection.
fn symbol_menu(
    state: ReviewState,
    backend: &Rc<Backend>,
    ed: &Editor,
    symbol: Option<Symbol>,
) -> floem::Menu {
    let mut menu = floem::Menu::new();
    for entry in Entry::for_symbol(symbol.as_ref()) {
        if entry == Entry::Copy && symbol.is_some() {
            menu = menu.separator();
        }
        let label = entry.label(symbol.as_ref());
        let backend = backend.clone();
        let ed = ed.clone();
        let symbol = symbol.clone();
        menu = menu.item(label, move |item| {
            let item = match entry.accelerator().and_then(|keys| keys.parse().ok()) {
                Some(accelerator) => item.accelerator(accelerator),
                None => item,
            };
            item.action(move || match (entry, &symbol) {
                (Entry::Definition, Some(symbol)) => go_to_definition(state, &backend, symbol),
                (Entry::TypeDefinition, Some(symbol)) => {
                    go_to_type_definition(state, &backend, symbol)
                }
                (Entry::Implementations, Some(symbol)) => {
                    go_to_implementations(state, &backend, symbol)
                }
                (Entry::References, Some(symbol)) => {
                    references::find(state, &backend, symbol, Locations::References)
                }
                (Entry::CallHierarchy, Some(symbol)) => hierarchy::show(state, &backend, symbol),
                (Entry::Copy, _) => copy_selection(&ed),
                (_, None) => {}
            })
        });
    }
    menu
}

/// Copy the editor's selection to the clipboard, as Cmd-C does.
fn copy_selection(ed: &Editor) {
    let doc = ed.doc();
    let text = ed.cursor.with_untracked(|c| {
        let rope = doc.rope_text();
        c.mode
            .regions_iter()
            .map(|(a, b, _)| {
                let (start, end) = (a.min(b), a.max(b));
                rope.slice_to_cow(start..end).into_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    });
    if !text.is_empty() {
        crate::ui::widgets::copy_to_clipboard(text);
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
        if state
            .pointer_symbol
            .with_untracked(|s| s.as_ref() == Some(&symbol))
        {
            state.ask_hover(&backend, symbol);
        }
    });
}

/// The pointer as a hand over a symbol while Cmd is held, and the text
/// beam otherwise. The editor's content view sets the beam on itself and
/// sits deepest under the pointer, so the hand goes on that element,
/// where it wins.
fn set_hand(cx: &mut floem::context::EventCx, ed: &Editor, hand: bool) {
    let Some(content) = ed.editor_view_id.get_untracked() else {
        return;
    };
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
    use floem::ui_events::pointer::{
        PointerButton, PointerId, PointerInfo, PointerState, PointerType,
    };
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
            source: None,
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
        LAST_EDITOR
            .with(|e| e.borrow().clone())
            .expect("an editor was made navigable")
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
        let content = ed
            .editor_view_id
            .get_untracked()
            .expect("the editor has a content view");
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
            if let Command::Definition {
                path,
                line,
                character,
                ..
            } = command
            {
                out.push((path, line, character));
            }
        }
        out
    }

    /// The implementation asks the window sent.
    fn implementations_asked(commands: &mut UnboundedReceiver<Command>) -> Vec<(String, u32, u32)> {
        let mut out = Vec::new();
        while let Ok(command) = commands.try_recv() {
            if let Command::Implementation {
                path,
                line,
                character,
                ..
            } = command
            {
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
        assert_eq!(
            state
                .pointer_symbol
                .get()
                .map(|s| (s.text, s.line, s.column)),
            Some(("alpha".into(), 2, 4))
        );
        harness.pointer_move(5.0, 5.0);
        harness.rebuild();
        assert_eq!(
            state.pointer_symbol.get(),
            None,
            "leaving the editor clears the symbol"
        );
    }

    #[test]
    fn the_tip_draws_only_an_answer_about_the_symbol_under_the_pointer() {
        let state = ReviewState::new();
        let beta = Symbol {
            path: "src/x.rs".into(),
            line: 1,
            column: 3,
            end: 7,
            text: "beta".into(),
        };
        let alpha = Symbol {
            path: "src/x.rs".into(),
            line: 0,
            column: 3,
            end: 8,
            text: "alpha".into(),
        };
        let backend = crate::backend::Backend::detached();

        state.pointer_symbol.set(Some(beta.clone()));
        state.ask_hover(&backend, beta.clone());
        assert_eq!(state.hover_for_pointer(), None, "asked, not answered");
        let token = state.hover.get().unwrap().token;

        // A second ask about the same symbol is no ask.
        state.ask_hover(&backend, beta.clone());
        assert_eq!(state.hover.get().unwrap().token, token);

        // The answer lands with its token.
        state
            .hover
            .update(|h| h.as_mut().unwrap().markdown = "```rust\nfn beta()\n```".into());
        assert_eq!(
            state.hover_for_pointer().as_deref(),
            Some("```rust\nfn beta()\n```")
        );

        // The pointer moves to another symbol: the answer is not its.
        state.pointer_symbol.set(Some(alpha));
        assert_eq!(state.hover_for_pointer(), None);
        state.pointer_symbol.set(None);
        assert_eq!(state.hover_for_pointer(), None);
        state.pointer_symbol.set(Some(beta));
        assert!(
            state.hover_for_pointer().is_some(),
            "and back, the answer is there again"
        );
    }

    /// A primary button going down and up at a window point with the
    /// command key held, as the window receives a Cmd-click.
    fn cmd_click_at(harness: &mut HeadlessHarness, x: f64, y: f64) {
        let modifiers = if cfg!(target_os = "macos") {
            Modifiers::META
        } else {
            Modifiers::CONTROL
        };
        for down in [true, false] {
            let mut state = PointerState {
                count: 1,
                modifiers,
                ..PointerState::default()
            };
            state.position.x = x;
            state.position.y = y;
            let event = PointerButtonEvent {
                state,
                button: Some(PointerButton::Primary),
                pointer: PointerInfo {
                    pointer_id: Some(PointerId::PRIMARY),
                    persistent_device_id: None,
                    pointer_type: PointerType::Mouse,
                },
            };
            harness.dispatch_event(floem::event::Event::Pointer(if down {
                PointerEvent::Down(event)
            } else {
                PointerEvent::Up(event)
            }));
        }
    }

    #[test]
    fn cmd_click_on_a_name_places_the_caret_and_asks_for_its_definition() {
        let (mut harness, state, mut commands) = recording_harness();
        let doc = stacked_doc(state);
        let ed = editor();
        let content = ed.editor_view_id.get_untracked().unwrap();

        // On the `t` of `beta`, row 2, head line 1.
        let offset = doc.offset_of_row(2) + 5;
        let (x, y) = point_of(offset);
        cmd_click_at(&mut harness, x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(
            definitions_asked(&mut commands),
            vec![("src/x.rs".into(), 1, 3)],
            "click at ({x}, {y})"
        );
        let ask = state.goto.get().expect("a definition is on its way");
        assert_eq!(ask.symbol, "beta");
        assert_eq!(ask.kind, Locations::Definitions);
        assert_eq!(
            ed.cursor.get().offset(),
            offset,
            "the caret is where the click was"
        );
        assert_eq!(state.last_position.get(), Some((1, 5)));
        assert!(
            harness.is_focused(content),
            "the press gave the editor the keyboard, as any press does"
        );

        // On the ghost, the click is the editor's own: nothing is asked,
        // and the editor places the caret there itself, which is nowhere
        // in the head file.
        let ghost = doc.offset_of_row(1) + 3;
        let (x, y) = point_of(ghost);
        cmd_click_at(&mut harness, x, y);
        harness.rebuild();
        harness.paint();
        assert!(definitions_asked(&mut commands).is_empty());
        assert_eq!(ed.cursor.get().offset(), ghost);
        assert_eq!(state.last_position.get(), None);

        // On the `(` after `alpha`: punctuation, so the editor's click too.
        let paren = doc.offset_of_row(0) + 8;
        let (x, y) = point_of(paren);
        cmd_click_at(&mut harness, x, y);
        harness.rebuild();
        harness.paint();
        assert!(definitions_asked(&mut commands).is_empty());
        assert_eq!(ed.cursor.get().offset(), paren);

        // With the server gone, the caret is placed and nothing is asked.
        state.goto.set(None);
        state.lsp_status.set(perch_lsp::Status::Failed {
            reason: "gone".into(),
        });
        let offset = doc.offset_of_row(0) + 4;
        let (x, y) = point_of(offset);
        cmd_click_at(&mut harness, x, y);
        harness.rebuild();
        harness.paint();
        assert!(definitions_asked(&mut commands).is_empty());
        assert!(state.goto.get().is_none());
        assert_eq!(ed.cursor.get().offset(), offset);
    }

    #[test]
    fn f12_asks_for_the_definition_of_the_symbol_at_the_caret() {
        let (mut harness, state, mut commands) = recording_harness();
        let doc = stacked_doc(state);
        let ed = editor();

        // The caret on `alpha` in the call, row 3, head line 2, as a click
        // would place it.
        let offset = doc.offset_of_row(3) + 6;
        ed.cursor
            .update(|c| c.set_offset(offset, CursorAffinity::Backward, false, false));
        harness.rebuild();
        assert_eq!(state.last_position.get(), Some((2, 6)));

        harness.dispatch_event(key(
            Code::F12,
            Key::Named(NamedKey::F12),
            Modifiers::empty(),
        ));
        harness.rebuild();
        assert_eq!(
            definitions_asked(&mut commands),
            vec![("src/x.rs".into(), 2, 4)]
        );
        assert_eq!(state.goto.get().map(|g| g.symbol), Some("alpha".into()));

        // Shift-F12 still asks for references, and not for a definition.
        harness.dispatch_event(key(Code::F12, Key::Named(NamedKey::F12), Modifiers::SHIFT));
        harness.rebuild();
        assert!(definitions_asked(&mut commands).is_empty());
        assert_eq!(
            state.references.get().map(|r| r.symbol.clone()),
            Some("alpha".into())
        );
    }

    #[test]
    fn the_answer_jumps_and_the_history_keys_walk_back_and_forward() {
        let (mut harness, state, _) = recording_harness();
        let doc = stacked_doc(state);
        let ed = editor();

        // The caret on `alpha` in the call, head line 2.
        ed.cursor.update(|c| {
            c.set_offset(
                doc.offset_of_row(3) + 6,
                CursorAffinity::Backward,
                false,
                false,
            )
        });
        harness.rebuild();
        assert_eq!(state.last_position.get(), Some((2, 6)));

        // The definition answers with the one place `alpha` is defined.
        state.goto.set(Some(GotoAsk {
            token: 1,
            symbol: "alpha".into(),
            kind: Locations::Definitions,
        }));
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
        assert_eq!(
            state.last_position.get(),
            Some((0, 3)),
            "the caret landed on the definition"
        );
        assert_eq!(ed.cursor.get().offset(), doc.offset_of_row(0) + 3);

        // Ctrl-- goes back to where the jump left.
        harness.dispatch_event(key(
            Code::Minus,
            Key::Character("-".into()),
            Modifiers::CONTROL,
        ));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.last_position.get(), Some((2, 0)));
        assert_eq!(ed.cursor.get().offset(), doc.offset_of_row(3));

        // Ctrl-Shift-- goes forward again.
        harness.dispatch_event(key(
            Code::Minus,
            Key::Character("_".into()),
            Modifiers::CONTROL | Modifiers::SHIFT,
        ));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.last_position.get(), Some((0, 0)));

        // And once more back, twice: the second has nowhere to go.
        harness.dispatch_event(key(
            Code::Minus,
            Key::Character("-".into()),
            Modifiers::CONTROL,
        ));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.last_position.get(), Some((2, 0)));
        harness.dispatch_event(key(
            Code::Minus,
            Key::Character("-".into()),
            Modifiers::CONTROL,
        ));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.last_position.get(), Some((2, 0)));
    }

    /// A file a comment names without a line opens as the tree opens it:
    /// asked for, with nothing to jump to and so nothing unfolded, and the
    /// place the reader left is where `Ctrl--` goes.
    #[test]
    fn a_file_named_alone_opens_with_no_jump_and_back_returns() {
        let (mut harness, state, mut commands) = recording_harness();
        state.go_to_file("src/y.rs", None);
        harness.rebuild();
        assert_eq!(state.pending_open.get(), None, "the view took it");
        assert_eq!(state.active.get().as_deref(), Some("src/y.rs"));
        assert_eq!(state.jump_to_line.get(), None);
        let mut opened = Vec::new();
        while let Ok(command) = commands.try_recv() {
            if let Command::OpenFile { path, .. } = command {
                opened.push(path);
            }
        }
        assert_eq!(opened, vec!["src/y.rs".to_string()]);
        let file = crate::backend::build_loaded_file(
            "src/y.rs".into(),
            HEAD.into(),
            BASE.into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        state.finish_load("src/y.rs", Arc::new(file));
        harness.rebuild();
        harness.paint();

        // Named again while it is open, it stays where it is.
        let epoch = state.editor_epoch.get();
        state.go_to_file("src/y.rs", None);
        harness.rebuild();
        assert_eq!(state.editor_epoch.get(), epoch, "the open file was not opened again");

        harness.dispatch_event(key(
            Code::Minus,
            Key::Character("-".into()),
            Modifiers::CONTROL,
        ));
        harness.rebuild();
        assert_eq!(state.active.get().as_deref(), Some("src/x.rs"));
    }

    #[test]
    fn the_keys_type_nothing_and_open_nothing_while_a_composer_has_the_keyboard() {
        let (mut harness, state, mut commands) = recording_harness();
        let doc = stacked_doc(state);
        editor().cursor.update(|c| {
            c.set_offset(
                doc.offset_of_row(3) + 6,
                CursorAffinity::Backward,
                false,
                false,
            )
        });
        harness.rebuild();
        state.composer_focused.set(true);

        harness.dispatch_event(key(
            Code::F12,
            Key::Named(NamedKey::F12),
            Modifiers::empty(),
        ));
        harness.dispatch_event(key(Code::F12, Key::Named(NamedKey::F12), Modifiers::SHIFT));
        harness.rebuild();
        assert!(definitions_asked(&mut commands).is_empty());
        assert!(state.references.get().is_none());
    }

    #[test]
    fn cmd_f12_asks_for_the_implementations_of_the_symbol_at_the_caret() {
        let (mut harness, state, mut commands) = recording_harness();
        let doc = stacked_doc(state);
        editor().cursor.update(|c| {
            c.set_offset(
                doc.offset_of_row(2) + 4,
                CursorAffinity::Backward,
                false,
                false,
            )
        });
        harness.rebuild();

        let os_mod = if cfg!(target_os = "macos") {
            Modifiers::META
        } else {
            Modifiers::CONTROL
        };
        harness.dispatch_event(key(Code::F12, Key::Named(NamedKey::F12), os_mod));
        harness.rebuild();
        assert_eq!(
            implementations_asked(&mut commands),
            vec![("src/x.rs".into(), 1, 3)]
        );
        let ask = state.goto.get().expect("implementations are on their way");
        assert_eq!(
            (ask.symbol.as_str(), ask.kind),
            ("beta", Locations::Implementations)
        );

        // Several answers list under the kind, and one opens.
        let located = |path: &str, line: usize| crate::review::model::Located {
            path: path.into(),
            outside: None,
            line,
            character: 0,
            text: "impl".into(),
        };
        state.finish_goto(
            ask.token,
            vec![located("src/x.rs", 1), located("src/y.rs", 4)],
        );
        let panel = state.references.get().expect("the panel opened");
        assert_eq!(panel.kind, Locations::Implementations);
        assert_eq!(panel.symbol, "beta");
        assert_eq!(panel.locations.len(), 2);
    }

    #[test]
    fn shift_alt_h_asks_for_the_call_hierarchy_of_the_symbol_at_the_caret() {
        let (mut harness, state, mut commands) = recording_harness();
        let doc = stacked_doc(state);
        editor().cursor.update(|c| {
            c.set_offset(
                doc.offset_of_row(3) + 6,
                CursorAffinity::Backward,
                false,
                false,
            )
        });
        harness.rebuild();

        // On a Mac, Alt-H types a dot above; the code says which key it was.
        harness.dispatch_event(key(
            Code::KeyH,
            Key::Character("\u{02d9}".into()),
            Modifiers::SHIFT | Modifiers::ALT,
        ));
        harness.rebuild();
        let mut asked = Vec::new();
        while let Ok(command) = commands.try_recv() {
            if let Command::PrepareCallHierarchy {
                path,
                line,
                character,
                ..
            } = command
            {
                asked.push((path, line, character));
            }
        }
        assert_eq!(asked, vec![("src/x.rs".into(), 2, 4)]);
        assert_eq!(
            state.hierarchy_ask.get().map(|a| a.symbol),
            Some("alpha".into())
        );
        assert!(
            state.hierarchy.get().is_none(),
            "nothing opens until the server names a function"
        );
    }

    #[test]
    fn the_type_definition_answers_as_a_definition_does() {
        let state = ReviewState::new();
        state.lsp_status.set(perch_lsp::Status::Ready);
        let (backend, mut commands) = crate::backend::Backend::recording();
        let symbol = Symbol {
            path: "src/x.rs".into(),
            line: 4,
            column: 8,
            end: 12,
            text: "self".into(),
        };
        go_to_type_definition(state, &backend, &symbol);
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::TypeDefinition { path, line: 4, character: 8, .. }) if path == "src/x.rs"
        ));
        let ask = state.goto.get().unwrap();
        assert_eq!(ask.kind, Locations::Definitions);
        state.finish_goto(
            ask.token,
            vec![crate::review::model::Located {
                path: "src/y.rs".into(),
                outside: None,
                line: 10,
                character: 11,
                text: "pub struct Db".into(),
            }],
        );
        assert_eq!(state.pending_jump.get(), Some(("src/y.rs".into(), 9, 11)));
    }

    #[test]
    fn the_menu_names_what_each_entry_acts_on_and_is_copy_alone_on_nothing() {
        let parse = Symbol {
            path: "src/x.rs".into(),
            line: 0,
            column: 3,
            end: 8,
            text: "parse".into(),
        };
        let entries = Entry::for_symbol(Some(&parse));
        assert_eq!(
            entries
                .iter()
                .map(|e| e.label(Some(&parse)))
                .collect::<Vec<_>>(),
            vec![
                "Go to Definition of `parse`",
                "Go to Type Definition of `parse`",
                "Go to Implementations of `parse`",
                "Find All References to `parse`",
                "Show Call Hierarchy for `parse`",
                "Copy",
            ]
        );
        assert_eq!(
            entries.iter().map(|e| e.accelerator()).collect::<Vec<_>>(),
            vec![
                Some("F12"),
                None,
                Some("super+F12"),
                Some("shift+F12"),
                Some("shift+alt+KeyH"),
                Some("super+KeyC")
            ]
        );
        for keys in entries.iter().filter_map(|e| e.accelerator()) {
            assert!(
                keys.parse::<floem::muda::accelerator::Accelerator>()
                    .is_ok(),
                "{keys}"
            );
        }
        assert_eq!(Entry::for_symbol(None), vec![Entry::Copy]);
        assert_eq!(Entry::Copy.label(None), "Copy");
    }

    #[test]
    fn the_caret_resting_on_a_symbol_tints_its_uses_and_leaving_it_clears_them() {
        let (mut harness, state, _) = recording_harness();
        let (backend, mut commands) = crate::backend::Backend::recording();
        let doc = stacked_doc(state);
        let ed = editor();

        // The caret on `alpha` in the call: the ask, as the timer makes it.
        ed.cursor.update(|c| {
            c.set_offset(
                doc.offset_of_row(3) + 6,
                CursorAffinity::Backward,
                false,
                false,
            )
        });
        harness.rebuild();
        let alpha = references::symbol_at_cursor(state).expect("the caret is on alpha");
        assert_eq!(
            (alpha.text.as_str(), alpha.line, alpha.column),
            ("alpha", 2, 4)
        );
        state.ask_highlights(&backend, alpha.clone());
        let token = state.highlight_ask.get().unwrap().0;
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::DocumentHighlight { path, line: 2, character: 4, .. }) if path == "src/x.rs"
        ));

        // A stale answer is dropped; the right one lands.
        state.finish_highlights(token + 1, vec![(9, 0, 1)]);
        assert!(state.highlights.get().is_empty());
        state.finish_highlights(token, vec![(0, 3, 8), (2, 4, 9)]);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.highlights.get().as_slice(), &[(0, 3, 8), (2, 4, 9)]);

        // Along the word, the tint stays; off it, the tint clears.
        ed.cursor.update(|c| {
            c.set_offset(
                doc.offset_of_row(3) + 8,
                CursorAffinity::Backward,
                false,
                false,
            )
        });
        harness.rebuild();
        assert_eq!(state.highlights.get().len(), 2, "still on alpha");
        ed.cursor
            .update(|c| c.set_offset(doc.offset_of_row(4), CursorAffinity::Backward, false, false));
        harness.rebuild();
        assert!(state.highlights.get().is_empty(), "the brace is nothing");
        assert!(state.highlight_ask.get().is_none());

        // Another tab clears them too.
        state.finish_highlights(token, vec![(0, 3, 8)]);
        state.highlight_ask.set(Some((token, alpha)));
        state.finish_highlights(token, vec![(0, 3, 8)]);
        assert!(!state.highlights.get().is_empty());
        state.open_tab("src/y.rs");
        assert!(state.highlights.get().is_empty());
    }
}
