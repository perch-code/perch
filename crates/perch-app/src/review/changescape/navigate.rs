//! From the Changescape tab into Code. Changescape's line numbers are of
//! the whole branch, its merge base and head, so the scope goes to Branch
//! first. An element on the head opens there at its first line. One only
//! on the base, a removed function, has no line in Code to open, so its
//! excerpt shows over the tab instead.

use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use perch_changescape::page::{lines, src_rows};
use perch_changescape::v3::{Element, ElementRef as Ref};
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
        Some(Target::BaseOnly) => state.changescape_popover.set(Some(el.to_string())),
        None => {}
    }
}

/// Open a file at a 1-based head line, as a table cell names one.
pub fn open_file_line(state: ReviewState, backend: &Backend, path: &str, line: Option<u32>) {
    branch_scope(state, backend);
    let at = line.map(|line| (line.saturating_sub(1) as usize, 0));
    state.go_to_file(path, at);
}

/// "Show in diff" on a base-only element: its file, split, at the head
/// line its own diff comes nearest to, or at the line its base range
/// starts on when its diff names none.
pub fn show_in_diff(state: ReviewState, backend: &Backend, r: &Ref) {
    state.changescape_popover.set(None);
    state.layout.set(Layout::Split);
    let line = nearest_head_line(r);
    open_file_line(state, backend, &r.f, line);
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
    if let Some((f, (start, _))) = element
        .refs
        .iter()
        .find_map(|r| Some((&r.f, lines(&r.head)?)))
    {
        return Some(Target::Head(f.clone(), start));
    }
    if let Some(r) = element.refs.iter().find(|r| r.file == Some(true)) {
        return Some(Target::Whole(r.f.clone()));
    }
    element
        .refs
        .iter()
        .any(|r| r.base.is_some())
        .then_some(Target::BaseOnly)
}

/// The base-side ref the popover shows: the first with a base range.
pub fn base_ref(element: &Element) -> Option<&Ref> {
    element.refs.iter().find(|r| r.base.is_some())
}

fn nearest_head_line(r: &Ref) -> Option<u32> {
    src_rows(r)
        .iter()
        .find_map(|row| row.head)
        .or(lines(&r.base).map(|(start, _)| start))
}

/// Line numbers on the page are of the whole branch.
fn branch_scope(state: ReviewState, backend: &Backend) {
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

    fn element(refs: &str) -> Element {
        serde_json::from_str(&format!(
            r#"{{ "t": "f()", "k": "function", "st": "changed", "refs": {refs} }}"#
        ))
        .unwrap()
    }

    #[test]
    fn the_first_head_range_wins() {
        let e = element(
            r#"[ { "f": "a.py", "base": [1, 2], "head": null }, { "f": "b.py", "base": null, "head": [40, 71] } ]"#,
        );
        assert_eq!(target(&e), Some(Target::Head("b.py".into(), 40)));
    }

    #[test]
    fn a_module_opens_its_file() {
        let e = element(r#"[ { "f": "a.py", "base": null, "head": null, "file": true } ]"#);
        assert_eq!(target(&e), Some(Target::Whole("a.py".into())));
    }

    #[test]
    fn a_removed_function_is_base_only() {
        let e = element(
            r#"[ { "f": "a.py", "base": [10, 12], "head": null, "src": [[10, null, "-", "def f():"]] } ]"#,
        );
        assert_eq!(target(&e), Some(Target::BaseOnly));
        assert_eq!(nearest_head_line(base_ref(&e).unwrap()), Some(10));
    }
}
