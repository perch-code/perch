//! From the Changescape tab into Code. Changescape's line numbers are of
//! the whole branch, its merge base and head, so the scope goes to Branch
//! first. An element on the head opens there at its first line. One only
//! on the base, a removed function, has no line in Code to open, so its
//! file opens split, at the head line its diff comes nearest to.

use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use perch_changescape::page::lines;
use perch_changescape::v4::{self, Element, RowsItem};
use perch_changescape::LoadedPage;
use perch_git::Scope;

use crate::backend::Backend;
use crate::review::model::{Layout, ReviewState};

/// Open what an element id names. A stale page's lines may be off; the
/// file opens at them anyway, under the tab's banner.
pub fn open_element(state: ReviewState, backend: &Backend, page: &LoadedPage, el: &str) {
    let Some(element) = page.elements.get(el) else {
        return;
    };
    match target(element) {
        Some(Target::Whole(path)) => {
            branch_scope(state, backend);
            state.go_to_file(&path, None);
        }
        Some(Target::Head(path, line)) => {
            branch_scope(state, backend);
            state.go_to_file(&path, Some((line.saturating_sub(1) as usize, 0)));
        }
        Some(Target::BaseOnly) => {
            if let Some(r) = base_ref(element) {
                show_in_diff(state, backend, r);
            }
        }
        None => {}
    }
}

/// Open a file at a 1-based head line, as a table cell names one.
pub fn open_file_line(state: ReviewState, backend: &Backend, path: &str, line: Option<u32>) {
    branch_scope(state, backend);
    let at = line.map(|line| (line.saturating_sub(1) as usize, 0));
    state.go_to_file(path, at);
}

/// A base-only element: its file, split, at the head line its own diff
/// comes nearest to, or at the line its base range starts on when its
/// diff names none.
fn show_in_diff(state: ReviewState, backend: &Backend, r: &v4::Ref) {
    let Some(path) = r.base_path.as_deref() else {
        return;
    };
    state.layout.set(Layout::Split);
    let line = nearest_head_line(r);
    open_file_line(state, backend, path, line);
}

#[derive(Debug, PartialEq, Eq)]
enum Target {
    /// A module: the whole file.
    Whole(String),
    /// The first ref with a head range: its file and first line.
    Head(String, u32),
    /// On the base side only.
    BaseOnly,
}

fn target(element: &Element) -> Option<Target> {
    if let Some((path, (start, _))) = element
        .refs
        .iter()
        .find_map(|r| Some((r.head_path.as_ref()?, lines(r.head.as_ref()?)?)))
    {
        return Some(Target::Head(path.clone(), start));
    }
    if let Some(r) = element.refs.iter().find(|r| r.whole_file) {
        let path = r.head_path.as_ref().or(r.base_path.as_ref())?;
        return Some(Target::Whole(path.clone()));
    }
    element
        .refs
        .iter()
        .any(|r| r.base.is_some())
        .then_some(Target::BaseOnly)
}

/// The base-side ref of an element: the first with a base range.
fn base_ref(element: &Element) -> Option<&v4::Ref> {
    element.refs.iter().find(|r| r.base.is_some())
}

/// The first head line the ref's rows name, or the first line of its base
/// range when no row names a head line.
fn nearest_head_line(r: &v4::Ref) -> Option<u32> {
    r.rows
        .iter()
        .find_map(|row| match row {
            RowsItem::Add { head, .. } | RowsItem::Keep { head, .. } => u32::try_from(*head).ok(),
            RowsItem::Del { .. } => None,
        })
        .or(r.base.as_ref().and_then(lines).map(|(start, _)| start))
}

/// Line numbers on the page are of the whole branch.
pub fn branch_scope(state: ReviewState, backend: &Backend) {
    if state.scope.get_untracked() == Scope::Branch {
        return;
    }
    let Some(item_id) = state
        .item
        .with_untracked(|item| item.as_ref().map(|item| item.id.clone()))
    else {
        return;
    };
    backend.set_scope(&item_id, Scope::Branch);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An element with the given refs, the rest of its fields filled in.
    fn element(refs: &str) -> Element {
        serde_json::from_str(&format!(
            r#"{{ "appears": [], "detail": null, "hash": null, "kind": "function",
                "label": "f()", "name": "f", "refs": {refs}, "status": "changed",
                "title": "f()", "url": null }}"#
        ))
        .unwrap()
    }

    #[test]
    fn the_first_head_range_wins() {
        let e = element(
            r#"[ { "base": { "start": 1, "end": 2 }, "base_path": "a.py", "head": null, "head_path": null, "rows": [], "whole_file": false },
                 { "base": null, "base_path": null, "head": { "start": 40, "end": 71 }, "head_path": "b.py", "rows": [], "whole_file": false } ]"#,
        );
        assert_eq!(target(&e), Some(Target::Head("b.py".into(), 40)));
    }

    #[test]
    fn a_module_opens_its_file() {
        let e = element(
            r#"[ { "base": null, "base_path": "a.py", "head": null, "head_path": "a.py", "rows": [], "whole_file": true } ]"#,
        );
        assert_eq!(target(&e), Some(Target::Whole("a.py".into())));
    }

    #[test]
    fn a_removed_function_is_base_only() {
        let e = element(
            r#"[ { "base": { "start": 10, "end": 12 }, "base_path": "a.py", "head": null, "head_path": null,
                   "rows": [ { "op": "del", "base": 10, "text": "def f():" } ], "whole_file": false } ]"#,
        );
        assert_eq!(target(&e), Some(Target::BaseOnly));
        assert_eq!(nearest_head_line(base_ref(&e).unwrap()), Some(10));
    }
}
