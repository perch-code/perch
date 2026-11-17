//! General: where the engine is, and with it on a host what the host
//! lacks, how Perch talks to GitHub, and where config.toml is.

use std::rc::Rc;

use floem::prelude::*;
use floem::views::{Decorators, Stack};
use perch_engine::host::HostCheck;
use perch_github::Setting;

use crate::theme;

use super::controls::{self, choices, select, setting_text};
use super::{Group, Page, Row};

/// The poll intervals offered, in seconds.
const POLL_INTERVALS: [u64; 5] = [30, 60, 120, 300, 600];

pub(super) fn groups(page: &Page) -> Vec<Group> {
    let mut files = vec![config_file(page)];
    if crate::remote::host().is_some() {
        files.push(client_config_file(page));
    }
    let mut engine_rows = vec![engine(&page.here())];
    // What the host's engine found, read tracked so a check that comes or
    // goes builds the page again.
    if crate::remote::host().is_some() {
        let checks = page.state.host_checks.get();
        engine_rows.extend(checks.iter().map(|check| host_check(page, check)));
    }
    vec![
        Group::new("Engine", engine_rows),
        Group::new("GitHub", vec![poll_interval(page), github_host(page)]),
        Group::new("Config file", files),
    ]
}

/// The host a window shows the engine of, as the running app has it.
fn running_host() -> Option<String> {
    crate::remote::host()
}

/// `[remote] host`: This Mac, or an SSH host. A change takes effect when
/// Perch starts again: every window holds what the engine it has told it,
/// and another engine's items, runs and settings are another app's worth
/// of state, so a restart is the one clean way across.
fn engine(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            let (value_page, write) = (page.clone(), page.clone());
            setting_text(
                &page,
                move || value_page.with_config(|c| c.remote.host().unwrap_or_default().to_string()),
                perch_engine::host::HERE_FIRST,
                move |typed| {
                    let host = perch_github::config::normalize_host(&typed);
                    if host.is_empty() {
                        write.write(Setting::RemoteHost(None));
                        return;
                    }
                    match perch_engine::wire::remote::bad_host(host) {
                        Some(why) => write.state.complain(why),
                        None => write.write(Setting::RemoteHost(Some(host.to_string()))),
                    }
                },
                "settings-engine-host",
            )
            .into_any()
        }
    };
    let pending = page.clone();
    Row::new(
        "engine",
        "Engine",
        || {
            format!(
                "Where Perch's engine runs: {}, or a Linux or macOS host you can ssh to, as you would name it to ssh, with a port after it if it has one (chris@devbox:2222). Perch installs perchd there itself. Changing it takes effect when Perch restarts.",
                perch_engine::host::HERE
            )
        },
        control,
    )
    .keywords(&["remote", "host", "ssh", "perchd", "server", "remote.host"])
    .reset("remote.host", Setting::RemoteHost(None))
    .restart(move || pending.with_config(|c| c.remote.host().map(str::to_string)) != running_host())
}

/// What a host check says under its title: what was found or is missing,
/// and what to run on the host when it is.
fn check_description(check: &HostCheck) -> String {
    match check.fix.as_deref().filter(|_| !check.ok) {
        Some(fix) => format!("{}\nOn the host, run: {fix}", check.detail),
        None => check.detail.clone(),
    }
}

/// How a check stands, in a word.
fn check_mark(check: &HostCheck) -> &'static str {
    match (check.ok, check.required) {
        (true, _) => "Found",
        (false, true) => "Missing",
        (false, false) => "Not installed",
    }
}

/// The check that looks for `what`, as the host's engine last said it.
fn current(state: crate::state::AppState, what: &str) -> Option<HostCheck> {
    state
        .host_checks
        .with(|checks| checks.iter().find(|c| c.what == what).cloned())
}

/// One of the host's checks: what it is, what was found or what to run,
/// how it stands, and a Copy for the command. The row is the check's by
/// what it looks for, and reads it as it now is, so a look that changes
/// only the words shows them.
fn host_check(page: &Page, check: &HostCheck) -> Row {
    let state = page.state;
    let what = Rc::new(check.what.clone());
    let name = what.to_lowercase().replace(' ', "-");
    let control = {
        let (what, name) = (what.clone(), name.clone());
        move || {
            let (mark, style, copy, shown, add) = (
                what.clone(),
                what.clone(),
                what.clone(),
                what.clone(),
                what.clone(),
            );
            Stack::horizontal((
                Label::derived(move || {
                    current(state, &mark)
                        .map(|c| check_mark(&c))
                        .unwrap_or_default()
                })
                .debug_name(format!("settings-host-check-mark:{name}"))
                .style(move |s| {
                    let blocks = current(state, &style).is_some_and(|c| c.blocks());
                    s.font_size(theme::TEXT_META)
                        .color(if blocks {
                            theme::notice_fg()
                        } else {
                            theme::fg_muted()
                        })
                        .apply_if(blocks, |s| s.font_weight(theme::weight_emphasis()))
                }),
                super::controls::button(
                    "Copy",
                    format!("settings-host-check-copy:{name}"),
                    || true,
                    move || {
                        if let Some(fix) = current(state, &copy).and_then(|c| c.fix) {
                            crate::ui::widgets::copy_to_clipboard(fix);
                        }
                    },
                )
                .style(move |s| {
                    let has_fix = current(state, &shown).is_some_and(|c| !c.ok && c.fix.is_some());
                    s.apply_if(!has_fix, |s| s.hide())
                }),
                // No repository yet: the page that adds one.
                super::controls::button(
                    "Add repository…",
                    format!("settings-host-check-add:{name}"),
                    || true,
                    move || {
                        state.settings.section.set(super::Section::Repositories);
                    },
                )
                .style(move |s| {
                    let adds = current(state, &add)
                        .is_some_and(|c| !c.ok && c.what == perch_engine::host::REPOSITORIES);
                    s.apply_if(!adds, |s| s.hide())
                }),
            ))
            .style(|s| s.gap(10.0).items_center())
            .into_any()
        }
    };
    Row::new(
        format!("host-check:{name}"),
        check.what.clone(),
        move || {
            current(state, &what)
                .map(|c| check_description(&c))
                .unwrap_or_default()
        },
        control,
    )
    .keywords(&["host", "setup", "check", "missing", "install", "perchd"])
}

/// `secs` as a person would say it: "30 seconds", "1 minute".
pub(super) fn interval_label(secs: u64) -> String {
    match secs {
        1 => "1 second".to_string(),
        60 => "1 minute".to_string(),
        s if s > 0 && s % 60 == 0 => format!("{} minutes", s / 60),
        s => format!("{s} seconds"),
    }
}

fn poll_interval(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            let (label_page, menu_page, enabled) = (page.clone(), page.clone(), page.clone());
            select(
                move || label_page.with_config(|c| interval_label(c.poll_interval_secs)),
                move || {
                    let current = menu_page.with_config(|c| c.poll_interval_secs);
                    let mut offered = POLL_INTERVALS.to_vec();
                    if !offered.contains(&current) {
                        offered.push(current);
                        offered.sort_unstable();
                    }
                    let write = menu_page.clone();
                    choices(
                        offered
                            .into_iter()
                            .map(|secs| (interval_label(secs), secs))
                            .collect(),
                        &current,
                        Rc::new(move |secs| write.write(Setting::PollIntervalSecs(Some(secs)))),
                    )
                },
                move || enabled.writable(),
                "settings-poll-interval",
            )
            .into_any()
        }
    };
    Row::new(
        "poll-interval",
        "Poll interval",
        || "How often Perch asks GitHub what's new. GitHub can ask for longer.".to_string(),
        control,
    )
    .keywords(&["refresh", "notifications", "poll_interval_secs"])
    .reset("poll_interval_secs", Setting::PollIntervalSecs(None))
}

fn github_host(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            let (value_page, write) = (page.clone(), page.clone());
            setting_text(
                &page,
                move || value_page.with_config(|c| c.github_host.clone().unwrap_or_default()),
                "github.example.com",
                move |typed| {
                    let host = typed.trim();
                    write.write(Setting::GithubHost(
                        (!host.is_empty()).then(|| host.to_string()),
                    ))
                },
                "settings-github-host",
            )
            .into_any()
        }
    };
    Row::new(
        "github-host",
        "GitHub Enterprise host",
        || {
            "A GitHub Enterprise Server whose repositories Perch reads, beside github.com."
                .to_string()
        },
        control,
    )
    .keywords(&["ghe", "enterprise", "server", "github_host"])
    .reset("github_host", Setting::GithubHost(None))
}

fn config_file(page: &Page) -> Row {
    let path_page = page.clone();
    let control = {
        let page = page.clone();
        move || {
            let reveal = page.clone();
            // On a host, opening it in an editor is all there is.
            let on_host = crate::remote::host().is_some();
            Stack::horizontal((
                controls::open_in_editor_button(page.clone(), "settings-open-config"),
                controls::button(
                    "Reveal",
                    "settings-reveal-config",
                    || true,
                    move || {
                        if let Some(path) = reveal.path() {
                            crate::ui::widgets::reveal_in_file_manager(&path);
                        }
                    },
                )
                .style(move |s| s.apply_if(on_host, |s| s.hide())),
            ))
            .style(|s| s.gap(8.0).items_center())
            .into_any()
        }
    };
    Row::new(
        "config-file",
        "config.toml",
        move || {
            path_page
                .path()
                .map(|path| controls::tilde(&path))
                .unwrap_or_default()
        },
        control,
    )
    .keywords(&["file", "path", "edit", "toml", "config"])
}

/// With the engine on a host, this Mac's own config.toml: where `[ui]`,
/// `[display]`, `[keybindings]` and `[remote]` are kept.
fn client_config_file(_page: &Page) -> Row {
    let here = || perch_github::config::config_path();
    let control = move || {
        Stack::horizontal((
            controls::button(
                "Open in editor",
                "settings-open-client-config",
                || true,
                move || {
                    if let Some(path) = here() {
                        crate::ui::widgets::open_in_editor_here(&path);
                    }
                },
            ),
            controls::button(
                "Reveal",
                "settings-reveal-client-config",
                || true,
                move || {
                    if let Some(path) = here() {
                        crate::ui::widgets::reveal_here(&path);
                    }
                },
            ),
        ))
        .style(|s| s.gap(8.0).items_center())
        .into_any()
    };
    Row::new(
        "client-config-file",
        format!("{}'s config.toml", perch_engine::host::HERE_FIRST),
        move || {
            let path = here()
                .map(
                    |path| match path.strip_prefix(perch_github::config::home_dir()) {
                        Ok(rest) => format!("~/{}", rest.display()),
                        Err(_) => path.display().to_string(),
                    },
                )
                .unwrap_or_default();
            format!("{path}: appearance, display, keyboard shortcuts and the engine's host.")
        },
        control,
    )
    .keywords(&["file", "path", "toml", "config", "local", "client"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_says_what_to_run_only_while_it_fails() {
        let mut check = HostCheck {
            what: "git".into(),
            ok: false,
            required: true,
            detail: "git is not on PATH.".into(),
            fix: Some("sudo apt-get install -y git".into()),
        };
        assert_eq!(
            check_description(&check),
            "git is not on PATH.\nOn the host, run: sudo apt-get install -y git"
        );
        assert_eq!(check_mark(&check), "Missing");
        check.required = false;
        assert_eq!(check_mark(&check), "Not installed");
        check.ok = true;
        check.detail = "git version 2.43.0.".into();
        assert_eq!(check_description(&check), "git version 2.43.0.");
        assert_eq!(check_mark(&check), "Found");
    }

    /// The check for a repository, failing, has a button to the page that
    /// adds one; the others have none.
    #[test]
    fn a_host_with_no_repository_is_a_click_from_adding_one() {
        use super::super::tests::{click, open_on, shown, window};
        use super::super::Section;
        crate::remote::set_place(crate::remote::Place::Host {
            host: "devbox".into(),
            home: "/home/dev".into(),
            data_root: "/home/dev/.perch".into(),
        });
        let (mut harness, state, _commands) = window(1400.0, 2000.0, "");
        let check = |what: &str| HostCheck {
            what: what.into(),
            ok: false,
            required: true,
            detail: "Missing.".into(),
            fix: None,
        };
        state
            .host_checks
            .set(vec![check("git"), check(perch_engine::host::REPOSITORIES)]);
        open_on(&mut harness, state, Section::General);
        assert!(!shown(&harness, "settings-host-check-add:git"));
        assert!(shown(&harness, "settings-host-check-add:repositories"));
        click(&mut harness, "settings-host-check-add:repositories");
        assert_eq!(
            state.settings.section.get_untracked(),
            Section::Repositories
        );
        crate::remote::set_place(crate::remote::Place::ThisMac);
    }

    #[test]
    fn an_interval_reads_in_the_largest_whole_unit() {
        assert_eq!(interval_label(30), "30 seconds");
        assert_eq!(interval_label(60), "1 minute");
        assert_eq!(interval_label(300), "5 minutes");
        assert_eq!(interval_label(90), "90 seconds");
        assert_eq!(interval_label(1), "1 second");
    }
}
