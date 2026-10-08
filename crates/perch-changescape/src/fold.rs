//! A table's rows as the page groups and folds them: the rows with no
//! group first, then a part for each of the table's `groups`, in the
//! page's order. In each part, the rows that are not minor show up to
//! `fold.show` of them, and the rest wait behind one control. Perch never
//! orders or ranks rows itself: the page wrote them in display order.

use crate::page::{rank, Rank, Table};
use crate::v3::{FacetOption, Fold, Row};

/// One part of a table: the rows with no group, or one group's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Part {
    /// The index of its group in the table's `groups`; `None` for the
    /// rows with no group.
    pub group: Option<usize>,
    /// The rows it shows folded, by index, in the page's order.
    pub shown: Vec<usize>,
    /// The rows behind its control.
    pub folded: Vec<usize>,
    /// How many of the folded rows are minor.
    pub minor: usize,
    /// It holds a row that must show, so it opens whatever its state.
    pub pinned: bool,
}

/// The parts of `table`, each with the rows `keep` keeps. A row that
/// `pinned` names never folds. A part with no rows is left out.
pub fn parts(
    table: &Table,
    keep: impl Fn(&Row) -> bool,
    pinned: impl Fn(&Row) -> bool,
) -> Vec<Part> {
    let show = table
        .fold
        .as_ref()
        .and_then(|fold| fold.show)
        .and_then(|n| usize::try_from(n.get()).ok())
        .unwrap_or(usize::MAX);
    let group_of = |row: &Row| {
        row.group
            .as_ref()
            .and_then(|id| table.groups.iter().position(|group| &group.id == id))
    };
    std::iter::once(None)
        .chain((0..table.groups.len()).map(Some))
        .filter_map(|group| {
            let mut part = Part {
                group,
                ..Part::default()
            };
            let mut normal = 0;
            for (i, row) in table.rows.iter().enumerate() {
                if group_of(row) != group || !keep(row) {
                    continue;
                }
                let minor = rank(row) == Rank::Minor;
                let past = !minor && {
                    normal += 1;
                    normal > show
                };
                let pin = pinned(row);
                part.pinned |= pin;
                if (minor || past) && !pin {
                    part.folded.push(i);
                    part.minor += usize::from(minor);
                } else {
                    part.shown.push(i);
                }
            }
            (!part.shown.is_empty() || !part.folded.is_empty()).then_some(part)
        })
        .collect()
}

/// What a part's control says while its rows are folded: how many, and
/// what the minor ones are where the page names them.
pub fn more_label(part: &Part, fold: Option<&Fold>) -> String {
    let n = part.folded.len();
    let label = fold.and_then(|fold| fold.label.as_deref());
    match label {
        Some(label) if part.minor == n => format!("Show {n} more · {label}"),
        Some(label) if part.minor > 0 => {
            format!("Show {n} more ({} minor: {label})", part.minor)
        }
        _ => format!("Show {n} more"),
    }
}

/// Whether a facet option leaves a row out: the row names an element, and
/// the option does not hold it. A row with no element is never out.
pub fn outside(option: Option<&FacetOption>, row: &Row) -> bool {
    match (option, &row.el) {
        (Some(option), Some(el)) => !option.els.iter().any(|e| e == el),
        _ => false,
    }
}
