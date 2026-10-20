//! Word wrap: where a line of code breaks to fit the editor's width.
//!
//! The code's font is monospaced, so a width is a count of columns, and a
//! line is broken by counting rather than by laying it out. A line breaks
//! before a word, after the spaces or the `,` or `;` ahead of it, as an
//! editor's word wrap does, and a word wider than a whole row is broken
//! where the row ends. The rows after the first repeat the line's indent,
//! so a wrapped line reads as one line; an indent so deep it would leave
//! too little room for the rest is not repeated.
//!
//! Whitespace never breaks a line itself: it hangs past the edge, as the
//! spaces before a break do in any editor, and the word after it starts
//! the next row.

use unicode_width::UnicodeWidthChar;

/// The columns a tab advances to the next multiple of, as the editor sets
/// it.
const TAB: usize = 4;

/// No row is narrower than this many columns, however narrow the pane:
/// a line broken into a few letters a row is not code anyone can read,
/// and the pane scrolls sideways instead.
pub const MIN_COLUMNS: usize = 20;

/// The columns a character takes at column `column`.
fn width(c: char, column: usize) -> usize {
    if c == '\t' {
        TAB - column % TAB
    } else {
        c.width().unwrap_or(0)
    }
}

/// The columns `text` takes, starting at column `column`.
fn width_of(text: &str, column: usize) -> usize {
    text.chars().fold(column, |at, c| at + width(c, at)) - column
}

/// Whether a line may break after a character.
fn breaks_after(c: char) -> bool {
    c.is_whitespace() || c == ',' || c == ';'
}

/// Where a line breaks to fit rows `columns` wide: the byte offset each
/// row after the first starts at, in order, and how many bytes of the
/// line's indent those rows repeat. No breaks for a line that fits.
pub fn breaks(line: &str, columns: usize) -> (Vec<usize>, usize) {
    let columns = columns.max(MIN_COLUMNS);
    // Every character but a tab is no wider than its bytes.
    if line.len() <= columns && !line.contains('\t') {
        return (Vec::new(), 0);
    }
    let lead = line.len() - line.trim_start_matches([' ', '\t']).len();
    let lead_width = width_of(&line[..lead], 0);
    let (indent, indent_width) = if lead_width * 2 <= columns {
        (lead, lead_width)
    } else {
        (0, 0)
    };

    let mut starts = Vec::new();
    let mut row_start = 0;
    let mut column = 0;
    // Where the row could break: the start of the latest word on it that
    // follows a break.
    let mut opening: Option<usize> = None;
    let mut after_break = false;
    for (at, c) in line.char_indices() {
        if after_break && !c.is_whitespace() && at > row_start {
            opening = Some(at);
        }
        if !c.is_whitespace() && at > row_start && column + width(c, column) > columns {
            let start = opening.filter(|&o| o > row_start).unwrap_or(at);
            starts.push(start);
            row_start = start;
            column = indent_width + width_of(&line[start..at], indent_width);
            opening = None;
        }
        column += width(c, column);
        after_break = at >= lead && breaks_after(c);
    }
    let indent = if starts.is_empty() { 0 } else { indent };
    (starts, indent)
}

/// How many columns of code fit in `width` pixels of the editor, at
/// `advance` pixels a column, keeping `margin` pixels clear at the right
/// edge. `None` before the editor has a width.
pub fn columns_for(width: f64, advance: f64, margin: f64) -> Option<usize> {
    if width <= 0.0 || advance <= 0.0 {
        return None;
    }
    Some((((width - margin) / advance).floor().max(0.0) as usize).max(MIN_COLUMNS))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows a line is broken into, each with the indent it repeats.
    fn rows(line: &str, columns: usize) -> Vec<String> {
        let (starts, indent) = breaks(line, columns);
        let mut out = Vec::new();
        let mut from = 0;
        for (i, to) in starts.iter().copied().chain([line.len()]).enumerate() {
            let lead = if i == 0 { "" } else { &line[..indent] };
            out.push(format!("{lead}{}", &line[from..to]));
            from = to;
        }
        out
    }

    #[test]
    fn a_line_that_fits_is_one_row() {
        assert_eq!(breaks("let x = 1;", 20), (vec![], 0));
        let exactly = "a".repeat(20);
        assert_eq!(breaks(&exactly, 20), (vec![], 0));
    }

    #[test]
    fn a_long_line_breaks_before_the_word_that_would_cross_the_edge() {
        let line = "alpha beta gamma delta epsilon zeta";
        assert_eq!(rows(line, 20), ["alpha beta gamma ", "delta epsilon zeta"]);
        // Every byte is on exactly one row.
        let (starts, _) = breaks(line, 20);
        assert_eq!(starts, vec![17]);
    }

    #[test]
    fn the_rows_after_the_first_repeat_the_lines_indent() {
        let line = "    call(first_argument, second_argument, third_argument);";
        let wrapped = rows(line, 30);
        assert_eq!(
            wrapped,
            [
                "    call(first_argument, ",
                "    second_argument, ",
                "    third_argument);"
            ]
        );
        for row in &wrapped {
            assert!(row.chars().count() <= 30, "{row:?} fits");
        }
    }

    #[test]
    fn an_indent_too_deep_to_leave_room_is_not_repeated() {
        let line = format!("{}{}", " ".repeat(16), "word ".repeat(10));
        let (starts, indent) = breaks(&line, 24);
        assert!(!starts.is_empty());
        assert_eq!(indent, 0, "sixteen of twenty-four columns is too deep");
    }

    #[test]
    fn a_word_wider_than_a_row_is_broken_where_the_row_ends() {
        let line = "x".repeat(50);
        assert_eq!(
            rows(&line, 20),
            ["x".repeat(20), "x".repeat(20), "x".repeat(10)]
        );
    }

    #[test]
    fn the_indent_is_never_a_place_to_break() {
        // No break between the indent and the first word, however long.
        let line = format!("        {}", "y".repeat(30));
        let wrapped = rows(&line, 24);
        assert_eq!(wrapped[0], format!("        {}", "y".repeat(16)));
        assert!(wrapped[1..].iter().all(|r| r.starts_with("        y")));
    }

    #[test]
    fn spaces_at_a_break_hang_past_the_edge_rather_than_start_a_row() {
        let line = format!("{}     {}", "a".repeat(19), "b".repeat(5));
        assert_eq!(
            rows(&line, 20),
            [format!("{}     ", "a".repeat(19)), "b".repeat(5)]
        );
    }

    #[test]
    fn a_tab_is_as_wide_as_the_next_tab_stop() {
        // Two tabs take eight columns, so a row has twelve after them:
        // six of the twelve `z `s.
        let line = format!("\t\t{}", "z ".repeat(12));
        let wrapped = rows(&line, 20);
        let row = format!("\t\t{}", "z ".repeat(6));
        assert_eq!(wrapped, [row.clone(), row], "{wrapped:?}");
    }

    #[test]
    fn a_wide_character_takes_two_columns() {
        let line = "漢".repeat(15);
        let (starts, _) = breaks(&line, 20);
        // Ten to a row of twenty columns, three bytes each.
        assert_eq!(starts, vec![30]);
    }

    #[test]
    fn a_row_is_never_narrower_than_the_minimum() {
        let line = "word ".repeat(10);
        assert_eq!(breaks(&line, 5), breaks(&line, MIN_COLUMNS));
    }

    #[test]
    fn the_columns_that_fit_a_width() {
        assert_eq!(columns_for(0.0, 8.0, 12.0), None);
        assert_eq!(columns_for(812.0, 8.0, 12.0), Some(100));
        assert_eq!(columns_for(815.9, 8.0, 12.0), Some(100));
        assert_eq!(columns_for(100.0, 8.0, 12.0), Some(MIN_COLUMNS));
    }
}
