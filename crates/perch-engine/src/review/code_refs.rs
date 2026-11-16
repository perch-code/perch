//! A symbol named in what people write, looked up through the language
//! servers' workspace symbol search: what is asked, and which of the
//! definitions that come back the name means. Finding the names in the
//! text and linking them is perch-app's `review::code_refs`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use perch_lsp::client::Defined;
use perch_lsp::lsp_types::{Location, SymbolKind};

use crate::review::model::Located;

/// The most definitions a name may have and still be linked to them.
pub const MAX_DEFINITIONS: usize = 10;

/// Whether a name reads as a type's, a trait's or a variant's: a capital,
/// then a small letter somewhere, which `WAL` and `T` do not have.
pub fn is_type_like(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_uppercase())
        && name.bytes().any(|b| b.is_ascii_lowercase())
}

/// A name to ask the language servers about: the name as written, which
/// its answer is kept under, its identifiers, qualifiers first, and the
/// one server to ask when a fence named its language. Asked of every
/// server otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameAsk {
    pub key: String,
    pub segments: Vec<String>,
    pub server: Option<perch_lsp::ServerKind>,
}

/// What a name's answer found: the definitions of its first `segments`
/// identifiers, which is all of them when the whole name resolved.
pub type Found = (usize, Arc<Vec<Located>>);

/// The definitions a name means, from the symbols the search found for
/// each of its identifiers, and how many of its identifiers they are the
/// definitions of: the whole name's when it resolves, else the longest
/// qualifier's that does and reads as a type, as `DbOptions` for
/// `DbOptions.ttl`, whose field the search does not index. `root` is the
/// worktree, for reading a module in a qualifier off a definition's path.
///
/// An identifier's definitions are those of its exact name inside what
/// qualifies it: the type or trait the search says holds it, or, for a
/// qualifier in lower case, a module or crate on the definition's path.
/// Types are kept over anything else of the name, so `Db` is the struct
/// and not a variant called `Db`; code written by hand over generated
/// code of the same name; and more than `MAX_DEFINITIONS` is too many to
/// mean one.
pub fn pick(
    segments: &[String],
    found: &HashMap<String, Vec<Defined>>,
    root: &Path,
) -> Option<(usize, Vec<Location>)> {
    (1..=segments.len()).rev().find_map(|k| {
        let name = &segments[k - 1];
        if k < segments.len() && !is_type_like(name) {
            return None;
        }
        let qualifier = k.checked_sub(2).map(|q| segments[q].as_str());
        let exact: Vec<&Defined> = found
            .get(name)?
            .iter()
            .filter(|d| d.name == *name)
            .collect();
        let mut chosen: Vec<&Defined> = match qualifier {
            None => exact,
            Some(qualifier) => {
                let held: Vec<&Defined> = exact
                    .iter()
                    .copied()
                    .filter(|d| d.container.as_deref().map(bare_type) == Some(qualifier))
                    .collect();
                if !held.is_empty() || qualifier.starts_with(|c: char| c.is_ascii_uppercase()) {
                    held
                } else {
                    exact
                        .into_iter()
                        .filter(|d| in_module(&d.location, qualifier, root))
                        .collect()
                }
            }
        };
        if chosen.iter().any(|d| is_type_kind(d.kind)) {
            chosen.retain(|d| is_type_kind(d.kind));
        }
        if chosen.iter().any(|d| !is_generated(&d.location)) {
            chosen.retain(|d| !is_generated(&d.location));
        }
        let mut seen = HashSet::new();
        let locations: Vec<Location> = chosen
            .into_iter()
            .filter(|d| seen.insert((d.location.uri.to_string(), d.location.range.start.line)))
            .map(|d| d.location.clone())
            .collect();
        (!locations.is_empty() && locations.len() <= MAX_DEFINITIONS).then_some((k, locations))
    })
}

/// A container as the type's bare name: `PeekingIterator<T>` as
/// `PeekingIterator`, `&'a Db` as `Db`.
fn bare_type(container: &str) -> &str {
    let name = container.split('<').next().unwrap_or(container).trim();
    let name = name.rsplit([' ', '&', '*']).next().unwrap_or(name);
    name.rsplit("::").next().unwrap_or(name)
}

/// Whether a definition is in the module or crate `module`: a directory
/// or a file of that name on its path within the worktree, a crate's
/// dashes read as underscores.
fn in_module(location: &Location, module: &str, root: &Path) -> bool {
    let Some(path) = perch_lsp::client::uri_to_path(location.uri.as_str()) else {
        return false;
    };
    let path = path.strip_prefix(root).unwrap_or(&path);
    path.iter().any(|part| {
        let part = part.to_string_lossy();
        let stem = part.strip_suffix(".rs").unwrap_or(&part);
        stem.replace('-', "_") == module
    })
}

/// Whether a definition is in generated code, as flatbuffers and bindgen
/// leave it: under a `generated` directory or in a `_generated` file.
fn is_generated(location: &Location) -> bool {
    location
        .uri
        .as_str()
        .split('/')
        .any(|part| part == "generated" || part.contains("_generated."))
}

fn is_type_kind(kind: SymbolKind) -> bool {
    kind == SymbolKind::STRUCT
        || kind == SymbolKind::ENUM
        || kind == SymbolKind::INTERFACE
        || kind == SymbolKind::CLASS
        || kind == SymbolKind::TYPE_PARAMETER
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_lsp::lsp_types::{Position, Range as LspRange, Uri};

    fn defined(
        name: &str,
        kind: SymbolKind,
        container: Option<&str>,
        path: &str,
        line: u32,
    ) -> Defined {
        let at = Position { line, character: 4 };
        Defined {
            name: name.into(),
            kind,
            container: container.map(str::to_string),
            location: Location {
                uri: format!("file:///w/{path}").parse::<Uri>().unwrap(),
                range: LspRange { start: at, end: at },
            },
        }
    }

    fn picked(
        segments: &[&str],
        found: &HashMap<String, Vec<Defined>>,
    ) -> Option<(usize, Vec<String>)> {
        let segments: Vec<String> = segments.iter().map(|s| s.to_string()).collect();
        pick(&segments, found, Path::new("/w")).map(|(k, locations)| {
            let at = locations
                .iter()
                .map(|l| {
                    format!(
                        "{}:{}",
                        l.uri.as_str().trim_start_matches("file:///w/"),
                        l.range.start.line
                    )
                })
                .collect();
            (k, at)
        })
    }

    #[test]
    fn a_name_picks_its_definitions() {
        let mut found: HashMap<String, Vec<Defined>> = HashMap::new();
        found.insert(
            "Db".into(),
            vec![
                defined("Db", SymbolKind::STRUCT, None, "slatedb/src/db.rs", 593),
                defined(
                    "Db",
                    SymbolKind::ENUM_MEMBER,
                    Some("ObjectStoreComponent"),
                    "slatedb/src/ios.rs",
                    52,
                ),
                defined(
                    "Db",
                    SymbolKind::STRUCT,
                    None,
                    "bindings/uniffi/src/db.rs",
                    18,
                ),
                defined(
                    "DbReader",
                    SymbolKind::STRUCT,
                    None,
                    "slatedb/src/db_reader.rs",
                    9,
                ),
            ],
        );
        found.insert(
            "open".into(),
            vec![
                defined(
                    "open",
                    SymbolKind::FUNCTION,
                    Some("Db"),
                    "slatedb/src/db.rs",
                    625,
                ),
                defined(
                    "open",
                    SymbolKind::FUNCTION,
                    Some("DbReader"),
                    "slatedb/src/db_reader.rs",
                    909,
                ),
                defined(
                    "open",
                    SymbolKind::FUNCTION,
                    Some("PeekingIterator<T>"),
                    "slatedb/src/p.rs",
                    1,
                ),
            ],
        );
        found.insert("ttl".into(), Vec::new());
        found.insert("db".into(), Vec::new());
        found.insert(
            "SsTableInfo".into(),
            vec![
                defined(
                    "SsTableInfo",
                    SymbolKind::STRUCT,
                    None,
                    "slatedb/src/db_state.rs",
                    496,
                ),
                defined(
                    "SsTableInfo",
                    SymbolKind::STRUCT,
                    None,
                    "slatedb/src/generated/root_generated.rs",
                    1207,
                ),
            ],
        );
        found.insert(
            "Table".into(),
            vec![defined(
                "Table",
                SymbolKind::STRUCT,
                None,
                "src/table_generated.rs",
                3,
            )],
        );
        found.insert(
            "new".into(),
            (0..40)
                .map(|i| {
                    defined(
                        "new",
                        SymbolKind::FUNCTION,
                        Some(&format!("T{i}")),
                        "x.rs",
                        i,
                    )
                })
                .collect(),
        );

        // The structs, not the variant of the same name, and never the
        // fuzzy match.
        assert_eq!(
            picked(&["Db"], &found),
            Some((
                1,
                vec![
                    "slatedb/src/db.rs:593".into(),
                    "bindings/uniffi/src/db.rs:18".into()
                ]
            ))
        );
        assert_eq!(
            picked(&["Db", "open"], &found),
            Some((2, vec!["slatedb/src/db.rs:625".into()]))
        );
        assert_eq!(
            picked(&["PeekingIterator", "open"], &found),
            Some((2, vec!["slatedb/src/p.rs:1".into()])),
            "a container's generics are not its name"
        );
        // A module or crate qualifies by the path.
        assert_eq!(
            picked(&["slatedb", "Db"], &found),
            Some((2, vec!["slatedb/src/db.rs:593".into()]))
        );
        assert_eq!(
            picked(&["uniffi", "Db"], &found),
            Some((2, vec!["bindings/uniffi/src/db.rs:18".into()]))
        );
        // A field is not indexed, so its type is what links.
        assert_eq!(
            picked(&["Db", "ttl"], &found),
            Some((
                1,
                vec![
                    "slatedb/src/db.rs:593".into(),
                    "bindings/uniffi/src/db.rs:18".into()
                ]
            ))
        );
        // Too many to mean one, alone; one inside its type.
        assert_eq!(picked(&["new"], &found), None);
        assert_eq!(
            picked(&["T7", "new"], &found),
            Some((2, vec!["x.rs:7".into()]))
        );
        assert_eq!(picked(&["Nothing"], &found), None);
        // Only a type stands in for a member it does not find.
        assert_eq!(picked(&["db", "ttl"], &found), None);
        // Generated code gives way to code written by hand, and is kept
        // when there is nothing else.
        assert_eq!(
            picked(&["SsTableInfo"], &found),
            Some((1, vec!["slatedb/src/db_state.rs:496".into()]))
        );
        assert_eq!(
            picked(&["Table"], &found),
            Some((1, vec!["src/table_generated.rs:3".into()]))
        );
        assert_eq!(bare_type("&'a Db"), "Db");
        assert_eq!(bare_type("BytesRange<'a>"), "BytesRange");
    }
}
