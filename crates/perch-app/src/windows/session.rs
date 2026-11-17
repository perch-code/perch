//! Window state on the machine running the UI. Item IDs belong to their
//! environment, so the two are saved and restored together.
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SavedWindow {
    pub host: Option<String>,
    pub item: Option<String>,
    pub frame: Option<String>,
}

pub(super) struct Session {
    path: PathBuf,
    /// In opening order, with the front window last.
    pub last: RefCell<Vec<SavedWindow>>,
    /// Opening the first window must not overwrite windows still to open.
    pub opening: Cell<usize>,
    pub quitting: Cell<bool>,
}

impl Session {
    pub fn load(home: &Path, legacy_host: Option<&str>) -> Self {
        let path = home.join("windows.json");
        let mut windows: Vec<SavedWindow> = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::warn!(%error, "could not read saved windows");
                Vec::new()
            }),
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(%error, "could not read saved windows");
                }
                Vec::new()
            }
        };
        if windows.is_empty() {
            windows.push(SavedWindow {
                host: legacy_host.map(str::to_owned),
                ..Default::default()
            });
        }
        Self {
            path,
            opening: Cell::new(windows.len()),
            last: RefCell::new(windows),
            quitting: Cell::new(false),
        }
    }

    pub fn save(&self, windows: Vec<SavedWindow>) {
        // When the last window closes, keep it for the next launch.
        if self.opening.get() > 0 || self.quitting.get() || windows.is_empty() {
            return;
        }
        if self.last.borrow().as_slice() == windows && self.path.is_file() {
            return;
        }
        if let Err(error) = self.write(&windows) {
            tracing::warn!(%error, "could not save window session");
        } else {
            self.last.replace(windows);
        }
    }

    fn write(&self, windows: &[SavedWindow]) -> std::io::Result<()> {
        std::fs::create_dir_all(self.path.parent().expect("session file in Perch home"))?;
        let partial = self
            .path
            .with_extension(format!("{}.partial", std::process::id()));
        std::fs::write(&partial, serde_json::to_vec(windows)?)?;
        std::fs::rename(partial, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopening_restores_each_window_and_uses_the_old_host_only_on_first_launch() {
        let home = tempfile::tempdir().unwrap();
        let load = || Session::load(home.path(), Some("legacy"));
        let session = load();
        assert_eq!(session.last.borrow()[0].host.as_deref(), Some("legacy"));
        let windows = vec![
            SavedWindow {
                item: Some("local/pr".into()),
                frame: Some("10,20,900,600".into()),
                ..Default::default()
            },
            SavedWindow {
                host: Some("user@dev:2222".into()),
                item: Some("remote/issue".into()),
                frame: None,
            },
            SavedWindow {
                host: Some("user@dev:2222".into()),
                ..Default::default()
            },
        ];
        session.opening.set(0);
        session.save(windows.clone());
        let next = load();
        assert_eq!(next.last.borrow().as_slice(), windows);
        next.save(windows[..1].to_vec());
        assert_eq!(
            load().last.borrow().as_slice(),
            windows,
            "partial restoration"
        );
        next.opening.set(0);
        next.save(windows[..2].to_vec());
        assert_eq!(
            load().last.borrow().as_slice(),
            &windows[..2],
            "one window closed"
        );
        next.quitting.set(true);
        next.save(windows[..1].to_vec());
        assert_eq!(
            load().last.borrow().as_slice(),
            &windows[..2],
            "quit preserves the session"
        );
        next.quitting.set(false);
        next.save(windows[..1].to_vec());
        next.save(Vec::new());
        assert_eq!(
            load().last.borrow().as_slice(),
            &windows[..1],
            "last window closed"
        );
    }
}
