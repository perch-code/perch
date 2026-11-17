//! The settings that are the client's, `[ui]`, `[display]`,
//! `[keybindings]` and `[remote]`, live in this machine's config.toml
//! whatever the engine. In process that is the engine's file too, and
//! every write goes through the engine as it always has. With the engine
//! on a host, the client's are written here and read from here, and the
//! Settings page shows the engine's file with the client's tables put in
//! from this one.

use std::cell::RefCell;
use std::sync::Arc;

use perch_github::{is_client_key, Config, Setting};

use crate::backend::SettingsFile;

/// `changes` split by whose they are: those this machine's file takes,
/// then those the engine's does. In process the engine takes them all.
pub fn route(changes: Vec<Setting>, remote: bool) -> (Vec<Setting>, Vec<Setting>) {
    if !remote {
        return (Vec::new(), changes);
    }
    changes.into_iter().partition(Setting::is_client_s)
}

/// This machine's config.toml, as the client reads it.
#[derive(Debug, Clone, Default)]
pub struct Local {
    /// The file's text, empty with no file.
    pub text: String,
    /// What it loads as, with what is wrong with its `[keybindings]` in
    /// its warnings; the defaults when it does not parse.
    pub config: Config,
    /// Why it does not parse.
    pub error: Option<String>,
}

impl Local {
    /// Read this machine's file.
    pub fn read() -> Self {
        let Some(path) = perch_github::config::config_path() else {
            return Self::default();
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                return Local {
                    error: Some(format!("can't read {}: {e}", path.display())),
                    ..Self::default()
                }
            }
        };
        Self::of(text)
    }

    /// Read this machine's file, keeping the client's settings it last
    /// read whole when it does not parse now, as the engine keeps its own:
    /// a typo saved mid-edit does not put the appearance and keys back to
    /// the defaults, nor this machine back as the engine.
    pub fn read_kept() -> Self {
        Self::kept(Self::read())
    }

    fn kept(read: Self) -> Self {
        LAST_WHOLE.with_borrow_mut(|last| match (&read.error, last.as_ref()) {
            (None, _) => {
                *last = Some(read.clone());
                read
            }
            (Some(_), Some(whole)) => Local {
                text: whole.text.clone(),
                config: whole.config.clone(),
                error: read.error,
            },
            (Some(_), None) => read,
        })
    }

    pub fn of(text: String) -> Self {
        match Config::parse_text(&text) {
            Ok(mut config) => {
                // The engine's check of its file is for the app in
                // process; the keys are this machine's, so checked here.
                config.warnings.clear();
                crate::keymap::add_warnings(&mut config);
                Local {
                    text,
                    config,
                    error: None,
                }
            }
            Err(error) => Local {
                text,
                config: Config::parse_text("").unwrap_or_default(),
                error: Some(error),
            },
        }
    }
}

thread_local! {
    /// This machine's file as last read whole.
    static LAST_WHOLE: RefCell<Option<Local>> = const { RefCell::new(None) };
}

/// The Settings to show before the engine on a host has sent its own:
/// this machine's tables over the defaults, at this machine's `path`, so
/// the page shows the host it is trying and can change it. Unreadable
/// when this machine's file is.
pub fn alone(path: std::path::PathBuf, local: &Local) -> SettingsFile {
    let mut file = merge(
        SettingsFile {
            path,
            config: Some(Arc::new(Config::parse_text("").unwrap_or_default())),
            error: None,
            present: Default::default(),
            warnings: Vec::new(),
        },
        local,
    );
    file.error = local.error.clone();
    file
}

/// The engine's Settings with the client's tables, the keys of them the
/// file sets and what is wrong with them, taken from this machine's file.
/// When the host's file does not parse, the client's tables go over the
/// defaults, so the page still shows this machine's host and appearance;
/// the host's error stays, and with it the engine's rows stay off.
pub fn merge(engine: SettingsFile, local: &Local) -> SettingsFile {
    let mut config = match engine.config {
        Some(config) => Config::clone(&config),
        None => Config::parse_text("").unwrap_or_default(),
    };
    config.ui = local.config.ui.clone();
    config.display = local.config.display.clone();
    config.keybindings = local.config.keybindings.clone();
    config.remote = local.config.remote.clone();
    config
        .warnings
        .extend(local.config.warnings.iter().cloned());
    let config = Some(Arc::new(config));
    let mut present: std::collections::BTreeSet<String> = engine
        .present
        .into_iter()
        .filter(|key| !is_client_key(key))
        .collect();
    present.extend(
        perch_github::present_keys(&local.text, &[])
            .into_iter()
            .filter(|key| is_client_key(key)),
    );
    let mut warnings = engine.warnings;
    warnings.extend(local.config.warnings.iter().cloned());
    if let Some(error) = &local.error {
        warnings.push(format!(
            "{}'s config.toml: {error}",
            perch_engine::host::HERE_FIRST
        ));
    }
    SettingsFile {
        path: engine.path,
        config,
        error: engine.error,
        present,
        warnings,
    }
}

/// Write the client's `changes` to this machine's file. Err says why the
/// file would not take them.
pub fn write_here(changes: &[Setting]) -> Result<(), String> {
    let Some(path) = perch_github::config::config_path() else {
        return Err("Perch has no home folder to keep config.toml in.".to_string());
    };
    perch_github::write_settings(&path, changes, &[]).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// A file that stops parsing keeps the client's settings it had, and
    /// says what is wrong.
    #[test]
    fn a_broken_file_here_keeps_the_client_s_settings_read_last() {
        let whole = Local::kept(Local::of(
            "[remote]\nhost = \"halo\"\n[ui]\nappearance = \"dark\"\n".to_string(),
        ));
        assert_eq!(whole.config.remote.host(), Some("halo"));
        let broken = Local::kept(Local::of("[remote\nhost = ".to_string()));
        assert!(broken.error.is_some());
        assert_eq!(broken.config.remote.host(), Some("halo"));
        assert_eq!(
            broken.config.ui.appearance,
            perch_github::config::Appearance::Dark
        );

        // Before the engine has said, the page has this machine's tables.
        let file = alone("/Users/chris/.perch/config.toml".into(), &whole);
        assert_eq!(file.config.unwrap().remote.host(), Some("halo"));
        assert!(file.error.is_none());
        assert!(file.present.contains("remote.host"));
        assert!(alone("/x".into(), &broken).error.is_some());
    }

    /// A host whose file does not parse: the page still has this
    /// machine's tables, so it shows the host Perch is on, with the
    /// host's error kept for the engine's rows.
    #[test]
    fn a_broken_file_on_the_host_keeps_this_machine_s_tables() {
        let local =
            Local::of("[remote]\nhost = \"halo\"\n[ui]\nappearance = \"dark\"\n".to_string());
        let engine = SettingsFile {
            path: "/home/chris/.perch/config.toml".into(),
            config: None,
            error: Some("line 1: expected `]`".into()),
            present: Default::default(),
            warnings: Vec::new(),
        };
        let merged = merge(engine, &local);
        assert_eq!(merged.error.as_deref(), Some("line 1: expected `]`"));
        let config = merged.config.unwrap();
        assert_eq!(config.remote.host(), Some("halo"));
        assert_eq!(config.ui.appearance, perch_github::config::Appearance::Dark);
        assert!(merged.present.contains("remote.host"));
    }

    #[test]
    fn with_the_engine_on_a_host_the_client_s_settings_stay_here() {
        let changes = vec![
            Setting::Zoom(Some(1.2)),
            Setting::PollIntervalSecs(Some(30)),
            Setting::Keybinding {
                action: "go-to-file".into(),
                keys: None,
            },
            Setting::RemoteHost(Some("halo".into())),
            Setting::AgentModel(Some("opus".into())),
        ];
        let (here, engine) = route(changes.clone(), true);
        assert_eq!(
            here,
            [
                Setting::Zoom(Some(1.2)),
                Setting::Keybinding {
                    action: "go-to-file".into(),
                    keys: None,
                },
                Setting::RemoteHost(Some("halo".into())),
            ]
        );
        assert_eq!(
            engine,
            [
                Setting::PollIntervalSecs(Some(30)),
                Setting::AgentModel(Some("opus".into())),
            ]
        );
        // In process the one file is the engine's, as it always was.
        assert_eq!(route(changes.clone(), false), (Vec::new(), changes));
    }

    #[test]
    fn the_page_shows_the_engine_s_file_with_this_machine_s_tables() {
        let engine_text =
            "poll_interval_secs = 120\n[display]\nzoom = 2.0\n[ui]\nappearance = \"dark\"\n";
        let engine_config = Config::parse_text(engine_text).unwrap();
        let engine = SettingsFile {
            path: "/home/chris/.perch/config.toml".into(),
            present: perch_github::present_keys(engine_text, &[]),
            warnings: Vec::new(),
            config: Some(Arc::new(engine_config)),
            error: None,
        };
        let local = Local::of(
            "poll_interval_secs = 5\n[display]\ncode_font_size = 14\n[keybindings]\nno-such-action = \"cmd-k\"\n[remote]\nhost = \"halo\"\n"
                .to_string(),
        );
        let merged = merge(engine, &local);
        let config = merged.config.unwrap();
        assert_eq!(config.poll_interval_secs, 120);
        assert_eq!(
            config.display.zoom,
            Config::parse_text("").unwrap().display.zoom
        );
        assert_eq!(
            config.ui.appearance,
            perch_github::config::Appearance::System
        );
        assert_eq!(config.remote.host(), Some("halo"));
        assert_eq!(
            merged.present,
            // The engine's keys are the engine's file's, this machine's
            // poll interval included.
            [
                "display.code_font_size",
                "keybindings.no-such-action",
                "poll_interval_secs",
                "remote.host",
            ]
            .into_iter()
            .map(String::from)
            .collect::<BTreeSet<_>>()
        );
        // This machine's keys are checked here.
        assert!(
            merged.warnings.iter().any(|w| w.contains("no-such-action")),
            "{:?}",
            merged.warnings
        );
        assert_eq!(
            merged.path,
            std::path::PathBuf::from("/home/chris/.perch/config.toml")
        );
    }
}
