//! A table's rows in the order the reader asks for: sorted by a column,
//! and kept to those that hold what the filter says. With neither, the
//! order is the skill's, which ranks them.

use std::cmp::Ordering;

use perch_changescape::page::{Cell, Table};

/// The column a table is sorted by, and which way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub column: usize,
    pub descending: bool,
}

/// The next sort when the header of `column` is clicked: a new column
/// starts the way its values read best, the same column turns over, and
/// a third click goes back to the skill's order.
pub fn next(sort: Option<Sort>, column: usize, numeric: bool) -> Option<Sort> {
    match sort {
        Some(s) if s.column == column && s.descending != numeric => None,
        Some(s) if s.column == column => Some(Sort {
            column,
            descending: !s.descending,
        }),
        _ => Some(Sort {
            column,
            descending: numeric,
        }),
    }
}

/// Whether a column reads as numbers: each cell that has anything has a
/// value or a `+a −b` count. Its first sort is the largest first.
pub fn numeric(table: &Table, key: &str) -> bool {
    let mut any = false;
    for row in &table.rows {
        let Some(cell) = row.cells.get(key) else {
            continue;
        };
        if cell.text.trim().is_empty() && cell.value.is_none() {
            continue;
        }
        if number(cell).is_none() {
            return false;
        }
        any = true;
    }
    any
}

/// A cell's number: its value, or the lines a `+534 −3` count names.
fn number(cell: &Cell) -> Option<f64> {
    cell.value.or_else(|| churn(&cell.text))
}

/// The lines added and removed in `+534 −3`, `+6 -1` or `+12`, together.
fn churn(text: &str) -> Option<f64> {
    let mut total = 0.0;
    let mut parts = 0;
    for part in text.split_whitespace() {
        let digits = part
            .strip_prefix('+')
            .or_else(|| part.strip_prefix('−'))
            .or_else(|| part.strip_prefix('-'))?;
        total += digits.replace(',', "").parse::<f64>().ok()?;
        parts += 1;
    }
    (parts > 0).then_some(total)
}

/// The rows to draw, by index: those matching `query`, in `sort`'s order.
/// Every word of the query must be in some cell of the row. Sorted, a
/// dimmed row stays under the others, and a row with nothing in the
/// column goes last either way.
pub fn order(table: &Table, sort: Option<Sort>, query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut rows: Vec<usize> = (0..table.rows.len())
        .filter(|&i| words.is_empty() || matches(table, i, &words))
        .collect();
    let Some(sort) = sort else {
        return rows;
    };
    let Some(column) = table.columns.get(sort.column) else {
        return rows;
    };
    let as_numbers = numeric(table, &column.key);
    let cell = |i: usize| table.rows[i].cells.get(&column.key);
    rows.sort_by(|&a, &b| {
        let (dim_a, dim_b) = (table.rows[a].dim, table.rows[b].dim);
        if dim_a != dim_b {
            return dim_a.cmp(&dim_b);
        }
        let (ca, cb) = (cell(a), cell(b));
        let (empty_a, empty_b) = (is_empty(ca), is_empty(cb));
        if empty_a || empty_b {
            return empty_a.cmp(&empty_b);
        }
        let (ca, cb) = (ca.unwrap(), cb.unwrap());
        let by = if as_numbers {
            number(ca)
                .unwrap_or(0.0)
                .total_cmp(&number(cb).unwrap_or(0.0))
        } else {
            natural(&ca.text, &cb.text)
        };
        if sort.descending {
            by.reverse()
        } else {
            by
        }
    });
    rows
}

fn is_empty(cell: Option<&Cell>) -> bool {
    cell.is_none_or(|cell| cell.text.trim().is_empty() && cell.value.is_none())
}

fn matches(table: &Table, row: usize, words: &[String]) -> bool {
    let text: String = table.rows[row]
        .cells
        .values()
        .map(|cell| cell.text.to_lowercase())
        .collect::<Vec<_>>()
        .join("\n");
    words.iter().all(|word| text.contains(word.as_str()))
}

/// Text compared as a person reads it: without case, and a run of digits
/// by its number, so `file9` comes before `file10`.
fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let run = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits
                };
                let (x, y) = (run(&mut a), run(&mut b));
                let (x, y) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let by = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
                if by != Ordering::Equal {
                    return by;
                }
            }
            (Some(x), Some(y)) => {
                let by = x.to_lowercase().cmp(y.to_lowercase());
                if by != Ordering::Equal {
                    return by;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(rows: serde_json::Value) -> Table {
        serde_json::from_value(serde_json::json!({
            "columns": [
                {"key": "name", "label": "Module", "kind": "mono"},
                {"key": "lines", "label": "+/−", "kind": "mono"},
            ],
            "rows": rows,
        }))
        .unwrap()
    }

    fn names(table: &Table, rows: &[usize]) -> Vec<String> {
        rows.iter()
            .map(|&i| table.rows[i].cells["name"].text.clone())
            .collect()
    }

    fn sample() -> Table {
        table(serde_json::json!([
            {"cells": {"name": "file10", "lines": "+6 −1"}},
            {"cells": {"name": "File9", "lines": "+534 −3"}},
            {"cells": {"name": "bot", "lines": "+900 −0"}, "dim": true},
            {"cells": {"name": "empty", "lines": ""}},
            {"cells": {"name": "file2", "lines": "+40 −40"}},
        ]))
    }

    #[test]
    fn with_no_sort_or_filter_the_order_is_the_skills() {
        let t = sample();
        assert_eq!(order(&t, None, ""), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn a_count_column_sorts_by_its_lines_with_dimmed_and_empty_rows_last() {
        let t = sample();
        assert!(numeric(&t, "lines"));
        assert!(!numeric(&t, "name"));
        let down = order(
            &t,
            Some(Sort {
                column: 1,
                descending: true,
            }),
            "",
        );
        assert_eq!(
            names(&t, &down),
            ["File9", "file2", "file10", "empty", "bot"]
        );
        let up = order(
            &t,
            Some(Sort {
                column: 1,
                descending: false,
            }),
            "",
        );
        assert_eq!(names(&t, &up), ["file10", "file2", "File9", "empty", "bot"]);
    }

    #[test]
    fn text_sorts_without_case_and_digits_by_their_number() {
        let t = sample();
        let up = order(
            &t,
            Some(Sort {
                column: 0,
                descending: false,
            }),
            "",
        );
        assert_eq!(names(&t, &up), ["empty", "file2", "File9", "file10", "bot"]);
    }

    #[test]
    fn every_word_of_the_filter_must_be_in_the_row() {
        let t = sample();
        assert_eq!(
            names(&t, &order(&t, None, "FILE")),
            ["file10", "File9", "file2"]
        );
        assert_eq!(names(&t, &order(&t, None, "file 534")), ["File9"]);
        assert!(order(&t, None, "nothing").is_empty());
    }

    #[test]
    fn a_header_click_turns_the_sort_over_and_then_off() {
        let numbers = next(None, 1, true);
        assert_eq!(
            numbers,
            Some(Sort {
                column: 1,
                descending: true
            })
        );
        let numbers = next(numbers, 1, true);
        assert_eq!(
            numbers,
            Some(Sort {
                column: 1,
                descending: false
            })
        );
        assert_eq!(next(numbers, 1, true), None);
        let text = next(None, 0, false);
        assert_eq!(
            text,
            Some(Sort {
                column: 0,
                descending: false
            })
        );
        assert_eq!(
            next(text, 1, true),
            Some(Sort {
                column: 1,
                descending: true
            })
        );
    }

    #[test]
    fn a_count_reads_with_either_minus_sign_and_commas() {
        assert_eq!(churn("+534 −3"), Some(537.0));
        assert_eq!(churn("+1,200 -20"), Some(1220.0));
        assert_eq!(churn("+12"), Some(12.0));
        assert_eq!(churn("Tests"), None);
        assert_eq!(churn(""), None);
    }
}
