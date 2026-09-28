//! Cmd-F: finding text in the open file.
//!
//! The bar over the top right of the code, as VS Code's: a box for the
//! query, three toggles for how it is read, how many matches there are
//! and which this is, the arrows that walk them, and Close. Enter walks
//! forward, Shift-Enter back, and Escape puts the bar away and gives the
//! keyboard back to the code. Every match is tinted in the editor, the
//! current one darker and selected, and the editor scrolls to it.
//!
//! The query is matched against the file's display buffer line by line:
//! the head with its removed lines spliced in, whether or not a line is
//! folded away. A match is a display line and two byte columns, which is
//! what the styling paints a row from and what a fold leaves alone; the
//! rows only come into it when a match is walked to, and a match inside
//! a fold reveals the fold first, as a jump does.
//!
//! The bar is one view for the pane, outside the container that swaps
//! editors, so it stays up from one file to the next with its text and
//! its keyboard; `wire` follows the file and the query from the review
//! state, and the actions reach whichever editor is up through the live
//! documents it registers.
//!
//! Conversation has the same bar over its page, with the same query and
//! toggles; what it finds there, and how it walks it, is
//! [`conversation_find`]'s. Each [`Pane`] has its bar up or down on its
//! own.

use std::sync::Arc;

use floem::event::{Event, EventPropagation, RouteKind};
use floem::peniko::Brush;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, Position};
use floem::views::editor::core::cursor::CursorAffinity;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::SelectionColor;
use floem::views::{
    dyn_container, Container, Decorators, Empty, Label, PlaceholderTextClass, Stack, TextInput,
};

use crate::icons::SidebarIcon;
use crate::review::cards::tip;
use crate::review::conversation_find;
use crate::review::doc::Doc;
use crate::review::editor;
use crate::review::hover;
use crate::review::model::{Layout, LiveDoc, ReviewState};
use crate::review::references;
use crate::theme;
use crate::ui::widgets::{clickable, sidebar_glyph};
use crate::ui::OS_MOD;

/// One match: the display line it is on, and its byte columns in that
/// line, the end exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Match {
    pub display: usize,
    pub start: usize,
    pub end: usize,
}

/// Which bar: the one over the code, or the one over Conversation's page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Code,
    Conversation,
}

impl Pane {
    /// Whether this pane's bar is up.
    fn open_signal(self, state: ReviewState) -> floem::reactive::RwSignal<bool> {
        match self {
            Pane::Code => state.find_open,
            Pane::Conversation => state.conversation_find_open,
        }
    }

    /// How many matches there are, tracked.
    fn found(self, state: ReviewState) -> usize {
        match self {
            Pane::Code => state.find_matches.with(|m| m.len()),
            Pane::Conversation => state.conversation_find_hits.with(|h| h.len()),
        }
    }

    /// Which match the bar is on, tracked.
    fn current(self, state: ReviewState) -> Option<usize> {
        match self {
            Pane::Code => state.find_current.get(),
            Pane::Conversation => state.conversation_find_current.get(),
        }
    }

    /// Why the pattern matched nothing, tracked.
    fn error(self, state: ReviewState) -> Option<String> {
        match self {
            Pane::Code => state.find_error.get(),
            Pane::Conversation => state.conversation_find_error.get(),
        }
    }
}

/// How the query is read: as itself or as a regular expression, and with
/// or without regard to case and to the word around a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    pub match_case: bool,
    pub whole_word: bool,
    pub regex: bool,
}

// ---------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------

/// Every match of `query` in `text`, in order down the lines and along
/// each, as `options` reads it. A query that is nothing matches nothing,
/// and one that is a regular expression that does not parse is the
/// error, in a line, for the bar to show. A match of no length, which a
/// pattern like `a*` has at every column, is no match: there is nothing
/// to tint or select.
pub fn matches(text: &str, query: &str, options: Options) -> Result<Vec<Match>, String> {
    let Some(regex) = pattern(query, options)? else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    // The lines as the document has them: without a carriage return, and
    // without an empty last one after the final newline.
    for (display, line) in text.lines().enumerate() {
        for (start, end) in line_matches(&regex, line, options.whole_word) {
            found.push(Match {
                display,
                start,
                end,
            });
        }
    }
    Ok(found)
}

/// The query as `options` read it, ready to match: `None` for a query
/// that is nothing, and the error in a line for a regular expression
/// that does not parse.
pub fn pattern(query: &str, options: Options) -> Result<Option<regex::Regex>, String> {
    if query.is_empty() {
        return Ok(None);
    }
    let pattern = if options.regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(!options.match_case)
        .build()
        .map(Some)
        .map_err(|e| describe(&e))
}

/// Every match of `regex` along one line, as byte columns, the end
/// exclusive.
pub fn line_matches(regex: &regex::Regex, line: &str, whole_word: bool) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut at = 0usize;
    while at <= line.len() {
        let Some(m) = regex.find_at(line, at) else {
            break;
        };
        let (start, end) = (m.start(), m.end());
        let whole = !whole_word || is_whole_word(line, start, end);
        if start < end && whole {
            found.push((start, end));
            at = end;
        } else {
            // Past the first character of what was found, so that a
            // match ruled out for the word around it, or one of no
            // length, does not hide one starting a character on.
            at = next_char(line, start);
        }
    }
    found
}

/// Whether the text around a match makes it a word of its own, by VS
/// Code's reading: on each side, the character beyond the match is not
/// one of a word, or the match's own character on that side is not, so
/// that a query of punctuation is whole against the letters beside it.
fn is_whole_word(line: &str, start: usize, end: usize) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let before = line[..start].chars().next_back();
    let first = line[start..end].chars().next();
    let after = line[end..].chars().next();
    let last = line[start..end].chars().next_back();
    let left = before.is_none_or(|c| !word(c)) || first.is_some_and(|c| !word(c));
    let right = after.is_none_or(|c| !word(c)) || last.is_some_and(|c| !word(c));
    left && right
}

/// The byte offset of the character after the one at `at`, or past the
/// end of the line for the last.
fn next_char(line: &str, at: usize) -> usize {
    line[at..]
        .chars()
        .next()
        .map(|c| at + c.len_utf8())
        .unwrap_or(line.len() + 1)
}

/// A regular expression's parse error in a line: the regex crate's
/// message is the pattern drawn with a caret under the fault and the
/// reason on the last line, and the reason is what fits in the bar.
fn describe(error: &regex::Error) -> String {
    let text = error.to_string();
    let reason = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().trim_start_matches("error: "))
        .unwrap_or("invalid regular expression");
    let mut chars = reason.chars();
    let first = chars
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    first + chars.as_str()
}

/// The match to go to from a place in the file, a display line and a
/// byte column: forward, the first at or after it, so that a query typed
/// on stays on the match it is on while the match still holds and moves
/// on when it does not; back, the last before it. Either wraps at the
/// end of the file. `None` when there are none.
pub fn nearest(matches: &[Match], from: (usize, usize), forward: bool) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let at = |m: &Match| (m.display, m.start);
    Some(if forward {
        matches.iter().position(|m| at(m) >= from).unwrap_or(0)
    } else {
        matches
            .iter()
            .rposition(|m| at(m) < from)
            .unwrap_or(matches.len() - 1)
    })
}

// ---------------------------------------------------------------------
// Following the file and the query
// ---------------------------------------------------------------------

/// What the matches were last made from, for telling a change of query
/// from a change of file. The file is known by its path and the editor's
/// epoch, which every load of it bumps, and by whether there is an
/// editor over it at all, which a picture flipped to its markup changes
/// without an epoch.
#[derive(PartialEq, Eq)]
struct Made {
    open: bool,
    query: String,
    options: Options,
    layout: Layout,
    epoch: u64,
    path: Option<String>,
    code: bool,
}

/// What the last run left behind besides the matches: what they were
/// made from, and where a typed query looks from, as a display line and
/// a byte column. That is where the caret was when the bar opened, or
/// wherever the reader has since put it: the bar's own moves, to the
/// match a typed query found or to the one Enter walked to, do not move
/// it, so a query typed on looks from the same place as the last, as VS
/// Code's does.
struct Run {
    made: Made,
    anchor: Option<(usize, usize)>,
}

/// Keeps the matches to the file and the query: made again when either
/// changes, or the bar opens or closes, and empty while it is closed.
/// When the query changes under the same editor, or how it is read, the
/// bar goes to the first match from its anchor, as the reader types;
/// when the bar opens, the match the caret is on is the current one, if
/// it is on one; another file, or the same one reloaded, starts with no
/// current match and no jump.
pub fn wire(state: ReviewState) {
    Effect::new(move |before: Option<Run>| {
        // Everything read first: a write below runs the editor's effects
        // in the middle of this one, and a read after that would not be
        // tracked.
        let open = state.find_open.get();
        let query = state.find_query.get();
        let options = state.find_options.get();
        let layout = state.layout.get();
        let epoch = state.editor_epoch.get();
        // A picture has no editor to find anything in, unless its markup
        // is shown.
        let file = state
            .active_file()
            .filter(|_| state.drawn_picture().is_none());
        let made = Made {
            open,
            query: query.clone(),
            options,
            layout,
            epoch,
            path: state.active.get(),
            code: file.is_some(),
        };
        if before.as_ref().is_some_and(|b| b.made == made) {
            return Run {
                made,
                anchor: before.and_then(|b| b.anchor),
            };
        }
        let same_editor = before.as_ref().is_some_and(|b| {
            let b = &b.made;
            b.epoch == epoch && b.layout == layout && b.path == made.path && b.code == made.code
        });
        let was_open = before.as_ref().is_some_and(|b| b.made.open);
        // The anchor stands while the caret is where the bar put it, on
        // the match it is on; anywhere else, the reader moved it there.
        let caret = caret(state, false);
        let placed = state.find_current.get_untracked().and_then(|i| {
            state
                .find_matches
                .with_untracked(|m| m.get(i).map(|m| (m.display, m.start)))
        });
        let anchor = match before.as_ref().and_then(|b| b.anchor) {
            Some(anchor) if same_editor && was_open && placed.is_some() && placed == caret => {
                Some(anchor)
            }
            _ => caret,
        };

        let (found, error) = match &file {
            Some(file) if open => match matches(&file.buffer.text, &query, options) {
                Ok(found) => (found, None),
                Err(error) => (Vec::new(), Some(error)),
            },
            _ => (Vec::new(), None),
        };
        // A split's head side has no row for a removed line, and the base
        // side is over the base file; a match on a ghost is nowhere to go.
        let found = match &file {
            Some(file) if layout == Layout::Split && editor::splits(file) => found
                .into_iter()
                .filter(|m| !file.buffer.row(m.display).is_some_and(|r| r.is_deleted()))
                .collect(),
            _ => found,
        };
        let found = Arc::new(found);

        // Written only when they change: the editor lays its rows out
        // again on either of the first two, and neither changes for a
        // file opened while the bar is down.
        if state.find_error.with_untracked(|e| *e != error) {
            state.find_error.set(error);
        }
        if state.find_matches.with_untracked(|m| **m != *found) {
            state.find_matches.set(found.clone());
        }
        let current = if same_editor && was_open && open {
            let index = anchor.and_then(|from| nearest(&found, from, true));
            if let Some(i) = index {
                go_to(state, found[i]);
            }
            index
        } else if same_editor && open {
            anchor.and_then(|from| found.iter().position(|m| (m.display, m.start) == from))
        } else {
            None
        };
        if state.find_current.get_untracked() != current {
            state.find_current.set(current);
        }
        Run { made, anchor }
    });
}

// ---------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------

/// Cmd-F: the bar up over the code, with the keyboard when asked for,
/// or, when it is up, the keyboard again with its text selected, to be
/// typed over. What is selected in the editor, when it is on one line,
/// or else the word the caret is on, becomes the query, as VS Code's
/// does; read as a pattern, it is made to stand for itself.
///
/// Over Conversation's page, the query is what it was.
pub fn open(state: ReviewState, pane: Pane, keyboard: bool) {
    let open = pane.open_signal(state);
    if !open.get_untracked() {
        open.set(true);
    }
    let seed = match pane {
        Pane::Code => seed(state),
        Pane::Conversation => None,
    };
    if let Some(text) = seed {
        let text = if state.find_options.get_untracked().regex {
            regex::escape(&text)
        } else {
            text
        };
        if state.find_query.with_untracked(|q| *q != text) {
            state.find_query.set(text);
        }
    }
    if !keyboard {
        return;
    }
    // After the query: the box selects what it has laid out, and it lays
    // the new text out in the same pass, on the message the write queued.
    // A box not built yet takes the keyboard once it is in the tree.
    match state.find_box.get_untracked() {
        Some(id) => take_keyboard(id),
        None => state.find_focus.set(true),
    }
}

/// Escape, or Close: the bar away, its tints with it, and the keyboard
/// back to the code, or the page. The match the bar was on over the
/// code stays selected.
pub fn close(state: ReviewState, pane: Pane) {
    if pane == Pane::Conversation {
        conversation_find::close(state);
        return;
    }
    let text_box = state.find_box.get_untracked();
    state.find_open.set(false);
    // The box goes with the bar, and a view taken down is told nothing;
    // said here so the code's keys are back before the next press.
    state.find_focused.set(false);
    match head_live(state).and_then(|live| live.content) {
        Some(content) => content.request_focus(),
        // No editor, while the file loads: the keyboard is nobody's
        // rather than the gone box's.
        None => {
            if let Some(id) = text_box {
                id.clear_focus();
            }
        }
    }
}

/// Enter, Shift-Enter and the arrows: the next match from the end of
/// the selection, or the one before its start, wrapping at either end,
/// as VS Code walks them. From a match the bar selected, that is the
/// next or the one before; from wherever the reader has since clicked,
/// it is the nearest from there. Over the page, see
/// [`conversation_find::walk`].
pub fn walk(state: ReviewState, pane: Pane, forward: bool) {
    if pane == Pane::Conversation {
        conversation_find::walk(state, forward);
        return;
    }
    let found = state.find_matches.get_untracked();
    let Some(from) = caret(state, forward) else {
        return;
    };
    let index = nearest(&found, from, forward);
    state.find_current.set(index);
    if let Some(i) = index {
        go_to(state, found[i]);
    }
}

/// Give the box the keyboard with its text selected whole, to be typed
/// over. floem's box selects its text on a focus that came by Tab, and
/// not on one asked for, so Select All is sent after the focus: both are
/// messages, taken in order, and the key finds the box focused.
fn take_keyboard(id: floem::ViewId) {
    id.request_focus();
    id.route_event(
        Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key: Key::Character("a".into()),
            code: Code::KeyA,
            location: Default::default(),
            modifiers: OS_MOD,
            repeat: false,
            is_composing: false,
        }),
        RouteKind::target(id.get_element_id()),
    );
}

/// The keyboard back to the box after a click on one of the bar's
/// tiles, which took it from the box as a click on anything does, so
/// Enter and the next letters go on working.
pub(crate) fn keep_keyboard(state: ReviewState) {
    if let Some(id) = state.find_box.get_untracked() {
        id.request_focus();
    }
}

/// The editor that has the file, as the live documents have it: the
/// stacked one, or the head side of a split. Nothing while no editor is
/// up, or while the entry is an editor's since rebuilt.
fn head_live(state: ReviewState) -> Option<LiveDoc> {
    let docs = state.docs.get_untracked()?;
    (docs.epoch == state.editor_epoch.get_untracked()).then(|| docs.layout.head().clone())
}

/// The editor's selection as byte offsets in its text, the two the same
/// for a caret.
fn selection(live: &LiveDoc) -> Option<(usize, usize)> {
    live.cursor.try_with_untracked(|c| {
        c.map(|c| {
            c.regions_iter()
                .fold((usize::MAX, 0usize), |(lo, hi), (a, b, _)| {
                    (lo.min(a.min(b)), hi.max(a.max(b)))
                })
        })
    })
}

/// Where the caret is, as a display line and a byte column: the start of
/// the selection, or its end when `end`. A caret on a row that is no
/// line, a divider's or a card's, is at the start of the nearest line.
fn caret(state: ReviewState, end: bool) -> Option<(usize, usize)> {
    let live = head_live(state)?;
    let (lo, hi) = selection(&live)?;
    let offset = if end { hi } else { lo };
    live.doc.try_with_untracked(|d| {
        let d = d?;
        let (row, column) = hover::line_col_of_offset(d.text(), offset);
        match d.display_line(row) {
            Some(display) => Some((display, column)),
            None => Some((d.nearest_display_line(row)?, 0)),
        }
    })
}

/// What Cmd-F looks for: the editor's selection, when there is one and
/// it is on one line, or else the word the caret is on.
fn seed(state: ReviewState) -> Option<String> {
    let live = head_live(state)?;
    let (lo, hi) = selection(&live)?;
    live.doc.try_with_untracked(|d| {
        let d = d?;
        if lo < hi {
            let text = d.text().get(lo..hi)?;
            return (!text.contains('\n')).then(|| text.to_string());
        }
        let (row, column) = hover::line_col_of_offset(d.text(), lo);
        let (_, word) = references::identifier_at(d.line(row), column)?;
        Some(word.to_string())
    })
}

/// Select a match in the editor, which scrolls to its caret, and reveal
/// the fold it is in first: the reveal reaches the editor before this
/// goes on, since the effect that rebuilds the document runs on the
/// write. A match with no row after that, a ghost's on a split's head
/// side, is left where it is.
fn go_to(state: ReviewState, m: Match) {
    let Some(live) = head_live(state) else { return };
    let Some(file) = Effect::untrack(|| state.active_file()) else {
        return;
    };
    let row_of = |d: Option<&Arc<Doc>>| d.and_then(|d| d.row_of_display(m.display));
    let mut row = live.doc.try_with_untracked(row_of);
    if row.is_none() {
        if let Some(head) = file.buffer.head_line(m.display) {
            state.reveal_around(&file, head);
            row = live.doc.try_with_untracked(row_of);
        }
    }
    let Some(row) = row else { return };
    let Some((start, end)) = live.doc.try_with_untracked(|d| {
        let d = d?;
        let at = d.offset_of_row(row);
        let len = d.line(row).len();
        Some((at + m.start.min(len), at + m.end.min(len)))
    }) else {
        return;
    };
    live.cursor
        .try_update(|c| c.set_insert(Selection::region(start, end, CursorAffinity::Backward)));
}

// ---------------------------------------------------------------------
// The bar
// ---------------------------------------------------------------------

/// The bar while it is up and there is a file under it that reads as
/// text, hung from the top right of the code, clear of the scrollbar,
/// and nothing while there is not: it waits out a picture and no file at
/// all, and is back over the next editor. The one view stays through a
/// change of file, with its text and its keyboard. Over Conversation's
/// page, it is there while it is up.
pub fn bar(state: ReviewState, pane: Pane) -> impl IntoView {
    let shown = Memo::new(move |_| match pane {
        Pane::Code => {
            state.find_open.get()
                && state.active.with(Option::is_some)
                && state.drawn_picture().is_none()
        }
        Pane::Conversation => state.conversation_find_open.get(),
    });
    dyn_container(
        move || shown.get(),
        move |shown| {
            if shown {
                body(state, pane).into_any()
            } else {
                Empty::new().into_any()
            }
        },
    )
    .style(|s| {
        s.position(Position::Absolute)
            .inset_top(0.0)
            .inset_right(theme::FIND_BAR_INSET)
            .z_index(100)
    })
}

fn body(state: ReviewState, pane: Pane) -> impl IntoView {
    Stack::horizontal((
        query_box(state),
        count(state, pane),
        arrow(state, pane, false),
        arrow(state, pane, true),
        close_button(state, pane),
    ))
    // The keys of the bar, caught on the way down to the box: the box
    // keeps every key once it has the keyboard, and would take Enter for
    // its own and drop the keyboard on Escape.
    .on_event_with_config(
        el::KeyDown,
        floem::context::EventCallbackConfig {
            phases: floem::context::Phases::CAPTURE,
        },
        move |_, key: &KeyboardEvent| {
            let plain = key.modifiers.is_empty();
            match key.code {
                Code::Enter if plain || key.modifiers == Modifiers::SHIFT => {
                    walk(state, pane, plain);
                    EventPropagation::Stop
                }
                Code::Escape if plain => {
                    close(state, pane);
                    EventPropagation::Stop
                }
                _ => EventPropagation::Continue,
            }
        },
    )
    .debug_name("find-bar")
    .style(|s| {
        s.height(theme::FIND_BAR_HEIGHT)
            .items_center()
            .padding_horiz(6.0)
            .gap(3.0)
            .background(theme::CANVAS_SUBTLE)
            .border(1.0)
            .border_top(0.0)
            .border_color(theme::BORDER)
            .border_bottom_left_radius(4.0)
            .border_bottom_right_radius(4.0)
            .box_shadow_blur(8.0)
            .box_shadow_v_offset(2.0)
            .box_shadow_color(theme::SHADOW)
    })
}

/// The box, with the three toggles laid over its right end.
fn query_box(state: ReviewState) -> impl IntoView {
    let input = TextInput::new(state.find_query)
        .placeholder("Find")
        .debug_name("find-box")
        .on_event_cont(el::FocusGained, move |_, _| state.find_focused.set(true))
        .on_event_cont(el::FocusLost, move |_, _| state.find_focused.set(false))
        .style(|s| {
            s.width(theme::FIND_BOX_WIDTH)
                .height(25.0)
                .font_size(theme::TEXT_META)
                .padding_left(6.0)
                // Room for the toggles.
                .padding_right(70.0)
                .background(theme::CANVAS)
                .hover(|s| s.background(theme::CANVAS))
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(3.0)
                .focus(|s| s.border_color(theme::ACCENT))
                .cursor_color(theme::FG)
                .set(SelectionColor, Brush::Solid(theme::SELECTION_BG))
                .class(PlaceholderTextClass, |s| s.color(theme::FG_SUBTLE))
        });
    let id = input.view_id();
    state.find_box.set(Some(id));
    // Cmd-F asked for the keyboard before this box existed; it is taken
    // once the box is in the tree, where a request can find it. A box
    // built again for the same bar, back on Code from Conversation, was
    // not asked, and leaves the keyboard where the reader has it.
    let input = input
        .on_event_cont(el::UpdatePhaseLayout, move |_, _| {
            if state.find_focus.get_untracked() {
                state.find_focus.set(false);
                take_keyboard(id);
            }
        })
        .on_cleanup(move || {
            // A view taken down is told nothing of the keyboard it had;
            // a box built in its place asks for it again if it wants it.
            state.find_focused.set(false);
            if state.find_box.get_untracked() == Some(id) {
                state.find_box.set(None);
            }
        });

    let toggles = Stack::horizontal((
        toggle_tile(
            state,
            "Aa",
            Face::Text,
            "Match case (⌥⌘C)",
            "find-match-case",
            |o| o.match_case,
            |o| o.match_case = !o.match_case,
        ),
        toggle_tile(
            state,
            "ab",
            Face::Underlined,
            "Match whole word (⌥⌘W)",
            "find-whole-word",
            |o| o.whole_word,
            |o| o.whole_word = !o.whole_word,
        ),
        toggle_tile(
            state,
            ".*",
            Face::Code,
            "Use regular expression (⌥⌘R)",
            "find-regex",
            |o| o.regex,
            |o| o.regex = !o.regex,
        ),
    ))
    .style(|s| {
        s.position(Position::Absolute)
            .inset_right(3.0)
            .items_center()
            .gap(2.0)
    });
    Stack::horizontal((input, toggles)).style(|s| s.items_center().position(Position::Relative))
}

/// How a toggle's letters are set: the UI font, the UI font with a rule
/// under it for the whole word, or the code font for the pattern.
#[derive(Clone, Copy)]
enum Face {
    Text,
    Underlined,
    Code,
}

/// One of the toggles: its letters in a tile, lit while it is on.
fn toggle_tile(
    state: ReviewState,
    letters: &'static str,
    face: Face,
    tip_text: &'static str,
    name: &'static str,
    is_on: fn(&Options) -> bool,
    flip: fn(&mut Options),
) -> impl IntoView {
    let on = move || state.find_options.with(is_on);
    let color = move || if on() { theme::FG } else { theme::FG_MUTED };
    let label = Label::derived(move || letters.to_string()).style(move |s| {
        let s = s
            .font_size(theme::TEXT_BADGE)
            .color(color())
            .selectable(false);
        match face {
            Face::Text => s,
            Face::Underlined => s.border_bottom(1.0).border_color(color()),
            Face::Code => s.font_family(theme::CODE_FONT_STACK.to_string()),
        }
    });
    let tile = Container::new(label)
        .on_event_stop(el::Click, move |_, _| {
            state.find_options.update(flip);
            keep_keyboard(state);
        })
        .debug_name(name)
        .style(move |s| {
            let base = clickable(s)
                .size(20.0, 20.0)
                .items_center()
                .justify_center()
                .border_radius(3.0);
            if on() {
                base.background(theme::BADGE_BG)
            } else {
                base.hover(|s| s.background(theme::CANVAS_HOVER))
            }
        });
    tip(tile, tip_text.to_string())
}

/// "3 of 8", "? of 8" before the bar has walked to one, "No results",
/// red when there was something to look for, or why a pattern did not
/// parse.
fn count(state: ReviewState, pane: Pane) -> impl IntoView {
    let text = move || count_text(state, pane).0;
    let red = move || count_text(state, pane).1;
    let label = Label::derived(text)
        .debug_name("find-count")
        .style(move |s| {
            s.font_size(theme::TEXT_BADGE)
                .color(if red() {
                    theme::CLOSED
                } else {
                    theme::FG_MUTED
                })
                .selectable(false)
                .max_width(220.0)
                .text_ellipsis()
        });
    // Room for "12 of 345" before the arrows move, as VS Code's count has.
    Container::new(label).style(|s| {
        s.min_width(69.0)
            .items_center()
            .justify_center()
            .padding_horiz(3.0)
    })
}

/// The count's words, and whether they are bad news. Nothing while the
/// file is still on its way.
fn count_text(state: ReviewState, pane: Pane) -> (String, bool) {
    if let Some(error) = pane.error(state) {
        return (error, true);
    }
    if pane == Pane::Code && state.active_file().is_none() {
        return (String::new(), false);
    }
    let n = pane.found(state);
    if n == 0 {
        let empty = state.find_query.with(|q| q.is_empty());
        return ("No results".to_string(), !empty);
    }
    match pane.current(state) {
        Some(i) => (format!("{} of {n}", i + 1), false),
        None => (format!("? of {n}"), false),
    }
}

/// Previous or next, as the strip's chevrons; greyed with nothing to
/// walk.
fn arrow(state: ReviewState, pane: Pane, forward: bool) -> impl IntoView {
    let glyph = if forward {
        SidebarIcon::ChevronDown
    } else {
        SidebarIcon::ChevronUp
    };
    let none = move || pane.found(state) == 0;
    let tile = Container::new(sidebar_glyph(glyph, theme::FG_MUTED))
        .on_event_stop(el::Click, move |_, _| {
            walk(state, pane, forward);
            keep_keyboard(state);
        })
        .debug_name(if forward {
            "find-next"
        } else {
            "find-previous"
        })
        .style(move |s| {
            let s = s
                .size(20.0, 20.0)
                .items_center()
                .justify_center()
                .border_radius(4.0);
            if none() {
                s.opacity(0.3_f32)
            } else {
                s.hover(|s| s.background(theme::BADGE_BG).cursor(CursorStyle::Pointer))
            }
        });
    let text = if forward {
        "Next match (Enter)"
    } else {
        "Previous match (⇧Enter)"
    };
    tip(tile, text.to_string())
}

fn close_button(state: ReviewState, pane: Pane) -> impl IntoView {
    let tile = Container::new(
        sidebar_glyph(SidebarIcon::Close, theme::FG_SUBTLE).style(|s| s.size(11.0, 11.0)),
    )
    .on_event_stop(el::Click, move |_, _| close(state, pane))
    .debug_name("find-close")
    .style(|s| {
        clickable(s)
            .size(18.0, 18.0)
            .items_center()
            .justify_center()
            .border_radius(3.0)
            .hover(|s| s.background(theme::BADGE_BG))
    });
    tip(tile, "Close (Escape)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(display: usize, start: usize, end: usize) -> Match {
        Match {
            display,
            start,
            end,
        }
    }

    #[test]
    fn a_plain_query_is_found_on_every_line_without_regard_to_case() {
        let text = "let path = 1;\nlet a = Path;\nPATHpath\n";
        let found = matches(text, "path", Options::default()).unwrap();
        assert_eq!(found, vec![m(0, 4, 8), m(1, 8, 12), m(2, 0, 4), m(2, 4, 8)]);

        let cased = matches(
            text,
            "path",
            Options {
                match_case: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(cased, vec![m(0, 4, 8), m(2, 4, 8)]);
    }

    #[test]
    fn nothing_is_found_for_nothing() {
        assert!(matches("a\nb\n", "", Options::default())
            .unwrap()
            .is_empty());
        assert!(matches("", "a", Options::default()).unwrap().is_empty());
    }

    #[test]
    fn a_plain_query_is_taken_as_itself_and_a_regex_as_a_pattern() {
        let text = "a.c\nabc\n";
        let plain = matches(text, "a.c", Options::default()).unwrap();
        assert_eq!(plain, vec![m(0, 0, 3)], "the dot is a dot");
        let pattern = matches(
            text,
            "a.c",
            Options {
                regex: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(pattern, vec![m(0, 0, 3), m(1, 0, 3)]);
    }

    #[test]
    fn a_pattern_that_does_not_parse_is_the_reason_in_a_line() {
        let err = matches(
            "a\n",
            "(",
            Options {
                regex: true,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.lines().count(), 1, "one line: {err:?}");
        assert!(err.starts_with(|c: char| c.is_uppercase()), "{err:?}");
        assert!(
            !err.contains("regex parse error"),
            "the reason, not the heading: {err:?}"
        );
    }

    #[test]
    fn a_match_of_no_length_is_no_match() {
        let found = matches(
            "aab\n",
            "a*",
            Options {
                regex: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(found, vec![m(0, 0, 2)]);
        assert!(matches(
            "ab\n",
            "^",
            Options {
                regex: true,
                ..Default::default()
            }
        )
        .unwrap()
        .is_empty());
        // Anchors still work at the line's ends.
        let found = matches(
            "ab\nba\n",
            "^a",
            Options {
                regex: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(found, vec![m(0, 0, 1)]);
        let found = matches(
            "ab\nba\n",
            "a$",
            Options {
                regex: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(found, vec![m(1, 1, 2)]);
    }

    #[test]
    fn whole_word_wants_no_word_on_either_side() {
        let text = "path pathname my_path (path) path.x\n";
        let whole = Options {
            whole_word: true,
            ..Default::default()
        };
        let found = matches(text, "path", whole).unwrap();
        assert_eq!(found, vec![m(0, 0, 4), m(0, 23, 27), m(0, 29, 33)]);

        // Punctuation in the query is its own boundary, as in VS Code.
        let found = matches("a.b xa.b a.bx\n", ".b", whole).unwrap();
        assert_eq!(found, vec![m(0, 1, 3), m(0, 6, 8)]);
        let found = matches("x=1 y==2\n", "=", whole).unwrap();
        assert_eq!(found, vec![m(0, 1, 2), m(0, 5, 6), m(0, 6, 7)]);

        // A match ruled out does not hide the one a character on.
        let found = matches("aba ba\n", "ba", whole).unwrap();
        assert_eq!(found, vec![m(0, 4, 6)]);
        assert!(matches("xaa\n", "aa", whole).unwrap().is_empty());
    }

    #[test]
    fn columns_are_bytes_and_a_multibyte_character_is_stepped_over() {
        let text = "é path\n";
        let found = matches(text, "path", Options::default()).unwrap();
        assert_eq!(found, vec![m(0, 3, 7)]);
        // Rejected on a multibyte first character: the step over it lands
        // on a character boundary rather than inside one.
        let whole = Options {
            whole_word: true,
            ..Default::default()
        };
        let found = matches("éé é\n", "é", whole).unwrap();
        assert_eq!(found, vec![m(0, 5, 7)]);
    }

    #[test]
    fn the_nearest_match_is_the_first_at_or_after_the_caret_or_the_last_before_it() {
        let found = vec![m(1, 4, 8), m(3, 0, 4), m(3, 8, 12)];
        assert_eq!(nearest(&found, (0, 0), true), Some(0));
        assert_eq!(
            nearest(&found, (1, 4), true),
            Some(0),
            "at the match is on it"
        );
        assert_eq!(nearest(&found, (1, 5), true), Some(1));
        assert_eq!(nearest(&found, (3, 8), true), Some(2));
        assert_eq!(
            nearest(&found, (3, 9), true),
            Some(0),
            "past the last wraps to the first"
        );
        assert_eq!(nearest(&[], (0, 0), true), None);

        assert_eq!(nearest(&found, (3, 9), false), Some(2));
        assert_eq!(
            nearest(&found, (3, 8), false),
            Some(1),
            "at the match is past it, going back"
        );
        assert_eq!(
            nearest(&found, (1, 4), false),
            Some(2),
            "before the first wraps to the last"
        );
        assert_eq!(nearest(&[], (0, 0), false), None);
    }
}

/// The bar in a headless window.
#[cfg(test)]
mod window {
    use std::rc::Rc;
    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::prelude::*;
    use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
    use perch_github::{Item, ItemCheckout};

    use super::Match;
    use crate::review::model::{ItemTab, Layout, LiveDoc, Phase, ReviewState};
    use crate::state::AppState;

    /// The whole window on Code with one changed file open: `head`
    /// against `base`, as `src/x.rs`.
    fn harness_with(head: &str, base: &str) -> (HeadlessHarness, ReviewState) {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let now = "2026-09-21T09:00:00Z".parse().unwrap();
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
        state.composer_autofocus.set(false);
        state.open_tab("src/x.rs");
        state.finish_load("src/x.rs", Arc::new(file("src/x.rs", head, base)));

        let backend = crate::backend::Backend::detached();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        (harness, state)
    }

    fn file(path: &str, head: &str, base: &str) -> crate::review::model::LoadedFile {
        crate::backend::build_loaded_file(
            path.into(),
            head.into(),
            base.into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        )
    }

    /// Four lines, the second rewritten from `path` to `Path`, so the
    /// unified buffer is the first, the ghost, the rewrite, a line
    /// without a match, and the last: nothing folds, and a row is its
    /// display line.
    fn harness() -> (HeadlessHarness, ReviewState) {
        harness_with(
            "let path = 1;\nlet a = Path;\nlet b = 2;\nlet c = path;\n",
            "let path = 1;\nlet a = path;\nlet b = 2;\nlet c = path;\n",
        )
    }

    fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().split_whitespace().any(|part| part == name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
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

    fn cmd(code: Code, ch: &str) -> floem::event::Event {
        key(code, Key::Character(ch.into()), crate::ui::OS_MOD)
    }

    fn typed(harness: &mut HeadlessHarness, text: &str) {
        for ch in text.chars() {
            harness.dispatch_event(key(
                Code::Unidentified,
                Key::Character(ch.to_string()),
                Modifiers::empty(),
            ));
        }
        harness.rebuild();
    }

    fn enter(modifiers: Modifiers) -> floem::event::Event {
        key(Code::Enter, Key::Named(NamedKey::Enter), modifiers)
    }

    fn escape() -> floem::event::Event {
        key(
            Code::Escape,
            Key::Named(NamedKey::Escape),
            Modifiers::empty(),
        )
    }

    fn m(display: usize, start: usize, end: usize) -> Match {
        Match {
            display,
            start,
            end,
        }
    }

    /// The editor that has the file, whichever layout is up.
    fn live(state: ReviewState) -> LiveDoc {
        state
            .docs
            .get()
            .expect("an editor is open")
            .layout
            .head()
            .clone()
    }

    /// The editor's selection, as byte offsets in its text.
    fn selection(state: ReviewState) -> (usize, usize) {
        live(state).cursor.with(|c| {
            c.regions_iter()
                .fold((usize::MAX, 0), |(lo, hi), (a, b, _)| {
                    (lo.min(a.min(b)), hi.max(a.max(b)))
                })
        })
    }

    /// The count's words, from the label's name, which quotes its text.
    fn count(harness: &HeadlessHarness) -> String {
        let id = named(harness.root_id(), "find-count").expect("the bar is up");
        let name = id.debug_name();
        let open = name.find('"').expect("the label quotes its text");
        let close = name.rfind('"').unwrap();
        name[open + 1..close].to_string()
    }

    /// Click a view where it is on the window.
    fn click(harness: &mut HeadlessHarness, name: &str) {
        let id = named(harness.root_id(), name).unwrap_or_else(|| panic!("{name} is there"));
        let center = id.get_visual_rect_no_clip().center();
        harness.click(center.x, center.y);
        harness.rebuild();
        harness.paint();
    }

    #[test]
    fn cmd_f_puts_the_bar_up_with_the_keyboard_and_escape_gives_it_back() {
        let (mut harness, state) = harness();
        assert!(named(harness.root_id(), "find-bar").is_none());
        let content = live(state).content.expect("the editor has a content view");

        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        harness.paint();
        assert!(state.find_open.get());
        let text_box = named(harness.root_id(), "find-box").expect("the bar is up");
        assert_eq!(state.find_box.get(), Some(text_box));
        assert!(harness.is_focused(text_box), "the box took the keyboard");
        assert!(state.find_focused.get());
        // The word the caret is on, at the top of the file, is the query,
        // and the caret is on its first match.
        assert_eq!(state.find_query.get(), "let");
        assert_eq!(state.find_current.get(), Some(0));
        assert_eq!(count(&harness), "1 of 5");

        // `a` is the composer's key from the code; in the box it is a
        // letter, and the query was selected whole to be typed over.
        typed(&mut harness, "pa");
        assert_eq!(state.find_query.get(), "pa");
        assert!(state.composer.get().is_none(), "the box kept its letters");
        assert_eq!(
            *state.find_matches.get(),
            vec![m(0, 4, 6), m(1, 8, 10), m(2, 8, 10), m(4, 8, 10)],
            "found on the head lines and on the ghost, without regard to case"
        );
        assert_eq!(
            state.find_current.get(),
            Some(0),
            "the first from the caret at the top"
        );
        assert_eq!(selection(state), (4, 6), "and selected");
        assert!(state.find_error.get().is_none());

        harness.dispatch_event(escape());
        harness.rebuild();
        harness.paint();
        assert!(!state.find_open.get());
        assert!(!state.find_focused.get());
        assert!(
            named(harness.root_id(), "find-bar").is_none(),
            "the bar is down"
        );
        assert!(state.find_box.get().is_none());
        assert!(
            harness.is_focused(content),
            "the code has the keyboard back"
        );
        assert!(state.find_matches.get().is_empty(), "no tints");
        assert_eq!(
            state.find_query.get(),
            "pa",
            "the query is kept for next time"
        );
        assert_eq!(selection(state), (4, 6), "the match stays selected");
    }

    #[test]
    fn enter_walks_forward_and_shift_enter_back_wrapping_at_the_ends() {
        let (mut harness, state) = harness();
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        typed(&mut harness, "path");
        assert_eq!(state.find_matches.with(|m| m.len()), 4);
        assert_eq!(state.find_current.get(), Some(0));
        assert_eq!(count(&harness), "1 of 4");

        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        assert_eq!(state.find_current.get(), Some(1));
        let text = live(state).doc.get().text.clone();
        let (lo, hi) = selection(state);
        assert_eq!(&text[lo..hi], "path", "the ghost line's, dimmed but there");
        assert_eq!(lo, text.find("let a = path").unwrap() + 8);
        assert_eq!(count(&harness), "2 of 4");

        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        assert_eq!(state.find_current.get(), Some(2));
        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        assert_eq!(state.find_current.get(), Some(3));
        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        assert_eq!(
            state.find_current.get(),
            Some(0),
            "past the last is the first"
        );

        harness.dispatch_event(enter(Modifiers::SHIFT));
        harness.rebuild();
        assert_eq!(
            state.find_current.get(),
            Some(3),
            "before the first is the last"
        );
        assert!(
            state.find_focused.get(),
            "the box keeps the keyboard through it all"
        );

        // Cmd-G and Cmd-Shift-G do the same, from the code or the box.
        harness.dispatch_event(cmd(Code::KeyG, "g"));
        harness.rebuild();
        assert_eq!(state.find_current.get(), Some(0));
        harness.dispatch_event(key(
            Code::KeyG,
            Key::Character("G".into()),
            crate::ui::OS_MOD | Modifiers::SHIFT,
        ));
        harness.rebuild();
        assert_eq!(state.find_current.get(), Some(3));

        // And the arrows.
        click(&mut harness, "find-next");
        assert_eq!(state.find_current.get(), Some(0));
        click(&mut harness, "find-previous");
        assert_eq!(state.find_current.get(), Some(3));
    }

    #[test]
    fn a_query_typed_on_looks_from_where_the_bar_opened_until_the_reader_moves() {
        let (mut harness, state) = harness();
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        typed(&mut harness, "l");
        // Every line starts with `let`: on the first.
        assert_eq!(state.find_current.get(), Some(0));
        assert_eq!(selection(state), (0, 1));

        // `le` still holds there.
        typed(&mut harness, "e");
        assert_eq!(state.find_query.get(), "le");
        assert_eq!(state.find_current.get(), Some(0));
        assert_eq!(selection(state), (0, 2));

        // Enter walks on; a letter more looks from where the bar opened
        // again, not from where Enter went, as in VS Code.
        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        assert_eq!(state.find_current.get(), Some(1), "on the ghost's");
        typed(&mut harness, "t");
        assert_eq!(state.find_current.get(), Some(0));
        assert_eq!(selection(state), (0, 3));

        // The reader clicks at the front of the last line: from there,
        // `let c` is that line's, and `let` typed back is too.
        let text = live(state).doc.get().text.clone();
        let last = text.find("let c").unwrap();
        live(state).cursor.update(|c| {
            c.set_insert(floem::views::editor::core::selection::Selection::caret(
                last,
                floem::views::editor::core::cursor::CursorAffinity::Backward,
            ))
        });
        typed(&mut harness, " c");
        assert_eq!(state.find_query.get(), "let c");
        assert_eq!(state.find_current.get(), Some(0), "the one `let c`");
        assert_eq!(selection(state), (last, last + 5));
        harness.dispatch_event(key(
            Code::Backspace,
            Key::Named(NamedKey::Backspace),
            Modifiers::empty(),
        ));
        harness.dispatch_event(key(
            Code::Backspace,
            Key::Named(NamedKey::Backspace),
            Modifiers::empty(),
        ));
        harness.rebuild();
        assert_eq!(state.find_query.get(), "let");
        let (lo, _) = selection(state);
        assert_eq!(lo, last, "from the click, not from the top");
    }

    #[test]
    fn the_toggles_read_the_query_another_way_and_have_keys_from_the_code() {
        let (mut harness, state) = harness();
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        typed(&mut harness, "path");
        assert_eq!(state.find_matches.with(|m| m.len()), 4);

        let alt_cmd = |code: Code, ch: &str| {
            key(
                code,
                Key::Character(ch.into()),
                crate::ui::OS_MOD | Modifiers::ALT,
            )
        };
        harness.dispatch_event(alt_cmd(Code::KeyC, "c"));
        harness.rebuild();
        assert!(state.find_options.get().match_case);
        assert_eq!(
            state.find_matches.with(|m| m.len()),
            3,
            "not the rewrite's Path"
        );

        // The tiles do the same as the keys.
        click(&mut harness, "find-match-case");
        assert!(!state.find_options.get().match_case, "off again");
        assert_eq!(state.find_matches.with(|m| m.len()), 4);

        harness.dispatch_event(alt_cmd(Code::KeyR, "r"));
        harness.rebuild();
        assert!(state.find_options.get().regex);
        state.find_query.set("pa(".into());
        harness.rebuild();
        assert!(state.find_matches.get().is_empty());
        let why = state
            .find_error
            .get()
            .expect("a pattern that does not parse says why");
        assert_eq!(count(&harness), why);

        state.find_query.set("p.th".into());
        harness.rebuild();
        assert!(state.find_error.get().is_none());
        assert_eq!(state.find_matches.with(|m| m.len()), 4);
        click(&mut harness, "find-regex");
        assert!(!state.find_options.get().regex);
        assert!(state.find_matches.get().is_empty(), "the dot is a dot");

        harness.dispatch_event(alt_cmd(Code::KeyW, "w"));
        harness.rebuild();
        assert!(state.find_options.get().whole_word);
        state.find_query.set("pat".into());
        harness.rebuild();
        assert!(state.find_matches.get().is_empty(), "no word is `pat`");
        assert_eq!(count(&harness), "No results");
        click(&mut harness, "find-whole-word");
        assert!(!state.find_options.get().whole_word);
        assert_eq!(state.find_matches.with(|m| m.len()), 4);
    }

    #[test]
    fn the_bar_stays_up_from_one_file_to_the_next_and_starts_that_one_over() {
        let (mut harness, state) = harness();
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        typed(&mut harness, "path");
        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        assert_eq!(state.find_current.get(), Some(1));

        state.open_tab("src/y.rs");
        state.finish_load(
            "src/y.rs",
            Arc::new(file(
                "src/y.rs",
                "no match here\nbut a path\n",
                "no match here\n",
            )),
        );
        harness.rebuild();
        harness.paint();
        assert!(state.find_open.get(), "still up");
        assert_eq!(
            *state.find_matches.get(),
            vec![m(1, 6, 10)],
            "the new file's"
        );
        assert_eq!(state.find_current.get(), None, "and on none of them yet");
        assert_eq!(count(&harness), "? of 1");
        let text = live(state).doc.get().text.clone();
        let (lo, hi) = selection(state);
        assert_eq!(lo, hi, "nothing selected until the bar is asked");

        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        assert_eq!(state.find_current.get(), Some(0));
        let (lo, hi) = selection(state);
        assert_eq!(&text[lo..hi], "path");
        assert_eq!(count(&harness), "1 of 1");

        // Back to the first file: its matches again, from the top.
        state.open_tab("src/x.rs");
        harness.rebuild();
        harness.paint();
        assert_eq!(state.find_matches.with(|m| m.len()), 4);
        assert_eq!(state.find_current.get(), None);
    }

    #[test]
    fn cmd_f_again_takes_the_selection_for_the_query() {
        let (mut harness, state) = harness();
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        typed(&mut harness, "zzz");
        assert_eq!(count(&harness), "No results");

        // The reader selects `let b` in the code, then asks again.
        let text = live(state).doc.get().text.clone();
        let at = text.find("let b").unwrap();
        live(state).cursor.update(|c| {
            c.set_insert(floem::views::editor::core::selection::Selection::region(
                at,
                at + 5,
                floem::views::editor::core::cursor::CursorAffinity::Backward,
            ))
        });
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        assert_eq!(state.find_query.get(), "let b");
        assert_eq!(state.find_current.get(), Some(0), "on the selection itself");
        assert_eq!(selection(state), (at, at + 5));
        assert!(harness.is_focused(state.find_box.get().unwrap()));

        // Selected whole, so what is typed next replaces it.
        typed(&mut harness, "c");
        assert_eq!(state.find_query.get(), "c");

        // A selection over more than one line is not a query.
        live(state).cursor.update(|c| {
            c.set_insert(floem::views::editor::core::selection::Selection::region(
                0,
                at + 5,
                floem::views::editor::core::cursor::CursorAffinity::Backward,
            ))
        });
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        assert_eq!(state.find_query.get(), "c");
    }

    #[test]
    fn a_click_on_a_tile_leaves_the_keyboard_with_the_box() {
        let (mut harness, state) = harness();
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        typed(&mut harness, "path");
        let text_box = state.find_box.get().unwrap();

        click(&mut harness, "find-next");
        assert_eq!(state.find_current.get(), Some(1));
        assert!(
            harness.is_focused(text_box),
            "the arrow gave the keyboard back"
        );
        assert!(state.find_focused.get());
        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        assert_eq!(state.find_current.get(), Some(2), "and Enter still walks");

        click(&mut harness, "find-match-case");
        assert!(harness.is_focused(text_box), "so did the toggle");
        typed(&mut harness, "x");
        assert_eq!(
            state.find_query.get(),
            "pathx",
            "and letters still land in the box"
        );
        assert!(state.composer.get().is_none());
    }

    #[test]
    fn a_selection_taken_for_a_pattern_stands_for_itself() {
        let (mut harness, state) =
            harness_with("fn f(x) { x }\nfn g(x) { f(x) }\n", "fn f(x) { x }\n");
        state.find_options.update(|o| o.regex = true);
        let text = live(state).doc.get().text.clone();
        let at = text.find("f(x)").unwrap();
        live(state).cursor.update(|c| {
            c.set_insert(floem::views::editor::core::selection::Selection::region(
                at,
                at + 4,
                floem::views::editor::core::cursor::CursorAffinity::Backward,
            ))
        });
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        assert_eq!(state.find_query.get(), "f\\(x\\)");
        assert_eq!(
            state.find_matches.with(|m| m.len()),
            2,
            "the brackets are brackets"
        );
        assert_eq!(state.find_current.get(), Some(0));
    }

    #[test]
    fn escape_on_conversation_leaves_the_bar_alone() {
        let (mut harness, state) = harness();
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        state.show_tab(ItemTab::Conversation);
        harness.rebuild();
        harness.paint();
        let control = named(harness.root_id(), "agents-button").unwrap();
        control.request_focus();
        harness.rebuild();
        harness.dispatch_event(escape());
        harness.rebuild();
        assert!(
            state.find_open.get(),
            "Escape is not about a bar out of sight"
        );
        assert!(harness.is_focused(control), "and the keyboard stayed put");
    }

    #[test]
    fn escape_from_the_code_puts_the_bar_away_and_the_close_glyph_does_too() {
        let (mut harness, state) = harness();
        let content = live(state).content.unwrap();
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        harness.paint();
        typed(&mut harness, "path");

        // Back to the code, with the bar still up and its tints on.
        content.request_focus();
        harness.rebuild();
        assert!(harness.is_focused(content));
        assert!(!state.find_focused.get());
        assert!(state.find_open.get());
        harness.dispatch_event(escape());
        harness.rebuild();
        harness.paint();
        assert!(!state.find_open.get(), "Escape from the code");

        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        harness.paint();
        assert!(state.find_open.get());
        assert_eq!(
            state.find_current.get(),
            Some(0),
            "the caret is still on the first match"
        );
        assert_eq!(count(&harness), "1 of 4");
        click(&mut harness, "find-close");
        assert!(!state.find_open.get(), "Close");
        assert!(harness.is_focused(content));
    }

    #[test]
    fn a_match_inside_a_fold_is_revealed_on_the_way_to_it() {
        let context = |from: usize, n: usize| {
            (from..from + n)
                .map(|i| format!("line {i}\n"))
                .collect::<String>()
        };
        let head = format!("new top\n{}new bottom\n", context(0, 60));
        let base = format!("old top\n{}old bottom\n", context(0, 60));
        let (mut harness, state) = harness_with(&head, &base);
        assert!(state.folded("src/x.rs"));
        let live = live(state);
        // The new top, the old top's ghost, then line 0: line 30 is display
        // line 32, behind the divider.
        assert!(
            live.doc.get().row_of_display(32).is_none(),
            "line 30 is behind the divider"
        );

        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        typed(&mut harness, "line 30");
        assert_eq!(
            *state.find_matches.get(),
            vec![m(32, 0, 7)],
            "found though folded"
        );
        assert_eq!(state.find_current.get(), Some(0));
        let row = live
            .doc
            .get()
            .row_of_display(32)
            .expect("the fold was revealed around it");
        let at = live.doc.get().offset_of_row(row);
        assert_eq!(selection(state), (at, at + 7));
        assert_eq!(&live.doc.get().text[at..at + 7], "line 30");
    }

    #[test]
    fn in_a_split_the_head_side_is_searched_and_a_ghost_is_not() {
        let (mut harness, state) = harness();
        state.layout.set(Layout::Split);
        harness.rebuild();
        harness.paint();
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        typed(&mut harness, "path");
        assert_eq!(
            *state.find_matches.get(),
            vec![m(0, 4, 8), m(2, 8, 12), m(4, 8, 12)],
            "the ghost's path has no row on the head side"
        );
        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        assert_eq!(state.find_current.get(), Some(1));
        let right = live(state);
        let (lo, hi) = selection(state);
        assert_eq!(&right.doc.get().text[lo..hi], "Path");
        let row = right.doc.get().row_of_display(2).unwrap();
        assert_eq!(lo, right.doc.get().offset_of_row(row) + 8);

        // Back to stacked: the ghost's is back, and the bar starts over.
        state.layout.set(Layout::Stacked);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.find_matches.with(|m| m.len()), 4);
        assert_eq!(state.find_current.get(), None);
    }

    #[test]
    fn leaving_the_item_puts_the_bar_away_and_keeps_the_query() {
        let (mut harness, state) = harness();
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        typed(&mut harness, "path");
        state.find_options.update(|o| o.match_case = true);
        harness.rebuild();
        state.close();
        harness.rebuild();
        harness.paint();
        assert!(!state.find_open.get());
        assert!(!state.find_focused.get());
        assert!(state.find_matches.get().is_empty());
        assert_eq!(state.find_current.get(), None);
        assert_eq!(state.find_query.get(), "path");
        assert!(state.find_options.get().match_case);
    }

    #[test]
    fn back_from_conversation_the_bar_is_up_again_but_the_keyboard_stays_put() {
        let (mut harness, state) = harness();
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        typed(&mut harness, "path");
        assert!(state.find_focused.get());

        state.show_tab(ItemTab::Conversation);
        harness.rebuild();
        harness.paint();
        assert!(state.find_box.get().is_none(), "the box went with the pane");
        assert!(
            !state.find_focused.get(),
            "and the code's keys are back, though the box was never told"
        );
        // `a` on Conversation opens the composer, as with no bar.
        let control = named(harness.root_id(), "agents-button").unwrap();
        control.request_focus();
        harness.rebuild();
        harness.dispatch_event(key(
            Code::KeyA,
            Key::Character("a".into()),
            Modifiers::empty(),
        ));
        harness.rebuild();
        assert!(
            state.composer.get().is_some(),
            "`a` reached the code's keys"
        );
        state.close_composer();

        state.show_tab(ItemTab::Code);
        harness.rebuild();
        harness.paint();
        assert!(state.find_open.get(), "still up");
        let text_box = named(harness.root_id(), "find-box").expect("built again");
        assert!(
            !harness.is_focused(text_box),
            "but not asked for the keyboard this time"
        );
        assert!(!state.find_focused.get());
        assert_eq!(state.find_query.get(), "path");
        assert_eq!(state.find_matches.with(|m| m.len()), 4);

        // Cmd-F asks.
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        assert!(harness.is_focused(text_box));
    }

    #[test]
    fn cmd_f_does_nothing_over_conversation_or_a_picture() {
        let (mut harness, state) = harness();
        state.tab.set(ItemTab::Conversation);
        harness.rebuild();
        harness.paint();
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        assert!(!state.find_open.get());

        state.tab.set(ItemTab::Code);
        state.open_tab("a.svg");
        state.finish_load(
            "a.svg",
            Arc::new(file(
                "a.svg",
                "<svg xmlns=\"http://www.w3.org/2000/svg\"/>\n",
                "",
            )),
        );
        harness.rebuild();
        harness.paint();
        assert!(state.drawn_picture().is_some(), "the file draws");
        harness.dispatch_event(cmd(Code::KeyF, "f"));
        harness.rebuild();
        assert!(!state.find_open.get());
    }
}
