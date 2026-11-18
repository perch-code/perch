//! The SSH hosts this machine's windows have connected to, in the order
//! they first did, for the environment menu at the drawer's foot. They
//! are the client's, like the window session: kept in this machine's
//! Perch home, whichever engine a window uses.
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use std::path::{Path, PathBuf};

pub(super) struct Hosts {
    path: PathBuf,
    known: RwSignal<Vec<String>>,
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
            known: RwSignal::new(known),
        }
    }

    pub fn list(&self) -> Vec<String> {
        self.known.get_untracked()
    }

    /// The short name in the footer follows additions and removals in other windows.
    pub fn tag(&self, host: &str) -> String {
        let short = machine(host);
        self.known.with(|known| {
            if known
                .iter()
                .any(|other| other != host && machine(other) == short)
            {
                host.to_owned()
            } else {
                short
            }
        })
    }

    /// A window connected to `host`: keep it, after those already kept.
    pub fn remember(&self, host: &str) {
        if self
            .known
            .with_untracked(|known| known.iter().any(|entry| entry == host))
        {
            return;
        }
        self.known.update(|known| known.push(host.to_owned()));
        self.save();
    }

    /// The reader took `host` out of the menu.
    pub fn forget(&self, host: &str) {
        if self
            .known
            .with_untracked(|known| known.iter().any(|entry| entry == host))
        {
            self.known
                .update(|known| known.retain(|entry| entry != host));
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
        std::fs::write(&partial, self.known.with_untracked(serde_json::to_vec)?)?;
        std::fs::rename(partial, &self.path)
    }
}

pub(super) fn machine(host: &str) -> String {
    let (name, _) = perch_engine::wire::remote::host_and_port(host);
    match name.rsplit_once('@') {
        Some((_, machine)) => machine.to_owned(),
        None => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_expands_while_another_connection_uses_the_machine_name() {
        let home = tempfile::tempdir().unwrap();
        let hosts = Hosts::load(home.path());
        let first = "me@halo:2222";
        let second = "other@halo:2222";
        hosts.remember(first);
        assert_eq!(hosts.tag(first), "halo");
        hosts.remember(second);
        assert_eq!(hosts.tag(first), first);
        assert_eq!(hosts.tag(second), second);
        hosts.forget(second);
        assert_eq!(hosts.tag(first), "halo");
        assert_eq!(hosts.tag("me@[::1]:2222"), "::1");
    }

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
