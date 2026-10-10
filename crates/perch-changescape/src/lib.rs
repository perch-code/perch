//! Changescape maps: the files Changescape writes for a pull request, found
//! and read for the Changescape tab. Perch computes no fact of its own. It
//! reads what Changescape left in its cache and draws that.
//!
//! One build of one pull request at one head commit is one folder:
//!
//! ```text
//! <root>/<owner>/<repo>/pr-<number>/<head_sha>/
//!   status.json   state of the build
//!   page.json     facts and captions, the only file Perch draws
//! ```
//!
//! This crate knows nothing about the UI: [`lookup`] finds a pull
//! request's build, [`load`] reads its page once, and [`StopIndex`] says
//! which tour stop shows a line.
//!
//! The page and status types are generated from the page format's schemas
//! in `schema/`, by `schema/generate.sh`, into `src/generated/`.

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
pub use skill::{skill_available, skill_writes_tour};

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

    /// A finished build with a page.
    pub fn finished(&self) -> bool {
        self.status.state == State::Done && self.page_path().is_file()
    }

    /// The first seven characters of the head it maps.
    pub fn short_sha(&self) -> &str {
        self.head_sha.get(..7).unwrap_or(&self.head_sha)
    }
}

/// What the cache holds for a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// A finished build of the pull request's current head.
    Current(Build),
    /// No finished build of the current head, but one of an earlier head.
    Stale(Build),
    /// A build of the current head is running. The second build is the
    /// last finished one of an earlier head, if there is one.
    Running(Build, Option<Build>),
    /// The build of the current head failed, and there is no earlier one.
    Failed(Build),
    None,
}

impl Lookup {
    /// The build the tab draws, if any.
    pub fn drawable(&self) -> Option<&Build> {
        match self {
            Lookup::Current(build) | Lookup::Stale(build) => Some(build),
            Lookup::Running(_, previous) => previous.as_ref(),
            Lookup::Failed(_) | Lookup::None => None,
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

/// `lookup`, while a run that this reader started still builds the
/// current head. A step of the skill can mark the build failed in the
/// middle of the run, when it rejects the narration, and the run goes on
/// to fix it and write the page again. So a failed status is not the end
/// yet: the page an earlier step wrote still draws, as a finished build,
/// and a build with no page to draw is running.
pub fn lookup_during_run(root: &Path, repo: &str, number: u64, head_sha: &str) -> Lookup {
    let pull = pull_dir(root, repo, number);
    match read_build(&pull.join(head_sha)) {
        Some(mut build) if build.status.state == State::Failed => {
            // A status the skill is still writing does not read yet.
            let half_written = build
                .status
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with(UNREADABLE_STATUS));
            if build.page_path().is_file() && !half_written {
                build.status.state = State::Done;
                build.status.error = None;
                Lookup::Current(build)
            } else {
                Lookup::Running(build, newest_earlier(&pull, head_sha))
            }
        }
        _ => lookup(root, repo, number, head_sha),
    }
}

/// Find the map of `repo` (`owner/name`) pull request `number` at
/// `head_sha`. File reads only; call it off the UI thread.
pub fn lookup(root: &Path, repo: &str, number: u64, head_sha: &str) -> Lookup {
    let pull = pull_dir(root, repo, number);
    let earlier = || newest_earlier(&pull, head_sha);
    match read_build(&pull.join(head_sha)) {
        Some(build) if build.status.state == State::Running => Lookup::Running(build, earlier()),
        Some(build) if build.finished() => Lookup::Current(build),
        // Failed, or done with no page.
        Some(build) => earlier().map_or(Lookup::Failed(build), Lookup::Stale),
        None => earlier().map_or(Lookup::None, Lookup::Stale),
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

/// The start of the error of a status that does not read.
const UNREADABLE_STATUS: &str = "status.json could not be read";

/// The build in `dir`, if it is one: a folder with a status or a page. A
/// page with no status is a finished build.
fn read_build(dir: &Path) -> Option<Build> {
    let head_sha = dir.file_name()?.to_str()?.to_string();
    let status = match std::fs::read(dir.join("status.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            bare_status(State::Failed, Some(format!("{UNREADABLE_STATUS}: {e}")))
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
/// finished last. A build with no finish time counts by its page's
/// modification time.
fn newest_earlier(pull: &Path, head_sha: &str) -> Option<Build> {
    std::fs::read_dir(pull)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name() != head_sha)
        .filter_map(|entry| read_build(&entry.path()))
        .filter(Build::finished)
        .max_by_key(finished_at)
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
    let page = Arc::new(LoadedPage::read(&path, &bytes)?);
    cache
        .lock()
        .unwrap()
        .insert(build.dir.clone(), (finished, page.clone()));
    Ok(page)
}

/// Per path, `(first, last, stop id)` of each head window, sorted by
/// first line.
type Windows = HashMap<String, Vec<(u32, u32, String)>>;

/// Which tour stop shows each head line of each file, for the Code strip.
/// Built once per page from the stops' hunk windows.
#[derive(Debug, Clone, Default)]
pub struct StopIndex {
    windows: Windows,
}

impl StopIndex {
    pub fn new(page: &LoadedPage) -> StopIndex {
        let mut windows = Windows::new();
        let stops = page
            .tour()
            .into_iter()
            .flat_map(|tour| &tour.chapters)
            .flat_map(|chapter| &chapter.stops);
        for stop in stops {
            for (file, shown) in stop.pictures.iter().filter_map(page::hunk) {
                for (first, last) in shown.iter().filter_map(|w| w.head) {
                    windows.entry(file.to_string()).or_default().push((
                        first,
                        last,
                        stop.id.clone(),
                    ));
                }
            }
        }
        for list in windows.values_mut() {
            list.sort();
        }
        StopIndex { windows }
    }

    /// The stop whose narrowest head window on `path` holds `line`
    /// (1-based).
    pub fn at(&self, path: &str, line: u32) -> Option<&str> {
        let list = self.windows.get(path)?;
        let end = list.partition_point(|(first, _, _)| *first <= line);
        list[..end]
            .iter()
            .filter(|(_, last, _)| line <= *last)
            .min_by_key(|(first, last, _)| last - first)
            .map(|(_, _, id)| id.as_str())
    }
}
