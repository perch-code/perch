//! `perch --changescape <owner/repo#N> [--head <sha>]`
//!
//! Prints what Perch finds of a pull request's Changescape: the lookup's
//! result, and the tour's chapters with their stops, verdicts and
//! pictures. The
//! head comes from the cached pull request, or from `--head` for one Perch
//! has not opened. Nothing is fetched and nothing is written.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use perch_changescape::v4::PicturesItem;
use perch_changescape::{page, LoadedPage, Lookup};
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
    // As the tab finds it, so a page whose status misstates its version
    // shows as the tab shows it.
    let found = crate::backend::changescape::find(&root, repo, number, &head);
    let _ = writeln!(out, "lookup: {}", describe(&found.lookup));
    if let Some(error) = found.error {
        return Err(error);
    }
    if let Some(page) = found.page {
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
        let stage = build
            .status
            .stage
            .map_or_else(|| "none".to_string(), |stage| stage.to_string());
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

fn render(out: &mut String, page: &LoadedPage) {
    let _ = writeln!(out, "title: {}", page.meta.title);
    let _ = writeln!(out, "page format: {}", page.schema);
    let _ = writeln!(out, "elements: {}", page.elements.len());
    let Some(tour) = page.tour() else {
        let _ = writeln!(out, "tour: none");
        return;
    };
    let _ = writeln!(
        out,
        "tour: {} chapters, {} stops, narrated: {}",
        tour.chapters.len(),
        page::stops(tour).count(),
        tour.narrated
    );
    for chapter in &tour.chapters {
        let spine = if chapter.spine.is_some() {
            ", spine"
        } else {
            ""
        };
        let _ = writeln!(out, "  chapter {}: {}{spine}", chapter.id, chapter.title);
        for stop in &chapter.stops {
            let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
            for picture in &stop.pictures {
                let kind = match picture {
                    PicturesItem::Hunk { .. } => "hunk",
                    PicturesItem::Seq { .. } => "seq",
                };
                *kinds.entry(kind).or_default() += 1;
            }
            let kinds: Vec<String> = kinds.iter().map(|(k, n)| format!("{k} {n}")).collect();
            let _ = writeln!(
                out,
                "    {} [{}, {} lines, {}]: {}",
                stop.id,
                stop.verdict,
                stop.lines,
                kinds.join(", "),
                stop.title
            );
        }
    }
    for band in &tour.below {
        let _ = writeln!(
            out,
            "  below {} ({}): {} lines",
            band.part, band.verdict, band.lines
        );
    }
}
