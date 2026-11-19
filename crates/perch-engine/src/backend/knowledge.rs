//! Perch's live, rebuildable Markdown view of the cache.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use perch_github::{
    Cache, CacheError, Config, Item, ItemKind, ItemSubject, SubjectState, HISTORY_SUBJECT_ID_PREFIX,
};
use perch_knowledge::sanitize::{clean, one_line};
use perch_knowledge::{
    child_dirs, encode_component, ensure_dir, prune_temporary_files, remove_dir, remove_empty_dir,
    safe_component, sync, write_file,
};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use super::Ctx;
use crate::context_cli::{gather_archive, gather_history, render, Mode};

const ITEM_KINDS: [&str; 5] = [
    "pulls",
    "issues",
    "worktrees",
    "github/pulls",
    "github/issues",
];
const HISTORY_EXPORT_BATCH: usize = 32;
const HISTORY_INDEX_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExportBatch {
    Startup,
    Interactive,
    History,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Place {
    repo: PathBuf,
    item: PathBuf,
}

impl Place {
    fn path(&self) -> PathBuf {
        self.repo.join(&self.item)
    }
}

fn repo_place(root: &Path, name: &str) -> Option<PathBuf> {
    let (owner, repo) = name.split_once('/')?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(
        root.join("knowledge/perch/repos")
            .join(encode_component(&owner.to_lowercase()))
            .join(encode_component(&repo.to_lowercase())),
    )
}

fn place(root: &Path, item: &Item) -> Option<Place> {
    let repo = repo_place(root, &item.repo_full_name)?;
    let item = if let Some(pr) = item.pull_request() {
        Path::new("pulls").join(pr.number.to_string())
    } else if let Some(issue) = item.issue() {
        Path::new("issues").join(issue.number.to_string())
    } else {
        let branch = safe_component(item.branch()?).to_ascii_lowercase();
        let hash = Sha256::digest(item.id.as_bytes());
        Path::new("worktrees").join(format!(
            "{branch}--{:02x}{:02x}{:02x}{:02x}",
            hash[0], hash[1], hash[2], hash[3]
        ))
    };
    Some(Place { repo, item })
}

fn history_place(root: &Path, repo: &str, kind: ItemKind, number: u64) -> Option<Place> {
    let repo = repo_place(root, repo)?;
    let kind = match kind {
        ItemKind::PullRequest => "pulls",
        ItemKind::Issue => "issues",
    };
    let item = Path::new("github").join(kind).join(number.to_string());
    Some(Place { repo, item })
}

fn stored_places(root: &Path, cache: &Cache) -> Result<Vec<Place>, CacheError> {
    Ok(cache
        .load_items()?
        .iter()
        .filter_map(|item| place(root, item))
        .chain(
            cache
                .load_history_locations()?
                .iter()
                .filter_map(|(repo, subject)| {
                    history_place(root, repo, subject.kind, subject.number)
                }),
        )
        .collect())
}

#[derive(Clone)]
pub(super) struct Archive {
    root: PathBuf,
    db: PathBuf,
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    marked: Mutex<BTreeSet<String>>,
    ready: Notify,
    catalog: Mutex<Catalog>,
    #[cfg(test)]
    after_gather: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

#[derive(Default)]
struct Catalog {
    places: HashMap<String, Place>,
    repos: BTreeMap<PathBuf, BTreeMap<PathBuf, Entry>>,
    changed_repos: BTreeSet<PathBuf>,
    startup_complete: bool,
}

#[derive(PartialEq, Eq)]
struct Entry {
    id: String,
    line: String,
}

impl Entry {
    fn new(
        place: &Place,
        id: String,
        title: &str,
        state: Option<SubjectState>,
        archived: bool,
    ) -> Self {
        let state = match state {
            Some(SubjectState::PrDraft) => "draft",
            Some(SubjectState::PrMerged) => "merged",
            Some(SubjectState::PrClosed | SubjectState::IssueClosed) => "closed",
            Some(SubjectState::IssueNotPlanned) => "not planned",
            Some(SubjectState::PrOpen | SubjectState::IssueOpen) => "open",
            None => "worktree",
        };
        let archived = if archived { "archived · " } else { "" };
        let line = format!(
            "- `{}/` · {archived}{state} · {}",
            place.item.display(),
            one_line(&clean(title))
        );
        Self { id, line }
    }
}

impl Catalog {
    fn write_entry(
        &mut self,
        place: Place,
        entry: Entry,
        files: &BTreeMap<String, String>,
    ) -> io::Result<()> {
        sync(&place.path(), files)?;
        let id = entry.id.clone();
        let entries = self.repos.entry(place.repo.clone()).or_default();
        if entries.get(&place.item) != Some(&entry) {
            if let Some(old) = entries.insert(place.item.clone(), entry) {
                if old.id != id {
                    self.places.remove(&old.id);
                }
            }
            self.changed_repos.insert(place.repo.clone());
        }
        if let Some(old) = self.places.insert(id.clone(), place.clone()) {
            if old != place {
                self.remove_at(&old, &id);
            }
        }
        Ok(())
    }

    fn remove_at(&mut self, place: &Place, id: &str) {
        let entries = self.repos.entry(place.repo.clone()).or_default();
        if entries.get(&place.item).is_some_and(|entry| entry.id != id) {
            return;
        }
        let path = place.path();
        if let Err(error) = remove_dir(&path) {
            warn_path(&path, error);
        }
        entries.remove(&place.item);
        self.changed_repos.insert(place.repo.clone());
    }

    fn remove_item(&mut self, id: &str, current: Option<Place>) {
        let previous = self.places.remove(id);
        if let Some(current) = current
            .as_ref()
            .filter(|place| Some(*place) != previous.as_ref())
        {
            self.remove_at(current, id);
        }
        if let Some(previous) = previous {
            self.remove_at(&previous, id);
        }
    }

    fn write_index(&mut self, repo: &Path) -> io::Result<()> {
        let entries = &self.repos[repo];
        if entries.is_empty() {
            // The exporter may not have reached this repository yet on
            // startup. Prune only if no item directory is there.
            prune_repo(repo)?;
            self.repos.remove(repo);
        } else {
            let mut contents = String::from("# Perch items\n\n");
            for entry in entries.values() {
                let _ = writeln!(contents, "{}", entry.line);
            }
            write_file(&repo.join("INDEX.md"), &contents)?;
            prune_repo(repo)?;
        }
        self.changed_repos.remove(repo);
        Ok(())
    }

    fn write_indexes(&mut self, missing_only: bool) {
        for repo in self.changed_repos.clone() {
            if missing_only && repo.join("INDEX.md").is_file() {
                continue;
            }
            if let Err(error) = self.write_index(&repo) {
                warn_path(&repo.join("INDEX.md"), error);
            }
        }
    }
}

// Resolve the configured home once. Archive components below it still use
// the filesystem helpers that refuse symlinks.
fn canonical_home(root: &Path) -> PathBuf {
    root.canonicalize()
        .or_else(|_| std::path::absolute(root))
        .unwrap_or_else(|_| root.to_owned())
}

impl Archive {
    pub(super) fn new(root: PathBuf, db: PathBuf) -> Self {
        Self {
            root: canonical_home(&root),
            db: std::path::absolute(&db).unwrap_or(db),
            shared: Arc::default(),
        }
    }

    pub(super) fn watch(&self, cache: &mut Cache) {
        let shared = self.shared.clone();
        if let Err(error) = cache.watch_item_changes(move |ids| {
            shared.marked.lock().expect("archive marks").extend(ids);
            shared.ready.notify_one();
        }) {
            tracing::warn!(%error, "could not watch the cache for item archive changes");
        }
    }

    pub(super) fn start(
        &self,
        lifecycle: Arc<perch_git::KeyedMutex>,
    ) -> tokio::task::JoinHandle<()> {
        let archive = self.clone();
        tokio::spawn(async move {
            if let Err(error) = archive.run(lifecycle).await {
                tracing::warn!(%error, "could not run the item archive exporter");
            }
        })
    }

    async fn run(&self, lifecycle: Arc<perch_git::KeyedMutex>) -> Result<(), String> {
        // The full pass reads everything committed before this point.
        self.shared.marked.lock().expect("archive marks").clear();
        let db = self.db.clone();
        let (mut cache, ids) = tokio::task::spawn_blocking(move || {
            let cache = Cache::open_read_only(&db)?;
            let ids = cache
                .load_items()?
                .into_iter()
                .map(|item| item.id)
                .collect();
            Ok::<_, CacheError>((cache, ids))
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

        cache = self
            .write_batch(cache, &lifecycle, ids, ExportBatch::Startup)
            .await?;
        let (returned, ids) = tokio::task::spawn_blocking(move || {
            let ids = cache.load_history_ids();
            (cache, ids)
        })
        .await
        .map_err(|e| e.to_string())?;
        cache = returned;
        let mut history: BTreeSet<_> = ids.map_err(|e| e.to_string())?.into_iter().collect();
        let mut rebuilding_history = !history.is_empty();
        let mut index_pending = false;
        let mut index_due = tokio::time::Instant::now() + HISTORY_INDEX_INTERVAL;
        loop {
            if history.is_empty() {
                tokio::select! {
                    _ = self.shared.ready.notified() => {
                        tokio::time::sleep(Duration::from_millis(150)).await;
                    }
                    _ = tokio::time::sleep_until(index_due), if index_pending => {}
                }
            }
            let ids = std::mem::take(&mut *self.shared.marked.lock().expect("archive marks"));
            let (synced, interactive): (BTreeSet<_>, BTreeSet<_>) = ids
                .into_iter()
                .partition(|id| id.starts_with(HISTORY_SUBJECT_ID_PREFIX));
            history.extend(synced);
            if !interactive.is_empty() {
                cache = self
                    .write_batch(cache, &lifecycle, interactive, ExportBatch::Interactive)
                    .await?;
            }
            if !history.is_empty() {
                let batch = (0..HISTORY_EXPORT_BATCH)
                    .map_while(|_| history.pop_first())
                    .collect();
                cache = self
                    .write_batch(cache, &lifecycle, batch, ExportBatch::History)
                    .await?;
                index_pending = true;
            }
            if index_pending
                && (tokio::time::Instant::now() >= index_due
                    || (rebuilding_history && history.is_empty()))
            {
                let shared = self.shared.clone();
                tokio::task::spawn_blocking(move || {
                    shared
                        .catalog
                        .lock()
                        .expect("archive catalog")
                        .write_indexes(false);
                })
                .await
                .map_err(|e| e.to_string())?;
                index_pending = false;
                index_due = tokio::time::Instant::now() + HISTORY_INDEX_INTERVAL;
                if history.is_empty() {
                    rebuilding_history = false;
                }
            }
        }
    }

    async fn write_batch(
        &self,
        mut cache: Cache,
        lifecycle: &perch_git::KeyedMutex,
        ids: BTreeSet<String>,
        kind: ExportBatch,
    ) -> Result<Cache, String> {
        let mut missing = Vec::new();
        for id in ids {
            let guard = lifecycle.lock(format!("item {id}")).await;
            let archive = self.clone();
            let (returned, gone) = tokio::task::spawn_blocking(move || {
                // Keep the lifecycle lock across both reads and disk writes,
                // even if the async worker is canceled during the write.
                let _guard = guard;
                let gone = match archive.write_item(&cache, &id) {
                    Ok(false) => Some(id),
                    Ok(true) => None,
                    Err(error) => {
                        tracing::warn!(item = id, %error, "could not export item");
                        None
                    }
                };
                (cache, gone)
            })
            .await
            .map_err(|e| e.to_string())?;
            cache = returned;
            missing.extend(gone);
        }
        let archive = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut catalog = archive.shared.catalog.lock().expect("archive catalog");
            // Process vanished rows after writes so taking over a pull
            // request replaces its old archive before any cleanup.
            for id in missing {
                catalog.remove_item(&id, None);
            }
            // History can arrive faster than each full repository index
            // can be rewritten. Put a new repository on disk promptly,
            // and let the outer loop batch updates to existing indexes.
            catalog.write_indexes(kind == ExportBatch::History);
            if kind == ExportBatch::Startup {
                catalog.startup_complete = true;
                match cache.read_snapshot(|cache| stored_places(&archive.root, cache)) {
                    Ok(Ok(places)) => archive.sweep(&catalog, places),
                    Ok(Err(error)) | Err(error) => {
                        tracing::warn!(%error, "could not list items to sweep the archive")
                    }
                }
                let legacy = archive.root.join("knowledge/reviews");
                if let Err(error) = remove_dir(&legacy) {
                    warn_path(&legacy, error);
                }
            }
            cache
        })
        .await
        .map_err(|e| e.to_string())
    }

    fn write_item(&self, cache: &Cache, id: &str) -> Result<bool, String> {
        if id.starts_with(HISTORY_SUBJECT_ID_PREFIX) {
            return self.write_history(cache, id);
        }
        let gathered = cache
            .read_snapshot(|cache| {
                cache
                    .load_item(id)?
                    .map(|item| {
                        let gathered = gather_archive(cache, &item)?;
                        Ok::<_, CacheError>((item, gathered))
                    })
                    .transpose()
            })
            .and_then(|result| result)
            .map_err(|error| error.to_string())?;
        let Some((item, gathered)) = gathered else {
            return Ok(false);
        };
        let place = place(&self.root, &item)
            .ok_or_else(|| format!("invalid repository or branch for {}", item.repo_full_name))?;
        let entry = Entry::new(
            &place,
            item.id.clone(),
            item.title(),
            item.subject.as_ref().map(ItemSubject::subject_state),
            item.is_archived(),
        );
        let files = render(&gathered, Mode::Archive);
        #[cfg(test)]
        if let Some(pause) = self.shared.after_gather.lock().unwrap().take() {
            pause();
        }
        self.shared
            .catalog
            .lock()
            .expect("archive catalog")
            .write_entry(place, entry, &files)
            .map_err(|error| error.to_string())?;
        Ok(true)
    }

    fn write_history(&self, cache: &Cache, id: &str) -> Result<bool, String> {
        let Some((subject, discussion)) =
            cache.load_history_subject(id).map_err(|e| e.to_string())?
        else {
            return Ok(false);
        };
        let place = history_place(
            &self.root,
            subject.repo_full_name(),
            subject.kind(),
            subject.number(),
        )
        .ok_or_else(|| format!("invalid repository for {id}"))?;
        let entry = Entry::new(
            &place,
            id.into(),
            subject.title(),
            Some(subject.subject_state()),
            false,
        );
        let files = render(&gather_history(id, subject, &discussion), Mode::Archive);
        self.shared
            .catalog
            .lock()
            .expect("archive catalog")
            .write_entry(place, entry, &files)
            .map_err(|e| e.to_string())?;
        Ok(true)
    }

    fn sweep(&self, catalog: &Catalog, places: Vec<Place>) {
        let keep: HashSet<_> = places
            .into_iter()
            .chain(catalog.places.values().cloned())
            .map(|place| place.path())
            .collect();
        let repos = self.root.join("knowledge/perch/repos");
        for owner in dirs(&repos) {
            for repo in dirs(&owner) {
                if let Err(error) = prune_temporary_files(&repo) {
                    warn_path(&repo, error);
                }
                for kind in ITEM_KINDS {
                    for item in dirs(&repo.join(kind)) {
                        if !keep.contains(&item) {
                            if let Err(error) = remove_dir(&item) {
                                warn_path(&item, error);
                            }
                        }
                    }
                }
                if let Err(error) = prune_repo(&repo) {
                    warn_path(&repo, error);
                }
            }
            if let Err(error) = remove_empty_dir(&owner) {
                warn_path(&owner, error);
            }
        }
        if let Err(error) = remove_empty_dir(&repos) {
            warn_path(&repos, error);
        }
    }

    /// The caller holds the item's lifecycle lock through this call.
    pub(super) async fn delete(&self, item: Item) {
        let archive = self.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || {
            let mut catalog = archive.shared.catalog.lock().expect("archive catalog");
            let current = place(&archive.root, &item);
            // A takeover can have committed before the exporter replaces
            // the old row's contents at the same path.
            if let Some(place) = &current {
                if let Some(entry) = catalog
                    .repos
                    .get(&place.repo)
                    .and_then(|entries| entries.get(&place.item))
                {
                    if entry.id != item.id {
                        match Cache::open_read_only(&archive.db)
                            .and_then(|cache| cache.load_item(&entry.id))
                        {
                            Ok(None) => {
                                let stale_id = entry.id.clone();
                                catalog.remove_item(&stale_id, None);
                            }
                            Ok(Some(_)) => {}
                            Err(error) => {
                                tracing::warn!(%error, "could not check the previous archive owner")
                            }
                        }
                    }
                }
            }
            catalog.remove_item(&item.id, current);
            if catalog.startup_complete {
                catalog.write_indexes(false);
            }
        })
        .await
        {
            tracing::warn!(%error, "could not delete item archive");
        }
    }
}

impl Ctx {
    pub(super) async fn knowledge_dir(&self, item: &Item) -> Option<PathBuf> {
        let place = place(&self.knowledge.root, item)?;
        let archive = self.knowledge.clone();
        let id = item.id.clone();
        tokio::task::spawn_blocking(move || {
            let mut catalog = archive.shared.catalog.lock().expect("archive catalog");
            if !catalog.startup_complete {
                drop(catalog);
                let cache = Cache::open_read_only(&archive.db).ok()?;
                return existing_knowledge_dir(
                    &place.repo,
                    Some(&place),
                    cache
                        .read_snapshot(|cache| stored_places(&archive.root, cache))
                        .ok()?
                        .ok()?,
                );
            }
            let entries = catalog.repos.get(&place.repo)?;
            if !entries.iter().any(|(path, entry)| {
                *path != place.item
                    && entry.id != id
                    && std::fs::symlink_metadata(place.repo.join(path).join("INDEX.md"))
                        .is_ok_and(|metadata| metadata.is_file())
            }) {
                return None;
            }
            if let Err(error) = ensure_dir(&place.repo).and_then(|()| {
                if !place.repo.join("INDEX.md").is_file() {
                    catalog.write_index(&place.repo)?;
                }
                Ok(())
            }) {
                warn_path(&place.repo, error);
                return None;
            }
            Some(place.repo)
        })
        .await
        .ok()
        .flatten()
    }
}

/// Locate the existing archive for a standalone Ask, without starting the
/// engine or creating files. GitHub may have an item Perch has never opened.
pub fn cached_knowledge_dir(
    root: &Path,
    repo: &str,
    kind: ItemKind,
    number: u64,
) -> Option<PathBuf> {
    let root = canonical_home(root);
    let cache = Cache::open_read_only(&root.join("perch.db")).ok()?;
    let (target, places) = cache
        .read_snapshot(|cache| {
            Ok::<_, CacheError>((
                cache.find_item_by_subject(repo, kind, number)?,
                stored_places(&root, cache)?,
            ))
        })
        .ok()?
        .ok()?;
    let own = target.as_ref().and_then(|item| place(&root, item));
    let repo_path = match &own {
        Some(place) => place.repo.clone(),
        None => repo_place(&root, repo)?,
    };
    existing_knowledge_dir(&repo_path, own.as_ref(), places)
}

/// How many files and directories a configured source may hold. The check
/// stops there and the source is not shared, as OpenCode's own scan of a
/// granted directory stops.
pub const SOURCE_ENTRY_LIMIT: usize = 200_000;

/// How long a source's check is trusted. A run checks its sources when it
/// is admitted and again when it starts, and Settings shows the same
/// answer, so one walk of a large source serves all of them.
const SOURCE_CHECK_TTL: Duration = Duration::from_secs(60);

/// Each source's last check, by directory, kept after it goes stale so a
/// change of answer can be told from the same answer again.
type SourceChecks = HashMap<PathBuf, (Instant, Result<(), String>)>;
static SOURCE_CHECKS: Mutex<Option<SourceChecks>> = Mutex::new(None);

/// Why a configured source can't be shared with runs, or `None` when it can.
/// A fresh answer is reused; a new one that differs from the last is logged.
fn source_problem(dir: &Path) -> Option<String> {
    let cached = SOURCE_CHECKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .and_then(|checks| checks.get(dir).cloned());
    if let Some((at, result)) = &cached {
        if at.elapsed() < SOURCE_CHECK_TTL {
            return result.clone().err();
        }
    }
    let result = perch_knowledge::check_source(dir, SOURCE_ENTRY_LIMIT);
    if cached.as_ref().map(|(_, previous)| previous) != Some(&result) {
        match &result {
            Err(why) => {
                tracing::warn!(dir = %dir.display(), %why, "knowledge source is not shared with runs")
            }
            Ok(()) if cached.is_some() => {
                tracing::info!(dir = %dir.display(), "knowledge source is shared with runs again")
            }
            Ok(()) => {}
        }
    }
    SOURCE_CHECKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert(dir.to_owned(), (Instant::now(), result.clone()));
    result.err()
}

/// Why each configured source that can't be shared isn't, by its `dir` as
/// config.toml has it, for the Settings page.
pub fn knowledge_problems(root: &Path, config: &Config) -> BTreeMap<PathBuf, String> {
    let root = canonical_home(root).join("knowledge");
    config
        .knowledge
        .iter()
        .filter(|source| source.validate().is_ok())
        .filter_map(|source| {
            source_problem(&root.join(&source.dir)).map(|why| (source.dir.clone(), why))
        })
        .collect()
}

/// Check every configured source again and give Settings what changed.
pub(super) async fn refresh_knowledge_status(ctx: Ctx) {
    let config = ctx.config();
    let root = ctx.preparer.store().root().to_owned();
    let problems = tokio::task::spawn_blocking(move || knowledge_problems(&root, &config))
        .await
        .unwrap_or_default();
    ctx.settings.set_knowledge_problems(&ctx, problems);
}

/// Locate configured sources on the engine and check whether the backend
/// can add them alongside the existing read grants.
pub fn external_knowledge_dirs(
    root: &Path,
    config: &Config,
    repo: &str,
    backend: perch_agent::Backend,
    cwd: &Path,
    read_dirs: &[PathBuf],
    mode: &perch_agent::Mode,
) -> Vec<PathBuf> {
    if config.knowledge.is_empty() {
        return Vec::new();
    }
    let root = canonical_home(root).join("knowledge");
    let mut added = read_dirs.to_vec();
    for source in &config.knowledge {
        if !source.applies_to(repo) || source.validate().is_err() {
            continue;
        }
        let dir = root.join(&source.dir);
        if dir
            .to_str()
            .is_none_or(|path| path.contains(char::is_control))
            || added.contains(&dir)
            || source_problem(&dir).is_some()
        {
            continue;
        }
        if perch_agent::can_add_read_dir(backend, cwd, &added, &dir, mode) {
            added.push(dir);
        }
    }
    added.split_off(read_dirs.len())
}

fn existing_knowledge_dir(repo: &Path, own: Option<&Place>, places: Vec<Place>) -> Option<PathBuf> {
    child_dirs(repo).ok()?;
    let has_index = |dir: &Path| {
        std::fs::symlink_metadata(dir.join("INDEX.md")).is_ok_and(|meta| meta.is_file())
    };
    if !has_index(repo) {
        return None;
    }
    places
        .into_iter()
        .any(|place| {
            place.repo == repo
                && Some(&place) != own
                && child_dirs(&place.path()).is_ok()
                && has_index(&place.path())
        })
        .then(|| repo.to_owned())
}

fn dirs(path: &Path) -> Vec<PathBuf> {
    match child_dirs(path) {
        Ok(dirs) => dirs,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            warn_path(path, error);
            Vec::new()
        }
    }
}

fn prune_repo(repo: &Path) -> io::Result<()> {
    for kind in ITEM_KINDS {
        remove_empty_dir(&repo.join(kind))?;
    }
    remove_empty_dir(&repo.join("github"))?;
    // The only file at this level is the generated index. Keep it when
    // any item directories (including ones still being rebuilt) remain.
    let index = repo.join("INDEX.md");
    if ITEM_KINDS.iter().all(|kind| {
        matches!(std::fs::symlink_metadata(repo.join(kind)), Err(error) if error.kind() == io::ErrorKind::NotFound)
    }) {
        perch_knowledge::remove_file(&index)?;
        remove_empty_dir(repo)?;
        if let Some(owner) = repo.parent() {
            remove_empty_dir(owner)?;
        }
    }
    Ok(())
}

fn warn_path(path: &Path, error: io::Error) {
    tracing::warn!(path = %path.display(), %error, "could not update item archive");
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::backend::{
        delete_item, take_pull_request,
        tests::{ctx, nowhere, pull_request},
    };
    use jiff::Timestamp;
    use perch_github::{
        Author, CheckState, Checks, CommentMessage, CommentThread, Discussion, ItemCheckout,
        ItemSubject, NewReviewerAttempt, ReviewAdmission, ReviewGroupStatus, ReviewSnapshot,
        ReviewerPhase, ThreadOrigin,
    };
    use std::fs;

    #[test]
    fn external_knowledge_grants_only_matching_configured_safe_directories() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let knowledge = root.join("knowledge");
        for relative in [
            "discord/slatedb",
            "notes",
            "perch",
            "unconfigured",
            "includes-link",
            "includes-instructions/topic",
        ] {
            fs::create_dir_all(knowledge.join(relative)).unwrap();
        }
        fs::write(knowledge.join("notes/INDEX.md"), "# Notes").unwrap();
        fs::write(
            knowledge.join("includes-instructions/topic/AGENTS.md"),
            "text",
        )
        .unwrap();
        let home = root.join("perch-home");
        std::os::unix::fs::symlink(&root, &home).unwrap();
        std::os::unix::fs::symlink(knowledge.join("perch"), knowledge.join("alias")).unwrap();
        std::os::unix::fs::symlink(&root, knowledge.join("includes-link/outside")).unwrap();
        let config = Config::parse_text(
            r#"
[[knowledge]]
dir = "discord/slatedb"
repos = ["slatedb/slatedb", "slatedb/slatedb-go"]
[[knowledge]]
dir = "notes"
[[knowledge]]
dir = "alias"
[[knowledge]]
dir = "includes-link"
[[knowledge]]
dir = "includes-instructions"
[[knowledge]]
dir = "missing"
"#,
        )
        .unwrap();
        let grants = |repo| {
            external_knowledge_dirs(
                &home,
                &config,
                repo,
                perch_agent::Backend::ClaudeCode,
                &root,
                &[],
                &perch_agent::Mode::Ask,
            )
        };
        assert_eq!(
            grants("SlateDB/SlateDB-Go"),
            [knowledge.join("discord/slatedb"), knowledge.join("notes")]
        );
        assert_eq!(grants("perch-code/perch"), [knowledge.join("notes")]);
        assert!(!knowledge.join("missing").exists());

        // Settings is told why each source that is skipped is.
        let problems = knowledge_problems(&home, &config);
        let why = |dir: &str| problems.get(Path::new(dir)).map(String::as_str);
        assert_eq!(why("discord/slatedb"), None);
        assert_eq!(why("notes"), None);
        assert_eq!(why("missing"), Some("the directory does not exist"));
        assert_eq!(why("includes-link"), Some("outside is a symlink"));
        assert_eq!(
            why("includes-instructions"),
            Some("topic/AGENTS.md is an agent instructions file")
        );
        assert!(why("alias").is_some());
    }

    #[test]
    fn a_source_is_walked_once_while_its_check_is_fresh() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().canonicalize().unwrap().join("notes");
        fs::create_dir_all(&source).unwrap();
        assert_eq!(source_problem(&source), None);
        // A link made now is not seen until the check goes stale, so a run's
        // admission and its start walk a large source once between them.
        std::os::unix::fs::symlink(&source, source.join("loop")).unwrap();
        assert_eq!(source_problem(&source), None);
        SOURCE_CHECKS
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .get_mut(&source)
            .unwrap()
            .0 -= SOURCE_CHECK_TTL;
        assert_eq!(source_problem(&source), Some("loop is a symlink".into()));
    }

    fn worktree(repo: &str, branch: &str) -> Item {
        let mut item = Item::new(repo, Timestamp::now());
        item.checkout = Some(ItemCheckout {
            branch: branch.into(),
            base_ref: "main".into(),
            head_sha: "head".into(),
            path: "/checkout".into(),
            owned: false,
            on_disk: true,
            churn: None,
            last_seen_head: None,
            divergence: None,
            source: None,
        });
        item
    }

    #[test]
    fn archive_places_are_stable_and_distinguish_worktrees() {
        let root = Path::new("/perch-home");
        let slash = worktree("Acme/Widgets", "fix/wal");
        let hyphen = worktree("acme/widgets", "fix-wal");
        let upper = worktree("acme/widgets", "Fix");
        let lower = worktree("acme/widgets", "fix");
        let path = |item: &Item| place(root, item).unwrap();
        assert_eq!(path(&slash).repo, path(&hyphen).repo);
        assert_ne!(path(&slash), path(&hyphen));
        assert_ne!(path(&upper), path(&lower));
        let mut changed = slash.clone();
        changed.checkout.as_mut().unwrap().branch = "renamed".into();
        assert_ne!(path(&changed), path(&slash));
        changed.subject = Some(ItemSubject::PullRequest(pull_request(7)));
        assert_eq!(path(&changed).item, Path::new("pulls/7"));
    }

    async fn flush(ctx: &Ctx, startup: bool) {
        let archive = &ctx.knowledge;
        let cache = Cache::open_read_only(&archive.db).unwrap();
        let ids = if startup {
            cache
                .load_items()
                .unwrap()
                .into_iter()
                .map(|item| item.id)
                .collect()
        } else {
            std::mem::take(&mut *archive.shared.marked.lock().unwrap())
        };
        archive
            .write_batch(
                cache,
                &ctx.item_lifecycle,
                ids,
                if startup {
                    ExportBatch::Startup
                } else {
                    ExportBatch::Interactive
                },
            )
            .await
            .unwrap();
    }

    fn tree(path: &Path) -> BTreeMap<PathBuf, String> {
        fn read(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, String>) {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    read(root, &entry.path(), out);
                } else {
                    out.insert(
                        entry.path().strip_prefix(root).unwrap().into(),
                        fs::read_to_string(entry.path()).unwrap(),
                    );
                }
            }
        }
        let mut files = BTreeMap::new();
        read(path, path, &mut files);
        files
    }

    pub(in crate::backend) async fn assert_rebuild(ctx: &Ctx) {
        flush(ctx, false).await;
        let root = ctx.knowledge.root.join("knowledge/perch");
        let before = tree(&root);
        assert!(before.keys().any(|file| file.ends_with("findings.md")));
        fs::remove_dir_all(&root).unwrap();
        *ctx.knowledge.shared.catalog.lock().unwrap() = Catalog::default();
        flush(ctx, true).await;
        assert_eq!(tree(&root), before);
    }

    pub(in crate::backend) fn admit_review(cache: &Cache, item: &Item, id: &str) {
        let now = Timestamp::now();
        cache
            .admit_review_group(&ReviewAdmission {
                group_id: id.into(),
                snapshot: ReviewSnapshot {
                    id: format!("{id}-snapshot"),
                    item_id: item.id.clone(),
                    repository: item.repo_full_name.clone(),
                    branch_label: item.branch().map(str::to_string),
                    requested_base_ref: "main".into(),
                    base_tip_sha: "base".into(),
                    merge_base_sha: "base".into(),
                    head_sha: "head".into(),
                    dirty_edits_excluded: false,
                    retention_key: format!("{id}-retention"),
                    retained_ref: None,
                    manifest_path: None,
                    diff_path: None,
                    created_at: now,
                    worktree_parent_sha: None,
                },
                attempts: vec![NewReviewerAttempt {
                    id: format!("{id}-attempt"),
                    run_id: None,
                    backend: "codex".into(),
                    executable: None,
                    executable_version: None,
                    model: "test".into(),
                    effort: "low".into(),
                    adapter_version: "1".into(),
                    output_format_version: "1".into(),
                    created_at: now,
                }],
                admitted_at: now,
                instructions: None,
            })
            .unwrap();
    }

    pub(in crate::backend) fn finish_review(cache: &Cache, id: &str) {
        let now = Timestamp::now();
        let attempt = format!("{id}-attempt");
        cache
            .transition_review_group(id, ReviewGroupStatus::Reviewing, now)
            .unwrap();
        cache
            .transition_reviewer_attempt(&attempt, ReviewerPhase::Reviewing, now)
            .unwrap();
        cache
            .transition_reviewer_attempt(&attempt, ReviewerPhase::Completed, now)
            .unwrap();
        cache
            .transition_review_group(id, ReviewGroupStatus::Reconciling, now)
            .unwrap();
        cache
            .finish_review_group(id, ReviewGroupStatus::Completed, now)
            .unwrap();
    }

    fn private_thread(item: &Item, id: &str) -> CommentThread {
        let now = Timestamp::now();
        CommentThread {
            id: id.into(),
            worktree_id: item.id.clone(),
            origin: ThreadOrigin::Local,
            github_id: None,
            anchor: None,
            outdated: false,
            resolved: false,
            node_id: None,
            can_resolve: false,
            created_at: now,
            messages: vec![CommentMessage {
                id: format!("{id}-question"),
                thread_id: id.into(),
                origin: ThreadOrigin::Local,
                author: Author::User,
                body: "Private question".into(),
                created_at: now,
                run: None,
                github_id: None,
                node_id: None,
                html_url: None,
                diff_hunk: None,
                reactions: Default::default(),
            }],
        }
    }

    #[tokio::test]
    async fn the_first_poll_proceeds_before_a_large_archive_is_written() {
        use crate::backend::{command_loop, connections::Inbound, tests::serve_by_method};

        let (url, requests) = serve_by_method(|_, _, _| ("200 OK", "[]".into()));
        let tmp = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx(&tmp.path().canonicalize().unwrap(), &url, &[]);
        ctx.with_cache(|cache| {
            for number in 0..2_000 {
                cache
                    .insert_item(&worktree("local/notes", &format!("branch-{number}")))
                    .unwrap();
            }
        });

        let (gathered, gathered_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        *ctx.knowledge.shared.after_gather.lock().unwrap() = Some(Box::new(move || {
            let _ = gathered.send(());
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
        }));
        let (commands, inbound) = tokio::sync::mpsc::unbounded_channel();
        let engine = tokio::spawn(command_loop(ctx.clone(), inbound));
        tokio::time::timeout(Duration::from_secs(5), gathered_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(requests
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.starts_with("/notifications")));
        assert!(!ctx.knowledge.root.join("knowledge/perch/repos").exists());
        commands.send(Inbound::Shutdown).unwrap();
        tokio::time::timeout(Duration::from_secs(5), engine)
            .await
            .unwrap()
            .unwrap();
        release.send(()).unwrap();
    }

    #[tokio::test]
    async fn delete_waits_for_an_export_that_already_gathered_the_item() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx(&dir.path().canonicalize().unwrap(), &nowhere(), &[]);
        let item = worktree("a/b", "ready");
        let path = place(&ctx.knowledge.root, &item).unwrap().path();
        ctx.with_cache(|cache| cache.insert_item(&item).unwrap());

        let (gathered, gathered_rx) = tokio::sync::oneshot::channel();
        let (resume, resume_rx) = std::sync::mpsc::channel();
        *ctx.knowledge.shared.after_gather.lock().unwrap() = Some(Box::new(move || {
            gathered.send(()).unwrap();
            resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }));
        let writing = tokio::spawn({
            let ctx = ctx.clone();
            async move { flush(&ctx, false).await }
        });
        gathered_rx.await.unwrap();
        let mut deleting = tokio::spawn({
            let ctx = ctx.clone();
            let id = item.id.clone();
            async move { delete_item(&ctx, &id, true).await }
        });
        let blocked = tokio::time::timeout(Duration::from_millis(50), &mut deleting).await;
        resume.send(()).unwrap();
        assert!(blocked.is_err(), "Delete must wait for the exporter");
        writing.await.unwrap();
        deleting.await.unwrap().unwrap();
        flush(&ctx, false).await;
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn startup_uses_the_existing_index_through_a_symlinked_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let (mut ctx, _) = ctx(&home, &nowhere(), &[]);
        let actual = ctx.knowledge.root.clone();
        fs::create_dir_all(&actual).unwrap();
        let alias = home.join("alias");
        std::os::unix::fs::symlink(&actual, &alias).unwrap();
        ctx.knowledge = Archive::new(alias, ctx.knowledge.db.clone());

        let items = ["one", "two", "three"].map(|branch| worktree("a/b", branch));
        ctx.with_cache(|cache| {
            for item in &items {
                cache.insert_item(item).unwrap();
            }
        });
        flush(&ctx, true).await;
        let repo = repo_place(&actual, "a/b").unwrap();
        let index = repo.join("INDEX.md");
        let contents = fs::read_to_string(&index).unwrap();
        let mtime = fs::metadata(&index).unwrap().modified().unwrap();

        *ctx.knowledge.shared.catalog.lock().unwrap() = Catalog::default();
        let archive = &ctx.knowledge;
        let cache = Cache::open_read_only(&archive.db).unwrap();
        assert_eq!(ctx.knowledge_dir(&items[0]).await, Some(repo.clone()));
        for item in &items[..2] {
            assert!(archive.write_item(&cache, &item.id).unwrap());
        }
        assert_eq!(ctx.knowledge_dir(&items[0]).await, Some(repo));
        delete_item(&ctx, &items[1].id, true).await.unwrap();
        assert!(!place(&actual, &items[1]).unwrap().path().exists());
        assert_eq!(fs::read_to_string(&index).unwrap(), contents);
        assert_eq!(fs::metadata(&index).unwrap().modified().unwrap(), mtime);
    }

    #[tokio::test]
    async fn the_live_archive_matches_a_rebuild_after_unopened_item_changes_and_takeover() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx(&dir.path().canonicalize().unwrap(), &nowhere(), &[]);
        let archive = &ctx.knowledge;
        let mut branch = worktree("a/b", "branch");
        let mut pull = Item::new("a/b", Timestamp::now());
        pull.subject = Some(ItemSubject::PullRequest(pull_request(7)));
        let pull_id = pull.id.clone();
        let branch_path = place(&archive.root, &branch).unwrap().path();
        let pull_place = place(&archive.root, &pull).unwrap();
        let pull_path = pull_place.path();
        ctx.with_cache(|cache| {
            cache.insert_item(&branch).unwrap();
            cache.insert_item(&pull).unwrap();
            cache
                .insert_comment_thread(&private_thread(&pull, "question"))
                .unwrap();
            cache
                .insert_comment_thread(&private_thread(&pull, "removed"))
                .unwrap();
            admit_review(cache, &pull, "pull-review");
        });
        flush(&ctx, true).await;
        assert!(branch_path.join("INDEX.md").is_file());
        assert!(pull_path.join("private/removed.md").is_file());
        assert_eq!(
            ctx.knowledge_dir(&branch).await,
            Some(pull_place.repo.clone())
        );

        ctx.with_cache(|cache| {
            cache
                .store_checks(
                    &pull.id,
                    &pull.pull_request().unwrap().head_sha,
                    &Checks {
                        state: CheckState::Passing,
                        url: None,
                        runs: vec![perch_github::Check {
                            name: "unit tests".into(),
                            workflow: None,
                            event: None,
                            state: perch_github::CheckRunState::Success,
                            description: None,
                            url: None,
                            started_at: None,
                            completed_at: None,
                            kind: perch_github::CheckKind::StatusContext,
                        }],
                    },
                )
                .unwrap();
            cache.delete_private_messages("removed").unwrap();
            finish_review(cache, "pull-review");
        });
        flush(&ctx, false).await;
        assert!(
            pull_path.join("checks.md").is_file(),
            "checks without a stored discussion"
        );
        assert!(!pull_path.join("private/removed.md").exists());
        assert!(fs::read_to_string(pull_path.join("findings.md"))
            .unwrap()
            .contains("### Review #1 · `head` · completed"));

        let mut merged = pull_request(7);
        merged.merged = true;
        merged.state = "closed".into();
        pull.subject = Some(ItemSubject::PullRequest(merged.clone()));
        ctx.with_cache(|cache| {
            cache.update_item(&pull).unwrap();
            cache
                .rewrite_discussion(&pull.id, &Discussion::default())
                .unwrap();
        });
        flush(&ctx, false).await;
        assert!(fs::read_to_string(pull_place.repo.join("INDEX.md"))
            .unwrap()
            .contains("merged"));

        ctx.with_cache(|cache| {
            admit_review(cache, &branch, "branch-review");
            finish_review(cache, "branch-review");
            assert!(take_pull_request(cache, &mut branch, merged).unwrap());
            assert!(cache.load_item(&pull_id).unwrap().is_none());
            assert_eq!(
                cache
                    .list_review_groups(&branch.id, None, 10)
                    .unwrap()
                    .len(),
                2
            );
            assert!(cache.load_discussion(&branch.id).unwrap().is_some());
        });
        flush(&ctx, false).await;
        assert!(!branch_path.exists());
        assert!(pull_path.join("private/question.md").is_file());
        assert!(ctx.knowledge_dir(&branch).await.is_none());

        let knowledge = archive.root.join("knowledge");
        let before = tree(&knowledge);
        let repo_index = pull_place.repo.join("INDEX.md");
        let mtime = fs::metadata(&repo_index).unwrap().modified().unwrap();
        flush(&ctx, true).await;
        assert_eq!(
            fs::metadata(&repo_index).unwrap().modified().unwrap(),
            mtime
        );
        fs::remove_dir_all(knowledge.join("perch")).unwrap();
        let legacy = knowledge.join("reviews/a/b");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("old.md"), "old history").unwrap();
        let abandoned = pull_place.repo.join("pulls/23");
        fs::create_dir_all(&abandoned).unwrap();
        fs::write(abandoned.join("INDEX.md"), "left after a crash").unwrap();
        // A fresh process starts with an empty catalog.
        *ctx.knowledge.shared.catalog.lock().unwrap() = Catalog::default();
        flush(&ctx, true).await;
        assert_eq!(tree(&knowledge), before);
        assert!(!abandoned.exists());
        assert!(!knowledge.join("reviews").exists());
        delete_item(&ctx, &branch.id, true).await.unwrap();
        assert!(!pull_path.exists());
        assert!(!pull_place.repo.exists());
        flush(&ctx, false).await;
        assert!(!pull_path.exists());
    }
}
