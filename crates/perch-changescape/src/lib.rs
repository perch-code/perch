//! Changescape maps: the files Changescape writes for a pull request, found
//! and read for Perch's Map. Perch computes no map fact. It reads what
//! Changescape left in its cache and draws that.
//!
//! One build of one pull request at one head commit is one folder:
//!
//! ```text
//! <root>/<owner>/<repo>/pr-<number>/<head_sha>/
//!   status.json   state of the build
//!   page.json     facts and captions, the only file Perch draws
//!   page.html     the same page for a browser
//! ```
//!
//! This crate knows nothing about the UI: [`lookup`] finds a pull
//! request's build, [`load`] reads its page once, and [`LineIndex`] says
//! which elements hold a line.
//!
//! The page and status types are generated from the page format's schemas
//! in `schema/`, by `schema/generate.sh`, into `src/generated/`.

pub mod fold;
pub mod page;
mod skill;

/// Code generated from the schemas. Not edited by hand.
mod generated {
    pub mod status;
    pub mod v3;
}

pub use generated::status::{BuildStage as Stage, BuildState as State, BuildStatus as Status};
pub use generated::v3;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub use page::LoadedPage;
pub use skill::skill_available;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not parse {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    /// The page is of a format version this Perch does not draw, though
    /// its status said otherwise.
    #[error("{path} is page format {schema}, which this Perch does not draw")]
    Version { path: PathBuf, schema: u32 },
}

/// The lowest page format version this Perch draws.
pub const MIN_PAGE_SCHEMA: u32 = 3;
/// The highest page format version this Perch draws.
pub const MAX_PAGE_SCHEMA: u32 = 3;
/// The version of a page that does not say which it is.
pub const UNSTATED_PAGE_SCHEMA: u32 = 2;

/// Whether this Perch draws a page of a format version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    Drawn,
    /// An older Changescape wrote it: a rebuild draws.
    Outdated,
    /// It needs a newer Perch.
    TooNew,
}

impl Version {
    pub fn of(schema: u32) -> Version {
        if schema < MIN_PAGE_SCHEMA {
            Version::Outdated
        } else if schema > MAX_PAGE_SCHEMA {
            Version::TooNew
        } else {
            Version::Drawn
        }
    }
}

/// One build's folder and what its status says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub dir: PathBuf,
    pub head_sha: String,
    pub status: Status,
}

impl Build {
    pub fn page_path(&self) -> PathBuf {
        self.dir.join("page.json")
    }

    pub fn html_path(&self) -> PathBuf {
        self.dir.join("page.html")
    }

    /// A finished build with a page, of any version.
    pub fn finished(&self) -> bool {
        self.status.state == State::Done && self.page_path().is_file()
    }

    /// Whether this Perch draws the version its status names.
    pub fn version(&self) -> Version {
        Version::of(self.page_schema())
    }

    /// The format version of the build's page, as its status says.
    pub fn page_schema(&self) -> u32 {
        self.status
            .page_schema
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(UNSTATED_PAGE_SCHEMA)
    }

    /// A finished build with a page of a version this Perch draws: one
    /// the Map can draw.
    pub fn drawable(&self) -> bool {
        self.finished() && self.version() == Version::Drawn
    }

    /// What a finished build is, by its version: drawn at the head or at
    /// an earlier one, or refused.
    fn found(self, current: bool) -> Lookup {
        match self.version() {
            Version::Drawn if current => Lookup::Current(self),
            Version::Drawn => Lookup::Stale(self),
            Version::Outdated => Lookup::Outdated(self),
            Version::TooNew => Lookup::TooNew(self),
        }
    }

    /// The first seven characters of the head it maps.
    pub fn short_sha(&self) -> &str {
        self.head_sha.get(..7).unwrap_or(&self.head_sha)
    }
}

/// What the cache holds for a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// A drawable build of the pull request's current head.
    Current(Build),
    /// No build of the current head, but a drawable one of an earlier head.
    Stale(Build),
    /// A build of the current head is running. The second build is the
    /// last drawable one of an earlier head, if there is one.
    Running(Build, Option<Build>),
    /// The build of the current head failed, and no earlier one is
    /// drawable.
    Failed(Build),
    /// A finished build whose page an older Changescape wrote, and no
    /// drawable one. A rebuild draws.
    Outdated(Build),
    /// A finished build whose page needs a newer Perch, and no drawable
    /// one. The browser page draws it.
    TooNew(Build),
    None,
}

impl Lookup {
    /// The build the Map draws, if any.
    pub fn drawable(&self) -> Option<&Build> {
        match self {
            Lookup::Current(build) | Lookup::Stale(build) => Some(build),
            Lookup::Running(_, previous) => previous.as_ref(),
            Lookup::Failed(_) | Lookup::Outdated(_) | Lookup::TooNew(_) | Lookup::None => None,
        }
    }

    /// The build whose page this Perch refuses, for its browser page.
    pub fn refused(&self) -> Option<&Build> {
        match self {
            Lookup::Outdated(build) | Lookup::TooNew(build) => Some(build),
            _ => None,
        }
    }

    /// This lookup once the page of its drawable build turns out to be of
    /// `schema`, a version the status did not say or said wrongly. A
    /// running build loses the earlier page it would show meanwhile.
    pub fn with_page_schema(self, schema: u32) -> Lookup {
        let version = Version::of(schema);
        if version == Version::Drawn {
            return self;
        }
        let refuse = |build| match version {
            Version::TooNew => Lookup::TooNew(build),
            _ => Lookup::Outdated(build),
        };
        match self {
            Lookup::Current(build) | Lookup::Stale(build) => refuse(build),
            Lookup::Running(build, Some(_)) => Lookup::Running(build, None),
            other => other,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Lookup::Running(..))
    }
}

/// The folder that holds every build of one pull request.
fn pull_dir(root: &Path, repo: &str, number: u64) -> PathBuf {
    root.join(repo).join(format!("pr-{number}"))
}

/// The folder of one build.
pub fn build_dir(root: &Path, repo: &str, number: u64, head_sha: &str) -> PathBuf {
    pull_dir(root, repo, number).join(head_sha)
}

/// Find the map of `repo` (`owner/name`) pull request `number` at
/// `head_sha`. File reads only; call it off the UI thread.
///
/// Only a page of a version this Perch draws counts: a finished build of
/// another version is `Outdated` or `TooNew`, and never an earlier head's
/// stand-in.
pub fn lookup(root: &Path, repo: &str, number: u64, head_sha: &str) -> Lookup {
    let pull = pull_dir(root, repo, number);
    let current = read_build(&pull.join(head_sha));
    let earlier = || newest_earlier(&pull, head_sha);
    match current {
        Some(build) if build.status.state == State::Running => {
            Lookup::Running(build, earlier().filter(Build::drawable))
        }
        Some(build) if build.drawable() => Lookup::Current(build),
        Some(build) => match earlier() {
            Some(earlier) if earlier.drawable() => Lookup::Stale(earlier),
            _ if build.finished() => build.found(true),
            None if build.status.state == State::Failed || !build.page_path().is_file() => {
                Lookup::Failed(build)
            }
            Some(earlier) => earlier.found(false),
            None => Lookup::None,
        },
        None => earlier().map_or(Lookup::None, |build| build.found(false)),
    }
}

/// A status that says only `state`, and `error` when there is one.
pub fn bare_status(state: State, error: Option<String>) -> Status {
    Status {
        error,
        finished: None,
        page_schema: None,
        schema: 2,
        skill_version: String::new(),
        stage: Stage::Facts,
        started: None,
        state,
    }
}

/// The build in `dir`, if it is one: a folder with a status or a page. A
/// page with no status is a finished build of a page that does not say
/// its version, so one an older Changescape wrote.
fn read_build(dir: &Path) -> Option<Build> {
    let head_sha = dir.file_name()?.to_str()?.to_string();
    let status = match std::fs::read(dir.join("status.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            bare_status(
                State::Failed,
                Some(format!("status.json could not be read: {e}")),
            )
        }),
        Err(_) if dir.join("page.json").is_file() => bare_status(State::Done, None),
        Err(_) => return None,
    };
    Some(Build {
        dir: dir.to_path_buf(),
        head_sha,
        status,
    })
}

/// The finished build of another head of the same pull request that
/// finished last: the newest drawable one, else the newest of any
/// version. A build with no finish time counts by its page's
/// modification time.
fn newest_earlier(pull: &Path, head_sha: &str) -> Option<Build> {
    let entries = std::fs::read_dir(pull).ok()?;
    let finished: Vec<Build> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name() != head_sha)
        .filter_map(|entry| read_build(&entry.path()))
        .filter(Build::finished)
        .collect();
    let newest = |drawable: bool| {
        finished
            .iter()
            .filter(|build| !drawable || build.drawable())
            .max_by_key(|build| finished_at(build))
            .cloned()
    };
    newest(true).or_else(|| newest(false))
}

fn finished_at(build: &Build) -> Option<jiff::Timestamp> {
    build
        .status
        .finished
        .as_deref()
        .and_then(|text| text.parse::<jiff::Timestamp>().ok())
        .or_else(|| {
            let modified = std::fs::metadata(build.page_path()).ok()?.modified().ok()?;
            jiff::Timestamp::try_from(modified).ok()
        })
}

/// Read a build's page. A page is parsed once per build and finish time,
/// and kept for the session.
pub fn load(build: &Build) -> Result<Arc<LoadedPage>, Error> {
    type Cache = Mutex<HashMap<PathBuf, (Option<String>, Arc<LoadedPage>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let finished = build.status.finished.clone();
    if let Some((at, page)) = cache.lock().unwrap().get(&build.dir) {
        if *at == finished {
            return Ok(page.clone());
        }
    }
    let path = build.page_path();
    let bytes = std::fs::read(&path).map_err(|source| Error::Read {
        path: path.clone(),
        source,
    })?;
    let page = Arc::new(LoadedPage::read(&bytes).map_err(|e| match e {
        page::ReadError::Json(source) => Error::Parse {
            path: path.clone(),
            source,
        },
        page::ReadError::Version(schema) => Error::Version {
            path: path.clone(),
            schema,
        },
    })?);
    cache
        .lock()
        .unwrap()
        .insert(build.dir.clone(), (finished, page.clone()));
    Ok(page)
}

/// Which side of the diff a line is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffSide {
    Base,
    Head,
}

/// Per side and path, `(start, end, element id)` sorted by start.
type Ranges = HashMap<(DiffSide, String), Vec<(u32, u32, String)>>;

/// The elements on each line of each file, for the Code strip and for
/// counting review threads. Built once per page.
#[derive(Debug, Clone, Default)]
pub struct LineIndex {
    ranges: Ranges,
}

impl LineIndex {
    pub fn new(page: &v3::Page) -> LineIndex {
        let mut ranges = Ranges::new();
        for (id, element) in &page.elements {
            for r in element.refs.iter().filter(|r| r.file != Some(true)) {
                for (side, range) in [(DiffSide::Base, &r.base), (DiffSide::Head, &r.head)] {
                    if let Some((start, end)) = page::lines(range) {
                        ranges.entry((side, r.f.clone())).or_default().push((
                            start,
                            end,
                            id.clone(),
                        ));
                    }
                }
            }
        }
        for list in ranges.values_mut() {
            list.sort();
        }
        LineIndex { ranges }
    }

    /// Every element whose range on `side` of `path` holds `line`
    /// (1-based), the narrowest first.
    pub fn at(&self, side: DiffSide, path: &str, line: u32) -> Vec<&str> {
        let Some(list) = self.ranges.get(&(side, path.to_string())) else {
            return Vec::new();
        };
        let end = list.partition_point(|(start, _, _)| *start <= line);
        let mut found: Vec<&(u32, u32, String)> = list[..end]
            .iter()
            .filter(|(_, last, _)| line <= *last)
            .collect();
        found.sort_by_key(|(start, last, _)| last - start);
        found.into_iter().map(|(_, _, id)| id.as_str()).collect()
    }

    /// The narrowest element on that line, for the Code strip.
    pub fn innermost(&self, side: DiffSide, path: &str, line: u32) -> Option<&str> {
        self.at(side, path, line).into_iter().next()
    }
}
