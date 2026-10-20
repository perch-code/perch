//! The code pane: gutter, editor, references panel, and the states that
//! stand in for them.
//!
//! Two layouts. Stacked is one editor over the document built from the
//! unified buffer, the head with the removed lines spliced in. Split is two
//! editors, base on the left and head on the right, each over a document
//! with one line per aligned row so the two are the same height and a row
//! on one side is the row beside it on the other. The right editor owns the
//! scroll position and the left follows it. Either side takes a selection,
//! and the cursor the reader last moved is the one the pane reads; the
//! jumps and the page keys move the right one.
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
//!
//! The rows' diff tints are a layer under the editor, placed the same
//! way, so that a selection is painted over a changed line's tint and not
//! under it.

use std::rc::Rc;
use std::sync::Arc;

use floem::context::LayoutChanged;
use floem::event::PointerScrollEventExt;
use floem::kurbo::{Rect, Size, Vec2};
use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::views::editor::core::cursor::{Cursor, CursorAffinity, CursorMode};
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::scroll::ScrollCustomStyle;
use floem::views::{
    dyn_container, dyn_stack, text_editor, ClipExt, Container, Decorators, Empty, Label, Stack,
    TextEditor,
};
use perch_github::CommentThread;

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::review::cards;
use crate::review::doc::{self, Card, CardId, Doc, DocRow, Reveals, Run, Tint};
use crate::review::find;
use crate::review::gutter::{
    self, column_offset, diff_gutter, plain_gutter, side_gutter, visible_lines,
};
use crate::review::hierarchy;
use crate::review::hover;
use crate::review::model::{
    display_line_of_base_line, Composer, FilePlace, Layout, LiveDoc, LiveDocs, LiveFile,
    LiveLayout, LoadedFile, Paging, ReviewState,
};
use crate::review::navigation;
use crate::review::picture;
use crate::review::references;
use crate::review::sticky;
use crate::review::styling::PerchStyling;
use crate::review::threads::{self, ThreadFace};
use crate::review::OpenFile;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{centered_message, clickable, icon_in};

pub fn editor_pane(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
) -> impl IntoView {
    // A jump, to a definition or back along the history, is a signal: its
    // answer arrives asynchronously, and opening the tab has to happen on
    // the UI thread, with the closure that opens files. Opening the active
    // file again would rebuild its editor for nothing; the jump alone
    // moves it.
    {
        let open_file = open_file.clone();
        Effect::new(move |_| {
            if let Some((path, line, column)) = state.pending_jump.get() {
                state.pending_jump.set(None);
                if state
                    .active
                    .with_untracked(|a| a.as_deref() != Some(path.as_str()))
                {
                    state.open_for_jump(|| open_file(&path));
                }
                state.jump_caret.set(Some((line, column)));
                state.jump_to_line.set(Some(line));
            }
        });
    }
    // A file a link names with no line opens as the tree opens it, where
    // it opens; the active file is only shown in the tree.
    {
        let open_file = open_file.clone();
        Effect::new(move |_| {
            if let Some(path) = state.pending_open.get() {
                state.pending_open.set(None);
                if state
                    .active
                    .with_untracked(|a| a.as_deref() != Some(path.as_str()))
                {
                    state.open_for_jump(|| open_file(&path));
                } else {
                    state.show_in_tree(&path);
                }
            }
        });
    }

    // An item's first visit to Code opens the tree beside it, open files
    // or not. Only then, so a tree the reader shuts on the item stays
    // shut.
    {
        let backend = backend.clone();
        Effect::new(move |_| {
            if state.tree_wanted.get() {
                state.tree_wanted.set(false);
                crate::ui::drawer::show(app, &backend, None);
            }
        });
    }

    // The find bar's matches follow the file and the query from here,
    // beside the jump: one effect for the pane, whichever editor is up.
    find::wire(state);

    let code_backend = backend.clone();

    Stack::vertical((
        Stack::new((
            dyn_container(
                // The epoch changes whenever the active file or its content
                // changes, and the layout when the toggle is clicked. Another
                // file or another layout is another editor: a fresh one resets
                // the layout caches cleanly, and nothing of the old is worth
                // keeping. What changes under the one file is for editing the
                // document in place, through the live documents held below.
                // A picture flipped to its markup, or back, is another view
                // of the file the same way.
                move || {
                    let path = state.active.get();
                    let source = path.as_deref().is_some_and(|path| state.shows_source(path));
                    (path, state.editor_epoch.get(), state.layout.get(), source)
                },
                move |(path, epoch, layout, source)| {
                    // Whatever editor stood here is going with its documents;
                    // the one built below registers its own.
                    state.docs.set(None);
                    match path {
                        None if state.changed_files.with_untracked(|f| f.is_empty()) => {
                            centered_message(
                                SidebarIcon::Search,
                                || "No changes".to_string(),
                                || {
                                    "Pick a file from the tree, or press ⌘P to go to one by name."
                                        .to_string()
                                },
                            )
                            .into_any()
                        }
                        None => centered_message(
                            SidebarIcon::Search,
                            || "No file open".to_string(),
                            || {
                                "Pick a file from the tree, or press ⌘P to go to one by name."
                                    .to_string()
                            },
                        )
                        .into_any(),
                        Some(path) => match state.file(&path) {
                            Some(file) => {
                                // A file that draws is drawn, unless the reader
                                // asked for its markup.
                                let drawn = if source { None } else { file.picture.clone() };
                                match drawn {
                                    Some(picture) => {
                                        // No editor, no selection: `a` would
                                        // otherwise anchor a question on the
                                        // lines the last editor had selected.
                                        state.selection.set(None);
                                        state.selection_base.set(None);
                                        picture::pane(&file, &picture).into_any()
                                    }
                                    None => code_view(
                                        app,
                                        state,
                                        file,
                                        epoch,
                                        layout,
                                        code_backend.clone(),
                                    )
                                    .into_any(),
                                }
                            }
                            None => {
                                ask_again_while_opening(
                                    state,
                                    code_backend.clone(),
                                    path.clone(),
                                    epoch,
                                    ASK_AGAIN_AFTER,
                                );
                                loading_view(path).into_any()
                            }
                        },
                    }
                },
            )
            .style(|s| s.size_full().background(theme::code_bg())),
            // The find bar hangs from the top right of the code, over it and
            // outside the container that swaps editors, so it stays up with
            // its text and its keyboard from one file to the next.
            find::bar(state, find::Pane::Code),
        ))
        .style(|s| {
            s.flex_grow(1.0_f32)
                .min_height(0.0)
                .width_full()
                .position(Position::Relative)
        }),
        references::panel(state, open_file),
        hierarchy::panel(state, backend),
    ))
    .style(|s| s.size_full().flex_col().min_height(0.0))
}

/// What a file's document is built from besides the file: whether the
/// file is folded, the head lines its threads are anchored on, the
/// reveals for its content, its cards with the rows each has, and the
/// columns its lines wrap at. Read tracked, so an effect over them runs
/// when any changes, or untracked for the document an editor is built
/// from.
struct Inputs {
    folded: bool,
    anchors: Vec<(usize, usize)>,
    reveals: Reveals,
    cards: Vec<Card>,
    /// The columns a row holds while lines wrap; `None` while they run on.
    wrap: Option<usize>,
}

impl Inputs {
    /// `file`'s stacked document; see `stacked_doc`.
    fn stacked(&self, file: &LoadedFile) -> Doc {
        stacked_doc(
            file,
            self.folded,
            &self.anchors,
            &self.reveals,
            &self.cards,
            self.wrap,
        )
    }

    /// `file`'s split documents; see `split_doc`.
    fn split(&self, file: &LoadedFile) -> Option<(Doc, Doc)> {
        split_doc(
            file,
            self.folded,
            &self.anchors,
            &self.reveals,
            &self.cards,
            self.wrap,
        )
    }
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

    fn inputs(&self, state: ReviewState, file: &LoadedFile, wrap: Option<usize>) -> Inputs {
        Inputs {
            folded: state.folded(&file.path),
            anchors: self.anchors.get(),
            reveals: state.reveals_for(&file.path, &file.blob),
            cards: self.cards.get(),
            wrap,
        }
    }
}

/// The columns an editor of `layout` is built wrapping at, while lines
/// wrap: the last such editor's. Its own width is not known until it has
/// been laid out, and the window is seldom another width by then.
fn first_columns(state: ReviewState, layout: Layout) -> Option<usize> {
    if !state.wrap.get_untracked() {
        return None;
    }
    state.wrap_columns.with_untracked(|c| c[layout.index()])
}

/// The columns an editor's rows hold while lines wrap, from its width,
/// with a gap at the right edge as wide as the one at the left, and
/// `None` while they run on. Until the editor has a width, what it was
/// built with. Each width is kept for the next editor of the layout; see
/// `first_columns`.
fn wrap_columns(
    state: ReviewState,
    viewport: RwSignal<Rect>,
    layout: Layout,
) -> Memo<Option<usize>> {
    let columns = Memo::new(move |_| {
        if !state.wrap.get() {
            return None;
        }
        let width = viewport.get().width();
        crate::review::wrap::columns_for(width, gutter::column_width(), theme::CODE_GUTTER_GAP)
            .or_else(|| state.wrap_columns.with_untracked(|c| c[layout.index()]))
    });
    Effect::new(move |_| {
        if let Some(c) = columns.get() {
            state
                .wrap_columns
                .update(|all| all[layout.index()] = Some(c));
        }
    });
    columns
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
) -> impl IntoView {
    let memos = Memos::new(state, &file);
    let split = Effect::untrack(|| match layout {
        Layout::Split => memos
            .inputs(state, &file, first_columns(state, Layout::Split))
            .split(&file),
        Layout::Stacked => None,
    });
    // New sides of the file that fit this editor are set in here, and
    // the effects below edit them into its documents; others built a new
    // editor, in `finish_load`, and this one is on its way out.
    let live = RwSignal::new(file.clone());
    let path = file.path.clone();
    Effect::new(move |_| {
        let Some(new) = state.files.with(|m| m.get(&path).cloned()) else {
            return;
        };
        if state.editor_epoch.get_untracked() != epoch {
            return;
        }
        let old = live.get_untracked();
        if !Arc::ptr_eq(&old, &new) && edits_in_place(&old, &new) {
            live.set(new);
        }
    });
    match split {
        Some(split) => split_view(app, state, live, memos, split, epoch, backend).into_any(),
        None => stacked_view(app, state, live, memos, epoch, backend).into_any(),
    }
}

/// Whether `new`, a reload of the file `old` under the same scope, can be
/// edited into the editor showing `old` rather than building another: the
/// same kind of view over it, stacked or split, code or picture, and a
/// gutter of the same width, which the layers are placed by. A file
/// deleted or brought back pins the other side's lines, so is built
/// again.
pub(crate) fn edits_in_place(old: &LoadedFile, new: &LoadedFile) -> bool {
    old.scope == new.scope
        && old.origin == new.origin
        && old.picture.is_none()
        && new.picture.is_none()
        && splits(old) == splits(new)
        && sticky::pins_base(old) == sticky::pins_base(new)
        && gutter::width_of(old) == gutter::width_of(new)
}

#[allow(clippy::too_many_arguments)]
fn stacked_view(
    app: AppState,
    state: ReviewState,
    live_file: LiveFile,
    memos: Memos,
    epoch: u64,
    backend: Rc<Backend>,
) -> impl IntoView {
    let file = live_file.get_untracked();
    let built = Effect::untrack(|| {
        memos
            .inputs(state, &file, first_columns(state, Layout::Stacked))
            .stacked(&file)
    });
    let text = built.text.clone();
    let doc = RwSignal::new(Arc::new(built));
    let editor = code_editor(
        text,
        PerchStyling::new(
            live_file,
            doc,
            state.highlights,
            state.find_matches,
            state.find_current,
            epoch,
        ),
    );
    let live = LiveDoc::of(&editor, doc);
    state.docs.set(Some(LiveDocs {
        epoch,
        layout: LiveLayout::Stacked(live.clone()),
    }));

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
    wire_highlights(state, &live, live_file);
    let columns = wrap_columns(state, viewport, Layout::Stacked);
    wire_folds(state, live_file, memos, columns, epoch, {
        move |file: &LoadedFile, inputs: &Inputs, reloaded: bool| {
            apply(
                &live,
                inputs.stacked(file),
                viewport,
                scroll_to,
                cursor,
                shifted,
                reloaded,
            )
        }
    });
    wire_cursor(
        state,
        backend.clone(),
        file.clone(),
        doc,
        cursor,
        shifted,
        CursorOf::Stacked,
    );
    wire_place(
        state,
        file.path.clone(),
        doc,
        viewport,
        scroll_to,
        cursor,
        epoch,
    );
    wire_jump(state, live_file, doc, scroll_to, cursor, epoch);
    wire_page(state, doc, viewport, scroll_delta, cursor, epoch);
    let editor = navigation::navigable(editor, state, backend.clone(), file.clone(), doc);

    let path = file.path.clone();
    let sticky = sticky::layer(
        state,
        backend.clone(),
        live_file,
        doc,
        viewport,
        gutter::width_of(&file),
        scroll_delta,
        None,
        Rc::new(move |y| scroll_to.set(Some(Vec2::new(viewport.get_untracked().x0, y)))),
    );
    // A file outside the worktree is a place to read: numbers beside it,
    // and no diff gutter, since there is no diff.
    let gutter = if file.origin.is_outside() {
        plain_gutter(state, app, live_file, doc, viewport, scroll_delta).into_any()
    } else {
        diff_gutter(state, app, live_file, doc, viewport, scroll_delta).into_any()
    };
    Stack::horizontal((
        gutter,
        // The layers are placed against this box, so it takes the
        // relative position; the editor fills it. The cards are over the
        // dividers, since a card is never in a divider.
        Stack::vertical((
            over_tints(editor, doc, viewport),
            divider_layer(state, live_file, doc, viewport, scroll_delta, Bars::Editor),
            card_layer(
                app,
                state,
                backend,
                path,
                doc,
                viewport,
                scroll_delta,
                None,
                scroll_to,
                theme::CODE_GUTTER_GAP,
                Some(theme::INLINE_THREAD_MAX_WIDTH),
            ),
        ))
        .style(|s| {
            s.flex_grow(1.0_f32)
                .min_width(0.0)
                .height_full()
                .position(Position::Relative)
        }),
        // Over the gutter and the code both, and over the cards.
        sticky,
    ))
    .style(|s| s.size_full().min_height(0.0).position(Position::Relative))
}

/// Whether a file has two sides to split: a base to put on the left, and
/// the alignment of the two, which a file that is not text has not.
pub(crate) fn splits(file: &LoadedFile) -> bool {
    file.base_buffer.is_some() && !file.truncated && !file.alignment.is_empty()
}

/// The folds of a file: the runs around its changes and anchors less the
/// reveals, or none when the worktree's files are shown whole. A file
/// outside the changed list has no status and is never folded, whatever
/// is anchored in it: there is no change to fold around, and it opens
/// whole as it did before there were folds.
fn folds_of(
    file: &LoadedFile,
    folded: bool,
    anchors: &[(usize, usize)],
    reveals: &Reveals,
) -> Vec<Run> {
    if folded && file.status.is_some() {
        reveals.apply(&doc::runs(&file.buffer, anchors))
    } else {
        Vec::new()
    }
}

/// The stacked document of a file: folded to the changes and the anchors
/// less the reveals, or whole, with each card's rows reserved under its
/// line, and its lines wrapped at `wrap` columns when there are some.
pub(crate) fn stacked_doc(
    file: &LoadedFile,
    folded: bool,
    anchors: &[(usize, usize)],
    reveals: &Reveals,
    cards: &[Card],
    wrap: Option<usize>,
) -> Doc {
    doc::build(
        &file.buffer,
        &folds_of(file, folded, anchors, reveals),
        cards,
        wrap,
    )
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
    wrap: Option<usize>,
) -> Option<(Doc, Doc)> {
    if !splits(file) {
        return None;
    }
    let base = file.base_buffer.as_ref()?;
    let folds = folds_of(file, folded, anchors, reveals);
    Some(doc::build_split(
        &file.alignment,
        &base.text,
        &file.buffer,
        &folds,
        cards,
        wrap,
    ))
}

/// Runs `rebuild` whenever what a document is built from changes, so it
/// can build the new document and edit the editor to match, told whether
/// the file is a new one. The inputs are read tracked, so the effect runs
/// on each of them: the file, when a reload is set in its place, Show
/// whole, the file's reveals, the anchors and the cards, which are memos so that
/// a thread's messages changing under them, as they do every line of a
/// streaming answer, do not rebuild a document that would be the same,
/// and the columns lines wrap at, which change as Word wrap is turned on
/// or off and as the pane is made wider or narrower.
fn wire_folds(
    state: ReviewState,
    file: LiveFile,
    memos: Memos,
    columns: Memo<Option<usize>>,
    epoch: u64,
    rebuild: impl Fn(&LoadedFile, &Inputs, bool) + 'static,
) {
    Effect::new(move |before: Option<Arc<LoadedFile>>| {
        let file = file.get();
        let inputs = memos.inputs(state, &file, columns.get());
        // The editor was just built from these; nothing to do but subscribe.
        let Some(before) = before else {
            return file;
        };
        if state.editor_epoch.get_untracked() != epoch {
            return file;
        }
        rebuild(&file, &inputs, !Arc::ptr_eq(&before, &file));
        file
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
///
/// A new document over a reloaded file is another buffer, whose display
/// lines are counted afresh, so a line is not found again by its number:
/// a region wholly above the view moves the offset by the rows it added
/// or took away, and one reaching into the view leaves it where it is.
#[allow(clippy::too_many_arguments)]
fn apply(
    live: &LiveDoc,
    new: Doc,
    viewport: RwSignal<Rect>,
    scroll_to: RwSignal<Option<Vec2>>,
    cursor: RwSignal<Cursor>,
    shifted: RwSignal<Option<usize>>,
    reloaded: bool,
) {
    let old = live.doc.get_untracked();
    let region = if reloaded {
        doc::changed_region_renumbered(&old, &new)
    } else {
        doc::changed_region(&old, &new)
    };
    if *old != new {
        live.doc.set(Arc::new(new.clone()));
    }
    let Some(region) = region else {
        return;
    };
    let replacement = new.text[region.new_bytes.clone()].to_string();
    live.handle.edit_single(
        Selection::region(
            region.old_bytes.start,
            region.old_bytes.end,
            CursorAffinity::Backward,
        ),
        &replacement,
        EditType::Other,
    );

    let start = region.old_bytes.start;
    let delta = region.byte_delta();
    if cursor.with_untracked(|c| c.regions_iter().any(|(a, b, _)| a.max(b) >= start)) {
        // Said before the move, which runs the cursor's effect.
        shifted.set(Some(shifted_offset(
            cursor.with_untracked(|c| c.offset()),
            start,
            delta,
        )));
        cursor.update(|c| shift_cursor(c, start, delta));
    }

    // Set whether or not the offset moves: moving the cursor asks the
    // editor to scroll it into view, and this, asked for after that, is
    // what the editor does last. Setting it also clamps the offset to the
    // new height, which a document that shrank under the scroll needs.
    let line_height = theme::code_line_height() as f64;
    let vp = viewport.get_untracked();
    let first_visible = (vp.y0 / line_height).floor() as usize;
    let y = if reloaded {
        if region.old_rows.end <= first_visible {
            vp.y0 + region.row_delta() as f64 * line_height
        } else {
            vp.y0
        }
    } else if region.old_rows.start < first_visible {
        // The first line in view that the new document still shows keeps
        // its place; a region that spans the viewport, as Show whole's
        // does, adds and takes away below it too, and that is not counted.
        // When no line from the top of the view down is still shown, the
        // rows after the region keep their place instead.
        // A row of a wrapped line keeps its place as the same row of the
        // line, or the line's last when it is on fewer rows now.
        let shift = (first_visible..old.len())
            .find_map(|row| {
                let lead = new.row_of_display(old.display_line(row)?)?;
                let into = (row - old.lead(row)).min(new.line_rows(lead).len() - 1);
                Some((lead + into) as isize - row as isize)
            })
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

/// The colour behind a row: a line's diff tint, on every row of a
/// wrapped one, grey for a split's filler, github.com's hunk row blue for
/// a divider. A plain line has none, and a spacer none, since the card is
/// drawn over its rows.
pub(crate) fn row_tint(row: &DocRow) -> Option<Color> {
    match row {
        DocRow::Line { tint, .. } | DocRow::Wrap { tint, .. } => match tint {
            Tint::Plain => None,
            Tint::Added => Some(theme::diff_added_bg()),
            Tint::Modified => Some(theme::diff_modified_bg()),
            Tint::Deleted => Some(theme::diff_deleted_bg()),
        },
        DocRow::Filler => Some(theme::diff_filler_bg()),
        DocRow::Divider { .. } => Some(theme::diff_divider_bg()),
        DocRow::Spacer { .. } => None,
    }
}

/// The rows' tints, under the editor: each row's colour across the box
/// the editor fills, placed from the viewport the way the gutter's column
/// is. Under, and not among the editor's own line styles, because the
/// editor paints those after the selection, and a tint there covered the
/// selection on every changed line; here the selection is over the tint,
/// as the text is. The tint runs from the gutter's edge, across the gap
/// before the text, which a line style left bare.
fn tint_layer(doc: RwSignal<Arc<Doc>>, viewport: RwSignal<Rect>) -> impl IntoView {
    let line_height = theme::code_line_height() as f64;
    let visible = move || {
        let vp = viewport.get();
        doc.with(|d| {
            visible_lines(vp.y0, vp.height(), line_height, d.len())
                .into_iter()
                .map(|row| (row, d.rows[row].clone()))
                .collect::<Vec<_>>()
        })
    };
    let column = dyn_stack(
        visible,
        |(row, entry)| (*row, entry.clone()),
        move |(row, entry)| {
            Empty::new()
                .debug_name(format!("tint:{row}"))
                .style(move |s| {
                    s.width_full()
                        .height(line_height)
                        .flex_shrink(0.0_f32)
                        .apply_opt(row_tint(&entry), |s, bg| s.background(bg))
                })
        },
    )
    .style(move |s| {
        // Slid by the sub-line remainder of the scroll, read live here
        // as the gutter reads it.
        let offset = column_offset(viewport.get().y0, line_height);
        s.absolute().inset_top(offset).width_full().flex_col()
    });

    Container::new(column)
        .style(|s| s.size_full().position(Position::Relative))
        .clip()
        .style(|s| s.absolute().inset(0.0).size_full().pointer_events_none())
}

/// An editor over its rows' tints: a box with the code's background, the
/// tint layer over that, and the editor over both, painting no background
/// of its own and set in from the left by the gap before the code.
fn over_tints(
    editor: impl IntoView + 'static,
    doc: RwSignal<Arc<Doc>>,
    viewport: RwSignal<Rect>,
) -> impl IntoView {
    // Breathing room between the change bars and the first character, so
    // a bar never reads as part of the code. Padding on this box rather
    // than on the editor: see `code_editor`.
    let editor = Container::new(editor).style(|s| {
        s.size_full()
            .min_width(0.0)
            .padding_left(theme::CODE_GUTTER_GAP)
    });
    Stack::vertical((tint_layer(doc, viewport), editor))
        .debug_name("tinted editor")
        .style(|s| {
            s.size_full()
                .min_width(0.0)
                .position(Position::Relative)
                .background(theme::code_bg())
        })
}

/// Where a divider layer's bars sit in the box the layer covers.
#[derive(Clone, Copy)]
enum Bars {
    /// Across the whole box: the stacked layout's editor.
    Editor,
    /// Across a split, whose layer covers the outer stack so that the
    /// bars are placed from the one scroll. The base side's gutter is left
    /// bare, since its rows are the expanders; the header is over the base
    /// side's code, from its first column, where github.com sets it at the
    /// row's left; and the rest, the rule and the head side, gutter and
    /// all, is the divider's tint from the layers beneath, and records the
    /// hover as the header does, so the whole row answers the pointer. A
    /// wheel over the header goes down the file with the head side and
    /// sideways to the base side's own scroll, `base_delta`, as one over
    /// the base side's code does.
    Split { base_delta: RwSignal<Vec2> },
}

/// The dividers' hunk headers, drawn over the editor at their rows:
/// `@@ -1473,6 +1509,7 @@ impl Manifest {` from the first code column,
/// down the middle of the divider's one or two rows, over the bar the
/// rows' own background paints. Placed the way the gutter's column is,
/// from the viewport, with each bar carrying its own offset because the
/// bars are sparse. The layer takes no pointer events; each bar does, to
/// record the hover that shows Fold again, and hands the wheel on to the
/// editor. The headers are worked out when the folds change, rather than
/// for each bar as it scrolls into view or each time a card's rows do.
fn divider_layer(
    state: ReviewState,
    file: LiveFile,
    doc: RwSignal<Arc<Doc>>,
    viewport: RwSignal<Rect>,
    scroll_delta: RwSignal<Vec2>,
    bars: Bars,
) -> impl IntoView {
    let line_height = theme::code_line_height() as f64;
    let folds = Memo::new(move |_| doc.with(|d| d.folds.clone()));
    let labels = Memo::new(move |_| {
        file.with(|file| {
            let base = file.base_buffer.as_ref().map(|b| b.text.as_str());
            folds.with(|folds| doc::hunk_labels(&file.buffer, base, folds))
        })
    });
    // Each divider in view once, by its rows, however many of them are.
    let visible = move || {
        let vp = viewport.get();
        doc.with(|d| {
            let mut out: Vec<(std::ops::Range<usize>, Run)> = Vec::new();
            for row in visible_lines(vp.y0, vp.height(), line_height, d.len()) {
                let (Some(span), Some(run)) = (d.divider_span(row), d.run_of(row)) else {
                    continue;
                };
                if out.last().is_none_or(|(last, _)| *last != span) {
                    out.push((span, run));
                }
            }
            out
        })
    };
    let stack = dyn_stack(
        visible,
        |(span, run)| (span.clone(), *run),
        move |(span, _)| divider_bar(state, file, doc, labels, span, viewport, scroll_delta, bars),
    )
    .style(|s| s.size_full());

    // A click on a bar gives the keyboard to the layer, which stays as
    // the bars scroll, so `]` works after it as after a click in the
    // gutter. Not to the header: floem's theme makes a label focusable,
    // and a header that took the keyboard would lose it when its bar
    // scrolled away.
    Container::new(stack)
        .style(|s| s.size_full().position(Position::Relative))
        .clip()
        .style(|s| {
            s.absolute()
                .inset(0.0)
                .size_full()
                .pointer_events_none()
                .focusable()
                .class(floem::views::LabelClass, |s| s.focus_none())
        })
}

/// One divider's bar over its rows. The header is the part that takes the
/// pointer; in a split, the rule and the head side take it too, and the
/// base side's gutter lets it through to the expanders beneath.
#[allow(clippy::too_many_arguments)]
fn divider_bar(
    state: ReviewState,
    file: LiveFile,
    doc: RwSignal<Arc<Doc>>,
    labels: Memo<Vec<Option<String>>>,
    span: std::ops::Range<usize>,
    viewport: RwSignal<Rect>,
    scroll_delta: RwSignal<Vec2>,
    bars: Bars,
) -> impl IntoView {
    let line_height = theme::code_line_height() as f64;
    let (start, height) = (span.start, span.len() as f64 * line_height);
    let gutter = file.with_untracked(|file| gutter::width_of(file));
    let header = divider_header(state, file, doc, labels, start);

    let bar = match bars {
        Bars::Editor => records_hover(
            state,
            start,
            passes_wheel(header, viewport, scroll_delta, None),
        )
        .into_any(),
        // The outer stack's shape: two halves about a one-pixel rule,
        // each beginning with its gutter.
        Bars::Split { base_delta } => {
            let base = Stack::horizontal((
                Empty::new().style(move |s| s.width(gutter).height_full().flex_shrink(0.0_f32)),
                records_hover(
                    state,
                    start,
                    passes_wheel(header, viewport, scroll_delta, Some(base_delta)),
                ),
            ))
            .style(|s| {
                s.flex_basis(0.0)
                    .flex_grow(1.0_f32)
                    .min_width(0.0)
                    .height_full()
            });
            let rest = Stack::horizontal((
                Empty::new().style(|s| s.width(1.0).height_full().flex_shrink(0.0_f32)),
                Empty::new().style(|s| s.flex_basis(0.0).flex_grow(1.0_f32).height_full()),
            ))
            .style(|s| {
                s.flex_basis(0.0)
                    .flex_grow(1.0_f32)
                    .min_width(0.0)
                    .height_full()
            });
            let rest = passes_wheel(rest, viewport, scroll_delta, None);
            Stack::horizontal((base, records_hover(state, start, rest))).into_any()
        }
    };
    bar.style(move |s| {
        // Read live, as the gutter reads its offset: the bar outlives a
        // scroll, and a position captured when it was built would not.
        let top = start as f64 * line_height - viewport.get().y0;
        s.absolute()
            .inset_top(top)
            .inset_left(0.0)
            .width_full()
            .height(height)
    })
}

/// A part of a divider's bar that hands the wheel on: down the file to
/// the editor that owns the scroll, `scroll_delta`, and across to
/// `across` where the part is over another editor, or to the same one.
pub(crate) fn passes_wheel(
    part: impl IntoView + 'static,
    viewport: RwSignal<Rect>,
    scroll_delta: RwSignal<Vec2>,
    across: Option<RwSignal<Vec2>>,
) -> impl IntoView {
    let line_height = theme::code_line_height() as f64;
    part.on_event_stop(el::PointerWheel, move |_, wheel| {
        let size = viewport.get_untracked().size();
        let delta = wheel.resolve_to_points(Some(Size::new(line_height, line_height)), Some(size));
        match across {
            None => scroll_delta.set(Vec2::new(-delta.x, -delta.y)),
            Some(across) => {
                scroll_delta.set(Vec2::new(0.0, -delta.y));
                across.set(Vec2::new(-delta.x, 0.0));
            }
        }
    })
}

/// A divider's header and its Fold again: the hunk header in the code's
/// font, muted as github.com sets it, from the first code column and cut
/// short with an ellipsis where the bar ends; and after it, while the
/// pointer is on the divider, Fold again when a reveal overlaps the run's
/// default, or the default of a run taken whole, divider and all, that
/// this divider carries; see `doc::reset_ranges`. The run and its header
/// are read from the document as it is now, not as it was when the bar
/// was built: a reveal below it grows the hunk the header counts.
fn divider_header(
    state: ReviewState,
    file: LiveFile,
    doc: RwSignal<Arc<Doc>>,
    labels: Memo<Vec<Option<String>>>,
    start: usize,
) -> impl IntoView {
    let label = Label::derived(move || {
        let run = doc.with(|d| match d.rows.get(start) {
            Some(DocRow::Divider { run, .. }) => Some(*run),
            _ => None,
        });
        run.and_then(|i| labels.with(|l| l.get(i).cloned().flatten()))
            .unwrap_or_default()
    })
    .style(|s| {
        s.selectable(false)
            .min_width(0.0)
            .font_family(theme::CODE_FONT_STACK.to_string())
            .font_size(theme::code_font_size())
            .color(theme::diff_divider_fg())
            .text_ellipsis()
    });
    let reset = dyn_container(
        move || {
            let hovered = state.hovered_line.get() == Some(start);
            let run = doc.with(|d| d.run_of(start)).filter(|_| hovered)?;
            let reset_file = file.get();
            let defaults = doc::runs(&reset_file.buffer, &state.anchors_in(&reset_file.path));
            let ranges = doc.with(|d| doc::reset_ranges(&d.folds, &defaults, run));
            let reveals = state.reveals_for(&reset_file.path, &reset_file.blob);
            ranges
                .iter()
                .any(|&(a, b)| reveals.overlaps(a, b))
                .then_some(ranges)
        },
        move |ranges| match ranges {
            Some(ranges) => fold_again(state, file, ranges).into_any(),
            None => Empty::new().into_any(),
        },
    );
    Stack::horizontal((label, reset)).style(|s| {
        s.flex_grow(1.0_f32)
            .min_width(0.0)
            .height_full()
            .items_center()
            .gap(6.0)
            .padding_horiz(theme::CODE_GUTTER_GAP)
    })
}

/// Fold again: a 16px slot holding a 12px Undo glyph, muted until the
/// pointer is on it, that folds `ranges`, the head ranges of runs as the
/// folds have them by default, back.
fn fold_again(state: ReviewState, file: LiveFile, ranges: Vec<(usize, usize)>) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let color = move || {
        if hovered.get() {
            theme::accent()
        } else {
            theme::fg_muted()
        }
    };
    cards::tip(
        Container::new(
            icon_in(
                move || icons::sidebar_icon(SidebarIcon::Undo, color()),
                color,
            )
            .style(|s| s.size(theme::CHEVRON_GLYPH, theme::CHEVRON_GLYPH)),
        )
        .debug_name("fold again")
        .on_event_stop(el::Click, move |_, _| {
            file.with_untracked(|file| state.reset(&file.path, &file.blob, &ranges))
        })
        .on_event_cont(el::PointerEnter, move |_, _| hovered.set(true))
        .on_event_cont(el::PointerLeave, move |_, _| hovered.set(false))
        .style(|s| {
            clickable(s)
                .size(theme::CHEVRON_SLOT, theme::CHEVRON_SLOT)
                .flex_shrink(0.0_f32)
                .items_center()
                .justify_center()
                .border_radius(3.0)
                .hover(|s| s.background(theme::badge_bg()))
        }),
        "Fold again".to_string(),
    )
}

/// A part of a divider's bar that records the pointer as the divider's
/// first row, as its expanders in the gutter do, so that Fold again shows
/// while the pointer is anywhere on the divider.
fn records_hover(state: ReviewState, row: usize, part: impl IntoView + 'static) -> impl IntoView {
    part.on_event_cont(el::PointerEnter, move |cx, _| {
        state.enter_row(cx.target, row)
    })
    .on_event_cont(el::PointerLeave, move |cx, _| state.leave_row(cx.target))
    .style(|s| s.pointer_events_auto())
}

/// What a card in the layer is built from: a thread, the reader's own or
/// one of GitHub's, or the composer while it is open on lines of the
/// file. A sent question's card has no item: its rows stay in the
/// document, blank, until its thread arrives.
#[derive(Clone)]
enum CardItem {
    Thread(CommentThread),
    Composer(Composer),
}

impl CardItem {
    fn id(&self) -> CardId {
        match self {
            CardItem::Thread(t) => CardId::Thread(t.id.clone()),
            CardItem::Composer(_) => CardId::Composer,
        }
    }

    /// What the card is keyed on: its id, and what changes what it is
    /// built from. A thread's messages are read live inside its card, so
    /// a streaming answer redraws in place, and the key carries whether
    /// the thread is outdated, which its header and its starting
    /// state show, its anchor, which GitHub moves under a thread of its
    /// own, how many messages it has, which is last and what the first
    /// says, which the collapsed row shows. The composer's carries which
    /// opening it is and nothing else: picking a model writes the
    /// composer, and a rebuild on that would throw the typed question
    /// away.
    fn key(&self) -> String {
        match self {
            CardItem::Thread(t) => {
                format!(
                    "thread:{}:{}:{}",
                    t.id,
                    t.outdated,
                    cards::thread_key_tail(t)
                )
            }
            CardItem::Composer(c) => format!("composer:{}", c.opening),
        }
    }

    /// The name the card is found by in the tree.
    fn name(&self) -> String {
        match self {
            CardItem::Thread(t) => format!("inline-card:{}", t.id),
            CardItem::Composer(_) => "inline-card:composer".to_string(),
        }
    }
}

/// The cards of the open file, drawn over the rows the document reserves
/// for them: the threads anchored in it, the reader's own and GitHub's,
/// and the composer when it is open on lines of it, as the document has
/// them. A layer over the editor, or over one pane of a split, whose
/// document has rows for the cards on that side, that takes no pointer
/// events itself; each card does, and is placed at its first spacer row
/// from the live document and the viewport in its own style closure, as
/// the divider bars are, because the cards are sparse and a card outlives
/// the scroll. `left` is where a card's edge sits: the first code column,
/// past the gutter in a split. `max_width` is how wide a card may be: in
/// the stacked layout no wider than github.com draws a thread, and in a
/// split as wide as its side. The wheel over a card goes down the file to
/// `scroll_delta`, and across to `across` where that is another editor's,
/// as over the base side of a split; see `passes_wheel`.
#[allow(clippy::too_many_arguments)]
fn card_layer(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    path: String,
    doc: RwSignal<Arc<Doc>>,
    viewport: RwSignal<Rect>,
    scroll_delta: RwSignal<Vec2>,
    across: Option<RwSignal<Vec2>>,
    scroll_to: RwSignal<Option<Vec2>>,
    left: f64,
    max_width: Option<f64>,
) -> impl IntoView {
    let cards = move || {
        // The document says which cards have rows: a thread whose line is
        // not shown has no card. The threads and the composer are read
        // tracked so a card's key follows them.
        let ids: Vec<CardId> = doc.with(|d| d.cards().iter().map(|(id, _)| id.clone()).collect());
        let composer = state.composer.with(|c| {
            c.as_ref()
                .filter(|c| {
                    c.thread_id.is_none() && c.anchor.as_ref().is_some_and(|a| a.path == path)
                })
                .cloned()
        });
        state.comment_threads.with(|threads| {
            ids.into_iter()
                .filter_map(|id| match &id {
                    CardId::Thread(thread_id) => threads
                        .iter()
                        .find(|t| t.id == *thread_id && state.thread_visible(t))
                        .cloned()
                        .map(CardItem::Thread),
                    CardId::Composer => composer.clone().map(CardItem::Composer),
                    CardId::Entry(_) => None,
                })
                .collect::<Vec<_>>()
        })
    };
    let stack = dyn_stack(cards, CardItem::key, move |item| {
        inline_card(
            app,
            state,
            backend.clone(),
            doc,
            viewport,
            scroll_delta,
            across,
            scroll_to,
            item,
            left,
            max_width,
        )
    })
    .style(|s| s.size_full());

    Container::new(stack)
        .style(|s| s.size_full().position(Position::Relative))
        .clip()
        .style(|s| s.absolute().inset(0.0).size_full().pointer_events_none())
}

/// One card in the layer, over its rows: a thread of the reader's own in
/// its inline face, the block for one of GitHub's
/// without its heading and lines, or the composer, set into its first
/// spacer row a little way in, so its border never touches a tinted
/// line, from the first code column to the right edge less the same
/// margin, or `max_width` across where that is less. Collapsed, it sits
/// further inside its rows. The wheel over it scrolls
/// the code, as it does over the gutter, unless what is inside it takes
/// the wheel first: down the file with `scroll_delta`, and across with
/// `across` when there is one.
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
/// rows whatever it measures, so it says nothing.
#[allow(clippy::too_many_arguments)]
fn inline_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    doc: RwSignal<Arc<Doc>>,
    viewport: RwSignal<Rect>,
    scroll_delta: RwSignal<Vec2>,
    across: Option<RwSignal<Vec2>>,
    scroll_to: RwSignal<Option<Vec2>>,
    item: CardItem,
    left: f64,
    max_width: Option<f64>,
) -> impl IntoView {
    let line_height = theme::code_line_height() as f64;
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
        let (top, bottom) = (
            span.start as f64 * line_height,
            span.end as f64 * line_height,
        );
        if let Some(y) = scroll_for_card(vp, top, bottom, line_height) {
            scroll_to.set(Some(Vec2::new(vp.x0, y)));
        }
    });

    let view = match item {
        CardItem::Thread(thread) => threads::thread_card(app, state, backend, thread).into_any(),
        CardItem::Composer(composer) => {
            threads::composer_view(app, state, backend, composer, ThreadFace::Inline).into_any()
        }
    };
    let measured_id = card_id.clone();
    let view = view.debug_name(name).on_event_stop(
        LayoutChanged::listener(),
        move |_, LayoutChanged { new_box, .. }| {
            if state.is_collapsed(&measured_id) {
                return;
            }
            let rows = rows_for_height(new_box.height(), line_height, theme::CARD_INSET);
            let has = doc.with_untracked(|d| d.card_span(&measured_id).map(|span| span.len()));
            if has.is_some_and(|has| has != rows) {
                state.set_card_rows(&measured_id, rows);
            }
        },
    );
    passes_wheel(view, viewport, scroll_delta, across).style(move |s| {
        // Read live, as the gutter reads its offset: the card outlives
        // a scroll and an edit above it, and a position captured when
        // it was built would not.
        let row = doc.with(|d| d.first_spacer_row(&card_id)).unwrap_or(0);
        let inset = if state.is_collapsed(&card_id) {
            theme::COLLAPSED_CARD_INSET
        } else {
            theme::CARD_INSET
        };
        let top = row as f64 * line_height - viewport.get().y0 + inset;
        s.absolute()
            .inset_top(top)
            .inset_left(left)
            .inset_right(theme::CODE_GUTTER_GAP)
            .apply_opt(max_width, |s, w| s.max_width(w))
            .pointer_events_auto()
    })
}

/// How many rows a card laid out `height` tall takes: the height with the
/// inset the card sits inside its first and last rows put back, in whole
/// rows, rounded up, so the border never touches the line after the
/// card. Never fewer than one.
///
/// The height is read to a hundredth of a pixel first. It comes from the
/// layout's single-precision sum of the card's lines, whose fractional
/// heights can land one float step over a row multiple, and rounding
/// that step up would give the card a blank row under it.
fn rows_for_height(height: f64, line_height: f64, inset: f64) -> usize {
    let height = (height * 100.0).round() / 100.0;
    ((height + 2.0 * inset) / line_height).ceil().max(1.0) as usize
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
/// wheel over the left is handed to the right. The left's scrollbar is
/// hidden, so there is one.
///
/// The two documents are edited in the same pass, from the pair the
/// builder returns, so the two stay the same height: the dividers are on
/// both sides at the same rows, and a card's rows are on its anchor's
/// side with fillers beside them on the other. The layers over the outer
/// stack draw the hunk headers over the base side, where the expanders
/// are, and each side's cards over that side, from the one scroll.
#[allow(clippy::too_many_arguments)]
fn split_view(
    app: AppState,
    state: ReviewState,
    live_file: LiveFile,
    memos: Memos,
    split: (Doc, Doc),
    epoch: u64,
    backend: Rc<Backend>,
) -> impl IntoView {
    let file = live_file.get_untracked();
    let (left_text, right_text) = (split.0.text.clone(), split.1.text.clone());
    let (left_doc, right_doc) = (
        RwSignal::new(Arc::new(split.0)),
        RwSignal::new(Arc::new(split.1)),
    );
    let line_height = theme::code_line_height() as f64;

    // Distinct cache keys, in case the two ever share a layout cache.
    let left = code_editor(
        left_text,
        PerchStyling::side(
            live_file,
            Side::Base,
            left_doc,
            state.highlights,
            state.find_matches,
            state.find_current,
            epoch.wrapping_mul(2),
        ),
    )
    .style(|s| s.custom_style_class(|s: ScrollCustomStyle| s.hide_bars(true)));
    let right = code_editor(
        right_text,
        PerchStyling::side(
            live_file,
            Side::Head,
            right_doc,
            state.highlights,
            state.find_matches,
            state.find_current,
            epoch.wrapping_mul(2) + 1,
        ),
    );
    let left_live = LiveDoc::of(&left, left_doc);
    let right_live = LiveDoc::of(&right, right_doc);
    state.docs.set(Some(LiveDocs {
        epoch,
        layout: LiveLayout::Split {
            left: left_live.clone(),
            right: right_live.clone(),
        },
    }));

    let (left_viewport, left_scroll_to, left_delta, left_cursor) = (
        left.editor().viewport,
        left.editor().scroll_to,
        left.editor().scroll_delta,
        left.editor().cursor,
    );
    let (right_viewport, right_scroll_to, right_delta, right_cursor) = (
        right.editor().viewport,
        right.editor().scroll_to,
        right.editor().scroll_delta,
        right.editor().cursor,
    );
    // Where an edit last moved each cursor to.
    let (left_shifted, right_shifted) = (RwSignal::new(None), RwSignal::new(None));
    // The side whose cursor is the pane's, the one the reader last moved;
    // see `wire_cursor`. The other side's selection goes, so that there is
    // one, as on a page.
    let active = RwSignal::new(Side::Head);
    Effect::new(move |before: Option<Side>| {
        let now = active.get();
        if before.is_some_and(|before| before != now) {
            match now {
                Side::Head => deselect(left_cursor, left_shifted, left_viewport, left_scroll_to),
                Side::Base => {
                    deselect(right_cursor, right_shifted, right_viewport, right_scroll_to)
                }
            }
        }
        now
    });

    // The folds are wired before the jump, as in the stacked layout. The
    // right is edited first: it owns the scroll, and the left follows.
    wire_highlights(state, &right_live, live_file);
    wire_highlights(state, &left_live, live_file);
    // The two sides are the same width, so the head's says for both.
    let columns = wrap_columns(state, right_viewport, Layout::Split);
    wire_folds(state, live_file, memos, columns, epoch, {
        move |file: &LoadedFile, inputs: &Inputs, reloaded: bool| {
            let Some((new_left, new_right)) = inputs.split(file) else {
                return;
            };
            apply(
                &right_live,
                new_right,
                right_viewport,
                right_scroll_to,
                right_cursor,
                right_shifted,
                reloaded,
            );
            apply(
                &left_live,
                new_left,
                left_viewport,
                left_scroll_to,
                left_cursor,
                left_shifted,
                reloaded,
            );
        }
    });
    wire_cursor(
        state,
        backend.clone(),
        file.clone(),
        right_doc,
        right_cursor,
        right_shifted,
        CursorOf::Head { active },
    );
    wire_cursor(
        state,
        backend.clone(),
        file.clone(),
        left_doc,
        left_cursor,
        left_shifted,
        CursorOf::Base {
            active,
            head: right_doc,
            file: live_file,
        },
    );
    wire_place(
        state,
        file.path.clone(),
        right_doc,
        right_viewport,
        right_scroll_to,
        right_cursor,
        epoch,
    );
    wire_jump(
        state,
        live_file,
        right_doc,
        right_scroll_to,
        right_cursor,
        epoch,
    );
    wire_page(
        state,
        right_doc,
        right_viewport,
        right_delta,
        right_cursor,
        epoch,
    );
    // The base side has no symbols the server knows, so its menu is Copy
    // alone. Made navigable first, so the head side's is the last editor
    // a test reads.
    let left = navigation::navigable(left, state, backend.clone(), file.clone(), left_doc);
    let right = navigation::navigable(right, state, backend.clone(), file.clone(), right_doc);

    // The two sides scroll down the file as one. The right owns the
    // scroll and the left follows it; the left moves on its own only to
    // bring its caret into view, after a click by its edge, a drag past
    // it or a key, and then the right follows the left. When both have
    // moved since they were last together, the right's move stands.
    // Sideways each keeps its own place: the lines on the two sides are
    // not the same length.
    Effect::new(move |together: Option<f64>| {
        let (left, right) = (left_viewport.get(), right_viewport.get());
        if left.y0 == right.y0 {
            return right.y0;
        }
        match together {
            Some(y) if y == right.y0 => {
                right_scroll_to.set(Some(Vec2::new(right.x0, left.y0)));
            }
            _ => left_scroll_to.set(Some(Vec2::new(left.x0, right.y0))),
        }
        together.unwrap_or(right.y0)
    });
    #[cfg(test)]
    SPLIT_VIEWPORTS.with(|v| v.set(Some((left_viewport, right_viewport))));
    #[cfg(test)]
    SPLIT_CURSORS.with(|c| c.set(Some((left_cursor, right_cursor))));

    // The head pins its blocks; the base is no file the server has,
    // except for a deleted file, which has no head and pins the base.
    // The right owns the scroll down the file either way.
    let pins_base = sticky::pins_base(&file);
    let side_sticky = sticky::layer(
        state,
        backend.clone(),
        live_file,
        if pins_base { left_doc } else { right_doc },
        if pins_base {
            left_viewport
        } else {
            right_viewport
        },
        gutter::width_of(&file),
        right_delta,
        pins_base.then_some(left_delta),
        Rc::new(move |y| {
            right_scroll_to.set(Some(Vec2::new(right_viewport.get_untracked().x0, y)))
        }),
    );

    let left_pane = Stack::horizontal((
        side_gutter(
            state,
            app,
            live_file,
            Side::Base,
            left_doc,
            left_viewport,
            right_delta,
        ),
        // Caught on the way down, before the left's own scroll takes it.
        Container::new(over_tints(left, left_doc, left_viewport))
            .on_event_with_config(
                el::PointerWheel,
                floem::context::EventCallbackConfig {
                    phases: floem::context::Phases::CAPTURE,
                },
                move |_, wheel| {
                    let size = left_viewport.get_untracked().size();
                    let delta = wheel
                        .resolve_to_points(Some(Size::new(line_height, line_height)), Some(size));
                    // Down the file is the right's; across is the left's own.
                    right_delta.set(Vec2::new(0.0, -delta.y));
                    left_delta.set(Vec2::new(-delta.x, 0.0));
                    floem::event::EventPropagation::Stop
                },
            )
            .style(|s| s.flex_grow(1.0_f32).min_width(0.0).height_full()),
    ))
    .style(|s| {
        s.flex_basis(0.0)
            .flex_grow(1.0_f32)
            .min_width(0.0)
            .height_full()
    });

    let gutter_width = gutter::width_of(&file);
    let right_pane = Stack::horizontal((
        side_gutter(
            state,
            app,
            live_file,
            Side::Head,
            right_doc,
            right_viewport,
            right_delta,
        ),
        over_tints(right, right_doc, right_viewport),
    ))
    .style(|s| {
        s.flex_basis(0.0)
            .flex_grow(1.0_f32)
            .min_width(0.0)
            .height_full()
    });

    let half = |s: floem::style::Style| s.flex_basis(0.0).flex_grow(1.0_f32).min_width(0.0);
    let rule = || Empty::new().style(|s| s.width(1.0).height_full().flex_shrink(0.0_f32));

    // Each side's cards over its half of the outer stack, from its first
    // code column, past its gutter, to its edge. Both follow the right,
    // which owns the scroll; a wheel across a card on the left moves the
    // left, as one over its code does.
    let cards = |doc, across| {
        let layer = card_layer(
            app,
            state,
            backend.clone(),
            file.path.clone(),
            doc,
            right_viewport,
            right_delta,
            across,
            right_scroll_to,
            gutter_width + theme::CODE_GUTTER_GAP,
            None,
        );
        Container::new(layer).style(move |s| half(s).height_full().position(Position::Relative))
    };
    let cards = Stack::horizontal((
        cards(left_doc, Some(left_delta)),
        rule(),
        cards(right_doc, None),
    ))
    .style(|s| s.absolute().inset(0.0).size_full().pointer_events_none());

    // The pinned lines, over the dividers and the cards, over the half
    // of the outer stack whose lines they are: the right, past the left
    // and the rule, or the left for a deleted file.
    let pinned = Container::new(side_sticky)
        .style(move |s| half(s).height_full().position(Position::Relative));
    let other = Empty::new().style(half);
    let sticky = if pins_base {
        Stack::horizontal((pinned.into_any(), rule(), other.into_any()))
    } else {
        Stack::horizontal((other.into_any(), rule(), pinned.into_any()))
    }
    .style(|s| s.absolute().inset(0.0).size_full().pointer_events_none());

    // The layers are placed against the outer stack, so it takes the
    // relative position; a bar spans the whole row, its header past the
    // base side's gutter, and a card is over its own side.
    Stack::horizontal((
        left_pane,
        Empty::new().style(|s| {
            s.width(1.0)
                .height_full()
                .flex_shrink(0.0_f32)
                .background(theme::border())
        }),
        right_pane,
        divider_layer(
            state,
            live_file,
            right_doc,
            right_viewport,
            right_delta,
            Bars::Split {
                base_delta: left_delta,
            },
        ),
        cards,
        sticky,
    ))
    .style(|s| s.size_full().min_height(0.0).position(Position::Relative))
}

/// Take a split side's selection away, leaving its caret where it is: the
/// other side has the reader's. Marked as an edit's move, so the side's
/// cursor effect does not take it for the reader's and take the pane
/// back; and the scroll asked for after it, which the editor does last,
/// keeps the editor from bringing the caret into view, as in `apply`.
fn deselect(
    cursor: RwSignal<Cursor>,
    shifted: RwSignal<Option<usize>>,
    viewport: RwSignal<Rect>,
    scroll_to: RwSignal<Option<Vec2>>,
) {
    if !cursor.with_untracked(|c| c.regions_iter().any(|(a, b, _)| a != b)) {
        return;
    }
    let offset = cursor.with_untracked(|c| c.offset());
    shifted.set(Some(offset));
    cursor.update(|c| {
        let affinity = c.affinity();
        c.set_offset(offset, affinity, false, false);
    });
    let vp = viewport.get_untracked();
    scroll_to.set(Some(Vec2::new(vp.x0, vp.y0)));
}

#[cfg(test)]
thread_local! {
    /// The viewports of the last split built, left and right, so a test can
    /// watch the two scroll together.
    static SPLIT_VIEWPORTS: std::cell::Cell<Option<(RwSignal<Rect>, RwSignal<Rect>)>> =
        const { std::cell::Cell::new(None) };
    /// The cursors of the last split built, left and right, so a test can
    /// see which side has a selection.
    static SPLIT_CURSORS: std::cell::Cell<Option<(RwSignal<Cursor>, RwSignal<Cursor>)>> =
        const { std::cell::Cell::new(None) };
    /// The viewport of the last stacked editor built, so a test can see
    /// that an edit left it where it was.
    static STACKED_VIEWPORT: std::cell::Cell<Option<RwSignal<Rect>>> = const { std::cell::Cell::new(None) };
    /// The cursor of the last stacked editor built, so a test can place it
    /// where a click would without working out where the click goes.
    static STACKED_CURSOR: std::cell::Cell<Option<RwSignal<Cursor>>> = const { std::cell::Cell::new(None) };
}

/// A read-only editor over one document, drawn as code.
///
/// The editor fills its box: floem sizes the scroll inside it to the
/// box's padding box, so padding here would push the scroll, and the
/// vertical bar at its right edge, out past the box. The gap before the
/// code is `over_tints`'s.
fn code_editor(text: String, styling: PerchStyling) -> TextEditor {
    text_editor(text)
        .read_only()
        .styling(styling)
        .editor_style(|s| {
            // The editor's own wrapping would put a long line on several
            // visual rows, which breaks the gutter's one-row-per-line
            // geometry and a split's rows beside each other. Long lines
            // scroll sideways instead, or Word wrap breaks them in the
            // document, a row each; see `doc`.
            s.hide_gutter(true)
                .wrap_method(floem::views::editor::text::WrapMethod::None)
                .selection_color(theme::selection_bg())
        })
        // No background: the box the editor sits in paints the code's,
        // and the rows' tints over it, under the editor. See `over_tints`.
        .style(|s| s.size_full().min_width(0.0))
}

/// Which editor's cursor `wire_cursor` reads.
#[derive(Clone, Copy)]
enum CursorOf {
    /// The stacked layout's one editor.
    Stacked,
    /// A split's head side. `active` is the side whose cursor is the
    /// pane's; see `wire_cursor`.
    Head { active: RwSignal<Side> },
    /// A split's base side, whose rows are base lines: a line is where
    /// the stacked layout shows it, its ghost or the head line it is, and
    /// a filler is the line the head added beside it, on `head`.
    Base {
        active: RwSignal<Side>,
        head: RwSignal<Arc<Doc>>,
        file: LiveFile,
    },
}

/// The cursor's line is kept, the selection becomes the
/// lines a thread would be anchored on, and the head position under the
/// caret is kept for the keys that act on the symbol there. Reading the
/// editor's own cursor rather than the pointer means floem's hit-testing
/// does the coordinate work. The document is the one the editor that owns
/// the cursor is laying out: the stacked one, or either side of a split.
/// It is read as it is when the cursor moves, not as it was when the
/// editor was built, so a row is right after the document has been edited.
///
/// A split has a cursor on each side, and the one the reader last moved,
/// by a click, a key or a jump, is the pane's: it takes `active` and is
/// the one read, and the other side's waits until the reader moves it.
/// The base side has no head position, as a ghost in the stacked layout
/// has none, and its line and selection are the stacked layout's.
///
/// The caret resting on an identifier tints its other uses in the file:
/// after a moment still, the server is asked for them, and the caret
/// moving off the identifier clears them.
///
/// An edit above the cursor moves its offset to keep it on its text, and
/// `apply` leaves the offset it moved it to in `shifted`. That move is no
/// click: the line and the selection are read again, since their rows may
/// have moved, but the symbol under the cursor is the one it was on, so
/// its position stands as it was.
fn wire_cursor(
    state: ReviewState,
    backend: Rc<Backend>,
    file: Arc<LoadedFile>,
    doc: RwSignal<Arc<Doc>>,
    cursor: RwSignal<Cursor>,
    shifted: RwSignal<Option<usize>>,
    of: CursorOf,
) {
    Effect::new(move |previous: Option<usize>| {
        let offset = cursor.with(|c| c.offset());
        // Taken whichever way this run goes, so that a later click landing
        // on the same offset is a click.
        let by_edit = shifted.try_update(|s| s.take()).flatten() == Some(offset);
        let side = match of {
            CursorOf::Stacked => None,
            CursorOf::Head { active } => Some((Side::Head, active)),
            CursorOf::Base { active, .. } => Some((Side::Base, active)),
        };
        let mut taken = false;
        if let Some((side, active)) = side {
            // The cursor placed as the editor is built is no reader's
            // move, and nor is one an edit made.
            if previous.is_some() && !by_edit && active.get_untracked() != side {
                active.set(side);
                taken = true;
            }
            if active.get_untracked() != side {
                return offset;
            }
        }
        // A side taking the pane says its line and selection, wherever
        // its caret is.
        if previous == Some(offset) && !taken {
            return offset;
        }
        let doc = doc.get_untracked();
        let base = match of {
            CursorOf::Base { head, file, .. } => Some((head.get_untracked(), file.get_untracked())),
            CursorOf::Stacked | CursorOf::Head { .. } => None,
        };
        let display_line = |row: usize| match &base {
            Some((head, file)) => match doc.display_line(row) {
                Some(line) => display_line_of_base_line(file, line),
                None => head.display_line(row),
            },
            None => doc.display_line(row),
        };

        let (row, column) = doc.locate(offset);
        if let Some(line) = doc.nearest(row, display_line) {
            state.cursor_line.set(line);
        }

        // The selection, as display lines, for a thread's anchor. A caret
        // is a one-line selection. Rows that are no line at all fall out,
        // and a selection of nothing but those is no selection.
        let (lo, hi) = cursor.with(|c| {
            c.mode
                .regions_iter()
                .fold((usize::MAX, 0usize), |(lo, hi), (a, b, _)| {
                    (lo.min(a.min(b)), hi.max(a.max(b)))
                })
        });
        if lo != usize::MAX {
            let (first_row, _) = doc.locate(lo);
            let (last_row, _) = doc.locate(hi);
            let first = (first_row..=last_row).find_map(display_line);
            let last = (first_row..=last_row).rev().find_map(display_line);
            state.selection.set(first.zip(last));
            // On the base side the selection is of base lines too, which a
            // question asked from there is anchored on.
            let on_base = base.as_ref().and_then(|_| {
                let first = (first_row..=last_row).find_map(|r| doc.display_line(r))?;
                let last = (first_row..=last_row)
                    .rev()
                    .find_map(|r| doc.display_line(r))?;
                Some((first, last))
            });
            state.selection_base.set(on_base);
        }

        if by_edit {
            return offset;
        }
        let position = doc.head_position(offset);
        state.last_position.set(position);

        // The identifier at the caret, by the caret's reading: just past
        // a word is still on it. The tint stays while the caret stays on
        // the symbol it is for, wherever in the word it is.
        let symbol = position.and_then(|(line, _)| {
            let source = doc.source_line(row);
            let (start, text) = references::identifier_at(&source, column)?;
            Some(crate::review::model::Symbol {
                path: file.path.clone(),
                line,
                column: start,
                end: start + text.len(),
                text: text.to_string(),
            })
        });
        let same = state
            .highlight_ask
            .with_untracked(|a| a.as_ref().map(|(_, s)| s) == symbol.as_ref());
        if same {
            return offset;
        }
        state.clear_highlights();
        if let Some(symbol) = symbol {
            let backend = backend.clone();
            floem::action::exec_after(
                std::time::Duration::from_millis(hover::ASK_DELAY_MILLIS),
                move |_| {
                    if state.last_position.with_untracked(|p| *p == position) {
                        state.ask_highlights(&backend, symbol);
                    }
                },
            );
        }
        offset
    });
}

/// The editor lays its lines out through the styling, and caches them;
/// when the tint changes, the symbol's or the find bar's, or a reload of
/// the file is set in place with its colours, the cached layouts are
/// stale, and are dropped so the lines in view are laid out again.
fn wire_highlights(state: ReviewState, live: &LiveDoc, file: LiveFile) {
    let handle = live.handle.clone();
    Effect::new(move |before: Option<()>| {
        // A reload set in place may colour the same text anew.
        file.track();
        state.highlights.track();
        state.find_matches.track();
        state.find_current.track();
        // The layouts built with the editor are right; only a change
        // after that makes them stale.
        if before.is_some() {
            handle.cache_rev().update(|rev| *rev += 1);
        }
    });
}

/// Puts the reader back where they were in the file as its editor is
/// built, and keeps their place as the editor scrolls and the caret
/// moves; see `FilePlace`. A jump waiting for this editor goes where it
/// was asked to instead. The place is kept only while this editor is the
/// one showing: an editor on its way out has no place to keep.
fn wire_place(
    state: ReviewState,
    path: String,
    doc: RwSignal<Arc<Doc>>,
    viewport: RwSignal<Rect>,
    scroll_to: RwSignal<Option<Vec2>>,
    cursor: RwSignal<Cursor>,
    epoch: u64,
) {
    let place = state
        .file_places
        .with_untracked(|places| places.get(&path).copied());
    if let Some(place) = place.filter(|_| state.jump_to_line.get_untracked().is_none()) {
        doc.with_untracked(|doc| restore_place(&place, doc, scroll_to, cursor));
    }
    Effect::new(move |_| {
        let viewport = viewport.get();
        let caret = cursor.with(|c| c.offset());
        if state.editor_epoch.get_untracked() != epoch {
            return;
        }
        let place = doc.with(|doc| place_of(doc, viewport, caret));
        state.file_places.update(|places| {
            places.insert(path.clone(), place);
        });
    });
}

/// Where the reader is in a document: see `FilePlace`.
fn place_of(doc: &Doc, viewport: Rect, caret: usize) -> FilePlace {
    let line_height = theme::code_line_height() as f64;
    let top = (viewport.y0 / line_height).floor().max(0.0) as usize;
    // From the row the line starts on, so a line wrapped at another
    // width when the place is put back is still where it was.
    let anchor = (top..doc.len()).find_map(|row| {
        let line = doc.display_line(row)?;
        Some((line, viewport.y0 - doc.lead(row) as f64 * line_height))
    });
    let (row, column) = doc.locate(caret);
    FilePlace {
        anchor,
        x: viewport.x0,
        y: viewport.y0,
        caret: doc.display_line(row).map(|line| (line, column)),
    }
}

/// Put the caret back on its line, and the view where it was: its line
/// as far from the top as it was, or the same pixels when the document
/// no longer shows the line. The caret goes first, since moving it asks
/// the editor to bring it into view, and the scroll, asked for after
/// that, is what the editor does last.
fn restore_place(
    place: &FilePlace,
    doc: &Doc,
    scroll_to: RwSignal<Option<Vec2>>,
    cursor: RwSignal<Cursor>,
) {
    let line_height = theme::code_line_height() as f64;
    // The line's text may have changed under the column since, which
    // `offset_at` clamps to.
    let caret = place
        .caret
        .and_then(|(line, column)| Some(doc.offset_at(doc.row_of_display(line)?, column)));
    if let Some(offset) = caret {
        cursor.update(|c| c.set_offset(offset, CursorAffinity::Backward, false, false));
    }
    let y = place
        .anchor
        .and_then(|(line, into)| {
            Some(doc.row_at_or_after_display(line)? as f64 * line_height + into)
        })
        .unwrap_or(place.y);
    scroll_to.set(Some(Vec2::new(place.x, y.max(0.0))));
}

/// A jump opens its file, then asks to be scrolled to. The head line has
/// to be translated through the row table first, because the spliced diff
/// shifts everything below a removal, and a split's fillers do the same.
/// A line inside a fold is on no row: the fold is revealed around it
/// first, like any other reveal, and Reset undoes it like any other. A
/// navigation's jump also lands the caret on the line, so the next jump
/// remembers leaving from there.
fn wire_jump(
    state: ReviewState,
    file: LiveFile,
    doc: RwSignal<Arc<Doc>>,
    scroll_to: RwSignal<Option<Vec2>>,
    cursor: RwSignal<Cursor>,
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
        if row.is_none() {
            state.reveal_around(&file.get_untracked(), head_line);
            row = doc.with_untracked(|d| d.row_of_head(head_line));
        }
        // A line the file does not have lands at the end of it.
        let row = row.unwrap_or_else(|| doc.with_untracked(|d| d.len().saturating_sub(1)));
        // Land the target a few lines below the top rather than flush
        // against it, so its context is visible.
        let target = row.saturating_sub(4) as f64;
        scroll_to.set(Some(Vec2::new(
            0.0,
            target * theme::code_line_height() as f64,
        )));
        let caret = state
            .jump_caret
            .try_update(|c| c.take_if(|(line, _)| *line == head_line))
            .flatten();
        if let Some((_, column)) = caret {
            let offset = doc.with_untracked(|d| d.offset_at(row, column));
            cursor.update(|c| c.set_offset(offset, CursorAffinity::Backward, false, false));
        }
    });
}

/// Page Up and Page Down scroll by a page of whole lines, less one kept
/// in sight so the reader keeps their place, and leave the caret where it
/// is, as a Mac's text views do. Cmd-Up and Cmd-Down put the caret at the
/// start of the first line or of the last, and the view flush with that
/// end of the file. The caret moves first: bringing it into view may leave
/// it mid-screen, and the scroll after it puts the end against the edge.
/// By a delta rather than to a point, past either end for Cmd-Up and
/// Cmd-Down, which the scroll clamps: a delta shows the bars that hide
/// while idle, as the wheel does.
fn wire_page(
    state: ReviewState,
    doc: RwSignal<Arc<Doc>>,
    viewport: RwSignal<Rect>,
    scroll_delta: RwSignal<Vec2>,
    cursor: RwSignal<Cursor>,
    epoch: u64,
) {
    Effect::new(move |_| {
        let Some(paging) = state.paging.get() else {
            return;
        };
        // As with a jump: a press as the editor is rebuilt is the next
        // editor's.
        if state.editor_epoch.get_untracked() != epoch {
            return;
        }
        state.paging.set(None);
        let line_height = theme::code_line_height() as f64;
        let page = || {
            let lines = (viewport.get_untracked().height() / line_height).floor() - 1.0;
            lines.max(1.0) * line_height
        };
        let end = match paging {
            Paging::Up => return scroll_delta.set(Vec2::new(0.0, -page())),
            Paging::Down => return scroll_delta.set(Vec2::new(0.0, page())),
            Paging::Top => false,
            Paging::End => true,
        };
        let offset = doc.with_untracked(|d| {
            // A line of the file, not a divider or a card's spacer.
            let mut rows = (0..d.len()).filter(|&r| d.display_line(r).is_some());
            let row = if end { rows.next_back() } else { rows.next() };
            row.map(|r| d.offset_of_row(d.lead(r)))
        });
        if let Some(offset) = offset {
            cursor.update(|c| c.set_offset(offset, CursorAffinity::Backward, false, false));
        }
        // Back to the left edge as well, as the start of a line is.
        let y = if end { f64::MAX } else { -f64::MAX };
        scroll_delta.set(Vec2::new(-f64::MAX, y));
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

/// How long a tab says "Opening…" before its file is asked for again.
const ASK_AGAIN_AFTER: std::time::Duration = std::time::Duration::from_secs(3);

/// Ask again for a file still opening after `wait`, and again after twice
/// as long, while its tab shows "Opening…". Every gate between the ask and
/// the answer drops what is not for the view now open, and one dropped
/// left the tab opening for good; a second answer is harmless. A load
/// that failed is not loading, and is not asked for again.
fn ask_again_while_opening(
    state: ReviewState,
    backend: Rc<Backend>,
    path: String,
    epoch: u64,
    wait: std::time::Duration,
) {
    floem::action::exec_after(wait, move |_| {
        if !state.still_opening(&path, epoch) {
            return;
        }
        tracing::info!(path, waited = ?wait, "file still opening; asked again");
        crate::review::load_file(&backend, &path);
        let next = (wait * 2).min(std::time::Duration::from_secs(60));
        ask_again_while_opening(state, backend, path, epoch, next);
    });
}

fn loading_view(path: String) -> impl IntoView {
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
    Container::new(
        Label::derived(move || format!("Opening {name}…"))
            .style(|s| s.font_size(theme::TEXT_BODY).color(theme::fg_muted())),
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
        split_doc(file, false, &[], &Reveals::default(), &[], None)
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
        assert!(
            split_of(&added).is_none(),
            "an added file has nothing on the left"
        );

        let plain = crate::backend::build_loaded_file(
            "lib.rs".into(),
            "fn a() {}\n".into(),
            String::new(),
            None,
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        assert!(
            split_of(&plain).is_none(),
            "a file outside the changed list is shown plainly"
        );

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
        assert_eq!(
            right.nearest_display_line(3),
            Some(3),
            "the row above, c, when nothing follows"
        );
        assert_eq!(right.row_of_head(2), Some(2));
        // And the stacked document over the same file is the buffer.
        let stacked = doc::build(&file.buffer, &[], &[], None);
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
        assert_eq!(
            doc::runs(&plain.buffer, &anchors).len(),
            2,
            "the runs are there to fold"
        );
        let doc = stacked_doc(&plain, true, &anchors, &Reveals::default(), &[], None);
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
        let doc = stacked_doc(&changed, true, &anchors, &Reveals::default(), &[], None);
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
        assert_eq!(
            scroll_for_card(vp, top, bottom, lh),
            Some(0.0),
            "never above the file"
        );
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
        // The layout sums the card's lines in single precision, so a card
        // that is five rows can arrive one float step over it: still five,
        // and eleven for one step over eleven rows.
        let step_over = |px: f64| f64::from(f32::from_bits((px as f32).to_bits() + 1));
        assert!(step_over(five) > five);
        assert_eq!(rows_for_height(step_over(five), lh, inset), 5);
        assert_eq!(
            rows_for_height(step_over(11.0 * lh - 2.0 * inset), lh, inset),
            11
        );
        assert_eq!(
            rows_for_height(five + 0.01, lh, inset),
            6,
            "a hundredth of a pixel is a real height"
        );
        // Never fewer than one, however little is there.
        assert_eq!(rows_for_height(0.0, lh, inset), 1);
        assert_eq!(rows_for_height(lh - 2.0 * inset, lh, inset), 1);
        // Nothing caps a tall card.
        assert_eq!(rows_for_height(140.0 * lh - 2.0 * inset, lh, inset), 140);
    }
}

/// The split in a headless window.
#[cfg(test)]
mod window {
    use std::rc::Rc;
    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::peniko::Brush;
    use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
    use floem::views::editor::core::cursor::{Cursor, CursorAffinity};
    use perch_github::{Item, ItemCheckout};

    use crate::review::model::{ItemTab, Layout, LiveDoc, LiveLayout, Phase, ReviewState};
    use crate::state::AppState;

    /// How wide the gutters are here: every file these tests open numbers
    /// fewer than a thousand lines, which is the fewest digits a gutter
    /// makes room for.
    fn gutter_width() -> f64 {
        crate::review::gutter::width(3)
    }

    /// A changed file, its gutter and tints, all of it dark once the system is.
    #[test]
    fn goes_dark_with_the_system() {
        let mut harness = harness().0;
        let left = crate::theme::dark_check::light_left(&mut harness);
        assert!(
            left.is_empty(),
            "light colours left in dark:\n{}",
            left.join("\n")
        );
    }

    /// The whole window on Code with one changed file open, so the split
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
        harness_on(head, base, crate::backend::Backend::detached())
    }

    /// The same, over a backend of the test's, such as a recording one.
    fn harness_on(
        head: String,
        base: String,
        backend: crate::backend::Backend,
    ) -> (HeadlessHarness, ReviewState) {
        harness_as(head, base, perch_git::ChangeStatus::Modified, backend)
    }

    /// The same, with the file's status given.
    fn harness_as(
        head: String,
        base: String,
        status: perch_git::ChangeStatus,
        backend: crate::backend::Backend,
    ) -> (HeadlessHarness, ReviewState) {
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
            divergence: None,
            source: None,
        });
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state.set_phase(Phase::Ready);
        state.tab.set(ItemTab::Code);
        // See the field: a focused editor cannot be driven headlessly.
        state.composer_autofocus.set(false);

        let file = Arc::new(crate::backend::build_loaded_file(
            "src/x.rs".into(),
            head,
            base,
            Some(status),
            false,
            perch_git::Scope::Branch,
            String::new(),
        ));
        state.open_tab("src/x.rs");
        state.finish_load("src/x.rs", file);

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
        assert!(
            state.jump_to_line.get().is_none(),
            "the split's right editor took the jump"
        );

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
        let docs = state
            .docs
            .get()
            .expect("the stacked editor registered its document");
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
        let docs = state
            .docs
            .get()
            .expect("the split registered both documents");
        assert_eq!(docs.epoch, state.editor_epoch.get());
        let LiveLayout::Split { left, right } = docs.layout else {
            panic!("the split layout holds two documents");
        };
        assert_eq!(
            left.doc.get().len(),
            right.doc.get().len(),
            "the two sides are the same height"
        );
        assert_eq!(right.doc.get().len(), 90);
        assert_eq!(laid_out_text(&left), left.doc.get().text);
        assert_eq!(laid_out_text(&right), right.doc.get().text);

        // Closing the file takes the documents with it.
        state.close_tab("src/x.rs");
        harness.rebuild();
        harness.paint();
        assert!(state.docs.get().is_none(), "no editor, no document");
    }

    /// Where the two editors sit: after the open drawer on the left, the
    /// rest of the window in halves, under the header's bar and strip and
    /// the file tabs. The drawer on the right is shut.
    fn pane_centers() -> (f64, f64, f64) {
        let left_edge = crate::theme::SIDEBAR_WIDTH;
        let half = (1400.0 - left_edge) / 2.0;
        let y = pane_origin().1 + 300.0;
        (left_edge + half * 0.6, left_edge + half * 1.6, y)
    }

    #[test]
    fn the_two_sides_scroll_as_one() {
        let (mut harness, state) = harness();
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let (left, right) = super::SPLIT_VIEWPORTS
            .with(|v| v.get())
            .expect("a split was built");
        let (x_left, x_right, y) = pane_centers();

        // The wheel over the right moves the right, and the left follows.
        harness.scroll_down(x_right, y, 200.0);
        harness.rebuild();
        harness.paint();
        assert!(right.get().y0 > 0.0, "the right did not scroll");
        assert_eq!(left.get().y0, right.get().y0, "the left did not follow");

        // The wheel over the left is caught before the left's own scroll
        // takes it and handed to the right, and the left follows that too.
        let before = right.get().y0;
        harness.scroll_down(x_left, y, 100.0);
        harness.rebuild();
        harness.paint();
        assert!(
            right.get().y0 > before,
            "a wheel over the left did not scroll the right"
        );
        assert_eq!(left.get().y0, right.get().y0);

        // And a jump lands both on the same row. Line 50 rather than one
        // nearer the end, so the scroll it asks for is short of the
        // document's bottom, whatever the header above the pane is.
        state.jump_to_line.set(Some(50));
        harness.rebuild();
        harness.paint();
        assert_eq!(left.get().y0, right.get().y0);
        assert_eq!(
            right.get().y0,
            (50.0 - 4.0) * crate::theme::code_line_height() as f64
        );
    }

    /// Scrolled into a function, the server's outline pins its first
    /// line and its impl's over the top of the code, the impl's first,
    /// each with its number over the gutter and its text from the first
    /// code column; a click on the function's scrolls it back to where it
    /// is pinned. The outline is asked for when the server comes up, and
    /// one counted in other lines than the head's pins nothing. Split,
    /// only the head pins.
    #[test]
    fn the_blocks_the_top_of_the_code_is_in_are_pinned_over_it() {
        use crate::review::model::Outline;
        use crate::review::sticky::lines_hash;

        // An impl on line 0 and its function on line 1, longer than the
        // pane and changed on every third line so that nothing folds; the
        // function's closing brace is line 122 and the impl's 123.
        let file = |changed: &str| {
            let body: String = (0..120)
                .map(|i| {
                    format!(
                        "        let x{i} = {};\n",
                        if i % 3 == 0 { changed } else { "0" }
                    )
                })
                .collect();
            format!("impl Default for Options {{\n    fn default() -> Self {{\n{body}    }}\n}}\n")
        };
        let head = file("1");
        let (backend, mut commands) = crate::backend::Backend::recording();
        let (mut harness, state) = harness_on(head.clone(), file("2"), backend);
        let line_height = crate::theme::code_line_height() as f64;
        let (left, top) = pane_origin();

        let mut asked = || {
            std::iter::from_fn(|| commands.try_recv().ok())
                .filter(|c| matches!(c, crate::backend::Command::DocumentSymbols { path, .. } if path == "src/x.rs"))
                .count()
        };
        assert_eq!(asked(), 0, "nothing is asked of a server that is not there");
        state.set_server_status(
            perch_lsp::ServerKind::RustAnalyzer,
            perch_lsp::Status::Ready,
        );
        harness.rebuild();
        assert_eq!(
            asked(),
            1,
            "the outline is asked for when the server comes up"
        );

        let outline = |lines| {
            let mut symbols = std::collections::HashMap::new();
            symbols.insert(
                "src/x.rs".to_string(),
                Arc::new(Outline {
                    lines,
                    blocks: vec![(0, 123), (1, 122)],
                }),
            );
            symbols
        };
        state.symbols.set(outline(lines_hash(head.lines())));
        let viewport = super::STACKED_VIEWPORT
            .with(|v| v.get())
            .expect("a stacked editor");
        harness.rebuild();
        harness.paint();
        assert_eq!(
            count(harness.root_id(), "sticky:"),
            0,
            "nothing is pinned at the top"
        );

        harness.scroll_down(800.0, 500.0, 20.0 * line_height);
        harness.rebuild();
        harness.paint();
        assert!(
            viewport.get().y0 > 10.0 * line_height,
            "the code did not scroll"
        );
        let root = harness.root_id();
        let (outer, inner) = (rect_of(root, "sticky:0"), rect_of(root, "sticky:1"));
        assert_eq!(
            (outer.x0, outer.y0, outer.height()),
            (left, top, line_height)
        );
        assert_eq!((inner.x0, inner.y0), (left, top + line_height));
        assert_eq!(count(root, "sticky:"), 2);
        let text = rect_of(root, "impl Default for Options {");
        assert_eq!(
            text.x0,
            left + gutter_width() + crate::theme::CODE_GUTTER_GAP
        );
        assert!(find(root, "fn default() -> Self {").is_some());

        // An outline of other text: a commit's head, say.
        state
            .symbols
            .set(outline(lines_hash("fn other() {}".lines())));
        harness.rebuild();
        harness.paint();
        assert_eq!(count(harness.root_id(), "sticky:"), 0);
        state.symbols.set(outline(lines_hash(head.lines())));
        harness.rebuild();
        harness.paint();

        // A reload edited in place, with its new outline: the pinned line
        // is the new text, and the old outline no longer fits.
        let epoch = state.editor_epoch.get();
        let edited = head.replacen("fn default() -> Self {", "fn default() -> Options {", 1);
        state.finish_load(
            "src/x.rs",
            Arc::new(crate::backend::build_loaded_file(
                "src/x.rs".into(),
                edited.clone(),
                file("2"),
                Some(perch_git::ChangeStatus::Modified),
                false,
                perch_git::Scope::Branch,
                String::new(),
            )),
        );
        harness.rebuild();
        harness.paint();
        assert_eq!(state.editor_epoch.get(), epoch, "edited in place");
        assert_eq!(
            count(harness.root_id(), "sticky:"),
            0,
            "the old outline is of other text"
        );
        state.symbols.set(outline(lines_hash(edited.lines())));
        harness.rebuild();
        harness.paint();
        let root = harness.root_id();
        assert!(find(root, "fn default() -> Options {").is_some());
        assert!(find(root, "fn default() -> Self {").is_none());

        let inner = rect_of(harness.root_id(), "sticky:1");
        harness.click(inner.center().x, inner.center().y);
        harness.rebuild();
        harness.paint();
        // The function's first line is scrolled to its slot, one line
        // down, with the impl's pinned over it. Changed, it is under the
        // removed lines the reload put above it.
        let row = stacked(state).doc.get().row_of_head(1).unwrap();
        assert_eq!(viewport.get().y0, (row - 1) as f64 * line_height);
        let root = harness.root_id();
        assert_eq!(count(root, "sticky:"), 1);
        assert!(find(root, "sticky:0").is_some());

        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let (_, x_right, y) = pane_centers();
        harness.scroll_down(x_right, y, 20.0 * line_height);
        harness.rebuild();
        harness.paint();
        let root = harness.root_id();
        assert_eq!(count(root, "sticky:0"), 1, "the head's alone");
        let pinned = rect_of(root, "sticky:0");
        let middle = left + (1400.0 - left - 1.0) / 2.0;
        assert!(pinned.x0 > middle, "over the head side: {pinned:?}");
    }

    /// A deleted file has no head, so it pins its removed lines' blocks:
    /// their outline is asked for with their text, since the file is on no
    /// disk, and the pinned lines are drawn from the base. Split, they are
    /// over the base side.
    #[test]
    fn a_deleted_file_pins_the_blocks_of_its_removed_lines() {
        use crate::review::model::Outline;
        use crate::review::sticky::lines_hash;

        let body: String = (0..120)
            .map(|i| format!("        let x{i} = 0;\n"))
            .collect();
        let base =
            format!("impl Default for Options {{\n    fn default() -> Self {{\n{body}    }}\n}}\n");
        let (backend, mut commands) = crate::backend::Backend::recording();
        let (mut harness, state) = harness_as(
            String::new(),
            base.clone(),
            perch_git::ChangeStatus::Deleted,
            backend,
        );
        let line_height = crate::theme::code_line_height() as f64;
        let (left, top) = pane_origin();

        let mut asked = || {
            std::iter::from_fn(|| commands.try_recv().ok())
                .filter_map(|c| match c {
                    crate::backend::Command::RemovedSymbols { path, text }
                        if path == "src/x.rs" =>
                    {
                        Some(text)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert!(
            asked().is_empty(),
            "nothing is asked of a server that is not there"
        );
        state.set_server_status(
            perch_lsp::ServerKind::RustAnalyzer,
            perch_lsp::Status::Ready,
        );
        harness.rebuild();
        assert_eq!(
            asked(),
            vec![base.clone()],
            "the removed lines are handed over"
        );

        let mut symbols = std::collections::HashMap::new();
        symbols.insert(
            "src/x.rs".to_string(),
            Arc::new(Outline {
                lines: lines_hash(base.lines()),
                blocks: vec![(0, 123), (1, 122)],
            }),
        );
        state.symbols.set(symbols);
        harness.rebuild();
        harness.paint();
        assert_eq!(
            count(harness.root_id(), "sticky:"),
            0,
            "nothing is pinned at the top"
        );

        harness.scroll_down(800.0, 500.0, 20.0 * line_height);
        harness.rebuild();
        harness.paint();
        let root = harness.root_id();
        let (outer, inner) = (rect_of(root, "sticky:0"), rect_of(root, "sticky:1"));
        assert_eq!(
            (outer.x0, outer.y0, outer.height()),
            (left, top, line_height)
        );
        assert_eq!((inner.x0, inner.y0), (left, top + line_height));
        assert!(find(root, "impl Default for Options {").is_some());
        assert!(find(root, "fn default() -> Self {").is_some());

        // Rebuilt, with the outline in hand, it is not asked for again.
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        assert!(asked().is_empty());
        let (_, x_right, y) = pane_centers();
        harness.scroll_down(x_right, y, 20.0 * line_height);
        harness.rebuild();
        harness.paint();
        let root = harness.root_id();
        assert_eq!(count(root, "sticky:"), 2);
        let pinned = rect_of(root, "sticky:0");
        let middle = left + (1400.0 - left - 1.0) / 2.0;
        assert_eq!(pinned.x0, left, "over the base side: {pinned:?}");
        assert!(pinned.x1 < middle + 1.0, "over the base side: {pinned:?}");
    }

    /// A press, a move and a release with the primary button.
    fn drag(harness: &mut HeadlessHarness, from: (f64, f64), to: (f64, f64)) {
        harness.dispatch_event(button(from.0, from.1, 1, true));
        harness.pointer_move(to.0, to.1);
        harness.dispatch_event(button(to.0, to.1, 1, false));
        harness.rebuild();
        harness.paint();
    }

    /// A drag on the left selects there, as one on the right does. The
    /// selection is in the lines the stacked layout shows, the ghost of the
    /// removed line among them, and the right lets its own go, so there is
    /// one, without scrolling back to it. A click on the right takes the
    /// pane back, and the left lets go the same way.
    #[test]
    fn a_drag_on_the_left_selects_there_and_the_right_lets_go() {
        let (mut harness, state) = harness();
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let (left_cursor, right_cursor) = super::SPLIT_CURSORS
            .with(|c| c.get())
            .expect("a split was built");
        let (left_viewport, right_viewport) = super::SPLIT_VIEWPORTS.with(|v| v.get()).unwrap();
        let selects =
            |cursor: RwSignal<Cursor>| cursor.with(|c| c.regions_iter().any(|(a, b, _)| a != b));
        // Rows go fn a, fn old beside fn b, fn c, three at a time; the
        // unified buffer has fn a, the ghost of fn old, fn b, fn c, so the
        // rows of the k-th three are display lines 4k, 4k + 1 for fn old
        // or 4k + 2 for fn b, and 4k + 3.
        let line_height = crate::theme::code_line_height() as f64;
        let (left, top) = pane_origin();
        let code = gutter_width() + crate::theme::CODE_GUTTER_GAP + 2.0;
        let x_left = left + code;
        let x_right = left + (1400.0 - left - 1.0) / 2.0 + 1.0 + code;
        // Where a row is on the screen, as the pane is scrolled now.
        let y =
            |row: usize| top + (row as f64 + 0.5) * line_height - right_viewport.get_untracked().y0;
        // The first three wholly in view.
        let three = || (right_viewport.get_untracked().y0 / line_height).ceil() as usize / 3 + 1;
        let scroll = |harness: &mut HeadlessHarness| {
            harness.scroll_down(x_right, top + 100.0, 15.0 * line_height);
            harness.rebuild();
            harness.paint();
            right_viewport.get()
        };

        drag(&mut harness, (x_right, y(4)), (x_right + 30.0, y(5)));
        assert!(selects(right_cursor));
        assert_eq!(state.selection.get(), Some((6, 7)));

        // With the right's selection scrolled away, fn old and fn c on
        // the left: the ghost and the line after fn b.
        let scrolled = scroll(&mut harness);
        assert!(scrolled.y0 > 6.0 * line_height);
        let k = three();
        drag(
            &mut harness,
            (x_left, y(3 * k + 1)),
            (x_left + 30.0, y(3 * k + 2)),
        );
        assert!(selects(left_cursor), "the left took no selection");
        assert!(!selects(right_cursor), "the right kept its selection");
        assert_eq!(state.selection.get(), Some((4 * k + 1, 4 * k + 3)));
        assert_eq!(state.cursor_line.get(), 4 * k + 3);
        assert_eq!(right_viewport.get(), scrolled, "letting go scrolled");
        assert_eq!(left_viewport.get().y0, scrolled.y0);

        let scrolled = scroll(&mut harness);
        let k = three();
        harness.click(x_right, y(3 * k + 1));
        harness.rebuild();
        harness.paint();
        assert!(!selects(left_cursor), "the left kept its selection");
        assert_eq!(state.selection.get(), Some((4 * k + 2, 4 * k + 2)));
        assert_eq!(state.cursor_line.get(), 4 * k + 2);
        assert_eq!(right_viewport.get(), scrolled, "letting go scrolled");
        assert_eq!(left_viewport.get().y0, scrolled.y0);
    }

    /// A filler on the left stands beside a line the head added, and a
    /// click on it is on that line.
    #[test]
    fn a_click_on_a_filler_on_the_left_is_on_the_added_line_beside_it() {
        // Display lines go fn a, fn new, fn c, three a time.
        let (mut harness, state) = harness_with(
            "fn a() {}\nfn new() {}\nfn c() {}\n".repeat(30),
            "fn a() {}\nfn c() {}\n".repeat(30),
        );
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let line_height = crate::theme::code_line_height() as f64;
        let (left, top) = pane_origin();
        harness.click(left + 200.0, top + 4.5 * line_height);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.cursor_line.get(), 4);
        assert_eq!(state.selection.get(), Some((4, 4)));
    }

    /// The left brings its caret into view as the right does, a key at a
    /// time past the bottom of the pane, and the right follows it down.
    #[test]
    fn a_caret_brought_into_view_on_the_left_takes_the_right_with_it() {
        use floem::prelude::{Code, Key, NamedKey};
        let (mut harness, state) = harness();
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let (left_viewport, right_viewport) = super::SPLIT_VIEWPORTS.with(|v| v.get()).unwrap();
        let line_height = crate::theme::code_line_height() as f64;
        let (left, top) = pane_origin();
        harness.click(left + 200.0, top + line_height / 2.0);
        harness.rebuild();
        harness.paint();
        let rows = (left_viewport.get().height() / line_height).ceil() as usize;
        for _ in 0..rows + 5 {
            harness.dispatch_event(key_press(Code::ArrowDown, Key::Named(NamedKey::ArrowDown)));
            harness.rebuild();
            harness.paint();
        }
        assert!(left_viewport.get().y0 > 0.0, "the left stayed at the top");
        assert_eq!(right_viewport.get().y0, left_viewport.get().y0);
    }

    /// The primary button going down or up at a point, `count` clicks
    /// in: the harness's own `double_click` sends the two together, with
    /// no move between.
    fn button(x: f64, y: f64, count: u8, down: bool) -> floem::event::Event {
        use floem::ui_events::pointer::{
            PointerButton, PointerButtonEvent, PointerEvent, PointerId, PointerInfo, PointerState,
            PointerType,
        };
        let mut state = PointerState::default();
        state.position.x = x;
        state.position.y = y;
        state.count = count;
        let event = PointerButtonEvent {
            state,
            button: Some(PointerButton::Primary),
            pointer: PointerInfo {
                pointer_id: Some(PointerId::PRIMARY),
                persistent_device_id: None,
                pointer_type: PointerType::Mouse,
            },
        };
        floem::event::Event::Pointer(if down {
            PointerEvent::Down(event)
        } else {
            PointerEvent::Up(event)
        })
    }

    /// macOS sends a pointer move between a double click's press and its
    /// release, without the pointer having left the character, and the
    /// editor took it as a drag that cut the word off at the pointer. The
    /// fork's editor keeps the word: this is the sequence as the window
    /// gets it, through the pane's own listeners.
    #[test]
    fn a_double_click_selects_the_whole_word() {
        let word = "the_word_under_a_double_click_stays_whole_wherever_the_pointer_settles";
        let (mut harness, state) = harness_with(
            format!(
                "{word} = 1;\n{}",
                "fn a() {}\nfn b() {}\nfn c() {}\n".repeat(30)
            ),
            "fn a() {}\nfn old() {}\nfn c() {}\n".repeat(30),
        );
        let live = stacked(state);
        let cursor = super::STACKED_CURSOR
            .with(|c| c.get())
            .expect("a stacked editor was built");
        let doc = live.doc.get();
        let row = doc.row_of_head(0).expect("the word's line is shown");
        let start = doc.text.find(word).expect("the word is laid out");
        let whole = (start, start + word.len());
        let selected = || cursor.with(|c| (c.start_offset(), c.offset()));

        // Forty points into the code is a few characters into the word.
        let (left, top) = pane_origin();
        let x = left + gutter_width() + crate::theme::CODE_GUTTER_GAP + 40.0;
        let y = top + (row as f64 + 0.5) * crate::theme::code_line_height() as f64;
        harness.dispatch_event(button(x, y, 2, true));
        harness.rebuild();
        harness.paint();
        assert_eq!(selected(), whole, "the press selects the word");

        harness.pointer_move(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(selected(), whole, "the move that follows leaves it whole");

        harness.dispatch_event(button(x, y, 2, false));
        harness.rebuild();
        harness.paint();
        assert_eq!(selected(), whole);
    }

    /// Every view under `id`, itself included, with `text` in its debug
    /// name, in tree order.
    fn find_all(id: floem::ViewId, text: &str, out: &mut Vec<floem::ViewId>) {
        if id.debug_name().contains(text) {
            out.push(id);
        }
        for child in id.children() {
            find_all(child, text, out);
        }
    }

    /// A row's tint in the layer under an editor, by the editor's box and
    /// the row: its box in the window and its colour.
    fn tint(
        harness: &HeadlessHarness,
        editor_box: floem::ViewId,
        row: usize,
    ) -> (floem::ViewId, floem::kurbo::Rect, Option<Brush>) {
        let id = find(editor_box, &format!("tint:{row}")).expect("the row is in view");
        (
            id,
            id.get_visual_rect_no_clip(),
            harness.get_computed_style(id).get(floem::style::Background),
        )
    }

    /// The editor paints a line's own styles after the selection, so a
    /// diff tint among them hid the selection on every changed line. The
    /// tint is a layer under the editor instead: on the row, across the
    /// editor's box from the gutter's edge, moving with the scroll, and
    /// painted before the editor's content, which paints the selection.
    #[test]
    fn a_changed_lines_tint_is_under_the_editor_so_the_selection_shows_over_it() {
        use crate::review::doc::{DocRow, Tint};
        use crate::theme::{code_line_height, diff_deleted_bg, diff_modified_bg};
        let line = |display, tint| DocRow::Line { display, tint };

        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = code_line_height() as f64;
        // Rows: fn a, the ghost of fn old, fn b in its place, fn c, and
        // so on every three.
        let doc = live.doc.get();
        assert_eq!(doc.rows[1], line(1, Tint::Deleted));
        assert_eq!(doc.rows[2], line(2, Tint::Modified));
        assert_eq!(doc.rows[3], line(3, Tint::Plain));

        let root = harness.root_id();
        let editor = find(root, "tinted editor").expect("the code editor's box");
        let content = find(editor, "Editor View").expect("the editor's content");
        let editor_box = editor.get_visual_rect_no_clip();
        let (left_edge, top) = pane_origin();
        assert_eq!(
            (editor_box.x0, editor_box.x1, editor_box.y0),
            (left_edge + gutter_width(), 1400.0, top),
            "the box is the pane past the gutter, under the tabs"
        );

        let (ghost, rect, bg) = tint(&harness, editor, 1);
        assert_eq!(
            (rect.x0, rect.x1),
            (editor_box.x0, editor_box.x1),
            "the tint runs across the editor's box, gap included"
        );
        assert_eq!((rect.y0, rect.height()), (top + line_height, line_height));
        assert_eq!(bg, Some(Brush::Solid(diff_deleted_bg())));
        let (changed, rect, bg) = tint(&harness, editor, 2);
        assert_eq!(rect.y0, top + 2.0 * line_height);
        assert_eq!(bg, Some(Brush::Solid(diff_modified_bg())));
        let (_, _, bg) = tint(&harness, editor, 3);
        assert_eq!(bg, None, "a plain line has no tint");

        let order = harness.paint_and_get_order();
        let painted = |id: floem::ViewId| {
            order
                .iter()
                .position(|p| *p == id)
                .unwrap_or_else(|| panic!("{} was painted", id.debug_name()))
        };
        assert!(
            painted(ghost) < painted(content) && painted(changed) < painted(content),
            "the tints are painted before the editor's content"
        );

        // Scrolled, a row's tint is where its line is.
        let (x, y) = (editor_box.center().x, editor_box.center().y);
        harness.scroll_down(x, y, 100.0);
        harness.rebuild();
        harness.paint();
        let scrolled = super::STACKED_VIEWPORT
            .with(|v| v.get())
            .expect("a stacked editor was built")
            .get()
            .y0;
        assert!(scrolled > 0.0, "the editor did not scroll");
        let (_, rect, bg) = tint(&harness, editor, 10);
        assert_eq!(rect.y0, top + 10.0 * line_height - scrolled);
        assert_eq!(bg, Some(Brush::Solid(diff_modified_bg())));

        // A split has a layer under each side, over that side's rows: the
        // deleted line on the left, the changed one on the right. The split
        // opens where the file was left, so back to the top first.
        harness.scroll_up(x, y, 100.0);
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let mut editors = Vec::new();
        find_all(root, "tinted editor", &mut editors);
        let [left, right] = editors[..] else {
            panic!("a split has two editors, not {}", editors.len())
        };
        let (left_box, right_box) = (
            left.get_visual_rect_no_clip(),
            right.get_visual_rect_no_clip(),
        );
        assert_eq!(left_box.x0, left_edge + gutter_width());
        assert!(left_box.x1 < right_box.x0, "the sides are side by side");
        assert_eq!(right_box.x1, 1400.0);
        let (_, rect, bg) = tint(&harness, left, 1);
        assert_eq!(
            (rect.x0, rect.x1, rect.y0),
            (left_box.x0, left_box.x1, top + line_height)
        );
        assert_eq!(bg, Some(Brush::Solid(diff_deleted_bg())));
        let (_, rect, bg) = tint(&harness, right, 1);
        assert_eq!(
            (rect.x0, rect.x1, rect.y0),
            (right_box.x0, right_box.x1, top + line_height)
        );
        assert_eq!(bg, Some(Brush::Solid(diff_modified_bg())));
    }

    /// The scrollbar's element under a window point, when the point is
    /// on the bar of the given scroll: floem draws its bars as elements
    /// of the scroll view beside the view's own, and only while there is
    /// something to scroll on that axis.
    fn bar_at(harness: &HeadlessHarness, scroll: floem::ViewId, x: f64, y: f64) -> bool {
        harness
            .element_id_at(x, y)
            .is_some_and(|e| e.owning_id() == scroll && !e.is_view())
    }

    /// The editor's scroll ends at the pane's right edge, so floem's
    /// vertical bar, which it draws over the scroll's last pixels, is on
    /// the screen: padding on the editor for the gap before the code had
    /// pushed the scroll, which floem sizes to the editor's padding box,
    /// out past the window, and the bar with it. The bars are the theme's,
    /// as on every other scroll in Perch: a rounded slider at floem's
    /// width, which fades out once the code stops scrolling when macOS
    /// overlays its scroll bars,
    /// down the right while the file is taller than the view and along
    /// the bottom while a line is wider than it, and nothing in either
    /// place while there is nothing more to see.
    #[test]
    fn the_bars_are_at_the_codes_edges_only_while_there_is_more_to_see() {
        use floem::style::{ScrollbarWidth, StyleProp};
        use floem::views::scroll::{Handle, Rounded, ShowBarsWhenIdle};
        let bar = ScrollbarWidth::default_value().0;
        let (harness, _) = harness();
        let root = harness.root_id();
        let editor = find(root, "tinted editor").expect("the code editor's box");
        let editor_box = editor.get_visual_rect_no_clip();
        let scroll = find(editor, "Scroll").expect("the editor's scroll");
        let scroll_box = scroll.get_visual_rect_no_clip();
        assert_eq!(
            (scroll_box.x0, scroll_box.x1),
            (editor_box.x0 + crate::theme::CODE_GUTTER_GAP, editor_box.x1),
            "the scroll runs from the gap to the pane's edge"
        );
        let style = harness.get_computed_style(scroll);
        assert_eq!(style.get(ScrollbarWidth).0, bar);
        assert_eq!(
            style.get(ShowBarsWhenIdle),
            !crate::scrollers::overlaid(),
            "the bars fade out while the code is still when macOS overlays them"
        );
        assert!(
            style.apply_class(Handle).get(Rounded),
            "the slider is a pill"
        );

        // Ninety lines is taller than the view: the bar is over the last
        // `bar` pixels of the code and answers the pointer there, and the
        // code answers it left of the bar.
        let y = editor_box.y0 + 100.0;
        assert!(bar_at(&harness, scroll, editor_box.x1 - bar + 1.0, y));
        assert!(bar_at(&harness, scroll, editor_box.x1 - 1.0, y));
        assert!(
            !bar_at(&harness, scroll, editor_box.x1 - bar - 1.0, y),
            "the code is left of the bar"
        );
        // No line is wider than the view: nothing along the bottom.
        let x = editor_box.x0 + 100.0;
        assert!(!bar_at(&harness, scroll, x, editor_box.y1 - 1.0));
        assert!(!bar_at(&harness, scroll, x, editor_box.y1 - bar + 1.0));
        drop(harness);

        // A file that fits the view has no bar.
        let (harness, _) = harness_with(
            "fn a() {}\nfn b() {}\n".to_string(),
            "fn a() {}\n".to_string(),
        );
        let editor = find(harness.root_id(), "tinted editor").unwrap();
        let editor_box = editor.get_visual_rect_no_clip();
        let scroll = find(editor, "Scroll").unwrap();
        assert!(!bar_at(
            &harness,
            scroll,
            editor_box.x1 - 1.0,
            editor_box.y0 + 5.0
        ));
        drop(harness);

        // Lines wider than the view, in a file that fits it: a bar along
        // the bottom, the same height, and none at the right.
        let wide: String = (0..20)
            .map(|i| format!("fn f{i}() {{ {} }}\n", "x".repeat(200)))
            .collect();
        let (harness, _) = harness_with(wide.clone(), wide);
        let editor = find(harness.root_id(), "tinted editor").unwrap();
        let editor_box = editor.get_visual_rect_no_clip();
        let scroll = find(editor, "Scroll").unwrap();
        let x = editor_box.x0 + 100.0;
        assert!(bar_at(&harness, scroll, x, editor_box.y1 - 1.0));
        assert!(bar_at(&harness, scroll, x, editor_box.y1 - bar + 1.0));
        assert!(
            !bar_at(&harness, scroll, x, editor_box.y1 - bar - 1.0),
            "the code is above the bar"
        );
        assert!(!bar_at(
            &harness,
            scroll,
            editor_box.x1 - 1.0,
            editor_box.y0 + 5.0
        ));
    }

    /// A split scrolls by the head side's bar alone: its scroll ends at
    /// the pane's edge with the bar there, and the base side's scroll
    /// ends at its own pane's edge, before the divider, with its bars
    /// hidden.
    #[test]
    fn a_split_has_one_bar_on_the_head_side() {
        use floem::views::scroll::HideBars;
        let (mut harness, state) = harness();
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let root = harness.root_id();
        let mut editors = Vec::new();
        find_all(root, "tinted editor", &mut editors);
        let [left, right] = editors[..] else {
            panic!("a split has two editors, not {}", editors.len())
        };
        let (left_box, right_box) = (
            left.get_visual_rect_no_clip(),
            right.get_visual_rect_no_clip(),
        );
        let left_scroll = find(left, "Scroll").unwrap();
        let right_scroll = find(right, "Scroll").unwrap();
        assert_eq!(left_scroll.get_visual_rect_no_clip().x1, left_box.x1);
        assert_eq!(right_scroll.get_visual_rect_no_clip().x1, right_box.x1);
        assert_eq!(right_box.x1, 1400.0);

        let y = right_box.y0 + 100.0;
        assert!(bar_at(&harness, right_scroll, right_box.x1 - 1.0, y));
        assert!(!harness.get_computed_style(right_scroll).get(HideBars));
        assert!(harness.get_computed_style(left_scroll).get(HideBars));
    }

    /// The tiles sit at the strip's right end: 8 of padding, then the
    /// tree's tile and its rule, then
    /// Split, Stacked and Word wrap reading leftwards, each `LAYOUT_TILE`
    /// square with 2 between, in a strip `TAB_HEIGHT` tall under the
    /// header's bar.
    fn tile_center(layout: Layout) -> (f64, f64) {
        let tile = crate::theme::LAYOUT_TILE;
        let split_right =
            1400.0 - 8.0 - tile - (crate::theme::TILE_RULE_GAP + 1.0 + crate::theme::TILE_RULE_GAP);
        let x = match layout {
            Layout::Split => split_right - tile / 2.0,
            Layout::Stacked => split_right - tile - 2.0 - tile / 2.0,
        };
        let y = crate::theme::REVIEW_HEADER_HEIGHT + crate::theme::TAB_HEIGHT / 2.0;
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
        assert_eq!(
            state.layout.get(),
            Layout::Split,
            "click at ({x}, {y}) over {under:?} did not split"
        );

        let (x, y) = tile_center(Layout::Stacked);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.layout.get(), Layout::Stacked);
    }

    /// The Show whole tile is left of Stacked, past the rule and the room
    /// on either side of it.
    /// Word wrap's tile, left of Stacked.
    fn wrap_tile_center() -> (f64, f64) {
        let (stacked, y) = tile_center(Layout::Stacked);
        (stacked - crate::theme::LAYOUT_TILE - 2.0, y)
    }

    fn fold_tile_center() -> (f64, f64) {
        let (wrap, y) = wrap_tile_center();
        let tile = crate::theme::LAYOUT_TILE;
        (
            wrap - tile / 2.0
                - crate::theme::TILE_RULE_GAP
                - 1.0
                - crate::theme::TILE_RULE_GAP
                - tile / 2.0,
            y,
        )
    }

    /// A pull request for the item, so the window draws the review.
    fn pull_request() -> perch_github::PullRequest {
        perch_github::PullRequest {
            number: 812,
            html_url: "https://github.com/slatedb/slatedb/pull/812".into(),
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

    /// Word wrap's tile wraps the open file's long lines at the editor's
    /// width, in place, and a click again lets them run on; the split's
    /// two sides wrap level with each other.
    #[test]
    fn the_wrap_tile_wraps_long_lines_at_the_editors_width_and_back() {
        let long = format!("    let words = [{}];\n", "\"word\", ".repeat(60));
        let (mut harness, state) = harness_with(
            format!("fn a() {{\n{long}}}\n"),
            "fn a() {\n}\n".to_string(),
        );
        let live = stacked(state);
        let whole = live.doc.get().text.clone();
        assert!(!state.wrap.get(), "lines run on by default");

        let (x, y) = wrap_tile_center();
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert!(
            state.wrap.get(),
            "click at ({x}, {y}) over {under:?} did not wrap"
        );
        let doc = live.doc.get();
        assert_eq!(laid_out_text(&live), doc.text, "edited into the editor");
        let wraps = doc
            .rows
            .iter()
            .filter(|r| matches!(r, crate::review::doc::DocRow::Wrap { .. }))
            .count();
        assert!(wraps > 0, "the long line wraps");
        // Every row fits the editor, at its width less the gap at the
        // right edge.
        let width = super::STACKED_VIEWPORT
            .with(|v| v.get())
            .unwrap()
            .get()
            .width();
        let columns = ((width - crate::theme::CODE_GUTTER_GAP)
            / crate::review::gutter::column_width()) as usize;
        assert_eq!(state.wrap_columns.get()[0], Some(columns));
        for row in 0..doc.len() {
            assert!(doc.line(row).len() <= columns, "row {row} fits");
        }
        assert_eq!(doc.source_line(1), long.trim_end());

        // Cmd-C over the wrapped line copies the line, not its rows.
        let (left, top) = pane_origin();
        harness.click(
            left + 200.0,
            top + crate::theme::code_line_height() as f64 / 2.0,
        );
        harness.rebuild();
        harness.paint();
        let (from, to) = (
            doc.offset_of_row(1),
            doc.offset_of_row(doc.line_rows(1).end),
        );
        super::STACKED_CURSOR
            .with(|c| c.get())
            .unwrap()
            .update(|c| {
                c.set_insert(floem::views::editor::core::selection::Selection::region(
                    from,
                    to,
                    CursorAffinity::Backward,
                ))
            });
        let cmd_c = || {
            floem::event::Event::Key(floem::prelude::KeyboardEvent {
                state: floem::prelude::KeyState::Down,
                key: floem::prelude::Key::Character("c".into()),
                code: floem::prelude::Code::KeyC,
                location: Default::default(),
                modifiers: crate::ui::OS_MOD,
                repeat: false,
                is_composing: false,
            })
        };
        crate::ui::widgets::copied();
        harness.dispatch_event(cmd_c());
        assert_eq!(crate::ui::widgets::copied(), std::slice::from_ref(&long));
        // With nothing selected, the caret's line, whichever of its rows
        // the caret is on.
        let caret = doc.offset_of_row(2) + 6;
        super::STACKED_CURSOR
            .with(|c| c.get())
            .unwrap()
            .update(|c| c.set_offset(caret, CursorAffinity::Backward, false, false));
        harness.dispatch_event(cmd_c());
        assert_eq!(crate::ui::widgets::copied(), std::slice::from_ref(&long));

        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert!(!state.wrap.get());
        assert_eq!(live.doc.get().text, whole, "the lines run on again");
        assert_eq!(laid_out_text(&live), whole);

        // Split, wrapped: the two sides are as long as each other, and
        // the line beside the wrapped one is as tall as it.
        state.wrap.set(true);
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let (left, right) = sides(state);
        let (left, right) = (left.doc.get(), right.doc.get());
        assert_eq!(left.len(), right.len());
        let row = right.row_of_head(1).unwrap();
        assert!(right.line_rows(row).len() > 1, "wrapped on the head side");
        assert!(right.line_rows(row).end <= left.len());
    }

    #[test]
    fn show_whole_unfolds_every_run_and_folding_again_keeps_the_reveals() {
        let (mut harness, state) = one_long_run();
        let live = stacked(state);
        state.pull_request.set(Some(Arc::new(pull_request())));
        harness.rebuild();
        harness.paint();
        assert!(state.folded("src/x.rs"), "folded by default");
        assert_eq!(state.hidden_lines(), 54);

        // Twenty lines at the bottom of the run, to be kept through Show
        // whole and back.
        let run = live.doc.get().run_of(5).expect("row 5 is the divider");
        let (first, last) = run.bottom();
        state.reveal("src/x.rs", "", first, last);
        harness.rebuild();
        harness.paint();
        assert_eq!(live.doc.get().len(), 104);
        assert_eq!(state.hidden_lines(), 34);

        let (x, y) = fold_tile_center();
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert!(
            !state.folded("src/x.rs"),
            "click at ({x}, {y}) over {under:?} did not show the file whole"
        );
        let doc = live.doc.get();
        assert_eq!(doc.len(), 64 + 72, "every line, no divider");
        assert!(doc.folds.is_empty());
        assert_eq!(state.hidden_lines(), 0, "nothing hidden");
        assert_eq!(laid_out_text(&live), doc.text);

        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert!(state.folded("src/x.rs"));
        assert_eq!(
            live.doc.get().len(),
            104,
            "folded again, with the reveal as it was"
        );
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
    /// enough to scroll: one run of fifty-four behind a divider of two
    /// rows at row 5, Down over Up, and nothing else folded.
    fn one_long_run() -> (HeadlessHarness, ReviewState) {
        let blocks = |word: &str| {
            (0..12)
                .map(|i| format!("{word}{i}\n{}", context(60 + i * 4, 4)))
                .collect::<String>()
        };
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

    /// Where the code pane starts: after the open drawer on the left,
    /// under the header's bar and the file tabs.
    fn pane_origin() -> (f64, f64) {
        (
            crate::theme::SIDEBAR_WIDTH,
            crate::theme::REVIEW_HEADER_HEIGHT + crate::theme::TAB_HEIGHT,
        )
    }

    #[test]
    fn a_divider_row_exists_and_up_on_its_gutter_grows_the_document_without_moving_the_viewport() {
        use crate::review::doc::{DocRow, Expand};
        let (mut harness, state) = one_long_run();
        let live = stacked(state);
        let line_height = crate::theme::code_line_height() as f64;
        // Display: ghost, new top, line 0..59 at 2 to 61, ghost, new
        // bottom, then the twelve blocks of six. The run is display lines
        // 5 to 58, head lines 4 to 57, and its divider is two rows.
        let doc = live.doc.get();
        assert_eq!(doc.len(), 64 + 72 - 54 + 2);
        let run = doc.run_of(5).expect("row 5 is the divider");
        assert_eq!(run.len(), 54);
        assert_eq!(
            doc.rows[5],
            DocRow::Divider {
                run: 0,
                expand: Expand::Down
            }
        );
        assert_eq!(
            doc.rows[6],
            DocRow::Divider {
                run: 0,
                expand: Expand::Up
            }
        );
        assert_eq!(laid_out_text(&live), doc.text);
        let viewport = super::STACKED_VIEWPORT
            .with(|v| v.get())
            .expect("a stacked editor was built");
        let y0 = viewport.get().y0;

        // The expanders are the gutter's rows beside the divider, the
        // gutter's width each, whether the pointer is there or not. Up
        // is the second, and opens the run's last lines, which follow
        // the divider. The pointer on it is on the divider's first row.
        let (left, top) = pane_origin();
        let (x, y) = (
            left + gutter_width() / 2.0,
            top + 6.0 * line_height + line_height / 2.0,
        );
        harness.pointer_move(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(
            state.hovered_line.get(),
            Some(5),
            "the expander records the pointer as the divider's"
        );
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();

        let doc = live.doc.get();
        assert_eq!(
            doc.len(),
            84 + 20,
            "click at ({x}, {y}) over {under:?} did not reveal twenty lines"
        );
        assert_eq!(
            doc.run_of(5).map(|r| r.len()),
            Some(34),
            "the divider stays, hiding twenty fewer"
        );
        assert_eq!(
            doc.row_of_head(38),
            Some(7),
            "the first revealed line follows the divider's two rows"
        );
        assert_eq!(
            laid_out_text(&live),
            doc.text,
            "the editor's text is the document's"
        );
        assert_eq!(viewport.get().y0, y0, "the viewport did not move");
        assert!(state
            .reveals_for("src/x.rs", "")
            .overlaps(run.default_heads.0, run.default_heads.1));
    }

    /// The window rect of the one view under `root` named `text`.
    fn rect_of(root: floem::ViewId, text: &str) -> floem::kurbo::Rect {
        find(root, text)
            .unwrap_or_else(|| panic!("no view named {text}"))
            .get_visual_rect_no_clip()
    }

    /// A divider is github.com's hunk row: its expanders are the gutter's
    /// rows beside it, the gutter wide and a row high, in the number
    /// cell's blue and GitHub's own blue under the pointer; its header is
    /// over the code, from the first code column, down the middle of its
    /// rows. The gutter is as wide as three digits need, narrower than the
    /// seventy it was when the controls sat in a row inside it.
    #[test]
    fn a_divider_is_githubs_hunk_row_with_its_expanders_in_the_gutter_and_its_header_over_the_code()
    {
        use crate::theme::{diff_expander_bg, diff_expander_hover_bg, CODE_GUTTER_GAP};
        let (mut harness, state) = one_long_run();
        let line_height = crate::theme::code_line_height() as f64;
        let root = harness.root_id();
        let (left, top) = pane_origin();
        assert!(
            gutter_width() < 60.0,
            "the gutter is {} wide",
            gutter_width()
        );
        // The named box is inside the clip that carries the width and the
        // rule.
        let gutter = find(root, "gutter")
            .and_then(|g| g.parent())
            .unwrap()
            .get_visual_rect_no_clip();
        assert_eq!((gutter.x0, gutter.width()), (left, gutter_width()));

        // Down over Up, each a row of the gutter, less its rule.
        let down = rect_of(root, "expander:Down");
        let up = rect_of(root, "expander:Up");
        assert_eq!(
            (down.x0, down.width(), down.y0, down.height()),
            (
                left,
                gutter_width() - 1.0,
                top + 5.0 * line_height,
                line_height
            )
        );
        assert_eq!((up.x0, up.y0), (left, top + 6.0 * line_height));
        assert_eq!(count(root, "expander:"), 2, "one divider, two expanders");
        let background = |harness: &HeadlessHarness, name: &str| {
            let id = find(harness.root_id(), name).unwrap();
            harness.get_computed_style(id).get(floem::style::Background)
        };
        assert_eq!(
            background(&harness, "expander:Down"),
            Some(Brush::Solid(diff_expander_bg()))
        );
        harness.pointer_move(up.center().x, up.center().y);
        harness.rebuild();
        harness.paint();
        assert_eq!(
            background(&harness, "expander:Up"),
            Some(Brush::Solid(diff_expander_hover_bg()))
        );
        assert_eq!(
            background(&harness, "expander:Down"),
            Some(Brush::Solid(diff_expander_bg()))
        );

        // The header counts the hunk below the run, which runs to the end
        // of the file, and takes its context from the base: the nearest
        // line above the hunk that starts an identifier.
        // Base line 58, the one right above the hunk, is `line 56`.
        let header = "@@ -59,64 +59,64 @@ line 56";
        let editor = rect_of(root, "tinted editor");
        let label = rect_of(root, header);
        assert_eq!(
            label.x0,
            editor.x0 + CODE_GUTTER_GAP,
            "from the first code column"
        );
        assert!(
            (label.center().y - (top + 6.0 * line_height)).abs() < 1.0,
            "down the middle of the two rows: {label:?}"
        );

        // Split, the expanders are the base side's, at the row's left end,
        // and the header is over the base side's code.
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let root = harness.root_id();
        let (_, right) = sides(state);
        let row = (0..right.doc.get().len())
            .find(|r| right.doc.get().run_of(*r).is_some())
            .expect("the split has the divider");
        let down = rect_of(root, "expander:Down");
        assert_eq!((down.x0, down.y0), (left, top + row as f64 * line_height));
        assert_eq!(count(root, "expander:"), 2, "none on the head side");
        let label = rect_of(root, header);
        assert_eq!(label.x0, left + gutter_width() + CODE_GUTTER_GAP);
        let middle = left + (1400.0 - left - 1.0) / 2.0;
        assert!(label.x1 < middle, "the header is over the base side");
    }

    /// A run that starts the file has no code above it to grow down: its
    /// divider is one row, Up alone.
    #[test]
    fn a_run_at_the_top_of_the_file_has_up_alone() {
        use crate::review::doc::{DocRow, Expand};
        let blocks = |word: &str| {
            (0..8)
                .map(|i| format!("{word}{i}\n{}", context(100 + i * 4, 4)))
                .collect::<String>()
        };
        let (harness, state) = harness_with(
            format!("{}{}", context(0, 100), blocks("new")),
            format!("{}{}", context(0, 100), blocks("old")),
        );
        let doc = stacked(state).doc.get();
        assert_eq!(
            doc.rows[0],
            DocRow::Divider {
                run: 0,
                expand: Expand::Up
            }
        );
        assert!(doc.display_line(1).is_some(), "the divider is one row");
        let root = harness.root_id();
        assert_eq!(count(root, "expander:"), 1);
        assert!(find(root, "expander:Up").is_some());
        // Lines 98 to 100 and the first change's pair lead the hunk.
        assert!(find(root, "@@ -98,").is_some(), "the header is drawn");
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
        let line_height = crate::theme::code_line_height() as f64;
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).unwrap();
        assert_eq!(state.cursor_line.get(), 0);

        // A bare key goes to whatever has the keyboard, and the window's
        // handler catches it on the way down. A click on a line's number
        // gives it to the gutter, not to the number, which would lose it
        // when the jump scrolls the row away and leave `[` with nowhere
        // to go.
        let (left, top) = pane_origin();
        harness.click(
            left + gutter_width() - 14.0,
            top + 3.0 * line_height + line_height / 2.0,
        );
        harness.rebuild();

        // The next change after the top starts at the ghost of old bottom,
        // display line 62; the first head line from there, new bottom, is
        // head line 61, on row 12 of the folded document after the
        // divider's two rows and the two ghosts of the rewritten pair.
        harness.dispatch_event(bracket(true));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.cursor_line.get(), 62);
        assert!(
            state.jump_to_line.get().is_none(),
            "the editor took the jump"
        );
        assert_eq!(live.doc.get().row_of_head(61), Some(12));
        assert_eq!(viewport.get().y0, (12.0 - 4.0) * line_height);
        assert_eq!(
            live.doc.get().len(),
            84,
            "a change is never folded, so nothing was revealed"
        );

        harness.dispatch_event(bracket(false));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.cursor_line.get(), 0);
        assert_eq!(viewport.get().y0, 0.0);
    }

    /// A click on a divider's header leaves the keyboard with the layer
    /// the bars are drawn in, which the jump does not scroll away, so the
    /// keys go on working after it.
    #[test]
    fn the_bracket_keys_work_after_a_click_on_a_header() {
        let (mut harness, state) = one_long_run();
        let header = find(harness.root_id(), "@@ -59,64 +59,64 @@").expect("the header is drawn");
        let at = header.get_visual_rect_no_clip().center();
        harness.click(at.x, at.y);
        harness.rebuild();

        harness.dispatch_event(bracket(true));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.cursor_line.get(), 62);
        harness.dispatch_event(bracket(false));
        harness.rebuild();
        harness.paint();
        assert_eq!(
            state.cursor_line.get(),
            0,
            "the keyboard went with the header"
        );
    }

    #[test]
    fn a_reveal_above_the_viewport_moves_the_scroll_so_the_visible_lines_stay() {
        // A hundred unchanged lines, then eight changes four lines apart:
        // the run at the top folds from line one, and the rest is long
        // enough to scroll.
        let blocks = |word: &str| {
            (0..8)
                .map(|i| format!("{word}{i}\n{}", context(100 + i * 4, 4)))
                .collect::<String>()
        };
        let (head, base) = (
            format!("{}{}", context(0, 100), blocks("new")),
            format!("{}{}", context(0, 100), blocks("old")),
        );
        let (mut harness, state) = harness_with(head, base);
        let live = stacked(state);
        let line_height = crate::theme::code_line_height() as f64;
        let run = live
            .doc
            .get()
            .run_of(0)
            .expect("the top run folds from line one");
        assert_eq!(run.len(), 97);
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).unwrap();

        let (left, top) = pane_origin();
        harness.scroll_down(left + 300.0, top + 300.0, 200.0);
        harness.rebuild();
        harness.paint();
        let before = viewport.get().y0;
        assert!(
            before >= line_height,
            "the editor did not scroll past the divider"
        );
        let first_row = (before / line_height).floor() as usize;
        let first_line = live.doc.get().display_line(first_row);

        let (first, last) = run.bottom();
        state.reveal("src/x.rs", "", first, last);
        harness.rebuild();
        harness.paint();
        assert_eq!(live.doc.get().len(), 52 + 20);
        assert_eq!(
            viewport.get().y0,
            before + 20.0 * line_height,
            "the scroll moved by the rows added above it"
        );
        let doc = live.doc.get();
        assert_eq!(
            doc.display_line(first_row + 20),
            first_line,
            "the same line is first in view"
        );
    }

    /// A click on the code at a row of the stacked editor, which puts the
    /// caret there.
    fn click_row(harness: &mut HeadlessHarness, row: usize) {
        let (left, top) = pane_origin();
        let x = left + gutter_width() + crate::theme::CODE_GUTTER_GAP + 20.0;
        let y = top + (row as f64 + 0.5) * crate::theme::code_line_height() as f64;
        harness.dispatch_event(button(x, y, 1, true));
        harness.dispatch_event(button(x, y, 1, false));
        harness.rebuild();
        harness.paint();
    }

    /// Scroll the stacked editor down by `rows`.
    fn scroll_rows(harness: &mut HeadlessHarness, rows: usize) {
        let (left, top) = pane_origin();
        let line_height = crate::theme::code_line_height() as f64;
        harness.scroll_down(left + 300.0, top + 300.0, rows as f64 * line_height);
        harness.rebuild();
        harness.paint();
    }

    fn rebuilt(harness: &mut HeadlessHarness) {
        harness.rebuild();
        harness.paint();
    }

    /// A restyle of the code, which hovering it, a resize or the system's
    /// appearance changing brings, leaves it where the reader scrolled it,
    /// though the caret is out of sight above. floem's editor scrolled its
    /// caret back into view on every restyle; the fork's does only when
    /// the caret moves.
    #[test]
    fn a_restyle_leaves_the_code_where_it_was_scrolled_to() {
        let (mut harness, state) = harness();
        let live = stacked(state);
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).unwrap();
        let line_height = crate::theme::code_line_height() as f64;
        click_row(&mut harness, 10);
        scroll_rows(&mut harness, 20);
        let scrolled = viewport.get().y0;
        assert!(
            scrolled > 11.0 * line_height,
            "the caret is not out of view"
        );

        live.content
            .expect("the editor's content view")
            .request_style(floem::style::recalc::StyleReason::full_recalc());
        rebuilt(&mut harness);
        assert_eq!(viewport.get().y0, scrolled, "the restyle moved the code");
    }

    /// A reload of the open file under the same scope, as an agent's edit
    /// brings, is edited into the editor showing it rather than building
    /// another, which would draw a frame at the top of the file before the
    /// place came back: the same editor, the new text laid out, and the
    /// lines in view where they were, stacked and split. A line added
    /// above the view moves the scroll by a line, so the same code stays
    /// on screen.
    #[test]
    fn a_reload_of_the_open_file_is_edited_into_its_editor_where_it_stands() {
        let base = "fn a() {}\nfn old() {}\nfn c() {}\n".repeat(30);
        let head = "fn a() {}\nfn b() {}\nfn c() {}\n".repeat(30);
        let (mut harness, state) = harness_with(head.clone(), base.clone());
        let line_height = crate::theme::code_line_height() as f64;
        let reload = |state: ReviewState, head: String| {
            state.finish_load(
                "src/x.rs",
                Arc::new(crate::backend::build_loaded_file(
                    "src/x.rs".into(),
                    head,
                    base.clone(),
                    Some(perch_git::ChangeStatus::Modified),
                    false,
                    perch_git::Scope::Branch,
                    String::new(),
                )),
            )
        };
        let stacked_y = || super::STACKED_VIEWPORT.with(|v| v.get()).unwrap().get().y0;
        scroll_rows(&mut harness, 20);
        let (epoch, scrolled) = (state.editor_epoch.get(), stacked_y());
        assert!(scrolled > 0.0);

        // Below the view: nothing on screen moves.
        let edited = head.replacen("fn c() {}\n", "fn c() {}\nfn added() {}\n", 29);
        reload(state, edited.clone());
        rebuilt(&mut harness);
        assert_eq!(
            state.editor_epoch.get(),
            epoch,
            "the editor was built again"
        );
        let live = stacked(state);
        assert!(live.doc.get().text.contains("fn added()"));
        assert_eq!(
            laid_out_text(&live),
            state.active_file().unwrap().buffer.text
        );
        assert_eq!(stacked_y(), scrolled);

        // Above it: the scroll follows the code down by the line added.
        let above = format!("// new\n{edited}");
        reload(state, above.clone());
        rebuilt(&mut harness);
        assert_eq!(state.editor_epoch.get(), epoch);
        assert!(laid_out_text(&stacked(state)).starts_with("// new\n"));
        assert_eq!(stacked_y(), scrolled + line_height);

        // The split takes one the same way, both sides edited.
        state.layout.set(Layout::Split);
        rebuilt(&mut harness);
        let (_, right) = super::SPLIT_VIEWPORTS.with(|v| v.get()).unwrap();
        let (epoch, at) = (state.editor_epoch.get(), right.get().y0);
        reload(state, format!("{above}fn tail() {{}}\n"));
        rebuilt(&mut harness);
        assert_eq!(state.editor_epoch.get(), epoch, "the split was built again");
        let (left, right_live) = sides(state);
        assert!(laid_out_text(&right_live).contains("fn tail()"));
        assert_eq!(laid_out_text(&left), left.doc.get().text);
        assert_eq!(right.get().y0, at);

        // The find bar's matches follow the new text, and the view stays
        // where the bar last put it.
        state.find_open.set(true);
        state.find_query.set("fn tail".into());
        rebuilt(&mut harness);
        assert_eq!(state.find_matches.get().len(), 1);
        let at = right.get().y0;
        reload(state, format!("{above}fn tail() {{}}\nfn tail() {{}}\n"));
        rebuilt(&mut harness);
        assert_eq!(state.editor_epoch.get(), epoch);
        assert_eq!(state.find_matches.get().len(), 2);
        assert_eq!(right.get().y0, at, "the reload moved the view");
        state.find_open.set(false);

        // A file whose gutter would widen is another editor.
        reload(state, "fn a() {}\n".repeat(1200));
        rebuilt(&mut harness);
        assert_ne!(state.editor_epoch.get(), epoch);
    }

    /// Each file's tab shows where the reader left it: scrolled as it was,
    /// with the caret on its line, after another file's tab or Conversation
    /// has shown in its place. The other layout keeps the same line at the
    /// top. A tab closed and opened again starts at the top, as it starts
    /// folded.
    #[test]
    fn a_file_shows_where_it_was_left_when_its_tab_shows_again() {
        let (mut harness, state) = harness();
        let line_height = crate::theme::code_line_height() as f64;
        let viewport = || super::STACKED_VIEWPORT.with(|v| v.get()).unwrap();
        let caret = || {
            super::STACKED_CURSOR
                .with(|c| c.get())
                .unwrap()
                .get()
                .offset()
        };
        click_row(&mut harness, 10);
        scroll_rows(&mut harness, 20);
        let (scrolled, at) = (viewport().get().y0, caret());
        assert!(scrolled > 0.0 && at > 0, "nothing to come back to");

        let other = crate::backend::build_loaded_file(
            "src/y.rs".into(),
            "fn y() {}\n".repeat(90),
            "fn y() {}\n".repeat(90),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        state.open_tab("src/y.rs");
        state.finish_load("src/y.rs", Arc::new(other));
        rebuilt(&mut harness);
        assert_eq!(viewport().get().y0, 0.0, "another file opens at its top");

        state.open_tab("src/x.rs");
        rebuilt(&mut harness);
        assert_eq!(
            viewport().get().y0,
            scrolled,
            "back from another file's tab"
        );
        assert_eq!(caret(), at, "the caret is where it was");

        state.show_tab(ItemTab::Conversation);
        rebuilt(&mut harness);
        state.show_tab(ItemTab::Code);
        rebuilt(&mut harness);
        assert_eq!(viewport().get().y0, scrolled, "back from Conversation");
        assert_eq!(caret(), at);

        // Split has no ghost rows, so the line at the top is on another
        // row there, and the code is scrolled to it.
        let top_line = stacked(state).doc.get().display_line(20);
        state.layout.set(Layout::Split);
        rebuilt(&mut harness);
        let (_, right) = super::SPLIT_VIEWPORTS.with(|v| v.get()).unwrap();
        let (_, head) = sides(state);
        let row = (right.get().y0 / line_height).round() as usize;
        assert!(row < 20, "the split did not scroll to the line: row {row}");
        assert_eq!(head.doc.get().display_line(row), top_line);
        state.layout.set(Layout::Stacked);
        rebuilt(&mut harness);
        assert_eq!(viewport().get().y0, scrolled, "and back to stacked");

        state.close_tab("src/x.rs");
        state.open_tab("src/x.rs");
        rebuilt(&mut harness);
        assert_eq!(
            viewport().get().y0,
            0.0,
            "a tab opened again starts at the top"
        );
    }

    /// Three changes fifty lines apart, then twelve four lines apart so
    /// the document is long enough to scroll: three runs of forty-four,
    /// so each divider is two rows, Down and Up. Stacked, the runs are
    /// display lines 5 to 48, 57 to 100 and 109 to 152 of 228, and the
    /// folded document's dividers are at rows 5, 15 and 25 of 102.
    fn three_runs() -> (HeadlessHarness, ReviewState) {
        let blocks = |word: &str| {
            (0..12)
                .map(|i| format!("{word}{i}\n{}", context(150 + i * 4, 4)))
                .collect::<String>()
        };
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
        let line_height = crate::theme::code_line_height() as f64;
        let viewport = super::STACKED_VIEWPORT
            .with(|v| v.get())
            .expect("a stacked editor was built");
        let doc = live.doc.get();
        assert_eq!(doc.len(), 102);
        for row in [5, 15, 25] {
            assert!(doc.run_of(row).is_some(), "row {row} is a divider");
            assert_eq!(doc.divider_span(row), Some(row..row + 2));
        }

        // Scroll to between the second divider and the third, so that
        // Show whole opens runs above the view and below it.
        let (left, top) = pane_origin();
        harness.scroll_down(left + 300.0, top + 300.0, 18.0 * line_height);
        harness.rebuild();
        harness.paint();
        let before = viewport.get().y0;
        let first_row = (before / line_height).floor() as usize;
        assert!(
            (17..25).contains(&first_row),
            "the editor scrolled to row {first_row}, not between the dividers"
        );
        let first_line = doc
            .display_line(first_row)
            .expect("a line is first in view");
        let within = before - first_row as f64 * line_height;

        state.toggle_whole();
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        assert_eq!(doc.len(), 228, "every line, no divider");
        let row = doc.row_of_display(first_line).expect("every line is shown");
        assert_eq!(row, first_line);
        assert_eq!(
            viewport.get().y0,
            row as f64 * line_height + within,
            "the same line is first in view"
        );
        assert_eq!(laid_out_text(&live), doc.text);

        state.toggle_whole();
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        assert_eq!(doc.len(), 102);
        assert_eq!(doc.display_line(first_row), Some(first_line));
        assert_eq!(
            viewport.get().y0,
            before,
            "folded again, the line is where it was"
        );
    }

    #[test]
    fn all_takes_the_divider_with_it_and_reset_is_on_the_divider_above() {
        let (mut harness, state) = three_runs();
        let live = stacked(state);
        let line_height = crate::theme::code_line_height() as f64;
        let doc = live.doc.get();
        let second = doc.run_of(15).expect("row 15 is the second divider");
        assert_eq!(second.len(), 44);

        // All on the second run: its lines take its divider's rows, and
        // the document has two dividers left.
        let (first, last) = second.whole();
        state.reveal("src/x.rs", "", first, last);
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        assert_eq!(doc.len(), 102 - 2 + 44);
        assert_eq!(doc.folds.len(), 2);
        assert!(doc.run_of(15).is_none(), "the divider is gone");
        assert!(doc.run_of(5).is_some(), "the one above is not");

        // Nothing offers Fold again until the pointer is on the divider
        // above; there, it follows the hunk header over the code.
        let root = harness.root_id();
        assert!(find(root, "fold again").is_none());
        let editor = find(root, "tinted editor").expect("the code editor's box");
        let (_, top) = pane_origin();
        let y = top + 5.0 * line_height + line_height / 2.0;
        harness.pointer_move(editor.get_visual_rect_no_clip().x0 + 40.0, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(
            state.hovered_line.get(),
            Some(5),
            "the header records the pointer"
        );
        let reset =
            find(harness.root_id(), "fold again").expect("Fold again shows on the hovered divider");
        let (x, y) = (
            reset.get_visual_rect_no_clip().center().x,
            reset.get_visual_rect_no_clip().center().y,
        );
        harness.pointer_move(x, y);
        harness.rebuild();
        harness.paint();
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(
            live.doc.get().len(),
            102,
            "click at ({x}, {y}) over {under:?} did not fold the run again"
        );
        assert!(state.reveals_for("src/x.rs", "").is_empty());
        assert_eq!(laid_out_text(&live), live.doc.get().text);
    }

    #[test]
    fn a_reveal_above_the_cursor_moves_it_and_keeps_its_head_position() {
        let (mut harness, state) = one_long_run();
        let live = stacked(state);
        let cursor = super::STACKED_CURSOR
            .with(|c| c.get())
            .expect("a stacked editor was built");
        // The editor's cursor starts on row 0, a ghost, which is nowhere
        // in the head file.
        assert_eq!(state.last_position.get(), None);

        // The cursor placed as a click would place it: on "new bottom",
        // head line 61, three columns in, below the divider at rows 5
        // and 6.
        let doc = live.doc.get();
        let row = doc.row_of_head(61).expect("new bottom is shown");
        assert_eq!(row, 12);
        let offset = doc.offset_of_row(row) + 3;
        cursor.update(|c| c.set_offset(offset, CursorAffinity::Backward, false, false));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.last_position.get(), Some((61, 3)));
        assert_eq!(
            state.request_token.get(),
            0,
            "a click places the caret and asks the server nothing"
        );
        assert_eq!(
            state.cursor_line.get(),
            64,
            "new bottom is display line 64, after the pair's two ghosts"
        );

        // Up on the divider above: twenty lines come in above the cursor,
        // which stays on its text, twenty rows down. The symbol under it is
        // the one it was on, so its head position stands.
        let run = doc.run_of(5).expect("row 5 is the divider");
        let (first, last) = run.bottom();
        state.reveal("src/x.rs", "", first, last);
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        assert_eq!(doc.len(), 104);
        assert_eq!(doc.row_of_head(61), Some(32));
        assert_eq!(cursor.get().offset(), doc.offset_of_row(32) + 3);
        assert_eq!(state.cursor_line.get(), 64);
        assert_eq!(state.last_position.get(), Some((61, 3)));

        // Reset folds them away again, moving the cursor back up: the same.
        state.reset("src/x.rs", "", &[run.default_heads]);
        harness.rebuild();
        harness.paint();
        assert_eq!(cursor.get().offset(), offset);
        assert_eq!(state.last_position.get(), Some((61, 3)));

        // A click somewhere else is still a click: the position follows.
        let elsewhere = live.doc.get().offset_of_row(13);
        cursor.update(|c| c.set_offset(elsewhere, CursorAffinity::Backward, false, false));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.last_position.get(), Some((62, 0)));
    }

    #[test]
    fn the_split_keeps_both_sides_the_same_height_through_a_down_click() {
        let (mut harness, state) = three_runs();
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let (left, right) = sides(state);
        let line_height = crate::theme::code_line_height() as f64;

        // The split pairs each rewritten line with its old one, so the
        // sides are the 213 aligned rows less the three runs, plus their
        // dividers' two rows each, at the same rows on both.
        let (left_doc, right_doc) = (left.doc.get(), right.doc.get());
        assert_eq!(right_doc.len(), 213 - 3 * 44 + 3 * 2);
        assert_eq!(left_doc.len(), right_doc.len());
        assert_eq!(right_doc.hidden_lines(), 3 * 44);
        for row in [4, 13, 22] {
            assert_eq!(
                right_doc.run_of(row).map(|r| r.len()),
                left_doc.run_of(row).map(|r| r.len()),
                "row {row}"
            );
            assert!(right_doc.run_of(row).is_some(), "row {row} is a divider");
        }
        assert_eq!(laid_out_text(&left), left_doc.text);
        assert_eq!(laid_out_text(&right), right_doc.text);
        let (left_viewport, right_viewport) = super::SPLIT_VIEWPORTS
            .with(|v| v.get())
            .expect("a split was built");
        let y0 = right_viewport.get().y0;

        // The expanders are in the base side's gutter, at the row's left
        // end as on github.com: Up is the divider's second row, and opens
        // the run's last lines, which follow the divider.
        let (pane_left, top) = pane_origin();
        let right_gutter = pane_left + (1400.0 - pane_left - 1.0) / 2.0 + 1.0;
        let (x, y) = (
            pane_left + gutter_width() / 2.0,
            top + 5.0 * line_height + line_height / 2.0,
        );
        harness.pointer_move(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(
            state.hovered_line.get(),
            Some(4),
            "the base side's expander records the pointer as the divider's"
        );
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();

        let (left_doc, right_doc) = (left.doc.get(), right.doc.get());
        assert_eq!(
            right_doc.len(),
            87 + 20,
            "click at ({x}, {y}) over {under:?} did not reveal twenty lines"
        );
        assert_eq!(
            left_doc.len(),
            right_doc.len(),
            "the base side grew with the head side"
        );
        assert_eq!(
            right_doc.run_of(4).map(|r| r.len()),
            Some(24),
            "the divider stays, hiding twenty fewer"
        );
        assert_eq!(left_doc.run_of(4).map(|r| r.len()), Some(24));
        assert_eq!(
            right_doc.row_of_head(28),
            Some(6),
            "the first revealed head line follows the divider"
        );
        assert_eq!(
            left_doc.display_line(6),
            Some(28),
            "beside its own base line"
        );
        assert_eq!(
            laid_out_text(&left),
            left_doc.text,
            "the left editor's text is its document's"
        );
        assert_eq!(
            laid_out_text(&right),
            right_doc.text,
            "the right editor's text is its document's"
        );
        assert_eq!(right_viewport.get().y0, y0, "the viewport did not move");
        assert_eq!(
            left_viewport.get().y0,
            right_viewport.get().y0,
            "the left is where the right is"
        );

        // The head side's gutter has no expanders: the same click there
        // reveals nothing.
        let (x, y) = (right_gutter + gutter_width() / 2.0, y);
        harness.pointer_move(x, y);
        harness.rebuild();
        harness.paint();
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(
            right.doc.get().len(),
            107,
            "the head side's divider row is the tint alone"
        );
        assert_eq!(left.doc.get().len(), 107);

        // The bar's header is over the base side's editor, and records
        // the pointer as the expanders do; so does the rest of the bar,
        // over the head side, which is the tint alone.
        harness.pointer_move(pane_left + gutter_width() + 100.0, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(
            state.hovered_line.get(),
            Some(4),
            "the header over the base side records the row"
        );
        harness.pointer_move(right_gutter + gutter_width() + 100.0, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(
            state.hovered_line.get(),
            Some(4),
            "the bar over the head side records the row too"
        );

        // A jump into the second run reveals around the target on both
        // sides and scrolls both to it.
        assert_eq!(
            right.doc.get().row_of_head(80),
            None,
            "head line 80 is folded"
        );
        state.jump_to_line.set(Some(80));
        harness.rebuild();
        harness.paint();
        assert!(
            state.jump_to_line.get().is_none(),
            "the split's right editor took the jump"
        );
        let (left_doc, right_doc) = (left.doc.get(), right.doc.get());
        // The piece above the target is still longer than a step and
        // keeps both rows; the one below is All, a row of its own.
        assert_eq!(
            right_doc.len(),
            107 + 7 + 1,
            "seven lines and a second divider for the run's other piece"
        );
        assert_eq!(left_doc.len(), right_doc.len());
        let row = right_doc.row_of_head(80).expect("the target is shown");
        assert_eq!(left_doc.display_line(row), Some(80));
        assert_eq!(laid_out_text(&left), left_doc.text);
        assert_eq!(laid_out_text(&right), right_doc.text);
        assert_eq!(
            right_viewport.get().y0,
            (row as f64 - 4.0) * line_height,
            "scrolled to the target"
        );
        assert_eq!(left_viewport.get().y0, right_viewport.get().y0);
    }

    /// A key that moves through the file, as the window receives it:
    /// Page Up and Page Down bare, the arrows with Cmd.
    fn paging_key(paging: crate::review::model::Paging) -> floem::event::Event {
        use crate::review::model::Paging;
        use floem::prelude::{Code, Key, KeyState, KeyboardEvent, Modifiers, NamedKey};
        let (code, key, modifiers) = match paging {
            Paging::Up => (Code::PageUp, NamedKey::PageUp, Modifiers::empty()),
            Paging::Down => (Code::PageDown, NamedKey::PageDown, Modifiers::empty()),
            Paging::Top => (Code::ArrowUp, NamedKey::ArrowUp, crate::ui::OS_MOD),
            Paging::End => (Code::ArrowDown, NamedKey::ArrowDown, crate::ui::OS_MOD),
        };
        floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key: Key::Named(key),
            code,
            location: Default::default(),
            modifiers,
            repeat: false,
            is_composing: false,
        })
    }

    /// Cmd-Down puts the caret on the file's last line and its end at the
    /// foot of the pane; Cmd-Up puts both back at the top. The code has
    /// the keyboard, and still the window has the key first: the editor's
    /// own, which moves the caret alone, never runs.
    #[test]
    fn cmd_up_and_down_go_to_the_end_of_the_file_and_back_to_its_top() {
        use crate::review::model::Paging;
        let (mut harness, state) = one_long_run();
        let live = stacked(state);
        let line_height = crate::theme::code_line_height() as f64;
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).unwrap();
        let rows = live.doc.get().len();
        let last = (0..rows)
            .rev()
            .find_map(|r| live.doc.get().display_line(r))
            .unwrap();
        // A bare key is no shortcut to floem: with nothing focused it goes
        // nowhere. A reader has clicked the code.
        let (left, top) = pane_origin();
        harness.click(left + 200.0, top + 3.0 * line_height + line_height / 2.0);
        harness.rebuild();

        harness.dispatch_event(paging_key(Paging::End));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.paging.get(), None, "the editor took the key");
        assert_eq!(state.cursor_line.get(), last);
        // The editor keeps a line's room under the last row, and the
        // scroll stops at the end of that.
        assert_eq!(viewport.get().y1, (rows + 1) as f64 * line_height);

        harness.dispatch_event(paging_key(Paging::Top));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.cursor_line.get(), 0);
        assert_eq!(viewport.get().y0, 0.0);

        // Split, the right side takes the key and the left follows it.
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let (_, right) = sides(state);
        let (left_viewport, right_viewport) = super::SPLIT_VIEWPORTS.with(|v| v.get()).unwrap();
        let rows = right.doc.get().len();
        // A click on the base side gives its editor the keyboard, and the
        // window still has the key first.
        harness.click(left + 200.0, top + 3.0 * line_height + line_height / 2.0);
        harness.rebuild();
        harness.dispatch_event(paging_key(Paging::End));
        harness.rebuild();
        harness.paint();
        assert_eq!(
            state.paging.get(),
            None,
            "the base side's editor took the key"
        );
        assert_eq!(right_viewport.get().y1, (rows + 1) as f64 * line_height);
        assert_eq!(left_viewport.get().y0, right_viewport.get().y0);
    }

    /// Page Down scrolls a page of whole lines, less one kept in sight,
    /// and leaves the caret where it was; Page Up scrolls back. The
    /// editor's own, half a screen with the caret, never runs.
    #[test]
    fn the_page_keys_scroll_by_a_page() {
        use crate::review::model::Paging;
        let (mut harness, state) = one_long_run();
        let line_height = crate::theme::code_line_height() as f64;
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).unwrap();
        let (left, top) = pane_origin();
        harness.click(left + 200.0, top + 3.0 * line_height + line_height / 2.0);
        harness.rebuild();
        let caret = state.cursor_line.get();
        let page = ((viewport.get().height() / line_height).floor() - 1.0) * line_height;
        assert!(page > 0.0);

        harness.dispatch_event(paging_key(Paging::Down));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.paging.get(), None, "the editor took the key");
        assert_eq!(viewport.get().y0, page);
        assert_eq!(state.cursor_line.get(), caret, "the caret stayed put");

        // The next page is past the end, and the scroll stops there: the
        // editor keeps a line's room under the last row.
        let rows = stacked(state).doc.get().len();
        let end = (rows + 1) as f64 * line_height;
        assert!(end < viewport.get().y1 + page);
        harness.dispatch_event(paging_key(Paging::Down));
        harness.rebuild();
        harness.paint();
        assert_eq!(viewport.get().y1, end);

        let y0 = viewport.get().y0;
        harness.dispatch_event(paging_key(Paging::Up));
        harness.rebuild();
        harness.paint();
        assert_eq!(viewport.get().y0, y0 - page);
        assert_eq!(state.cursor_line.get(), caret);
    }

    #[test]
    fn a_jump_into_a_fold_reveals_around_the_target_before_scrolling() {
        let (mut harness, state) = one_long_run();
        let live = stacked(state);
        let line_height = crate::theme::code_line_height() as f64;
        assert_eq!(
            live.doc.get().row_of_head(30),
            None,
            "head line 30 is folded"
        );
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).unwrap();

        state.jump_to_line.set(Some(30));
        harness.rebuild();
        harness.paint();
        assert!(
            state.jump_to_line.get().is_none(),
            "the editor took the jump"
        );
        let doc = live.doc.get();
        // Three lines each side: head lines 27 to 33 are shown between
        // two dividers, and the run is two runs.
        assert!(state.reveals_for("src/x.rs", "").overlaps(27, 33));
        assert_eq!(doc.folds.len(), 2);
        for head in 27..=33 {
            assert!(
                doc.row_of_head(head).is_some(),
                "head line {head} is in view"
            );
        }
        assert_eq!(doc.row_of_head(26), None);
        assert_eq!(doc.row_of_head(34), None);
        let row = doc.row_of_head(30).unwrap();
        assert_eq!(row, 10, "after the two rows of the divider above");
        assert_eq!(laid_out_text(&live), doc.text);
        assert_eq!(
            viewport.get().y0,
            (row as f64 - 4.0) * line_height,
            "scrolled to the target"
        );

        // Reset on the divider above folds it again.
        state.reset("src/x.rs", "", &[doc.run_of(5).unwrap().default_heads]);
        harness.rebuild();
        harness.paint();
        assert_eq!(live.doc.get().len(), 84);
        assert_eq!(laid_out_text(&live), live.doc.get().text);
    }

    // Cards.

    /// A question and its answer on head lines `start` to `line` of the
    /// open file, one-based, both finished.
    fn thread_on(id: &str, start: u32, line: u32, outdated: bool) -> perch_github::CommentThread {
        use perch_github::{
            Anchor, Author, CommentMessage, CommentThread, RunRecord, RunStatus, RunStep, Side,
            ThreadOrigin,
        };
        let created_at = "2026-09-18T09:59:00Z".parse().unwrap();
        let run = RunRecord {
            id: format!("{id}-run"),
            owner: perch_github::RunOwner::Discussion {
                item_id: "wt".into(),
                thread_id: id.into(),
                request_id: format!("{id}-q"),
            },
            backend: "claude-code".into(),
            model: "sonnet".into(),
            effort: "medium".into(),
            session_id: Some("sess".into()),
            status: RunStatus::Completed,
            cost_usd: Some(0.03),
            duration_ms: Some(14_000),
            steps: vec![RunStep {
                summary: "Read src/x.rs".into(),
                ok: true,
                note: false,
                pending: false,
                user: false,
            }],
            error: None,
            started_at: Some("2026-09-18T10:00:00Z".parse().unwrap()),
            ended_at: Some("2026-09-18T10:00:14Z".parse().unwrap()),
            ..RunRecord::default()
        };
        let message =
            |suffix: &str, author: Author, body: &str, run: Option<RunRecord>| CommentMessage {
                id: format!("{id}-{suffix}"),
                thread_id: id.into(),
                origin: ThreadOrigin::Local,
                author,
                body: body.into(),
                created_at,
                run,
                github_id: None,
                node_id: None,
                html_url: None,
                diff_hunk: None,
                reactions: Default::default(),
            };
        CommentThread {
            id: id.into(),
            worktree_id: "wt".into(),
            origin: ThreadOrigin::Local,
            github_id: None,
            anchor: Some(Anchor {
                path: "src/x.rs".into(),
                start_line: start,
                line,
                side: Side::Right,
                commit_sha: "h".into(),
            }),
            outdated,
            resolved: false,
            node_id: None,
            can_resolve: false,
            created_at,
            messages: vec![
                message("m1", Author::User, "why?", None),
                message(
                    "m2",
                    Author::Agent {
                        backend: "claude-code".into(),
                        model: "sonnet".into(),
                    },
                    "because\n\n```rust\nfn x() {}\n```\n\n- one\n- two",
                    Some(run),
                ),
            ],
        }
    }

    /// A thread of the given message bodies, the reader's and the agent's
    /// in turn with no runs, on head lines `start` to `line` of the open
    /// file, one-based.
    fn thread_with(
        id: &str,
        start: u32,
        line: u32,
        bodies: &[&str],
    ) -> perch_github::CommentThread {
        use perch_github::{Anchor, Author, CommentMessage, CommentThread, Side, ThreadOrigin};
        let created_at = "2026-09-18T09:59:00Z".parse().unwrap();
        CommentThread {
            id: id.into(),
            worktree_id: "wt".into(),
            origin: ThreadOrigin::Local,
            github_id: None,
            anchor: Some(Anchor {
                path: "src/x.rs".into(),
                start_line: start,
                line,
                side: Side::Right,
                commit_sha: "h".into(),
            }),
            outdated: false,
            resolved: false,
            node_id: None,
            can_resolve: false,
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
                        Author::Agent {
                            backend: "claude-code".into(),
                            model: "sonnet".into(),
                        }
                    },
                    body: body.to_string(),
                    created_at,
                    run: None,
                    github_id: None,
                    node_id: None,
                    html_url: None,
                    diff_hunk: None,
                    reactions: Default::default(),
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

    /// How many views under `id`, itself included, have `text` in their
    /// debug name.
    fn count(id: floem::ViewId, text: &str) -> usize {
        usize::from(id.debug_name().contains(text))
            + id.children()
                .into_iter()
                .map(|c| count(c, text))
                .sum::<usize>()
    }

    /// The card layer's card for a thread: its box, relative to the layer,
    /// which sits over the editor at the code pane's origin past the
    /// gutter.
    fn card_rect(harness: &HeadlessHarness, thread_id: &str) -> floem::kurbo::Rect {
        let id = find(harness.root_id(), &format!("inline-card:{thread_id}"))
            .expect("the card is in the layer");
        harness.get_layout_rect(id)
    }

    /// The rows a thread's card takes as it has been laid out: what the
    /// correction gives it from its box, which the document is checked
    /// against.
    fn laid_out_rows(harness: &HeadlessHarness, thread_id: &str) -> usize {
        let rect = card_rect(harness, thread_id);
        super::rows_for_height(
            rect.height(),
            crate::theme::code_line_height() as f64,
            crate::theme::CARD_INSET,
        )
    }

    #[test]
    fn a_thread_in_the_open_file_reserves_rows_under_its_lines_and_its_card_sits_on_them() {
        use crate::review::doc::{CardId, DocRow};
        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = crate::theme::code_line_height() as f64;
        let viewport = super::STACKED_VIEWPORT
            .with(|v| v.get())
            .expect("a stacked editor was built");
        assert_eq!(
            live.doc.get().len(),
            120,
            "the buffer, with a ghost under every rewritten line"
        );
        assert!(
            find(harness.root_id(), "inline-card:").is_none(),
            "no thread, no card"
        );

        // A thread on head lines 5 to 7 arrives, as the backend sends it.
        // Line 7 is an `fn a`, display line 8; the ghost at 9 is the old
        // side of the rewritten line 8 after it, not something removed
        // under line 7, so the card goes between the two.
        let thread = thread_on("t1", 5, 7, false);
        let card = CardId::Thread("t1".into());
        let estimate = crate::review::doc::estimate_rows(&thread, false);
        state.set_comment_threads(Arc::new(vec![thread]));
        assert_eq!(
            live.doc.get().card_span(&card),
            Some(9..9 + estimate),
            "the estimate, until the card is laid out"
        );
        harness.rebuild();
        harness.paint();
        // Laid out, the card has the rows its height takes.
        let rows = laid_out_rows(&harness, "t1");
        let doc = live.doc.get();
        assert_eq!(
            doc.len(),
            120 + rows,
            "the card's rows were spliced in place"
        );
        assert_eq!(doc.display_line(8), Some(8));
        assert_eq!(state.active_file().unwrap().buffer.head_line(8), Some(6));
        assert_eq!(
            state.active_file().unwrap().buffer.head_line(9),
            None,
            "display line 9 is the ghost"
        );
        assert_eq!(doc.card_span(&card), Some(9..9 + rows));
        for row in 9..9 + rows {
            assert_eq!(
                doc.rows[row],
                DocRow::Spacer {
                    card: card.clone(),
                    row: row - 9
                }
            );
        }
        assert_eq!(
            doc.display_line(9 + rows),
            Some(9),
            "the code goes on under the card"
        );
        assert_eq!(laid_out_text(&live), doc.text);
        assert_eq!(viewport.get().y0, 0.0);

        // The card is drawn over its rows: its top is its first spacer
        // row's, set in by the inset, and its left edge the first code
        // column's.
        let rect = card_rect(&harness, "t1");
        assert_eq!(rect.y0, 9.0 * line_height + crate::theme::CARD_INSET);
        assert_eq!(rect.x0, crate::theme::CODE_GUTTER_GAP);
        assert!(
            rect.height() > line_height,
            "an open card is more than one row"
        );
        assert!(
            !state.is_collapsed(&card),
            "a thread that is not outdated starts open"
        );

        // Collapsed, the card is its rows, and the document is shorter by
        // the rows the card gave back.
        let folded = crate::review::doc::COLLAPSED_ROWS;
        state.toggle_card(&card);
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        assert_eq!(doc.len(), 120 + folded, "shrunk by the difference");
        assert_eq!(doc.card_span(&card), Some(9..9 + folded));
        assert_eq!(doc.display_line(9 + folded), Some(9));
        assert_eq!(laid_out_text(&live), doc.text);
        let rect = card_rect(&harness, "t1");
        let inset = crate::theme::COLLAPSED_CARD_INSET;
        assert_eq!(
            rect.y0,
            9.0 * line_height + inset,
            "a collapsed card sits inside its rows"
        );
        assert_eq!(rect.height(), folded as f64 * line_height - 2.0 * inset);

        // The chevron at the row's left opens it again: past the gutter,
        // the margin, the border and the row's padding, in its slot.
        let (left, top) = pane_origin();
        let x = left
            + gutter_width()
            + rect.x0
            + 1.0
            + crate::theme::COLLAPSED_CARD_PADDING
            + crate::theme::CARD_CHEVRON_SLOT / 2.0;
        let y = top + rect.y0 + rect.height() / 2.0;
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert!(
            !state.is_collapsed(&card),
            "click at ({x}, {y}) over {under:?} did not open the card"
        );
        assert_eq!(
            live.doc.get().len(),
            120 + rows,
            "open again at the rows the layout gave it, not the estimate"
        );
        assert_eq!(laid_out_text(&live), live.doc.get().text);

        // The thread going takes its rows with it.
        state.set_comment_threads(Arc::new(Vec::new()));
        harness.rebuild();
        harness.paint();
        assert_eq!(live.doc.get().len(), 120);
        assert!(find(harness.root_id(), "inline-card:").is_none());
    }

    /// A name on a collapsed card is a link to its page on GitHub, and
    /// the click opens the page, not the card. The reader's own row says
    /// "You" until GitHub has named them, and "You" leads nowhere.
    #[test]
    fn a_name_on_a_collapsed_card_is_a_link_to_its_page_on_github() {
        use crate::review::doc::CardId;
        let (mut harness, state) = harness();
        let opened = crate::ui::widgets::opened;
        opened();
        let click = |harness: &mut HeadlessHarness, name: &str| {
            let id = find(harness.root_id(), &format!("name label - Label: {name:?}"))
                .unwrap_or_else(|| panic!("no name {name:?}"));
            let c = id.get_visual_rect_no_clip().center();
            assert_eq!(
                harness.element_id_at(c.x, c.y).map(|e| e.owning_id()),
                Some(id),
                "{name} is not under the pointer at {c:?}"
            );
            harness.click(c.x, c.y);
            harness.rebuild();
            harness.paint();
        };

        // GitHub's thread on head line 7, collapsed to its row.
        state.set_comment_threads(Arc::new(vec![github_thread(100, 7, &["nit: name this"])]));
        harness.rebuild();
        harness.paint();
        let card = CardId::Thread("gh-100".into());
        state.toggle_card(&card);
        harness.rebuild();
        harness.paint();
        assert!(state.is_collapsed(&card));
        click(&mut harness, "octocat");
        assert_eq!(opened(), vec!["https://github.com/octocat".to_string()]);
        assert!(
            state.is_collapsed(&card),
            "the click on the name opened the card"
        );

        // The reader's own thread on the same lines, collapsed.
        state.set_comment_threads(Arc::new(vec![thread_on("t1", 5, 7, false)]));
        harness.rebuild();
        harness.paint();
        let card = CardId::Thread("t1".into());
        state.toggle_card(&card);
        harness.rebuild();
        harness.paint();
        assert!(state.is_collapsed(&card));
        click(&mut harness, "You");
        assert_eq!(opened(), Vec::<String>::new(), "You led somewhere");
        assert!(
            state.is_collapsed(&card),
            "the click on You opened the card"
        );
    }

    #[test]
    fn a_line_with_a_thread_offers_a_plus_that_stacks_a_second_thread_under_it() {
        use crate::review::doc::CardId;
        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = crate::theme::code_line_height() as f64;
        state.set_comment_threads(Arc::new(vec![thread_on("t1", 5, 7, false)]));
        harness.rebuild();
        harness.paint();
        // Head line 7, the thread's last, is on row 8 of a document with
        // nothing folded above it, and its card under that.
        let (left, top) = pane_origin();
        assert_eq!(live.doc.get().row_of_head(6), Some(8));
        let (x, y) = (
            left + crate::theme::GUTTER_SLOT_INSET + crate::theme::GUTTER_SLOT / 2.0,
            top + 8.0 * line_height + line_height / 2.0,
        );
        harness.pointer_move(x, y);
        harness.rebuild();
        harness.paint();
        let plus = find(harness.root_id(), "gutter-plus")
            .expect("a line with a thread still offers a question")
            .get_visual_rect_no_clip();
        harness.click(plus.center().x, plus.center().y);
        harness.rebuild();
        harness.paint();
        let composer = state.composer.get().expect("the composer opened");
        assert_eq!(composer.thread_id, None, "a new thread, not a follow-up");
        let anchor = composer.anchor.expect("on the line");
        assert_eq!(
            (anchor.side, anchor.start_line, anchor.line),
            (perch_github::Side::Right, 7, 7)
        );
        assert_eq!(state.focus_card.get(), None, "nothing was focused");

        // The composer's card stacks under the thread's, under the line.
        let doc = live.doc.get();
        let thread = doc
            .card_span(&CardId::Thread("t1".into()))
            .expect("the thread's card");
        let composer = doc
            .card_span(&CardId::Composer)
            .expect("the composer's card");
        assert_eq!(thread.start, 9);
        assert_eq!(composer.start, thread.end);
    }

    #[test]
    fn a_focused_card_scrolls_into_view() {
        use crate::review::doc::CardId;
        let (mut harness, state) = harness();
        stacked(state);
        let line_height = crate::theme::code_line_height() as f64;
        let viewport = super::STACKED_VIEWPORT.with(|v| v.get()).unwrap();
        state.set_comment_threads(Arc::new(vec![thread_on("t1", 5, 7, false)]));
        harness.rebuild();
        harness.paint();
        let card = CardId::Thread("t1".into());
        let (left, top) = pane_origin();

        // Scrolled past the card, focusing it brings it back into view
        // with the line above it.
        harness.scroll_down(left + 300.0, top + 300.0, 40.0 * line_height);
        harness.rebuild();
        harness.paint();
        assert!(
            viewport.get().y0 > 20.0 * line_height,
            "the editor did not scroll past the card"
        );
        state.focus_card.set(Some(card));
        harness.rebuild();
        harness.paint();
        assert_eq!(
            viewport.get().y0,
            8.0 * line_height,
            "the anchor's last line, above the card, is the first row in view"
        );
    }

    #[test]
    fn a_split_draws_a_card_on_its_anchors_side_and_fills_the_other() {
        use crate::review::doc::{CardId, DocRow};
        let (mut harness, state) = harness();
        let line_height = crate::theme::code_line_height() as f64;
        let thread = thread_on("t1", 5, 7, false);
        state.set_comment_threads(Arc::new(vec![thread]));
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let (left, right) = sides(state);
        let card = CardId::Thread("t1".into());
        // The card on the head side corrects both sides' rows: its own on
        // the right, and fillers beside them on the left.
        let rows = laid_out_rows(&harness, "t1");
        let (left_doc, right_doc) = (left.doc.get(), right.doc.get());
        assert_eq!(left_doc.len(), right_doc.len());
        assert_eq!(right_doc.len(), 90 + rows);
        // The split pairs each ghost with its line, so display line 9,
        // the ghost, is on the next line's row: head line 7 is row 6, and
        // the card follows it, before that row, as it is before the ghost
        // in the stacked layout.
        assert_eq!(right_doc.row_of_head(6), Some(6));
        assert_eq!(right_doc.card_span(&card), Some(7..7 + rows));
        assert_eq!(left_doc.card_span(&card), None);
        assert!(left_doc.rows[7..7 + rows]
            .iter()
            .all(|r| *r == DocRow::Filler));
        assert_eq!(laid_out_text(&left), left_doc.text);
        assert_eq!(laid_out_text(&right), right_doc.text);

        // The card is over the head side, from its first code column,
        // past its gutter, to the pane's edge less the margin.
        let rect = card_rect(&harness, "t1");
        assert_eq!(rect.y0, 7.0 * line_height + crate::theme::CARD_INSET);
        assert_eq!(rect.x0, gutter_width() + crate::theme::CODE_GUTTER_GAP);
        let (pane_left, _) = pane_origin();
        let right_gutter = pane_left + (1400.0 - pane_left - 1.0) / 2.0 + 1.0;
        let on_screen = |harness: &HeadlessHarness, id: &str| {
            find(harness.root_id(), &format!("inline-card:{id}"))
                .expect("the card is drawn")
                .get_visual_rect_no_clip()
        };
        let window = on_screen(&harness, "t1");
        assert_eq!(
            window.x0,
            right_gutter + gutter_width() + crate::theme::CODE_GUTTER_GAP
        );
        assert_eq!(window.x1, 1400.0 - crate::theme::CODE_GUTTER_GAP);

        // A thread on base line 2, which the change rewrote, is on the
        // base side: under its row, the second, on the left, with fillers
        // beside it on the right, and drawn over the base side.
        let mut on_base = thread_on("t2", 2, 2, false);
        on_base.anchor.as_mut().unwrap().side = perch_github::Side::Left;
        state.set_comment_threads(Arc::new(vec![on_base]));
        harness.rebuild();
        harness.paint();
        let card = CardId::Thread("t2".into());
        let rows = laid_out_rows(&harness, "t2");
        let (left_doc, right_doc) = (left.doc.get(), right.doc.get());
        assert_eq!(left_doc.len(), 90 + rows);
        assert_eq!(right_doc.len(), left_doc.len());
        assert_eq!(left_doc.card_span(&card), Some(2..2 + rows));
        assert_eq!(right_doc.card_span(&card), None);
        assert!(right_doc.rows[2..2 + rows]
            .iter()
            .all(|r| *r == DocRow::Filler));
        assert_eq!(laid_out_text(&left), left_doc.text);
        assert_eq!(laid_out_text(&right), right_doc.text);
        let window = on_screen(&harness, "t2");
        assert_eq!(
            window.y0,
            pane_origin().1 + 2.0 * line_height + crate::theme::CARD_INSET
        );
        assert_eq!(
            window.x0,
            pane_left + gutter_width() + crate::theme::CODE_GUTTER_GAP
        );
        assert_eq!(
            window.x1,
            right_gutter - 1.0 - crate::theme::CODE_GUTTER_GAP,
            "to the base side's edge, less the margin"
        );

        // Collapsing it shrinks both sides by the difference.
        state.toggle_card(&card);
        harness.rebuild();
        harness.paint();
        let folded = crate::review::doc::COLLAPSED_ROWS;
        assert_eq!(right.doc.get().len(), 90 + folded);
        assert_eq!(left.doc.get().len(), 90 + folded);
        assert_eq!(laid_out_text(&left), left.doc.get().text);
        assert_eq!(laid_out_text(&right), right.doc.get().text);
    }

    #[test]
    fn a_removed_line_on_the_base_side_of_a_split_takes_a_question_there() {
        use crate::review::doc::CardId;
        let (mut harness, state) = harness();
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let line_height = crate::theme::code_line_height() as f64;
        let (pane_left, top) = pane_origin();
        let right_gutter = pane_left + (1400.0 - pane_left - 1.0) / 2.0 + 1.0;
        let slot = crate::theme::GUTTER_SLOT_INSET + crate::theme::GUTTER_SLOT / 2.0;
        let row = |n: f64| top + n * line_height + line_height / 2.0;
        let hover = |harness: &mut HeadlessHarness, x: f64, y: f64| {
            harness.pointer_move(x, y);
            harness.rebuild();
            harness.paint();
        };

        // The first row is a line the change kept: the base offers
        // nothing on it, and the head does.
        hover(&mut harness, pane_left + slot, row(0.0));
        assert_eq!(count(harness.root_id(), "gutter-plus"), 0);
        hover(&mut harness, right_gutter + slot, row(0.0));
        assert_eq!(count(harness.root_id(), "gutter-plus"), 1);

        // The second is base line 2, which the change rewrote. The base's
        // row offers a question, and the head's beside it does not light.
        hover(&mut harness, pane_left + slot, row(1.0));
        assert_eq!(count(harness.root_id(), "gutter-plus"), 1);
        let plus = find(harness.root_id(), "gutter-plus")
            .expect("the base row offers a question")
            .get_visual_rect_no_clip();
        assert!(
            plus.x1 <= pane_left + gutter_width(),
            "in the base side's gutter: {plus:?}"
        );
        harness.click(plus.center().x, plus.center().y);
        harness.rebuild();
        harness.paint();
        let composer = state.composer.get().expect("the composer opened");
        assert_eq!(
            composer.anchor,
            Some(perch_github::Anchor {
                path: "src/x.rs".into(),
                start_line: 2,
                line: 2,
                side: perch_github::Side::Left,
                commit_sha: "h".into(),
            })
        );

        // Its card is under the row, on the base side.
        let (left, right) = sides(state);
        let span = left
            .doc
            .get()
            .card_span(&CardId::Composer)
            .expect("the composer is a card on the left");
        assert_eq!(span.start, 2);
        assert_eq!(right.doc.get().card_span(&CardId::Composer), None);
        let card = find(harness.root_id(), "inline-card:composer")
            .expect("the composer's card is drawn")
            .get_visual_rect_no_clip();
        assert!(card.x1 < right_gutter, "over the base side: {card:?}");
    }

    /// A selection on the base side is of base lines, kept ones too: the
    /// `+` on a removed line in it, and Ask, anchor a question on all of
    /// them there. The head's `+` reads the head's selection, not the
    /// base's, and a selection on the head is the head's again.
    #[test]
    fn a_selection_on_the_base_side_anchors_a_question_on_its_base_lines() {
        use perch_github::Side::{Left, Right};
        let (mut harness, state) = harness();
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        let line_height = crate::theme::code_line_height() as f64;
        let (pane_left, top) = pane_origin();
        let right_gutter = pane_left + (1400.0 - pane_left - 1.0) / 2.0 + 1.0;
        let code = gutter_width() + crate::theme::CODE_GUTTER_GAP + 2.0;
        let slot = crate::theme::GUTTER_SLOT_INSET + crate::theme::GUTTER_SLOT / 2.0;
        let row = |n: f64| top + n * line_height + line_height / 2.0;
        let lines = |a: Option<perch_github::Anchor>| a.map(|a| (a.side, a.start_line, a.line));
        let plus = |harness: &mut HeadlessHarness, x: f64, y: f64| {
            harness.pointer_move(x, y);
            harness.rebuild();
            harness.paint();
            let at = find(harness.root_id(), "gutter-plus")
                .expect("the row offers a question")
                .get_visual_rect_no_clip()
                .center();
            harness.click(at.x, at.y);
            harness.rebuild();
            harness.paint();
            state.composer.get().and_then(|c| c.anchor)
        };

        // fn old, which the change rewrote, and fn c, which it kept: base
        // lines 2 and 3.
        drag(
            &mut harness,
            (pane_left + code, row(1.0)),
            (pane_left + code + 30.0, row(2.0)),
        );
        assert_eq!(state.selection_base.get(), Some((1, 2)));
        assert_eq!(lines(state.anchor_from_selection()), Some((Left, 2, 3)));

        // The head's `+` beside fn c is on fn c alone.
        assert_eq!(
            lines(plus(&mut harness, right_gutter + slot, row(2.0))),
            Some((Right, 3, 3))
        );
        state.close_composer();
        harness.rebuild();
        harness.paint();

        // The base's `+` on fn old takes the whole selection.
        assert_eq!(
            lines(plus(&mut harness, pane_left + slot, row(1.0))),
            Some((Left, 2, 3))
        );
        state.close_composer();
        harness.rebuild();
        harness.paint();

        // A selection on the head side is the head's.
        drag(
            &mut harness,
            (right_gutter + code, row(0.0)),
            (right_gutter + code + 30.0, row(1.0)),
        );
        assert_eq!(state.selection_base.get(), None);
        assert_eq!(lines(state.anchor_from_selection()), Some((Right, 1, 2)));
    }

    #[test]
    fn a_ghost_in_the_stacked_layout_takes_a_question_on_the_base() {
        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = crate::theme::code_line_height() as f64;
        let (pane_left, top) = pane_origin();
        // Display line 1 is the ghost of base line 2.
        assert_eq!(live.doc.get().display_line(1), Some(1));
        let (x, y) = (
            pane_left + crate::theme::GUTTER_SLOT_INSET + crate::theme::GUTTER_SLOT / 2.0,
            top + line_height * 1.5,
        );
        harness.pointer_move(x, y);
        harness.rebuild();
        harness.paint();
        let plus = find(harness.root_id(), "gutter-plus")
            .expect("a ghost offers a question")
            .get_visual_rect_no_clip();
        harness.click(plus.center().x, plus.center().y);
        harness.rebuild();
        harness.paint();
        let composer = state.composer.get().expect("the composer opened");
        let anchor = composer.anchor.expect("on the ghost's line");
        assert_eq!(
            (anchor.side, anchor.start_line, anchor.line),
            (perch_github::Side::Left, 2, 2)
        );
        assert!(
            state.composer_is_inline(),
            "its card is under the ghost, in the code"
        );

        // The selection of nothing but ghosts is the base's lines too, and
        // one that takes in a head line is the head's.
        let file = state.active_file().expect("the file is open");
        let on = |first, last| {
            state
                .anchor_for_display_range(&file, first, last)
                .map(|a| (a.side, a.start_line, a.line))
        };
        assert_eq!(on(1, 1), Some((perch_github::Side::Left, 2, 2)));
        assert_eq!(on(1, 2), Some((perch_github::Side::Right, 2, 2)));
        assert_eq!(on(0, 2), Some((perch_github::Side::Right, 1, 2)));
    }

    // The height correction.

    /// A question alone is estimated at eight rows: its one line, the
    /// byline and the gap, and five for the header, the Follow up box and
    /// the foot. On the screen the comment's padding and the chrome are
    /// more than the estimate charges, so the laid-out card is always
    /// taller than eight rows. The assertions are on the rows the layout
    /// gave, so they hold wherever the fonts put it.
    #[test]
    fn a_card_laid_out_taller_than_its_rows_moves_the_lines_below_it_and_nothing_else() {
        use crate::review::doc::CardId;
        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = crate::theme::code_line_height() as f64;
        let viewport = super::STACKED_VIEWPORT
            .with(|v| v.get())
            .expect("a stacked editor was built");
        let card = CardId::Thread("t1".into());

        // The thread arrives, and the document is built from the estimate
        // before anything is laid out: the card is under row 8, and the
        // line under the card is eight rows on.
        let thread = thread_with("t1", 5, 7, &["why?"]);
        let estimate = crate::review::doc::estimate_rows(&thread, false);
        assert_eq!(estimate, 8);
        state.set_comment_threads(Arc::new(vec![thread]));
        let before = live.doc.get();
        assert_eq!(before.len(), 120 + estimate);
        assert_eq!(before.card_span(&card), Some(9..9 + estimate));
        assert_eq!(before.row_of_display(9), Some(9 + estimate));
        assert!(
            state.card_rows.get().is_empty(),
            "nothing has been laid out"
        );

        // Laid out, the card says how tall it is, and the document has
        // the rows that takes: the lines above the card are where they
        // were, the lines below it are further down by the difference,
        // and the view, which the card is inside, did not move.
        harness.rebuild();
        harness.paint();
        let rows = laid_out_rows(&harness, "t1");
        assert!(
            rows > estimate,
            "the chrome is more than the estimate's: {rows} rows for {}px",
            card_rect(&harness, "t1").height()
        );
        let grew = rows - estimate;
        let after = live.doc.get();
        assert_eq!(
            state.card_rows.get().get(&card),
            Some(&rows),
            "the layout's answer is kept"
        );
        assert_eq!(after.len(), 120 + rows);
        assert_eq!(after.card_span(&card), Some(9..9 + rows));
        assert_eq!(
            after.rows[..9],
            before.rows[..9],
            "the lines above the card are where they were"
        );
        assert_eq!(
            after.row_of_display(9),
            Some(9 + estimate + grew),
            "the line under the card is {grew} rows further down"
        );
        assert_eq!(after.row_of_display(119), Some(119 + estimate + grew));
        assert_eq!(laid_out_text(&live), after.text);
        assert_eq!(
            viewport.get().y0,
            0.0,
            "the card is in view, so the view did not move"
        );
        let rect = card_rect(&harness, "t1");
        assert_eq!(
            rect.y0,
            9.0 * line_height + crate::theme::CARD_INSET,
            "the card is where it was"
        );

        // Every scroll lays the card out again at the same height, and
        // it says nothing new: the document is the one it was.
        let (left, top) = pane_origin();
        harness.scroll_down(left + 300.0, top + 300.0, 3.0 * line_height);
        harness.rebuild();
        harness.paint();
        assert_eq!(viewport.get().y0, 3.0 * line_height, "the editor scrolled");
        assert!(
            Arc::ptr_eq(&live.doc.get(), &after),
            "the document was not built again"
        );
        assert_eq!(state.card_rows.get().get(&card), Some(&rows));
    }

    #[test]
    fn a_card_above_the_viewport_growing_keeps_the_same_first_visible_line() {
        use crate::review::doc::CardId;
        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = crate::theme::code_line_height() as f64;
        let viewport = super::STACKED_VIEWPORT
            .with(|v| v.get())
            .expect("a stacked editor was built");
        let card = CardId::Thread("t1".into());

        // The question arrives and is laid out; then the editor is
        // scrolled well past its card.
        state.set_comment_threads(Arc::new(vec![thread_with("t1", 5, 7, &["why?"])]));
        harness.rebuild();
        harness.paint();
        let rows = live
            .doc
            .get()
            .card_span(&card)
            .expect("the card has rows")
            .len();
        assert_eq!(rows, laid_out_rows(&harness, "t1"));
        let (left, top) = pane_origin();
        harness.scroll_down(left + 300.0, top + 300.0, 40.0 * line_height);
        harness.rebuild();
        harness.paint();
        let before = viewport.get().y0;
        let first_row = (before / line_height).floor() as usize;
        assert!(
            first_row > 9 + rows,
            "the editor did not scroll past the card: row {first_row} is first in view"
        );
        let first_line = live
            .doc
            .get()
            .display_line(first_row)
            .expect("a line is first in view");

        // The answer lands: the card is built again for the thread's two
        // messages and is a line and a gap taller, which is more than a
        // row. The rows it gains are above the view, so the scroll moves
        // by them and the same line is first in view.
        state.set_comment_threads(Arc::new(vec![thread_with(
            "t1",
            5,
            7,
            &["why?", "because"],
        )]));
        harness.rebuild();
        harness.paint();
        let grown = laid_out_rows(&harness, "t1");
        assert!(
            grown > rows,
            "the answer did not grow the card: {rows} rows to {grown}"
        );
        let grew = grown - rows;
        let doc = live.doc.get();
        assert_eq!(doc.card_span(&card), Some(9..9 + grown));
        assert_eq!(doc.len(), 120 + grown);
        assert_eq!(
            viewport.get().y0,
            before + grew as f64 * line_height,
            "the scroll moved by the rows added above it"
        );
        assert_eq!(
            doc.display_line(first_row + grew),
            Some(first_line),
            "the same line is first in view"
        );
        assert_eq!(laid_out_text(&live), doc.text);
    }

    #[test]
    fn a_long_card_is_as_tall_as_its_thread_and_takes_the_rows_for_it() {
        use crate::review::doc::CardId;
        let (mut harness, state) = harness();
        let live = stacked(state);
        let card = CardId::Thread("t1".into());

        // A hundred-line answer does not scroll inside its card: the card
        // is as tall as every line, and the document gives it the rows.
        let answer = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let thread = thread_with("t1", 5, 7, &["why?", &answer]);
        state.set_comment_threads(Arc::new(vec![thread]));
        harness.rebuild();
        harness.paint();
        let rows = laid_out_rows(&harness, "t1");
        assert!(rows > 100, "every line has room: {rows} rows");
        let doc = live.doc.get();
        assert_eq!(doc.card_span(&card), Some(9..9 + rows));
        assert_eq!(doc.len(), 120 + rows);
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

        // Follow up: the composer's box takes the Follow up box's place
        // inside the card, which grows around it, and the document with
        // it. The box is never focused here; see `composer_autofocus`.
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        state.open_composer(
            Some("t1".into()),
            thread.anchor.clone(),
            String::new(),
            choice,
        );
        harness.rebuild();
        harness.paint();
        let grown = laid_out_rows(&harness, "t1");
        assert!(
            grown > rows,
            "the box is taller than the line it replaced: {rows} rows to {grown}"
        );
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

    #[test]
    fn a_follow_up_composer_is_drawn_inline_then_falls_back_to_conversation() {
        let (mut harness, state) = harness();
        let thread = thread_with("t1", 5, 7, &["why?", "because"]);
        state.set_comment_threads(Arc::new(vec![thread.clone()]));
        harness.rebuild();
        harness.paint();
        // The thread is drawn in the code, with its Follow up box and no
        // composer.
        assert_eq!(count(harness.root_id(), "composer-box"), 0);
        assert_eq!(count(harness.root_id(), "Follow up…"), 1);

        // Following up puts one box inside the card in the code. The box is
        // never focused here; see `composer_autofocus`.
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        state.open_composer(
            Some("t1".into()),
            thread.anchor.clone(),
            String::new(),
            choice,
        );
        harness.rebuild();
        harness.paint();
        assert_eq!(
            count(harness.root_id(), "composer-box"),
            1,
            "one box, not one per face"
        );
        let card = find(harness.root_id(), "inline-card:t1").expect("the card is in the layer");
        assert_eq!(
            count(card, "composer-box"),
            1,
            "inside the card in the code"
        );
        assert_eq!(count(harness.root_id(), "Follow up…"), 0);
        assert_eq!(state.tab.get(), ItemTab::Code);
        assert!(state.composer_in_code());

        // With the thread's file closed there is no inline card for the box,
        // so the open composer moves to the thread on Conversation.
        state.close_tab("src/x.rs");
        harness.rebuild();
        harness.paint();
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert!(!state.composer_in_code());
        assert!(find(harness.root_id(), "inline-card:").is_none());
        assert_eq!(count(harness.root_id(), "composer-box"), 1);
        assert_eq!(count(harness.root_id(), "Follow up…"), 0);
    }

    #[test]
    fn a_run_folded_again_keeps_the_composers_lines_and_its_card_in_view() {
        use crate::review::doc::CardId;
        let (mut harness, state) = one_long_run();
        let live = stacked(state);
        let line_height = crate::theme::code_line_height() as f64;
        let run = live.doc.get().run_of(5).expect("row 5 is the divider");

        // Up on the divider shows the run's last twenty lines, as the
        // divider's own test has it: display line 46, `line 44`, is row 14.
        let (left, top) = pane_origin();
        let (x, y) = (
            left + gutter_width() / 2.0,
            top + 6.0 * line_height + line_height / 2.0,
        );
        harness.pointer_move(x, y);
        harness.rebuild();
        harness.paint();
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(live.doc.get().row_of_display(46), Some(14));

        // A question on it: its card is under it, and its line is an
        // anchor as a thread's would be.
        let anchor = perch_github::Anchor {
            path: "src/x.rs".into(),
            start_line: 46,
            line: 46,
            side: perch_github::Side::Right,
            commit_sha: "h".into(),
        };
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        state.open_composer(None, Some(anchor), "why?".into(), choice);
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        let rows = doc
            .card_span(&CardId::Composer)
            .expect("the composer is a card")
            .len();
        assert_eq!(doc.card_span(&CardId::Composer), Some(15..15 + rows));
        assert!(find(harness.root_id(), "inline-card:composer").is_some());
        assert_eq!(state.anchors_in("src/x.rs"), vec![(45, 45)]);

        // Reset folds the run back to its default, which is cut around
        // the composer's line as it would be around a thread's: the line,
        // its context and the card stay in view, with a divider either
        // side, rather than the run closing over the card.
        state.reset("src/x.rs", "", &[run.default_heads]);
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        assert_eq!(
            doc.run_of(5).map(|r| (r.first, r.last)),
            Some((5, 42)),
            "the run above, to the context before the line"
        );
        assert_eq!(
            doc.row_of_display(46),
            Some(10),
            "after the two rows of the divider above"
        );
        assert_eq!(doc.card_span(&CardId::Composer), Some(11..11 + rows));
        assert_eq!(
            doc.run_of(11 + rows + 3).map(|r| (r.first, r.last)),
            Some((50, 58)),
            "the run below, from the context after it"
        );
        assert_eq!(laid_out_text(&live), doc.text);
        assert!(
            find(harness.root_id(), "inline-card:composer").is_some(),
            "the card is drawn"
        );
        assert!(state.composer_is_inline());
        assert_eq!(state.composer_draft.get(), "why?");

        // Sent, the placeholder holds the line open the same way until
        // the thread arrives with an anchor of its own.
        state.composer_sent();
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        assert_eq!(doc.row_of_display(46), Some(10));
        assert_eq!(doc.card_span(&CardId::Composer), Some(11..11 + rows));
        assert_eq!(doc.run_of(5).map(|r| (r.first, r.last)), Some((5, 42)));
    }

    // GitHub's threads and the composer as cards.

    /// One of GitHub's threads on head line `line` of the open file,
    /// one-based, on the head side: a comment by octocat and, for each
    /// body after the first, a reply by hubot.
    fn github_thread(id: u64, line: u32, bodies: &[&str]) -> perch_github::CommentThread {
        use perch_github::{Anchor, Author, CommentMessage, CommentThread, Side, ThreadOrigin};
        let created_at: jiff::Timestamp = "2026-09-18T09:59:00Z".parse().unwrap();
        let thread_id = format!("gh-{id}");
        CommentThread {
            id: thread_id.clone(),
            worktree_id: "wt".into(),
            origin: ThreadOrigin::GitHub,
            github_id: Some(id),
            anchor: Some(Anchor {
                path: "src/x.rs".into(),
                start_line: line,
                line,
                side: Side::Right,
                commit_sha: "h".into(),
            }),
            outdated: false,
            resolved: false,
            node_id: None,
            can_resolve: false,
            created_at,
            messages: bodies
                .iter()
                .enumerate()
                .map(|(i, body)| CommentMessage {
                    id: format!("gh-{}", id + i as u64),
                    thread_id: thread_id.clone(),
                    origin: ThreadOrigin::GitHub,
                    author: Author::GitHubUser(if i == 0 { "octocat" } else { "hubot" }.into()),
                    body: body.to_string(),
                    created_at: created_at + jiff::SignedDuration::from_secs(60 * i as i64),
                    run: None,
                    github_id: Some(id + i as u64),
                    node_id: Some(format!("PRRC_{}", id + i as u64)),
                    html_url: Some(format!(
                        "https://github.com/o/r/pull/1#discussion_r{}",
                        id + i as u64
                    )),
                    diff_hunk: (i == 0).then(|| "@@ -5,3 +5,3 @@\n a\n b\n c".to_string()),
                    reactions: Default::default(),
                })
                .collect(),
        }
    }

    #[test]
    fn a_github_thread_is_a_card_under_its_line_marked_by_a_hollow_marker() {
        use crate::review::doc::{CardId, DocRow};
        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = crate::theme::code_line_height() as f64;
        let viewport = super::STACKED_VIEWPORT
            .with(|v| v.get())
            .expect("a stacked editor was built");
        assert_eq!(live.doc.get().len(), 120);

        // A comment and a reply on head line 7, display line 8, arrive
        // with the threads: the card goes under the line, before the
        // ghost of the rewritten line after it, as the reader's does.
        let thread = github_thread(100, 7, &["nit: name this", "done"]);
        let card = CardId::Thread("gh-100".into());
        let estimate = crate::review::doc::estimate_rows(&thread, false);
        state.set_comment_threads(Arc::new(vec![thread]));
        assert_eq!(
            live.doc.get().card_span(&card),
            Some(9..9 + estimate),
            "the estimate, until the card is laid out"
        );
        assert!(
            !state.is_collapsed(&card),
            "not resolved, so it starts open"
        );
        harness.rebuild();
        harness.paint();
        let rows = laid_out_rows(&harness, "gh-100");
        let doc = live.doc.get();
        assert_eq!(doc.len(), 120 + rows);
        assert_eq!(doc.card_span(&card), Some(9..9 + rows));
        for row in 9..9 + rows {
            assert_eq!(
                doc.rows[row],
                DocRow::Spacer {
                    card: card.clone(),
                    row: row - 9
                }
            );
        }
        assert_eq!(
            doc.display_line(9 + rows),
            Some(9),
            "the code goes on under the card"
        );
        assert_eq!(laid_out_text(&live), doc.text);
        let rect = card_rect(&harness, "gh-100");
        assert_eq!(rect.y0, 9.0 * line_height + crate::theme::CARD_INSET);
        assert_eq!(rect.x0, crate::theme::CODE_GUTTER_GAP);
        assert!(rect.height() > line_height);

        // The comment carries its author's name, and the reply, its own
        // author's, draws under it.
        let root = find(harness.root_id(), "name label - Label: \"octocat\"")
            .expect("the comment's byline names octocat");
        let reply = find(harness.root_id(), "name label - Label: \"hubot\"")
            .expect("the reply's byline names hubot");
        assert!(
            reply.get_visual_rect_no_clip().y0 > root.get_visual_rect_no_clip().y0,
            "the reply is under the comment"
        );

        // The pointer on the comment's line trades its hollow marker for
        // the `+`, which asks another question there rather than focusing
        // the card.
        let (left, top) = pane_origin();
        harness.pointer_move(
            left + crate::theme::GUTTER_SLOT_INSET + crate::theme::GUTTER_SLOT / 2.0,
            top + 8.0 * line_height + line_height / 2.0,
        );
        harness.rebuild();
        harness.paint();
        assert_eq!(count(harness.root_id(), "gutter-plus"), 1);
        assert_eq!(state.focus_card.get(), None);
        assert_eq!(
            viewport.get().y0,
            0.0,
            "the card is in view, so nothing scrolled"
        );

        // Collapsed, it is one strip: the chevron, who commented, the
        // first line and how many replies follow.
        let folded = crate::review::doc::COLLAPSED_ROWS;
        let inset = crate::theme::COLLAPSED_CARD_INSET;
        state.toggle_card(&card);
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        assert_eq!(doc.len(), 120 + folded);
        assert_eq!(doc.card_span(&card), Some(9..9 + folded));
        assert_eq!(laid_out_text(&live), doc.text);
        let rect = card_rect(&harness, "gh-100");
        assert_eq!(rect.y0, 9.0 * line_height + inset);
        assert_eq!(rect.height(), folded as f64 * line_height - 2.0 * inset);
        state.toggle_card(&card);
        harness.rebuild();
        harness.paint();
        assert_eq!(
            live.doc.get().len(),
            120 + rows,
            "open again at the rows the layout gave it"
        );

        // A thread of the reader's own on the same line, asked earlier,
        // goes before it.
        let mut mine = thread_with("t1", 5, 7, &["why?"]);
        mine.created_at = "2026-09-18T09:00:00Z".parse().unwrap();
        state.set_comment_threads(Arc::new(vec![
            mine,
            github_thread(100, 7, &["nit: name this", "done"]),
        ]));
        harness.rebuild();
        harness.paint();
        let doc = live.doc.get();
        let mine = doc
            .card_span(&CardId::Thread("t1".into()))
            .expect("the reader's card has rows");
        assert_eq!(mine.start, 9);
        assert_eq!(doc.card_span(&card), Some(mine.end..mine.end + rows));
        assert_eq!(laid_out_text(&live), doc.text);

        // The thread going takes the card and its rows with it.
        state.set_comment_threads(Arc::new(Vec::new()));
        harness.rebuild();
        harness.paint();
        assert_eq!(live.doc.get().card_span(&card), None);
        assert!(find(harness.root_id(), "inline-card:gh-").is_none());
    }

    #[test]
    fn ask_about_this_on_a_github_comment_opens_the_composer_under_its_thread() {
        let (mut harness, state) = harness();
        state.set_comment_threads(Arc::new(vec![github_thread(100, 7, &["nit: name this"])]));
        harness.rebuild();
        harness.paint();

        // The Ask is drawn while the card is under the pointer.
        let rect = card_rect(&harness, "gh-100");
        let (left, top) = pane_origin();
        let inside = (
            left + gutter_width() + rect.center().x,
            top + rect.center().y,
        );
        harness.pointer_move(inside.0, inside.1);
        harness.rebuild();
        harness.paint();
        let ask = find(harness.root_id(), "ask-about").expect("the comment offers Ask about this");
        let c = ask.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        harness.rebuild();
        harness.paint();
        let composer = state.composer.get().expect("the composer opened");
        assert_eq!(composer.thread_id.as_deref(), Some("gh-100"));
        assert_eq!(
            composer.anchor.as_ref().map(|a| (a.start_line, a.line)),
            Some((7, 7)),
            "on the thread's own lines"
        );
        assert_eq!(
            state.composer_draft.get(),
            "",
            "nothing is quoted; the thread carries the comment"
        );
        assert!(state.follow_up_is_inline("gh-100"));
    }

    /// The reaction picker opens from a card in the code as it does on
    /// Conversation: in the window's overlay, over the card layer and the
    /// code, where neither the editor's scroll nor its layers reach it.
    #[test]
    fn the_reaction_picker_opens_over_the_code_from_a_card_in_it() {
        use perch_github::{ReactionGroup, ReactionKind, Reactions};
        let (mut harness, state) = harness();
        state.set_comment_threads(Arc::new(vec![github_thread(100, 7, &["nit: name this"])]));
        let hearts = Reactions {
            groups: vec![ReactionGroup {
                kind: ReactionKind::Heart,
                count: 1,
                viewer_reacted: false,
                reactors: vec![],
            }],
        };
        state.install_reactions([("PRRC_100", &hearts)]);
        harness.rebuild();
        harness.paint();

        let card = find(harness.root_id(), "inline-card:gh-100").unwrap();
        let face = find(
            find(card, "reactions").expect("the comment's pills"),
            "react-button",
        )
        .unwrap();
        let c = face.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        harness.rebuild();
        harness.paint();

        let picker = find(harness.root_id(), "react-picker").expect("the click opened it");
        let at = face.get_visual_rect_no_clip();
        let rect = picker.get_visual_rect_no_clip();
        assert_eq!(
            (rect.y1, rect.x0),
            (at.y0 - 8.0, at.x0),
            "not over the face"
        );
        for kind in ReactionKind::ALL {
            let pick = find(picker, &format!("react-pick:{}", kind.graphql())).unwrap();
            let c = pick.get_visual_rect_no_clip().center();
            assert_eq!(
                harness.element_id_at(c.x, c.y).map(|e| e.owning_id()),
                Some(pick),
                "something in the code is drawn over {kind:?}"
            );
        }
    }

    #[test]
    fn a_resolved_github_thread_starts_collapsed_and_opens_when_toggled() {
        use crate::review::doc::CardId;
        let (mut harness, state) = harness();
        let live = stacked(state);
        let mut thread = github_thread(100, 7, &["nit: name this", "done"]);
        thread.resolved = true;
        state.set_comment_threads(Arc::new(vec![thread]));
        let card = CardId::Thread("gh-100".into());
        assert!(
            state.is_collapsed(&card),
            "resolved on GitHub, so folded away"
        );
        harness.rebuild();
        harness.paint();
        let folded = crate::review::doc::COLLAPSED_ROWS;
        assert_eq!(live.doc.get().card_span(&card), Some(9..9 + folded));
        let line_height = crate::theme::code_line_height() as f64;
        let folded_height = folded as f64 * line_height - 2.0 * crate::theme::COLLAPSED_CARD_INSET;
        assert_eq!(card_rect(&harness, "gh-100").height(), folded_height);

        state.toggle_card(&card);
        harness.rebuild();
        harness.paint();
        assert!(!state.is_collapsed(&card));
        assert!(card_rect(&harness, "gh-100").height() > folded_height);
        assert!(
            find(harness.root_id(), "name label - Label: \"hubot\"").is_some(),
            "open, the reply shows"
        );
    }

    #[test]
    fn the_composer_opened_on_lines_is_a_card_under_them_and_escape_takes_its_rows_away() {
        use crate::review::doc::{CardId, DocRow, COMPOSER_ROWS};
        use floem::prelude::{Code, Key, NamedKey};
        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = crate::theme::code_line_height() as f64;
        let cursor = super::STACKED_CURSOR
            .with(|c| c.get())
            .expect("a stacked editor was built");
        let card = CardId::Composer;

        // The cursor placed as a click would place it: on head line 7,
        // display line 8, row 8. The selection is that line.
        let doc = live.doc.get();
        let row = doc.row_of_head(6).expect("head line 7 is shown");
        assert_eq!(row, 8);
        cursor.update(|c| {
            c.set_offset(
                doc.offset_of_row(row) + 2,
                CursorAffinity::Backward,
                false,
                false,
            )
        });
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

        // `a` opens the composer on the line. It is a card under the line,
        // at its estimate until it is laid out, and Files stays visible.
        harness.dispatch_event(key_press(Code::KeyA, Key::Character("a".into())));
        let composer = state.composer.get().expect("`a` opened the composer");
        assert_eq!(
            composer.anchor.as_ref().map(|a| (a.start_line, a.line)),
            Some((7, 7))
        );
        assert!(state.composer_is_inline());
        assert_eq!(state.tab.get(), ItemTab::Code);
        harness.rebuild();
        harness.paint();
        let rows = laid_out_rows(&harness, "composer");
        assert_eq!(
            rows, COMPOSER_ROWS,
            "an empty composer is laid out at its estimate, so nothing jumps"
        );
        assert!(
            state.card_rows.get().is_empty(),
            "the estimate was right, so nothing was written"
        );
        let doc = live.doc.get();
        assert_eq!(doc.len(), 120 + rows);
        assert_eq!(doc.card_span(&card), Some(9..9 + rows));
        for row in 9..9 + rows {
            assert_eq!(
                doc.rows[row],
                DocRow::Spacer {
                    card: CardId::Composer,
                    row: row - 9
                }
            );
        }
        assert_eq!(
            doc.display_line(9 + rows),
            Some(9),
            "the code goes on under the card"
        );
        assert_eq!(laid_out_text(&live), doc.text);
        let rect = card_rect(&harness, "composer");
        assert_eq!(rect.y0, 9.0 * line_height + crate::theme::CARD_INSET);
        assert_eq!(rect.x0, crate::theme::CODE_GUTTER_GAP);
        let (pane_left, _) = pane_origin();
        let right_edge = 1400.0 - pane_left - gutter_width() - crate::theme::CODE_GUTTER_GAP;
        assert!(
            right_edge > rect.x0 + crate::theme::INLINE_THREAD_MAX_WIDTH,
            "the pane is wider than a card may be"
        );
        assert_eq!(
            rect.x1,
            rect.x0 + crate::theme::INLINE_THREAD_MAX_WIDTH,
            "no wider than github.com draws a thread, short of the right edge"
        );

        // Escape, with nothing typed, puts the composer away, and the
        // rows go with it.
        harness.dispatch_event(key_press(Code::Escape, Key::Named(NamedKey::Escape)));
        harness.rebuild();
        harness.paint();
        assert!(
            state.composer.get().is_none(),
            "escape did not close the empty composer"
        );
        let doc = live.doc.get();
        assert_eq!(doc.len(), 120);
        assert_eq!(doc.card_span(&card), None);
        assert_eq!(laid_out_text(&live), doc.text);
        assert!(find(harness.root_id(), "inline-card:").is_none());
        assert!(
            !state.card_rows.get().contains_key(&card),
            "the next composer starts on the estimate"
        );

        // A question about the item has no inline anchor, so it opens on
        // Conversation instead.
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        state.open_composer(None, None, String::new(), choice);
        harness.rebuild();
        harness.paint();
        assert!(!state.composer_is_inline());
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert_eq!(count(harness.root_id(), "composer-box"), 1);
        assert!(find(harness.root_id(), "inline-card:").is_none());
    }

    #[test]
    fn a_sent_question_keeps_its_rows_until_its_thread_arrives_and_the_threads_card_takes_them() {
        use crate::review::doc::CardId;
        let (mut harness, state) = harness();
        let live = stacked(state);
        let line_height = crate::theme::code_line_height() as f64;
        let viewport = super::STACKED_VIEWPORT
            .with(|v| v.get())
            .expect("a stacked editor was built");
        let anchor = perch_github::Anchor {
            path: "src/x.rs".into(),
            start_line: 5,
            line: 7,
            side: perch_github::Side::Right,
            commit_sha: "h".into(),
        };
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        state.open_composer(None, Some(anchor), String::new(), choice);
        harness.rebuild();
        harness.paint();
        let rows = laid_out_rows(&harness, "composer");
        assert_eq!(
            live.doc.get().card_span(&CardId::Composer),
            Some(9..9 + rows)
        );

        // Sent: the composer is gone from the code, and its rows are not.
        state.composer_sent();
        harness.rebuild();
        harness.paint();
        assert!(state.composer.get().is_none());
        assert!(
            find(harness.root_id(), "inline-card:").is_none(),
            "the question went, and the box with it"
        );
        let doc = live.doc.get();
        assert_eq!(
            doc.card_span(&CardId::Composer),
            Some(9..9 + rows),
            "the rows stay under the lines"
        );
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
        assert_eq!(
            card_rect(&harness, "t1").y0,
            9.0 * line_height + crate::theme::CARD_INSET
        );
        assert!(!state.card_rows.get().contains_key(&CardId::Composer));
    }
}
