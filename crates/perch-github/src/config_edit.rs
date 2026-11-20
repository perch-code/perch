//! Changing config.toml a key at a time, for the Settings page.
//!
//! The file stays the reader's. Every write reads it again from disk,
//! changes only the keys it was asked to through `toml_edit`, so comments,
//! order and formatting survive, and replaces the file only once the result
//! still loads. A key set to nothing is taken out, which puts it back to
//! its default.

use std::collections::BTreeSet;
use std::path::Path;

use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value};

use crate::config::{
    toml_error_line, Appearance, Approvals, Config, KnowledgeSource, RepoConfig, Theme,
};

/// Remote windows write app preferences on the UI thread while the local
/// engine can edit the same file from its worker thread.
pub(crate) fn config_write_guard() -> std::sync::MutexGuard<'static, ()> {
    static WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    WRITE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One of the coding agents, as far as its config table goes: Claude Code
/// under `[agent]`, Codex under `[agent.codex]`, Gemini under
/// `[agent.gemini]`. Its own small enum, since this crate doesn't depend on
/// perch-agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum AgentBackend {
    Claude,
    Codex,
    Gemini,
    OpenCode,
}

impl AgentBackend {
    pub const ALL: [AgentBackend; 4] = [Self::Claude, Self::Codex, Self::Gemini, Self::OpenCode];

    fn table(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["agent"],
            Self::Codex => &["agent", "codex"],
            Self::Gemini => &["agent", "gemini"],
            Self::OpenCode => &["agent", "opencode"],
        }
    }
}

/// One key the Settings page can write. `None`, an empty string or an
/// empty list takes the key out of the file, which resets it to its
/// default, except for a keybinding, where an empty list means no keys.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Setting {
    PollIntervalSecs(Option<u64>),
    GithubHost(Option<String>),
    CloneRoot(Option<String>),
    AgentModel(Option<String>),
    AgentEffort(Option<String>),
    AgentMaxConcurrentRuns(Option<usize>),
    /// `[agent] sandbox`: `Some(false)` turns Claude Code's and Gemini's
    /// sandboxes off; None takes the key out, back to on.
    AgentSandbox(Option<bool>),
    AgentBinary {
        backend: AgentBackend,
        binary: Option<String>,
    },
    AgentModels {
        backend: AgentBackend,
        models: Option<Vec<String>>,
    },
    WorkBranchPrefix(Option<String>),
    WorkApprovals(Option<Approvals>),
    Appearance(Option<Appearance>),
    Theme(Option<Theme>),
    CodeFont(Option<String>),
    Zoom(Option<f64>),
    CodeFontSize(Option<f32>),
    LspMaxMemoryMb(Option<u64>),
    /// `[keybindings] <action>`: None takes it out, back to the action's
    /// default keys; an empty list writes `[]`, no keys at all; one key is
    /// written as a string and several as a list.
    Keybinding {
        action: String,
        keys: Option<Vec<String>>,
    },
    /// `[remote] host`: None, or a blank, takes it out, back to the engine
    /// on this machine.
    RemoteHost(Option<String>),
    /// A key of the `[[repo]]` whose identity this is, compared without
    /// regard to case.
    Repo {
        identity: String,
        change: RepoSetting,
    },
    /// A `[[knowledge]]` entry, identified by its relative directory.
    Knowledge {
        dir: std::path::PathBuf,
        change: KnowledgeSetting,
    },
}

/// The tables that are the client's: how its windows look and what its
/// keys do, and where its engine is. Read from the file on the machine the
/// window is on, whatever the engine; in process that is the one file.
pub const CLIENT_TABLES: [&str; 4] = ["ui", "display", "keybindings", "remote"];

impl Setting {
    /// Whether the setting lives in one of [`CLIENT_TABLES`].
    pub fn is_client_s(&self) -> bool {
        matches!(
            self,
            Setting::Appearance(_)
                | Setting::Theme(_)
                | Setting::CodeFont(_)
                | Setting::Zoom(_)
                | Setting::CodeFontSize(_)
                | Setting::Keybinding { .. }
                | Setting::RemoteHost(_)
        )
    }
}

/// Whether a key as `present_keys` gives it, such as `display.zoom`, is in
/// one of [`CLIENT_TABLES`].
pub fn is_client_key(key: &str) -> bool {
    let table = key.split('.').next().unwrap_or(key);
    CLIENT_TABLES.contains(&table)
}

/// A key of one `[[repo]]`, or of its `[repo.work]` or `[repo.lsp]`. A
/// switch that is off is taken out rather than written as false.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum RepoSetting {
    Name(Option<String>),
    Base(Option<String>),
    SyncHistory(bool),
    Approvals(Option<Approvals>),
    WorkTrust(bool),
    LspTrust(bool),
    Write(Vec<String>),
    Network(Vec<String>),
}

/// An edit to one external knowledge directory. An omitted repository list
/// grants it to every repository. An empty one grants it to none.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum KnowledgeSetting {
    Add(Option<Vec<String>>),
    Repositories(Option<Vec<String>>),
    Remove,
}

/// What a write left in the file: the config it now loads as, and the
/// warnings loading it gave.
#[derive(Debug, Clone)]
pub struct Written {
    pub config: Config,
    pub warnings: Vec<String>,
}

/// Where a setting lives: which table, from the file's top or from one
/// repository's `[[repo]]`, and which key in it.
struct Place {
    repo: Option<String>,
    tables: &'static [&'static str],
    /// A keybinding's key is its action's id, which the setting carries.
    key: std::borrow::Cow<'static, str>,
    /// How a message names the key, such as `[agent.codex] binary`.
    label: String,
}

/// Make the changes to the config file at `path` and replace it with the
/// result.
///
/// The file is read fresh, so hand edits made since Perch last read it are
/// kept. A missing file is started from nothing. Nothing is written when
/// the file doesn't parse, when a change names a repository the file
/// doesn't hold, when the result wouldn't load, or when it would load with
/// a different number of repositories (a rename onto another's name, say).
/// The new file takes the old one's permissions, or owner-only for a new
/// one, since the file can hold a token.
///
/// `known` is the running config's repositories, resolved. A `[[repo]]`
/// with only a root has no identity in the file itself: the scan gives it
/// one from its remote. `known` is how such an entry is found by that
/// identity. Without it, a root-only entry answers only to `local/<name>`.
pub fn write_settings(
    path: &Path,
    changes: &[Setting],
    known: &[RepoConfig],
) -> Result<Written, String> {
    let _guard = config_write_guard();
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("Couldn't read {}: {e}", path.display())),
    };
    let (edited, config) = edit_text(&text, changes, known)?;
    if edited != text {
        replace_file(path, &edited)
            .map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
    }
    let warnings = config.warnings.clone();
    Ok(Written { config, warnings })
}

/// `write_settings` on the text alone: the new text, and the config it
/// loads as.
fn edit_text(
    text: &str,
    changes: &[Setting],
    known: &[RepoConfig],
) -> Result<(String, Config), String> {
    // toml_edit reads a byte-order mark and CRLF line ends but writes
    // neither, so they come off here and go back on the result. Otherwise
    // a file kept that way would change on every line, not just the keys.
    let (bom, text) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    let crlf = text.contains("\r\n");
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| {
        format!(
            "config.toml doesn't parse, so nothing was changed: {}",
            toml_error_line(text, e.span(), e.message())
        )
    })?;
    let before = Config::parse_text(text)
        .map_err(|e| format!("config.toml doesn't load, so nothing was changed: {e}"))?;
    let loaded_before: Vec<Option<String>> = repo_elements(&doc)
        .zip(loaded(&doc, &before))
        .map(|(element, loads)| {
            loads.then(|| element_identity(element, known).unwrap_or_else(|| "[[repo]]".into()))
        })
        .collect();
    for change in changes {
        apply(&mut doc, change, known)?;
    }
    let edited = doc.to_string();
    let after = Config::parse_text(&edited).map_err(|e| {
        format!("That change would leave config.toml unreadable, so it wasn't made: {e}")
    })?;
    // An entry the file loaded before has to load after: a rename onto
    // another repository's name would drop one of the two. An entry that
    // was skipped may come back, as when a rename settles a clash.
    let lost: Vec<String> = loaded_before
        .into_iter()
        .zip(loaded(&doc, &after))
        .filter_map(|(before, loads)| before.filter(|_| !loads))
        .collect();
    if !lost.is_empty() {
        let why: Vec<&str> = after
            .warnings
            .iter()
            .filter(|w| !before.warnings.contains(w))
            .map(String::as_str)
            .collect();
        let mut message = format!(
            "That change would stop config.toml loading {}, so it wasn't made",
            lost.join(" and ")
        );
        if !why.is_empty() {
            message.push_str(&format!(": {}", why.join(" ")));
        }
        return Err(message);
    }
    let mut edited = edited;
    if crlf {
        edited = edited.replace("\r\n", "\n").replace('\n', "\r\n");
    }
    Ok((format!("{bom}{edited}"), after))
}

fn repo_elements(doc: &DocumentMut) -> impl Iterator<Item = &Table> {
    doc.get("repo")
        .and_then(Item::as_array_of_tables)
        .into_iter()
        .flat_map(ArrayOfTables::iter)
}

/// Which of the file's `[[repo]]` elements `config`, the file loaded,
/// kept. Loading only ever skips entries and keeps the rest in order, so
/// an element loaded when it reads, alone, as the next entry kept.
fn loaded(doc: &DocumentMut, config: &Config) -> Vec<bool> {
    let mut kept = config.repo.iter().peekable();
    repo_elements(doc)
        .map(|element| {
            let loads = element_entry(element).is_some_and(|entry| kept.peek() == Some(&&entry));
            if loads {
                kept.next();
            }
            loads
        })
        .collect()
}

fn apply(doc: &mut DocumentMut, change: &Setting, known: &[RepoConfig]) -> Result<(), String> {
    if let Setting::Knowledge { dir, change } = change {
        return edit_knowledge(doc, dir, change);
    }
    let (place, value) = place_of(change)?;
    let left = match &place.repo {
        None => edit_in(doc.as_table_mut(), false, place.tables, &place, value)?,
        Some(identity) => {
            let Some(repos) = doc
                .as_table_mut()
                .get_mut("repo")
                .and_then(Item::as_array_of_tables_mut)
            else {
                return Err(no_repo(identity));
            };
            let index = repos
                .iter()
                .position(|element| {
                    element_identity(element, known)
                        .is_some_and(|id| id.eq_ignore_ascii_case(identity))
                })
                .ok_or_else(|| no_repo(identity))?;
            let element = repos.get_mut(index).ok_or_else(|| no_repo(identity))?;
            let header = element.position();
            edit_in(element, false, place.tables, &place, value)?.map(|mut left| {
                left.header = left.header.or(header);
                left
            })
        }
    };
    if let Some(left) = left {
        put_above_header(doc, left);
    }
    Ok(())
}

fn knowledge_dir(table: &dyn TableLike) -> Option<&Path> {
    table.get("dir").and_then(Item::as_str).map(Path::new)
}

fn edit_knowledge(
    doc: &mut DocumentMut,
    dir: &Path,
    change: &KnowledgeSetting,
) -> Result<(), String> {
    let repos = match change {
        KnowledgeSetting::Add(repos) | KnowledgeSetting::Repositories(repos) => {
            KnowledgeSource {
                dir: dir.to_path_buf(),
                repos: repos.clone(),
            }
            .validate()?;
            repos
                .as_ref()
                .map(|repos| Value::Array(repos.iter().map(String::as_str).collect()))
        }
        KnowledgeSetting::Remove => None,
    };
    let add = matches!(change, KnowledgeSetting::Add(_));
    let remove = matches!(change, KnowledgeSetting::Remove);
    let missing = || format!("config.toml has no [[knowledge]] for {}", dir.display());
    let Some(item) = doc.get_mut("knowledge") else {
        if !add {
            return Err(missing());
        }
        let mut array = ArrayOfTables::new();
        array.push(knowledge_table(dir, repos)?);
        doc.insert("knowledge", Item::ArrayOfTables(array));
        return Ok(());
    };
    let place = Place {
        repo: None,
        tables: &[],
        key: "repos".into(),
        label: format!("[[knowledge]] repos for {}", dir.display()),
    };
    let left = match item {
        Item::ArrayOfTables(array) => {
            let index = knowledge_index(
                array.iter().map(|table| Some(table as &dyn TableLike)),
                dir,
                add,
            )?;
            match index {
                None => {
                    array.push(knowledge_table(dir, repos)?);
                    None
                }
                Some(index) if remove => {
                    array.remove(index);
                    None
                }
                Some(index) => {
                    let table = array.get_mut(index).expect("matched table");
                    let header = table.position();
                    edit_in(table, false, &[], &place, repos)?.map(|mut left| {
                        left.header = header;
                        left
                    })
                }
            }
        }
        Item::Value(Value::Array(array)) => {
            let index = knowledge_index(
                array
                    .iter()
                    .map(|value| value.as_inline_table().map(|table| table as &dyn TableLike)),
                dir,
                add,
            )?;
            match index {
                None => array.push(knowledge_table(dir, repos)?.into_inline_table()),
                Some(index) if remove => {
                    array.remove(index);
                }
                Some(index) => {
                    let table = array
                        .get_mut(index)
                        .and_then(Value::as_inline_table_mut)
                        .expect("matched table");
                    edit_in(table, true, &[], &place, repos)?;
                }
            }
            None
        }
        _ => return Err("config.toml must list knowledge entries as [[knowledge]]".into()),
    };
    if let Some(left) = left {
        put_above_header(doc, left);
    }
    Ok(())
}

fn knowledge_index<'a>(
    entries: impl Iterator<Item = Option<&'a dyn TableLike>>,
    dir: &Path,
    add: bool,
) -> Result<Option<usize>, String> {
    let mut matching = entries
        .enumerate()
        .filter_map(|(index, table)| (table.and_then(knowledge_dir) == Some(dir)).then_some(index));
    let index = matching.next();
    if (add && index.is_some()) || matching.next().is_some() {
        return Err(format!("config.toml already lists {}", dir.display()));
    }
    if !add && index.is_none() {
        return Err(format!(
            "config.toml has no [[knowledge]] for {}",
            dir.display()
        ));
    }
    Ok(index)
}

fn knowledge_table(dir: &Path, repos: Option<Value>) -> Result<Table, String> {
    let dir = dir.to_str().ok_or("Knowledge directory must be UTF-8")?;
    let mut table = Table::new();
    table.insert("dir", toml_edit::value(dir));
    if let Some(repos) = repos {
        table.insert("repos", Item::Value(repos));
    }
    Ok(table)
}

fn no_repo(identity: &str) -> String {
    format!("config.toml has no [[repo]] for {identity}, so nothing was changed")
}

/// Where a change goes, and the value it leaves there: None to take the
/// key out.
fn place_of(change: &Setting) -> Result<(Place, Option<Value>), String> {
    let top = |tables: &'static [&'static str], key: std::borrow::Cow<'static, str>| Place {
        repo: None,
        tables,
        label: label(tables, &key, None),
        key,
    };
    Ok(match change {
        Setting::PollIntervalSecs(secs) => (
            top(&[], "poll_interval_secs".into()),
            integer(*secs, "poll_interval_secs")?,
        ),
        Setting::GithubHost(host) => (top(&[], "github_host".into()), string(host)),
        Setting::CloneRoot(root) => (top(&[], "clone_root".into()), string(root)),
        Setting::AgentModel(model) => (top(&["agent"], "model".into()), string(model)),
        Setting::AgentEffort(effort) => (top(&["agent"], "effort".into()), string(effort)),
        Setting::AgentMaxConcurrentRuns(runs) => (
            top(&["agent"], "max_concurrent_runs".into()),
            integer(runs.map(|n| n as u64), "max_concurrent_runs")?,
        ),
        Setting::AgentSandbox(on) => (top(&["agent"], "sandbox".into()), on.map(Value::from)),
        Setting::AgentBinary { backend, binary } => {
            (top(backend.table(), "binary".into()), string(binary))
        }
        Setting::AgentModels { backend, models } => (
            top(backend.table(), "models".into()),
            list(models.as_deref().unwrap_or_default()),
        ),
        Setting::WorkBranchPrefix(prefix) => {
            (top(&["work"], "branch_prefix".into()), string(prefix))
        }
        Setting::WorkApprovals(approvals) => (
            top(&["work"], "approvals".into()),
            approvals.map(|a| Value::from(a.name())),
        ),
        Setting::Appearance(appearance) => (
            top(&["ui"], "appearance".into()),
            appearance.map(|a| Value::from(a.name())),
        ),
        Setting::Theme(theme) => (
            top(&["ui"], "theme".into()),
            theme.map(|theme| Value::from(theme.name())),
        ),
        Setting::CodeFont(font) => (top(&["ui"], "code_font".into()), string(font)),
        Setting::Zoom(zoom) => (top(&["display"], "zoom".into()), float(*zoom, "zoom")?),
        Setting::CodeFontSize(size) => {
            let value = match size {
                // A whole size is written as one, as a person would.
                Some(s) if s.is_finite() && s.fract() == 0.0 => Some(Value::from(*s as i64)),
                Some(s) => float(Some(f64::from(*s)), "code_font_size")?,
                None => None,
            };
            (top(&["display"], "code_font_size".into()), value)
        }
        Setting::LspMaxMemoryMb(mb) => (
            top(&["lsp"], "max_memory_mb".into()),
            integer(*mb, "max_memory_mb")?,
        ),
        Setting::RemoteHost(host) => (top(&["remote"], "host".into()), string(host)),
        Setting::Keybinding { action, keys } => (
            top(&["keybindings"], action.clone().into()),
            keys.as_deref().map(keybinding),
        ),
        Setting::Repo { identity, change } => {
            let (tables, key, value): (&'static [&'static str], _, _) = match change {
                RepoSetting::Name(name) => (&[], "name", string(name)),
                RepoSetting::Base(base) => (&[], "base", string(base)),
                RepoSetting::SyncHistory(on) => (&[], "sync_history", on.then(|| true.into())),
                RepoSetting::Approvals(a) => (&["work"], "approvals", a.map(|a| a.name().into())),
                RepoSetting::WorkTrust(on) => (&["work"], "trust", on.then(|| true.into())),
                RepoSetting::LspTrust(on) => (&["lsp"], "trust", on.then(|| true.into())),
                RepoSetting::Write(dirs) => (&["work"], "write", list(dirs)),
                RepoSetting::Network(hosts) => (&["work"], "network", list(hosts)),
            };
            let place = Place {
                repo: Some(identity.clone()),
                tables,
                key: key.into(),
                label: label(tables, key, Some(identity)),
            };
            (place, value)
        }
        Setting::Knowledge { .. } => unreachable!("handled in edit_knowledge"),
    })
}

fn label(tables: &[&str], key: &str, repo: Option<&str>) -> String {
    let table = match repo {
        Some(identity) => {
            let mut name = vec!["repo"];
            name.extend(tables);
            format!("{identity}'s [{}] ", name.join("."))
        }
        None if tables.is_empty() => String::new(),
        None => format!("[{}] ", tables.join(".")),
    };
    format!("{table}{key}")
}

/// A string, trimmed; an empty one takes the key out.
fn string(value: &Option<String>) -> Option<Value> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(Value::from)
}

/// A list of strings, each trimmed and the empty ones dropped; an empty
/// list takes the key out.
fn list(items: &[String]) -> Option<Value> {
    let items: Vec<&str> = items
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    (!items.is_empty()).then(|| Value::Array(items.into_iter().collect::<Array>()))
}

/// A keybinding's keys, each trimmed and the empty ones dropped: one as a
/// string, otherwise a list, which can be empty.
fn keybinding(keys: &[String]) -> Value {
    let keys: Vec<&str> = keys
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    match keys.as_slice() {
        [one] => Value::from(*one),
        _ => Value::Array(keys.into_iter().collect::<Array>()),
    }
}

fn integer(value: Option<u64>, key: &str) -> Result<Option<Value>, String> {
    value
        .map(|n| {
            i64::try_from(n)
                .map(Value::from)
                .map_err(|_| format!("{n} is too big for {key}"))
        })
        .transpose()
}

fn float(value: Option<f64>, key: &str) -> Result<Option<Value>, String> {
    match value {
        Some(f) if !f.is_finite() => Err(format!("{f} isn't a number {key} can take")),
        other => Ok(other.map(Value::from)),
    }
}

/// Comment lines that sat above a key taken out, with nothing after the
/// key in its table's body to hang them on. They go above the next table
/// header, which is where the text after the body begins.
struct LeftComment {
    comment: String,
    /// The header of the table whose body the key was in; None for the top
    /// of the file.
    header: Option<isize>,
}

/// Set or take out `place.key` in the table `tables` names under `table`,
/// making the tables on the way when a value is set.
///
/// A table made here takes the shape of its parent: inside an inline
/// table it is inline, inside a dotted key it is dotted, and otherwise it
/// is a `[header]` table, with only the last of a run of new ones shown,
/// so `[agent.codex]` comes without an empty `[agent]` above it. Taking out
/// a table's last key also takes out the table, unless a comment sits on
/// its header or the key's own comment has to stay below it.
///
/// Comment lines above a key taken out move to whatever follows it, so they
/// stay where they were. What comes back is a comment that has to go above
/// the next header.
fn edit_in(
    table: &mut dyn TableLike,
    inline: bool,
    tables: &[&str],
    place: &Place,
    value: Option<Value>,
) -> Result<Option<LeftComment>, String> {
    let Some((head, rest)) = tables.split_first() else {
        let added = value.is_some() && !table.contains_key(&place.key);
        let left = set_key(table, inline, place, value)?;
        // A key added to an inline table would sit after the old last
        // one's space before the `}`; tidy the table's spacing instead.
        if inline && added {
            table.fmt();
        }
        return Ok(left.map(|comment| LeftComment {
            comment,
            header: None,
        }));
    };
    if table.get(head).is_none_or(Item::is_none) {
        if value.is_none() {
            return Ok(None);
        }
        let child = if inline {
            Item::Value(Value::InlineTable(InlineTable::new()))
        } else {
            let mut child = Table::new();
            if table.is_dotted() {
                child.set_dotted(true);
            } else {
                child.set_implicit(!rest.is_empty());
            }
            Item::Table(child)
        };
        table.insert(head, child);
    }
    let removing = value.is_none();
    let index = table.iter().position(|(k, _)| k == *head).unwrap_or(0);
    let item = table.get_mut(head).expect("just found or made");
    let (left, left_bare, dotted) = match item {
        Item::Table(child) => {
            let left = edit_in(child, false, rest, place, value)?;
            let dotted = child.is_dotted();
            let header = child.position();
            // A comment out of a header table's body stays under that
            // header, so the header stays even with nothing else below it.
            let bare =
                child.is_empty() && !has_comment(child.decor()) && (dotted || left.is_none());
            let left = left.map(|mut left| {
                if !dotted {
                    left.header = left.header.or(header);
                }
                left
            });
            (left, bare, dotted)
        }
        Item::Value(Value::InlineTable(child)) => {
            let left = edit_in(child, true, rest, place, value)?;
            (left, child.is_empty(), true)
        }
        _ => {
            return Err(format!(
                "config.toml has {} as a value, not a table, so {} wasn't changed",
                head, place.label
            ))
        }
    };
    let removed = removing && left_bare;
    if removed {
        table.remove(head);
    }
    // A dotted table's keys sit in this table's body, so its comment can
    // go on whatever follows it here.
    Ok(match left {
        Some(left) if dotted && left.header.is_none() && !inline => {
            let next = if removed { index } else { index + 1 };
            put_above_first(table, next, left.comment).map(|comment| LeftComment {
                comment,
                header: None,
            })
        }
        left => left,
    })
}

/// Set the key, or take it out. Taking it out hands back the comment lines
/// above it when nothing after it in the body could take them.
fn set_key(
    table: &mut dyn TableLike,
    inline: bool,
    place: &Place,
    value: Option<Value>,
) -> Result<Option<String>, String> {
    let Some(mut value) = value else {
        let index = table.iter().position(|(k, _)| *k == place.key);
        let comment = table
            .key(&place.key)
            .and_then(|key| key.leaf_decor().prefix())
            .and_then(|prefix| prefix.as_str())
            .filter(|prefix| prefix.contains('#'))
            .map(str::to_string);
        table.remove(&place.key);
        return Ok(match (comment, index) {
            (Some(comment), Some(index)) if !inline => put_above_first(table, index, comment),
            _ => None,
        });
    };
    match table.get_mut(&place.key) {
        // In place, keeping what sat around the old value, such as a
        // comment after it.
        Some(Item::Value(old)) => {
            *value.decor_mut() = old.decor().clone();
            *old = value;
        }
        Some(item) if !item.is_none() => {
            return Err(format!(
                "config.toml has {} as a table, not a value, so it wasn't changed",
                place.label
            ))
        }
        _ => {
            table.insert(&place.key, Item::Value(value));
        }
    }
    Ok(None)
}

/// Put `comment` above the first key from the `skip`th on that this
/// table's body shows, going into dotted tables, whose keys show there
/// too. Hands it back when the body shows nothing from there on.
fn put_above_first(table: &mut dyn TableLike, skip: usize, comment: String) -> Option<String> {
    let names: Vec<String> = table
        .iter()
        .skip(skip)
        .map(|(k, _)| k.to_string())
        .collect();
    let mut comment = comment;
    for name in names {
        let Some((mut key, item)) = table.get_key_value_mut(&name) else {
            continue;
        };
        match item {
            Item::Value(_) => {
                prepend(key.leaf_decor_mut(), &comment);
                return None;
            }
            Item::Table(child) if child.is_dotted() => {
                comment = put_above_first(child, 0, comment)?;
            }
            _ => {}
        }
    }
    Some(comment)
}

/// Put `left` above the first header after the one it came from, or at the
/// end of the file when none follows.
fn put_above_header(doc: &mut DocumentMut, left: LeftComment) {
    let mut found = Vec::new();
    headers(doc.as_table(), &mut Vec::new(), &mut found);
    let next = found
        .into_iter()
        .filter(|(position, _)| left.header.is_none_or(|header| *position > header))
        .min_by_key(|(position, _)| *position);
    match next.and_then(|(_, path)| header_at(doc.as_table_mut(), &path)) {
        Some(table) => prepend(table.decor_mut(), &left.comment),
        None => {
            let trailing = doc.trailing().as_str().unwrap_or_default().to_string();
            doc.set_trailing(format!("{}{trailing}", left.comment));
        }
    }
}

fn prepend(decor: &mut toml_edit::Decor, comment: &str) {
    let old = decor
        .prefix()
        .and_then(|p| p.as_str())
        .unwrap_or_default()
        .to_string();
    decor.set_prefix(format!("{comment}{old}"));
}

/// A step from a table to one below it: a key, then an index for an array
/// of tables.
#[derive(Clone)]
enum Step {
    Key(String),
    Index(usize),
}

/// Every table under `table` that shows a header, with its place in the
/// file and the way to it.
fn headers(table: &Table, path: &mut Vec<Step>, found: &mut Vec<(isize, Vec<Step>)>) {
    for (key, item) in table.iter() {
        match item {
            Item::Table(child) => {
                path.push(Step::Key(key.to_string()));
                if let Some(position) = child.position() {
                    if !child.is_dotted() && !child.is_implicit() {
                        found.push((position, path.clone()));
                    }
                }
                headers(child, path, found);
                path.pop();
            }
            Item::ArrayOfTables(array) => {
                for (index, child) in array.iter().enumerate() {
                    path.push(Step::Key(key.to_string()));
                    path.push(Step::Index(index));
                    if let Some(position) = child.position() {
                        found.push((position, path.clone()));
                    }
                    headers(child, path, found);
                    path.pop();
                    path.pop();
                }
            }
            _ => {}
        }
    }
}

fn header_at<'a>(mut table: &'a mut Table, path: &[Step]) -> Option<&'a mut Table> {
    let mut steps = path.iter().peekable();
    while let Some(step) = steps.next() {
        let Step::Key(key) = step else {
            return None;
        };
        let index = match steps.peek() {
            Some(Step::Index(index)) => Some(*index),
            _ => None,
        };
        table = match (table.get_mut(key)?, index) {
            (Item::ArrayOfTables(array), Some(index)) => {
                steps.next();
                array.get_mut(index)?
            }
            (Item::Table(child), None) => child,
            _ => return None,
        };
    }
    Some(table)
}

/// Whether a comment sits on a table's header, above it or after it.
fn has_comment(decor: &toml_edit::Decor) -> bool {
    [decor.prefix(), decor.suffix()]
        .into_iter()
        .flatten()
        .filter_map(|raw| raw.as_str())
        .any(|text| text.contains('#'))
}

/// The identity one `[[repo]]` element reads as, by loading it alone the
/// way `Config` loads the file: its `github` (or an old `name =
/// "owner/repo"`), else what `known` resolved an entry with the same root
/// and name to, else `local/<name>`. None for an element `Config` would
/// skip.
fn element_identity(element: &Table, known: &[RepoConfig]) -> Option<String> {
    let entry = element_entry(element)?;
    if let Some(github) = &entry.github {
        return Some(github.clone());
    }
    let resolved = known.iter().find(|k| {
        k.github.is_none() && k.root == entry.root && k.name == entry.name && k.is_resolved()
    });
    if let Some(identity) = resolved.and_then(RepoConfig::identity) {
        return Some(identity.to_string());
    }
    let mut entry = entry;
    entry.resolve(None);
    entry.identity().map(str::to_string)
}

/// One `[[repo]]` element loaded alone, the way `Config` loads the file.
/// None for an element it would skip.
fn element_entry(element: &Table) -> Option<RepoConfig> {
    let mut alone = DocumentMut::new();
    let mut repos = ArrayOfTables::new();
    repos.push(element.clone());
    alone.insert("repo", Item::ArrayOfTables(repos));
    let config = Config::parse(&alone.to_string()).ok()?;
    let [entry] = <[RepoConfig; 1]>::try_from(config.repo).ok()?;
    Some(entry)
}

/// The keys the file sets, dotted: `poll_interval_secs`,
/// `agent.codex.binary`, `display.zoom`. A repository's are under its
/// identity, lowercased: `repo.owner/name.work.trust`, `repo.local/notes.base`,
/// with `known` placing root-only entries as `write_settings` does. Empty
/// for a file that doesn't parse.
pub fn present_keys(text: &str, known: &[RepoConfig]) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let Ok(doc) = text.parse::<DocumentMut>() else {
        return keys;
    };
    for (key, item) in doc.as_table().iter() {
        match item {
            Item::ArrayOfTables(repos) if key == "repo" => {
                for element in repos.iter() {
                    if let Some(identity) = element_identity(element, known) {
                        let prefix = format!("repo.{}", identity.to_lowercase());
                        collect_keys(&prefix, element, &mut keys);
                    }
                }
            }
            _ => collect_key(key, item, &mut keys),
        }
        if key == "knowledge" {
            if let Some(array) = item.as_array_of_tables() {
                for table in array.iter() {
                    collect_knowledge_keys(table, &mut keys);
                }
            } else if let Some(array) = item.as_array() {
                for table in array.iter().filter_map(Value::as_inline_table) {
                    collect_knowledge_keys(table, &mut keys);
                }
            }
        }
    }
    keys
}

fn collect_knowledge_keys(table: &dyn TableLike, keys: &mut BTreeSet<String>) {
    if let Some(dir) = knowledge_dir(table) {
        collect_keys(&format!("knowledge.{}", dir.display()), table, keys);
    }
}

fn collect_keys(prefix: &str, table: &dyn TableLike, keys: &mut BTreeSet<String>) {
    for (key, item) in table.iter() {
        collect_key(&format!("{prefix}.{key}"), item, keys);
    }
}

fn collect_key(key: &str, item: &Item, keys: &mut BTreeSet<String>) {
    match item {
        Item::None => {}
        Item::Table(table) => collect_keys(key, table, keys),
        Item::Value(Value::InlineTable(table)) => collect_keys(key, table, keys),
        _ => {
            keys.insert(key.to_string());
        }
    }
}

/// Write `text` beside the file, then rename it over the file, so a reader
/// never sees half of it. The new file has the old one's permissions, or
/// is the owner's alone when there was none. A symlinked config is written
/// through to its target, so the link stays.
fn replace_file(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;

    let target = match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::canonicalize(path)?,
        _ => path.to_path_buf(),
    };
    let old = std::fs::metadata(&target).ok().map(|m| m.permissions());
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let temp = target.with_extension("toml.writing");
    let written = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        match old {
            Some(permissions) => std::fs::set_permissions(&temp, permissions)?,
            None => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
                }
            }
        }
        std::fs::rename(&temp, &target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentConfig;

    fn write(dir: &tempfile::TempDir, text: &str) -> std::path::PathBuf {
        let path = dir.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    fn edit(text: &str, changes: &[Setting]) -> String {
        edit_text(text, changes, &[]).unwrap().0
    }

    fn repo(identity: &str, change: RepoSetting) -> Setting {
        Setting::Repo {
            identity: identity.into(),
            change,
        }
    }

    fn knowledge(dir: &str, change: KnowledgeSetting) -> Setting {
        Setting::Knowledge {
            dir: dir.into(),
            change,
        }
    }

    #[test]
    fn knowledge_edits_preserve_other_sources_and_repo_scope() {
        let text = "poll_interval_secs = 60\n\n# Design discussions\n[[knowledge]]\ndir = \"discord/project\"\nrepos = [] # Paused\n";
        let add = knowledge("notes", KnowledgeSetting::Add(None));
        let update = knowledge(
            "discord/project",
            KnowledgeSetting::Repositories(Some(vec!["acme/widgets".into()])),
        );
        let edited = edit(text, &[add, update]);
        assert!(edited.contains("poll_interval_secs = 60"));
        assert!(edited.contains("# Design discussions"));
        assert!(edited.contains("repos = [\"acme/widgets\"] # Paused"));
        assert!(edited.contains("[[knowledge]]\ndir = \"notes\""));
        assert!(present_keys(&edited, &[]).contains("knowledge.discord/project.repos"));
        assert!(!present_keys(&edited, &[]).contains("knowledge.notes.repos"));
        let remove = knowledge("notes", KnowledgeSetting::Remove);
        let reset = knowledge("discord/project", KnowledgeSetting::Repositories(None));
        let edited = edit(&edited, &[remove, reset]);
        let config = Config::parse_text(&edited).unwrap();
        assert_eq!(config.knowledge.len(), 1);
        assert!(config.knowledge[0].repos.is_none());
        assert!(edited.contains("poll_interval_secs = 60"));
    }

    #[test]
    fn write_knowledge_preserves_inline_arrays_and_rejects_invalid_adds() {
        let text = "knowledge = [{ dir = \"notes\" }] # Keep this comment\n";
        let add = knowledge("discord/project", KnowledgeSetting::Add(Some(Vec::new())));
        let (edited, config) = edit_text(text, &[add], &[]).unwrap();
        assert_eq!(config.knowledge[1].repos, Some(Vec::new()));
        assert!(edited.ends_with(" # Keep this comment\n"));
        assert!(present_keys(&edited, &[]).contains("knowledge.discord/project.repos"));
        let duplicate = knowledge("discord//project", KnowledgeSetting::Add(None));
        assert!(edit_text(&edited, &[duplicate], &[]).is_err());
        let reserved = knowledge("perch/repos", KnowledgeSetting::Add(None));
        assert!(edit_text(&edited, &[reserved], &[]).is_err());
        assert_eq!(
            edit(
                &edited,
                &[knowledge("discord/project", KnowledgeSetting::Remove)]
            ),
            text
        );
    }

    #[test]
    fn comments_order_and_formatting_are_kept() {
        let text = "# Perch\npoll_interval_secs = 60 # a minute\n\n# who\n[agent]\n# the default\nmodel   =   \"opus\"\neffort = 'high'\n\n[work]\nbranch_prefix = \"chris/\"\n";
        let out = edit(
            text,
            &[
                Setting::PollIntervalSecs(Some(120)),
                Setting::AgentModel(Some("sonnet".into())),
            ],
        );
        assert_eq!(
            out,
            "# Perch\npoll_interval_secs = 120 # a minute\n\n# who\n[agent]\n# the default\nmodel   =   \"sonnet\"\neffort = 'high'\n\n[work]\nbranch_prefix = \"chris/\"\n"
        );
    }

    #[test]
    fn nothing_changed_writes_nothing() {
        let text = "[agent]\nmodel = \"opus\"\n";
        assert_eq!(
            edit(text, &[Setting::AgentModel(Some("opus".into()))]),
            text
        );
        assert_eq!(edit(text, &[Setting::PollIntervalSecs(None)]), text);
    }

    #[test]
    fn a_missing_file_is_made_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("home").join("config.toml");
        let written = write_settings(&path, &[Setting::PollIntervalSecs(Some(30))], &[]).unwrap();
        assert_eq!(written.config.poll_interval_secs, 30);
        assert_eq!(read(&path), "poll_interval_secs = 30\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(!path.with_extension("toml.writing").exists());
    }

    #[test]
    fn taking_a_key_out_of_a_missing_file_makes_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write_settings(&path, &[Setting::Zoom(None)], &[]).unwrap();
        assert!(!path.exists());
    }

    #[test]
    #[cfg(unix)]
    fn the_files_permissions_are_kept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "poll_interval_secs = 60\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        write_settings(&path, &[Setting::PollIntervalSecs(Some(300))], &[]).unwrap();
        assert_eq!(read(&path), "poll_interval_secs = 300\n");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640);
    }

    #[test]
    #[cfg(unix)]
    fn a_symlinked_file_is_written_through_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles.toml");
        std::fs::write(&real, "poll_interval_secs = 60\n").unwrap();
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_settings(&link, &[Setting::PollIntervalSecs(Some(30))], &[]).unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(read(&real), "poll_interval_secs = 30\n");
    }

    #[test]
    fn a_missing_table_is_made_after_the_rest() {
        let out = edit(
            "poll_interval_secs = 60\n\n[agent]\nmodel = \"opus\"\n",
            &[Setting::Zoom(Some(1.25)), Setting::CodeFontSize(Some(14.0))],
        );
        assert_eq!(
            out,
            "poll_interval_secs = 60\n\n[agent]\nmodel = \"opus\"\n\n[display]\nzoom = 1.25\ncode_font_size = 14\n"
        );
        let c = Config::parse_text(&out).unwrap();
        assert_eq!(c.display.zoom, 1.25);
        assert_eq!(c.display.code_font_size, 14.0);
    }

    #[test]
    fn a_top_level_key_goes_above_the_tables() {
        let out = edit(
            "[agent]\nmodel = \"opus\"\n",
            &[Setting::GithubHost(Some("ghe.example.com".into()))],
        );
        assert_eq!(
            out,
            "github_host = \"ghe.example.com\"\n[agent]\nmodel = \"opus\"\n"
        );
    }

    #[test]
    fn a_nested_table_comes_without_an_empty_parent() {
        let out = edit(
            "",
            &[Setting::AgentBinary {
                backend: AgentBackend::Codex,
                binary: Some("/opt/codex".into()),
            }],
        );
        assert_eq!(out, "[agent.codex]\nbinary = \"/opt/codex\"\n");
        let out = edit(
            &out,
            &[
                Setting::AgentModels {
                    backend: AgentBackend::Gemini,
                    models: Some(vec!["gemini-3-pro".into(), " ".into()]),
                },
                Setting::AgentMaxConcurrentRuns(Some(2)),
            ],
        );
        let c = Config::parse_text(&out).unwrap();
        assert_eq!(c.agent.codex.binary, "/opt/codex");
        assert_eq!(c.agent.gemini.models, vec!["gemini-3-pro".to_string()]);
        assert_eq!(c.agent.max_concurrent_runs, 2);
        assert!(
            out.contains("[agent.gemini]\nmodels = [\"gemini-3-pro\"]\n"),
            "{out}"
        );
    }

    /// The sandbox is written only to turn it off; on again, the key
    /// comes out and the default, on, holds.
    #[test]
    fn the_sandbox_is_written_off_and_taken_out_on() {
        let out = edit(
            "[agent]\nmodel = \"opus\"\n",
            &[Setting::AgentSandbox(Some(false))],
        );
        assert_eq!(out, "[agent]\nmodel = \"opus\"\nsandbox = false\n");
        assert!(!Config::parse_text(&out).unwrap().agent.sandbox);
        let out = edit(&out, &[Setting::AgentSandbox(None)]);
        assert_eq!(out, "[agent]\nmodel = \"opus\"\n");
        assert!(Config::parse_text(&out).unwrap().agent.sandbox);
    }

    #[test]
    fn a_new_key_joins_an_existing_table() {
        let out = edit(
            "[agent]\nmodel = \"opus\"\n\n[work]\nbranch_prefix = \"c/\"\n",
            &[Setting::AgentEffort(Some("max".into()))],
        );
        assert_eq!(
            out,
            "[agent]\nmodel = \"opus\"\neffort = \"max\"\n\n[work]\nbranch_prefix = \"c/\"\n"
        );
    }

    #[test]
    fn a_dotted_key_is_edited_in_place() {
        let text = "display.zoom = 1.5\npoll_interval_secs = 60\n\n[agent]\nmodel = \"opus\"\n";
        let out = edit(text, &[Setting::Zoom(Some(2.0))]);
        assert_eq!(
            out,
            "display.zoom = 2.0\npoll_interval_secs = 60\n\n[agent]\nmodel = \"opus\"\n"
        );
        let out = edit(text, &[Setting::CodeFontSize(Some(16.0))]);
        assert!(out.contains("display.code_font_size = 16\n"), "{out}");
        assert!(!out.contains("[display]"), "{out}");
        assert_eq!(
            Config::parse_text(&out).unwrap().display.code_font_size,
            16.0
        );

        let out = edit(
            "agent.codex.binary = \"codex\"\n",
            &[Setting::AgentModels {
                backend: AgentBackend::Codex,
                models: Some(vec!["gpt-6".into()]),
            }],
        );
        assert_eq!(
            out,
            "agent.codex.binary = \"codex\"\nagent.codex.models = [\"gpt-6\"]\n"
        );
    }

    #[test]
    fn an_inline_table_is_edited_in_place() {
        let out = edit(
            "display = { zoom = 1.5 }\n",
            &[Setting::Zoom(Some(1.75)), Setting::CodeFontSize(Some(13.0))],
        );
        assert_eq!(out, "display = { zoom = 1.75, code_font_size = 13 }\n");
        let out = edit(&out, &[Setting::Zoom(None), Setting::CodeFontSize(None)]);
        assert_eq!(out, "");
    }

    #[test]
    fn removing_a_key_resets_it() {
        let text = "poll_interval_secs = 30\n\n[ui]\nappearance = \"dark\"\ntheme = \"warm-linen\"\ncode_font = \"Iosevka\"\n";
        let out = edit(text, &[Setting::Appearance(None), Setting::Theme(None)]);
        assert_eq!(
            out,
            "poll_interval_secs = 30\n\n[ui]\ncode_font = \"Iosevka\"\n"
        );
        let c = Config::parse_text(&out).unwrap();
        assert_eq!(c.ui.appearance, Appearance::System);
        assert_eq!(c.ui.theme, Theme::Classic);

        let out = edit(
            &out,
            &[
                Setting::CodeFont(Some("  ".into())),
                Setting::PollIntervalSecs(None),
            ],
        );
        assert_eq!(out, "");
        assert_eq!(Config::parse_text(&out).unwrap().poll_interval_secs, 60);
    }

    #[test]
    fn emptying_a_table_keeps_its_header_when_a_comment_is_on_it() {
        let out = edit("# how big\n[display]\nzoom = 1.5\n", &[Setting::Zoom(None)]);
        assert_eq!(out, "# how big\n[display]\n");
    }

    #[test]
    fn a_comment_above_a_key_taken_out_stays_where_it_was() {
        // Onto the next key in the table.
        let out = edit(
            "[agent]\n# model = \"sonnet\"\nmodel = \"opus\"\neffort = \"high\"\n",
            &[Setting::AgentModel(None)],
        );
        assert_eq!(out, "[agent]\n# model = \"sonnet\"\neffort = \"high\"\n");

        // Onto the next header, keeping the table it was under.
        let out = edit(
            "[agent]\nmodel = \"opus\"\n# effort = \"max\"\neffort = \"high\"\n\n[work]\nbranch_prefix = \"c/\"\n",
            &[Setting::AgentEffort(None)],
        );
        assert_eq!(
            out,
            "[agent]\nmodel = \"opus\"\n# effort = \"max\"\n\n[work]\nbranch_prefix = \"c/\"\n"
        );
        let out = edit("[display]\n# big\nzoom = 1.5\n", &[Setting::Zoom(None)]);
        assert_eq!(out, "[display]\n# big\n");

        // A dotted key's comment goes to the key after it, or the first header.
        let out = edit(
            "# my zoom\ndisplay.zoom = 1.5\npoll_interval_secs = 60\n",
            &[Setting::Zoom(None)],
        );
        assert_eq!(out, "# my zoom\npoll_interval_secs = 60\n");
        let out = edit(
            "# my zoom\ndisplay.zoom = 1.5\n\n[agent]\nmodel = \"opus\"\n",
            &[Setting::Zoom(None)],
        );
        assert_eq!(out, "# my zoom\n\n[agent]\nmodel = \"opus\"\n");

        // A repository's sub-table, with another repository after it.
        let out = edit(
            "[[repo]]\ngithub = \"a/one\"\n[repo.work]\n# Let agents reach the cache\nwrite = [\"/tmp/x\"]\ntrust = true\n\n[[repo]]\ngithub = \"a/two\"\n",
            &[repo("a/one", RepoSetting::Write(vec![]))],
        );
        assert_eq!(
            out,
            "[[repo]]\ngithub = \"a/one\"\n[repo.work]\n# Let agents reach the cache\ntrust = true\n\n[[repo]]\ngithub = \"a/two\"\n"
        );
        let out = edit(
            "[[repo]]\ngithub = \"a/one\"\n[repo.work]\n# on purpose\ntrust = true\n\n[[repo]]\ngithub = \"a/two\"\n",
            &[repo("a/one", RepoSetting::WorkTrust(false))],
        );
        assert_eq!(
            out,
            "[[repo]]\ngithub = \"a/one\"\n[repo.work]\n# on purpose\n\n[[repo]]\ngithub = \"a/two\"\n"
        );

        // The last key of the file: onto the end.
        let out = edit(
            "[agent]\nmodel = \"opus\"\n# last\neffort = \"high\"\n# end\n",
            &[Setting::AgentEffort(None)],
        );
        assert_eq!(out, "[agent]\nmodel = \"opus\"\n# last\n# end\n");
    }

    #[test]
    fn a_comment_after_a_header_keeps_the_table() {
        let out = edit("[display] # sizes\nzoom = 1.5\n", &[Setting::Zoom(None)]);
        assert_eq!(out, "[display] # sizes\n");
    }

    #[test]
    fn crlf_line_ends_and_a_byte_order_mark_are_kept() {
        let text = "\u{feff}poll_interval_secs = 60\r\n[agent]\r\nmodel = \"opus\"\r\n";
        let out = edit(text, &[Setting::AgentEffort(Some("high".into()))]);
        assert_eq!(
            out,
            "\u{feff}poll_interval_secs = 60\r\n[agent]\r\nmodel = \"opus\"\r\neffort = \"high\"\r\n"
        );
        assert_eq!(edit(text, &[Setting::PollIntervalSecs(Some(60))]), text);
    }

    #[test]
    fn an_empty_list_removes_the_key() {
        let out = edit(
            "[agent]\nmodels = [\"opus\"]\n",
            &[Setting::AgentModels {
                backend: AgentBackend::Claude,
                models: Some(vec![]),
            }],
        );
        assert_eq!(out, "");
        assert_eq!(
            Config::parse_text(&out).unwrap().agent.models,
            AgentConfig::default().models
        );
    }

    #[test]
    fn every_setting_reads_back() {
        let changes = vec![
            Setting::PollIntervalSecs(Some(600)),
            Setting::GithubHost(Some("ghe.corp".into())),
            Setting::CloneRoot(Some("~/Code".into())),
            Setting::AgentModel(Some("sonnet".into())),
            Setting::AgentEffort(Some("low".into())),
            Setting::AgentMaxConcurrentRuns(Some(8)),
            Setting::AgentBinary {
                backend: AgentBackend::Claude,
                binary: Some("/bin/claude".into()),
            },
            Setting::AgentBinary {
                backend: AgentBackend::Gemini,
                binary: Some("agy2".into()),
            },
            Setting::AgentModels {
                backend: AgentBackend::Codex,
                models: Some(vec!["a".into(), "b".into()]),
            },
            Setting::WorkBranchPrefix(Some("me/".into())),
            Setting::WorkApprovals(Some(Approvals::Ask)),
            Setting::Appearance(Some(Appearance::Light)),
            Setting::Theme(Some(Theme::BlueMist)),
            Setting::CodeFont(Some("Fira Code".into())),
            Setting::Zoom(Some(0.9)),
            Setting::CodeFontSize(Some(11.0)),
            Setting::LspMaxMemoryMb(Some(4096)),
            Setting::Keybinding {
                action: "go_to_file".into(),
                keys: Some(vec!["super+t".into()]),
            },
        ];
        let (text, c) = edit_text("", &changes, &[]).unwrap();
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert_eq!(c.poll_interval_secs, 600);
        assert_eq!(c.github_host.as_deref(), Some("ghe.corp"));
        assert_eq!(c.clone_root.as_deref(), Some("~/Code"));
        assert_eq!(c.agent.model, "sonnet");
        assert_eq!(c.agent.effort, "low");
        assert_eq!(c.agent.max_concurrent_runs, 8);
        assert_eq!(c.agent.binary, "/bin/claude");
        assert_eq!(c.agent.gemini.binary, "agy2");
        assert_eq!(c.agent.codex.models, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(c.work.branch_prefix, "me/");
        assert_eq!(c.work.approvals, Approvals::Ask);
        assert_eq!(c.ui.appearance, Appearance::Light);
        assert_eq!(c.ui.theme, Theme::BlueMist);
        assert_eq!(c.ui.code_font.as_deref(), Some("Fira Code"));
        assert_eq!(c.display.zoom, 0.9);
        assert_eq!(c.display.code_font_size, 11.0);
        assert_eq!(c.lsp.max_memory_mb, 4096);
        assert_eq!(c.keybindings["go_to_file"].as_str(), Some("super+t"));

        let keys = present_keys(&text, &[]);
        for key in [
            "poll_interval_secs",
            "github_host",
            "clone_root",
            "agent.model",
            "agent.effort",
            "agent.max_concurrent_runs",
            "agent.binary",
            "agent.gemini.binary",
            "agent.codex.models",
            "work.branch_prefix",
            "work.approvals",
            "ui.appearance",
            "ui.theme",
            "ui.code_font",
            "display.zoom",
            "display.code_font_size",
            "lsp.max_memory_mb",
            "keybindings.go_to_file",
        ] {
            assert!(keys.contains(key), "{key} missing from {keys:?}");
        }
        assert_eq!(keys.len(), 18, "{keys:?}");
    }

    fn keybinding(action: &'static str, keys: Option<&[&str]>) -> Setting {
        Setting::Keybinding {
            action: action.to_string(),
            keys: keys.map(|keys| keys.iter().map(|k| k.to_string()).collect()),
        }
    }

    #[test]
    fn a_keybinding_is_one_string_or_a_list() {
        let out = edit("", &[keybinding("go_to_file", Some(&["super+t"]))]);
        assert_eq!(out, "[keybindings]\ngo_to_file = \"super+t\"\n");
        let out = edit(
            &out,
            &[keybinding("go_to_file", Some(&["super+t", " super+p "]))],
        );
        assert_eq!(
            out,
            "[keybindings]\ngo_to_file = [\"super+t\", \"super+p\"]\n"
        );
        let c = Config::parse_text(&out).unwrap();
        assert_eq!(
            c.keybindings["go_to_file"].as_array().map(Vec::len),
            Some(2)
        );
    }

    #[test]
    fn an_empty_keybinding_is_written_as_no_keys() {
        let out = edit(
            "[keybindings]\npin = \"p\"\n",
            &[keybinding("pin", Some(&[]))],
        );
        assert_eq!(out, "[keybindings]\npin = []\n");
        assert!(present_keys(&out, &[]).contains("keybindings.pin"));
        let out = edit("", &[keybinding("comment", Some(&[""]))]);
        assert_eq!(out, "[keybindings]\ncomment = []\n");
    }

    #[test]
    fn taking_a_keybinding_out_puts_its_defaults_back() {
        let text =
            "[ui]\nappearance = \"dark\"\n\n[keybindings]\npin = []\n# mine\ncomment = \"c\"\n";
        let out = edit(text, &[keybinding("pin", None)]);
        assert_eq!(
            out,
            "[ui]\nappearance = \"dark\"\n\n[keybindings]\n# mine\ncomment = \"c\"\n"
        );
        let out = edit(
            "[ui]\nappearance = \"dark\"\n\n[keybindings]\ncomment = \"c\"\n",
            &[keybinding("comment", None)],
        );
        assert_eq!(out, "[ui]\nappearance = \"dark\"\n");
    }

    const REPOS: &str = "[[repo]]\ngithub = \"a/one\"\n\n# notes\n[[repo]]\nroot = \"/srv/notes\"\n\n[[repo]]\ngithub = \"a/two\"\nbase = \"main\"\n[repo.work]\napprovals = \"ask\"\n\n[agent]\nmodel = \"opus\"\n";

    #[test]
    fn a_repository_key_lands_on_its_own_entry() {
        let out = edit(
            REPOS,
            &[repo("A/Two", RepoSetting::Base(Some("trunk".into())))],
        );
        assert_eq!(out, REPOS.replace("base = \"main\"", "base = \"trunk\""));

        let out = edit(
            REPOS,
            &[repo("a/one", RepoSetting::Base(Some("dev".into())))],
        );
        assert_eq!(
            out,
            REPOS.replace(
                "github = \"a/one\"\n",
                "github = \"a/one\"\nbase = \"dev\"\n"
            )
        );
    }

    #[test]
    fn history_sync_is_opt_in_for_resolved_github_repositories() {
        let text = "[[repo]]\ngithub = \"a/one\"\n\n[[repo]]\nroot = \"/srv/two\"\n\n[[repo]]\nroot = \"/srv/notes\"\nsync_history = true\n";
        let mut config = Config::parse_text(text).unwrap();
        config.repo[1].resolve(Some("a/two".into()));
        config.repo[2].resolve(None);
        assert_eq!(config.history_sync_repos().count(), 0);

        let (edited, _) = edit_text(
            text,
            &[repo("A/Two", RepoSetting::SyncHistory(true))],
            &config.repo,
        )
        .unwrap();
        let mut enabled = Config::parse_text(&edited).unwrap();
        enabled.repo[1].resolve(Some("a/two".into()));
        enabled.repo[2].resolve(None);
        assert_eq!(enabled.history_sync_repos().collect::<Vec<_>>(), ["a/two"]);
        assert!(present_keys(&edited, &config.repo).contains("repo.a/two.sync_history"));

        let (disabled, _) = edit_text(
            &edited,
            &[repo("a/two", RepoSetting::SyncHistory(false))],
            &config.repo,
        )
        .unwrap();
        assert_eq!(disabled, text);
    }

    #[test]
    fn a_repository_sub_table_is_made_under_its_own_entry() {
        let out = edit(
            REPOS,
            &[
                repo("a/one", RepoSetting::WorkTrust(true)),
                repo("a/one", RepoSetting::Network(vec!["crates.io".into()])),
                repo("local/notes", RepoSetting::LspTrust(true)),
                repo("a/two", RepoSetting::Write(vec!["/tmp/cache".into()])),
            ],
        );
        assert_eq!(
            out,
            "[[repo]]\ngithub = \"a/one\"\n\n[repo.work]\ntrust = true\nnetwork = [\"crates.io\"]\n\n# notes\n[[repo]]\nroot = \"/srv/notes\"\n\n[repo.lsp]\ntrust = true\n\n[[repo]]\ngithub = \"a/two\"\nbase = \"main\"\n[repo.work]\napprovals = \"ask\"\nwrite = [\"/tmp/cache\"]\n\n[agent]\nmodel = \"opus\"\n"
        );
        let mut c = Config::parse_text(&out).unwrap();
        assert!(c.repo[0].work.trust);
        assert_eq!(c.repo[0].work.network, vec!["crates.io".to_string()]);
        assert!(c.repo[1].lsp.trust);
        c.repo[1].resolve(None);
        assert!(c.lsp_trusted("local/notes"));
        assert!(!c.lsp_trusted("a/one"));
        assert_eq!(c.repo[2].work.write, vec!["/tmp/cache".to_string()]);
        assert_eq!(c.approvals_for("a/two"), Approvals::Ask);
    }

    #[test]
    fn a_switch_turned_off_is_taken_out_with_its_empty_table() {
        let text = "[[repo]]\ngithub = \"a/one\"\n[repo.work]\ntrust = true\n\n[[repo]]\ngithub = \"a/two\"\n";
        let out = edit(text, &[repo("a/one", RepoSetting::WorkTrust(false))]);
        assert_eq!(
            out,
            "[[repo]]\ngithub = \"a/one\"\n\n[[repo]]\ngithub = \"a/two\"\n"
        );
        let out = edit(
            "[[repo]]\ngithub = \"a/one\"\n[repo.work]\ntrust = true\napprovals = \"never\"\n",
            &[
                repo("a/one", RepoSetting::WorkTrust(false)),
                repo("a/one", RepoSetting::Approvals(None)),
            ],
        );
        assert_eq!(out, "[[repo]]\ngithub = \"a/one\"\n");
    }

    #[test]
    fn a_root_only_repository_is_found_by_the_identity_its_scan_gave() {
        let text = "[[repo]]\nroot = \"/srv/perch\"\n\n[[repo]]\nroot = \"/srv/notes\"\n";
        let mut known = Config::parse_text(text).unwrap().repo;
        known[0].resolve(Some("chris/perch".into()));
        known[1].resolve(None);
        let (out, _) = edit_text(
            text,
            &[
                repo("Chris/Perch", RepoSetting::Base(Some("main".into()))),
                repo("local/notes", RepoSetting::Base(Some("trunk".into()))),
            ],
            &known,
        )
        .unwrap();
        assert_eq!(
            out,
            "[[repo]]\nroot = \"/srv/perch\"\nbase = \"main\"\n\n[[repo]]\nroot = \"/srv/notes\"\nbase = \"trunk\"\n"
        );
        // Without the scan's answer it is only local/perch.
        assert!(edit_text(text, &[repo("chris/perch", RepoSetting::Base(None))], &[]).is_err());
    }

    #[test]
    fn an_unknown_repository_is_refused() {
        let err =
            edit_text(REPOS, &[repo("a/three", RepoSetting::WorkTrust(true))], &[]).unwrap_err();
        assert!(err.contains("a/three"), "{err}");
        let err = edit_text("", &[repo("a/one", RepoSetting::WorkTrust(true))], &[]).unwrap_err();
        assert!(err.contains("a/one"), "{err}");
    }

    #[test]
    fn a_rename_onto_another_repositorys_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, REPOS);
        let err = write_settings(
            &path,
            &[repo("a/one", RepoSetting::Name(Some("two".into())))],
            &[],
        )
        .unwrap_err();
        assert!(err.contains("stop config.toml loading a/two"), "{err}");
        assert!(err.contains("would both show as \"two\""), "{err}");
        assert_eq!(read(&path), REPOS);

        let written = write_settings(
            &path,
            &[repo("a/one", RepoSetting::Name(Some("First".into())))],
            &[],
        )
        .unwrap();
        assert_eq!(written.config.display_name("a/one"), "First");
    }

    #[test]
    fn a_rename_that_brings_back_a_skipped_entry_is_made() {
        // b/x is skipped for showing as "x" too; naming a/x settles it.
        let text = "[[repo]]\ngithub = \"a/x\"\n\n[[repo]]\ngithub = \"b/x\"\n";
        assert_eq!(Config::parse_text(text).unwrap().repo.len(), 1);
        let (out, c) = edit_text(
            text,
            &[repo("a/x", RepoSetting::Name(Some("Ax".into())))],
            &[],
        )
        .unwrap();
        assert_eq!(
            out,
            "[[repo]]\ngithub = \"a/x\"\nname = \"Ax\"\n\n[[repo]]\ngithub = \"b/x\"\n"
        );
        assert_eq!(c.repo.len(), 2);
    }

    #[test]
    fn a_rename_that_swaps_which_entries_load_is_refused() {
        // Same count either way, but c/z would give way to b/x.
        let text = "[[repo]]\ngithub = \"a/x\"\n\n[[repo]]\ngithub = \"b/x\"\n\n[[repo]]\ngithub = \"c/z\"\nname = \"Bar\"\n";
        let err = edit_text(
            text,
            &[repo("a/x", RepoSetting::Name(Some("Bar".into())))],
            &[],
        )
        .unwrap_err();
        assert!(err.contains("stop config.toml loading c/z"), "{err}");
    }

    #[test]
    fn a_result_that_would_not_load_is_refused_and_the_file_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, REPOS);
        // `local` is the owner reserved for repositories with no remote, so
        // a name that reads as one would drop the entry.
        let err = write_settings(
            &path,
            &[repo(
                "local/notes",
                RepoSetting::Name(Some("local/x".into())),
            )],
            &[],
        )
        .unwrap_err();
        assert!(
            err.contains("stop config.toml loading local/notes"),
            "{err}"
        );
        assert!(write_settings(&path, &[Setting::Zoom(Some(f64::NAN))], &[]).is_err());
        assert_eq!(read(&path), REPOS);
        assert!(!path.with_extension("toml.writing").exists());
    }

    #[test]
    fn a_value_where_a_table_goes_is_refused() {
        // The loader refuses these first; the writer's own check is a
        // second line behind it.
        for text in ["[agent]\ncodex = 1\n", "[ui]\n[ui.appearance]\nx = 1\n"] {
            let err = edit_text(
                text,
                &[
                    Setting::AgentBinary {
                        backend: AgentBackend::Codex,
                        binary: Some("x".into()),
                    },
                    Setting::Appearance(Some(Appearance::Dark)),
                ],
                &[],
            )
            .unwrap_err();
            assert!(err.contains("doesn't load"), "{err}");
        }
        let mut doc: DocumentMut = "[agent]\ncodex = 1\n".parse().unwrap();
        let err = apply(
            &mut doc,
            &Setting::AgentBinary {
                backend: AgentBackend::Codex,
                binary: Some("x".into()),
            },
            &[],
        )
        .unwrap_err();
        assert!(err.contains("not a table"), "{err}");
        let mut doc: DocumentMut = "[ui]\n[ui.appearance]\nx = 1\n".parse().unwrap();
        let err = apply(&mut doc, &Setting::Appearance(Some(Appearance::Dark)), &[]).unwrap_err();
        assert!(err.contains("not a value"), "{err}");
    }

    #[test]
    fn a_file_that_loads_wrong_is_refused() {
        let text = "agent = \"claude\"\n";
        let err = edit_text(text, &[Setting::PollIntervalSecs(Some(30))], &[]).unwrap_err();
        assert!(err.contains("doesn't load"), "{err}");
        let err = edit_text(
            "[agent]\nmodels = 3\n",
            &[Setting::AgentModel(Some("x".into()))],
            &[],
        )
        .unwrap_err();
        assert!(err.contains("line 2"), "{err}");
    }

    #[test]
    fn a_file_that_will_not_parse_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let text = "poll_interval_secs = 60\n[agent\nmodel = \"opus\"\n";
        let path = write(&dir, text);
        let err = write_settings(&path, &[Setting::PollIntervalSecs(Some(30))], &[]).unwrap_err();
        assert!(err.starts_with("config.toml doesn't parse"), "{err}");
        assert!(err.contains("line 2"), "{err}");
        assert_eq!(read(&path), text);
        assert!(!path.with_extension("toml.writing").exists());
    }

    #[test]
    fn warnings_come_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "");
        let written = write_settings(&path, &[Setting::Zoom(Some(3.0))], &[]).unwrap();
        assert_eq!(written.config.display.zoom, 2.0);
        assert_eq!(written.warnings.len(), 1, "{:?}", written.warnings);
        assert_eq!(read(&path), "[display]\nzoom = 3.0\n");
    }

    #[test]
    fn the_client_s_settings_are_told_from_the_engine_s() {
        assert!(Setting::Zoom(Some(1.2)).is_client_s());
        assert!(Setting::RemoteHost(Some("halo".into())).is_client_s());
        assert!(Setting::Keybinding {
            action: "x".into(),
            keys: None
        }
        .is_client_s());
        assert!(!Setting::PollIntervalSecs(Some(30)).is_client_s());
        assert!(!Setting::AgentModel(None).is_client_s());
        assert!(is_client_key("display.zoom"));
        assert!(is_client_key("keybindings.go-to-file"));
        assert!(is_client_key("remote.host"));
        assert!(!is_client_key("agent.codex.binary"));
        assert!(!is_client_key("repo.a/ui.root"));
    }

    #[test]
    fn a_remote_host_is_written_and_a_blank_one_taken_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write_settings(&path, &[Setting::RemoteHost(Some(" halo ".into()))], &[]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let config = Config::parse_text(&text).unwrap();
        assert_eq!(config.remote.host(), Some("halo"));
        // No command: the app installs and runs its own perchd there.
        assert_eq!(config.remote.command(), None);
        assert!(present_keys(&text, &[]).contains("remote.host"));
        write_settings(&path, &[Setting::RemoteHost(Some("  ".into()))], &[]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(Config::parse_text(&text).unwrap().remote.host(), None);
    }

    #[test]
    fn present_keys_name_repositories_by_identity() {
        let text = "poll_interval_secs = 30\n[agent.codex]\nbinary = \"codex\"\n\n[[repo]]\nroot = \"/srv/perch\"\n[repo.work]\ntrust = true\n\n[[repo]]\ngithub = \"A/Two\"\nbase = \"main\"\n[repo.lsp]\ntrust = true\n\n[[repo]]\nroot = \"/srv/Notes\"\n";
        let mut known = Config::parse_text(text).unwrap().repo;
        known[0].resolve(Some("chris/perch".into()));
        let keys = present_keys(text, &known);
        let want: BTreeSet<String> = [
            "poll_interval_secs",
            "agent.codex.binary",
            "repo.chris/perch.root",
            "repo.chris/perch.work.trust",
            "repo.a/two.github",
            "repo.a/two.base",
            "repo.a/two.lsp.trust",
            "repo.local/notes.root",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(keys, want);
        assert!(present_keys("[agent", &[]).is_empty());
    }

    #[test]
    fn config_read_says_why_rather_than_falling_back() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("none.toml");
        assert_eq!(Config::read(&missing).unwrap().poll_interval_secs, 60);
        let path = write(&dir, "poll_interval_secs = \"soon\"\n");
        let err = Config::read(&path).unwrap_err();
        assert!(err.starts_with("line 1: "), "{err}");
        let path = write(&dir, "poll_interval_secs = 90\n");
        assert_eq!(Config::read(&path).unwrap().poll_interval_secs, 90);
    }
}
