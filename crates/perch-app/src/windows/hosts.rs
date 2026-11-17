//! The SSH hosts this machine's windows have connected to, in the order
//! they first did, for the environment menu at the drawer's foot. They
//! are the client's, like the window session: kept in this machine's
//! Perch home, whichever engine a window uses.
use std::cell::RefCell;
use std::path::{Path, PathBuf};

pub(super) struct Hosts {
    path: PathBuf,
    known: RefCell<Vec<String>>,
}

impl Hosts {
    pub fn load(home: &Path) -> Self {
        let path = home.join("hosts.json");
        let known = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::warn!(%error, "could not read the hosts connected to");
                Vec::new()
            }),
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(%error, "could not read the hosts connected to");
                }
                Vec::new()
            }
        };
        Self {
            path,
            known: RefCell::new(known),
        }
    }

    pub fn list(&self) -> Vec<String> {
        self.known.borrow().clone()
    }

    /// A window connected to `host`: keep it, after those already kept.
    pub fn remember(&self, host: &str) {
        if self.known.borrow().iter().any(|known| known == host) {
            return;
        }
        self.known.borrow_mut().push(host.to_owned());
        self.save();
    }

    /// The reader took `host` out of the menu.
    pub fn forget(&self, host: &str) {
        let before = self.known.borrow().len();
        self.known.borrow_mut().retain(|known| known != host);
        if self.known.borrow().len() != before {
            self.save();
        }
    }

    fn save(&self) {
        if let Err(error) = self.write() {
            tracing::warn!(%error, "could not keep the hosts connected to");
        }
    }

    fn write(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.path.parent().expect("hosts file in Perch home"))?;
        let partial = self
            .path
            .with_extension(format!("{}.partial", std::process::id()));
        std::fs::write(&partial, serde_json::to_vec(&*self.known.borrow())?)?;
        std::fs::rename(partial, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_kept_once_in_the_order_they_connected_until_forgotten() {
        let home = tempfile::tempdir().unwrap();
        let hosts = Hosts::load(home.path());
        assert!(hosts.list().is_empty());
        hosts.remember("chris@devbox:2222");
        hosts.remember("halo");
        hosts.remember("chris@devbox:2222");
        assert_eq!(hosts.list(), ["chris@devbox:2222", "halo"]);
        assert_eq!(
            Hosts::load(home.path()).list(),
            ["chris@devbox:2222", "halo"]
        );
        hosts.forget("chris@devbox:2222");
        hosts.forget("never-seen");
        assert_eq!(Hosts::load(home.path()).list(), ["halo"]);
    }
}
