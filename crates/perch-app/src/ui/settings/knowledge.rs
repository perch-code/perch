//! The engine's external knowledge directories and their repository scope.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Effect, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{Decorators, Stack};
use perch_github::config::KnowledgeSource;
use perch_github::{KnowledgeSetting, Setting};

use super::controls::{button, entry_input, list_line, setting_text, split_list, toggle};
use super::{Group, Page, Row};

pub(super) fn groups(page: &Page) -> Vec<Group> {
    let mut groups = page.with_config(|config| {
        config
            .knowledge
            .iter()
            .map(|source| {
                let mut rows = Vec::new();
                rows.extend(status(page, &source.dir));
                rows.push(all_repositories(page, &source.dir));
                // A list to fill in only once it is not every repository,
                // so clearing it narrows the directory rather than widening it.
                if source.repos.is_some() {
                    rows.push(repositories(page, &source.dir));
                }
                rows.push(remove(page, &source.dir));
                Group::new(source.dir.display().to_string(), rows)
            })
            .collect::<Vec<_>>()
    });
    groups.push(Group::new("Add a knowledge directory", vec![add(page)]));
    groups
}

fn setting(dir: &Path, change: KnowledgeSetting) -> Setting {
    Setting::Knowledge {
        dir: dir.to_path_buf(),
        change,
    }
}

fn repos_of(page: &Page, dir: &Path) -> Option<Vec<String>> {
    page.with_config(|config| {
        config
            .knowledge
            .iter()
            .find(|source| source.dir == dir)
            .and_then(|source| source.repos.clone())
    })
}

/// The add box's repositories: blank there is every repository, as its
/// placeholder says. Editing a directory's list never reads blank so.
fn repos_from_line(line: &str) -> Option<Vec<String>> {
    (!line.trim().is_empty()).then(|| split_list(line))
}

/// Why the engine does not give runs the directory, when it does not.
fn problem_of(page: &Page, dir: &Path) -> Option<String> {
    page.state.settings_file.with(|file| {
        file.as_ref()
            .and_then(|file| file.knowledge_problems.get(dir).cloned())
    })
}

/// A row saying why runs are not given the directory, only while that is
/// so.
fn status(page: &Page, dir: &Path) -> Option<Row> {
    problem_of(page, dir)?;
    let (page, dir) = (page.clone(), dir.to_path_buf());
    Some(
        Row::new(
            format!("knowledge:{}:status", dir.display()),
            "Not shared with runs",
            move || match problem_of(&page, &dir) {
                Some(why) => format!(
                    "knowledge/{}: {why}. Perch looks again as runs start.",
                    dir.display()
                ),
                None => "Shared with runs.".into(),
            },
            || floem::views::Empty::new().into_any(),
        )
        .keywords(&["knowledge", "status", "symlink", "missing"]),
    )
}

fn all_repositories(page: &Page, dir: &Path) -> Row {
    let name = format!("knowledge:{}:all", dir.display());
    let debug = format!("settings-{name}");
    let (description_page, description_dir) = (page.clone(), dir.to_path_buf());
    let (control_page, control_dir) = (page.clone(), dir.to_path_buf());
    Row::new(
        name,
        "All repositories",
        move || match repos_of(&description_page, &description_dir) {
            None => "Runs in every repository are given this directory.".into(),
            Some(_) => "Runs are given this directory only in the repositories listed.".into(),
        },
        move || {
            let (on_page, on_dir) = (control_page.clone(), control_dir.clone());
            let enabled = control_page.clone();
            let (write_page, write_dir) = (control_page.clone(), control_dir.clone());
            toggle(
                move || repos_of(&on_page, &on_dir).is_none(),
                move || enabled.writable(),
                // Off starts from no repository, not from every one.
                move |on| {
                    write_page.write(setting(
                        &write_dir,
                        KnowledgeSetting::Repositories((!on).then(Vec::new)),
                    ))
                },
                debug.clone(),
            )
            .into_any()
        },
    )
    .keywords(&["knowledge", "scope", "repos", "every"])
}

fn repositories(page: &Page, dir: &Path) -> Row {
    let name = format!("knowledge:{}:repos", dir.display());
    let debug = format!("settings-{name}");
    let (description_page, description_dir) = (page.clone(), dir.to_path_buf());
    let (control_page, control_dir) = (page.clone(), dir.to_path_buf());
    Row::new(
        name,
        "Repositories",
        move || match repos_of(&description_page, &description_dir) {
            Some(repos) if repos.is_empty() => {
                "No repository yet. Enter owner/repo names separated by commas.".into()
            }
            _ => "Repository names, separated by commas.".into(),
        },
        move || {
            let (value_page, value_dir) = (control_page.clone(), control_dir.clone());
            let (write_page, write_dir) = (control_page.clone(), control_dir.clone());
            setting_text(
                &control_page,
                move || list_line(&repos_of(&value_page, &value_dir).unwrap_or_default()),
                "owner/repo",
                // Blank is no repository: every repository is the switch.
                move |line| {
                    write_page.write(setting(
                        &write_dir,
                        KnowledgeSetting::Repositories(Some(split_list(&line))),
                    ))
                },
                debug.clone(),
            )
            .into_any()
        },
    )
    .keywords(&["knowledge", "scope", "repos"])
    .reset(
        format!("knowledge.{}.repos", dir.display()),
        setting(dir, KnowledgeSetting::Repositories(None)),
    )
}

fn remove(page: &Page, dir: &Path) -> Row {
    let name = format!("knowledge:{}:remove", dir.display());
    let debug = format!("settings-{name}");
    let page = page.clone();
    let dir = dir.to_path_buf();
    Row::new(
        name,
        "Remove directory",
        || "Remove the directory from agent context and keep its files.".into(),
        move || {
            let enabled = page.clone();
            let write = page.clone();
            let dir = dir.clone();
            button(
                "Remove",
                debug.clone(),
                move || enabled.writable(),
                move || write.write(setting(&dir, KnowledgeSetting::Remove)),
            )
            .into_any()
        },
    )
    .keywords(&["knowledge", "source"])
}

fn add(page: &Page) -> Row {
    let page = page.clone();
    Row::new(
        "knowledge-add",
        "Directory and repositories",
        || {
            "Use a path relative to the engine’s knowledge folder. Enter comma-separated repository names or leave the field blank for all repositories."
                .into()
        },
        move || {
            let dir = RwSignal::new(String::new());
            let repos = RwSignal::new(String::new());
            let pending = RwSignal::new(false);
            let settings_file = page.state.settings_file;
            Effect::new(move |_| {
                settings_file.with(|_| {});
                if pending.get_untracked() {
                    pending.set(false);
                }
            });
            let enabled = {
                let page = page.clone();
                move || page.writable() && !pending.get()
            };
            let submit = {
                let page = page.clone();
                let enabled = enabled.clone();
                Rc::new(move || {
                    let typed = dir.get_untracked();
                    let typed = typed.trim();
                    if typed.is_empty() || !enabled() {
                        return;
                    }
                    let source = KnowledgeSource {
                        dir: PathBuf::from(typed),
                        repos: repos_from_line(&repos.get_untracked()),
                    };
                    if let Err(error) = source.validate() {
                        page.state.complain(error);
                        return;
                    }
                    pending.set(true);
                    page.write(setting(&source.dir, KnowledgeSetting::Add(source.repos)));
                })
            };
            Stack::vertical((
                entry_input(
                    page.state.settings,
                    enabled.clone(),
                    dir,
                    "discord/slatedb",
                    {
                        let submit = submit.clone();
                        move || submit()
                    },
                    "settings-knowledge-add-dir",
                ),
                entry_input(
                    page.state.settings,
                    enabled.clone(),
                    repos,
                    "All repositories",
                    {
                        let submit = submit.clone();
                        move || submit()
                    },
                    "settings-knowledge-add-repos",
                ),
                button(
                    "Add directory",
                    "settings-knowledge-add-go",
                    move || enabled() && dir.with(|text| !text.trim().is_empty()),
                    move || submit(),
                ),
            ))
            .style(|s| s.gap(8.0).items_end())
            .into_any()
        },
    )
    .keywords(&["knowledge", "directory", "source", "discord", "slack", "repos"])
}
