//! `perch --changescape <owner/repo#N> [--head <sha>]`
//!
//! Prints what Perch finds of a pull request's Changescape: the lookup's
//! result, each section with its view count, and the blocks by type. The
//! head comes from the cached pull request, or from `--head` for one Perch
//! has not opened. Nothing is fetched and nothing is written.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use perch_changescape::page::BlockBody;
use perch_changescape::{Lookup, Page};
use perch_github::{Cache, ItemKind};

pub fn run(args: &[String]) {
    let usage = || {
        eprintln!("usage: perch --changescape <owner/repo#N> [--head <sha>]");
        std::process::exit(2);
    };
    let (spec, head) = match args {
        [spec] => (spec, None),
        [spec, flag, sha] if flag == "--head" => (spec, Some(sha.clone())),
        _ => usage(),
    };
    let Some((repo, number)) = crate::reference::launch_hook(spec) else {
        usage();
        return;
    };
    match print(&repo, number, head) {
        Ok(out) => print!("{out}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn print(repo: &str, number: u64, head: Option<String>) -> Result<String, String> {
    let config = perch_github::Config::load();
    let root = config.changescape.cache_root();
    let head = match head {
        Some(head) => head,
        None => cached_head(repo, number)?,
    };
    let mut out = String::new();
    let _ = writeln!(out, "cache root: {}", root.display());
    if !config.changescape.enabled {
        let _ = writeln!(
            out,
            "note: changescape.enabled is false; Perch shows nothing"
        );
    }
    let lookup = perch_changescape::lookup(&root, repo, number, &head);
    let _ = writeln!(out, "lookup: {}", describe(&lookup));
    if let Some(build) = lookup.drawable() {
        let page = perch_changescape::load(build).map_err(|e| e.to_string())?;
        render(&mut out, &page);
    }
    Ok(out)
}

/// The head of the pull request as Perch last saw it.
fn cached_head(repo: &str, number: u64) -> Result<String, String> {
    let path = perch_github::config::db_path().ok_or("no data dir")?;
    if !path.exists() {
        return Err(format!("no cache at {}; pass --head <sha>", path.display()));
    }
    let (cache, _) = Cache::open_with_outcome(&path, None).map_err(|e| e.to_string())?;
    let item = cache
        .find_item_by_subject(repo, ItemKind::PullRequest, number)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{repo}#{number} is not in the cache; pass --head <sha>"))?;
    item.pull_request()
        .map(|pull| pull.head_sha.clone())
        .ok_or_else(|| format!("{repo}#{number} is not a pull request"))
}

fn describe(lookup: &Lookup) -> String {
    let at = |build: &perch_changescape::Build| {
        let stage = build.status.stage.as_deref().unwrap_or("-");
        format!(
            "{} (stage {stage}) in {}",
            build.short_sha(),
            build.dir.display()
        )
    };
    match lookup {
        Lookup::Current(build) => format!("current {}", at(build)),
        Lookup::Stale(build) => format!("stale {}", at(build)),
        Lookup::Running(build, None) => format!("running {}", at(build)),
        Lookup::Running(build, Some(previous)) => {
            format!("running {}; drawing {}", at(build), at(previous))
        }
        Lookup::Failed(build) => format!(
            "failed {}: {}",
            at(build),
            build.status.error.as_deref().unwrap_or("no page.json")
        ),
        Lookup::None => "none".to_string(),
    }
}

fn render(out: &mut String, page: &Page) {
    let _ = writeln!(out, "title: {}", page.meta.title);
    let _ = writeln!(out, "sections:");
    for section in &page.sections {
        let views = page.section_views(section);
        if views.is_empty() {
            let _ = writeln!(
                out,
                "  {} ({}): no views. {}",
                section.id, section.t, section.reason
            );
        } else {
            let _ = writeln!(
                out,
                "  {} ({}): {} views",
                section.id,
                section.t,
                views.len()
            );
        }
    }
    let mut kinds: BTreeMap<String, (usize, bool)> = BTreeMap::new();
    for block in page.views.iter().flat_map(|view| &view.blocks) {
        let entry = kinds.entry(block.kind.clone()).or_default();
        entry.0 += 1;
        entry.1 |= matches!(block.body, BlockBody::Other);
    }
    let _ = writeln!(out, "blocks:");
    for (kind, (count, fallback)) in kinds {
        let note = if fallback { " (browser only)" } else { "" };
        let _ = writeln!(out, "  {kind}: {count}{note}");
    }
    let _ = writeln!(out, "elements: {}", page.elements.len());
}
