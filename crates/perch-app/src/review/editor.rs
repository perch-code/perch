//! The code pane: gutter, editor, symbol panel, and the states that stand in
//! for them.
//!
//! Two layouts. Stacked is one editor over the document built from the
//! unified buffer, the head with the removed lines spliced in. Split is two
//! editors, base on the left and head on the right, each over a document
//! with one line per aligned row so the two are the same height and a row
//! on one side is the row beside it on the other. The right editor owns the
//! scroll position and the left follows it; everything that reads the
//! cursor is wired to the right one.
//!
//! Every editor's document is held live on the review state, the handle
//! the editor lays out from beside the row table over it, so that what the
//! editor shows can be edited in place when the document changes under it.

use std::rc::Rc;
use std::sync::Arc;

use floem::event::PointerScrollEventExt;
use floem::kurbo::{Size, Vec2};
use floem::prelude::*;
use floem::reactive::{Effect, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::editor::core::cursor::Cursor;
use floem::views::scroll::ScrollCustomStyle;
use floem::views::{
    Container, Decorators, Empty, Label, ScrollExt, Stack, TextEditor, dyn_container, text_editor,
};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::doc::{self, Doc};
use crate::review::gutter::{diff_gutter, side_gutter};
use crate::review::hover;
use crate::review::model::{Layout, LiveDoc, LiveDocs, LiveLayout, LoadedFile, ReviewState};
use crate::review::references;
use crate::review::styling::PerchStyling;
use crate::review::OpenFile;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::centered_message;

pub fn editor_pane(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
) -> impl IntoView {
    // A go-to-definition answer arrives asynchronously; opening the tab has
    // to happen on the UI thread, so the view picks it up from a signal.
    {
        let open_file = open_file.clone();
        Effect::new(move |_| {
            if let Some((path, line)) = state.pending_jump.get() {
                state.pending_jump.set(None);
                open_file(&path);
                state.jump_to_line.set(Some(line));
            }
        });
    }

    let code_backend = backend.clone();

    Stack::vertical((
        dyn_container(
            // The epoch changes whenever the active file or its content
            // changes, and the layout when the toggle is clicked. Another
            // file or another layout is another editor: a fresh one resets
            // the layout caches cleanly, and nothing of the old is worth
            // keeping. What changes under the one file is for editing the
            // document in place, through the live documents held below.
            move || (state.active.get(), state.editor_epoch.get(), state.layout.get()),
            move |(path, epoch, layout)| {
                // Whatever editor stood here is going with its documents;
                // the one built below registers its own.
                state.docs.set(None);
                match path {
                    None => centered_message(
                        SidebarIcon::Search,
                        || "No file open".to_string(),
                        || "Pick a file from the list to read it.".to_string(),
                    )
                    .into_any(),
                    Some(path) => match state.file(&path) {
                        Some(file) => {
                            code_view(app, state, file, epoch, layout, code_backend.clone()).into_any()
                        }
                        None => loading_view(path).into_any(),
                    },
                }
            },
        )
        .style(|s| s.flex_grow(1.0_f32).min_height(0.0).width_full().background(theme::CODE_BG)),
        symbol_panel(state, backend),
        references::panel(state, open_file),
    ))
    .style(|s| s.size_full().flex_col().min_height(0.0))
}

/// The file, stacked or split. A file with nothing to put on the left,
/// because it was added, is outside the changed list or is binary, is
/// stacked whatever the toggle says.
fn code_view(
    app: AppState,
    state: ReviewState,
    file: Arc<LoadedFile>,
    epoch: u64,
    layout: Layout,
    backend: Rc<Backend>,
) -> impl IntoView {
    let split = match layout {
        Layout::Split => split_of(&file),
        Layout::Stacked => None,
    };
    match split {
        Some(split) => split_view(app, state, file, split, epoch, backend).into_any(),
        None => stacked_view(app, state, file, epoch, backend).into_any(),
    }
}

fn stacked_view(
    app: AppState,
    state: ReviewState,
    file: Arc<LoadedFile>,
    epoch: u64,
    backend: Rc<Backend>,
) -> impl IntoView {
    let built = doc::build(&file.buffer, &[], &[]);
    let text = built.text.clone();
    let doc = RwSignal::new(Arc::new(built));
    let editor = code_editor(text, PerchStyling::new(file.clone(), doc, epoch));
    state.docs.set(Some(LiveDocs {
        epoch,
        layout: LiveLayout::Stacked(LiveDoc { handle: editor.doc(), doc }),
    }));
    wire_cursor(state, backend, file.clone(), doc, editor.editor().cursor);
    wire_jump(state, doc, editor.editor().scroll_to, epoch);

    // The gutter tracks the same signals the editor lays out from, so the two
    // stay aligned without any scroll plumbing of their own.
    let viewport = editor.editor().viewport;
    let scroll_delta = editor.editor().scroll_delta;
    Stack::horizontal((diff_gutter(state, app, file, doc, viewport, scroll_delta), editor))
        .style(|s| s.size_full().min_height(0.0))
}

/// Base on the left, head on the right, over one scroll position.
///
/// Both editors keep a scroll of their own, so the two are tied together:
/// the right one's viewport drives the left through `scroll_to`, and a
/// wheel over the left, which takes no pointer events, is handed to the
/// right. The left's scrollbar is hidden, so there is one.
fn split_view(
    app: AppState,
    state: ReviewState,
    file: Arc<LoadedFile>,
    split: (Doc, Doc),
    epoch: u64,
    backend: Rc<Backend>,
) -> impl IntoView {
    let (left_text, right_text) = (split.0.text.clone(), split.1.text.clone());
    let (left_doc, right_doc) = (RwSignal::new(Arc::new(split.0)), RwSignal::new(Arc::new(split.1)));
    let line_height = theme::CODE_LINE_HEIGHT as f64;

    // Distinct cache keys, in case the two ever share a layout cache.
    let left = code_editor(
        left_text,
        PerchStyling::side(file.clone(), Side::Base, left_doc, epoch.wrapping_mul(2)),
    )
    .style(|s| {
        s.pointer_events_none()
            .custom_style_class(|s: ScrollCustomStyle| s.hide_bars(true))
    });
    let right = code_editor(
        right_text,
        PerchStyling::side(file.clone(), Side::Head, right_doc, epoch.wrapping_mul(2) + 1),
    );
    state.docs.set(Some(LiveDocs {
        epoch,
        layout: LiveLayout::Split {
            left: LiveDoc { handle: left.doc(), doc: left_doc },
            right: LiveDoc { handle: right.doc(), doc: right_doc },
        },
    }));

    wire_cursor(state, backend, file.clone(), right_doc, right.editor().cursor);
    wire_jump(state, right_doc, right.editor().scroll_to, epoch);

    let (left_viewport, left_scroll_to, left_delta) =
        (left.editor().viewport, left.editor().scroll_to, left.editor().scroll_delta);
    let (right_viewport, right_delta) = (right.editor().viewport, right.editor().scroll_delta);

    // The left follows the right down the file. Sideways it keeps its own
    // place: the lines on the two sides are not the same length.
    Effect::new(move |_| {
        let y = right_viewport.get().y0;
        let here = left_viewport.get_untracked();
        if here.y0 != y {
            left_scroll_to.set(Some(Vec2::new(here.x0, y)));
        }
    });
    #[cfg(test)]
    SPLIT_VIEWPORTS.with(|v| v.set(Some((left_viewport, right_viewport))));

    let left_pane = Stack::horizontal((
        side_gutter(state, app, file.clone(), Side::Base, left_doc, left_viewport, right_delta),
        Container::new(left)
            .on_event_stop(el::PointerWheel, move |_, wheel| {
                let size = left_viewport.get_untracked().size();
                let delta = wheel.resolve_to_points(Some(Size::new(line_height, line_height)), Some(size));
                // Down the file is the right's; across is the left's own.
                right_delta.set(Vec2::new(0.0, -delta.y));
                left_delta.set(Vec2::new(-delta.x, 0.0));
            })
            .style(|s| s.flex_grow(1.0_f32).min_width(0.0).height_full()),
    ))
    .style(|s| s.flex_basis(0.0).flex_grow(1.0_f32).min_width(0.0).height_full());

    let right_pane = Stack::horizontal((
        side_gutter(state, app, file, Side::Head, right_doc, right_viewport, right_delta),
        right,
    ))
    .style(|s| s.flex_basis(0.0).flex_grow(1.0_f32).min_width(0.0).height_full());

    Stack::horizontal((
        left_pane,
        Empty::new().style(|s| s.width(1.0).height_full().flex_shrink(0.0_f32).background(theme::BORDER)),
        right_pane,
    ))
    .style(|s| s.size_full().min_height(0.0))
}

#[cfg(test)]
thread_local! {
    /// The viewports of the last split built, left and right, so a test can
    /// watch the two scroll together.
    static SPLIT_VIEWPORTS: std::cell::Cell<Option<(RwSignal<floem::kurbo::Rect>, RwSignal<floem::kurbo::Rect>)>> =
        const { std::cell::Cell::new(None) };
}

/// A read-only editor over one document, drawn as code.
fn code_editor(text: String, styling: PerchStyling) -> TextEditor {
    text_editor(text)
        .read_only()
        .styling(styling)
        .editor_style(|s| {
            // Wrapping would put a long line on several visual rows, which
            // breaks the gutter's one-row-per-line geometry and makes a diff
            // hard to follow. Long lines scroll sideways instead.
            s.hide_gutter(true)
                .wrap_method(floem::views::editor::text::WrapMethod::None)
        })
        .style(|s| {
            s.size_full()
                .min_width(0.0)
                // Breathing room between the change bars and the first
                // character, so a bar never reads as part of the code.
                .padding_left(theme::CODE_GUTTER_GAP)
                .background(theme::CODE_BG)
        })
}

/// Clicking a symbol asks the language server about it, and the selection
/// becomes the lines a thread would be anchored on. Reading the editor's own
/// cursor rather than the pointer means floem's hit-testing does the
/// coordinate work. The document is the one the editor that owns the
/// cursor is laying out: the stacked one, or the head side of a split. It
/// is read as it is when the cursor moves, not as it was when the editor
/// was built, so a row is right after the document has been edited.
fn wire_cursor(
    state: ReviewState,
    backend: Rc<Backend>,
    file: Arc<LoadedFile>,
    doc: RwSignal<Arc<Doc>>,
    cursor: RwSignal<Cursor>,
) {
    Effect::new(move |previous: Option<usize>| {
        let offset = cursor.with(|c| c.offset());
        if previous == Some(offset) {
            return offset;
        }
        let doc = doc.get_untracked();

        let (row, _) = hover::line_col_of_offset(doc.text(), offset);
        if let Some(display_line) = doc.nearest_display_line(row) {
            state.cursor_line.set(display_line);
        }
        state.hover_text.set(None);

        // The selection, as display lines, for a thread's anchor. A caret
        // is a one-line selection. Rows that are no line at all fall out,
        // and a selection of nothing but those is no selection.
        let (lo, hi) = cursor.with(|c| {
            c.mode.regions_iter().fold((usize::MAX, 0usize), |(lo, hi), (a, b, _)| {
                (lo.min(a.min(b)), hi.max(a.max(b)))
            })
        });
        if lo != usize::MAX {
            let (first_row, _) = hover::line_col_of_offset(doc.text(), lo);
            let (last_row, _) = hover::line_col_of_offset(doc.text(), hi);
            let first = (first_row..=last_row).find_map(|r| doc.display_line(r));
            let last = (first_row..=last_row).rev().find_map(|r| doc.display_line(r));
            state.selection.set(first.zip(last));
        }

        if let Some((line, column)) = doc.head_position(offset) {
            state.last_position.set(Some((line, column)));
            let token = state.next_token();
            backend.hover(token, &file.path, line as u32, column as u32);
        } else {
            state.last_position.set(None);
        }
        offset
    });
}

/// A definition inside the worktree opens its file, then asks to be
/// scrolled to. The head line has to be translated through the row table
/// first, because the spliced diff shifts everything below a removal, and a
/// split's fillers do the same.
fn wire_jump(state: ReviewState, doc: RwSignal<Arc<Doc>>, scroll_to: RwSignal<Option<Vec2>>, epoch: u64) {
    Effect::new(move |_| {
        let Some(head_line) = state.jump_to_line.get() else {
            return;
        };
        // A jump written in the same breath as a file open lands here
        // before the container has swapped this editor for the new one:
        // effects run on the write, the rebuild waits for the update pass.
        // Left in place, the editor being built picks it up.
        if state.editor_epoch.get_untracked() != epoch {
            return;
        }
        state.jump_to_line.set(None);
        let row = doc.with_untracked(|d| d.row_of_head(head_line)).unwrap_or(head_line);
        // Land the target a few lines below the top rather than flush
        // against it, so its context is visible.
        let target = row.saturating_sub(4) as f64;
        scroll_to.set(Some(Vec2::new(0.0, target * theme::CODE_LINE_HEIGHT as f64)));
    });
}

// ---------------------------------------------------------------------------
// Split
// ---------------------------------------------------------------------------

/// Which half of a split. Not `perch_github::Side`: that one is where a
/// GitHub comment sits in a diff, and this one is only ever a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Base,
    Head,
}

/// The split for a loaded file, or `None` when there is no base to put on
/// the left. The head's text and statuses, and where each head line is
/// displayed, come out of the unified buffer, which has every head line in
/// order with the ghosts between.
fn split_of(file: &LoadedFile) -> Option<(Doc, Doc)> {
    let base = file.base_buffer.as_ref()?;
    if file.truncated || file.alignment.is_empty() {
        return None;
    }
    Some(doc::build_split(&file.alignment, &base.text, &file.buffer, &[], &[]))
}

/// What the language server says about the symbol under the cursor.
///
/// A strip below the code rather than a floating popup: it never covers the
/// line being read, and it needs no pointer coordinates to place.
fn symbol_panel(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    dyn_container(
        move || state.hover_text.get(),
        move |text| match text {
            None => Empty::new().into_any(),
            Some(text) => panel_body(state, backend.clone(), text).into_any(),
        },
    )
    .style(|s| s.width_full().flex_shrink(0.0_f32))
}

fn panel_body(state: ReviewState, backend: Rc<Backend>, text: String) -> impl IntoView {
    let condensed = hover::condense(&text, 12);

    Stack::vertical((
        Stack::horizontal((
            Label::derived(|| "Symbol".to_string()).style(|s| {
                s.font_size(theme::TEXT_SECTION)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(theme::FG_SUBTLE)
                    .flex_grow(1.0_f32)
            }),
            definition_button(state, backend.clone()),
            references_button(state, backend),
            dismiss_button(state),
        ))
        .style(|s| {
            s.width_full()
                .items_center()
                .gap(6.0)
                .padding_horiz(12.0)
                .height(26.0)
                .flex_shrink(0.0_f32)
        }),
        Label::derived(move || condensed.clone())
            .style(|s| {
                s.font_family(theme::CODE_FONT_STACK.to_string())
                    .font_size(theme::CODE_FONT_SIZE)
                    .color(theme::FG)
                    .line_height(1.45)
                    .padding_horiz(12.0)
                    .padding_bottom(10.0)
                    .width_full()
            })
            .scroll()
            .style(|s| s.width_full().max_height(180.0)),
    ))
    .style(|s| {
        s.width_full()
            .flex_col()
            .background(theme::CANVAS_SUBTLE)
            .border_top(1.0)
            .border_color(theme::BORDER)
    })
}

fn definition_button(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    panel_button("Go to definition", move || {
        let Some(file) = state.active_file() else { return };
        let Some(position) = state.last_position.get() else { return };
        let token = state.next_token();
        backend.definition(token, &file.path, position.0 as u32, position.1 as u32);
    })
}

/// Asks about the same symbol the panel is describing, the way the
/// definition button does.
fn references_button(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    panel_button("Find references", move || {
        let Some((line, column)) = state.last_position.get() else { return };
        references::find(state, &backend, line, Some(column));
    })
}

/// A text button in the panel's header row.
fn panel_button(text: &'static str, on_click: impl Fn() + 'static) -> impl IntoView {
    Container::new(Label::derived(move || text.to_string()).style(|s| {
        s.font_size(theme::TEXT_BADGE).color(theme::ACCENT)
    }))
    .on_event_stop(el::Click, move |_, _| on_click())
    .style(|s| {
        s.padding_horiz(8.0)
            .padding_vert(2.0)
            .border_radius(theme::RADIUS)
            .hover(|s| s.background(theme::ACCENT_SUBTLE).cursor(CursorStyle::Pointer))
    })
}

fn dismiss_button(state: ReviewState) -> impl IntoView {
    Container::new(
        crate::ui::widgets::sidebar_glyph(SidebarIcon::Close, theme::FG_SUBTLE)
            .style(|s| s.size(11.0, 11.0)),
    )
    .on_event_stop(el::Click, move |_, _| state.dismiss_hover())
    .style(|s| {
        s.size(18.0, 18.0)
            .items_center()
            .justify_center()
            .border_radius(3.0)
            .hover(|s| s.background(theme::BADGE_BG).cursor(CursorStyle::Pointer))
    })
}

fn loading_view(path: String) -> impl IntoView {
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
    Container::new(
        Label::derived(move || format!("Opening {name}…")).style(|s| {
            s.font_size(theme::TEXT_BODY).color(theme::FG_MUTED)
        }),
    )
    .style(|s| s.size_full().items_center().justify_center())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(doc: &Doc) -> Vec<&str> {
        doc.text.lines().collect()
    }

    #[test]
    fn a_split_needs_a_base_side() {
        let added = crate::backend::build_loaded_file(
            "new.rs".into(),
            "fn a() {}\n".into(),
            String::new(),
            Some(perch_git::ChangeStatus::Added),
            false,
            perch_git::Scope::Branch,
        );
        assert!(split_of(&added).is_none(), "an added file has nothing on the left");

        let plain = crate::backend::build_loaded_file(
            "lib.rs".into(),
            "fn a() {}\n".into(),
            String::new(),
            None,
            false,
            perch_git::Scope::Branch,
        );
        assert!(split_of(&plain).is_none(), "a file outside the changed list is shown plainly");

        let binary = crate::backend::build_loaded_file(
            "a.png".into(),
            String::new(),
            String::new(),
            Some(perch_git::ChangeStatus::Modified),
            true,
            perch_git::Scope::Branch,
        );
        assert!(split_of(&binary).is_none());

        let changed = crate::backend::build_loaded_file(
            "lib.rs".into(),
            "fn a() {}\nfn b() {}\n".into(),
            "fn a() {}\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
        );
        let (left, right) = split_of(&changed).expect("a changed file has both sides");
        assert_eq!(lines(&left), vec!["fn a() {}", ""]);
        assert_eq!(lines(&right), vec!["fn a() {}", "fn b() {}"]);
    }

    #[test]
    fn the_head_side_of_a_loaded_file_maps_rows_back_to_display_lines() {
        // base a b c d; head a B c: b rewritten, d removed. The unified
        // buffer is a, ghost(b), B, c, ghost(d): head line 1 is display
        // line 2, and the split's fourth row is a filler.
        let file = crate::backend::build_loaded_file(
            "x.rs".into(),
            "a\nB\nc\n".into(),
            "a\nb\nc\nd\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
        );
        let (_, right) = split_of(&file).unwrap();
        assert_eq!(right.display_line(1), Some(2));
        assert_eq!(right.display_line(3), None);
        assert_eq!(right.nearest_display_line(3), Some(3), "the row above, c, when nothing follows");
        assert_eq!(right.row_of_head(2), Some(2));
        // And the stacked document over the same file is the buffer.
        let stacked = doc::build(&file.buffer, &[], &[]);
        assert_eq!(stacked.text, file.buffer.text);
        assert_eq!(stacked.len(), file.buffer.len());
        assert_eq!(stacked.row_of_head(1), Some(2));
    }
}

/// The split in a headless window.
#[cfg(test)]
mod window {
    use std::rc::Rc;
    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::reactive::{SignalGet, SignalUpdate};
    use perch_github::{Item, ItemCheckout};

    use crate::review::model::{ItemTab, Layout, LiveDoc, LiveLayout, Phase, ReviewState};
    use crate::state::AppState;

    /// The whole window on Files with one changed file open, so the split
    /// is built where it lives: under the tab bar, beside the rail.
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
        // See the field: a focused editor cannot be driven headlessly.
        state.composer_autofocus.set(false);

        let file = Arc::new(crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "fn a() {}\nfn b() {}\nfn c() {}\n".repeat(30),
            "fn a() {}\nfn old() {}\nfn c() {}\n".repeat(30),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
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

    #[test]
    fn toggling_to_split_builds_both_sides_and_back_again() {
        let (mut harness, state) = harness();
        assert_eq!(state.layout.get(), Layout::Stacked);

        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();

        // Both editors are wired: a jump lands on the head's row.
        state.jump_to_line.set(Some(40));
        harness.rebuild();
        harness.paint();
        assert!(state.jump_to_line.get().is_none(), "the split's right editor took the jump");

        state.layout.set(Layout::Stacked);
        harness.rebuild();
        harness.paint();
    }

    /// The text an editor lays out is its document's, as the live entry
    /// holds them.
    fn laid_out_text(live: &LiveDoc) -> String {
        String::from(&live.handle.text())
    }

    #[test]
    fn every_editor_registers_its_live_document_for_the_epoch_it_was_built_in() {
        let (mut harness, state) = harness();
        let docs = state.docs.get().expect("the stacked editor registered its document");
        assert_eq!(docs.epoch, state.editor_epoch.get());
        let LiveLayout::Stacked(live) = docs.layout else {
            panic!("the stacked layout holds one document");
        };
        let file = state.active_file().unwrap();
        // The unified buffer has a ghost under every rewritten line.
        assert_eq!(live.doc.get().len(), 120);
        assert_eq!(live.doc.get().text, file.buffer.text);
        assert_eq!(laid_out_text(&live), file.buffer.text);

        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let docs = state.docs.get().expect("the split registered both documents");
        assert_eq!(docs.epoch, state.editor_epoch.get());
        let LiveLayout::Split { left, right } = docs.layout else {
            panic!("the split layout holds two documents");
        };
        assert_eq!(left.doc.get().len(), right.doc.get().len(), "the two sides are the same height");
        assert_eq!(right.doc.get().len(), 90);
        assert_eq!(laid_out_text(&left), left.doc.get().text);
        assert_eq!(laid_out_text(&right), right.doc.get().text);

        // Closing the file takes the documents with it.
        state.close_tab("src/x.rs");
        harness.rebuild();
        harness.paint();
        assert!(state.docs.get().is_none(), "no editor, no document");
    }

    /// Where the two editors sit: the activity rail, the sidebar and the
    /// file rail on the left, then the rest of the window in halves.
    fn pane_centers() -> (f64, f64, f64) {
        let left_edge = crate::theme::ACTIVITY_RAIL_WIDTH + crate::theme::SIDEBAR_WIDTH + crate::theme::RAIL_WIDTH;
        let half = (1400.0 - left_edge) / 2.0;
        let y = crate::theme::REVIEW_HEADER_HEIGHT + 2.0 * crate::theme::TAB_HEIGHT + 300.0;
        (left_edge + half * 0.6, left_edge + half * 1.6, y)
    }

    #[test]
    fn the_two_sides_scroll_as_one() {
        let (mut harness, state) = harness();
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let (left, right) = super::SPLIT_VIEWPORTS.with(|v| v.get()).expect("a split was built");
        let (x_left, x_right, y) = pane_centers();

        // The wheel over the right moves the right, and the left follows.
        harness.scroll_down(x_right, y, 200.0);
        harness.rebuild();
        harness.paint();
        assert!(right.get().y0 > 0.0, "the right did not scroll");
        assert_eq!(left.get().y0, right.get().y0, "the left did not follow");

        // The wheel over the left, which takes no pointer events itself,
        // is handed to the right, and the left follows that too.
        let before = right.get().y0;
        harness.scroll_down(x_left, y, 100.0);
        harness.rebuild();
        harness.paint();
        assert!(right.get().y0 > before, "a wheel over the left did not scroll the right");
        assert_eq!(left.get().y0, right.get().y0);

        // And a jump lands both on the same row.
        state.jump_to_line.set(Some(60));
        harness.rebuild();
        harness.paint();
        assert_eq!(left.get().y0, right.get().y0);
        assert_eq!(right.get().y0, (60.0 - 4.0) * crate::theme::CODE_LINE_HEIGHT as f64);
    }

    /// The left takes no pointer events, so a click there moves no cursor
    /// and changes no selection. (It would also focus the editor, which
    /// asks the window for IME, and the headless window has none to give:
    /// a left that took the click would fail this test by panicking.)
    #[test]
    fn a_click_on_the_left_is_nothing() {
        let (mut harness, state) = harness();
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        // The right's cursor starts on the first row, and says so.
        let (selection, line) = (state.selection.get(), state.cursor_line.get());
        let (x_left, _, y) = pane_centers();
        harness.click(x_left, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.selection.get(), selection);
        assert_eq!(state.cursor_line.get(), line);
    }

    /// The tiles sit at the strip's right end: 8 of padding, then Split
    /// then Stacked reading leftwards, each `LAYOUT_TILE` square with 2
    /// between, in a strip `TAB_HEIGHT` tall under the header and the
    /// item's tab strip.
    fn tile_center(layout: Layout) -> (f64, f64) {
        let tile = crate::theme::LAYOUT_TILE;
        let x = match layout {
            Layout::Split => 1400.0 - 8.0 - tile / 2.0,
            Layout::Stacked => 1400.0 - 8.0 - tile - 2.0 - tile / 2.0,
        };
        let y = crate::theme::REVIEW_HEADER_HEIGHT + crate::theme::TAB_HEIGHT + crate::theme::TAB_HEIGHT / 2.0;
        (x, y)
    }

    #[test]
    fn the_toggle_sets_the_layout() {
        let (mut harness, state) = harness();
        let (x, y) = tile_center(Layout::Split);
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.layout.get(), Layout::Split, "click at ({x}, {y}) over {under:?} did not split");

        let (x, y) = tile_center(Layout::Stacked);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.layout.get(), Layout::Stacked);
    }
}
