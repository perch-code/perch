//! Repositories: a card for each `[[repo]]`, then Add repository….
//!
//! A card's rows find their repository again by its identity each time
//! they read the file, so a value written elsewhere shows at once. The
//! page builds a card again only when a repository comes, goes or is
//! renamed.

use floem::prelude::*;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::views::{dyn_container, Decorators, Label};
use floem::AnyView;
use perch_github::{Approvals, RepoConfig, RepoSetting, Setting};

use super::controls::{self, dropdown, list_line, setting_text, split_list, toggle};
use super::{Group, Page, Row};
use crate::theme;

pub(super) fn groups(page: &Page) -> Vec<Group> {
    let place = page.state.place.get();
    let repos = page.with_config(|c| c.repo.clone());
    let mut groups: Vec<Group> = repos
        .iter()
        .map(|repo| {
            let identity = identity_of(repo);
            let mut rows = vec![name(page, repo, &identity)];
            rows.extend([root(page, &identity), base(page, &identity)]);
            if !perch_github::is_local(&identity) {
                rows.push(sync_history(page, &identity));
            }
            rows.extend([
                approvals(page, &identity),
                work_trust(page, &identity),
                lsp_trust(page, &identity),
                write_dirs(page, &identity),
                network(page, &identity),
                remove(page, repo, &identity),
            ]);
            let place = place
                .root_path(repo)
                .map(|root| place.tilde(&root))
                .unwrap_or_default();
            // What the scan resolved, not just `github`: a root-only entry
            // whose checkout has a GitHub remote is that repository.
            let whose = if perch_github::is_local(&identity) {
                "Local only".to_string()
            } else {
                identity.clone()
            };
            let subtitle = if place.is_empty() {
                whose
            } else {
                format!("{whose} · {place}")
            };
            Group::new(repo.display_name(), rows).subtitle(subtitle)
        })
        .collect();
    let mut adding = vec![add(page)];
    // Only a host clones: on this machine a folder is picked.
    if let Some(host) = page.state.place.get().host() {
        adding.push(clone_root(page, host));
    }
    groups.push(Group::new("Add a repository", adding));
    groups
}

/// The folder a repository added by its name is cloned into, as the file
/// has it, or `~` with none.
fn clone_root_of(page: &Page) -> String {
    page.with_config(|c| {
        c.clone_root
            .as_deref()
            .map(str::trim)
            .filter(|root| !root.is_empty())
            .unwrap_or("~")
            .to_string()
    })
}

/// `clone_root`: where on `host` a repository added by name is cloned.
fn clone_root(page: &Page, host: String) -> Row {
    let control = {
        let page = page.clone();
        move || {
            let (value_page, write) = (page.clone(), page.clone());
            setting_text(
                &page,
                move || value_page.with_config(|c| c.clone_root.clone().unwrap_or_default()),
                "~",
                move |typed| {
                    let root = typed.trim();
                    write.write(Setting::CloneRoot(
                        (!root.is_empty()).then(|| root.to_string()),
                    ))
                },
                "settings-clone-root",
            )
            .into_any()
        }
    };
    Row::new(
        "clone-root",
        "Clone into",
        move || {
            format!(
                "The folder on {host} a repository added by its name is cloned into, such as ~/Code. Your home directory there with none."
            )
        },
        control,
    )
    .keywords(&["clone", "folder", "code", "directory", "clone_root"])
    .reset("clone_root", Setting::CloneRoot(None))
}

/// The identity a write names a repository by: what the backend resolved,
/// else what the writer settles on for a root-only entry it can't match,
/// `local/<name>`.
pub(super) fn identity_of(repo: &RepoConfig) -> String {
    match repo.identity() {
        Some(identity) => identity.to_string(),
        None => format!(
            "{}/{}",
            perch_github::config::LOCAL_OWNER,
            repo.display_name()
        ),
    }
}

/// Read the repository known as `identity` as the file has it now,
/// tracked. `None` once it has left the file.
fn with_repo<T>(page: &Page, identity: &str, read: impl FnOnce(Option<&RepoConfig>) -> T) -> T {
    page.with_config(|c| {
        read(
            c.repo
                .iter()
                .find(|r| identity_of(r).eq_ignore_ascii_case(identity)),
        )
    })
}

/// The dotted key `present_keys` gives a repository's `key`.
fn key(identity: &str, key: &str) -> String {
    format!("repo.{}.{key}", identity.to_ascii_lowercase())
}

fn change(identity: &str, change: RepoSetting) -> Setting {
    Setting::Repo {
        identity: identity.to_string(),
        change,
    }
}

/// A row name unique on the page: `repo:<identity>:<what>`.
fn row_name(identity: &str, what: &str) -> String {
    format!("repo:{}:{what}", identity.to_ascii_lowercase())
}

/// A one-line box over one of a repository's text keys.
fn text_row(
    page: &Page,
    identity: &str,
    what: &str,
    read: impl Fn(&RepoConfig) -> String + Copy + 'static,
    placeholder: &'static str,
    write: impl Fn(String) -> RepoSetting + Copy + 'static,
) -> impl Fn() -> AnyView + 'static {
    let page = page.clone();
    let identity = identity.to_string();
    let debug = format!("settings-{}", row_name(&identity, what));
    move || {
        let (value_page, write_page) = (page.clone(), page.clone());
        let (value_identity, write_identity) = (identity.clone(), identity.clone());
        setting_text(
            &page,
            move || {
                with_repo(&value_page, &value_identity, |r| {
                    r.map(read).unwrap_or_default()
                })
            },
            placeholder,
            move |typed| write_page.write(change(&write_identity, write(typed))),
            debug.clone(),
        )
        .into_any()
    }
}

fn name(page: &Page, repo: &RepoConfig, identity: &str) -> Row {
    let row = row_name(identity, "name");
    // Only a repository on GitHub is known by something other than its
    // name, whether the file says `github` or the scan read it off the
    // root's remote. A local one's name is its identity, `local/<name>`,
    // so a rename here would lose track of its items and work.
    if perch_github::is_local(identity) {
        let shown = {
            let (page, identity) = (page.clone(), identity.to_string());
            move || {
                let page = page.clone();
                let identity = identity.clone();
                let debug = format!("settings-{}-fixed", row_name(&identity, "name"));
                Label::derived(move || {
                    with_repo(&page, &identity, |r| {
                        r.map(RepoConfig::display_name).unwrap_or_default()
                    })
                })
                .debug_name(debug)
                .style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::fg_muted())
                        .text_ellipsis()
                        .max_width(controls::CONTROL_WIDTH)
                })
                .into_any()
            }
        };
        return Row::new(
            row,
            "Name",
            || {
                "Perch knows a local-only repository by its name, so renaming it would \
                 leave its items behind. Change it in config.toml if you must."
                    .to_string()
            },
            shown,
        )
        .keywords(&["rename", "name", "local"]);
    }
    // Without `github`, a blank name falls back to the checkout's folder.
    let (empty_means, placeholder) = if repo.github.is_some() {
        ("the repository's name on GitHub", "As on GitHub")
    } else {
        ("the checkout's folder name", "As the folder")
    };
    Row::new(
        row,
        "Name",
        move || format!("What the sidebar calls it. Empty means {empty_means}."),
        text_row(
            page,
            identity,
            "name",
            |r| r.name.clone().unwrap_or_default(),
            placeholder,
            |typed| {
                let name = typed.trim();
                RepoSetting::Name((!name.is_empty()).then(|| name.to_string()))
            },
        ),
    )
    .keywords(&["rename", "name", "sidebar"])
    .reset(
        key(identity, "name"),
        change(identity, RepoSetting::Name(None)),
    )
}

fn root(page: &Page, identity: &str) -> Row {
    let control = {
        let (page, identity) = (page.clone(), identity.to_string());
        move || {
            let (page, identity) = (page.clone(), identity.clone());
            controls::button(
                // A host's checkout opens in an editor that reaches it.
                if page.state.place.get().host().is_some() {
                    "Open in editor"
                } else {
                    "Reveal"
                },
                format!("settings-{}", row_name(&identity, "reveal")),
                || true,
                move || {
                    if let Some(root) = with_repo(&page, &identity, |r| {
                        r.and_then(|repo| page.state.place.get().root_path(repo))
                    }) {
                        crate::ui::widgets::reveal_in_file_manager(page.state, &root);
                    }
                },
            )
            .into_any()
        }
    };
    let (described, identity_owned) = (page.clone(), identity.to_string());
    Row::new(
        row_name(identity, "root"),
        "Checkout",
        move || {
            with_repo(&described, &identity_owned, |r| {
                let place = described.state.place.get();
                r.and_then(|repo| place.root_path(repo))
                    .map(|root| place.tilde(&root))
                    .unwrap_or_else(|| match place.host() {
                        Some(host) => format!("No checkout on {host}."),
                        None => "No checkout on this machine.".to_string(),
                    })
            })
        },
        control,
    )
    .keywords(&["root", "path", "folder", "checkout", "reveal"])
}

fn base(page: &Page, identity: &str) -> Row {
    Row::new(
        row_name(identity, "base"),
        "Base branch",
        || "What new work branches from. Empty means the remote's default branch.".to_string(),
        text_row(
            page,
            identity,
            "base",
            |r| r.base.clone().unwrap_or_default(),
            "The remote's default",
            |typed| {
                let base = typed.trim();
                RepoSetting::Base((!base.is_empty()).then(|| base.to_string()))
            },
        ),
    )
    .keywords(&["base", "branch", "main", "default"])
    .reset(
        key(identity, "base"),
        change(identity, RepoSetting::Base(None)),
    )
}

fn approvals(page: &Page, identity: &str) -> Row {
    let control = {
        let (page, identity) = (page.clone(), identity.to_string());
        move || {
            let (read_identity, write_identity) = (identity.clone(), identity.clone());
            dropdown(
                &page,
                move |page| {
                    let inherited = page.with_config(|c| c.work.approvals);
                    let mut offered = vec![(format!("Inherit ({})", inherited.label()), None)];
                    offered.extend(
                        [Approvals::Auto, Approvals::Ask, Approvals::Never]
                            .into_iter()
                            .map(|a| (a.label().to_string(), Some(a))),
                    );
                    let current =
                        with_repo(page, &read_identity, |r| r.and_then(|r| r.work.approvals));
                    (offered, current)
                },
                move |picked| change(&write_identity, RepoSetting::Approvals(picked)),
                format!("settings-{}", row_name(&identity, "approvals")),
            )
        }
    };
    Row::new(
        row_name(identity, "approvals"),
        "Approvals",
        || {
            "Who decides when a Work run here asks to go past its sandbox. Allow all lets \
             every request through without asking."
                .to_string()
        },
        control,
    )
    .keywords(&[
        "approve",
        "permission",
        "sandbox",
        "auto-review",
        "allow all",
    ])
    .reset(
        key(identity, "work.approvals"),
        change(identity, RepoSetting::Approvals(None)),
    )
}

/// A switch over one of a repository's yes-or-no keys.
fn switch(
    page: &Page,
    identity: &str,
    what: &str,
    read: impl Fn(&RepoConfig) -> bool + Copy + 'static,
    write: impl Fn(bool) -> RepoSetting + Copy + 'static,
) -> impl Fn() -> AnyView + 'static {
    let page = page.clone();
    let identity = identity.to_string();
    let debug = format!("settings-{}", row_name(&identity, what));
    move || {
        let (on_page, enabled, write_page) = (page.clone(), page.clone(), page.clone());
        let (on_identity, write_identity) = (identity.clone(), identity.clone());
        toggle(
            move || with_repo(&on_page, &on_identity, |r| r.is_some_and(read)),
            move || enabled.writable(),
            move |on| write_page.write(change(&write_identity, write(on))),
            debug.clone(),
        )
        .into_any()
    }
}

fn work_trust(page: &Page, identity: &str) -> Row {
    Row::new(
        row_name(identity, "work-trust"),
        "Trust the repository's agent config",
        || {
            "Work runs load its .claude/ settings, .mcp.json, .codex/ and .agents/. What \
             they hold runs as you, outside the sandbox. Gemini needs this to run here."
                .to_string()
        },
        switch(
            page,
            identity,
            "work-trust",
            |r| r.work.trust,
            RepoSetting::WorkTrust,
        ),
    )
    .keywords(&["trust", "mcp", "claude", "agents", "work.trust"])
}

fn sync_history(page: &Page, identity: &str) -> Row {
    Row::new(
        row_name(identity, "sync-history"),
        "Sync GitHub history",
        || {
            "Keep issues, pull requests and their discussions available for agents to search."
                .into()
        },
        switch(
            page,
            identity,
            "sync-history",
            |r| r.sync_history,
            RepoSetting::SyncHistory,
        ),
    )
    .keywords(&["github", "history", "knowledge", "archive", "sync_history"])
}

fn lsp_trust(page: &Page, identity: &str) -> Row {
    Row::new(
        row_name(identity, "lsp-trust"),
        "Start language servers without asking",
        || {
            "rust-analyzer and jdtls start on every item here, anyone's pull request \
             included. They run the checkout's build scripts as you."
                .to_string()
        },
        switch(
            page,
            identity,
            "lsp-trust",
            |r| r.lsp.trust,
            RepoSetting::LspTrust,
        ),
    )
    .keywords(&["trust", "lsp", "language server", "rust-analyzer", "jdtls"])
}

fn write_dirs(page: &Page, identity: &str) -> Row {
    Row::new(
        row_name(identity, "write"),
        "Extra writable directories",
        || {
            "Directories a Work run may write besides its worktree, such as a package \
             cache, between commas. ~ is your home folder."
                .to_string()
        },
        text_row(
            page,
            identity,
            "write",
            |r| list_line(&r.work.write),
            "None",
            |typed| RepoSetting::Write(split_list(&typed)),
        ),
    )
    .keywords(&["sandbox", "write", "directories", "cache", "work.write"])
    .reset(
        key(identity, "work.write"),
        change(identity, RepoSetting::Write(Vec::new())),
    )
}

fn network(page: &Page, identity: &str) -> Row {
    Row::new(
        row_name(identity, "network"),
        "Network hosts",
        || {
            "Hosts a Claude Code run may reach without asking, such as index.crates.io, \
             between commas. Codex and Gemini ask for each."
                .to_string()
        },
        text_row(
            page,
            identity,
            "network",
            |r| list_line(&r.work.network),
            "None",
            |typed| RepoSetting::Network(split_list(&typed)),
        ),
    )
    .keywords(&["sandbox", "network", "hosts", "domains", "work.network"])
    .reset(
        key(identity, "work.network"),
        change(identity, RepoSetting::Network(Vec::new())),
    )
}

fn remove(page: &Page, repo: &RepoConfig, identity: &str) -> Row {
    let shown_name = repo.display_name();
    let control = {
        let (page, identity) = (page.clone(), identity.to_string());
        move || {
            let asking = RwSignal::new(false);
            let (page, identity, shown_name) = (page.clone(), identity.clone(), shown_name.clone());
            let debug = format!("settings-{}", row_name(&identity, "remove"));
            dyn_container(
                move || asking.get(),
                move |open| {
                    if !open {
                        let enabled = page.clone();
                        return controls::button(
                            "Remove…",
                            debug.clone(),
                            move || enabled.writable(),
                            move || asking.set(true),
                        )
                        .into_any();
                    }
                    let (page, identity) = (page.clone(), identity.clone());
                    crate::ui::widgets::confirmation(
                        "settings-remove-repo",
                        format!("Remove {shown_name} from Perch?"),
                        "Its [[repo]] leaves config.toml. Its items stay, and nothing on disk \
                         is touched."
                            .to_string(),
                        "Remove",
                        move || {
                            asking.set(false);
                            crate::ui::new_work::remove_repo(page.state, &page.backend, &identity);
                        },
                        move || asking.set(false),
                    )
                    .into_any()
                },
            )
            .style(|s| {
                s.width(controls::CONTROL_WIDTH + 60.0)
                    .max_width_full()
                    .justify_end()
            })
            .into_any()
        }
    };
    Row::new(
        row_name(identity, "remove"),
        "Remove repository",
        || "Perch stops listing it. Its checkout and worktrees stay where they are.".to_string(),
        control,
    )
    .keywords(&["remove", "delete", "forget"])
}

/// Add repository with the engine on `host`, where the folders are not
/// this Mac's to pick from: a box for a repository on GitHub, which the
/// engine clones there, or the path of a clone already there.
fn add_on_host(page: &Page, host: String) -> Row {
    let title = format!("A repository on {host}");
    let said = host.clone();
    let control = {
        let page = page.clone();
        move || {
            let (state, backend, host) = (page.state, page.backend.clone(), host.clone());
            let enabled = page.clone();
            controls::entry(
                state.settings,
                move || enabled.writable(),
                state.settings.adding,
                state.settings.adding_sent,
                "owner/name, or a path",
                "Add",
                move |typed| match perch_engine::repos::repo_source(&typed) {
                    Err(why) => {
                        state.complain(why);
                        false
                    }
                    Ok(source) => {
                        if let perch_engine::repos::RepoSource::GitHub(id) = source {
                            state.notify(
                                format!("Adding {} on {host}…", id.full_name()),
                                crate::state::Tone::Success,
                            );
                        }
                        backend.add_repository_from(&typed);
                        true
                    }
                },
                "settings-add-repository",
            )
            .into_any()
        }
    };
    let where_to = page.clone();
    Row::new(
        "add-repository",
        title,
        move || {
            format!(
                "Type owner/name to clone it from GitHub into {} on {said}, or the path of a clone already there. Perch adds it to the config.toml on {said}.",
                clone_root_of(&where_to)
            )
        },
        control,
    )
    .keywords(&["add", "new", "repository", "folder", "clone", "host"])
}

fn add(page: &Page) -> Row {
    if let Some(host) = page.state.place.get().host() {
        return add_on_host(page, host);
    }
    let control = {
        let page = page.clone();
        move || {
            let (enabled, backend, state) = (page.clone(), page.backend.clone(), page.state);
            controls::button(
                "Add repository…",
                "settings-add-repository",
                move || enabled.writable(),
                move || crate::ui::new_work::add_repository(state, backend.clone()),
            )
            .into_any()
        }
    };
    Row::new(
        "add-repository",
        "A checkout on this machine",
        || "Pick its folder. Perch adds it to config.toml and lists it in the sidebar.".to_string(),
        control,
    )
    .keywords(&["add", "new", "repository", "folder", "clone"])
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::super::tests::{
        click, enter_in, exactly_named, file, goes_dark, named, open_on, pick, sent, shown, window,
        written,
    };
    use super::super::Section;
    use super::*;
    use crate::backend::{Backend, Command};

    /// One repository on GitHub, with a base and a host, and one only on
    /// this machine.
    const FILE: &str = "[[repo]]\ngithub = \"acme/widgets\"\nroot = \"/srv/widgets\"\n\
                        base = \"develop\"\n\n[repo.work]\nnetwork = [\"index.crates.io\"]\n\n\
                        [[repo]]\nroot = \"/srv/notes\"\n";

    fn repo_change(identity: &str, change: RepoSetting) -> Setting {
        Setting::Repo {
            identity: identity.into(),
            change,
        }
    }

    /// What Add repository was asked to add from, of the commands sent
    /// since the last look.
    fn added(commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> Vec<String> {
        sent(commands)
            .into_iter()
            .filter_map(|command| match command {
                Command::AddRepositoryFrom { source } => Some(source),
                _ => None,
            })
            .collect()
    }

    /// With the engine on a host there is no folder to pick: the row is a
    /// box that takes a repository's name on GitHub or a path there, sends
    /// it on Enter, says a name is being added, keeps what was typed until
    /// the engine has added it, and sends nothing it can't read. Add
    /// repository anywhere else in the app opens Settings here.
    #[test]
    fn on_a_host_a_repository_is_added_by_its_name_or_its_path() {
        let (mut harness, state, mut commands) = window(1400.0, 2600.0, FILE);
        state.place.set(crate::remote::Place::Host {
            host: "devbox".into(),
            home: "/home/dev".into(),
            data_root: "/home/dev/.perch".into(),
        });
        crate::ui::new_work::add_repository(state, Rc::new(Backend::detached()));
        assert_eq!(
            state.settings.section.get_untracked(),
            Section::Repositories
        );
        assert!(state.settings.open.get_untracked());
        open_on(&mut harness, state, Section::Repositories);
        assert!(shown(&harness, "settings-add-repository-go"));

        enter_in(&mut harness, "settings-add-repository", "acme/gadgets");
        assert_eq!(added(&mut commands), ["acme/gadgets"]);
        assert_eq!(
            state.notice().as_deref(),
            Some("Adding acme/gadgets on devbox…")
        );
        // Enter again while that one is on its way sends nothing more.
        enter_in(&mut harness, "settings-add-repository", "");
        assert!(added(&mut commands).is_empty());

        // Added: the box is empty again, and what is typed next is all
        // that is sent. The button sends as Enter does.
        state.apply(crate::backend::Event::RepositoryAdded(
            "acme/gadgets".into(),
        ));
        super::super::tests::settle(&mut harness);
        assert_eq!(
            state.notice().as_deref(),
            Some("Added acme/gadgets on devbox.")
        );
        click(&mut harness, "settings-add-repository");
        super::super::tests::type_text(&mut harness, "~/code/notes");
        click(&mut harness, "settings-add-repository-go");
        assert_eq!(added(&mut commands), ["~/code/notes"]);

        // Where a named one is cloned is a setting beside it, the home
        // directory until it is set.
        assert!(shown(&harness, "settings-clone-root"));
        enter_in(&mut harness, "settings-clone-root", "~/Code");
        assert_eq!(
            written(&mut commands),
            vec![Setting::CloneRoot(Some("~/Code".into()))]
        );

        // Neither a name nor a path: said, and not sent.
        state.apply(crate::backend::Event::RepositoryAdded("local/notes".into()));
        super::super::tests::settle(&mut harness);
        enter_in(&mut harness, "settings-add-repository", "notes");
        assert!(added(&mut commands).is_empty());
        assert!(state
            .notice()
            .is_some_and(|said| said.contains("neither a repository on GitHub")));
        state.place.set(crate::remote::Place::ThisMac);
    }

    #[test]
    fn a_card_for_each_repository_with_every_row() {
        let (mut harness, state, _commands) = window(1400.0, 2600.0, FILE);
        open_on(&mut harness, state, Section::Repositories);
        assert!(shown(&harness, "settings-group:widgets"));
        assert!(shown(&harness, "settings-group:notes"));
        assert!(shown(&harness, "settings-group:Add a repository"));
        for identity in ["acme/widgets", "local/notes"] {
            for what in [
                "name",
                "root",
                "base",
                "approvals",
                "work-trust",
                "lsp-trust",
                "write",
                "network",
                "remove",
            ] {
                let row = format!("settings-row:repo:{identity}:{what}");
                assert!(shown(&harness, &row), "{row}");
            }
        }
        assert!(shown(&harness, "settings-add-repository"));
        assert!(shown(&harness, "settings-repo:acme/widgets:sync-history"));
        assert!(!shown(&harness, "settings-repo:local/notes:sync-history"));
        assert!(shown(&harness, "settings-reset:repo:acme/widgets:base"));
        assert!(shown(&harness, "settings-reset:repo:acme/widgets:network"));
        assert!(!shown(&harness, "settings-reset:repo:acme/widgets:write"));
        assert!(!shown(&harness, "settings-reset:repo:local/notes:base"));
    }

    /// A local-only repository is known by its name, so the name can't be
    /// typed over; one on GitHub can be renamed.
    #[test]
    fn a_local_only_repositorys_name_is_read_only() {
        let (mut harness, state, mut commands) = window(1400.0, 2600.0, FILE);
        open_on(&mut harness, state, Section::Repositories);
        let root = harness.root_id();
        assert!(exactly_named(root, "settings-repo:local/notes:name").is_none());
        assert!(exactly_named(root, "settings-repo:local/notes:name-fixed").is_some());
        assert!(exactly_named(root, "settings-repo:acme/widgets:name").is_some());
        assert!(exactly_named(root, "settings-repo:acme/widgets:name-fixed").is_none());

        enter_in(&mut harness, "settings-repo:acme/widgets:name", "gizmos");
        assert_eq!(
            written(&mut commands),
            vec![repo_change(
                "acme/widgets",
                RepoSetting::Name(Some("gizmos".into()))
            )]
        );
    }

    /// A root-only entry whose checkout the scan found on GitHub is shown
    /// and renamed as a GitHub repository, not a local one.
    #[test]
    fn a_root_only_repository_on_github_is_not_local_only() {
        let text = "[[repo]]\nroot = \"/srv/perch\"\n";
        let (mut harness, state, mut commands) = window(1400.0, 1600.0, text);
        let mut resolved = file(text);
        let mut config = (*resolved.config.take().unwrap()).clone();
        config.repo[0].resolve(Some("acme/perch".into()));
        resolved.config = Some(std::sync::Arc::new(config));
        state.settings_file.set(Some(resolved));

        let page = Page::new(state, Rc::new(Backend::detached()));
        let cards = groups(&page);
        assert_eq!(
            cards[0].subtitle.as_deref(),
            Some("acme/perch · /srv/perch")
        );

        open_on(&mut harness, state, Section::Repositories);
        let root = harness.root_id();
        assert!(exactly_named(root, "settings-repo:acme/perch:name-fixed").is_none());
        enter_in(&mut harness, "settings-repo:acme/perch:name", "Perch");
        assert_eq!(
            written(&mut commands),
            vec![repo_change(
                "acme/perch",
                RepoSetting::Name(Some("Perch".into()))
            )]
        );
    }

    #[test]
    fn a_change_writes_to_its_own_repository() {
        let (mut harness, state, mut commands) = window(1400.0, 2600.0, FILE);
        open_on(&mut harness, state, Section::Repositories);

        click(&mut harness, "settings-repo:acme/widgets:sync-history");
        click(&mut harness, "settings-repo:acme/widgets:work-trust");
        click(&mut harness, "settings-repo:local/notes:lsp-trust");
        pick(
            &mut harness,
            "settings-repo:acme/widgets:approvals",
            "Allow all",
        );
        pick(
            &mut harness,
            "settings-repo:local/notes:approvals",
            "Inherit (Auto-review)",
        );
        enter_in(&mut harness, "settings-repo:local/notes:base", "trunk");
        click(&mut harness, "settings-reset:repo:acme/widgets:network");
        assert_eq!(
            written(&mut commands),
            vec![
                repo_change("acme/widgets", RepoSetting::SyncHistory(true)),
                repo_change("acme/widgets", RepoSetting::WorkTrust(true)),
                repo_change("local/notes", RepoSetting::LspTrust(true)),
                repo_change(
                    "acme/widgets",
                    RepoSetting::Approvals(Some(Approvals::Never))
                ),
                repo_change("local/notes", RepoSetting::Approvals(None)),
                repo_change("local/notes", RepoSetting::Base(Some("trunk".into()))),
                repo_change("acme/widgets", RepoSetting::Network(Vec::new())),
            ]
        );

        // A switch the file has on turns off.
        state
            .settings_file
            .set(Some(super::super::tests::file(&format!(
                "{FILE}\n[repo.lsp]\ntrust = true\n"
            ))));
        super::super::tests::settle(&mut harness);
        click(&mut harness, "settings-repo:local/notes:lsp-trust");
        assert_eq!(
            written(&mut commands),
            vec![repo_change("local/notes", RepoSetting::LspTrust(false))]
        );
    }

    /// Remove asks first; Cancel leaves the repository be, and only the
    /// confirmation sends RemoveRepository.
    #[test]
    fn remove_asks_before_it_removes() {
        let (mut harness, state, mut commands) = window(1400.0, 2600.0, FILE);
        open_on(&mut harness, state, Section::Repositories);
        assert!(!shown(&harness, "settings-remove-repo-confirmation"));

        click(&mut harness, "settings-repo:acme/widgets:remove");
        assert!(shown(&harness, "settings-remove-repo-confirmation"));
        assert!(sent(&mut commands).is_empty(), "nothing yet");
        click(&mut harness, "cancel-settings-remove-repo");
        assert!(!shown(&harness, "settings-remove-repo-confirmation"));
        assert!(sent(&mut commands).is_empty());

        click(&mut harness, "settings-repo:acme/widgets:remove");
        click(&mut harness, "confirm-settings-remove-repo");
        let removed: Vec<String> = sent(&mut commands)
            .into_iter()
            .filter_map(|command| match command {
                Command::RemoveRepository { identity } => Some(identity),
                _ => None,
            })
            .collect();
        assert_eq!(removed, vec!["acme/widgets".to_string()]);
        assert!(!shown(&harness, "settings-remove-repo-confirmation"));
        assert!(named(harness.root_id(), "settings-group:widgets").is_some());
    }

    #[test]
    fn repositories_go_dark_with_the_system() {
        let (mut harness, state, _commands) = window(1400.0, 2600.0, FILE);
        open_on(&mut harness, state, Section::Repositories);
        click(&mut harness, "settings-repo:local/notes:remove");
        assert!(shown(&harness, "settings-remove-repo-confirmation"));
        goes_dark(&mut harness);
    }
}
