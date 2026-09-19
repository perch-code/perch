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
//! A reveal, Show whole, and a thread appearing, going or collapsing are
//! all that one edit: the inputs change, the builder makes the new
//! document, and `apply` turns the old text into it with the one region
//! that differs, so the scroll position and the layout cache survive.
//!
//! A card, for a thread of the reader's own, one of GitHub's review
//! threads or the composer, is drawn over the rows the document reserves
//! for it, by a layer over the editor that places each card from the
//! live document and the viewport, the way the gutter places its rows.
//! The rows are an estimate until the card has been laid out; then the
//! card says how tall it is, and when that is not the rows it has, the
//! rows are set and the document is edited like any other change to it.

use std::rc::Rc;
use std::sync::Arc;

use floem::context::LayoutChanged;
use floem::event::PointerScrollEventExt;
use floem::kurbo::{Rect, Size, Vec2};
use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, Position};
use floem::text::FontWeight;
use floem::views::editor::core::cursor::{Cursor, CursorAffinity, CursorMode};
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::scroll::ScrollCustomStyle;
use floem::views::{
    ClipExt, Container, Decorators, Empty, Label, ScrollExt, Stack, TextEditor, dyn_container, dyn_stack, text_editor,
};
use perch_github::CommentThread;

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::cards;
use crate::review::doc::{self, Card, CardId, Doc, Reveals, Run};
use crate::review::gutter::{diff_gutter, side_gutter, visible_lines};
use crate::review::hover;
use crate::review::model::{Composer, Layout, LiveDoc, LiveDocs, LiveLayout, LoadedFile, ReviewState};
use crate::review::references;
use crate::review::styling::PerchStyling;
use crate::review::threads::{self, ThreadFace};
use crate::review::timeline::LineThread;
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
    let code_open = open_file.clone();

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
                            code_view(app, state, file, epoch, layout, code_backend.clone(), code_open.clone()).into_any()
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

/// What a file's document is built from besides the file: whether the
/// worktree is folded, the head lines its threads are anchored on, the
/// reveals for its content, and its cards with the rows each has. Read
/// tracked, so an effect over them runs when any changes, or untracked
/// for the document an editor is built from.
struct Inputs {
    folded: bool,
    anchors: Vec<(usize, usize)>,
    reveals: Reveals,
    cards: Vec<Card>,
}

/// The anchors and the cards of a file, as memos: a thread's messages
/// change every line of a streaming answer, and a memo only tells its
/// readers when what it holds has changed, so the document is not built
/// again for a thread whose lines and rows are what they were.
#[derive(Clone, Copy)]
struct Memos {
    anchors: Memo<Vec<(usize, usize)>>,
    cards: Memo<Vec<Card>>,
}

impl Memos {
    fn new(state: ReviewState, file: &Arc<LoadedFile>) -> Self {
        let anchors_file = file.clone();
        let cards_file = file.clone();
        Self {
            anchors: Memo::new(move |_| state.anchors_in(&anchors_file.path)),
            cards: Memo::new(move |_| state.cards_in(&cards_file.path)),
        }
    }

    fn inputs(&self, state: ReviewState, file: &LoadedFile) -> Inputs {
        Inputs {
            folded: state.folded.get(),
            anchors: self.anchors.get(),
            reveals: state.reveals_for(&file.path, &file.blob),
            cards: self.cards.get(),
        }
    }
}

/// The file, stacked or split. A file with nothing to put on the left,
/// because it was added, is outside the changed list or is binary, is
/// stacked whatever the toggle says.
///
/// The documents are built here from the signals as they are, untracked:
/// the view is built once, and what changes under it edits the document
/// rather than building another editor.
fn code_view(
    app: AppState,
    state: ReviewState,
    file: Arc<LoadedFile>,
    epoch: u64,
    layout: Layout,
    backend: Rc<Backend>,
    open_file: OpenFile,
) -> impl IntoView {
    let memos = Memos::new(state, &file);
    let split = Effect::untrack(|| match layout {
        Layout::Split => {
            let inputs = memos.inputs(state, &file);
            split_doc(&file, inputs.folded, &inputs.anchors, &inputs.reveals, &inputs.cards)
        }
        Layout::Stacked => None,
    });
    match split {
        Some(split) => split_view(app, state, file, memos, split, epoch, backend, open_file).into_any(),
        None => stacked_view(app, state, file, memos, epoch, backend, open_file).into_any(),
    }
}

#[allow(clippy::too_many_arguments)]
fn stacked_view(
    app: AppState,
    state: ReviewState,
    file: Arc<LoadedFile>,
    memos: Memos,
    epoch: u64,
    backend: Rc<Backend>,
    open_file: OpenFile,
) -> impl IntoView {
    let built = Effect::untrack(|| {
        let inputs = memos.inputs(state, &file);
        stacked_doc(&file, inputs.folded, &inputs.anchors, &inputs.reveals, &inputs.cards)
    });
    let text = built.text.clone();
    let doc = RwSignal::new(Arc::new(built));
    let editor = code_editor(text, PerchStyling::new(file.clone(), doc, epoch));
    let live = LiveDoc { handle: editor.doc(), doc };
    state.docs.set(Some(LiveDocs { epoch, layout: LiveLayout::Stacked(live.clone()) }));

    // The gutter and the divider layer track the same signals the editor
    // lays out from, so they stay aligned without any scroll plumbing of
    // their own.
    let viewport = editor.editor().viewport;
    let scroll_delta = editor.editor().scroll_delta;
    let scroll_to = editor.editor().scroll_to;
    let cursor = editor.editor().cursor;
    // Where an edit last moved the cursor to; see `apply` and `wire_cursor`.
    let shifted = RwSignal::new(None);
    #[cfg(test)]
    STACKED_VIEWPORT.with(|v| v.set(Some(viewport)));
    #[cfg(test)]
    STACKED_CURSOR.with(|c| c.set(Some(cursor)));

    // The folds are wired before the jump: a jump into a fold reveals it,
    // and the reveal has to reach the editor before the jump looks for the
    // row to scroll to.
    wire_folds(state, file.clone(), memos, epoch, {
        let file = file.clone();
        move |inputs: &Inputs| {
            let new = stacked_doc(&file, inputs.folded, &inputs.anchors, &inputs.reveals, &inputs.cards);
            apply(&live, new, viewport, scroll_to, cursor, shifted)
        }
    });
    wire_cursor(state, backend.clone(), file.clone(), doc, cursor, shifted);
    wire_jump(state, file.clone(), doc, scroll_to, epoch);

    let path = file.path.clone();
    Stack::horizontal((
        diff_gutter(state, app, file, doc, viewport, scroll_delta),
        // The layers are placed against this box, so it takes the
        // relative position; the editor fills it. The cards are over the
        // dividers, since a card is never in a divider.
        Stack::vertical((
            editor,
            divider_layer(state, doc, viewport, scroll_delta, Bars::Editor),
            card_layer(app, state, backend, open_file, path, doc, viewport, scroll_delta, scroll_to, theme::CODE_GUTTER_GAP),
        ))
        .style(|s| s.flex_grow(1.0_f32).min_width(0.0).height_full().position(Position::Relative)),
    ))
    .style(|s| s.size_full().min_height(0.0))
}

/// The folds of a file: the runs around its changes and anchors less the
/// reveals, or none when the worktree's files are shown whole. A file
/// outside the changed list has no status and is never folded, whatever
/// is anchored in it: there is no change to fold around, and it opens
/// whole as it did before there were folds.
fn folds_of(file: &LoadedFile, folded: bool, anchors: &[(usize, usize)], reveals: &Reveals) -> Vec<Run> {
    if folded && file.status.is_some() {
        reveals.apply(&doc::runs(&file.buffer, anchors))
    } else {
        Vec::new()
    }
}

/// The stacked document of a file: folded to the changes and the anchors
/// less the reveals, or whole, with each card's rows reserved under its
/// line.
pub(crate) fn stacked_doc(
    file: &LoadedFile,
    folded: bool,
    anchors: &[(usize, usize)],
    reveals: &Reveals,
    cards: &[Card],
) -> Doc {
    doc::build(&file.buffer, &folds_of(file, folded, anchors, reveals), cards)
}

/// The two documents of a file's split, folded as the stacked one is, or
/// `None` when there is no base to put on the left. The head's text and
/// statuses, and where each head line is displayed, come out of the
/// unified buffer, which has every head line in order with the ghosts
/// between; the folds are in its display lines, and a divider or a
/// card's rows go into both sides at the same row.
pub(crate) fn split_doc(
    file: &LoadedFile,
    folded: bool,
    anchors: &[(usize, usize)],
    reveals: &Reveals,
    cards: &[Card],
) -> Option<(Doc, Doc)> {
    let base = file.base_buffer.as_ref()?;
    if file.truncated || file.alignment.is_empty() {
        return None;
    }
    let folds = folds_of(file, folded, anchors, reveals);
    Some(doc::build_split(&file.alignment, &base.text, &file.buffer, &folds, cards))
}

/// Runs `rebuild` whenever what a document is built from changes, so it
/// can build the new document and edit the editor to match. The inputs
/// are read tracked, so the effect runs on each of them: Show whole, the
/// file's reveals, and the anchors and the cards, which are memos so that
/// a thread's messages changing under them, as they do every line of a
/// streaming answer, do not rebuild a document that would be the same.
fn wire_folds(state: ReviewState, file: Arc<LoadedFile>, memos: Memos, epoch: u64, rebuild: impl Fn(&Inputs) + 'static) {
    Effect::new(move |first: Option<bool>| {
        let inputs = memos.inputs(state, &file);
        // The editor was just built from these; nothing to do but subscribe.
        if first.is_none() {
            return true;
        }
        if state.editor_epoch.get_untracked() != epoch {
            return true;
        }
        rebuild(&inputs);
        true
    });
}

/// Turns what the editor shows into `new`, in place.
///
/// The one region that differs between the two documents is replaced in
/// the editor's text, and the row table is set to the new document, so
/// everything reading rows through it is right from here on. The editor
/// keeps its scroll offset in pixels, so when the region begins above the
/// first visible row the offset is moved so that the first line in view
/// the new document still shows keeps its place on the screen, whatever
/// the region added or took away below it; when the region begins at or
/// below the top, nothing is moved and the code below the region moves.
/// The cursor's byte offset stays put through an edit, which would leave
/// it on other text, so an offset at or after the region is moved by the
/// bytes the edit added or took away, and the offset it lands on is left
/// in `shifted` for the cursor's effect, so that it does not take the move
/// for a click.
fn apply(
    live: &LiveDoc,
    new: Doc,
    viewport: RwSignal<Rect>,
    scroll_to: RwSignal<Option<Vec2>>,
    cursor: RwSignal<Cursor>,
    shifted: RwSignal<Option<usize>>,
) {
    let old = live.doc.get_untracked();
    let region = doc::changed_region(&old, &new);
    if *old != new {
        live.doc.set(Arc::new(new.clone()));
    }
    let Some(region) = region else {
        return;
    };
    let replacement = new.text[region.new_bytes.clone()].to_string();
    live.handle.edit_single(
        Selection::region(region.old_bytes.start, region.old_bytes.end, CursorAffinity::Backward),
        &replacement,
        EditType::Other,
    );

    let start = region.old_bytes.start;
    let delta = region.byte_delta();
    if cursor.with_untracked(|c| c.regions_iter().any(|(a, b, _)| a.max(b) >= start)) {
        // Said before the move, which runs the cursor's effect.
        shifted.set(Some(shifted_offset(cursor.with_untracked(|c| c.offset()), start, delta)));
        cursor.update(|c| shift_cursor(c, start, delta));
    }

    // Set whether or not the offset moves: moving the cursor asks the
    // editor to scroll it into view, and this, asked for after that, is
    // what the editor does last. Setting it also clamps the offset to the
    // new height, which a document that shrank under the scroll needs.
    let line_height = theme::CODE_LINE_HEIGHT as f64;
    let vp = viewport.get_untracked();
    let first_visible = (vp.y0 / line_height).floor() as usize;
    let y = if region.old_rows.start < first_visible {
        // The first line in view that the new document still shows keeps
        // its place; a region that spans the viewport, as Show whole's
        // does, adds and takes away below it too, and that is not counted.
        // When no line from the top of the view down is still shown, the
        // rows after the region keep their place instead.
        let shift = (first_visible..old.len())
            .find_map(|row| Some(new.row_of_display(old.display_line(row)?)? as isize - row as isize))
            .unwrap_or_else(|| region.row_delta());
        vp.y0 + shift as f64 * line_height
    } else {
        vp.y0
    };
    scroll_to.set(Some(Vec2::new(vp.x0, y.max(0.0))));
}

/// Where an offset lands after an edit of `delta` bytes at `start`: moved
/// by the delta when at or after it, and no further back than `start`, so
/// an offset inside the region the edit took away lands at the region's
/// beginning.
fn shifted_offset(offset: usize, start: usize, delta: isize) -> usize {
    if offset >= start {
        (offset as isize + delta).max(start as isize) as usize
    } else {
        offset
    }
}

/// Move every offset of the cursor as `shifted_offset` says.
fn shift_cursor(cursor: &mut Cursor, start: usize, delta: isize) {
    let shift = |offset: &mut usize| *offset = shifted_offset(*offset, start, delta);
    match &mut cursor.mode {
        CursorMode::Normal { offset, .. } => shift(offset),
        CursorMode::Visual { start, end, .. } => {
            shift(start);
            shift(end);
        }
        CursorMode::Insert(selection) => {
            for region in selection.regions_mut() {
                shift(&mut region.start);
                shift(&mut region.end);
            }
        }
    }
}

/// Where a divider layer's bars sit in the box the layer covers.
#[derive(Clone, Copy)]
enum Bars {
    /// Across the whole box: the stacked layout's editor.
    Editor,
    /// Across the head side of a split, whose layer covers the outer
    /// stack so that the bars are placed from the one scroll: the right
    /// half of it, less the gutter. The label and the controls are the
    /// head side's; the base side's row is its tint alone.
    HeadSide,
}

/// The dividers' labels, drawn over the editor at their rows: `47
/// unchanged lines` centred across the bar the row's own background
/// paints. Placed the way the gutter's column is, from the viewport, with
/// each bar carrying its own offset because the bars are sparse. The layer
/// takes no pointer events; each bar does, to record the hover that shows
/// the gutter's controls, and hands the wheel on to the editor.
fn divider_layer(
    state: ReviewState,
    doc: RwSignal<Arc<Doc>>,
    viewport: RwSignal<Rect>,
    scroll_delta: RwSignal<Vec2>,
    bars: Bars,
) -> impl IntoView {
    let line_height = theme::CODE_LINE_HEIGHT as f64;
    let visible = move || {
        let vp = viewport.get();
        doc.with(|d| {
            visible_lines(vp.y0, vp.height(), line_height, d.len())
                .into_iter()
                .filter_map(|row| d.run_of(row).map(|run| (row, run)))
                .collect::<Vec<_>>()
        })
    };
    let stack = dyn_stack(visible, |(row, run)| (*row, *run), move |(row, run)| {
        divider_bar(state, row, run, viewport, scroll_delta, bars)
    })
    .style(|s| s.size_full());

    Container::new(stack)
        .style(|s| s.size_full().position(Position::Relative))
        .clip()
        .style(|s| s.absolute().inset(0.0).size_full().pointer_events_none())
}

/// One divider's label over its row. The label is the part that takes
/// the pointer; in a split it is set into the bar where the head side's
/// editor is, and the rest of the bar lets the pointer through to the
/// base side and the gutters beneath.
fn divider_bar(
    state: ReviewState,
    row: usize,
    run: Run,
    viewport: RwSignal<Rect>,
    scroll_delta: RwSignal<Vec2>,
    bars: Bars,
) -> impl IntoView {
    let line_height = theme::CODE_LINE_HEIGHT as f64;
    let label = doc::divider_label(run.len());
    let label = Container::new(Label::derived(move || label.clone()).style(|s| {
        s.selectable(false)
            .font_family(theme::CODE_FONT_STACK.to_string())
            .font_size(theme::CODE_FONT_SIZE - 1.0)
            .color(theme::DIFF_DIVIDER_FG)
    }))
    .on_event_cont(el::PointerEnter, move |_, _| state.hovered_line.set(Some(row)))
    .on_event_cont(el::PointerLeave, move |_, _| {
        state.hovered_line.update(|h| {
            if *h == Some(row) {
                *h = None;
            }
        })
    })
    .on_event_stop(el::PointerWheel, move |_, wheel| {
        let size = viewport.get_untracked().size();
        let delta = wheel.resolve_to_points(Some(Size::new(line_height, line_height)), Some(size));
        scroll_delta.set(Vec2::new(-delta.x, -delta.y));
    })
    .style(|s| {
        s.flex_grow(1.0_f32)
            .min_width(0.0)
            .height_full()
            .items_center()
            .justify_center()
            .pointer_events_auto()
    });

    let bar = match bars {
        Bars::Editor => label.into_any(),
        // The outer stack's shape: two halves about a one-pixel rule, the
        // right one beginning with its gutter.
        Bars::HeadSide => Stack::horizontal((
            Empty::new().style(|s| s.flex_basis(0.0).flex_grow(1.0_f32).height_full()),
            Empty::new().style(|s| s.width(1.0).height_full().flex_shrink(0.0_f32)),
            Stack::horizontal((
                Empty::new().style(|s| s.width(theme::GUTTER_WIDTH).height_full().flex_shrink(0.0_f32)),
                label,
            ))
            .style(|s| s.flex_basis(0.0).flex_grow(1.0_f32).min_width(0.0).height_full()),
        ))
        .into_any(),
    };
    bar.style(move |s| {
        // Read live, as the gutter reads its offset: the bar outlives a
        // scroll, and a position captured when it was built would not.
        let top = row as f64 * line_height - viewport.get().y0;
        s.absolute().inset_top(top).inset_left(0.0).width_full().height(line_height)
    })
}

/// What a card in the layer is built from: a thread of the reader's own,
/// one of GitHub's, or the composer while it is open on lines of the
/// file. A sent question's card has no item: its rows stay in the
/// document, blank, until its thread arrives.
#[derive(Clone)]
enum CardItem {
    Thread(CommentThread),
    GitHub(LineThread),
    Composer(Composer),
}

impl CardItem {
    fn id(&self) -> CardId {
        match self {
            CardItem::Thread(t) => CardId::Thread(t.id.clone()),
            CardItem::GitHub(t) => CardId::GitHub(t.comment.id),
            CardItem::Composer(_) => CardId::Composer,
        }
    }

    /// What the card is keyed on: its id, and what changes what it is
    /// built from. A thread's messages are read live inside its card, so
    /// a streaming answer redraws in place, and the key carries whether
    /// the thread is outdated, which its first row and its starting
    /// state show, and how many messages it has. GitHub's carries the
    /// replies and the last edit. The composer's carries which opening
    /// it is and nothing else: picking a model writes the composer, and
    /// a rebuild on that would throw the typed question away.
    fn key(&self) -> String {
        match self {
            CardItem::Thread(t) => format!("thread:{}:{}:{}", t.id, t.outdated, t.messages.len()),
            CardItem::GitHub(t) => {
                let edited = std::iter::once(&t.comment).chain(t.replies.iter()).filter_map(|c| c.updated_at).max();
                format!("github:{}:{}:{edited:?}", t.comment.id, t.replies.len())
            }
            CardItem::Composer(c) => format!("composer:{}", c.opening),
        }
    }

    /// The name the card is found by in the tree.
    fn name(&self) -> String {
        match self {
            CardItem::Thread(t) => format!("inline-card:{}", t.id),
            CardItem::GitHub(t) => format!("inline-card:github:{}", t.comment.id),
            CardItem::Composer(_) => "inline-card:composer".to_string(),
        }
    }
}

/// The cards of the open file, drawn over the rows the document reserves
/// for them: the reader's threads anchored in it, GitHub's review threads
/// on it, and the composer when it is open on lines of it, as the
/// document has them. A layer over the editor, or over both panes of a
/// split, that takes no pointer events itself; each card does, and is
/// placed at its first spacer row from the live document and the
/// viewport in its own style closure, as the divider bars are, because
/// the cards are sparse and a card outlives the scroll. `left` is where a
/// card's edge sits: the first code column, past the gutter in a split.
#[allow(clippy::too_many_arguments)]
fn card_layer(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
    path: String,
    doc: RwSignal<Arc<Doc>>,
    viewport: RwSignal<Rect>,
    scroll_delta: RwSignal<Vec2>,
    scroll_to: RwSignal<Option<Vec2>>,
    left: f64,
) -> impl IntoView {
    let cards = move || {
        // The document says which cards have rows: a thread whose line is
        // not shown has no card. The threads, the discussion and the
        // composer are read tracked so a card's key follows them.
        let ids: Vec<CardId> = doc.with(|d| d.cards().iter().map(|(id, _)| id.clone()).collect());
        let github = state.github_threads_in(&path);
        let composer = state.composer.with(|c| {
            c.as_ref()
                .filter(|c| c.thread_id.is_none() && c.anchor.as_ref().is_some_and(|a| a.path == path))
                .cloned()
        });
        state.comment_threads.with(|threads| {
            ids.into_iter()
                .filter_map(|id| match &id {
                    CardId::Thread(thread_id) => threads.iter().find(|t| t.id == *thread_id).cloned().map(CardItem::Thread),
                    CardId::GitHub(comment_id) => {
                        github.iter().find(|t| t.comment.id == *comment_id).cloned().map(CardItem::GitHub)
                    }
                    CardId::Composer => composer.clone().map(CardItem::Composer),
                })
                .collect::<Vec<_>>()
        })
    };
    let stack = dyn_stack(cards, CardItem::key, move |item| {
        inline_card(app, state, backend.clone(), open_file.clone(), doc, viewport, scroll_delta, scroll_to, item, left)
    })
    .style(|s| s.size_full());

    Container::new(stack)
        .style(|s| s.size_full().position(Position::Relative))
        .clip()
        .style(|s| s.absolute().inset(0.0).size_full().pointer_events_none())
}

/// One card in the layer, over its rows: the column's card for a thread
/// of the reader's own in its inline face, the block for one of GitHub's
/// without its heading and lines, or the composer, set into its first
/// spacer row a little way in, so its border never touches a tinted
/// line, from the first code column to the right edge less the same
/// margin. Collapsed, it is its one row whole. The wheel over it scrolls
/// the code, as it does over the gutter, unless what is inside it takes
/// the wheel first.
///
/// A gutter marker click on the thread scrolls the editor so that the
/// card is in view, with the line above it; the card flashes itself.
///
/// The card's rows are an estimate until it has been laid out. Every
/// layout tells the card its box, and when the rows that box needs are
/// not the rows the document has for it, the rows are set and the
/// document follows, as it follows any other change to its inputs. Only
/// when they differ: an absolutely placed card is laid out again on
/// every scroll, and a write on each would edit the document, lay the
/// card out again and write again without end. A collapsed card is its
/// one row whatever it measures, so it says nothing.
#[allow(clippy::too_many_arguments)]
fn inline_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
    doc: RwSignal<Arc<Doc>>,
    viewport: RwSignal<Rect>,
    scroll_delta: RwSignal<Vec2>,
    scroll_to: RwSignal<Option<Vec2>>,
    item: CardItem,
    left: f64,
) -> impl IntoView {
    let line_height = theme::CODE_LINE_HEIGHT as f64;
    let card_id = item.id();
    let name = item.name();

    let focus_id = card_id.clone();
    Effect::new(move |first: Option<()>| {
        let focused = state.focus_card.with(|f| f.as_ref() == Some(&focus_id));
        if first.is_none() || !focused {
            return;
        }
        let Some(span) = doc.with_untracked(|d| d.card_span(&focus_id)) else {
            return;
        };
        let vp = viewport.get_untracked();
        let (top, bottom) = (span.start as f64 * line_height, span.end as f64 * line_height);
        if let Some(y) = scroll_for_card(vp, top, bottom, line_height) {
            scroll_to.set(Some(Vec2::new(vp.x0, y)));
        }
    });

    let view = match item {
        CardItem::Thread(thread) => {
            threads::thread_card(app, state, backend, open_file, thread, ThreadFace::Inline).into_any()
        }
        CardItem::GitHub(thread) => cards::line_thread_block(app, state, backend, open_file, thread, ThreadFace::Inline),
        CardItem::Composer(composer) => threads::composer_view(app, state, backend, composer, ThreadFace::Inline).into_any(),
    };
    let measured_id = card_id.clone();
    view.debug_name(name)
        .on_event_stop(LayoutChanged::listener(), move |_, LayoutChanged { new_box, .. }| {
            if state.is_collapsed(&measured_id) {
                return;
            }
            let rows = rows_for_height(new_box.height(), line_height, theme::CARD_INSET);
            let has = doc.with_untracked(|d| d.card_span(&measured_id).map(|span| span.len()));
            if has.is_some_and(|has| has != rows) {
                state.set_card_rows(&measured_id, rows);
            }
        })
        .on_event_stop(el::PointerWheel, move |_, wheel| {
            let size = viewport.get_untracked().size();
            let delta = wheel.resolve_to_points(Some(Size::new(line_height, line_height)), Some(size));
            scroll_delta.set(Vec2::new(-delta.x, -delta.y));
        })
        .style(move |s| {
            // Read live, as the gutter reads its offset: the card outlives
            // a scroll and an edit above it, and a position captured when
            // it was built would not.
            let row = doc.with(|d| d.first_spacer_row(&card_id)).unwrap_or(0);
            let inset = if state.is_collapsed(&card_id) { 0.0 } else { theme::CARD_INSET };
            let top = row as f64 * line_height - viewport.get().y0 + inset;
            s.absolute()
                .inset_top(top)
                .inset_left(left)
                .inset_right(theme::CODE_GUTTER_GAP)
                .pointer_events_auto()
        })
}

/// How many rows a card laid out `height` tall takes: the height with the
/// inset the card sits inside its first and last rows put back, in whole
/// rows, rounded up, so the border never touches the line after the
/// card. Never fewer than one, and never more than a card may take: the
/// card scrolls inside itself at that height, so the rows it is given
/// stop where the estimate's cap does.
fn rows_for_height(height: f64, line_height: f64, inset: f64) -> usize {
    let rows = ((height + 2.0 * inset) / line_height).ceil().max(1.0) as usize;
    rows.min(doc::CARD_MAX_ROWS)
}

/// Where the editor scrolls to so that a card from `top` to `bottom` is
/// in view, with the line above it, or `None` when it is in view already:
/// a card above the view comes down to the top of it, and one below comes
/// up to the bottom, but no further than puts its top out of view, so a
/// card taller than the view shows its beginning.
fn scroll_for_card(vp: Rect, top: f64, bottom: f64, line_height: f64) -> Option<f64> {
    if top >= vp.y0 && bottom <= vp.y1 {
        return None;
    }
    let with_line_above = (top - line_height).max(0.0);
    if top < vp.y0 {
        Some(with_line_above)
    } else {
        Some((bottom - vp.height()).min(with_line_above).max(0.0))
    }
}

/// Base on the left, head on the right, over one scroll position.
///
/// Both editors keep a scroll of their own, so the two are tied together:
/// the right one's viewport drives the left through `scroll_to`, and a
/// wheel over the left, which takes no pointer events, is handed to the
/// right. The left's scrollbar is hidden, so there is one.
///
/// The two documents are edited in the same pass, from the pair the
/// builder returns, so the two stay the same height: the dividers and the
/// cards' rows are on both sides at the same rows, and the layers over
/// the outer stack draw the labels on the head side and the cards across
/// both sides from the one scroll.
#[allow(clippy::too_many_arguments)]
fn split_view(
    app: AppState,
    state: ReviewState,
    file: Arc<LoadedFile>,
    memos: Memos,
    split: (Doc, Doc),
    epoch: u64,
    backend: Rc<Backend>,
    open_file: OpenFile,
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
    let left_live = LiveDoc { handle: left.doc(), doc: left_doc };
    let right_live = LiveDoc { handle: right.doc(), doc: right_doc };
    state.docs.set(Some(LiveDocs {
        epoch,
        layout: LiveLayout::Split { left: left_live.clone(), right: right_live.clone() },
    }));

    let (left_viewport, left_scroll_to, left_delta, left_cursor) =
        (left.editor().viewport, left.editor().scroll_to, left.editor().scroll_delta, left.editor().cursor);
    let (right_viewport, right_scroll_to, right_delta, right_cursor) =
        (right.editor().viewport, right.editor().scroll_to, right.editor().scroll_delta, right.editor().cursor);
    // Where an edit last moved each cursor to; only the right's is read,
    // since only the right's cursor is wired.
    let (left_shifted, right_shifted) = (RwSignal::new(None), RwSignal::new(None));

    // The folds are wired before the jump, as in the stacked layout. The
    // right is edited first: it owns the scroll, and the left follows.
    wire_folds(state, file.clone(), memos, epoch, {
        let file = file.clone();
        move |inputs: &Inputs| {
            let Some((new_left, new_right)) =
                split_doc(&file, inputs.folded, &inputs.anchors, &inputs.reveals, &inputs.cards)
            else {
                return;
            };
            apply(&right_live, new_right, right_viewport, right_scroll_to, right_cursor, right_shifted);
            apply(&left_live, new_left, left_viewport, left_scroll_to, left_cursor, left_shifted);
        }
    });
    wire_cursor(state, backend.clone(), file.clone(), right_doc, right_cursor, right_shifted);
    wire_jump(state, file.clone(), right_doc, right_scroll_to, epoch);

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

    let path = file.path.clone();
    let right_pane = Stack::horizontal((
        side_gutter(state, app, file, Side::Head, right_doc, right_viewport, right_delta),
        right,
    ))
    .style(|s| s.flex_basis(0.0).flex_grow(1.0_f32).min_width(0.0).height_full());

    // The layers are placed against the outer stack, so it takes the
    // relative position; the bars are set into it where the head side is,
    // and a card spans both sides from the base side's first code column,
    // past its gutter.
    Stack::horizontal((
        left_pane,
        Empty::new().style(|s| s.width(1.0).height_full().flex_shrink(0.0_f32).background(theme::BORDER)),
        right_pane,
        divider_layer(state, right_doc, right_viewport, right_delta, Bars::HeadSide),
        card_layer(
            app,
            state,
            backend,
            open_file,
            path,
            right_doc,
            right_viewport,
            right_delta,
            right_scroll_to,
            theme::GUTTER_WIDTH + theme::CODE_GUTTER_GAP,
        ),
    ))
    .style(|s| s.size_full().min_height(0.0).position(Position::Relative))
}

#[cfg(test)]
thread_local! {
    /// The viewports of the last split built, left and right, so a test can
    /// watch the two scroll together.
    static SPLIT_VIEWPORTS: std::cell::Cell<Option<(RwSignal<Rect>, RwSignal<Rect>)>> =
        const { std::cell::Cell::new(None) };
    /// The viewport of the last stacked editor built, so a test can see
    /// that an edit left it where it was.
    static STACKED_VIEWPORT: std::cell::Cell<Option<RwSignal<Rect>>> = const { std::cell::Cell::new(None) };
    /// The cursor of the last stacked editor built, so a test can place it
    /// where a click would, since a headless editor cannot take a click.
    static STACKED_CURSOR: std::cell::Cell<Option<RwSignal<Cursor>>> = const { std::cell::Cell::new(None) };
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
///
/// An edit above the cursor moves its offset to keep it on its text, and
/// `apply` leaves the offset it moved it to in `shifted`. That move is no
/// click: the line and the selection are read again, since their rows may
/// have moved, but the symbol under the cursor is the one it was on, so
/// the panel stays as it is, open or dismissed, and nothing is asked again.
fn wire_cursor(
    state: ReviewState,
    backend: Rc<Backend>,
    file: Arc<LoadedFile>,
    doc: RwSignal<Arc<Doc>>,
    cursor: RwSignal<Cursor>,
    shifted: RwSignal<Option<usize>>,
) {
    Effect::new(move |previous: Option<usize>| {
        let offset = cursor.with(|c| c.offset());
        // Taken whichever way this run goes, so that a later click landing
        // on the same offset is a click.
        let by_edit = shifted.try_update(|s| s.take()).flatten() == Some(offset);
        if previous == Some(offset) {
            return offset;
        }
        let doc = doc.get_untracked();

        let (row, _) = hover::line_col_of_offset(doc.text(), offset);
        if let Some(display_line) = doc.nearest_display_line(row) {
            state.cursor_line.set(display_line);
        }

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

        if by_edit {
            return offset;
        }
        state.hover_text.set(None);
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
/// split's fillers do the same. A line inside a fold is on no row: the
/// fold is revealed around it first, like any other reveal, and Reset
/// undoes it like any other.
fn wire_jump(
    state: ReviewState,
    file: Arc<LoadedFile>,
    doc: RwSignal<Arc<Doc>>,
    scroll_to: RwSignal<Option<Vec2>>,
    epoch: u64,
) {
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
        let mut row = doc.with_untracked(|d| d.row_of_head(head_line));
        if row.is_none() && state.folded.get_untracked() && file.buffer.display_line(head_line).is_some() {
            // The reveal reaches the editor before this returns: the
            // effect that rebuilds the document runs on the write.
            state.reveal(
                &file.path,
                &file.blob,
                head_line.saturating_sub(doc::CONTEXT_LINES),
                head_line + doc::CONTEXT_LINES,
            );
            row = doc.with_untracked(|d| d.row_of_head(head_line));
        }
        // A line the file does not have lands at the end of it.
        let row = row.unwrap_or_else(|| doc.with_untracked(|d| d.len().saturating_sub(1)));
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

    /// A file's split shown whole, with nothing folded.
    fn split_of(file: &LoadedFile) -> Option<(Doc, Doc)> {
        split_doc(file, false, &[], &Reveals::default(), &[])
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
            String::new(),
        );
        assert!(split_of(&added).is_none(), "an added file has nothing on the left");

        let plain = crate::backend::build_loaded_file(
            "lib.rs".into(),
            "fn a() {}\n".into(),
            String::new(),
            None,
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        assert!(split_of(&plain).is_none(), "a file outside the changed list is shown plainly");

        let binary = crate::backend::build_loaded_file(
            "a.png".into(),
            String::new(),
            String::new(),
            Some(perch_git::ChangeStatus::Modified),
            true,
            perch_git::Scope::Branch,
            String::new(),
        );
        assert!(split_of(&binary).is_none());

        let changed = crate::backend::build_loaded_file(
            "lib.rs".into(),
            "fn a() {}\nfn b() {}\n".into(),
            "fn a() {}\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
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
            String::new(),
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

    #[test]
    fn a_file_outside_the_changed_list_is_whole_whatever_is_anchored_in_it() {
        let text: String = (0..30).map(|i| format!("line {i}\n")).collect();
        let plain = crate::backend::build_loaded_file(
            "lib.rs".into(),
            text.clone(),
            String::new(),
            None,
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        // A thread on line 11 would split the file into two runs, were it
        // a changed file; there is no change to fold around, so it is not.
        let anchors = [(10, 10)];
        assert_eq!(doc::runs(&plain.buffer, &anchors).len(), 2, "the runs are there to fold");
        let doc = stacked_doc(&plain, true, &anchors, &Reveals::default(), &[]);
        assert!(doc.folds.is_empty());
        assert_eq!(doc.hidden_lines(), 0);
        assert_eq!(doc.text, text);
        assert_eq!(doc.len(), 30);

        // A changed file with the same anchor folds around it.
        let changed = crate::backend::build_loaded_file(
            "lib.rs".into(),
            text.clone(),
            format!("{text}old end\n"),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        let doc = stacked_doc(&changed, true, &anchors, &Reveals::default(), &[]);
        assert_eq!(doc.folds.len(), 2);
        assert!(doc.hidden_lines() > 0);
    }

    #[test]
    fn a_card_scrolls_into_view_only_when_it_is_out_of_it() {
        let lh = 21.0;
        // A view of twenty rows, from row 10.
        let vp = Rect::new(0.0, 10.0 * lh, 800.0, 30.0 * lh);
        let card = |first: f64, rows: f64| (first * lh, (first + rows) * lh);
        let (top, bottom) = card(12.0, 5.0);
        assert_eq!(scroll_for_card(vp, top, bottom, lh), None, "wholly in view");
        // Above: down to the top, with the line above it.
        let (top, bottom) = card(4.0, 5.0);
        assert_eq!(scroll_for_card(vp, top, bottom, lh), Some(3.0 * lh));
        let (top, bottom) = card(0.0, 5.0);
        assert_eq!(scroll_for_card(vp, top, bottom, lh), Some(0.0), "never above the file");
        // Below, or hanging out of the bottom: up to the bottom.
        let (top, bottom) = card(40.0, 5.0);
        assert_eq!(scroll_for_card(vp, top, bottom, lh), Some(25.0 * lh));
        let (top, bottom) = card(28.0, 5.0);
        assert_eq!(scroll_for_card(vp, top, bottom, lh), Some(13.0 * lh));
        // Taller than the view: its beginning, with the line above it.
        let (top, bottom) = card(40.0, 40.0);
        assert_eq!(scroll_for_card(vp, top, bottom, lh), Some(39.0 * lh));
    }

    #[test]
    fn a_cards_rows_are_its_height_with_the_inset_in_whole_rows_up_to_the_cap() {
        let (lh, inset) = (21.0, 3.0);
        // A card that fills five rows less the inset at each end is five;
        // a pixel more spills into a sixth; a pixel less is still five.
        let five = 5.0 * lh - 2.0 * inset;
        assert_eq!(rows_for_height(five, lh, inset), 5);
        assert_eq!(rows_for_height(five + 1.0, lh, inset), 6);
        assert_eq!(rows_for_height(five - 1.0, lh, inset), 5);
        assert_eq!(rows_for_height(five - lh, lh, inset), 4);
        // Never fewer than one, however little is there.
        assert_eq!(rows_for_height(0.0, lh, inset), 1);
        assert_eq!(rows_for_height(lh - 2.0 * inset, lh, inset), 1);
        // A card as tall as its rows at the cap allow is the cap, not a
        // row over it, and nothing taller is more.
        let cap = doc::CARD_MAX_ROWS as f64 * lh - 2.0 * inset;
        assert_eq!(rows_for_height(cap, lh, inset), doc::CARD_MAX_ROWS);
        assert_eq!(rows_for_height(cap - lh, lh, inset), doc::CARD_MAX_ROWS - 1);
        assert_eq!(rows_for_height(cap + 100.0 * lh, lh, inset), doc::CARD_MAX_ROWS);
    }
}

/// The split in a headless window.
#[cfg(test)]
mod window {
    use std::rc::Rc;
    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::reactive::{SignalGet, SignalUpdate};
    use floem::views::editor::core::cursor::CursorAffinity;
    use perch_github::{Item, ItemCheckout};

    use crate::review::model::{ItemTab, Layout, LiveDoc, LiveLayout, Phase, ReviewState};
    use crate::state::AppState;

    /// The whole window on Files with one changed file open, so the split
    /// is built where it lives: under the tab bar, beside the rail. The
    /// file is a 90-line one changed every third line, so nothing in it
    /// folds and a row is its head line.
    fn harness() -> (HeadlessHarness, ReviewState) {
        harness_with(
            "fn a() {}\nfn b() {}\nfn c() {}\n".repeat(30),
            "fn a() {}\nfn old() {}\nfn c() {}\n".repeat(30),
        )
    }

    /// The window with a changed file of the given head and base open.
    fn harness_with(head: String, base: String) -> (HeadlessHarness, ReviewState) {
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
            head,
            base,
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

    /// The Show whole tile is left of Stacked, past the rule and the room
    /// on either side of it.
    fn fold_tile_center() -> (f64, f64) {
        let (stacked, y) = tile_center(Layout::Stacked);
        let tile = crate::theme::LAYOUT_TILE;
        (stacked - tile / 2.0 - crate::theme::TILE_RULE_GAP - 1.0 - crate::theme::TILE_RULE_GAP - tile / 2.0, y)
    }

    /// A pull request for the item, so the status bar draws the review's
    /// line rather than the item count.
    fn pull_request() -> perch_github::PullRequest {
        perch_github::PullRequest {
            number: 812,
            title: "Fold the diff".into(),
            author_login: "octocat".into(),
            author_avatar_url: String::new(),
            head_sha: "h".into(),
            head_ref: "fold".into(),
            base_sha: "m".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
            additions: 1,
            deletions: 1,
            changed_files: 1,
            repo_full_name: "slatedb/slatedb".into(),
            head_repo_full_name: None,
            updated_at: None,
            review_requested: false,
            checks: None,
            review_decision: None,
            merged_at: None,
            merged_by: None,
            closed_at: None,
            requested_reviewers: Vec::new(),
            milestone: None,
            labels: Vec::new(),
            assignees: Vec::new(),
        }
    }

    #[test]
    fn show_whole_unfolds_every_run_and_folding_again_keeps_the_reveals() {
        let (mut harness, state) = one_long_run();
        let live = stacked(state);
        // With a pull request open the status bar carries the review's
        // line, and with it the hidden count.
        state.pull_request.set(Some(Arc::new(pull_request())));
        harness.rebuild();
        harness.paint();
        assert!(state.folded.get(), "folded by default");
        assert_eq!(state.hidden_lines(), 54);

        // Twenty lines at the bottom of the run, to be kept through Show
        // whole and back.
        let run = live.doc.get().run_of(5).expect("row 5 is the divider");
        let (first, last) = run.bottom();
        state.reveal("src/x.rs", "", first, last);
        harness.rebuild();
        harness.paint();
        assert_eq!(live.doc.get().len(), 103);
        assert_eq!(state.hidden_lines(), 34);

        let (x, y) = fold_tile_center();
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert!(!state.folded.get(), "click at ({x}, {y}) over {under:?} did not show the files whole");
        let doc = live.doc.get();
        assert_eq!(doc.len(), 64 + 72, "every line, no divider");
        assert!(doc.folds.is_empty());
        assert_eq!(state.hidden_lines(), 0, "nothing hidden, so the status line says nothing");
        assert_eq!(laid_out_text(&live), doc.text);

        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert!(state.folded.get());
        assert_eq!(live.doc.get().len(), 103, "folded again, with the reveal as it was");
        assert_eq!(state.hidden_lines(), 34);
        assert_eq!(laid_out_text(&live), live.doc.get().text);
    }

    // Folds.

    /// `n` lines of context, numbered from `from`.
    fn context(from: usize, n: usize) -> String {
        (from..from + n).map(|i| format!("line {i}\n")).collect()
    }

    /// A changed first line, sixty unchanged, a changed last line, and
    /// then twelve changes four lines apart so the document is long
    /// enough to scroll: one run of fifty-four behind a divider at row 5,
    /// with Up and Down, and nothing else folded.
    fn one_long_run() -> (HeadlessHarness, ReviewState) {
        let blocks = |word: &str| (0..12).map(|i| format!("{word}{i}\n{}", context(60 + i * 4, 4))).collect::<String>();
        harness_with(
            format!("new top\n{}new bottom\n{}", context(0, 60), blocks("new")),
            format!("old top\n{}old bottom\n{}", context(0, 60), blocks("old")),
        )
    }

    /// The stacked editor's live document.
    fn stacked(state: ReviewState) -> LiveDoc {
        match state.docs.get().expect("an editor is open").layout {
            LiveLayout::Stacked(live) => live,
            LiveLayout::Split { .. } => panic!("the layout is stacked"),
        }
    }

    /// The split's two live documents, left and right.
    fn sides(state: ReviewState) -> (LiveDoc, LiveDoc) {
        match state.docs.get().expect("an editor is open").layout {
            LiveLayout::Split { left, right } => (left, right),
            LiveLayout::Stacked(_) => panic!("the layout is split"),
        }
    }

    /// Where the code pane starts: after the open drawer and the file
    /// rail, under the header and the two strips.
    fn pane_origin() -> (f64, f64) {
        (
            crate::theme::SIDEBAR_WIDTH + crate::theme::RAIL_WIDTH,
            crate::theme::REVIEW_HEADER_HEIGHT + 2.0 * crate::theme::TAB_HEIGHT,
        )
    }

    #[test]
    fn a_divider_row_exists_and_down_on_its_gutter_grows_the_document_without_moving_the_viewport() {
        let (mut harness, state) = one_long_run();
        let live = stacked(state);
        let line_height = crate::theme::CODE_LINE_HEIGHT as f64;
        // Display: ghost, new top, line 0..59 at 2 to 61, ghost, new
        // bottom, then the twelve blocks of six. The run is display lines
        // 5 to 58, head lines 4 to 57.
        let doc = live.doc.get();
        assert_eq!(doc.len(), 64 + 72 - 54 + 1);
        let run = doc.run_of(5).expect("row 5 is the divider");
        assert_eq!(run.len(), 54);
        assert_eq!(laid_out_text(&live), doc.text);
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).expect("a stacked editor was built");
        let y0 = viewport.get().y0;

        // The controls show while the row is hovered: Up and Down, 16
        // wide with 2 between, centred in the gutter's 70. Down's centre
        // is 44 from the gutter's left.
        let (left, top) = pane_origin();
        let (x, y) = (left + 44.0, top + 5.0 * line_height + line_height / 2.0);
        harness.pointer_move(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.hovered_line.get(), Some(5), "the divider's gutter row records the pointer");
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();

        let doc = live.doc.get();
        assert_eq!(doc.len(), 83 + 20, "click at ({x}, {y}) over {under:?} did not reveal twenty lines");
        assert_eq!(doc.run_of(5).map(|r| r.len()), Some(34), "the divider stays, hiding twenty fewer");
        assert_eq!(doc.row_of_head(38), Some(6), "the first revealed line follows the divider");
        assert_eq!(laid_out_text(&live), doc.text, "the editor's text is the document's");
        assert_eq!(viewport.get().y0, y0, "the viewport did not move");
        assert!(state.reveals_for("src/x.rs", "").overlaps(run.default_heads.0, run.default_heads.1));
    }

    /// An unshifted bracket key, as the window receives it.
    fn bracket(right: bool) -> floem::event::Event {
        use floem::prelude::{Code, Key};
        if right {
            key_press(Code::BracketRight, Key::Character("]".into()))
        } else {
            key_press(Code::BracketLeft, Key::Character("[".into()))
        }
    }

    /// An unmodified key press, as the window receives it.
    fn key_press(code: floem::prelude::Code, key: floem::prelude::Key) -> floem::event::Event {
        use floem::prelude::{KeyState, KeyboardEvent, Modifiers};
        floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key,
            code,
            location: Default::default(),
            modifiers: Modifiers::empty(),
            repeat: false,
            is_composing: false,
        })
    }

    #[test]
    fn the_bracket_keys_go_between_the_changes() {
        let (mut harness, state) = one_long_run();
        let live = stacked(state);
        let line_height = crate::theme::CODE_LINE_HEIGHT as f64;
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).unwrap();
        assert_eq!(state.cursor_line.get(), 0);

        // A bare key goes to whatever has the keyboard, and the window's
        // handler catches it on the way down. A click in the code would
        // give it to the editor, which cannot be focused headlessly; a
        // click on the gutter gives it to the gutter. The divider's row,
        // at its left edge, away from the controls.
        let (left, top) = pane_origin();
        harness.click(left + 5.0, top + 5.0 * line_height + line_height / 2.0);
        harness.rebuild();

        // The next change after the top starts at the ghost of old bottom,
        // display line 62; the first head line from there, new bottom, is
        // head line 61, on row 11 of the folded document after the two
        // ghosts of the rewritten pair.
        harness.dispatch_event(bracket(true));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.cursor_line.get(), 62);
        assert!(state.jump_to_line.get().is_none(), "the editor took the jump");
        assert_eq!(live.doc.get().row_of_head(61), Some(11));
        assert_eq!(viewport.get().y0, (11.0 - 4.0) * line_height);
        assert_eq!(live.doc.get().len(), 83, "a change is never folded, so nothing was revealed");

        harness.dispatch_event(bracket(false));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.cursor_line.get(), 0);
        assert_eq!(viewport.get().y0, 0.0);
    }

    #[test]
    fn a_reveal_above_the_viewport_moves_the_scroll_so_the_visible_lines_stay() {
        // A hundred unchanged lines, then eight changes four lines apart:
        // the run at the top folds from line one, and the rest is long
        // enough to scroll.
        let blocks = |word: &str| (0..8).map(|i| format!("{word}{i}\n{}", context(100 + i * 4, 4))).collect::<String>();
        let (head, base) = (format!("{}{}", context(0, 100), blocks("new")), format!("{}{}", context(0, 100), blocks("old")));
        let (mut harness, state) = harness_with(head, base);
        let live = stacked(state);
        let line_height = crate::theme::CODE_LINE_HEIGHT as f64;
        let run = live.doc.get().run_of(0).expect("the top run folds from line one");
        assert_eq!(run.len(), 97);
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).unwrap();

        let (left, top) = pane_origin();
        harness.scroll_down(left + 300.0, top + 300.0, 200.0);
        harness.rebuild();
        harness.paint();
        let before = viewport.get().y0;
        assert!(before >= line_height, "the editor did not scroll past the divider");
        let first_row = (before / line_height).floor() as usize;
        let first_line = live.doc.get().display_line(first_row);

        let (first, last) = run.bottom();
        state.reveal("src/x.rs", "", first, last);
        harness.rebuild();
        harness.paint();
        assert_eq!(live.doc.get().len(), 52 + 20);
        assert_eq!(viewport.get().y0, before + 20.0 * line_height, "the scroll moved by the rows added above it");
        let doc = live.doc.get();
        assert_eq!(doc.display_line(first_row + 20), first_line, "the same line is first in view");
    }

    /// Three changes fifty lines apart, then twelve four lines apart so
    /// the document is long enough to scroll: three runs of forty-four,
    /// so each divider offers Up and Down. Stacked, the runs are display
    /// lines 5 to 48, 57 to 100 and 109 to 152 of 228, and the folded
    /// document's dividers are at rows 5, 14 and 23 of 99.
    fn three_runs() -> (HeadlessHarness, ReviewState) {
        let blocks = |word: &str| (0..12).map(|i| format!("{word}{i}\n{}", context(150 + i * 4, 4))).collect::<String>();
        harness_with(
            format!(
                "new top\n{}new mid\n{}new bottom\n{}{}",
                context(0, 50),
                context(50, 50),
                context(100, 50),
                blocks("new")
            ),
            format!(
                "old top\n{}old mid\n{}old bottom\n{}{}",
                context(0, 50),
                context(50, 50),
                context(100, 50),
                blocks("old")
            ),
        )
    }

    #[test]
    fn show_whole_with_dividers_above_and_below_the_viewport_keeps_the_same_line_in_view() {
        let (mut harness, state) = three_runs();
        let live = stacked(state);
        let line_height = crate::theme::CODE_LINE_HEIGHT as f64;
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).expect("a stacked editor was built");
        let doc = live.doc.get();
        assert_eq!(doc.len(), 99);
        for row in [5, 14, 23] {
            assert!(doc.run_of(row).is_some(), "row {row} is a divider");
        }

        // Scroll to between the second divider and the third, so that
        // Show whole opens runs above the view and below it.
        let (left, top) = pane_origin();
        harness.scroll_down(left + 300.0, top + 300.0, 16.0 * line_height);
        harness.rebuild();
        harness.paint();
        let before = viewport.get().y0;
        let first_row = (before / line_height).floor() as usize;
        assert!((15..23).contains(&first_row), "the editor scrolled to row {first_row}, not between the dividers");
        let first_line = doc.display_line(first_row).expect("a line is first in view");
        let within = before - first_row as f64 * line_height;

        state.folded.set(false);
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        assert_eq!(doc.len(), 228, "every line, no divider");
        let row = doc.row_of_display(first_line).expect("every line is shown");
        assert_eq!(row, first_line);
        assert_eq!(viewport.get().y0, row as f64 * line_height + within, "the same line is first in view");
        assert_eq!(laid_out_text(&live), doc.text);

        state.folded.set(true);
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        assert_eq!(doc.len(), 99);
        assert_eq!(doc.display_line(first_row), Some(first_line));
        assert_eq!(viewport.get().y0, before, "folded again, the line is where it was");
    }

    #[test]
    fn all_takes_the_divider_with_it_and_reset_is_on_the_divider_above() {
        let (mut harness, state) = three_runs();
        let live = stacked(state);
        let line_height = crate::theme::CODE_LINE_HEIGHT as f64;
        let doc = live.doc.get();
        let second = doc.run_of(14).expect("row 14 is the second divider");
        assert_eq!(second.len(), 44);

        // All on the second run: its lines take its divider's row, and the
        // document has two dividers left.
        let (first, last) = second.whole();
        state.reveal("src/x.rs", "", first, last);
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        assert_eq!(doc.len(), 99 - 1 + 44);
        assert_eq!(doc.folds.len(), 2);
        assert!(doc.run_of(14).is_none(), "the divider is gone");
        assert!(doc.run_of(5).is_some(), "the one above is not");

        // Hovered, the divider above offers Reset as a third slot after Up
        // and Down: 52 wide centred in the gutter's 70, so Reset's centre
        // is 53 from the gutter's left.
        let (left, top) = pane_origin();
        let (x, y) = (left + 53.0, top + 5.0 * line_height + line_height / 2.0);
        harness.pointer_move(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.hovered_line.get(), Some(5));
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(live.doc.get().len(), 99, "click at ({x}, {y}) over {under:?} did not fold the run again");
        assert!(state.reveals_for("src/x.rs", "").is_empty());
        assert_eq!(laid_out_text(&live), live.doc.get().text);
    }

    #[test]
    fn a_reveal_above_the_cursor_moves_it_without_asking_about_its_symbol_again() {
        let (mut harness, state) = one_long_run();
        let live = stacked(state);
        let cursor = super::STACKED_CURSOR.with(|c| c.get()).expect("a stacked editor was built");
        // The editor's cursor starts on row 0, a ghost, which is nowhere
        // in the head file: nothing was asked.
        assert_eq!(state.last_position.get(), None);
        assert_eq!(state.request_token.get(), 0);

        // The cursor placed as a click would place it: on "new bottom",
        // head line 61, three columns in, below the divider at row 5.
        let doc = live.doc.get();
        let row = doc.row_of_head(61).expect("new bottom is shown");
        assert_eq!(row, 11);
        let offset = doc.offset_of_row(row) + 3;
        cursor.update(|c| c.set_offset(offset, CursorAffinity::Backward, false, false));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.last_position.get(), Some((61, 3)));
        assert_eq!(state.request_token.get(), 1, "the symbol under the cursor was asked about");
        assert_eq!(state.cursor_line.get(), 64, "new bottom is display line 64, after the pair's two ghosts");
        state.hover_text.set(Some("fn new_bottom()".into()));

        // Down on the divider above: twenty lines come in above the cursor,
        // which stays on its text, twenty rows down. The symbol under it is
        // the one it was on, so nothing is asked again and the panel stays.
        let run = doc.run_of(5).expect("row 5 is the divider");
        let (first, last) = run.bottom();
        state.reveal("src/x.rs", "", first, last);
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        assert_eq!(doc.len(), 103);
        assert_eq!(doc.row_of_head(61), Some(31));
        assert_eq!(cursor.get().offset(), doc.offset_of_row(31) + 3);
        assert_eq!(state.cursor_line.get(), 64);
        assert_eq!(state.last_position.get(), Some((61, 3)));
        assert_eq!(state.request_token.get(), 1, "nothing was asked again");
        assert_eq!(state.hover_text.get().as_deref(), Some("fn new_bottom()"), "the panel stays as it was");

        // Reset folds them away again, moving the cursor back up: the same.
        state.reset("src/x.rs", "", &[run.default_heads]);
        harness.rebuild();
        harness.paint();
        assert_eq!(cursor.get().offset(), offset);
        assert_eq!(state.request_token.get(), 1);
        assert_eq!(state.hover_text.get().as_deref(), Some("fn new_bottom()"));

        // A click somewhere else is still a click: the panel closes and
        // the new symbol is asked about.
        let elsewhere = live.doc.get().offset_of_row(12);
        cursor.update(|c| c.set_offset(elsewhere, CursorAffinity::Backward, false, false));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.hover_text.get(), None);
        assert_eq!(state.request_token.get(), 2);
        assert_eq!(state.last_position.get(), Some((62, 0)));
    }

    #[test]
    fn the_split_keeps_both_sides_the_same_height_through_a_down_click() {
        let (mut harness, state) = three_runs();
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let (left, right) = sides(state);
        let line_height = crate::theme::CODE_LINE_HEIGHT as f64;

        // The split pairs each rewritten line with its old one, so the
        // sides are the 213 aligned rows less the three runs, plus their
        // dividers, at the same rows on both.
        let (left_doc, right_doc) = (left.doc.get(), right.doc.get());
        assert_eq!(right_doc.len(), 213 - 3 * 44 + 3);
        assert_eq!(left_doc.len(), right_doc.len());
        assert_eq!(right_doc.hidden_lines(), 3 * 44);
        for row in [4, 12, 20] {
            assert_eq!(right_doc.run_of(row).map(|r| r.len()), left_doc.run_of(row).map(|r| r.len()), "row {row}");
            assert!(right_doc.run_of(row).is_some(), "row {row} is a divider");
        }
        assert_eq!(laid_out_text(&left), left_doc.text);
        assert_eq!(laid_out_text(&right), right_doc.text);
        let (left_viewport, right_viewport) = super::SPLIT_VIEWPORTS.with(|v| v.get()).expect("a split was built");
        let y0 = right_viewport.get().y0;

        // The head side's gutter is at the right pane's left edge, past
        // the base side and the rule between. Its divider row holds the
        // controls: Down's centre is 44 from the gutter's left.
        let (pane_left, top) = pane_origin();
        let right_gutter = pane_left + (1400.0 - pane_left - 1.0) / 2.0 + 1.0;
        let (x, y) = (right_gutter + 44.0, top + 4.0 * line_height + line_height / 2.0);
        harness.pointer_move(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.hovered_line.get(), Some(4), "the head side's divider row records the pointer");
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();

        let (left_doc, right_doc) = (left.doc.get(), right.doc.get());
        assert_eq!(right_doc.len(), 84 + 20, "click at ({x}, {y}) over {under:?} did not reveal twenty lines");
        assert_eq!(left_doc.len(), right_doc.len(), "the base side grew with the head side");
        assert_eq!(right_doc.run_of(4).map(|r| r.len()), Some(24), "the divider stays, hiding twenty fewer");
        assert_eq!(left_doc.run_of(4).map(|r| r.len()), Some(24));
        assert_eq!(right_doc.row_of_head(28), Some(5), "the first revealed head line follows the divider");
        assert_eq!(left_doc.display_line(5), Some(28), "beside its own base line");
        assert_eq!(laid_out_text(&left), left_doc.text, "the left editor's text is its document's");
        assert_eq!(laid_out_text(&right), right_doc.text, "the right editor's text is its document's");
        assert_eq!(right_viewport.get().y0, y0, "the viewport did not move");
        assert_eq!(left_viewport.get().y0, right_viewport.get().y0, "the left is where the right is");

        // The base side's gutter has no controls: the same click there
        // reveals nothing.
        let (x, y) = (pane_left + 44.0, y);
        harness.pointer_move(x, y);
        harness.rebuild();
        harness.paint();
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(right.doc.get().len(), 104, "the base side's divider row is the tint alone");
        assert_eq!(left.doc.get().len(), 104);

        // The bar's label is over the head side's editor, and records the
        // pointer as the gutter row does; over the base side's editor the
        // bar lets the pointer through, and nothing records it.
        harness.pointer_move(right_gutter + crate::theme::GUTTER_WIDTH + 100.0, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.hovered_line.get(), Some(4), "the label over the head side records the row");
        harness.pointer_move(pane_left + crate::theme::GUTTER_WIDTH + 100.0, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.hovered_line.get(), None, "the bar over the base side is nothing");

        // A jump into the second run reveals around the target on both
        // sides and scrolls both to it.
        assert_eq!(right.doc.get().row_of_head(80), None, "head line 80 is folded");
        state.jump_to_line.set(Some(80));
        harness.rebuild();
        harness.paint();
        assert!(state.jump_to_line.get().is_none(), "the split's right editor took the jump");
        let (left_doc, right_doc) = (left.doc.get(), right.doc.get());
        assert_eq!(right_doc.len(), 104 + 7 + 1, "seven lines and a second divider for the run's other piece");
        assert_eq!(left_doc.len(), right_doc.len());
        let row = right_doc.row_of_head(80).expect("the target is shown");
        assert_eq!(left_doc.display_line(row), Some(80));
        assert_eq!(laid_out_text(&left), left_doc.text);
        assert_eq!(laid_out_text(&right), right_doc.text);
        assert_eq!(right_viewport.get().y0, (row as f64 - 4.0) * line_height, "scrolled to the target");
        assert_eq!(left_viewport.get().y0, right_viewport.get().y0);
    }

    #[test]
    fn a_jump_into_a_fold_reveals_around_the_target_before_scrolling() {
        let (mut harness, state) = one_long_run();
        let live = stacked(state);
        let line_height = crate::theme::CODE_LINE_HEIGHT as f64;
        assert_eq!(live.doc.get().row_of_head(30), None, "head line 30 is folded");
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).unwrap();

        state.jump_to_line.set(Some(30));
        harness.rebuild();
        harness.paint();
        assert!(state.jump_to_line.get().is_none(), "the editor took the jump");
        let doc = live.doc.get();
        // Three lines each side: head lines 27 to 33 are shown between
        // two dividers, and the run is two runs.
        assert!(state.reveals_for("src/x.rs", "").overlaps(27, 33));
        assert_eq!(doc.folds.len(), 2);
        for head in 27..=33 {
            assert!(doc.row_of_head(head).is_some(), "head line {head} is in view");
        }
        assert_eq!(doc.row_of_head(26), None);
        assert_eq!(doc.row_of_head(34), None);
        let row = doc.row_of_head(30).unwrap();
        assert_eq!(row, 9);
        assert_eq!(laid_out_text(&live), doc.text);
        assert_eq!(viewport.get().y0, (row as f64 - 4.0) * line_height, "scrolled to the target");

        // Reset on the divider above folds it again.
        state.reset("src/x.rs", "", &[doc.run_of(5).unwrap().default_heads]);
        harness.rebuild();
        harness.paint();
        assert_eq!(live.doc.get().len(), 83);
        assert_eq!(laid_out_text(&live), live.doc.get().text);
    }

    // Cards.

    /// A question and its answer on head lines `start` to `line` of the
    /// open file, one-based, both finished.
    fn thread_on(id: &str, start: u32, line: u32, outdated: bool) -> perch_github::CommentThread {
        use perch_github::{Anchor, Author, CommentMessage, CommentThread, RunRecord, RunStatus, RunStep, Side, ThreadOrigin};
        let created_at = "2026-09-18T09:59:00Z".parse().unwrap();
        let run = RunRecord {
            id: format!("{id}-run"),
            thread_id: id.into(),
            backend: "claude-code".into(),
            model: "sonnet".into(),
            effort: "medium".into(),
            session_id: Some("sess".into()),
            status: RunStatus::Finished,
            cost_usd: Some(0.03),
            duration_ms: Some(14_000),
            steps: vec![RunStep { summary: "Read src/x.rs".into(), ok: true }],
            error: None,
            started_at: "2026-09-18T10:00:00Z".parse().unwrap(),
            ended_at: Some("2026-09-18T10:00:14Z".parse().unwrap()),
        };
        let message = |suffix: &str, author: Author, body: &str, run: Option<RunRecord>| CommentMessage {
            id: format!("{id}-{suffix}"),
            thread_id: id.into(),
            origin: ThreadOrigin::Local,
            author,
            body: body.into(),
            created_at,
            run,
        };
        CommentThread {
            id: id.into(),
            worktree_id: "wt".into(),
            origin: ThreadOrigin::Local,
            anchor: Some(Anchor { path: "src/x.rs".into(), start_line: start, line, side: Side::Right, commit_sha: "h".into() }),
            outdated,
            resolved: false,
            created_at,
            messages: vec![
                message("m1", Author::User, "why?", None),
                message(
                    "m2",
                    Author::Agent { backend: "claude-code".into(), model: "sonnet".into() },
                    "because\n\n```rust\nfn x() {}\n```\n\n- one\n- two",
                    Some(run),
                ),
            ],
        }
    }

    /// A thread of the given message bodies, the reader's and the agent's
    /// in turn with no runs, on head lines `start` to `line` of the open
    /// file, one-based.
    fn thread_with(id: &str, start: u32, line: u32, bodies: &[&str]) -> perch_github::CommentThread {
        use perch_github::{Anchor, Author, CommentMessage, CommentThread, Side, ThreadOrigin};
        let created_at = "2026-09-18T09:59:00Z".parse().unwrap();
        CommentThread {
            id: id.into(),
            worktree_id: "wt".into(),
            origin: ThreadOrigin::Local,
            anchor: Some(Anchor { path: "src/x.rs".into(), start_line: start, line, side: Side::Right, commit_sha: "h".into() }),
            outdated: false,
            resolved: false,
            created_at,
            messages: bodies
                .iter()
                .enumerate()
                .map(|(i, body)| CommentMessage {
                    id: format!("{id}-m{i}"),
                    thread_id: id.into(),
                    origin: ThreadOrigin::Local,
                    author: if i % 2 == 0 {
                        Author::User
                    } else {
                        Author::Agent { backend: "claude-code".into(), model: "sonnet".into() }
                    },
                    body: body.to_string(),
                    created_at,
                    run: None,
                })
                .collect(),
        }
    }

    /// The first view whose debug name contains `text`.
    fn find(id: floem::ViewId, text: &str) -> Option<floem::ViewId> {
        if id.debug_name().contains(text) {
            return Some(id);
        }
        id.children().into_iter().find_map(|c| find(c, text))
    }

    /// The card layer's card for a thread: its box, relative to the layer,
    /// which sits over the editor at the code pane's origin past the
    /// gutter.
    fn card_rect(harness: &HeadlessHarness, thread_id: &str) -> floem::kurbo::Rect {
        let id = find(harness.root_id(), &format!("inline-card:{thread_id}")).expect("the card is in the layer");
        harness.get_layout_rect(id)
    }

    /// The rows a thread's card takes as it has been laid out: what the
    /// correction gives it from its box, which the document is checked
    /// against.
    fn laid_out_rows(harness: &HeadlessHarness, thread_id: &str) -> usize {
        let rect = card_rect(harness, thread_id);
        super::rows_for_height(rect.height(), crate::theme::CODE_LINE_HEIGHT as f64, crate::theme::CARD_INSET)
    }

    #[test]
    fn a_thread_in_the_open_file_reserves_rows_under_its_lines_and_its_card_sits_on_them() {
        use crate::review::doc::{CardId, DocRow};
        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = crate::theme::CODE_LINE_HEIGHT as f64;
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).expect("a stacked editor was built");
        assert_eq!(live.doc.get().len(), 120, "the buffer, with a ghost under every rewritten line");
        assert!(find(harness.root_id(), "inline-card:").is_none(), "no thread, no card");

        // A thread on head lines 5 to 7 arrives, as the backend sends it.
        // Line 7 is an `fn a`, display line 8; the ghost at 9 is the old
        // side of the rewritten line 8 after it, not something removed
        // under line 7, so the card goes between the two.
        let thread = thread_on("t1", 5, 7, false);
        let card = CardId::Thread("t1".into());
        let estimate = crate::review::doc::estimate_rows(&thread, false);
        state.set_comment_threads(Arc::new(vec![thread]));
        assert_eq!(live.doc.get().card_span(&card), Some(9..9 + estimate), "the estimate, until the card is laid out");
        harness.rebuild();
        harness.paint();
        // Laid out, the card has the rows its height takes.
        let rows = laid_out_rows(&harness, "t1");
        let doc = live.doc.get();
        assert_eq!(doc.len(), 120 + rows, "the card's rows were spliced in place");
        assert_eq!(doc.display_line(8), Some(8));
        assert_eq!(state.active_file().unwrap().buffer.head_line(8), Some(6));
        assert_eq!(state.active_file().unwrap().buffer.head_line(9), None, "display line 9 is the ghost");
        assert_eq!(doc.card_span(&card), Some(9..9 + rows));
        for row in 9..9 + rows {
            assert_eq!(doc.rows[row], DocRow::Spacer { card: card.clone(), row: row - 9 });
        }
        assert_eq!(doc.display_line(9 + rows), Some(9), "the code goes on under the card");
        assert_eq!(laid_out_text(&live), doc.text);
        assert_eq!(viewport.get().y0, 0.0);

        // The card is drawn over its rows: its top is its first spacer
        // row's, set in by the inset, and its left edge the first code
        // column's.
        let rect = card_rect(&harness, "t1");
        assert_eq!(rect.y0, 9.0 * line_height + crate::theme::CARD_INSET);
        assert_eq!(rect.x0, crate::theme::CODE_GUTTER_GAP);
        assert!(rect.height() > line_height, "an open card is more than one row");
        assert!(!state.is_collapsed(&card), "a thread that is not outdated starts open");

        // Collapsed, the card is one row, and the document is shorter by
        // the rows the card gave back.
        state.toggle_card(&card);
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        assert_eq!(doc.len(), 120 + 1, "shrunk by the difference");
        assert_eq!(doc.card_span(&card), Some(9..10));
        assert_eq!(doc.display_line(10), Some(9));
        assert_eq!(laid_out_text(&live), doc.text);
        let rect = card_rect(&harness, "t1");
        assert_eq!(rect.y0, 9.0 * line_height, "a collapsed card takes its row whole");
        assert_eq!(rect.height(), line_height);

        // The chevron at the row's left opens it again: past the gutter,
        // the margin, the border and the row's padding, in its slot.
        let (left, top) = pane_origin();
        let x = left + crate::theme::GUTTER_WIDTH + rect.x0 + 1.0 + crate::theme::COLLAPSED_CARD_PADDING + crate::theme::CHEVRON_SLOT / 2.0;
        let y = top + rect.y0 + line_height / 2.0;
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert!(!state.is_collapsed(&card), "click at ({x}, {y}) over {under:?} did not open the card");
        assert_eq!(live.doc.get().len(), 120 + rows, "open again at the rows the layout gave it, not the estimate");
        assert_eq!(laid_out_text(&live), live.doc.get().text);

        // The thread going takes its rows with it.
        state.set_comment_threads(Arc::new(Vec::new()));
        harness.rebuild();
        harness.paint();
        assert_eq!(live.doc.get().len(), 120);
        assert!(find(harness.root_id(), "inline-card:").is_none());
    }

    #[test]
    fn a_marker_click_focuses_the_card_and_scrolls_it_into_view_without_opening_the_column() {
        use crate::review::doc::CardId;
        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = crate::theme::CODE_LINE_HEIGHT as f64;
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).unwrap();
        state.set_comment_threads(Arc::new(vec![thread_on("t1", 5, 7, false)]));
        harness.rebuild();
        harness.paint();
        let card = CardId::Thread("t1".into());
        // The column opens itself for a file with threads, as it did
        // before the cards; shut, it stays shut through the click.
        assert!(state.threads_open.get());
        state.threads_open.set(false);
        harness.rebuild();
        harness.paint();

        // The marker sits in the gutter's slot on the anchor's first line,
        // head line 5, display line 6, on row 6 of a document with nothing
        // folded above it.
        let (left, top) = pane_origin();
        assert_eq!(live.doc.get().row_of_head(4), Some(6));
        let (x, y) = (left + 3.0 + 8.0, top + 6.0 * line_height + line_height / 2.0);
        // The slot is rebuilt when the pointer arrives on its row, so the
        // pointer settles there before the click, as it would in a hand.
        harness.pointer_move(x, y);
        harness.rebuild();
        harness.paint();
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.focus_card.get(), Some(card.clone()), "click at ({x}, {y}) over {under:?} did not focus the card");
        assert!(!state.threads_open.get(), "the column is not opened for it");
        assert_eq!(viewport.get().y0, 0.0, "the card is in view, so nothing scrolled");

        // Scrolled past the card, a click on its marker brings it back
        // into view with the line above it.
        harness.scroll_down(left + 300.0, top + 300.0, 40.0 * line_height);
        harness.rebuild();
        harness.paint();
        assert!(viewport.get().y0 > 20.0 * line_height, "the editor did not scroll past the card");
        state.focus_card.set(Some(card));
        harness.rebuild();
        harness.paint();
        assert_eq!(viewport.get().y0, 8.0 * line_height, "the anchor's last line, above the card, is the first row in view");
    }

    #[test]
    fn a_split_reserves_the_cards_rows_on_both_sides_and_the_card_spans_them() {
        use crate::review::doc::CardId;
        let (mut harness, state) = harness();
        let line_height = crate::theme::CODE_LINE_HEIGHT as f64;
        let thread = thread_on("t1", 5, 7, false);
        state.set_comment_threads(Arc::new(vec![thread]));
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        // The column opened itself for the thread; shut, the pane is the
        // rest of the window.
        state.threads_open.set(false);
        harness.rebuild();
        harness.paint();
        let (left, right) = sides(state);
        let card = CardId::Thread("t1".into());
        // The card over the outer stack corrects both sides' rows.
        let rows = laid_out_rows(&harness, "t1");
        let (left_doc, right_doc) = (left.doc.get(), right.doc.get());
        assert_eq!(left_doc.len(), right_doc.len());
        assert_eq!(right_doc.len(), 90 + rows);
        // The split pairs each ghost with its line, so display line 9,
        // the ghost, is on the next line's row: head line 7 is row 6, and
        // the card follows it on both sides, before that row, as it is
        // before the ghost in the stacked layout.
        assert_eq!(right_doc.row_of_head(6), Some(6));
        assert_eq!(right_doc.card_span(&card), Some(7..7 + rows));
        assert_eq!(left_doc.card_span(&card), right_doc.card_span(&card));
        assert_eq!(laid_out_text(&left), left_doc.text);
        assert_eq!(laid_out_text(&right), right_doc.text);

        // The card is over the outer stack, from the base side's first
        // code column, past its gutter.
        let rect = card_rect(&harness, "t1");
        assert_eq!(rect.y0, 7.0 * line_height + crate::theme::CARD_INSET);
        assert_eq!(rect.x0, crate::theme::GUTTER_WIDTH + crate::theme::CODE_GUTTER_GAP);
        let (pane_left, _) = pane_origin();
        assert_eq!(rect.x1, 1400.0 - pane_left - crate::theme::CODE_GUTTER_GAP, "to the right edge, less the margin");

        // Collapsing it shrinks both sides by the difference.
        state.toggle_card(&card);
        harness.rebuild();
        harness.paint();
        assert_eq!(right.doc.get().len(), 90 + 1);
        assert_eq!(left.doc.get().len(), 90 + 1);
        assert_eq!(laid_out_text(&left), left.doc.get().text);
        assert_eq!(laid_out_text(&right), right.doc.get().text);
    }

    // The height correction.

    /// A question alone is estimated at five rows: its one line, the
    /// byline and the gap, the first row and the follow-up line. On the
    /// screen the first row, the padding and the follow-up line are more
    /// than the two rows the estimate charges, so the laid-out card is
    /// always taller than five rows; here, with the fonts on this
    /// machine, it is seven. The assertions are on the rows the layout
    /// gave, so they hold wherever the fonts put it.
    #[test]
    fn a_card_laid_out_taller_than_its_rows_moves_the_lines_below_it_and_nothing_else() {
        use crate::review::doc::CardId;
        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = crate::theme::CODE_LINE_HEIGHT as f64;
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).expect("a stacked editor was built");
        let card = CardId::Thread("t1".into());

        // The thread arrives, and the document is built from the estimate
        // before anything is laid out: the card is under row 8, and the
        // line under the card is five rows on.
        let thread = thread_with("t1", 5, 7, &["why?"]);
        let estimate = crate::review::doc::estimate_rows(&thread, false);
        assert_eq!(estimate, 5);
        state.set_comment_threads(Arc::new(vec![thread]));
        let before = live.doc.get();
        assert_eq!(before.len(), 120 + estimate);
        assert_eq!(before.card_span(&card), Some(9..9 + estimate));
        assert_eq!(before.row_of_display(9), Some(9 + estimate));
        assert!(state.card_rows.get().is_empty(), "nothing has been laid out");

        // Laid out, the card says how tall it is, and the document has
        // the rows that takes: the lines above the card are where they
        // were, the lines below it are further down by the difference,
        // and the view, which the card is inside, did not move.
        harness.rebuild();
        harness.paint();
        let rows = laid_out_rows(&harness, "t1");
        assert!(rows > estimate, "the chrome is more than two rows: {rows} rows for {}px", card_rect(&harness, "t1").height());
        let grew = rows - estimate;
        let after = live.doc.get();
        assert_eq!(state.card_rows.get().get(&card), Some(&rows), "the layout's answer is kept");
        assert_eq!(after.len(), 120 + rows);
        assert_eq!(after.card_span(&card), Some(9..9 + rows));
        assert_eq!(after.rows[..9], before.rows[..9], "the lines above the card are where they were");
        assert_eq!(after.row_of_display(9), Some(9 + estimate + grew), "the line under the card is {grew} rows further down");
        assert_eq!(after.row_of_display(119), Some(119 + estimate + grew));
        assert_eq!(laid_out_text(&live), after.text);
        assert_eq!(viewport.get().y0, 0.0, "the card is in view, so the view did not move");
        let rect = card_rect(&harness, "t1");
        assert_eq!(rect.y0, 9.0 * line_height + crate::theme::CARD_INSET, "the card is where it was");

        // Every scroll lays the card out again at the same height, and
        // it says nothing new: the document is the one it was.
        let (left, top) = pane_origin();
        harness.scroll_down(left + 300.0, top + 300.0, 3.0 * line_height);
        harness.rebuild();
        harness.paint();
        assert_eq!(viewport.get().y0, 3.0 * line_height, "the editor scrolled");
        assert!(Arc::ptr_eq(&live.doc.get(), &after), "the document was not built again");
        assert_eq!(state.card_rows.get().get(&card), Some(&rows));
    }

    #[test]
    fn a_card_above_the_viewport_growing_keeps_the_same_first_visible_line() {
        use crate::review::doc::CardId;
        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = crate::theme::CODE_LINE_HEIGHT as f64;
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).expect("a stacked editor was built");
        let card = CardId::Thread("t1".into());

        // The question arrives and is laid out; then the editor is
        // scrolled well past its card.
        state.set_comment_threads(Arc::new(vec![thread_with("t1", 5, 7, &["why?"])]));
        harness.rebuild();
        harness.paint();
        let rows = live.doc.get().card_span(&card).expect("the card has rows").len();
        assert_eq!(rows, laid_out_rows(&harness, "t1"));
        let (left, top) = pane_origin();
        harness.scroll_down(left + 300.0, top + 300.0, 40.0 * line_height);
        harness.rebuild();
        harness.paint();
        let before = viewport.get().y0;
        let first_row = (before / line_height).floor() as usize;
        assert!(first_row > 9 + rows, "the editor did not scroll past the card: row {first_row} is first in view");
        let first_line = live.doc.get().display_line(first_row).expect("a line is first in view");

        // The answer lands: the card is built again for the thread's two
        // messages and is a line and a gap taller, which is more than a
        // row. The rows it gains are above the view, so the scroll moves
        // by them and the same line is first in view.
        state.set_comment_threads(Arc::new(vec![thread_with("t1", 5, 7, &["why?", "because"])]));
        harness.rebuild();
        harness.paint();
        let grown = laid_out_rows(&harness, "t1");
        assert!(grown > rows, "the answer did not grow the card: {rows} rows to {grown}");
        let grew = grown - rows;
        let doc = live.doc.get();
        assert_eq!(doc.card_span(&card), Some(9..9 + grown));
        assert_eq!(doc.len(), 120 + grown);
        assert_eq!(viewport.get().y0, before + grew as f64 * line_height, "the scroll moved by the rows added above it");
        assert_eq!(doc.display_line(first_row + grew), Some(first_line), "the same line is first in view");
        assert_eq!(laid_out_text(&live), doc.text);
    }

    #[test]
    fn a_card_is_never_taller_than_the_cap_and_the_estimate_agrees_with_it_there() {
        use crate::review::doc::{CARD_MAX_ROWS, CardId};
        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = crate::theme::CODE_LINE_HEIGHT as f64;
        let card = CardId::Thread("t1".into());

        // A hundred-line answer is far more than forty rows of card, so
        // the estimate is the cap, and so is the card: its messages
        // scroll inside it, and its box stops where the rows do.
        let answer = (0..100).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let thread = thread_with("t1", 5, 7, &["why?", &answer]);
        assert_eq!(crate::review::doc::estimate_rows(&thread, false), CARD_MAX_ROWS);
        state.set_comment_threads(Arc::new(vec![thread]));
        harness.rebuild();
        harness.paint();
        let rect = card_rect(&harness, "t1");
        assert_eq!(rect.height(), CARD_MAX_ROWS as f64 * line_height - 2.0 * crate::theme::CARD_INSET);
        assert_eq!(laid_out_rows(&harness, "t1"), CARD_MAX_ROWS);
        let doc = live.doc.get();
        assert_eq!(doc.card_span(&card), Some(9..9 + CARD_MAX_ROWS));
        assert_eq!(doc.len(), 120 + CARD_MAX_ROWS);
        assert!(!state.card_rows.get().contains_key(&card), "the estimate was right, so nothing was written");
        assert_eq!(laid_out_text(&live), doc.text);
    }

    #[test]
    fn a_follow_up_composer_grows_the_card_and_closing_it_gives_the_rows_back() {
        use crate::review::doc::CardId;
        let (mut harness, state) = harness();
        let live = stacked(state);
        let card = CardId::Thread("t1".into());
        let thread = thread_with("t1", 5, 7, &["why?", "because"]);
        state.set_comment_threads(Arc::new(vec![thread.clone()]));
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        let rows = doc.card_span(&card).expect("the card has rows").len();
        assert_eq!(rows, laid_out_rows(&harness, "t1"));
        let len = doc.len();

        // Follow up: the composer's box takes the follow-up line's place
        // inside the card, which grows around it, and the document with
        // it. The box is never focused here; see `composer_autofocus`.
        let choice = (perch_agent::Backend::ClaudeCode, "sonnet".to_string(), perch_agent::Effort::Medium);
        state.open_composer(Some("t1".into()), thread.anchor.clone(), String::new(), choice);
        harness.rebuild();
        harness.paint();
        let grown = laid_out_rows(&harness, "t1");
        assert!(grown > rows, "the box is taller than the line it replaced: {rows} rows to {grown}");
        let doc = live.doc.get();
        assert_eq!(doc.card_span(&card), Some(9..9 + grown));
        assert_eq!(doc.len(), len + (grown - rows));
        assert_eq!(laid_out_text(&live), doc.text);

        // Put away, the line is back, and the rows with it: the document
        // is as long as it was.
        state.close_composer();
        harness.rebuild();
        harness.paint();
        assert_eq!(laid_out_rows(&harness, "t1"), rows);
        let doc = live.doc.get();
        assert_eq!(doc.card_span(&card), Some(9..9 + rows));
        assert_eq!(doc.len(), len);
        assert_eq!(laid_out_text(&live), doc.text);
        assert_eq!(state.card_rows.get().get(&card), Some(&rows));
    }

    // GitHub's threads and the composer as cards.

    /// A review comment from GitHub on head line `line` of the open file,
    /// one-based, on the head side.
    fn github_comment(id: u64, line: u64, reply_to: Option<u64>, body: &str) -> perch_github::ReviewComment {
        perch_github::ReviewComment {
            id,
            pull_request_review_id: None,
            in_reply_to_id: reply_to,
            user: "octocat".into(),
            body: body.into(),
            path: "src/x.rs".into(),
            line: Some(line),
            original_line: None,
            diff_hunk: String::new(),
            side: perch_github::Side::Right,
            created_at: "2026-09-18T09:59:00Z".parse().unwrap(),
            updated_at: None,
            html_url: String::new(),
        }
    }

    /// A discussion holding the given review comments and nothing else.
    fn discussion_of(review_comments: Vec<perch_github::ReviewComment>) -> perch_github::Discussion {
        perch_github::Discussion {
            opened_at: None,
            body: String::new(),
            comments: Vec::new(),
            reviews: Vec::new(),
            review_comments,
            events: Vec::new(),
        }
    }

    #[test]
    fn a_github_thread_in_the_discussion_is_a_card_under_its_line_that_the_hollow_marker_focuses() {
        use crate::review::doc::{CardId, DocRow};
        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = crate::theme::CODE_LINE_HEIGHT as f64;
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).expect("a stacked editor was built");
        assert_eq!(live.doc.get().len(), 120);

        // A comment and a reply on head line 7, display line 8, arrive
        // with the discussion: the card goes under the line, before the
        // ghost of the rewritten line after it, as the reader's does.
        let comments = vec![github_comment(100, 7, None, "nit: name this"), github_comment(101, 7, Some(100), "done")];
        let card = CardId::GitHub(100);
        let thread = crate::review::model::github_threads_of(Some(&discussion_of(comments.clone())), "src/x.rs").remove(0);
        let estimate = crate::review::doc::estimate_github_rows(&thread, false);
        state.discussion.set(Some(Arc::new(discussion_of(comments))));
        assert_eq!(live.doc.get().card_span(&card), Some(9..9 + estimate), "the estimate, until the card is laid out");
        assert!(!state.is_collapsed(&card), "nothing says GitHub has it resolved, so it starts open");
        harness.rebuild();
        harness.paint();
        let rows = laid_out_rows(&harness, "github:100");
        let doc = live.doc.get();
        assert_eq!(doc.len(), 120 + rows);
        assert_eq!(doc.card_span(&card), Some(9..9 + rows));
        for row in 9..9 + rows {
            assert_eq!(doc.rows[row], DocRow::Spacer { card: card.clone(), row: row - 9 });
        }
        assert_eq!(doc.display_line(9 + rows), Some(9), "the code goes on under the card");
        assert_eq!(laid_out_text(&live), doc.text);
        let rect = card_rect(&harness, "github:100");
        assert_eq!(rect.y0, 9.0 * line_height + crate::theme::CARD_INSET);
        assert_eq!(rect.x0, crate::theme::CODE_GUTTER_GAP);
        assert!(rect.height() > line_height);
        assert!(!state.threads_open.get(), "GitHub's threads do not open the column");

        // The hollow marker on the comment's line focuses the card and
        // leaves the column shut.
        let (left, top) = pane_origin();
        let (x, y) = (left + 3.0 + 8.0, top + 8.0 * line_height + line_height / 2.0);
        harness.pointer_move(x, y);
        harness.rebuild();
        harness.paint();
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.focus_card.get(), Some(card.clone()), "click at ({x}, {y}) over {under:?} did not focus the card");
        assert!(!state.threads_open.get());
        assert_eq!(viewport.get().y0, 0.0, "the card is in view, so nothing scrolled");

        // Collapsed, it is one row: the chevron, who commented, the first
        // line and how many replies follow.
        state.toggle_card(&card);
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        assert_eq!(doc.len(), 120 + 1);
        assert_eq!(doc.card_span(&card), Some(9..10));
        assert_eq!(laid_out_text(&live), doc.text);
        let rect = card_rect(&harness, "github:100");
        assert_eq!(rect.y0, 9.0 * line_height);
        assert_eq!(rect.height(), line_height);
        state.toggle_card(&card);
        harness.rebuild();
        harness.paint();
        assert_eq!(live.doc.get().len(), 120 + rows, "open again at the rows the layout gave it");

        // A thread of the reader's own on the same line goes before it.
        state.set_comment_threads(Arc::new(vec![thread_with("t1", 5, 7, &["why?"])]));
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        let mine = doc.card_span(&CardId::Thread("t1".into())).expect("the reader's card has rows");
        assert_eq!(mine.start, 9);
        assert_eq!(doc.card_span(&card), Some(mine.end..mine.end + rows));
        assert_eq!(laid_out_text(&live), doc.text);

        // The discussion going takes the card and its rows with it.
        state.discussion.set(None);
        harness.rebuild();
        harness.paint();
        assert_eq!(live.doc.get().card_span(&card), None);
        assert!(find(harness.root_id(), "inline-card:github").is_none());
    }

    /// The column's slot for a new thread's composer: what it holds, by
    /// the name of the view in it, `Empty` when it holds nothing.
    fn column_slot(harness: &HeadlessHarness) -> String {
        let slot = find(harness.root_id(), "column-composer").expect("the column is open");
        slot.children().first().map(|c| c.debug_name()).unwrap_or_default()
    }

    #[test]
    fn the_composer_opened_on_lines_is_a_card_under_them_and_escape_takes_its_rows_away() {
        use crate::review::doc::{CardId, COMPOSER_ROWS, DocRow};
        use floem::prelude::{Code, Key, NamedKey};
        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = crate::theme::CODE_LINE_HEIGHT as f64;
        let cursor = super::STACKED_CURSOR.with(|c| c.get()).expect("a stacked editor was built");
        let card = CardId::Composer;

        // The cursor placed as a click would place it: on head line 7,
        // display line 8, row 8. The selection is that line.
        let doc = live.doc.get();
        let row = doc.row_of_head(6).expect("head line 7 is shown");
        assert_eq!(row, 8);
        cursor.update(|c| c.set_offset(doc.offset_of_row(row) + 2, CursorAffinity::Backward, false, false));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.selection.get(), Some((8, 8)));

        // A bare key goes to whatever has the keyboard, and the window's
        // handler catches it on the way down; a click on the gutter's
        // numbers gives it the keyboard, as a click in the code cannot
        // headlessly.
        let (left, top) = pane_origin();
        harness.click(left + 40.0, top + 8.0 * line_height + line_height / 2.0);
        harness.rebuild();
        harness.paint();

        // `a` opens the composer on the line. It is a card under the
        // line, at its estimate until it is laid out, and the column is
        // not opened for it.
        harness.dispatch_event(key_press(Code::KeyA, Key::Character("a".into())));
        let composer = state.composer.get().expect("`a` opened the composer");
        assert_eq!(composer.anchor.as_ref().map(|a| (a.start_line, a.line)), Some((7, 7)));
        assert!(state.composer_is_inline());
        assert!(!state.threads_open.get(), "the column is not opened for a composer drawn in the code");
        harness.rebuild();
        harness.paint();
        let rows = laid_out_rows(&harness, "composer");
        assert_eq!(rows, COMPOSER_ROWS, "an empty composer is laid out at its estimate, so nothing jumps");
        assert!(state.card_rows.get().is_empty(), "the estimate was right, so nothing was written");
        let doc = live.doc.get();
        assert_eq!(doc.len(), 120 + rows);
        assert_eq!(doc.card_span(&card), Some(9..9 + rows));
        for row in 9..9 + rows {
            assert_eq!(doc.rows[row], DocRow::Spacer { card: CardId::Composer, row: row - 9 });
        }
        assert_eq!(doc.display_line(9 + rows), Some(9), "the code goes on under the card");
        assert_eq!(laid_out_text(&live), doc.text);
        let rect = card_rect(&harness, "composer");
        assert_eq!(rect.y0, 9.0 * line_height + crate::theme::CARD_INSET);
        assert_eq!(rect.x0, crate::theme::CODE_GUTTER_GAP);
        let (pane_left, _) = pane_origin();
        assert_eq!(
            rect.x1,
            1400.0 - pane_left - crate::theme::GUTTER_WIDTH - crate::theme::CODE_GUTTER_GAP,
            "to the right edge, less the margin"
        );

        // Opened, the column's slot for the composer is empty: the
        // composer is in the code and nowhere else.
        state.threads_open.set(true);
        harness.rebuild();
        harness.paint();
        assert_eq!(column_slot(&harness), "Empty");
        assert!(find(harness.root_id(), "inline-card:composer").is_some());

        // Escape, with nothing typed, puts the composer away, and the
        // rows go with it.
        harness.dispatch_event(key_press(Code::Escape, Key::Named(NamedKey::Escape)));
        harness.rebuild();
        harness.paint();
        assert!(state.composer.get().is_none(), "escape did not close the empty composer");
        let doc = live.doc.get();
        assert_eq!(doc.len(), 120);
        assert_eq!(doc.card_span(&card), None);
        assert_eq!(laid_out_text(&live), doc.text);
        assert!(find(harness.root_id(), "inline-card:").is_none());
        assert!(!state.card_rows.get().contains_key(&card), "the next composer starts on the estimate");

        // A question about the worktree is the column's, as it was.
        let choice = (perch_agent::Backend::ClaudeCode, "sonnet".to_string(), perch_agent::Effort::Medium);
        state.open_composer(None, None, String::new(), choice);
        harness.rebuild();
        harness.paint();
        assert!(!state.composer_is_inline());
        assert_ne!(column_slot(&harness), "Empty");
        assert_eq!(live.doc.get().len(), 120);
        assert!(find(harness.root_id(), "inline-card:").is_none());
    }

    #[test]
    fn a_sent_question_keeps_its_rows_until_its_thread_arrives_and_the_threads_card_takes_them() {
        use crate::review::doc::CardId;
        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = crate::theme::CODE_LINE_HEIGHT as f64;
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).expect("a stacked editor was built");
        let anchor = perch_github::Anchor {
            path: "src/x.rs".into(),
            start_line: 5,
            line: 7,
            side: perch_github::Side::Right,
            commit_sha: "h".into(),
        };
        let choice = (perch_agent::Backend::ClaudeCode, "sonnet".to_string(), perch_agent::Effort::Medium);
        state.open_composer(None, Some(anchor), String::new(), choice);
        harness.rebuild();
        harness.paint();
        let rows = laid_out_rows(&harness, "composer");
        assert_eq!(live.doc.get().card_span(&CardId::Composer), Some(9..9 + rows));

        // Sent: the composer is gone from the code, and its rows are not.
        state.composer_sent();
        harness.rebuild();
        harness.paint();
        assert!(state.composer.get().is_none());
        assert!(find(harness.root_id(), "inline-card:").is_none(), "the question went, and the box with it");
        let doc = live.doc.get();
        assert_eq!(doc.card_span(&CardId::Composer), Some(9..9 + rows), "the rows stay under the lines");
        assert_eq!(doc.len(), 120 + rows);
        assert_eq!(laid_out_text(&live), doc.text);
        assert_eq!(viewport.get().y0, 0.0);

        // The thread arrives, and its card takes the rows in one edit:
        // the composer's spacers become the thread's, and the lines
        // above are where they were. One edit, not the rows given up and
        // then the thread's added: the document is built once for it.
        let built = doc.clone();
        let builds = floem::reactive::RwSignal::new(0);
        let doc_signal = live.doc;
        floem::reactive::Effect::new(move |_| {
            floem::reactive::SignalTrack::track(&doc_signal);
            builds.update(|b| *b += 1);
        });
        assert_eq!(builds.get(), 1, "the effect's first run");
        let thread = thread_with("t1", 5, 7, &["why?"]);
        let estimate = crate::review::doc::estimate_rows(&thread, false);
        state.set_comment_threads(Arc::new(vec![thread]));
        assert_eq!(builds.get(), 2, "the document was built once");
        let doc = live.doc.get();
        let thread_card = CardId::Thread("t1".into());
        assert_eq!(doc.card_span(&CardId::Composer), None);
        assert_eq!(doc.card_span(&thread_card), Some(9..9 + estimate));
        assert_eq!(doc.rows[..9], built.rows[..9]);
        harness.rebuild();
        harness.paint();
        let laid = laid_out_rows(&harness, "t1");
        let doc = live.doc.get();
        assert_eq!(doc.card_span(&thread_card), Some(9..9 + laid));
        assert_eq!(doc.len(), 120 + laid);
        assert_eq!(laid_out_text(&live), doc.text);
        assert_eq!(card_rect(&harness, "t1").y0, 9.0 * line_height + crate::theme::CARD_INSET);
        assert!(!state.card_rows.get().contains_key(&CardId::Composer));
    }
}
