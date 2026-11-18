//! Perch's live, rebuildable Markdown view of the cache.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use perch_github::{Cache, CacheError, Item, ItemKind, SubjectState};
use perch_knowledge::sanitize::{clean, one_line};
use perch_knowledge::{
    child_dirs, encode_component, ensure_dir, prune_temporary_files, remove_dir, remove_empty_dir,
    safe_component, sync, write_file,
};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use super::Ctx;
use crate::context_cli::{gather_archive, render, Mode};

const ITEM_KINDS: [&str; 3] = ["pulls", "issues", "worktrees"];

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
}

#[derive(PartialEq, Eq)]
struct Entry {
    id: String,
    line: String,
}

impl Catalog {
    fn write_item(
        &mut self,
        place: Place,
        item: &Item,
        files: &BTreeMap<String, String>,
    ) -> io::Result<()> {
        sync(&place.path(), files)?;

        let state = match item.subject.as_ref().map(|subject| subject.subject_state()) {
            Some(SubjectState::PrDraft) => "draft",
            Some(SubjectState::PrMerged) => "merged",
            Some(SubjectState::PrClosed | SubjectState::IssueClosed) => "closed",
            Some(SubjectState::IssueNotPlanned) => "not planned",
            Some(SubjectState::PrOpen | SubjectState::IssueOpen) => "open",
            None => "worktree",
        };
        let archived = if item.is_archived() {
            "archived · "
        } else {
            ""
        };
        let line = format!(
            "- `{}/` · {archived}{state} · {}",
            place.item.display(),
            one_line(&clean(item.title()))
        );
        let entry = Entry {
            id: item.id.clone(),
            line,
        };
        let entries = self.repos.entry(place.repo.clone()).or_default();
        if entries.get(&place.item) != Some(&entry) {
            if let Some(old) = entries.insert(place.item.clone(), entry) {
                if old.id != item.id {
                    self.places.remove(&old.id);
                }
            }
            self.changed_repos.insert(place.repo.clone());
        }
        if let Some(old) = self.places.insert(item.id.clone(), place.clone()) {
            if old != place {
                self.remove_at(&old, &item.id);
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

    fn write_indexes(&mut self) {
        for repo in self.changed_repos.clone() {
            if let Err(error) = self.write_index(&repo) {
                warn_path(&repo.join("INDEX.md"), error);
            }
        }
    }
}

impl Archive {
    pub(super) fn new(root: PathBuf, db: PathBuf) -> Self {
        Self {
            root: std::path::absolute(&root).unwrap_or(root),
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
        let (mut cache, items) = tokio::task::spawn_blocking(move || {
            let cache = Cache::open_read_only(&db)?;
            let items = cache.load_items()?;
            Ok::<_, CacheError>((cache, items))
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

        let ids = items.into_iter().map(|item| item.id).collect();
        cache = self.write_batch(cache, &lifecycle, ids, true).await?;
        loop {
            self.shared.ready.notified().await;
            tokio::time::sleep(Duration::from_millis(150)).await;
            let ids = std::mem::take(&mut *self.shared.marked.lock().expect("archive marks"));
            if !ids.is_empty() {
                cache = self.write_batch(cache, &lifecycle, ids, false).await?;
            }
        }
    }

    async fn write_batch(
        &self,
        mut cache: Cache,
        lifecycle: &perch_git::KeyedMutex,
        ids: BTreeSet<String>,
        startup: bool,
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
            catalog.write_indexes();
            if startup {
                match cache.load_items() {
                    Ok(items) => archive.sweep(&catalog, &items),
                    Err(error) => {
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
        let files = render(&gathered, Mode::Archive);
        #[cfg(test)]
        if let Some(pause) = self.shared.after_gather.lock().unwrap().take() {
            pause();
        }
        self.shared
            .catalog
            .lock()
            .expect("archive catalog")
            .write_item(place, &item, &files)
            .map_err(|error| error.to_string())?;
        Ok(true)
    }

    fn sweep(&self, catalog: &Catalog, items: &[Item]) {
        let keep: HashSet<_> = items
            .iter()
            .filter_map(|item| place(&self.root, item))
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
            catalog.write_indexes();
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
        let shared = self.knowledge.shared.clone();
        let id = item.id.clone();
        tokio::task::spawn_blocking(move || {
            let mut catalog = shared.catalog.lock().expect("archive catalog");
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
                if catalog.changed_repos.contains(&place.repo)
                    || !place.repo.join("INDEX.md").is_file()
                {
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
    let cache = Cache::open_read_only(&root.join("perch.db")).ok()?;
    let (target, items) = cache
        .read_snapshot(|cache| {
            Ok::<_, CacheError>((
                cache.find_item_by_subject(repo, kind, number)?,
                cache.load_items()?,
            ))
        })
        .ok()?
        .ok()?;
    let own = target.as_ref().and_then(|item| place(root, item));
    let repo_path = match &own {
        Some(place) => place.repo.clone(),
        None => repo_place(root, repo)?,
    };
    if !std::fs::symlink_metadata(repo_path.join("INDEX.md")).is_ok_and(|meta| meta.is_file()) {
        return None;
    }
    let other = items
        .iter()
        .filter_map(|item| place(root, item))
        .any(|place| {
            place.repo == repo_path
                && Some(&place) != own.as_ref()
                && std::fs::symlink_metadata(place.path().join("INDEX.md"))
                    .is_ok_and(|meta| meta.is_file())
        });
    if other {
        // Opening the directory without following symlinks also verifies
        // its ancestors before a backend is pointed at it.
        child_dirs(&repo_path).ok()?;
        std::path::absolute(repo_path).ok()
    } else {
        None
    }
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
