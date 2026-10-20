//! What is under the pointer, and the tooltip that says what it is.
//!
//! The pointer's point is mapped to a byte offset by the editor's own
//! hit-testing, which already accounts for scrolling, the hidden gutter
//! and per-line heights. From there this module takes over: the offset to
//! a row and a column, the row to a head line through the document's row
//! table, and the column to the identifier around it. A row that is no
//! line of the head file, a ghost, a divider, a filler or a spacer, is
//! nowhere, and so is a column on whitespace or punctuation.
//!
//! The tooltip is floem's own, wrapped around the editor: floem owns the
//! timer, the placement and the closing, and this module owns what is in
//! it. The content is rust-analyzer's markdown, the signature in the code
//! font and the doc comment under it, cut so the tip stays a glance.

use floem::prelude::*;
use floem::style::{
    BorderBottomLeftRadius, BorderBottomRightRadius, BorderTopLeftRadius, BorderTopRightRadius,
};
use floem::views::{Decorators, Empty, Label, Stack};

use crate::review::doc::Doc;
use crate::review::markdown::{self, Block};
use crate::review::model::Symbol;
use crate::review::references::{self, identifier_at};
use crate::theme;

/// How long the pointer rests before the tooltip opens.
pub const TOOLTIP_DELAY_MILLIS: u64 = 500;
/// How long the pointer rests on one identifier before the server is
/// asked about it. Shorter than the tooltip's delay, so the answer is
/// usually there when the tip opens.
pub const ASK_DELAY_MILLIS: u64 = 150;
/// How many lines of the doc comment the tooltip shows.
pub const DOC_LINES: usize = 12;
/// How wide the tooltip grows before it wraps.
pub const TOOLTIP_WIDTH: f64 = 560.0;
/// How narrow the tooltip goes to stay inside the code pane when the
/// pointer is near its right edge, before it is moved left instead.
pub const TOOLTIP_MIN_WIDTH: f64 = 300.0;

/// How the tooltip fits beside the pointer: `width` is what it may grow
/// to, the room between the pointer and the pane's right edge up to the
/// full width, and `shift` is how far left it moves when even the
/// narrowest tip would not fit there. floem opens the tip at the pointer
/// and clamps nothing; this keeps a long signature on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub width: f64,
    pub shift: f64,
}

impl Fit {
    /// The fit for a pointer `room` pixels from the pane's right edge.
    pub fn in_room(room: f64) -> Fit {
        let room = room.max(0.0);
        if room >= TOOLTIP_MIN_WIDTH {
            Fit {
                width: room.min(TOOLTIP_WIDTH),
                shift: 0.0,
            }
        } else {
            Fit {
                width: TOOLTIP_MIN_WIDTH,
                shift: TOOLTIP_MIN_WIDTH - room,
            }
        }
    }

    /// The full width, for a pointer with room to spare.
    #[cfg(test)]
    pub fn full() -> Fit {
        Fit {
            width: TOOLTIP_WIDTH,
            shift: 0.0,
        }
    }
}

/// The identifier the character at a byte offset of an editor's document
/// belongs to, as a symbol in the head file `path`. The offset is a
/// character's, not a caret's: the one under the pointer, so a pointer on
/// the `(` after a name is on nothing, where a caret there would still be
/// on the name. `None` on a row that is no head line too.
pub fn symbol_at(doc: &Doc, path: &str, offset: usize) -> Option<Symbol> {
    // The character in the document, not the line's at the column: in the
    // indent a wrapped row repeats, the pointer is on a space.
    if !doc.text()[offset.min(doc.text().len())..]
        .chars()
        .next()
        .is_some_and(references::is_identifier_char)
    {
        return None;
    }
    let (row, column) = doc.locate(offset);
    let line = doc.head_line(row)?;
    let text = doc.source_line(row);
    let (start, text) = identifier_at(&text, column)?;
    Some(Symbol {
        path: path.to_string(),
        line,
        column: start,
        end: start + text.len(),
        text: text.to_string(),
    })
}

/// Trim a hover's doc comment to what fits in a tooltip.
///
/// rust-analyzer's markdown opens with fenced blocks, the symbol's path
/// and its signature, then a rule, then the doc comment. The blocks and
/// the rule are kept whole; the doc comment after them is cut at
/// `max_lines` with an ellipsis on a line of its own. A fence the cut
/// lands inside is closed first, so the ellipsis is not swallowed by it.
/// Runs of blank lines are collapsed on the way.
pub fn cut(markdown: &str, max_lines: usize) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut lines = markdown.lines().peekable();

    // The leading fenced blocks, and the rule after them.
    let mut in_fence = false;
    while let Some(line) = lines.peek() {
        let trimmed = line.trim();
        if in_fence {
            out.push(line);
            lines.next();
            if trimmed.starts_with("```") {
                in_fence = false;
            }
            continue;
        }
        if trimmed.starts_with("```") {
            in_fence = true;
            out.push(line);
            lines.next();
        } else if trimmed.is_empty() || trimmed == "---" {
            out.push(line);
            lines.next();
        } else {
            break;
        }
    }

    // The doc comment, cut at `max_lines` of it, blank lines between
    // paragraphs counted with the rest, since they take the same room.
    // The ellipsis goes on only when a line of text was left out, and
    // never after a blank line.
    let rest: Vec<&str> = lines.collect();
    let mut kept = 0usize;
    let mut blank_run = 0usize;
    let mut in_fence = false;
    for (i, line) in rest.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run > 1 || kept == 0 {
                continue;
            }
        } else {
            blank_run = 0;
            if trimmed.starts_with("```") {
                in_fence = !in_fence;
            }
        }
        out.push(line);
        kept += 1;
        if kept >= max_lines && rest[i + 1..].iter().any(|l| !l.trim().is_empty()) {
            if in_fence {
                // A fence with nothing in it yet is dropped rather than
                // closed empty.
                if out.last().is_some_and(|l| l.trim().starts_with("```")) {
                    out.pop();
                } else {
                    out.push("```");
                }
            }
            while out.last().is_some_and(|l| l.trim().is_empty()) {
                out.pop();
            }
            out.push("\u{2026}");
            break;
        }
    }

    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// The card inside the tooltip: the signature in the code font, and the
/// doc comment under it. Nothing but the card carries chrome, so a tip
/// with nothing to say draws nothing at all.
pub fn card(markdown: &str, fit: Fit) -> impl IntoView {
    let (signature, doc) = split(markdown::parse(&cut(markdown, DOC_LINES)));
    let alone = doc.is_empty();
    Stack::vertical((
        signature_view(signature, alone),
        if doc.is_empty() {
            Empty::new().into_any()
        } else {
            markdown::render_blocks(doc, theme::TEXT_META)
                .style(|s| {
                    // Not for dragging over, like the signature: the card
                    // is up only while the pointer rests.
                    s.padding_horiz(12.0)
                        .padding_vert(9.0)
                        .width_full()
                        .border_top(1.0)
                        .border_color(theme::border_muted())
                        .selectable(false)
                })
                .into_any()
        },
    ))
    .style(move |s| {
        s.flex_col()
            .max_width(fit.width)
            .margin_left(-fit.shift)
            .background(theme::canvas())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(theme::RADIUS_LG)
            .box_shadow_blur(14.0)
            .box_shadow_v_offset(4.0)
            .box_shadow_color(theme::shadow())
    })
}

/// The signature area: the symbol's path, muted and small, over its
/// signature in the code font. Whatever rust-analyzer put in the leading
/// fenced blocks, in order; a lone block is the signature. Its corners
/// follow the card's at the top, and at the bottom only when nothing
/// follows it; over the doc comment's rule they are square.
/// The signature area and the doc comment. rust-analyzer opens with the
/// symbol's path and then its signature, each in a fenced block, so every
/// leading block is the signature; then a rule, which the card's own
/// border stands in for; then the doc.
fn split(blocks: Vec<Block>) -> (Vec<String>, Vec<Block>) {
    let leading = blocks
        .iter()
        .take_while(|b| matches!(b, Block::Code { .. }))
        .count();
    let (signature, doc) = blocks.split_at(leading);
    let signature = signature
        .iter()
        .filter_map(|b| match b {
            Block::Code { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    let doc = doc
        .iter()
        .skip_while(|b| matches!(b, Block::Rule))
        .cloned()
        .collect();
    (signature, doc)
}

fn signature_view(blocks: Vec<String>, alone: bool) -> impl IntoView {
    let count = blocks.len();
    let children: Vec<_> = blocks
        .into_iter()
        .enumerate()
        .map(|(i, code)| {
            let is_path = count > 1 && i + 1 < count;
            Label::derived(move || code.clone())
                .style(move |s| {
                    let s = s
                        .font_family(theme::CODE_FONT_STACK.to_string())
                        .width_full()
                        .text_wrap()
                        .selectable(false);
                    if is_path {
                        s.font_size(theme::TEXT_SECTION)
                            .color(theme::fg_subtle())
                            .margin_bottom(4.0)
                    } else {
                        s.font_size(theme::code_font_size())
                            .font_weight(theme::weight_emphasis())
                            .color(theme::fg())
                            .line_height(theme::PROSE_LINE_HEIGHT)
                    }
                })
                .into_any()
        })
        .collect();
    Stack::vertical_from_iter(children).style(move |s| {
        // Inside the card's border, so a point less than its radius.
        let radius = theme::RADIUS_LG - 1.0;
        s.flex_col()
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(9.0)
            .background(theme::canvas_subtle())
            .set(BorderTopLeftRadius, radius)
            .set(BorderTopRightRadius, radius)
            .set(BorderBottomLeftRadius, if alone { radius } else { 0.0 })
            .set(BorderBottomRightRadius, if alone { radius } else { 0.0 })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::doc::{self, Run};
    use perch_git::DisplayBuffer;

    fn plain(text: &str) -> doc::Doc {
        doc::build(&DisplayBuffer::plain(text), &[], &[], None)
    }

    #[test]
    fn offsets_resolve_to_a_line_and_column() {
        let doc = plain("one\ntwo\nthree\n");
        assert_eq!(doc.locate(0), (0, 0));
        assert_eq!(doc.locate(2), (0, 2));
        assert_eq!(doc.locate(4), (1, 0));
        assert_eq!(doc.locate(9), (2, 1));
    }

    #[test]
    fn an_offset_past_the_end_clamps_rather_than_panicking() {
        let doc = plain("one\ntwo\n");
        let (line, _) = doc.locate(9_999);
        assert!(line < doc.len());
    }

    #[test]
    fn columns_are_byte_offsets_so_wide_characters_line_up() {
        // "é" is two bytes; the protocol's UTF-8 encoding counts bytes.
        let (_, col) = plain("let é = 1;\n").locate(6);
        assert_eq!(col, 6);
    }

    /// A changed file: `old` became `new` on the second line, and the
    /// rest is context, so the stacked buffer has a ghost of `old` before
    /// `new`.
    fn diffed() -> DisplayBuffer {
        let head = "fn alpha() {}\nfn new_name() {}\nfn gamma() {}\nfn delta() {}\nfn epsilon() {}\nfn zeta() {}\nfn eta() {}\nfn theta() {}\nfn iota() {}\nfn kappa() {}\n";
        let base = "fn alpha() {}\nfn old_name() {}\nfn gamma() {}\nfn delta() {}\nfn epsilon() {}\nfn zeta() {}\nfn eta() {}\nfn theta() {}\nfn iota() {}\nfn kappa() {}\n";
        let diff = perch_git::file_diff(base, head);
        DisplayBuffer::unified(head, base, &diff)
    }

    #[test]
    fn a_symbol_is_found_through_the_rows_of_a_document_with_a_ghost() {
        let buffer = diffed();
        let doc = doc::build(&buffer, &[], &[], None);
        // Row 0 is `fn alpha`, row 1 the ghost of `old_name`, row 2 `new_name`.
        assert_eq!(doc.head_line(1), None, "the ghost is no head line");
        let on_ghost = doc.offset_of_row(1) + 4;
        assert_eq!(symbol_at(&doc, "src/x.rs", on_ghost), None);

        let on_new = doc.offset_of_row(2) + 5;
        assert_eq!(
            symbol_at(&doc, "src/x.rs", on_new),
            Some(Symbol {
                path: "src/x.rs".into(),
                line: 1,
                column: 3,
                end: 11,
                text: "new_name".into(),
            })
        );

        // Punctuation and whitespace are nothing: the character under the
        // pointer, not the caret beside it, which would still be on the
        // name it follows.
        let on_paren = doc.offset_of_row(2) + 11;
        assert_eq!(symbol_at(&doc, "src/x.rs", on_paren), None);
        let on_space = doc.offset_of_row(0) + 2;
        assert_eq!(symbol_at(&doc, "src/x.rs", on_space), None);
        let on_f = doc.offset_of_row(0);
        assert_eq!(
            symbol_at(&doc, "src/x.rs", on_f).map(|s| s.text),
            Some("fn".into())
        );
        let past_the_end = doc.offset_of_row(0) + 13;
        assert_eq!(symbol_at(&doc, "src/x.rs", past_the_end), None);
    }

    #[test]
    fn a_symbol_below_a_fold_has_its_head_line_and_a_divider_has_none() {
        let buffer = diffed();
        // Fold display lines 4 to 8, head lines 3 to 7: rows are alpha,
        // ghost, new_name, gamma, a divider, then iota and kappa.
        let folds = vec![Run::new(4, 8, 3)];
        let doc = doc::build(&buffer, &folds, &[], None);
        assert!(doc.run_of(4).is_some(), "row 4 is the divider");
        assert_eq!(symbol_at(&doc, "src/x.rs", doc.offset_of_row(4)), None);

        let on_iota = doc.offset_of_row(5) + 3;
        let symbol = symbol_at(&doc, "src/x.rs", on_iota).expect("iota is a symbol");
        assert_eq!(symbol.text, "iota");
        assert_eq!(symbol.line, 8, "head line, past the fold");
        assert_eq!((symbol.column, symbol.end), (3, 7));
    }

    #[test]
    fn a_symbol_on_a_wrapped_row_is_at_its_column_of_the_whole_line() {
        let text = "fn f() {\n    call(first_argument, second_argument);\n}\n";
        let doc = doc::build(&DisplayBuffer::plain(text), &[], &[], Some(30));
        // The call's second row is `    second_argument);`, its own
        // indent then the rest of the line.
        let row = doc.row_of_head(1).unwrap() + 1;
        assert_eq!(doc.line(row), "    second_argument);");
        let symbol = symbol_at(&doc, "src/x.rs", doc.offset_of_row(row) + 6)
            .expect("second_argument is a symbol");
        assert_eq!(symbol.text, "second_argument");
        assert_eq!(symbol.line, 1);
        assert_eq!((symbol.column, symbol.end), (25, 40));
        // The indent the row repeats is only spaces.
        assert_eq!(
            symbol_at(&doc, "src/x.rs", doc.offset_of_row(row) + 3),
            None
        );
    }

    const HOVER: &str = "```rust\nslatedb::db::Db\n```\n\n```rust\npub async fn flush(&self) -> Result<(), crate::Error>\n```\n\n---\n\nFlush in-memory writes.\n\nMore.\n";

    #[test]
    fn the_signature_is_kept_whole_and_the_doc_comment_is_cut() {
        let short = cut(HOVER, 1);
        assert!(short.starts_with("```rust\nslatedb::db::Db\n```\n\n```rust\npub async fn flush"));
        assert!(short.contains("---"));
        assert!(
            short.ends_with("Flush in-memory writes.\n\u{2026}"),
            "{short}"
        );

        // Room for it all, or exactly enough: nothing is cut, and there is
        // no ellipsis.
        assert!(!cut(HOVER, 20).contains('\u{2026}'));
        assert!(!cut(HOVER, 3).contains('\u{2026}'));
    }

    #[test]
    fn a_cut_inside_a_fence_closes_the_fence_first() {
        let text =
            "```rust\nfn x()\n```\n\nDocs.\n\n```rust\nlet a = 1;\nlet b = 2;\nlet c = 3;\n```\n";
        let short = cut(text, 4);
        assert!(short.ends_with("let a = 1;\n```\n\u{2026}"), "{short}");
        // A cut on the fence's opening line drops the fence rather than
        // closing an empty one.
        assert!(
            cut(text, 3).ends_with("Docs.\n\u{2026}"),
            "{}",
            cut(text, 3)
        );
        let blocks = markdown::parse(&short);
        assert!(
            matches!(blocks.last(), Some(Block::Paragraph(_))),
            "the ellipsis is a paragraph of its own, not code: {blocks:?}"
        );
    }

    #[test]
    fn blank_runs_in_the_doc_comment_are_collapsed_and_count_once() {
        let text = "```rust\nfn x()\n```\n\na\n\n\n\nb\n";
        assert_eq!(cut(text, 20), "```rust\nfn x()\n```\n\na\n\nb");
        assert_eq!(
            cut(text, 3),
            "```rust\nfn x()\n```\n\na\n\nb",
            "a, the blank, and b fit"
        );
        // Cut after the blank: the ellipsis follows `a`, not the blank.
        assert_eq!(cut(text, 2), "```rust\nfn x()\n```\n\na\n\u{2026}");
        assert_eq!(cut(text, 1), "```rust\nfn x()\n```\n\na\n\u{2026}");
    }

    #[test]
    fn the_doc_comment_is_at_most_the_cap_tall_whatever_its_blanks() {
        let paragraphs = (1..=20)
            .map(|i| format!("Paragraph {i}."))
            .collect::<Vec<_>>()
            .join("\n\n");
        let text = format!("```rust\nfn x()\n```\n\n{paragraphs}\n");
        let short = cut(&text, DOC_LINES);
        let doc_lines = short.lines().skip(4).count();
        assert!(
            doc_lines <= DOC_LINES + 1,
            "{doc_lines} lines, the ellipsis among them"
        );
        assert!(short.ends_with('\u{2026}'));
    }

    #[test]
    fn a_hover_with_no_doc_comment_is_its_signature_alone() {
        assert_eq!(cut("```rust\nfn x()\n```\n", 12), "```rust\nfn x()\n```");
    }

    #[test]
    fn plain_text_passes_through_cut_unchanged() {
        assert_eq!(cut("just this", 10), "just this");
    }

    #[test]
    fn the_tip_narrows_to_the_room_beside_the_pointer_and_then_moves_left() {
        assert_eq!(
            Fit::in_room(900.0),
            Fit {
                width: TOOLTIP_WIDTH,
                shift: 0.0
            }
        );
        assert_eq!(
            Fit::in_room(400.0),
            Fit {
                width: 400.0,
                shift: 0.0
            }
        );
        assert_eq!(
            Fit::in_room(120.0),
            Fit {
                width: TOOLTIP_MIN_WIDTH,
                shift: TOOLTIP_MIN_WIDTH - 120.0
            }
        );
        assert_eq!(
            Fit::in_room(-5.0),
            Fit {
                width: TOOLTIP_MIN_WIDTH,
                shift: TOOLTIP_MIN_WIDTH
            }
        );
    }

    /// The card laid out in a window, so its size can be measured: wider
    /// text wraps inside the width rather than running past it.
    #[test]
    fn the_rule_after_the_signature_is_the_cards_border_not_a_second_line() {
        let (signature, doc) = split(markdown::parse(HOVER));
        assert_eq!(signature.len(), 2, "{signature:?}");
        assert_eq!(
            doc,
            vec![
                Block::Paragraph(vec![markdown::Span {
                    text: "Flush in-memory writes.".into(),
                    ..Default::default()
                }]),
                Block::Paragraph(vec![markdown::Span {
                    text: "More.".into(),
                    ..Default::default()
                }]),
            ]
        );
        // A signature with nothing after its rule stands alone.
        let (_, doc) = split(markdown::parse("```rust\nfn x()\n```\n\n---"));
        assert!(doc.is_empty(), "{doc:?}");
    }

    #[test]
    fn the_card_is_no_wider_than_its_width_and_wraps_inside_it() {
        use floem::headless::{HeadlessHarness, TestRoot};
        use floem::views::Decorators;
        use floem::{HasViewId, IntoView};

        let long = format!(
            "```rust\nslatedb::db::Db\n```\n\n```rust\npub async fn flush_with_options(&self, options: FlushOptions, wait_for_durability: bool) -> Result<(), crate::Error>\n```\n\n---\n\n{}\n",
            "Flush in-memory writes to object storage, and wait until they are durable. ".repeat(6)
        );
        let root = TestRoot::new();
        let view = card(&long, Fit::full())
            .into_view()
            .style(|s| s.position(floem::style::Position::Absolute));
        let id = view.view_id();
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        let rect = harness.get_layout_rect(id);
        assert!(rect.width() <= TOOLTIP_WIDTH + 0.5, "{} wide", rect.width());
        assert!(
            rect.width() > 400.0,
            "{} wide: the card takes its width",
            rect.width()
        );
        // Two signature lines, a rule, and a wrapped paragraph: taller than
        // one line of each.
        assert!(
            rect.height() > 4.0 * theme::code_line_height() as f64,
            "{} tall",
            rect.height()
        );

        // A short one is only as wide as its text.
        let root = TestRoot::new();
        let view = card("```rust\nfn x()\n```", Fit::full())
            .into_view()
            .style(|s| s.position(floem::style::Position::Absolute));
        let id = view.view_id();
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        let rect = harness.get_layout_rect(id);
        assert!(rect.width() < 200.0, "{} wide", rect.width());
        assert!(
            rect.height() < 3.0 * theme::code_line_height() as f64,
            "{} tall",
            rect.height()
        );
    }
}
