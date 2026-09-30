//! Sticky scroll: the first lines of the blocks the top of the code is
//! inside, `impl Default for X {` over `fn default() -> Self {`, pinned
//! over the code as the rest of each block scrolls under them.
//!
//! The blocks are the language server's outline of the file, its
//! `textDocument/documentSymbol`, so any server that answers it pins its
//! language's blocks with nothing here to teach. A block starts on the
//! line of its symbol's name, as VS Code's sticky scroll has it, and ends
//! on its range's last line. The server knows the file on disk, so an
//! outline is taken only while the head the editor shows is that text: a
//! commit's head pins nothing, and nor does the base side of a split. A
//! deleted file has no head and nothing on disk, so it pins its removed
//! lines instead, which are its base whole: the server is handed their
//! text under the file's path for as long as it takes to outline it.
//!
//! The layer places the blocks against the document the editor is laying
//! out, so a
//! block's first line is where its row is, and a line folded away stands
//! at the last row of its divider: a change deep in a long function whose
//! `fn` line is folded still pins the function. Each pinned line takes the
//! next slot down, and a block whose last line reaches the slot pushes its
//! line up out of the way, as the next block's first line would push it in
//! a list's section headers.
//!
//! A pinned line is its number, in a gutter as wide as the one beside the
//! code, and its text in the code's font and colours, slid sideways with
//! the code. A click scrolls its line back to where it is pinned, which
//! shows the block's first lines in place; the wheel over one scrolls the
//! code.

use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use floem::kurbo::{Rect, Vec2};
use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalWith};
use floem::style::Position;
use floem::text::{Attrs, AttrsList, FamilyOwned, LineHeightValue, TextLayout};
use floem::views::{dyn_stack, rich_text, ClipExt, Container, Decorators, Empty, Label, Stack};
use perch_git::ChangeStatus;

use crate::backend::Backend;
use crate::review::doc::Doc;
use crate::review::editor::{passes_wheel, row_tint};
use crate::review::model::{LiveFile, LoadedFile, ReviewState};
use crate::theme;
use crate::ui::widgets::clickable;

/// No more lines are pinned than this, however deep the nesting, as
/// VS Code caps its sticky scroll.
const MAX_PINNED: usize = 5;

/// A block placed in the document: the rows of its first and last lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Placed {
    pub line: usize,
    pub start_row: usize,
    pub end_row: usize,
}

/// A pinned line: the block's first line, the row it is on, and how far
/// down the layer it is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Pin {
    pub line: usize,
    pub start_row: usize,
    pub slot: usize,
    pub y: f64,
}

/// Where each block, `(first, last)` in head lines, is in the document.
/// `row_of` is the row a head line is on, `None` when it is folded away,
/// and `lines` how many lines the head has; a folded line is placed on the row before the next
/// line shown, which is its divider's last, or on the last row when no
/// line below it is shown.
pub(crate) fn place(
    blocks: &[(usize, usize)],
    rows: usize,
    lines: usize,
    row_of: impl Fn(usize) -> Option<usize>,
) -> Vec<Placed> {
    if blocks.is_empty() || rows == 0 {
        return Vec::new();
    }
    // Worked out once from the bottom up.
    let mut next = rows;
    let mut at = vec![0; lines];
    for line in (0..lines).rev() {
        at[line] = match row_of(line) {
            Some(row) => {
                next = row;
                row
            }
            None => next.saturating_sub(1),
        };
    }
    let at = |line: usize| at.get(line).copied().unwrap_or(rows - 1).min(rows - 1);
    blocks
        .iter()
        .map(|&(first, last)| Placed {
            line: first,
            start_row: at(first),
            end_row: at(last),
        })
        .collect()
}

/// The lines pinned at a scroll: each block, outermost first, whose first
/// row has gone above the next free slot and whose last row is still
/// below it, up to `max`. A slot's line is pushed up by the block's last
/// row as that row comes into the slot. Pure, for the tests.
pub(crate) fn pinned(placed: &[Placed], scroll_top: f64, line_height: f64, max: usize) -> Vec<Pin> {
    let mut out: Vec<Pin> = Vec::new();
    if line_height <= 0.0 {
        return out;
    }
    for p in placed {
        let slot = out.len();
        if slot >= max {
            break;
        }
        let slot_top = scroll_top + slot as f64 * line_height;
        // In order of first line, so every block after this one starts
        // lower still.
        if p.start_row as f64 * line_height >= slot_top {
            break;
        }
        let bottom = (p.end_row + 1) as f64 * line_height;
        if bottom <= slot_top {
            continue;
        }
        let y = (slot as f64 * line_height).min(bottom - scroll_top - line_height);
        out.push(Pin {
            line: p.line,
            start_row: p.start_row,
            slot,
            y,
        });
    }
    out
}

/// A hash of a text's lines, which says whether an outline was counted
/// in the lines the editor shows. Lines rather than the text, so that the
/// file's last newline, which the head's lines drop, is no difference.
pub(crate) fn lines_hash<'a>(lines: impl Iterator<Item = &'a str>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for line in lines {
        line.hash(&mut hasher);
    }
    hasher.finish()
}

/// Whether a file pins its base's blocks rather than its head's: a
/// deleted file, whose head is empty. Its removed lines are the base in
/// order, so a base line is the display line it is on, in the stacked
/// document and on the left of a split both.
pub(crate) fn pins_base(file: &LoadedFile) -> bool {
    file.status == Some(ChangeStatus::Deleted)
}

/// The lines the file's outline is counted in, as the file loaded them:
/// the buffer's head lines, or for a file that pins its base, its removed
/// lines.
fn outline_lines(file: &LoadedFile) -> impl Iterator<Item = &str> {
    let base = pins_base(file);
    file.buffer
        .text
        .lines()
        .zip(&file.buffer.rows)
        .filter(move |(_, row)| row.is_deleted() == base)
        .map(|(line, _)| line)
}

/// How many lines may be pinned in a view this tall: no more than a third
/// of it, so the code is never all header.
fn max_for(height: f64, line_height: f64) -> usize {
    ((height / line_height / 3.0).floor().max(0.0) as usize).min(MAX_PINNED)
}

/// The layer of pinned lines over an editor and its gutter: the stacked
/// layout's, or the right of a split, or its left for a deleted file.
/// `viewport` is that editor's, for the scroll down the file and the
/// text's slide sideways; `scroll_delta` is what a wheel over a pinned
/// line moves, down the file and across unless `across` takes the
/// sideways part, and `scroll_to` scrolls the file to a height.
///
/// Opening a file asks the server for its outline once it has the file.
/// A file open before the server came up had nobody to ask, so the layer
/// asks when the server does come up. A deleted file is not on disk for
/// the opening to announce, so the layer asks for its removed lines'
/// outline itself, unless it has it.
///
/// The file is read live: a reload is edited into the editor in place,
/// and the new head is the text the new outline is taken against and the
/// pinned lines are drawn from.
#[allow(clippy::too_many_arguments)]
pub(crate) fn layer(
    state: ReviewState,
    backend: Rc<Backend>,
    live_file: LiveFile,
    doc: RwSignal<Arc<Doc>>,
    viewport: RwSignal<Rect>,
    gutter_width: f64,
    scroll_delta: RwSignal<Vec2>,
    across: Option<RwSignal<Vec2>>,
    scroll_to: Rc<dyn Fn(f64)>,
) -> impl IntoView {
    let line_height = theme::CODE_LINE_HEIGHT as f64;
    let path = live_file.with_untracked(|f| f.path.clone());
    // Fixed for the editor's life: a reload that deletes the file, or
    // brings it back, builds another.
    let base = live_file.with_untracked(|f| pins_base(f));
    // The outline's hash and how many lines it is counted in.
    let counted = Memo::new(move |_| {
        live_file.with(|f| (lines_hash(outline_lines(f)), outline_lines(f).count()))
    });
    {
        let path = path.clone();
        Effect::new(move |was: Option<bool>| {
            let usable = state.lsp_status.with(|s| s.is_usable());
            if !usable {
                return false;
            }
            if !base {
                if was == Some(false) {
                    backend.document_symbols(&path);
                }
                return true;
            }
            let (hash, _) = counted.get_untracked();
            let known = state
                .symbols
                .with_untracked(|s| s.get(&path).is_some_and(|o| o.lines == hash));
            if was != Some(true) && !known {
                let text = live_file.with_untracked(|f| {
                    let mut text = outline_lines(f).collect::<Vec<_>>().join("\n");
                    text.push('\n');
                    text
                });
                backend.removed_symbols(&path, text);
            }
            true
        });
    }
    let outline = Memo::new(move |_| {
        let (hash, _) = counted.get();
        state.symbols.with(|symbols| {
            symbols
                .get(&path)
                .filter(|o| o.lines == hash)
                .map(|o| o.blocks.clone())
                .unwrap_or_default()
        })
    });
    let placed = Memo::new(move |_| {
        let (_, lines) = counted.get();
        outline.with(|blocks| {
            doc.with(|d| {
                place(blocks, d.len(), lines, |line| {
                    if base {
                        d.row_of_display(line)
                    } else {
                        d.row_of_head(line)
                    }
                })
            })
        })
    });
    let pins = Memo::new(move |_| {
        let vp = viewport.get();
        let max = max_for(vp.height(), line_height);
        placed.with(|p| pinned(p, vp.y0, line_height, max))
    });

    // Keyed by the file as well, so a line whose text a reload changed is
    // drawn again.
    let rows = dyn_stack(
        move || {
            let file = live_file.with(|f| Arc::as_ptr(f) as usize);
            pins.get()
                .into_iter()
                .map(|pin| (pin, file))
                .collect::<Vec<_>>()
        },
        |(pin, file)| (pin.line, pin.slot, *file),
        move |(pin, _)| {
            pinned_row(
                live_file.get_untracked(),
                doc,
                pins,
                pin,
                viewport,
                gutter_width,
                scroll_delta,
                across,
                scroll_to.clone(),
            )
        },
    )
    .style(|s| s.size_full());

    // The hairline under the last pinned line, and a soft shadow below
    // it, so the code reads as scrolling under the lines.
    let edge = Empty::new().style(move |s| {
        let bottom = pins.with(|p| p.iter().map(|pin| pin.y + line_height).fold(0.0, f64::max));
        s.absolute()
            .inset_top(bottom - 1.0)
            .inset_left(0.0)
            .width_full()
            .height(1.0)
            .background(theme::BORDER_MUTED)
            .box_shadow_blur(6.0)
            .box_shadow_v_offset(2.0)
            .box_shadow_color(theme::SHADOW)
            .apply_if(bottom <= 0.0, |s| s.hide())
    });

    Container::new(Stack::new((rows, edge)).style(|s| s.size_full()))
        .style(|s| s.size_full().position(Position::Relative))
        .clip()
        .style(|s| {
            s.absolute()
                .inset(0.0)
                .size_full()
                .pointer_events_none()
                .class(floem::views::LabelClass, |s| s.focus_none())
        })
}

/// One pinned line: its number over the gutter, its text over the code.
#[allow(clippy::too_many_arguments)]
fn pinned_row(
    file: Arc<LoadedFile>,
    doc: RwSignal<Arc<Doc>>,
    pins: Memo<Vec<Pin>>,
    pin: Pin,
    viewport: RwSignal<Rect>,
    gutter_width: f64,
    scroll_delta: RwSignal<Vec2>,
    across: Option<RwSignal<Vec2>>,
    scroll_to: Rc<dyn Fn(f64)>,
) -> impl IntoView {
    let line_height = theme::CODE_LINE_HEIGHT as f64;
    let Pin { line, slot, .. } = pin;
    let (text, attrs) = line_layout(&file, line);
    let width = {
        let mut layout = TextLayout::new();
        layout.set_text(&text, attrs.clone(), None);
        layout.size().width.ceil() + 1.0
    };
    let again = (text.clone(), attrs.clone());
    // The row's own tint, as the line is drawn in the code: a pinned `fn`
    // that the change added reads as added.
    let tint = move || {
        let row = pins.with(|p| p.iter().find(|p| p.line == line).map(|p| p.start_row));
        doc.with(|d| {
            row.and_then(|r| d.rows.get(r))
                .filter(|r| matches!(r, crate::review::doc::DocRow::Line { .. }))
                .and_then(row_tint)
        })
    };

    let number = Label::derived(move || (line + 1).to_string()).style(move |s| {
        s.width(gutter_width)
            .height_full()
            .flex_shrink(0.0_f32)
            .items_center()
            .selectable(false)
            .font_family(theme::CODE_FONT_STACK.to_string())
            .font_size(theme::CODE_FONT_SIZE)
            .color(theme::GUTTER_FG)
            .text_align(floem::text::Alignment::End)
            .padding_right(theme::GUTTER_NUMBER_PADDING + theme::GUTTER_BAR)
            .background(theme::GUTTER_BG)
            .border_right(1.0)
            .border_color(theme::BORDER_MUTED)
    });

    let text = rich_text(text, attrs, move || again.clone()).style(move |s| {
        // Read live, as the gutter reads its offset: the code slides
        // sideways under a row built before the scroll.
        s.absolute()
            .inset_top(0.0)
            .inset_left(theme::CODE_GUTTER_GAP - viewport.get().x0)
            .width(width)
            .height(line_height)
            .selectable(false)
    });
    let code = Container::new(text).clip().style(move |s| {
        s.flex_grow(1.0_f32)
            .min_width(0.0)
            .height_full()
            .position(Position::Relative)
            .background(theme::CODE_BG)
    });
    let code = Container::new(code).style(move |s| {
        s.flex_grow(1.0_f32)
            .min_width(0.0)
            .height_full()
            .apply_opt(tint(), |s, bg| s.background(bg))
    });

    let row = Stack::horizontal((number, code))
        .debug_name(format!("sticky:{line}"))
        .on_event_stop(el::Click, move |_, _| {
            let pin = pins.with_untracked(|p| p.iter().find(|p| p.line == line).copied());
            if let Some(pin) = pin {
                scroll_to(pin.start_row.saturating_sub(pin.slot) as f64 * line_height);
            }
        });
    passes_wheel(row, viewport, scroll_delta, across).style(move |s| {
        let y = pins.with(|p| p.iter().find(|p| p.line == line).map_or(0.0, |p| p.y));
        clickable(s)
            .absolute()
            .inset_top(y)
            .inset_left(0.0)
            .width_full()
            .height(line_height)
            .items_center()
            // An inner line slides up under the one it is inside.
            .z_index((MAX_PINNED - slot) as i32)
            .pointer_events_auto()
    })
}

/// A line of the outline's text and its colours, as the code draws it.
/// A tab is four spaces, as the editor sets it.
fn line_layout(file: &LoadedFile, line: usize) -> (String, AttrsList) {
    let raw = outline_lines(file).nth(line).unwrap_or("");
    let spans = if pins_base(file) {
        file.base_spans.line(line)
    } else {
        file.head_spans.line(line)
    };
    let (text, map) = expand_tabs(raw);
    let families: Vec<FamilyOwned> = FamilyOwned::parse_list(theme::CODE_FONT_STACK).collect();
    let default = Attrs::new()
        .family(&families)
        .font_size(theme::CODE_FONT_SIZE)
        .weight(theme::weight_body())
        .color(theme::FG)
        // As the editor sets its lines, so the text sits in the row where
        // the same line sits in the code.
        .line_height(LineHeightValue::Pt(theme::CODE_LINE_HEIGHT));
    let mut attrs = AttrsList::new(default.clone());
    for span in spans {
        let (start, end) = (map(span.start.min(raw.len())), map(span.end.min(raw.len())));
        if start < end {
            attrs.add_span(
                start..end,
                default.clone().color(theme::syntax_color(span.capture)),
            );
        }
    }
    (text, attrs)
}

/// A line with each tab as four spaces, and where a byte of the line is
/// in the result.
fn expand_tabs(line: &str) -> (String, impl Fn(usize) -> usize) {
    let tabs: Vec<usize> = line.match_indices('\t').map(|(i, _)| i).collect();
    let text = line.replace('\t', "    ");
    (text, move |offset: usize| {
        offset + 3 * tabs.partition_point(|&t| t < offset)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LH: f64 = 10.0;

    fn scope(start: usize, end: usize) -> (usize, usize) {
        (start, end)
    }

    /// Placed one row per line, nothing folded.
    fn flat(blocks: &[(usize, usize)], rows: usize) -> Vec<Placed> {
        place(blocks, rows, rows, |line| (line < rows).then_some(line))
    }

    fn lines(pins: &[Pin]) -> Vec<usize> {
        pins.iter().map(|p| p.line).collect()
    }

    #[test]
    fn nothing_is_pinned_at_the_top_of_the_file() {
        let placed = flat(&[scope(0, 20), scope(1, 10)], 30);
        assert!(pinned(&placed, 0.0, LH, 5).is_empty());
    }

    #[test]
    fn the_blocks_the_top_is_inside_are_pinned_outermost_first() {
        // impl over 0..=20, fn over 1..=10; scrolled five lines down.
        let placed = flat(&[scope(0, 20), scope(1, 10)], 30);
        let pins = pinned(&placed, 5.0 * LH, LH, 5);
        assert_eq!(lines(&pins), vec![0, 1]);
        assert_eq!(pins[0].y, 0.0);
        assert_eq!(pins[1].y, LH);
    }

    #[test]
    fn a_block_scrolled_past_is_not_pinned_but_the_next_one_is() {
        // Two functions in an impl; the top is in the second.
        let placed = flat(&[scope(0, 30), scope(1, 5), scope(7, 20)], 40);
        let pins = pinned(&placed, 10.0 * LH, LH, 5);
        assert_eq!(lines(&pins), vec![0, 7]);
    }

    #[test]
    fn a_blocks_last_line_pushes_its_pinned_line_up() {
        // fn over 1..=10 under impl 0..=20. At a scroll of 9.5 lines, the
        // fn's slot is from 10.5 to 11.5, and its last row ends at 11:
        // the line is pushed up by half a line.
        let placed = flat(&[scope(0, 20), scope(1, 10)], 30);
        let pins = pinned(&placed, 9.5 * LH, LH, 5);
        assert_eq!(lines(&pins), vec![0, 1]);
        assert!((pins[1].y - 0.5 * LH).abs() < 1e-9, "{pins:?}");
        // A line further and the fn is gone.
        let pins = pinned(&placed, 10.5 * LH, LH, 5);
        assert_eq!(lines(&pins), vec![0]);
    }

    #[test]
    fn no_more_than_the_cap_are_pinned() {
        let placed = flat(
            &[scope(0, 50), scope(1, 49), scope(2, 48), scope(3, 47)],
            60,
        );
        assert_eq!(lines(&pinned(&placed, 20.0 * LH, LH, 2)), vec![0, 1]);
        assert!(pinned(&placed, 20.0 * LH, LH, 0).is_empty());
    }

    #[test]
    fn a_folded_first_line_stands_at_its_dividers_last_row() {
        // Lines 0..=9 of a function are folded behind a two-row divider;
        // rows 0 and 1 are the divider, and line 10 is on row 2.
        let row_of = |line: usize| (line >= 10).then(|| line - 8);
        let placed = place(&[scope(3, 14)], 7, 15, row_of);
        assert_eq!(
            placed,
            vec![Placed {
                line: 3,
                start_row: 1,
                end_row: 6
            }]
        );
        // With the divider scrolled off, the folded `fn` is pinned.
        assert_eq!(lines(&pinned(&placed, 2.0 * LH, LH, 5)), vec![3]);
    }

    #[test]
    fn the_lines_hash_ignores_the_last_newline_and_nothing_else() {
        let hash = |text: &str| lines_hash(text.lines());
        assert_eq!(hash("a\nb\n"), hash("a\nb"));
        assert_ne!(hash("a\nb\n"), hash("a\nc\n"));
        assert_ne!(hash("ab\n"), hash("a\nb\n"));
    }

    #[test]
    fn a_tab_is_four_spaces_and_the_spans_move_with_it() {
        let (text, map) = expand_tabs("\tfn\tx");
        assert_eq!(text, "    fn    x");
        assert_eq!(map(1), 4, "fn");
        assert_eq!(map(4), 10, "x");
        assert_eq!(map(0), 0);
    }
}
