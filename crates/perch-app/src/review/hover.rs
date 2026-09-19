//! Asking the language server about whatever the reader just clicked.
//!
//! Driven by the editor's cursor rather than by raw pointer coordinates.
//! floem places the cursor with its own hit-testing, which already accounts
//! for scrolling, the hidden gutter, and per-line heights; recomputing that
//! from a pointer position would be duplicating logic that is easy to get
//! subtly wrong and hard to see wrong. Where the cursor's offset lands in
//! the head file is the document's to say, in `review::doc`.

/// A byte offset in the buffer, as line and column.
///
/// Columns are byte offsets within the line, which is what the protocol's
/// UTF-8 position encoding wants and what the highlight spans already use.
pub fn line_col_of_offset(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let mut line = 0usize;
    let mut line_start = 0usize;

    for (i, byte) in text.bytes().enumerate() {
        if i >= offset {
            break;
        }
        if byte == b'\n' {
            line += 1;
            line_start = i + 1;
        }
    }

    (line, offset.saturating_sub(line_start))
}

/// Trim a hover reply to what fits in a popup.
///
/// rust-analyzer returns the whole doc comment, which for a well-documented
/// type runs to dozens of lines and would cover the code being reviewed.
pub fn condense(text: &str, max_lines: usize) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut blank_run = 0;

    for line in text.lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            // Collapse the runs of blank lines markdown leaves behind.
            if blank_run > 1 || out.is_empty() {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push(line);
        if out.len() >= max_lines {
            out.push("…");
            break;
        }
    }

    while out.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        out.pop();
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_resolve_to_a_line_and_column() {
        let text = "one\ntwo\nthree\n";
        assert_eq!(line_col_of_offset(text, 0), (0, 0));
        assert_eq!(line_col_of_offset(text, 2), (0, 2));
        assert_eq!(line_col_of_offset(text, 4), (1, 0));
        assert_eq!(line_col_of_offset(text, 9), (2, 1));
    }

    #[test]
    fn an_offset_past_the_end_clamps_rather_than_panicking() {
        let text = "one\ntwo\n";
        let (line, _) = line_col_of_offset(text, 9_999);
        assert!(line <= text.lines().count());
    }

    #[test]
    fn columns_are_byte_offsets_so_wide_characters_line_up() {
        // "é" is two bytes; the protocol's UTF-8 encoding counts bytes.
        let text = "let é = 1;\n";
        let (_, col) = line_col_of_offset(text, 6);
        assert_eq!(col, 6);
    }

    #[test]
    fn long_hover_text_is_cut_to_fit() {
        let text = (1..=40).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let short = condense(&text, 10);
        assert_eq!(short.lines().count(), 11, "ten lines plus an ellipsis");
        assert!(short.ends_with('…'));
    }

    #[test]
    fn blank_runs_are_collapsed() {
        let condensed = condense("a\n\n\n\nb", 20);
        assert_eq!(condensed, "a\n\nb");
    }

    #[test]
    fn leading_and_trailing_blanks_are_dropped() {
        assert_eq!(condense("\n\nhello\n\n\n", 20), "hello");
    }

    #[test]
    fn short_text_passes_through_unchanged() {
        assert_eq!(condense("just this", 10), "just this");
    }
}
