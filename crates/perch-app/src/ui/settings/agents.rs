//! Agents: the default model, effort, how many runs at once, and whether
//! Claude Code and Gemini run commands in their sandboxes, then a card for
//! each CLI with its executable and its models.
//!
//! Two configs meet here. What the file says (`Page::with_config`) is what
//! the boxes hold, binaries as typed. What Perch found on this machine
//! (`AppState::agent`) is what the descriptions report and what the model
//! dropdown offers, as the composer's picker does.

use std::path::Path;

use floem::prelude::*;
use perch_github::{AgentBackend, AgentConfig, Setting};

use super::controls::{dropdown, list_line, select, setting_text, split_list, toggle};
use super::{Group, Page, Row};
use crate::review::threads::{model_groups, model_label};
use crate::ui::menu::Menu;

/// The limits on runs at once offered, besides the file's own.
const MOST_RUNS: usize = 8;

pub(super) fn groups(page: &Page) -> Vec<Group> {
    let mut groups = vec![Group::new(
        "Defaults",
        vec![
            model(page),
            effort(page),
            concurrent_runs(page),
            sandbox(page),
        ],
    )];
    groups.extend(AgentBackend::ALL.into_iter().map(|backend| {
        Group::new(
            title(backend),
            vec![binary(page, backend), models(page, backend)],
        )
    }));
    groups
}

fn title(backend: AgentBackend) -> &'static str {
    match backend {
        AgentBackend::Claude => "Claude Code",
        AgentBackend::Codex => "Codex",
        AgentBackend::Gemini => "Gemini (Antigravity)",
        AgentBackend::OpenCode => "OpenCode",
    }
}

/// The word a row's name and its control's debug name use for a backend.
fn slug(backend: AgentBackend) -> &'static str {
    match backend {
        AgentBackend::Claude => "claude",
        AgentBackend::Codex => "codex",
        AgentBackend::Gemini => "gemini",
        AgentBackend::OpenCode => "opencode",
    }
}

/// Where the file keeps a backend's keys, as `present_keys` names them.
fn table(backend: AgentBackend) -> &'static str {
    match backend {
        AgentBackend::Claude => "agent",
        AgentBackend::Codex => "agent.codex",
        AgentBackend::Gemini => "agent.gemini",
        AgentBackend::OpenCode => "agent.opencode",
    }
}

/// A backend's executable and models in `agent`.
fn parts(agent: &AgentConfig, backend: AgentBackend) -> (&str, Option<&str>, &[String]) {
    match backend {
        AgentBackend::Claude => (
            &agent.binary,
            agent.binary_version.as_deref(),
            &agent.models,
        ),
        AgentBackend::Codex => (
            &agent.codex.binary,
            agent.codex.binary_version.as_deref(),
            &agent.codex.models,
        ),
        AgentBackend::OpenCode => (
            &agent.opencode.binary,
            agent.opencode.binary_version.as_deref(),
            &agent.opencode.models,
        ),
        AgentBackend::Gemini => (
            &agent.gemini.binary,
            agent.gemini.binary_version.as_deref(),
            &agent.gemini.models,
        ),
    }
}

/// What Perch found for a backend: where its executable is and which
/// version, or that it isn't there. Discovery turns a name it finds into
/// the full path; one it doesn't find keeps the name as typed.
pub(super) fn found(page: &Page, backend: AgentBackend) -> String {
    let typed = page.with_config(|c| parts(&c.agent, backend).0.to_string());
    page.state.agent.with(|agent| {
        // The agent config before discovery has said anything.
        if *agent == AgentConfig::default() {
            return "Looking for it…".to_string();
        }
        let (binary, version, _) = parts(agent, backend);
        let path = Path::new(binary);
        // A full path typed is kept as it is, found or not. Whether it is
        // there is the engine's machine's to say: with the engine on a
        // host, by the version it got from running it.
        let there = if page.state.place.get().host().is_some() {
            version.is_some()
        } else {
            path.is_file()
        };
        if path.is_absolute() && (binary != typed || there) {
            match version {
                Some(version) => {
                    format!("Found {}, {version}.", page.state.place.get().tilde(path))
                }
                None => format!("Found {}.", page.state.place.get().tilde(path)),
            }
        } else {
            format!("Not found as {binary}. Give its full path if it isn't on PATH.")
        }
    })
}

fn model(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            let (label_page, menu_page, enabled) = (page.clone(), page.clone(), page.clone());
            select(
                move || label_page.with_config(|c| model_label(&c.agent.model)),
                move || model_menu(&menu_page),
                move || enabled.writable(),
                "settings-agent-model",
            )
            .into_any()
        }
    };
    Row::new(
        "agent-model",
        "Default model",
        || "What a new conversation or run starts on.".to_string(),
        control,
    )
    .keywords(&[
        "model",
        "claude",
        "codex",
        "gemini",
        "opencode",
        "agent.model",
    ])
    .reset("agent.model", Setting::AgentModel(None))
}

/// The models Perch found, a section for each agent as the composer's
/// picker has them, and the file's own model at the end when none of them
/// is it.
fn model_menu(page: &Page) -> Menu {
    let current = page.with_config(|c| c.agent.model.clone());
    let offered = page.state.agent.with(model_groups);
    let mut listed = false;
    let mut menu = Menu::new();
    for group in offered {
        menu = menu.separator();
        for (backend, model) in group {
            let checked = model == current;
            listed |= checked;
            let write = page.clone();
            menu = menu.item(model_label(&model), move |item| {
                item.checked(checked)
                    .detail(backend.label())
                    .action(move || write.write(Setting::AgentModel(Some(model.clone()))))
            });
        }
    }
    if !listed {
        menu = menu.separator().item(model_label(&current), |item| {
            item.checked(true)
                .enabled(false)
                .note("Not offered: its agent wasn't found")
        });
    }
    menu
}

fn effort(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            dropdown(
                &page,
                |page| {
                    let current = page.with_config(|c| c.agent.effort.clone());
                    let mut offered: Vec<(String, String)> = perch_agent::Effort::ALL
                        .into_iter()
                        .map(|e| (e.label().to_string(), e.name().to_string()))
                        .collect();
                    match perch_agent::Effort::parse(&current) {
                        Some(e) => (offered, e.name().to_string()),
                        // A word Perch doesn't know, as the file has it.
                        None => {
                            offered.push((current.clone(), current.clone()));
                            (offered, current)
                        }
                    }
                },
                |name| Setting::AgentEffort(Some(name)),
                "settings-agent-effort",
            )
        }
    };
    Row::new(
        "agent-effort",
        "Default effort",
        || "How hard the model thinks, unless you pick otherwise.".to_string(),
        control,
    )
    .keywords(&["effort", "thinking", "reasoning", "agent.effort"])
    .reset("agent.effort", Setting::AgentEffort(None))
}

fn concurrent_runs(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            dropdown(
                &page,
                |page| {
                    let current = page.with_config(|c| c.agent.max_concurrent_runs);
                    let mut offered: Vec<usize> = (1..=MOST_RUNS).collect();
                    if !offered.contains(&current) {
                        offered.push(current);
                        offered.sort_unstable();
                    }
                    let offered = offered.into_iter().map(|n| (n.to_string(), n)).collect();
                    (offered, current)
                },
                |n| Setting::AgentMaxConcurrentRuns(Some(n)),
                "settings-agent-runs",
            )
        }
    };
    Row::new(
        "agent-runs",
        "Runs at once",
        || {
            "How many agent runs go at the same time. The rest wait their turn, and Gemini \
             runs one at a time."
                .to_string()
        },
        control,
    )
    .keywords(&[
        "concurrent",
        "parallel",
        "queue",
        "agent.max_concurrent_runs",
    ])
    .reset(
        "agent.max_concurrent_runs",
        Setting::AgentMaxConcurrentRuns(None),
    )
}

fn binary(page: &Page, backend: AgentBackend) -> Row {
    let key = format!("{}.binary", table(backend));
    let control = {
        let page = page.clone();
        let key = key.clone();
        move || {
            let (value_page, write) = (page.clone(), page.clone());
            let key = key.clone();
            // Blank while the file doesn't name one: the placeholder says
            // what is looked for instead.
            setting_text(
                &page,
                move || {
                    if value_page.present(&key) {
                        value_page.with_config(|c| parts(&c.agent, backend).0.to_string())
                    } else {
                        String::new()
                    }
                },
                default_binary(backend),
                move |typed| {
                    let binary = typed.trim();
                    write.write(Setting::AgentBinary {
                        backend,
                        binary: (!binary.is_empty()).then(|| binary.to_string()),
                    })
                },
                format!("settings-{}-binary", slug(backend)),
            )
            .into_any()
        }
    };
    let described = page.clone();
    Row::new(
        format!("{}-binary", slug(backend)),
        "Executable",
        move || found(&described, backend),
        control,
    )
    .keywords(&["binary", "path", "cli", "install", "executable"])
    .reset(
        key,
        Setting::AgentBinary {
            backend,
            binary: None,
        },
    )
}

/// `[agent] sandbox`: on unless the file says `false`. Turned back on, the
/// key comes out of the file.
fn sandbox(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            let (on_page, enabled, write) = (page.clone(), page.clone(), page.clone());
            toggle(
                move || on_page.with_config(|c| c.agent.sandbox),
                move || enabled.writable(),
                move |on| write.write(Setting::AgentSandbox((!on).then_some(false))),
                "settings-agent-sandbox",
            )
            .into_any()
        }
    };
    Row::new(
        "agent-sandbox",
        "Sandbox",
        || {
            "Run Claude Code's and Gemini's commands in their sandboxes (bubblewrap and socat \
             for Claude Code on Linux, agy's terminal sandbox for Gemini). Turn it off only \
             where the machine is itself the boundary, such as a throwaway VM or container. \
             Off, Claude Code Work asks before every command, and Claude Code and Gemini \
             questions and reviews run their read-only commands without that backstop."
                .to_string()
        },
        control,
    )
    .keywords(&[
        "sandbox",
        "bubblewrap",
        "bwrap",
        "socat",
        "gemini",
        "agy",
        "container",
        "vm",
        "agent.sandbox",
    ])
    .reset("agent.sandbox", Setting::AgentSandbox(None))
}

fn default_binary(backend: AgentBackend) -> &'static str {
    match backend {
        AgentBackend::Claude => "claude",
        AgentBackend::Codex => "codex",
        AgentBackend::Gemini => "agy",
        AgentBackend::OpenCode => "opencode",
    }
}

fn models(page: &Page, backend: AgentBackend) -> Row {
    let key = format!("{}.models", table(backend));
    let control = {
        let page = page.clone();
        let key = key.clone();
        move || {
            let (value_page, write) = (page.clone(), page.clone());
            let key = key.clone();
            setting_text(
                &page,
                move || {
                    if value_page.present(&key) {
                        value_page.with_config(|c| list_line(parts(&c.agent, backend).2))
                    } else {
                        String::new()
                    }
                },
                match backend {
                    AgentBackend::Claude => "fable, opus, sonnet, haiku",
                    AgentBackend::Codex => "Codex's catalog",
                    AgentBackend::Gemini => "agy's catalog",
                    AgentBackend::OpenCode => "OpenCode's catalog",
                },
                move |typed| {
                    let models = split_list(&typed);
                    write.write(Setting::AgentModels {
                        backend,
                        models: (!models.is_empty()).then_some(models),
                    })
                },
                format!("settings-{}-models", slug(backend)),
            )
            .into_any()
        }
    };
    let description = match backend {
        AgentBackend::Claude => "The models the picker offers, between commas.",
        AgentBackend::Codex => {
            "The models the picker offers, between commas. Left empty, Codex's own catalog."
        }
        AgentBackend::OpenCode => {
            "Provider/model IDs, between commas. Left empty, OpenCode's own catalog."
        }
        AgentBackend::Gemini => {
            "The models the picker offers, between commas. Left empty, agy's own catalog."
        }
    };
    Row::new(
        format!("{}-models", slug(backend)),
        "Models",
        move || description.to_string(),
        control,
    )
    .keywords(&["model", "list", "picker", "catalog"])
    .reset(
        key,
        Setting::AgentModels {
            backend,
            models: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::super::tests::{
        click, enter_in, goes_dark, open_on, pick, settle, shown, window, written,
    };
    use super::super::Section;
    use super::*;
    use floem::reactive::SignalUpdate;

    /// The agent config as discovery leaves it: Claude Code found by name,
    /// Codex with its catalog, agy nowhere.
    fn discovered() -> AgentConfig {
        let mut agent = AgentConfig {
            binary: "/usr/local/bin/claude".into(),
            binary_version: Some("2.1.3 (Claude Code)".into()),
            ..AgentConfig::default()
        };
        agent.codex.binary = "/opt/perch-test/codex".into();
        agent.codex.models = vec!["gpt-6.1-sol".into(), "gpt-5.5".into()];
        agent.gemini.models.clear();
        agent
    }

    const FILE: &str = "[agent]\nmodel = \"sonnet\"\neffort = \"max\"\n\n\
                        [agent.codex]\nbinary = \"/opt/perch-test/codex\"\n";

    #[test]
    fn the_agents_say_what_was_found_and_offer_what_was_found() {
        let (mut harness, state, mut commands) = window(1400.0, 1400.0, FILE);
        let page = Page::new(state, std::rc::Rc::new(crate::backend::Backend::detached()));
        assert_eq!(found(&page, AgentBackend::Claude), "Looking for it…");

        state.agent.set(discovered());
        open_on(&mut harness, state, Section::Agents);
        for row in [
            "agent-model",
            "agent-effort",
            "agent-runs",
            "agent-sandbox",
            "claude-binary",
            "claude-models",
            "codex-binary",
            "codex-models",
            "gemini-binary",
            "gemini-models",
        ] {
            assert!(shown(&harness, &format!("settings-row:{row}")), "{row}");
        }
        assert!(shown(&harness, "settings-group:Gemini (Antigravity)"));
        assert_eq!(
            found(&page, AgentBackend::Claude),
            "Found /usr/local/bin/claude, 2.1.3 (Claude Code)."
        );
        // Named in the file as a path that isn't there.
        assert_eq!(
            found(&page, AgentBackend::Codex),
            "Not found as /opt/perch-test/codex. Give its full path if it isn't on PATH."
        );
        assert!(found(&page, AgentBackend::Gemini).starts_with("Not found as agy."));
        // With the engine on a host, a full path is the host's: whether it
        // is there is what the engine got from running it, not this Mac's
        // files.
        state.place.set(crate::remote::Place::Host {
            host: "halo".into(),
            home: std::path::PathBuf::new(),
            data_root: std::path::PathBuf::new(),
        });
        state
            .agent
            .update(|agent| agent.codex.binary_version = Some("codex 9".into()));
        assert_eq!(
            found(&page, AgentBackend::Codex),
            "Found /opt/perch-test/codex, codex 9."
        );
        state
            .agent
            .update(|agent| agent.codex.binary_version = None);
        state.place.set(crate::remote::Place::ThisMac);

        // The file's choices, with resets where it makes them.
        assert!(shown(&harness, "settings-reset:agent-model"));
        assert!(shown(&harness, "settings-reset:codex-binary"));
        assert!(!shown(&harness, "settings-reset:claude-binary"));
        assert!(!shown(&harness, "settings-reset:agent-runs"));

        // The model dropdown offers what was found, Codex's after Claude's.
        pick(&mut harness, "settings-agent-model", "Sol 6.1");
        assert_eq!(
            written(&mut commands),
            vec![Setting::AgentModel(Some("gpt-6.1-sol".into()))]
        );
        pick(&mut harness, "settings-agent-effort", "Low");
        assert_eq!(
            written(&mut commands),
            vec![Setting::AgentEffort(Some("low".into()))]
        );
        pick(&mut harness, "settings-agent-runs", "6");
        assert_eq!(
            written(&mut commands),
            vec![Setting::AgentMaxConcurrentRuns(Some(6))]
        );

        // The sandbox is on while the file says nothing, and turning it
        // off writes `false`.
        assert!(!shown(&harness, "settings-reset:agent-sandbox"));
        click(&mut harness, "settings-agent-sandbox");
        assert_eq!(
            written(&mut commands),
            vec![Setting::AgentSandbox(Some(false))]
        );

        // An executable box is blank while the file names none.
        enter_in(&mut harness, "settings-claude-binary", "claudex");
        assert_eq!(
            written(&mut commands),
            vec![Setting::AgentBinary {
                backend: AgentBackend::Claude,
                binary: Some("claudex".into()),
            }]
        );
        enter_in(&mut harness, "settings-gemini-models", "flash");
        assert_eq!(
            written(&mut commands),
            vec![Setting::AgentModels {
                backend: AgentBackend::Gemini,
                models: Some(vec!["flash".into()]),
            }]
        );

        click(&mut harness, "settings-reset:codex-binary");
        assert_eq!(
            written(&mut commands),
            vec![Setting::AgentBinary {
                backend: AgentBackend::Codex,
                binary: None,
            }]
        );
        settle(&mut harness);
    }

    /// A model the file names that no agent found stays in the menu,
    /// checked and greyed.
    #[test]
    fn a_model_nothing_offers_is_kept_in_the_menu() {
        let (_harness, state, _commands) = window(800.0, 600.0, "[agent]\nmodel = \"mystery\"\n");
        state.agent.set(discovered());
        let page = Page::new(state, std::rc::Rc::new(crate::backend::Backend::detached()));
        let outline = model_menu(&page).outline();
        assert_eq!(outline.last().map(String::as_str), Some("✓ (Mystery)"));
        assert!(outline.iter().any(|row| row.contains("Sol 6.1")));
    }

    /// Off in the file, the switch is off with a reset, and turning it
    /// back on takes the key out.
    #[test]
    fn a_sandbox_turned_off_is_turned_on_by_taking_the_key_out() {
        let (mut harness, state, mut commands) =
            window(1400.0, 1400.0, "[agent]\nsandbox = false\n");
        state.agent.set(discovered());
        open_on(&mut harness, state, Section::Agents);
        assert!(shown(&harness, "settings-reset:agent-sandbox"));
        click(&mut harness, "settings-agent-sandbox");
        assert_eq!(written(&mut commands), vec![Setting::AgentSandbox(None)]);
        settle(&mut harness);
    }

    #[test]
    fn agents_go_dark_with_the_system() {
        let (mut harness, state, _commands) = window(1400.0, 1400.0, FILE);
        state.agent.set(discovered());
        open_on(&mut harness, state, Section::Agents);
        goes_dark(&mut harness);
    }
}
