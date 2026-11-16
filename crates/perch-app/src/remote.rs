//! Where the engine is: in this process, or on a host the app reaches by
//! ssh (`[remote] host`). What opens a path on this machine follows it:
//! a host's path opens in a terminal or an editor that reaches the host,
//! and `~` in a path shown is the host's home, as its engine said
//! (`Welcome.home`).

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use perch_engine::wire::remote::{bad_host, shell_quote};

/// Where the engine is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Place {
    /// In this process.
    #[default]
    ThisMac,
    /// On `host`, as ssh is given it. `home` and `data_root` are the
    /// engine's, once it has said: empty until then.
    Host {
        host: String,
        home: PathBuf,
        data_root: PathBuf,
    },
}

thread_local! {
    /// The UI thread's: set once at launch, and given the engine's home
    /// when it connects.
    static PLACE: RefCell<Place> = const { RefCell::new(Place::ThisMac) };
}

pub fn place() -> Place {
    PLACE.with_borrow(Clone::clone)
}

pub fn set_place(place: Place) {
    PLACE.set(place);
}

/// The host the engine is on, with one.
pub fn host() -> Option<String> {
    match place() {
        Place::ThisMac => None,
        Place::Host { host, .. } => Some(host),
    }
}

/// The engine said where it is: its home is what `~` means in its paths.
pub fn connected(welcome: &perch_engine::wire::Welcome) {
    PLACE.with_borrow_mut(|place| {
        if let Place::Host {
            home, data_root, ..
        } = place
        {
            *home = welcome.home.clone();
            *data_root = welcome.data_root.clone();
        }
    });
}

/// The engine's home: this machine's in process, the host's once it has
/// said, and none before.
pub fn home() -> Option<PathBuf> {
    match place() {
        Place::ThisMac => Some(perch_github::config::home_dir()),
        Place::Host { home, .. } => (!home.as_os_str().is_empty()).then_some(home),
    }
}

/// A path as the reader would type it, with the engine's home as `~`.
pub fn tilde(path: &Path) -> String {
    match home().and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// A repository's checkout as the engine's machine has it: a `~` in the
/// file is that machine's home.
pub fn root_path(repo: &perch_github::RepoConfig) -> Option<PathBuf> {
    let root = repo.root.as_deref()?;
    Some(match home() {
        Some(home) => perch_github::config::expand_home_in(root, &home),
        None => PathBuf::from(root),
    })
}

/// The line Terminal runs to open a shell in `path` on `host`: ssh with a
/// terminal, a `cd` there, and the reader's login shell. The path is quoted
/// for the host's shell, and the whole of what ssh runs there for this
/// one's. Neither the host nor the path can be read as an option.
pub fn terminal_line(host: &str, path: &Path) -> String {
    let there = format!(
        "cd -- {} && exec $SHELL -l",
        shell_quote(&path.to_string_lossy())
    );
    format!("ssh -t -- {} {}", shell_quote(host), shell_quote(&there))
}

/// How `terminal` is told to run a program and its arguments: most take
/// the rest of the line after `-e`, but gnome-terminal wants `--`, and
/// xfce4-terminal's `-e` takes one string, so `-x`.
fn run_flag(terminal: &str) -> &'static str {
    match Path::new(terminal)
        .file_name()
        .and_then(|name| name.to_str())
    {
        Some("gnome-terminal") => "--",
        Some("xfce4-terminal") => "-x",
        _ => "-e",
    }
}

/// `text` as an AppleScript string, quotes and all.
fn applescript_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// osascript's arguments to run `line` in a new Terminal window.
pub fn terminal_script(line: &str) -> Vec<String> {
    [
        "tell application \"Terminal\"".to_string(),
        "activate".to_string(),
        format!("do script {}", applescript_string(line)),
        "end tell".to_string(),
    ]
    .into_iter()
    .flat_map(|statement| ["-e".to_string(), statement])
    .collect()
}

/// The editor that opens `path` on `host`, and its arguments: Zed, given
/// its own `zed` (or the app's bundled CLI), else VS Code's Remote SSH.
pub fn editor_command(host: &str, path: &Path, zed: Option<&Path>) -> (PathBuf, Vec<String>) {
    let path = path.to_string_lossy();
    match zed {
        Some(zed) => {
            let slash = if path.starts_with('/') { "" } else { "/" };
            (
                zed.to_path_buf(),
                vec![format!("ssh://{host}{slash}{path}")],
            )
        }
        None => (
            PathBuf::from("code"),
            vec![
                "--remote".to_string(),
                format!("ssh-remote+{host}"),
                path.into_owned(),
            ],
        ),
    }
}

/// Zed's command line tool, if Zed is installed: `zed` on `PATH`, or the
/// one inside the app.
fn zed() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("zed"))
            .find(|zed| zed.is_file())
    });
    on_path.or_else(|| {
        cfg!(target_os = "macos")
            .then(|| PathBuf::from("/Applications/Zed.app/Contents/MacOS/cli"))
            .filter(|cli| cli.is_file())
    })
}

/// Open a shell in `path` on `host`, in Terminal on macOS, elsewhere in
/// `$TERMINAL` or the first terminal found. What stops it is said in the
/// windows.
pub fn open_terminal_on(host: &str, path: &Path) {
    if let Some(why) = bad_host(host) {
        crate::windows::notice(why);
        return;
    }
    let line = terminal_line(host, path);
    if cfg!(target_os = "macos") {
        if let Err(e) = std::process::Command::new("osascript")
            .args(terminal_script(&line))
            .spawn()
        {
            tracing::warn!(host, path = %path.display(), error = %e, "could not open Terminal");
            crate::windows::notice(format!("Could not open Terminal: {e}"));
        }
        return;
    }
    let chosen = std::env::var("TERMINAL")
        .ok()
        .filter(|t| !t.trim().is_empty());
    for candidate in chosen.iter().map(String::as_str).chain([
        "x-terminal-emulator",
        "gnome-terminal",
        "konsole",
        "xfce4-terminal",
        "alacritty",
        "xterm",
    ]) {
        let mut words = candidate.split_whitespace();
        let Some(program) = words.next() else {
            continue;
        };
        let spawned = std::process::Command::new(program)
            .args(words)
            .args([run_flag(program), "sh", "-c", &line])
            .spawn();
        if spawned.is_ok() {
            return;
        }
    }
    tracing::warn!(host, path = %path.display(), "no terminal found; set $TERMINAL");
    crate::windows::notice("No terminal was found to open. Set $TERMINAL to one.".into());
}

/// Open `path` on `host` in an editor that reaches it. What stops it is
/// said in the windows: an app opened from the Finder has a bare `PATH`,
/// where `code` may not be.
pub fn open_editor_on(host: &str, path: &Path) {
    if let Some(why) = bad_host(host) {
        crate::windows::notice(why);
        return;
    }
    let (program, args) = editor_command(host, path, zed().as_deref());
    if let Err(e) = std::process::Command::new(&program).args(&args).spawn() {
        tracing::warn!(
            host,
            path = %path.display(),
            editor = %program.display(),
            error = %e,
            "could not open an editor on the host"
        );
        crate::windows::notice(format!(
            "Could not open {} on {host}: neither Zed nor VS Code's `code` command was found ({e}).",
            path.display()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_on_the_host_is_opened_with_the_path_quoted_twice() {
        assert_eq!(
            terminal_line("halo", Path::new("/srv/code/perch")),
            "ssh -t -- halo 'cd -- /srv/code/perch && exec $SHELL -l'"
        );
        // A path with a space and a quote is one word in the host's shell,
        // and the whole is one word in this one's.
        let line = terminal_line("chris@halo", Path::new("/srv/it's here"));
        assert_eq!(
            line,
            r#"ssh -t -- chris@halo 'cd -- '\''/srv/it'\''\'\'''\''s here'\'' && exec $SHELL -l'"#
        );
        // What Terminal is told is AppleScript: its quotes and backslashes
        // escaped.
        let script = terminal_script(r#"ssh -t h 'cd "a\b"'"#);
        assert_eq!(script[0], "-e");
        assert_eq!(script[1], "tell application \"Terminal\"");
        assert_eq!(script[5], r#"do script "ssh -t h 'cd \"a\\b\"'""#);
        assert_eq!(script.len(), 8);
    }

    #[test]
    fn each_terminal_is_told_to_run_the_line_its_own_way() {
        assert_eq!(run_flag("gnome-terminal"), "--");
        assert_eq!(run_flag("/usr/bin/xfce4-terminal"), "-x");
        assert_eq!(run_flag("xterm"), "-e");
        assert_eq!(run_flag("x-terminal-emulator"), "-e");
    }

    #[test]
    fn a_host_path_opens_in_zed_when_it_is_there_else_in_vs_code() {
        let path = Path::new("/home/chris/Code/perch");
        assert_eq!(
            editor_command("halo", path, Some(Path::new("/usr/local/bin/zed"))),
            (
                PathBuf::from("/usr/local/bin/zed"),
                vec!["ssh://halo/home/chris/Code/perch".to_string()]
            )
        );
        assert_eq!(
            editor_command("halo", path, None),
            (
                PathBuf::from("code"),
                vec![
                    "--remote".to_string(),
                    "ssh-remote+halo".to_string(),
                    "/home/chris/Code/perch".to_string()
                ]
            )
        );
    }

    #[test]
    fn a_host_s_paths_are_shortened_with_its_home_once_it_has_said() {
        set_place(Place::Host {
            host: "halo".into(),
            home: PathBuf::new(),
            data_root: PathBuf::new(),
        });
        // Before the engine has said where its home is, nothing is taken
        // for it: least of all this machine's.
        assert_eq!(home(), None);
        assert_eq!(tilde(Path::new("/home/chris/x")), "/home/chris/x");
        connected(&perch_engine::wire::Welcome {
            protocol: perch_engine::wire::PROTOCOL,
            engine_version: "0".into(),
            connection: 1,
            host: "halo.local".into(),
            home: "/home/chris".into(),
            data_root: "/home/chris/.perch".into(),
            clients: 0,
            runs: 0,
        });
        assert_eq!(host().as_deref(), Some("halo"));
        assert_eq!(tilde(Path::new("/home/chris/Code/perch")), "~/Code/perch");
        assert_eq!(tilde(Path::new("/home/chris")), "~");
        let mut repo = perch_github::RepoConfig::default();
        repo.root = Some("~/Code/perch".into());
        assert_eq!(
            root_path(&repo),
            Some(PathBuf::from("/home/chris/Code/perch"))
        );
        set_place(Place::ThisMac);
        assert_eq!(home(), Some(perch_github::config::home_dir()));
    }
}
