//! The code map of a page: the changed modules, by the packages they are
//! in, and the files in no module, by the tour's package of each. The
//! page names its modules and packages, so the map reads the change by
//! its structure rather than by its paths.
//!
//! A changed file is in the module whose file it is. A module is in the
//! longest package whose name is a prefix of its own, split on `.` or
//! `::`. A package is in the longest other package that is such a prefix.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::page::LoadedPage;
use crate::v4::{self, ElementKind, FileStatus, Status};

/// The structure of the changed files.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodeMap {
    /// The top packages, largest first: the real ones, then the groups of
    /// modules in no package.
    pub packages: Vec<Package>,
    /// The files in no module, one group per tour package, by lines.
    pub other: Vec<Other>,
}

impl CodeMap {
    /// Every file of the map, once.
    pub fn file_count(&self) -> usize {
        self.packages.iter().map(Package::file_count).sum::<usize>()
            + self.other.iter().map(|o| o.files.len()).sum::<usize>()
    }

    /// The added and removed lines of the map: the sum over its top nodes.
    pub fn totals(&self) -> (i64, i64) {
        let packages = self.packages.iter().map(|p| (p.adds, p.dels));
        let other = self.other.iter().map(|o| (o.adds, o.dels));
        packages
            .chain(other)
            .fold((0, 0), |(a, d), (x, y)| (a + x, d + y))
    }
}

/// A package with changed modules in it, directly or below.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    /// The name relative to the parent package, or the whole name at the top.
    pub name: String,
    /// The element's id, such as `pkg:changescape`. None for a group of
    /// modules in no package, named by the tour package of their files.
    pub element: Option<String>,
    pub status: Status,
    /// The lines of the files below, summed.
    pub adds: i64,
    pub dels: i64,
    /// The changed files below.
    pub files: usize,
    pub packages: Vec<Package>,
    pub modules: Vec<Module>,
}

impl Package {
    fn file_count(&self) -> usize {
        self.files
    }

    /// The paths of the changed files below.
    pub fn paths(&self) -> Vec<String> {
        let modules = self.modules.iter().flat_map(Module::paths);
        let packages = self.packages.iter().flat_map(Package::paths);
        modules.chain(packages).collect()
    }
}

/// A module with changed files: its last name segment and its files.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub name: String,
    /// The element's id, such as `mod:changescape.build`.
    pub element: String,
    pub status: Status,
    pub adds: i64,
    pub dels: i64,
    pub files: Vec<MapFile>,
}

impl Module {
    /// The paths of the module's files.
    pub fn paths(&self) -> Vec<String> {
        self.files.iter().map(|f| f.path.clone()).collect()
    }
}

/// A changed file of the map and its added and removed lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapFile {
    pub path: String,
    pub adds: i64,
    pub dels: i64,
}

/// The files of one tour package that no module holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Other {
    pub name: String,
    pub files: Vec<MapFile>,
    pub adds: i64,
    pub dels: i64,
}

/// The code map of `page`. Deterministic: the same page gives the same map.
pub fn code_map(page: &LoadedPage) -> CodeMap {
    let tour_map: &[v4::MapPackage] = match page.tour() {
        Some(tour) => &tour.map,
        None => &[],
    };
    let elements: BTreeMap<&str, &v4::Element> = page
        .elements
        .iter()
        .map(|(id, el)| (id.as_str(), el))
        .collect();

    // The module of each path, the most specific name winning.
    let mut module_at: HashMap<&str, &str> = HashMap::new();
    for (&id, el) in &elements {
        if el.kind != ElementKind::Module {
            continue;
        }
        for r in &el.refs {
            for path in [r.head_path.as_deref(), r.base_path.as_deref()]
                .into_iter()
                .flatten()
            {
                let longer = module_at
                    .get(path)
                    .is_none_or(|other| elements[other].name.len() < el.name.len());
                if longer {
                    module_at.insert(path, id);
                }
            }
        }
    }

    let mut in_module: BTreeMap<&str, Vec<MapFile>> = BTreeMap::new();
    let mut unmapped: Vec<MapFile> = Vec::new();
    for file in &page.files {
        let key = match file.status {
            FileStatus::Removed => &file.old_path,
            _ => &file.path,
        };
        let row = MapFile {
            path: file.path.clone(),
            adds: file.adds,
            dels: file.dels,
        };
        match module_at.get(key.as_str()) {
            Some(&id) => in_module.entry(id).or_default().push(row),
            None => unmapped.push(row),
        }
    }

    // Each package by name, the first by id if two share one.
    let mut by_name: BTreeMap<&str, (&str, Status)> = BTreeMap::new();
    for (id, el) in &elements {
        if el.kind == ElementKind::Package {
            by_name.entry(el.name.as_str()).or_insert((id, el.status));
        }
    }

    // The packages that hold a changed module, and the modules in each.
    let mut kept: BTreeSet<&str> = BTreeSet::new();
    let mut home: BTreeMap<&str, Vec<Module>> = BTreeMap::new();
    let mut orphans = Vec::new();
    for (id, mut files) in in_module {
        let el = elements[id];
        files.sort_by(by_lines);
        let holders: Vec<&str> = by_name
            .keys()
            .copied()
            .filter(|pkg| within(&el.name, pkg))
            .collect();
        kept.extend(holders.iter().copied());
        let home_pkg = holders.iter().max_by_key(|pkg| pkg.len()).copied();
        // A module named like its package is the package's own file, shown by its stem.
        let name = match (home_pkg, files.first()) {
            (Some(pkg), Some(file)) if pkg == el.name => file_stem(&file.path).to_string(),
            _ => last_segment(&el.name).to_string(),
        };
        let module = Module {
            name,
            element: id.to_string(),
            status: el.status,
            adds: files.iter().map(|f| f.adds).sum(),
            dels: files.iter().map(|f| f.dels).sum(),
            files,
        };
        match home_pkg {
            Some(pkg) => home.entry(pkg).or_default().push(module),
            None => orphans.push(module),
        }
    }

    // The parent of each kept package: the longest other kept prefix.
    let mut children: BTreeMap<Option<&str>, Vec<&str>> = BTreeMap::new();
    for &name in &kept {
        let parent = kept
            .iter()
            .copied()
            .filter(|other| *other != name && within(name, other))
            .max_by_key(|other| other.len());
        children.entry(parent).or_default().push(name);
    }
    let mut tree = Tree {
        by_name: &by_name,
        children: &children,
        home,
    };
    let mut packages = tree.packages(None);

    // The modules in no package, grouped by the tour package of their file.
    let mut orphan_groups: BTreeMap<String, Vec<Module>> = BTreeMap::new();
    for module in orphans {
        let path = module.files.first().map_or("", |f| f.path.as_str());
        orphan_groups
            .entry(tour_group(tour_map, path))
            .or_default()
            .push(module);
    }
    let mut groups_of_modules: Vec<Package> = orphan_groups
        .into_iter()
        .map(|(name, mut modules)| {
            modules.sort_by(by_size);
            let status = match modules.first() {
                Some(first) if modules.iter().all(|m| m.status == first.status) => first.status,
                _ => Status::Changed,
            };
            let files = modules.iter().map(|m| m.files.len()).sum();
            Package {
                name,
                element: None,
                status,
                adds: modules.iter().map(|m| m.adds).sum(),
                dels: modules.iter().map(|m| m.dels).sum(),
                files,
                packages: Vec::new(),
                modules,
            }
        })
        .collect();
    groups_of_modules.sort_by(|a, b| by_total(a.adds + a.dels, &a.name, b.adds + b.dels, &b.name));
    packages.extend(groups_of_modules);

    // The files in no module, by the tour package that holds each.
    let mut groups: BTreeMap<String, Vec<MapFile>> = BTreeMap::new();
    for file in unmapped {
        let group = tour_group(tour_map, &file.path);
        groups.entry(group).or_default().push(file);
    }
    let mut other: Vec<Other> = groups
        .into_iter()
        .map(|(name, mut files)| {
            files.sort_by(by_lines);
            Other {
                name,
                adds: files.iter().map(|f| f.adds).sum(),
                dels: files.iter().map(|f| f.dels).sum(),
                files,
            }
        })
        .collect();
    other.sort_by(|a, b| by_total(a.adds + a.dels, &a.name, b.adds + b.dels, &b.name));

    CodeMap { packages, other }
}

/// The packages of the tree under one parent, built with their modules.
struct Tree<'a> {
    by_name: &'a BTreeMap<&'a str, (&'a str, Status)>,
    children: &'a BTreeMap<Option<&'a str>, Vec<&'a str>>,
    home: BTreeMap<&'a str, Vec<Module>>,
}

impl<'a> Tree<'a> {
    /// The packages directly under `parent`, largest first.
    fn packages(&mut self, parent: Option<&'a str>) -> Vec<Package> {
        let names = self.children.get(&parent).cloned().unwrap_or_default();
        let mut out: Vec<Package> = names
            .into_iter()
            .map(|name| {
                let (element, status) = self.by_name[name];
                let packages = self.packages(Some(name));
                let mut modules = self.home.remove(name).unwrap_or_default();
                modules.sort_by(by_size);
                let shown = match parent {
                    Some(parent) => rest(name, parent),
                    None => name,
                };
                let files = packages.iter().map(Package::file_count).sum::<usize>()
                    + modules.iter().map(|m| m.files.len()).sum::<usize>();
                let adds = packages.iter().map(|p| p.adds).sum::<i64>()
                    + modules.iter().map(|m| m.adds).sum::<i64>();
                let dels = packages.iter().map(|p| p.dels).sum::<i64>()
                    + modules.iter().map(|m| m.dels).sum::<i64>();
                Package {
                    name: shown.to_string(),
                    element: Some(element.to_string()),
                    status,
                    adds,
                    dels,
                    files,
                    packages,
                    modules,
                }
            })
            .collect();
        out.sort_by(|a, b| by_total(a.adds + a.dels, &a.name, b.adds + b.dels, &b.name));
        out
    }
}

/// Whether `name` is `pkg` or below it, on a `.` or `::` boundary.
fn within(name: &str, pkg: &str) -> bool {
    name == pkg
        || name
            .strip_prefix(pkg)
            .is_some_and(|rest| rest.starts_with('.') || rest.starts_with("::"))
}

/// What is left of `name` below its `parent`, without the separator.
fn rest<'a>(name: &'a str, parent: &str) -> &'a str {
    let below = &name[parent.len()..];
    below
        .strip_prefix("::")
        .or_else(|| below.strip_prefix('.'))
        .unwrap_or(below)
}

/// The last segment of a dotted or `::` name.
fn last_segment(name: &str) -> &str {
    name.rsplit(['.', ':'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(name)
}

/// The file name of a path without its folder and extension.
fn file_stem(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    match file.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => file,
    }
}

/// The tour package that lists `path`, or else its top folder.
fn tour_group(tour_map: &[v4::MapPackage], path: &str) -> String {
    tour_map
        .iter()
        .find(|pkg| pkg.files.iter().any(|f| f == path))
        .map_or_else(|| top_folder(path), |pkg| pkg.package.clone())
}

/// The first folder of a path, or `(root)` for a file at the top.
fn top_folder(path: &str) -> String {
    match path.split_once('/') {
        Some((folder, _)) => folder.to_string(),
        None => "(root)".to_string(),
    }
}

/// Files by changed lines, largest first, then by path.
fn by_lines(a: &MapFile, b: &MapFile) -> std::cmp::Ordering {
    (b.adds + b.dels)
        .cmp(&(a.adds + a.dels))
        .then_with(|| a.path.cmp(&b.path))
}

/// Modules by changed lines, largest first, then by name.
fn by_size(a: &Module, b: &Module) -> std::cmp::Ordering {
    by_total(a.adds + a.dels, &a.name, b.adds + b.dels, &b.name)
}

/// Largest first, then by name.
fn by_total(a: i64, a_name: &str, b: i64, b_name: &str) -> std::cmp::Ordering {
    b.cmp(&a).then_with(|| a_name.cmp(b_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed Python page: a package `changescape` with a child
    /// `changescape.page`, and fixtures that no module holds.
    fn py() -> LoadedPage {
        let bytes = include_bytes!("../tests/fixtures/codemap/py-pr22.json");
        LoadedPage::read(std::path::Path::new("py-pr22.json"), bytes).unwrap()
    }

    /// A trimmed Rust page: `::` names, and one module in a package below.
    fn rs() -> LoadedPage {
        let bytes = include_bytes!("../tests/fixtures/codemap/rs-slatedb-2153.json");
        LoadedPage::read(std::path::Path::new("rs-slatedb-2153.json"), bytes).unwrap()
    }

    /// Every module of the map, at any depth.
    fn all_modules(map: &CodeMap) -> Vec<&Module> {
        fn below<'a>(packages: &'a [Package], out: &mut Vec<&'a Module>) {
            for package in packages {
                out.extend(package.modules.iter());
                below(&package.packages, out);
            }
        }
        let mut out: Vec<&Module> = Vec::new();
        below(&map.packages, &mut out);
        out
    }

    /// The path of every file the map shows, each in its node.
    fn shown(map: &CodeMap) -> Vec<String> {
        let mut out: Vec<String> = all_modules(map).iter().flat_map(|m| m.paths()).collect();
        out.extend(
            map.other
                .iter()
                .flat_map(|o| o.files.iter().map(|f| f.path.clone())),
        );
        out
    }

    fn package<'a>(packages: &'a [Package], name: &str) -> Option<&'a Package> {
        packages.iter().find(|p| p.name == name)
    }

    fn module<'a>(modules: &'a [Module], name: &str) -> Option<&'a Module> {
        modules.iter().find(|m| m.name == name)
    }

    fn changed_paths(page: &LoadedPage) -> Vec<String> {
        page.files.iter().map(|f| f.path.clone()).collect()
    }

    #[test]
    fn python_packages_nest_and_a_module_sits_in_the_longest_one() {
        let map = code_map(&py());
        let changescape = package(&map.packages, "changescape").expect("top package");
        assert_eq!(changescape.element.as_deref(), Some("pkg:changescape"));
        let page = package(&changescape.packages, "page").expect("child package");
        assert_eq!(page.element.as_deref(), Some("pkg:changescape.page"));
        assert_eq!(page.status, Status::Added);
        let tour = module(&page.modules, "tour").expect("module in the child");
        assert_eq!(tour.element, "mod:changescape.page.tour");
        assert!(module(&changescape.modules, "tour").is_none());
    }

    #[test]
    fn a_removed_module_is_found_by_its_old_path() {
        let map = code_map(&py());
        let narrate = all_modules(&map)
            .into_iter()
            .find(|m| m.element == "mod:changescape.narrate")
            .expect("removed module");
        assert_eq!(narrate.status, Status::Removed);
        assert_eq!(narrate.paths(), ["skill/src/changescape/narrate.py"]);
    }

    #[test]
    fn files_in_no_module_are_in_groups_by_tour_package() {
        let map = code_map(&py());
        let fixtures = map
            .other
            .iter()
            .find(|o| o.name == "contract-fixtures")
            .expect("contract fixtures group");
        assert!(fixtures
            .files
            .iter()
            .all(|f| f.path.starts_with("contract-fixtures/")));
        assert_eq!(fixtures.files.len(), 74);
    }

    #[test]
    fn the_files_split_as_the_page_has_them() {
        let (py, rs) = (code_map(&py()), code_map(&rs()));
        let in_modules = |map: &CodeMap| all_modules(map).iter().map(|m| m.files.len()).sum();
        let in_other = |map: &CodeMap| map.other.iter().map(|o| o.files.len()).sum::<usize>();
        assert_eq!((in_modules(&py), in_other(&py)), (107, 100));
        assert_eq!((in_modules(&rs), in_other(&rs)), (9, 0));
    }

    #[test]
    fn every_changed_file_is_shown_once() {
        for page in [py(), rs()] {
            let map = code_map(&page);
            let mut shown = shown(&map);
            shown.sort();
            let mut changed = changed_paths(&page);
            changed.sort();
            assert_eq!(shown, changed);
        }
    }

    #[test]
    fn the_totals_are_the_pages() {
        for page in [py(), rs()] {
            let adds: i64 = page.files.iter().map(|f| f.adds).sum();
            let dels: i64 = page.files.iter().map(|f| f.dels).sum();
            assert_eq!(code_map(&page).totals(), (adds, dels));
        }
    }

    #[test]
    fn rust_modules_split_on_colons() {
        let map = code_map(&rs());
        let slatedb = package(&map.packages, "slatedb").expect("top package");
        let db = package(&slatedb.packages, "db").expect("child package");
        assert_eq!(db.element.as_deref(), Some("pkg:slatedb::db"));
        let builder = module(&db.modules, "builder").expect("module in the child");
        assert_eq!(builder.element, "mod:slatedb::db::builder");
        assert!(package(&map.packages, "slatedb_txn_obj").is_some());
        assert!(package(&slatedb.packages, "slatedb_txn_obj").is_none());
    }

    #[test]
    fn the_top_nodes_are_largest_first() {
        let map = code_map(&py());
        let lines = |p: &Package| p.adds + p.dels;
        assert!(map
            .packages
            .windows(2)
            .all(|w| lines(&w[0]) >= lines(&w[1])));
        let other: Vec<i64> = map.other.iter().map(|o| o.adds + o.dels).collect();
        assert!(other.windows(2).all(|w| w[0] >= w[1]));
    }

    /// The tree as indented text, one node a line: packages and modules
    /// with their lines and status, then the other groups.
    fn outline(map: &CodeMap) -> String {
        fn package(p: &Package, depth: usize, out: &mut String) {
            let pad = "  ".repeat(depth);
            out.push_str(&format!(
                "{pad}{}  +{} −{}  [{}]\n",
                p.name, p.adds, p.dels, p.status
            ));
            for child in &p.packages {
                package(child, depth + 1, out);
            }
            for m in &p.modules {
                module(m, depth + 1, out);
            }
        }
        fn module(m: &Module, depth: usize, out: &mut String) {
            let pad = "  ".repeat(depth);
            out.push_str(&format!(
                "{pad}· {}  +{} −{}  [{}]\n",
                m.name, m.adds, m.dels, m.status
            ));
        }
        let mut out = String::new();
        for p in &map.packages {
            package(p, 0, &mut out);
        }
        for o in &map.other {
            out.push_str(&format!(
                "other: {} ({} files) +{} −{}\n",
                o.name,
                o.files.len(),
                o.adds,
                o.dels
            ));
        }
        out
    }

    #[test]
    fn modules_in_no_package_are_grouped_by_their_tour_package() {
        let map = code_map(&py());
        let tests = map
            .packages
            .iter()
            .find(|p| p.element.is_none() && p.name == "tests")
            .expect("group of the tests package");
        assert!(tests.modules.iter().any(|m| m.name == "test_tour_check"));
        assert!(tests.packages.is_empty());
        let real = map
            .packages
            .iter()
            .take_while(|p| p.element.is_some())
            .count();
        assert!(real < map.packages.len());
        assert!(map.packages[real..].iter().all(|p| p.element.is_none()));
        let lines = |p: &Package| p.adds + p.dels;
        assert!(map.packages[real..]
            .windows(2)
            .all(|w| lines(&w[0]) >= lines(&w[1])));
    }

    #[test]
    fn print_outlines() {
        for (name, page) in [("py-pr22", py()), ("rs-slatedb-2153", rs())] {
            println!("== {name}\n{}", outline(&code_map(&page)));
        }
    }

    #[test]
    fn a_module_named_like_its_package_shows_its_file_stem() {
        let map = code_map(&py());
        let changescape = package(&map.packages, "changescape").expect("top package");
        let page = package(&changescape.packages, "page").expect("child package");
        let init = module(&page.modules, "__init__").expect("the package's own file");
        assert_eq!(init.element, "mod:changescape.page");
        assert!(module(&page.modules, "changescape.page").is_none());
        assert!(module(&page.modules, "page").is_none());

        let map = code_map(&rs());
        let txn = package(&map.packages, "slatedb_txn_obj").expect("top package");
        let lib = module(&txn.modules, "lib").expect("the crate root");
        assert_eq!(lib.element, "mod:slatedb_txn_obj");
    }
}
