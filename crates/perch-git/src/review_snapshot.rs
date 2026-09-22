//! Immutable source capture for native code reviews.
//!
//! A snapshot owns its object database. It never borrows objects from the
//! reader's clone, so historical diffs and anchors survive source removal and
//! aggressive garbage collection. Capture is staged beside the destination
//! and published with one rename; every error or cancelled future removes its
//! staging directory.

use std::collections::{HashMap, HashSet};
use std::ops::RangeInclusive;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::cmd::Git;
use crate::diff::{file_diff, parse_numstat, ChangeStatus};
use crate::scope::alignment;

const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(300);
const SNAPSHOT_FORMAT: u32 = 1;
const MANIFEST_FILE: &str = "manifest.json";
const DIFF_FILE: &str = "review.diff";
const RETAINED_REPO: &str = "repo.git";
const CHECKOUT_DIR: &str = "checkout";

/// Immutable names passed to native review adapters.
pub const BASE_TIP_REF: &str = "refs/perch/review/base-tip";
pub const MERGE_BASE_REF: &str = "refs/perch/review/merge-base";
pub const HEAD_REF: &str = "refs/perch/review/head";

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error(transparent)]
    Git(#[from] crate::Error),
    #[error("filesystem error: {0}")]
    Io(String),
    #[error("snapshot manifest error: {0}")]
    Manifest(String),
    #[error("invalid snapshot id {0:?}")]
    InvalidSnapshotId(String),
    #[error("unsafe repository path {0:?}")]
    InvalidRepositoryPath(String),
    #[error("snapshot {0:?} already exists")]
    AlreadyExists(String),
    #[error("snapshot {0:?} does not exist")]
    NotFound(String),
    #[error("the requested base and HEAD share no commit")]
    NoMergeBase,
    #[error("git returned an invalid object id: {0:?}")]
    InvalidObjectId(String),
    #[error("the retained checkout for snapshot {0:?} is dirty or at the wrong commit")]
    CheckoutChanged(String),
}

impl From<std::io::Error> for SnapshotError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<serde_json::Error> for SnapshotError {
    fn from(error: serde_json::Error) -> Self {
        Self::Manifest(error.to_string())
    }
}

/// Everything resolved once at review admission. All later Git operations
/// use these object IDs rather than resolving a moving ref again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedReviewRange {
    pub source_repository: String,
    pub branch_label: String,
    pub requested_base: String,
    pub base_tip: String,
    pub merge_base: String,
    pub head: String,
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotCapture {
    pub snapshot_id: String,
    pub source_checkout: PathBuf,
    pub source_repository: String,
    pub branch_label: String,
    pub requested_base: String,
}

/// A clean result is distinct from an error: callers should show the empty
/// scope message and must not admit an agent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureOutcome {
    Captured(ReviewSnapshot),
    Empty(ResolvedReviewRange),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotFile {
    /// Path on the head side, or the base path for a deletion.
    pub path: String,
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    pub base_blob: Option<String>,
    pub head_blob: Option<String>,
    pub base_mode: String,
    pub head_mode: String,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
}

impl SnapshotFile {
    pub fn path_on(&self, side: DiffSide) -> Option<&str> {
        match side {
            DiffSide::Left => self
                .base_blob
                .as_ref()
                .map(|_| self.old_path.as_deref().unwrap_or(&self.path)),
            DiffSide::Right => self.head_blob.as_ref().map(|_| self.path.as_str()),
        }
    }

    pub fn blob_on(&self, side: DiffSide) -> Option<&str> {
        match side {
            DiffSide::Left => self.base_blob.as_deref(),
            DiffSide::Right => self.head_blob.as_deref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub format_version: u32,
    pub snapshot_id: String,
    pub captured_at: jiff::Timestamp,
    pub range: ResolvedReviewRange,
    pub files: Vec<SnapshotFile>,
    pub diff_file: String,
    pub retained_repo: String,
    pub checkout_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewSnapshot {
    root: PathBuf,
    pub manifest: SnapshotManifest,
}

impl ReviewSnapshot {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn retained_repo(&self) -> PathBuf {
        self.root.join(RETAINED_REPO)
    }

    pub fn checkout(&self) -> PathBuf {
        self.root.join(CHECKOUT_DIR)
    }

    pub fn diff_path(&self) -> PathBuf {
        self.root.join(DIFF_FILE)
    }

    pub fn read_diff(&self) -> Result<Vec<u8>, SnapshotError> {
        std::fs::read(self.diff_path()).map_err(Into::into)
    }

    /// Read the exact retained blob for one changed-file side. Paths which
    /// are not in the captured diff return `None` rather than being resolved
    /// against an unrelated tree entry.
    pub async fn read_file(
        &self,
        side: DiffSide,
        path: &str,
    ) -> Result<Option<Vec<u8>>, SnapshotError> {
        validate_repository_path(path)?;
        let blob = self.manifest.files.iter().find_map(|file| {
            (file.path_on(side) == Some(path))
                .then(|| file.blob_on(side))
                .flatten()
        });
        let Some(blob) = blob else { return Ok(None) };
        let git = Git::new(None);
        git.run_bytes(
            Some(&self.retained_repo()),
            ["cat-file", "blob", blob],
            QUERY_TIMEOUT,
        )
        .await
        .map(Some)
        .map_err(Into::into)
    }

    /// The head lines of a changed file that its hunks show, one range to
    /// a hunk: the changed lines and three either side, as a unified diff
    /// draws them. These are the lines of a pull request's diff, which
    /// GitHub takes a review comment on and outside which it refuses one.
    /// An added file is one hunk. Empty for a path with no head side, a
    /// binary file, or one whose content did not change.
    pub async fn head_hunks(&self, path: &str) -> Result<Vec<RangeInclusive<u32>>, SnapshotError> {
        validate_repository_path(path)?;
        let Some(file) = self
            .manifest
            .files
            .iter()
            .find(|file| file.path_on(DiffSide::Right) == Some(path))
        else {
            return Ok(Vec::new());
        };
        file_hunks(
            &Git::new(None),
            &self.retained_repo(),
            file,
            DiffSide::Right,
        )
        .await
    }

    /// Load text/blob inputs for the pure mapper. Binary files retain their
    /// identity but deliberately have no text for context mapping.
    pub async fn mapping_snapshot(&self) -> Result<MappingSnapshot, SnapshotError> {
        let git = Git::new(None);
        let repo = self.retained_repo();
        let mut documents = Vec::new();
        let mut seen = HashSet::new();
        for file in &self.manifest.files {
            for side in [DiffSide::Left, DiffSide::Right] {
                let (Some(path), Some(blob)) = (file.path_on(side), file.blob_on(side)) else {
                    continue;
                };
                if !seen.insert((side, path.to_string(), blob.to_string())) {
                    continue;
                }
                let bytes = git
                    .run_bytes(Some(&repo), ["cat-file", "blob", blob], QUERY_TIMEOUT)
                    .await?;
                let text = (!file.binary)
                    .then(|| String::from_utf8(bytes).ok())
                    .flatten();
                let counterpart_path = match side {
                    DiffSide::Left => file.head_blob.as_ref().map(|_| file.path.clone()),
                    DiffSide::Right => file
                        .base_blob
                        .as_ref()
                        .map(|_| file.old_path.clone().unwrap_or_else(|| file.path.clone())),
                };
                documents.push(MappingDocument {
                    path: path.to_string(),
                    counterpart_path,
                    side,
                    blob: blob.to_string(),
                    text,
                });
            }
        }
        Ok(MappingSnapshot {
            snapshot_id: self.manifest.snapshot_id.clone(),
            documents,
        })
    }
}

/// The files a range of commits changed, listed from a clone as a
/// snapshot's manifest lists them, with nothing captured: a pull request's
/// diff while its checkout is open, to ask which lines GitHub takes a
/// review comment on.
#[derive(Debug, Clone)]
pub struct RangeDiff {
    repo: PathBuf,
    files: Vec<SnapshotFile>,
}

impl RangeDiff {
    /// What `merge_base..head` changed, read in `repo`, which has both.
    pub async fn read(repo: &Path, merge_base: &str, head: &str) -> Result<Self, SnapshotError> {
        let files = range_files(&Git::new(None), repo, merge_base, head, QUERY_TIMEOUT).await?;
        Ok(Self {
            repo: repo.to_path_buf(),
            files,
        })
    }

    /// The lines on `side` of the file at `path` that the range's hunks
    /// show, one range to a hunk, as `ReviewSnapshot::head_hunks` reads a
    /// head's. `path` is the file's name as GitHub knows it in a pull
    /// request: the head's, even for its base side, and the base's for a
    /// deleted file. Empty for a file the range did not change.
    pub async fn hunks(
        &self,
        path: &str,
        side: DiffSide,
    ) -> Result<Vec<RangeInclusive<u32>>, SnapshotError> {
        let Some(file) = self.files.iter().find(|file| file.path == path) else {
            return Ok(Vec::new());
        };
        file_hunks(&Git::new(None), &self.repo, file, side).await
    }
}

#[derive(Debug, Clone)]
pub struct ReviewSnapshotStore {
    root: PathBuf,
    git: Git,
}

impl ReviewSnapshotStore {
    /// `root` is Perch's data root. Snapshots live below a dedicated
    /// `review-snapshots` directory and never overlap normal worktrees.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into().join("review-snapshots"),
            git: Git::new(None),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Remove capture staging directories left by process death and complete
    /// snapshots which were published but never admitted to the database.
    ///
    /// The store is dedicated to review snapshots, but cleanup is still
    /// deliberately conservative: an unreferenced directory is removed only
    /// when it contains a valid manifest whose identity matches its basename.
    /// Unknown files and malformed directories are left for diagnosis.
    pub fn scavenge(&self, retained_ids: &HashSet<String>) -> Result<usize, SnapshotError> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => {
                return Err(io_error(
                    &format!("reading snapshot store {}", self.root.display()),
                    error,
                ))
            }
        };
        let mut removed = 0;
        for entry in entries {
            let entry = entry.map_err(|error| {
                io_error(
                    &format!("reading an entry below {}", self.root.display()),
                    error,
                )
            })?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let path = entry.path();
            if retained_ids.contains(name) {
                continue;
            }
            if name.starts_with('.') && name.contains(".partial-") {
                remove_store_entry(&path)?;
                removed += 1;
                continue;
            }
            if validate_snapshot_id(name).is_err() {
                continue;
            }
            let manifest = std::fs::read(path.join(MANIFEST_FILE))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<SnapshotManifest>(&bytes).ok());
            if manifest.as_ref().is_some_and(|manifest| {
                manifest.format_version == SNAPSHOT_FORMAT && manifest.snapshot_id == name
            }) {
                remove_store_entry(&path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub fn open(&self, snapshot_id: &str) -> Result<Option<ReviewSnapshot>, SnapshotError> {
        let root = self.snapshot_path(snapshot_id)?;
        let metadata = match std::fs::symlink_metadata(&root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(SnapshotError::Manifest(format!(
                "snapshot {snapshot_id:?} is not a retained directory"
            )));
        }
        let bytes = std::fs::read(root.join(MANIFEST_FILE))?;
        let manifest: SnapshotManifest = serde_json::from_slice(&bytes)?;
        if manifest.format_version != SNAPSHOT_FORMAT || manifest.snapshot_id != snapshot_id {
            return Err(SnapshotError::Manifest(format!(
                "snapshot {snapshot_id:?} has mismatched identity or format"
            )));
        }
        validate_manifest(&manifest)?;
        Ok(Some(ReviewSnapshot { root, manifest }))
    }

    pub async fn capture(&self, request: SnapshotCapture) -> Result<CaptureOutcome, SnapshotError> {
        self.capture_with_resolved_hook(request, |_| {}).await
    }

    async fn capture_with_resolved_hook(
        &self,
        request: SnapshotCapture,
        after_resolve: impl FnOnce(&ResolvedReviewRange),
    ) -> Result<CaptureOutcome, SnapshotError> {
        validate_snapshot_id(&request.snapshot_id)?;
        let source = std::fs::canonicalize(&request.source_checkout)?;
        let final_root = self.snapshot_path(&request.snapshot_id)?;
        if final_root.exists() {
            return Err(SnapshotError::AlreadyExists(request.snapshot_id));
        }

        let range = self.resolve_range(&source, &request).await?;
        after_resolve(&range);
        let diff = self.binary_diff(&source, &range).await?;
        if diff.is_empty() {
            return Ok(CaptureOutcome::Empty(range));
        }
        let files = range_files(
            &self.git,
            &source,
            &range.merge_base,
            &range.head,
            CAPTURE_TIMEOUT,
        )
        .await?;

        let staging =
            create_unique_directory(&self.root, &format!(".{}.partial", request.snapshot_id))?;
        let mut cleanup = StagingCleanup::new(staging.clone());
        let retained_repo = staging.join(RETAINED_REPO);
        self.git
            .run(
                None,
                [
                    "init".to_string(),
                    "--bare".to_string(),
                    "--quiet".to_string(),
                    retained_repo.to_string_lossy().into_owned(),
                ],
                QUERY_TIMEOUT,
            )
            .await?;
        self.retain_objects(&source, &retained_repo, &range).await?;
        self.write_private_refs(&retained_repo, &range).await?;
        self.materialize_checkout(&retained_repo, &staging.join(CHECKOUT_DIR), &range.head)
            .await?;

        std::fs::write(staging.join(DIFF_FILE), diff)?;
        let manifest = SnapshotManifest {
            format_version: SNAPSHOT_FORMAT,
            snapshot_id: request.snapshot_id.clone(),
            captured_at: jiff::Timestamp::now(),
            range,
            files,
            diff_file: DIFF_FILE.into(),
            retained_repo: RETAINED_REPO.into(),
            checkout_dir: CHECKOUT_DIR.into(),
        };
        std::fs::write(
            staging.join(MANIFEST_FILE),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        make_tree_readonly(&staging.join(CHECKOUT_DIR))?;

        match std::fs::rename(&staging, &final_root) {
            Ok(()) => cleanup.disarm(),
            Err(_error) if final_root.exists() => {
                return Err(SnapshotError::AlreadyExists(request.snapshot_id));
            }
            Err(error) => {
                return Err(io_error(
                    &format!(
                        "publishing snapshot {} as {}",
                        staging.display(),
                        final_root.display()
                    ),
                    error,
                ));
            }
        }
        Ok(CaptureOutcome::Captured(ReviewSnapshot {
            root: final_root,
            manifest,
        }))
    }

    /// Recreate a cleaned-up detached checkout solely from retained data.
    /// An existing checkout is accepted only when it is clean and still at
    /// the captured head; no reset is performed over unexpected edits.
    pub async fn ensure_checkout(&self, snapshot_id: &str) -> Result<PathBuf, SnapshotError> {
        let snapshot = self
            .open(snapshot_id)?
            .ok_or_else(|| SnapshotError::NotFound(snapshot_id.to_string()))?;
        let checkout = snapshot.checkout();
        if checkout.exists() {
            self.verify_checkout(&snapshot, &checkout).await?;
            make_tree_readonly(&checkout)?;
            return Ok(checkout);
        }

        let temporary = create_unique_directory(&snapshot.root, ".checkout.partial")?;
        // `git init` requires the destination not to exist, so the guard owns
        // the parent while the actual repository is built one level below.
        let candidate = temporary.join(CHECKOUT_DIR);
        let mut cleanup = StagingCleanup::new(temporary.clone());
        self.materialize_checkout(
            &snapshot.retained_repo(),
            &candidate,
            &snapshot.manifest.range.head,
        )
        .await?;
        make_tree_readonly(&candidate)?;
        // macOS rejects renaming a directory whose own mode is read-only.
        // Keep only the unpublished root owner-writable for the rename; every
        // descendant is already hardened, and the destination root is made
        // read-only before this method returns it.
        set_path_owner_writable(&candidate, true)?;
        match std::fs::rename(&candidate, &checkout) {
            Ok(()) => {
                set_path_owner_writable(&checkout, false)?;
                cleanup.disarm();
                let _ = std::fs::remove_dir(&temporary);
                Ok(checkout)
            }
            Err(_) if checkout.exists() => {
                // A concurrent recreation won the rename. Accept only the
                // exact clean checkout for this snapshot, never an arbitrary
                // directory which happened to appear at the destination.
                self.verify_checkout(&snapshot, &checkout).await?;
                make_tree_readonly(&checkout)?;
                Ok(checkout)
            }
            Err(error) => Err(io_error(
                &format!(
                    "publishing checkout {} as {}",
                    candidate.display(),
                    checkout.display()
                ),
                error,
            )),
        }
    }

    /// Remove only the disposable checkout. The retained object database,
    /// refs, manifest and exact diff remain readable.
    pub fn cleanup_checkout(&self, snapshot_id: &str) -> Result<bool, SnapshotError> {
        let snapshot = self
            .open(snapshot_id)?
            .ok_or_else(|| SnapshotError::NotFound(snapshot_id.to_string()))?;
        let checkout = snapshot.checkout();
        if !checkout.exists() {
            return Ok(false);
        }
        make_tree_writable(&checkout)?;
        std::fs::remove_dir_all(checkout)?;
        Ok(true)
    }

    /// Explicitly release exactly one snapshot. Callers must first stop all
    /// processes using its checkout. Objects are not shared between snapshot
    /// repositories, so no other snapshot can be affected.
    pub fn release(&self, snapshot_id: &str) -> Result<bool, SnapshotError> {
        let path = self.snapshot_path(snapshot_id)?;
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() || metadata.is_file() {
            std::fs::remove_file(path)?;
        } else {
            make_tree_writable(&path)?;
            std::fs::remove_dir_all(path)?;
        }
        Ok(true)
    }

    async fn verify_checkout(
        &self,
        snapshot: &ReviewSnapshot,
        checkout: &Path,
    ) -> Result<(), SnapshotError> {
        let metadata = std::fs::symlink_metadata(checkout)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(SnapshotError::CheckoutChanged(
                snapshot.manifest.snapshot_id.clone(),
            ));
        }
        let head = self
            .git
            .run(Some(checkout), ["rev-parse", "HEAD"], QUERY_TIMEOUT)
            .await?
            .trim()
            .to_string();
        // A hardened checkout cannot take Git's optional index lock. This
        // flag keeps status strictly read-only while still finding untracked
        // files and worktree changes.
        let dirty = self
            .git
            .run(
                Some(checkout),
                [
                    "--no-optional-locks",
                    "status",
                    "--porcelain",
                    "--untracked-files=normal",
                ],
                QUERY_TIMEOUT,
            )
            .await?;
        if head == snapshot.manifest.range.head && dirty.is_empty() {
            Ok(())
        } else {
            Err(SnapshotError::CheckoutChanged(
                snapshot.manifest.snapshot_id.clone(),
            ))
        }
    }

    async fn resolve_range(
        &self,
        source: &Path,
        request: &SnapshotCapture,
    ) -> Result<ResolvedReviewRange, SnapshotError> {
        let base_spec = format!("{}^{{commit}}", request.requested_base);
        let base_tip = self.resolve_commit(source, &base_spec).await?;
        let head = self.resolve_commit(source, "HEAD^{commit}").await?;
        let merge_base = match self
            .git
            .run(
                Some(source),
                ["merge-base", base_tip.as_str(), head.as_str()],
                QUERY_TIMEOUT,
            )
            .await
        {
            Ok(value) => checked_object_id(value.trim())?,
            Err(crate::Error::Git { status: 1, .. }) => return Err(SnapshotError::NoMergeBase),
            Err(error) => return Err(error.into()),
        };
        let dirty = !self
            .git
            .run(
                Some(source),
                ["status", "--porcelain", "--untracked-files=normal"],
                QUERY_TIMEOUT,
            )
            .await?
            .is_empty();
        Ok(ResolvedReviewRange {
            source_repository: request.source_repository.clone(),
            branch_label: request.branch_label.clone(),
            requested_base: request.requested_base.clone(),
            base_tip,
            merge_base,
            head,
            dirty,
        })
    }

    async fn resolve_commit(&self, source: &Path, spec: &str) -> Result<String, SnapshotError> {
        let output = self
            .git
            .run(
                Some(source),
                ["rev-parse", "--verify", "--end-of-options", spec],
                QUERY_TIMEOUT,
            )
            .await?;
        checked_object_id(output.trim())
    }

    async fn binary_diff(
        &self,
        source: &Path,
        range: &ResolvedReviewRange,
    ) -> Result<Vec<u8>, SnapshotError> {
        self.git
            .run_bytes(
                Some(source),
                [
                    "diff",
                    "--binary",
                    "--full-index",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-color",
                    "--find-renames",
                    range.merge_base.as_str(),
                    range.head.as_str(),
                    "--",
                ],
                CAPTURE_TIMEOUT,
            )
            .await
            .map_err(Into::into)
    }

    async fn retain_objects(
        &self,
        source: &Path,
        retained_repo: &Path,
        range: &ResolvedReviewRange,
    ) -> Result<(), SnapshotError> {
        let pack_prefix = retained_repo.join("objects/pack/review");
        let revisions = format!("{}\n{}\n{}\n", range.base_tip, range.merge_base, range.head);
        self.git
            .run_with_input(
                Some(source),
                [
                    "pack-objects".to_string(),
                    "--revs".to_string(),
                    "--delta-base-offset".to_string(),
                    pack_prefix.to_string_lossy().into_owned(),
                ],
                revisions.as_bytes(),
                CAPTURE_TIMEOUT,
            )
            .await?;
        for oid in [&range.base_tip, &range.merge_base, &range.head] {
            self.git
                .run(
                    Some(retained_repo),
                    ["cat-file", "-e", &format!("{oid}^{{commit}}")],
                    QUERY_TIMEOUT,
                )
                .await?;
        }
        Ok(())
    }

    async fn write_private_refs(
        &self,
        retained_repo: &Path,
        range: &ResolvedReviewRange,
    ) -> Result<(), SnapshotError> {
        for (name, oid) in [
            (BASE_TIP_REF, range.base_tip.as_str()),
            (MERGE_BASE_REF, range.merge_base.as_str()),
            (HEAD_REF, range.head.as_str()),
        ] {
            self.git
                .run(
                    Some(retained_repo),
                    ["update-ref", name, oid],
                    QUERY_TIMEOUT,
                )
                .await?;
        }
        Ok(())
    }

    async fn materialize_checkout(
        &self,
        retained_repo: &Path,
        checkout: &Path,
        head: &str,
    ) -> Result<(), SnapshotError> {
        self.git
            .run(
                None,
                [
                    "init".to_string(),
                    "--quiet".to_string(),
                    checkout.to_string_lossy().into_owned(),
                ],
                QUERY_TIMEOUT,
            )
            .await?;
        self.git
            .run(
                Some(checkout),
                [
                    "fetch".to_string(),
                    "--quiet".to_string(),
                    "--no-tags".to_string(),
                    retained_repo.to_string_lossy().into_owned(),
                    format!("+{BASE_TIP_REF}:{BASE_TIP_REF}"),
                    format!("+{MERGE_BASE_REF}:{MERGE_BASE_REF}"),
                    format!("+{HEAD_REF}:{HEAD_REF}"),
                ],
                CAPTURE_TIMEOUT,
            )
            .await?;
        self.git
            .run(
                Some(checkout),
                ["checkout", "--detach", "--quiet", head],
                QUERY_TIMEOUT,
            )
            .await?;
        let dirty = self
            .git
            .run(
                Some(checkout),
                ["status", "--porcelain", "--untracked-files=normal"],
                QUERY_TIMEOUT,
            )
            .await?;
        if !dirty.is_empty() {
            return Err(SnapshotError::CheckoutChanged(
                checkout.display().to_string(),
            ));
        }
        Ok(())
    }

    fn snapshot_path(&self, snapshot_id: &str) -> Result<PathBuf, SnapshotError> {
        validate_snapshot_id(snapshot_id)?;
        Ok(self.root.join(snapshot_id))
    }
}

fn remove_store_entry(path: &Path) -> Result<(), SnapshotError> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| io_error(&format!("reading {} for cleanup", path.display()), error))?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        std::fs::remove_file(path)?;
    } else {
        make_tree_writable(path)?;
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

struct StagingCleanup {
    path: PathBuf,
    armed: bool,
}

impl StagingCleanup {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagingCleanup {
    fn drop(&mut self) {
        if self.armed {
            // Capture may have hardened the checkout immediately before a
            // failed publish. Restore owner-write permission so cancellation
            // and all error paths can still remove the entire partial tree.
            let _ = make_tree_writable(&self.path);
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

fn create_unique_directory(parent: &Path, prefix: &str) -> Result<PathBuf, SnapshotError> {
    std::fs::create_dir_all(parent).map_err(|error| {
        io_error(
            &format!("creating staging parent {}", parent.display()),
            error,
        )
    })?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for _ in 0..64 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            "{prefix}-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(io_error(
                    &format!("creating staging directory {}", path.display()),
                    error,
                ));
            }
        }
    }
    Err(SnapshotError::Io(format!(
        "could not allocate a unique staging directory below {}",
        parent.display()
    )))
}

/// Remove write bits from a checkout after it has been fully materialized.
/// Symlinks are never followed. The native-agent sandbox remains the primary
/// boundary; these permissions make accidental edits fail at the filesystem
/// too and allow concurrent readers to share one checkout safely.
fn make_tree_readonly(path: &Path) -> Result<(), SnapshotError> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| io_error(&format!("reading metadata for {}", path.display()), error))?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)
            .map_err(|error| io_error(&format!("reading directory {}", path.display()), error))?
        {
            let child = entry
                .map_err(|error| {
                    io_error(&format!("reading entry below {}", path.display()), error)
                })?
                .path();
            make_tree_readonly(&child)?;
        }
    }
    set_owner_writable(path, &metadata, false)
}

/// Make a hardened tree removable without following any symlink it contains.
fn make_tree_writable(path: &Path) -> Result<(), SnapshotError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(io_error(
                &format!("reading metadata for {}", path.display()),
                error,
            ));
        }
    };
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    // Directories must be searchable before their children can be visited.
    set_owner_writable(path, &metadata, true)?;
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)
            .map_err(|error| io_error(&format!("reading directory {}", path.display()), error))?
        {
            let child = entry
                .map_err(|error| {
                    io_error(&format!("reading entry below {}", path.display()), error)
                })?
                .path();
            make_tree_writable(&child)?;
        }
    }
    Ok(())
}

fn set_path_owner_writable(path: &Path, writable: bool) -> Result<(), SnapshotError> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| io_error(&format!("reading metadata for {}", path.display()), error))?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    set_owner_writable(path, &metadata, writable)
}

#[cfg(unix)]
fn set_owner_writable(
    path: &Path,
    metadata: &std::fs::Metadata,
    writable: bool,
) -> Result<(), SnapshotError> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = metadata.permissions();
    let mode = permissions.mode();
    permissions.set_mode(if writable {
        mode | 0o200
    } else {
        mode & !0o222
    });
    std::fs::set_permissions(path, permissions).map_err(|error| {
        io_error(
            &format!("changing permissions for {}", path.display()),
            error,
        )
    })
}

#[cfg(not(unix))]
fn set_owner_writable(
    path: &Path,
    metadata: &std::fs::Metadata,
    writable: bool,
) -> Result<(), SnapshotError> {
    let mut permissions = metadata.permissions();
    permissions.set_readonly(!writable);
    std::fs::set_permissions(path, permissions).map_err(|error| {
        io_error(
            &format!("changing permissions for {}", path.display()),
            error,
        )
    })
}

fn io_error(action: &str, error: std::io::Error) -> SnapshotError {
    SnapshotError::Io(format!("{action}: {error}"))
}

fn validate_snapshot_id(value: &str) -> Result<(), SnapshotError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(SnapshotError::InvalidSnapshotId(value.to_string()))
    }
}

/// The files `merge_base..head` changed, renames found, with each side's
/// blob and whether git calls the file binary, read in `repo`.
async fn range_files(
    git: &Git,
    repo: &Path,
    merge_base: &str,
    head: &str,
    timeout: Duration,
) -> Result<Vec<SnapshotFile>, SnapshotError> {
    let raw = git
        .run_bytes(
            Some(repo),
            [
                "diff",
                "--raw",
                "--no-abbrev",
                "-z",
                "--find-renames",
                merge_base,
                head,
                "--",
            ],
            timeout,
        )
        .await?;
    let numstat = git
        .run_bytes(
            Some(repo),
            [
                "diff",
                "--numstat",
                "-z",
                "--find-renames",
                merge_base,
                head,
                "--",
            ],
            timeout,
        )
        .await?;
    parse_snapshot_files(&raw, &numstat)
}

/// The lines on `side` of a changed file that its hunks show, one range
/// to a hunk: the changed lines and three either side, as a unified diff
/// draws them. An added file is all one hunk on the head, as a deleted
/// file is on the base. Empty for a file with nothing on that side, a
/// binary one, or one whose content did not change.
///
/// Git is given the context, the joining of hunks and the algorithm
/// rather than the reader's config, which can make the hunks wider than
/// GitHub's.
async fn file_hunks(
    git: &Git,
    repo: &Path,
    file: &SnapshotFile,
    side: DiffSide,
) -> Result<Vec<RangeInclusive<u32>>, SnapshotError> {
    let Some(blob) = file.blob_on(side).filter(|_| !file.binary) else {
        return Ok(Vec::new());
    };
    let other = match side {
        DiffSide::Left => file.head_blob.as_deref(),
        DiffSide::Right => file.base_blob.as_deref(),
    };
    let Some(other) = other else {
        let text = git
            .run_bytes(Some(repo), ["cat-file", "blob", blob], QUERY_TIMEOUT)
            .await?;
        let lines = text.split(|&byte| byte == b'\n').count()
            - usize::from(text.is_empty() || text.ends_with(b"\n"));
        return Ok(match u32::try_from(lines) {
            Ok(0) | Err(_) => Vec::new(),
            Ok(lines) => vec![1..=lines],
        });
    };
    let (base, head) = match side {
        DiffSide::Left => (blob, other),
        DiffSide::Right => (other, blob),
    };
    let diff = git
        .run_bytes(
            Some(repo),
            [
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--unified=3",
                "--inter-hunk-context=0",
                "--diff-algorithm=myers",
                base,
                head,
            ],
            QUERY_TIMEOUT,
        )
        .await?;
    Ok(hunk_ranges(&String::from_utf8_lossy(&diff), side))
}

/// One side of each hunk header in a unified diff, `@@ -a,b +c,d @@`, as
/// the lines it covers there: `-a,b` on the base, `+c,d` on the head. `+c`
/// alone is one line, and a hunk with no lines on a side, `+c,0`, covers
/// none. Only a header starts with `@@`: every line of a hunk's body
/// starts with a space, `+`, `-` or `\`.
fn hunk_ranges(diff: &str, side: DiffSide) -> Vec<RangeInclusive<u32>> {
    diff.lines()
        .filter_map(|line| line.strip_prefix("@@ "))
        .filter_map(|header| {
            let mut sides = header.split_whitespace();
            let lines = match side {
                DiffSide::Left => sides.next()?.strip_prefix('-')?,
                DiffSide::Right => sides.nth(1)?.strip_prefix('+')?,
            };
            let (start, count) = match lines.split_once(',') {
                Some((start, count)) => (start.parse::<u32>().ok()?, count.parse::<u32>().ok()?),
                None => (lines.parse::<u32>().ok()?, 1),
            };
            (count > 0).then(|| start..=start + (count - 1))
        })
        .collect()
}

fn validate_repository_path(value: &str) -> Result<(), SnapshotError> {
    let path = Path::new(value);
    let valid = !value.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if valid {
        Ok(())
    } else {
        Err(SnapshotError::InvalidRepositoryPath(value.to_string()))
    }
}

fn validate_manifest(manifest: &SnapshotManifest) -> Result<(), SnapshotError> {
    if manifest.diff_file != DIFF_FILE
        || manifest.retained_repo != RETAINED_REPO
        || manifest.checkout_dir != CHECKOUT_DIR
    {
        return Err(SnapshotError::Manifest(
            "snapshot storage paths do not match the supported format".into(),
        ));
    }
    for oid in [
        &manifest.range.base_tip,
        &manifest.range.merge_base,
        &manifest.range.head,
    ] {
        checked_object_id(oid)?;
    }
    for file in &manifest.files {
        validate_repository_path(&file.path)?;
        if let Some(path) = file.old_path.as_deref() {
            validate_repository_path(path)?;
        }
        for oid in [file.base_blob.as_deref(), file.head_blob.as_deref()]
            .into_iter()
            .flatten()
        {
            checked_object_id(oid)?;
        }
    }
    Ok(())
}

fn checked_object_id(value: &str) -> Result<String, SnapshotError> {
    if matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(value.to_ascii_lowercase())
    } else {
        Err(SnapshotError::InvalidObjectId(value.to_string()))
    }
}

fn parse_snapshot_files(raw: &[u8], numstat: &[u8]) -> Result<Vec<SnapshotFile>, SnapshotError> {
    let raw = std::str::from_utf8(raw)
        .map_err(|_| SnapshotError::Manifest("non-UTF-8 path in snapshot diff".into()))?;
    let numstat = std::str::from_utf8(numstat)
        .map_err(|_| SnapshotError::Manifest("non-UTF-8 path in snapshot numstat".into()))?;
    let counts: HashMap<String, (u32, u32, bool)> = parse_numstat(numstat)
        .into_iter()
        .map(|(additions, deletions, binary, path)| (path, (additions, deletions, binary)))
        .collect();
    let mut fields = raw.split('\0').filter(|field| !field.is_empty());
    let mut files = Vec::new();
    while let Some(record) = fields.next() {
        let mut parts = record.strip_prefix(':').unwrap_or(record).split(' ');
        let (Some(base_mode), Some(head_mode), Some(base_sha), Some(head_sha), Some(code)) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err(SnapshotError::Manifest(format!(
                "malformed raw diff record {record:?}"
            )));
        };
        let status = code
            .chars()
            .next()
            .and_then(ChangeStatus::from_letter)
            .ok_or_else(|| SnapshotError::Manifest(format!("unknown diff status {code:?}")))?;
        let first = fields
            .next()
            .ok_or_else(|| SnapshotError::Manifest("raw diff path is missing".into()))?;
        validate_repository_path(first)?;
        let (path, old_path) = match status {
            ChangeStatus::Renamed | ChangeStatus::Copied => {
                let second = fields.next().ok_or_else(|| {
                    SnapshotError::Manifest("renamed raw diff path is missing".into())
                })?;
                validate_repository_path(second)?;
                (second.to_string(), Some(first.to_string()))
            }
            _ => (first.to_string(), None),
        };
        let (additions, deletions, binary) = counts.get(&path).copied().unwrap_or((0, 0, false));
        let base_blob = (!base_sha.bytes().all(|byte| byte == b'0'))
            .then(|| checked_object_id(base_sha))
            .transpose()?;
        let head_blob = (!head_sha.bytes().all(|byte| byte == b'0'))
            .then(|| checked_object_id(head_sha))
            .transpose()?;
        files.push(SnapshotFile {
            path,
            old_path,
            status,
            base_blob,
            head_blob,
            base_mode: base_mode.to_string(),
            head_mode: head_mode.to_string(),
            additions,
            deletions,
            binary,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiffSide {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotAnchor {
    pub path: String,
    pub side: DiffSide,
    /// Inclusive, one-based line range.
    pub start_line: u32,
    pub end_line: u32,
    pub blob: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingDocument {
    pub path: String,
    /// Path for the same diff entry on the opposite side, when present.
    pub counterpart_path: Option<String>,
    pub side: DiffSide,
    pub blob: String,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingSnapshot {
    pub snapshot_id: String,
    pub documents: Vec<MappingDocument>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MappingMethod {
    ExactBlob,
    RenameBlob,
    UniqueContext,
    Diff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MappingConfidence {
    Exact,
    High,
    Medium,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappingEvidence {
    pub summary: String,
    pub source_blob: String,
    pub target_blob: Option<String>,
    pub source_range: (u32, u32),
    pub target_range: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappedAnchor {
    pub anchor: SnapshotAnchor,
    pub method: MappingMethod,
    pub confidence: MappingConfidence,
    pub evidence: MappingEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappingCandidate {
    pub path: String,
    pub side: DiffSide,
    pub start_line: u32,
    pub end_line: u32,
    pub blob: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnmappedReason {
    SourceMissing,
    InvalidRange,
    Binary,
    Deleted,
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnchorMapping {
    Mapped(MappedAnchor),
    Ambiguous {
        candidates: Vec<MappingCandidate>,
        evidence: MappingEvidence,
    },
    Unmapped {
        reason: UnmappedReason,
        evidence: MappingEvidence,
    },
}

/// Deterministically project one historical anchor to the equivalent side of
/// another retained snapshot. The function performs no IO and never falls
/// back to a numeric line on unrelated content.
pub fn map_anchor_between(
    anchor: &SnapshotAnchor,
    source: &MappingSnapshot,
    target: &MappingSnapshot,
) -> AnchorMapping {
    let source_document = source.documents.iter().find(|document| {
        document.side == anchor.side && document.path == anchor.path && document.blob == anchor.blob
    });
    let Some(source_document) = source_document else {
        return unmapped(
            anchor,
            UnmappedReason::SourceMissing,
            "source blob is not retained",
        );
    };
    let Some(source_text) = source_document.text.as_deref() else {
        return unmapped(
            anchor,
            UnmappedReason::Binary,
            "binary content has no line mapping",
        );
    };
    let source_lines: Vec<&str> = source_text.lines().collect();
    let start = anchor.start_line.saturating_sub(1) as usize;
    let end = anchor.end_line as usize;
    if anchor.start_line == 0 || end < start + 1 || end > source_lines.len() {
        return unmapped(
            anchor,
            UnmappedReason::InvalidRange,
            "source line range is invalid",
        );
    }

    let same_side: Vec<&MappingDocument> = target
        .documents
        .iter()
        .filter(|document| document.side == anchor.side)
        .collect();

    let exact: Vec<&MappingDocument> = same_side
        .iter()
        .copied()
        .filter(|document| document.blob == anchor.blob)
        .collect();
    if !exact.is_empty() {
        let preferred: Vec<_> = exact
            .iter()
            .copied()
            .filter(|document| {
                document.path == anchor.path
                    || document.counterpart_path.as_deref() == Some(anchor.path.as_str())
            })
            .collect();
        let chosen = match (preferred.as_slice(), exact.as_slice()) {
            ([only], _) => Some(*only),
            ([], [only]) => Some(*only),
            _ => None,
        };
        if let Some(document) = chosen {
            let renamed = document.path != anchor.path;
            return mapped(
                anchor,
                document,
                anchor.start_line,
                anchor.end_line,
                if renamed {
                    MappingMethod::RenameBlob
                } else {
                    MappingMethod::ExactBlob
                },
                MappingConfidence::Exact,
                if renamed {
                    "identical retained blob at the renamed path"
                } else {
                    "identical retained blob at the same path"
                },
            );
        }
        return ambiguous(
            anchor,
            exact
                .into_iter()
                .map(|document| candidate(document, anchor.start_line, anchor.end_line))
                .collect(),
            "the same blob occurs at more than one target path",
        );
    }

    let context_start = start.saturating_sub(2);
    let context_end = (end + 2).min(source_lines.len());
    let before = start - context_start;
    let context = &source_lines[context_start..context_end];
    let context_hits = block_hits(&same_side, context, before, end - start);
    match context_hits.as_slice() {
        [(document, mapped_start, mapped_end)] => {
            return mapped(
                anchor,
                document,
                *mapped_start,
                *mapped_end,
                MappingMethod::UniqueContext,
                MappingConfidence::High,
                "the selected lines and surrounding context occur once",
            );
        }
        hits if hits.len() > 1 => {
            return ambiguous(
                anchor,
                hits.iter()
                    .map(|(document, start, end)| candidate(document, *start, *end))
                    .collect(),
                "the surrounding context occurs more than once",
            );
        }
        _ => {}
    }

    let selected = &source_lines[start..end];
    let selected_is_distinctive =
        selected.len() > 1 || selected.iter().map(|line| line.trim().len()).sum::<usize>() >= 24;
    if selected_is_distinctive {
        let selected_hits = block_hits(&same_side, selected, 0, selected.len());
        match selected_hits.as_slice() {
            [(document, mapped_start, mapped_end)] => {
                return mapped(
                    anchor,
                    document,
                    *mapped_start,
                    *mapped_end,
                    MappingMethod::UniqueContext,
                    MappingConfidence::High,
                    "the selected text occurs once in the target side",
                );
            }
            hits if hits.len() > 1 => {
                return ambiguous(
                    anchor,
                    hits.iter()
                        .map(|(document, start, end)| candidate(document, *start, *end))
                        .collect(),
                    "the selected text occurs more than once",
                );
            }
            _ => {}
        }
    }

    let related: Vec<&MappingDocument> = same_side
        .iter()
        .copied()
        .filter(|document| {
            document.path == anchor.path
                || document.counterpart_path.as_deref() == Some(anchor.path.as_str())
                || source_document.counterpart_path.as_deref() == Some(document.path.as_str())
        })
        .collect();
    if let [document] = related.as_slice() {
        if let Some(target_text) = document.text.as_deref() {
            if let Some((mapped_start, mapped_end)) =
                diff_map_range(source_text, target_text, anchor.start_line, anchor.end_line)
            {
                return mapped(
                    anchor,
                    document,
                    mapped_start,
                    mapped_end,
                    MappingMethod::Diff,
                    MappingConfidence::Medium,
                    "histogram diff preserves every selected line contiguously",
                );
            }
        }
    }

    let deleted = target.documents.iter().any(|document| {
        document.side != anchor.side
            && (document.path == anchor.path
                || document.counterpart_path.as_deref() == Some(anchor.path.as_str())
                || source_document.counterpart_path.as_deref() == Some(document.path.as_str()))
    });
    unmapped(
        anchor,
        if deleted {
            UnmappedReason::Deleted
        } else {
            UnmappedReason::Changed
        },
        if deleted {
            "the path has no blob on the requested target side"
        } else {
            "no unique unchanged context maps to the target side"
        },
    )
}

fn block_hits<'a>(
    documents: &'a [&'a MappingDocument],
    needle: &[&str],
    selected_offset: usize,
    selected_len: usize,
) -> Vec<(&'a MappingDocument, u32, u32)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for document in documents {
        let Some(text) = document.text.as_deref() else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() < needle.len() {
            continue;
        }
        for index in 0..=lines.len() - needle.len() {
            if lines[index..index + needle.len()] == needle[..] {
                let start = index + selected_offset + 1;
                let end = start + selected_len.saturating_sub(1);
                hits.push((*document, start as u32, end as u32));
            }
        }
    }
    hits
}

fn diff_map_range(
    source: &str,
    target: &str,
    start_line: u32,
    end_line: u32,
) -> Option<(u32, u32)> {
    let source_lines: Vec<&str> = source.lines().collect();
    let target_lines: Vec<&str> = target.lines().collect();
    let rows = alignment(&file_diff(source, target), source, target);
    let mut mapped = Vec::new();
    for source_line in start_line..=end_line {
        let target_line = rows.iter().find_map(|(left, right)| {
            (*left == Some(source_line as usize))
                .then_some(*right)
                .flatten()
        })?;
        if source_lines.get(source_line as usize - 1)? != target_lines.get(target_line - 1)? {
            return None;
        }
        mapped.push(target_line as u32);
    }
    let first = *mapped.first()?;
    let last = *mapped.last()?;
    (last - first + 1 == mapped.len() as u32).then_some((first, last))
}

fn candidate(document: &MappingDocument, start: u32, end: u32) -> MappingCandidate {
    MappingCandidate {
        path: document.path.clone(),
        side: document.side,
        start_line: start,
        end_line: end,
        blob: document.blob.clone(),
    }
}

fn evidence(
    anchor: &SnapshotAnchor,
    target_blob: Option<String>,
    target_range: Option<(u32, u32)>,
    summary: impl Into<String>,
) -> MappingEvidence {
    MappingEvidence {
        summary: summary.into(),
        source_blob: anchor.blob.clone(),
        target_blob,
        source_range: (anchor.start_line, anchor.end_line),
        target_range,
    }
}

fn mapped(
    source: &SnapshotAnchor,
    document: &MappingDocument,
    start_line: u32,
    end_line: u32,
    method: MappingMethod,
    confidence: MappingConfidence,
    summary: &str,
) -> AnchorMapping {
    AnchorMapping::Mapped(MappedAnchor {
        anchor: SnapshotAnchor {
            path: document.path.clone(),
            side: document.side,
            start_line,
            end_line,
            blob: document.blob.clone(),
        },
        method,
        confidence,
        evidence: evidence(
            source,
            Some(document.blob.clone()),
            Some((start_line, end_line)),
            summary,
        ),
    })
}

fn ambiguous(
    source: &SnapshotAnchor,
    candidates: Vec<MappingCandidate>,
    summary: &str,
) -> AnchorMapping {
    AnchorMapping::Ambiguous {
        candidates,
        evidence: evidence(source, None, None, summary),
    }
}

fn unmapped(source: &SnapshotAnchor, reason: UnmappedReason, summary: &str) -> AnchorMapping {
    AnchorMapping::Unmapped {
        reason,
        evidence: evidence(source, None, None, summary),
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    fn git(cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn commit_all(repo: &Path, message: &str) -> String {
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-q", "-m", message]);
        git(repo, &["rev-parse", "HEAD"])
    }

    fn repository() -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        git(repo.path(), &["config", "user.name", "Perch Test"]);
        git(repo.path(), &["config", "user.email", "perch@example.com"]);
        repo
    }

    fn request(id: &str, repo: &Path) -> SnapshotCapture {
        SnapshotCapture {
            snapshot_id: id.into(),
            source_checkout: repo.to_path_buf(),
            source_repository: "local/test".into(),
            branch_label: "feature".into(),
            requested_base: "main".into(),
        }
    }

    fn captured(outcome: CaptureOutcome) -> ReviewSnapshot {
        match outcome {
            CaptureOutcome::Captured(snapshot) => snapshot,
            CaptureOutcome::Empty(range) => panic!("unexpected empty range: {range:?}"),
        }
    }

    fn anchor(
        snapshot: &MappingSnapshot,
        path: &str,
        side: DiffSide,
        start_line: u32,
        end_line: u32,
    ) -> SnapshotAnchor {
        let document = snapshot
            .documents
            .iter()
            .find(|document| document.path == path && document.side == side)
            .unwrap_or_else(|| panic!("missing {side:?} document {path}"));
        SnapshotAnchor {
            path: path.into(),
            side,
            start_line,
            end_line,
            blob: document.blob.clone(),
        }
    }

    #[tokio::test]
    async fn dirty_source_is_recorded_but_excluded_from_the_snapshot() {
        let repo = repository();
        std::fs::write(repo.path().join("code.rs"), "base\n").unwrap();
        commit_all(repo.path(), "base");
        git(repo.path(), &["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo.path().join("code.rs"), "base\ncommitted\n").unwrap();
        let committed_head = commit_all(repo.path(), "feature");
        std::fs::write(
            repo.path().join("code.rs"),
            "base\ncommitted\nDIRTY LOCAL EDIT\n",
        )
        .unwrap();
        std::fs::write(repo.path().join("untracked.txt"), "not reviewed\n").unwrap();

        let storage = tempfile::tempdir().unwrap();
        let store = ReviewSnapshotStore::new(storage.path());
        let snapshot = captured(store.capture(request("dirty", repo.path())).await.unwrap());

        assert!(snapshot.manifest.range.dirty);
        assert_eq!(snapshot.manifest.range.head, committed_head);
        let retained = snapshot
            .read_file(DiffSide::Right, "code.rs")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(String::from_utf8(retained).unwrap(), "base\ncommitted\n");
        let diff = String::from_utf8(snapshot.read_diff().unwrap()).unwrap();
        assert!(diff.contains("committed"));
        assert!(!diff.contains("DIRTY LOCAL EDIT"));
        assert!(!snapshot.checkout().join("untracked.txt").exists());
        assert_eq!(
            git(
                &snapshot.checkout(),
                &[
                    "--no-optional-locks",
                    "status",
                    "--porcelain",
                    "--untracked-files=normal"
                ]
            ),
            ""
        );

        let mut escaped = snapshot.manifest.clone();
        escaped.checkout_dir = "../../outside".into();
        std::fs::write(
            snapshot.root().join(MANIFEST_FILE),
            serde_json::to_vec(&escaped).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            store.open("dirty"),
            Err(SnapshotError::Manifest(_))
        ));
        assert!(store.release("dirty").unwrap());
    }

    #[tokio::test]
    async fn an_empty_committed_range_is_a_typed_non_capture() {
        let repo = repository();
        std::fs::write(repo.path().join("code.rs"), "same\n").unwrap();
        let head = commit_all(repo.path(), "base");
        let storage = tempfile::tempdir().unwrap();
        let store = ReviewSnapshotStore::new(storage.path());

        let outcome = store.capture(request("empty", repo.path())).await.unwrap();

        match outcome {
            CaptureOutcome::Empty(range) => {
                assert_eq!(range.head, head);
                assert_eq!(range.merge_base, head);
            }
            CaptureOutcome::Captured(_) => panic!("an empty range was retained"),
        }
        assert!(store.open("empty").unwrap().is_none());
    }

    #[tokio::test]
    async fn moving_head_during_and_after_capture_cannot_mix_revisions() {
        let repo = repository();
        std::fs::write(repo.path().join("code.rs"), "base\n").unwrap();
        commit_all(repo.path(), "base");
        git(repo.path(), &["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo.path().join("code.rs"), "base\nfirst head\n").unwrap();
        let resolved_head = commit_all(repo.path(), "first");
        let source = repo.path().to_path_buf();
        let storage = tempfile::tempdir().unwrap();
        let store = ReviewSnapshotStore::new(storage.path());

        let snapshot = captured(
            store
                .capture_with_resolved_hook(request("moving", repo.path()), move |range| {
                    assert_eq!(range.head, resolved_head);
                    std::fs::write(source.join("code.rs"), "base\nsecond head\n").unwrap();
                    commit_all(&source, "second");
                })
                .await
                .unwrap(),
        );
        let source_after_hook = git(repo.path(), &["rev-parse", "HEAD"]);
        assert_ne!(snapshot.manifest.range.head, source_after_hook);
        assert_eq!(
            std::fs::read_to_string(snapshot.checkout().join("code.rs")).unwrap(),
            "base\nfirst head\n"
        );

        std::fs::write(repo.path().join("code.rs"), "base\nthird head\n").unwrap();
        commit_all(repo.path(), "third");
        assert_eq!(
            std::fs::read_to_string(snapshot.checkout().join("code.rs")).unwrap(),
            "base\nfirst head\n"
        );
        assert!(!String::from_utf8(snapshot.read_diff().unwrap())
            .unwrap()
            .contains("third head"));
    }

    #[tokio::test]
    async fn retained_objects_survive_source_prune_and_removal() {
        let repo = repository();
        std::fs::write(repo.path().join("code.rs"), "base\n").unwrap();
        commit_all(repo.path(), "base");
        git(repo.path(), &["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo.path().join("code.rs"), "base\nretained\n").unwrap();
        commit_all(repo.path(), "feature");
        let storage = tempfile::tempdir().unwrap();
        let store = ReviewSnapshotStore::new(storage.path());
        let snapshot = captured(
            store
                .capture(request("retained", repo.path()))
                .await
                .unwrap(),
        );
        store.cleanup_checkout("retained").unwrap();

        git(repo.path(), &["checkout", "-q", "main"]);
        git(repo.path(), &["branch", "-D", "feature"]);
        git(repo.path(), &["reflog", "expire", "--expire=now", "--all"]);
        git(repo.path(), &["gc", "--prune=now"]);
        std::fs::remove_dir_all(repo.path()).unwrap();

        assert!(String::from_utf8(snapshot.read_diff().unwrap())
            .unwrap()
            .contains("retained"));
        assert_eq!(
            String::from_utf8(
                snapshot
                    .read_file(DiffSide::Right, "code.rs")
                    .await
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            "base\nretained\n"
        );
        let checkout = store.ensure_checkout("retained").await.unwrap();
        assert_eq!(
            std::fs::read_to_string(checkout.join("code.rs")).unwrap(),
            "base\nretained\n"
        );
    }

    #[test]
    fn a_hunk_header_gives_the_lines_it_covers_on_either_side() {
        let diff = "diff --git a/x b/x\n\
                    index 1111111..2222222 100644\n\
                    --- a/x\n\
                    +++ b/x\n\
                    @@ -7,7 +7,8 @@ fn a()\n line\n-old\n+new\n+more\n\
                    @@ -30 +31 @@\n-a\n+b\n\
                    @@ -40,2 +42,0 @@\n-gone\n-too\n\
                    @@ -50,0 +51,2 @@\n+new\n+too\n";
        assert_eq!(
            hunk_ranges(diff, DiffSide::Right),
            vec![7..=14, 31..=31, 51..=52]
        );
        assert_eq!(
            hunk_ranges(diff, DiffSide::Left),
            vec![7..=13, 30..=30, 40..=41]
        );
    }

    #[tokio::test]
    async fn head_hunks_are_the_changed_lines_and_three_either_side() {
        let repo = repository();
        let base: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        std::fs::write(repo.path().join("code.rs"), &base).unwrap();
        std::fs::write(repo.path().join("same.rs"), "unchanged\n").unwrap();
        commit_all(repo.path(), "base");
        git(repo.path(), &["checkout", "-q", "-b", "feature"]);
        let head = base
            .replace("line 10\n", "line ten\n")
            .replace("line 25\n", "line twenty-five\n");
        std::fs::write(repo.path().join("code.rs"), head).unwrap();
        std::fs::write(repo.path().join("added.rs"), "one\ntwo\nthree").unwrap();
        git(repo.path(), &["mv", "same.rs", "moved.rs"]);
        commit_all(repo.path(), "feature");

        let storage = tempfile::tempdir().unwrap();
        let store = ReviewSnapshotStore::new(storage.path());
        let snapshot = captured(store.capture(request("hunks", repo.path())).await.unwrap());

        assert_eq!(
            snapshot.head_hunks("code.rs").await.unwrap(),
            vec![7..=13, 22..=28]
        );
        assert_eq!(snapshot.head_hunks("added.rs").await.unwrap(), vec![1..=3]);
        assert!(snapshot.head_hunks("moved.rs").await.unwrap().is_empty());
        assert!(snapshot.head_hunks("same.rs").await.unwrap().is_empty());
        assert!(snapshot
            .head_hunks("elsewhere.rs")
            .await
            .unwrap()
            .is_empty());
    }

    /// A pull request's diff read from its clone, with no snapshot taken:
    /// each side's hunks, the file known by its name on the head.
    #[tokio::test]
    async fn a_range_read_from_a_clone_gives_either_sides_hunks() {
        let repo = repository();
        let base: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        let old: String = (1..=30).map(|n| format!("old {n}\n")).collect();
        std::fs::write(repo.path().join("code.rs"), &base).unwrap();
        std::fs::write(repo.path().join("old.rs"), &old).unwrap();
        std::fs::write(repo.path().join("gone.rs"), "gone\nfor good\n").unwrap();
        std::fs::write(repo.path().join("same.rs"), "unchanged\n").unwrap();
        let merge_base = commit_all(repo.path(), "base");
        git(repo.path(), &["checkout", "-q", "-b", "feature"]);
        // Two lines become one, so the sides' numbers part after it.
        let code = base
            .replace("line 10\n", "line ten\n")
            .replace("line 20\nline 21\n", "line twenty\n");
        std::fs::write(repo.path().join("code.rs"), code).unwrap();
        git(repo.path(), &["mv", "old.rs", "moved.rs"]);
        std::fs::write(
            repo.path().join("moved.rs"),
            old.replace("old 15\n", "old fifteen\n"),
        )
        .unwrap();
        std::fs::remove_file(repo.path().join("gone.rs")).unwrap();
        std::fs::write(repo.path().join("added.rs"), "one\ntwo\nthree").unwrap();
        std::fs::write(repo.path().join("image.bin"), [0u8, 1, 2, 0]).unwrap();
        let head = commit_all(repo.path(), "feature");

        let diff = RangeDiff::read(repo.path(), &merge_base, &head)
            .await
            .unwrap();
        let (left, right) = (DiffSide::Left, DiffSide::Right);
        assert_eq!(
            diff.hunks("code.rs", right).await.unwrap(),
            vec![7..=13, 17..=23]
        );
        assert_eq!(
            diff.hunks("code.rs", left).await.unwrap(),
            vec![7..=13, 17..=24]
        );
        assert_eq!(diff.hunks("moved.rs", right).await.unwrap(), vec![12..=18]);
        assert_eq!(diff.hunks("moved.rs", left).await.unwrap(), vec![12..=18]);
        assert!(
            diff.hunks("old.rs", left).await.unwrap().is_empty(),
            "a renamed file goes by its new name"
        );
        assert_eq!(diff.hunks("added.rs", right).await.unwrap(), vec![1..=3]);
        assert!(diff.hunks("added.rs", left).await.unwrap().is_empty());
        assert_eq!(diff.hunks("gone.rs", left).await.unwrap(), vec![1..=2]);
        assert!(diff.hunks("gone.rs", right).await.unwrap().is_empty());
        assert!(diff.hunks("image.bin", right).await.unwrap().is_empty());
        assert!(diff.hunks("same.rs", right).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cleanup_and_release_target_only_one_snapshot() {
        let repo = repository();
        std::fs::write(repo.path().join("code.rs"), "base\n").unwrap();
        commit_all(repo.path(), "base");
        git(repo.path(), &["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo.path().join("code.rs"), "base\nfeature\n").unwrap();
        commit_all(repo.path(), "feature");
        let storage = tempfile::tempdir().unwrap();
        let store = ReviewSnapshotStore::new(storage.path());
        let first = captured(store.capture(request("one", repo.path())).await.unwrap());
        let second = captured(store.capture(request("two", repo.path())).await.unwrap());

        #[cfg(unix)]
        assert_tree_readonly(&first.checkout());

        assert!(store.cleanup_checkout("one").unwrap());
        assert!(!first.checkout().exists());
        assert!(first.retained_repo().exists());
        assert!(first.diff_path().exists());
        assert!(second.checkout().exists());
        let recreated = store.ensure_checkout("one").await.unwrap();
        assert!(recreated.exists());
        #[cfg(unix)]
        assert_tree_readonly(&recreated);

        assert!(store.release("one").unwrap());
        assert!(store.open("one").unwrap().is_none());
        assert!(store.open("two").unwrap().is_some());
        assert!(second.read_diff().is_ok());
        assert!(!store.release("one").unwrap());
    }

    #[tokio::test]
    async fn rename_insert_and_file_deletion_map_conservatively() {
        let repo = repository();
        std::fs::write(
            repo.path().join("old.rs"),
            "alpha\na distinctive review target line\nomega\n",
        )
        .unwrap();
        commit_all(repo.path(), "base");
        git(repo.path(), &["checkout", "-q", "-b", "feature"]);
        std::fs::write(
            repo.path().join("old.rs"),
            "alpha\na distinctive review target line\nomega changed\n",
        )
        .unwrap();
        commit_all(repo.path(), "feature");
        let storage = tempfile::tempdir().unwrap();
        let store = ReviewSnapshotStore::new(storage.path());
        let first = captured(
            store
                .capture(request("map-one", repo.path()))
                .await
                .unwrap(),
        );

        git(repo.path(), &["mv", "old.rs", "new.rs"]);
        commit_all(repo.path(), "rename");
        let renamed = captured(
            store
                .capture(request("map-two", repo.path()))
                .await
                .unwrap(),
        );
        let first_map = first.mapping_snapshot().await.unwrap();
        let renamed_map = renamed.mapping_snapshot().await.unwrap();
        let original = anchor(&first_map, "old.rs", DiffSide::Right, 2, 2);
        match map_anchor_between(&original, &first_map, &renamed_map) {
            AnchorMapping::Mapped(mapped) => {
                assert_eq!(mapped.anchor.path, "new.rs");
                assert_eq!(mapped.anchor.start_line, 2);
                assert_eq!(mapped.method, MappingMethod::RenameBlob);
                assert_eq!(mapped.confidence, MappingConfidence::Exact);
            }
            other => panic!("rename did not map: {other:?}"),
        }
        let invalid = anchor(&first_map, "old.rs", DiffSide::Right, 99, 99);
        assert!(matches!(
            map_anchor_between(&invalid, &first_map, &renamed_map),
            AnchorMapping::Unmapped {
                reason: UnmappedReason::InvalidRange,
                ..
            }
        ));

        std::fs::write(
            repo.path().join("new.rs"),
            "inserted first\nalpha\na distinctive review target line\nomega changed\n",
        )
        .unwrap();
        commit_all(repo.path(), "insert");
        let inserted = captured(
            store
                .capture(request("map-three", repo.path()))
                .await
                .unwrap(),
        );
        let inserted_map = inserted.mapping_snapshot().await.unwrap();
        let renamed_anchor = anchor(&renamed_map, "new.rs", DiffSide::Right, 2, 2);
        match map_anchor_between(&renamed_anchor, &renamed_map, &inserted_map) {
            AnchorMapping::Mapped(mapped) => {
                assert_eq!(mapped.anchor.start_line, 3);
                assert_eq!(mapped.method, MappingMethod::UniqueContext);
            }
            other => panic!("insert did not map: {other:?}"),
        }

        git(repo.path(), &["rm", "-q", "new.rs"]);
        commit_all(repo.path(), "delete");
        let deleted = captured(
            store
                .capture(request("map-four", repo.path()))
                .await
                .unwrap(),
        );
        let deleted_map = deleted.mapping_snapshot().await.unwrap();
        let inserted_anchor = anchor(&inserted_map, "new.rs", DiffSide::Right, 3, 3);
        assert!(matches!(
            map_anchor_between(&inserted_anchor, &inserted_map, &deleted_map),
            AnchorMapping::Unmapped {
                reason: UnmappedReason::Deleted,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn left_side_anchor_maps_across_a_rebased_base() {
        let repo = repository();
        std::fs::write(
            repo.path().join("code.rs"),
            "one\na stable base-side anchor line\nthree\n",
        )
        .unwrap();
        commit_all(repo.path(), "base one");
        git(repo.path(), &["checkout", "-q", "-b", "feature"]);
        std::fs::write(
            repo.path().join("code.rs"),
            "one\na stable base-side anchor line\nthree feature\n",
        )
        .unwrap();
        commit_all(repo.path(), "feature one");
        let storage = tempfile::tempdir().unwrap();
        let store = ReviewSnapshotStore::new(storage.path());
        let first = captured(
            store
                .capture(request("left-one", repo.path()))
                .await
                .unwrap(),
        );

        git(repo.path(), &["checkout", "-q", "main"]);
        std::fs::write(
            repo.path().join("code.rs"),
            "inserted on base\none\na stable base-side anchor line\nthree\n",
        )
        .unwrap();
        commit_all(repo.path(), "base two");
        git(repo.path(), &["checkout", "-q", "feature"]);
        git(repo.path(), &["rebase", "main"]);
        let second = captured(
            store
                .capture(request("left-two", repo.path()))
                .await
                .unwrap(),
        );

        let first_map = first.mapping_snapshot().await.unwrap();
        let second_map = second.mapping_snapshot().await.unwrap();
        let old_base = anchor(&first_map, "code.rs", DiffSide::Left, 2, 2);
        match map_anchor_between(&old_base, &first_map, &second_map) {
            AnchorMapping::Mapped(mapped) => {
                assert_eq!(mapped.anchor.side, DiffSide::Left);
                assert_eq!(mapped.anchor.start_line, 3);
                assert_eq!(mapped.method, MappingMethod::UniqueContext);
            }
            other => panic!("base-side line did not map: {other:?}"),
        }
    }

    #[tokio::test]
    async fn duplicate_context_is_ambiguous_instead_of_guessing() {
        let repo = repository();
        std::fs::write(repo.path().join("src.rs"), "base\n").unwrap();
        commit_all(repo.path(), "base");
        git(repo.path(), &["checkout", "-q", "-b", "feature"]);
        let repeated = "a very distinctive duplicated vulnerable call sequence";
        std::fs::write(
            repo.path().join("src.rs"),
            format!("source prefix\n{repeated}\nsource suffix\n"),
        )
        .unwrap();
        commit_all(repo.path(), "source finding");
        let storage = tempfile::tempdir().unwrap();
        let store = ReviewSnapshotStore::new(storage.path());
        let source_snapshot = captured(
            store
                .capture(request("ambiguous-one", repo.path()))
                .await
                .unwrap(),
        );

        git(repo.path(), &["rm", "-q", "src.rs"]);
        std::fs::write(
            repo.path().join("a.rs"),
            format!("a prefix\n{repeated}\na suffix\n"),
        )
        .unwrap();
        std::fs::write(
            repo.path().join("b.rs"),
            format!("b prefix\n{repeated}\nb suffix\n"),
        )
        .unwrap();
        commit_all(repo.path(), "two candidates");
        let target_snapshot = captured(
            store
                .capture(request("ambiguous-two", repo.path()))
                .await
                .unwrap(),
        );

        let source_map = source_snapshot.mapping_snapshot().await.unwrap();
        let target_map = target_snapshot.mapping_snapshot().await.unwrap();
        let original = anchor(&source_map, "src.rs", DiffSide::Right, 2, 2);
        match map_anchor_between(&original, &source_map, &target_map) {
            AnchorMapping::Ambiguous { candidates, .. } => {
                assert_eq!(candidates.len(), 2);
                assert_eq!(
                    candidates
                        .iter()
                        .map(|candidate| candidate.path.as_str())
                        .collect::<HashSet<_>>(),
                    HashSet::from(["a.rs", "b.rs"])
                );
            }
            other => panic!("duplicate context was guessed: {other:?}"),
        }
    }

    #[test]
    fn unsafe_ids_and_interrupted_staging_cannot_escape_or_leak() {
        let storage = tempfile::tempdir().unwrap();
        let store = ReviewSnapshotStore::new(storage.path());
        assert!(matches!(
            store.release("../../outside"),
            Err(SnapshotError::InvalidSnapshotId(_))
        ));

        let partial = storage.path().join("interrupted.partial");
        std::fs::create_dir(&partial).unwrap();
        std::fs::write(partial.join("partial.pack"), "partial").unwrap();
        {
            let _cleanup = StagingCleanup::new(partial.clone());
            make_tree_readonly(&partial).unwrap();
        }
        assert!(!partial.exists());
    }

    #[tokio::test]
    async fn concurrent_capture_publishes_once_without_partial_directories() {
        let repo = repository();
        std::fs::write(repo.path().join("code.rs"), "base\n").unwrap();
        commit_all(repo.path(), "base");
        git(repo.path(), &["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo.path().join("code.rs"), "base\nfeature\n").unwrap();
        commit_all(repo.path(), "feature");
        let storage = tempfile::tempdir().unwrap();
        let store = ReviewSnapshotStore::new(storage.path());

        let (left, right) = tokio::join!(
            store.capture(request("racing", repo.path())),
            store.capture(request("racing", repo.path()))
        );
        let outcomes = [left, right];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, Ok(CaptureOutcome::Captured(_))))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, Err(SnapshotError::AlreadyExists(_))))
                .count(),
            1
        );
        assert!(std::fs::read_dir(store.root()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".partial-")
        }));
    }

    #[tokio::test]
    async fn startup_scavenge_removes_only_staging_and_unadmitted_snapshots() {
        let repo = repository();
        std::fs::write(repo.path().join("code.rs"), "base\n").unwrap();
        commit_all(repo.path(), "base");
        git(repo.path(), &["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo.path().join("code.rs"), "base\nfeature\n").unwrap();
        commit_all(repo.path(), "feature");
        let storage = tempfile::tempdir().unwrap();
        let store = ReviewSnapshotStore::new(storage.path());
        captured(store.capture(request("kept", repo.path())).await.unwrap());
        captured(store.capture(request("orphan", repo.path())).await.unwrap());
        let partial = store.root().join(".crashed.partial-123");
        std::fs::create_dir_all(&partial).unwrap();
        std::fs::write(partial.join("incomplete"), b"data").unwrap();
        let unknown = store.root().join("unknown");
        std::fs::create_dir_all(&unknown).unwrap();

        assert_eq!(store.scavenge(&HashSet::from(["kept".into()])).unwrap(), 2);
        assert!(store.open("kept").unwrap().is_some());
        assert!(store.open("orphan").unwrap().is_none());
        assert!(!partial.exists());
        assert!(
            unknown.exists(),
            "malformed storage is retained for diagnosis"
        );
    }

    #[cfg(unix)]
    fn assert_tree_readonly(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let metadata = std::fs::symlink_metadata(path).unwrap();
        if metadata.file_type().is_symlink() {
            return;
        }
        assert_eq!(
            metadata.permissions().mode() & 0o222,
            0,
            "{} remained writable",
            path.display()
        );
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                assert_tree_readonly(&entry.unwrap().path());
            }
        }
    }
}
