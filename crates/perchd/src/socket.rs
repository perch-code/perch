//! Where the engine listens: `$PERCH_HOME/run/perchd.sock`, in a directory
//! only its owner can enter, and itself only its owner's to connect to.

use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

/// The socket of the engine on this Perch home.
pub fn path() -> PathBuf {
    perch_github::config::perch_home()
        .join("run")
        .join("perchd.sock")
}

/// Make the socket's directory, owner-only, and clear the way for a new
/// socket. A socket file nothing answers on is left by an engine that was
/// killed, and goes; one something answers on is another engine's, and
/// stays, with the reason.
pub fn prepare(socket: &Path) -> Result<(), String> {
    let dir = socket.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("could not make {} private: {e}", dir.display()))?;
    let Ok(found) = std::fs::symlink_metadata(socket) else {
        return Ok(());
    };
    if !found.file_type().is_socket() {
        return Err(format!(
            "{} is in the way, and is not a socket",
            socket.display()
        ));
    }
    if UnixStream::connect(socket).is_ok() {
        return Err(format!("an engine already answers on {}", socket.display()));
    }
    tracing::info!(socket = %socket.display(), "removing a socket nothing answers on");
    std::fs::remove_file(socket)
        .map_err(|e| format!("could not remove the stale {}: {e}", socket.display()))
}

/// Listen on `socket`, which only its owner may connect to. The directory
/// is owner-only already, so the moment before the mode is set is no gap.
pub fn bind(socket: &Path) -> Result<std::os::unix::net::UnixListener, String> {
    let listener = std::os::unix::net::UnixListener::bind(socket)
        .map_err(|e| format!("could not listen on {}: {e}", socket.display()))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("could not make {} private: {e}", socket.display()))?;
    Ok(listener)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn short_dir() -> tempfile::TempDir {
        // A socket's path has to fit in about a hundred bytes.
        tempfile::Builder::new().tempdir_in("/tmp").unwrap()
    }

    #[test]
    fn a_socket_nothing_answers_on_is_cleared_and_a_live_one_is_not() {
        let dir = short_dir();
        let socket = dir.path().join("run/perchd.sock");
        prepare(&socket).unwrap();
        let mode = std::fs::metadata(socket.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);

        let listener = bind(&socket).unwrap();
        let mode = std::fs::symlink_metadata(&socket)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let said = prepare(&socket).unwrap_err();
        assert!(said.contains("already answers"), "{said}");

        // Gone without removing its file, as a killed engine goes.
        drop(listener);
        assert!(socket.exists());
        prepare(&socket).unwrap();
        assert!(!socket.exists());
    }

    #[test]
    fn a_file_that_is_not_a_socket_is_left_alone() {
        let dir = short_dir();
        let socket = dir.path().join("run/perchd.sock");
        std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
        std::fs::write(&socket, "mine").unwrap();
        assert!(prepare(&socket).unwrap_err().contains("not a socket"));
        assert_eq!(std::fs::read_to_string(&socket).unwrap(), "mine");
    }
}
