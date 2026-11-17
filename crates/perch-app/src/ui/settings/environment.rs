//! The environment for this window and its remote host’s setup checks.
use super::controls;
use super::{Group, Page, Row};
use crate::theme;
use floem::prelude::*;
use floem::views::{Decorators, Stack};
use perch_engine::host::HostCheck;
use std::rc::Rc;

pub(super) fn groups(page: &Page) -> Vec<Group> {
    let state = page.state;
    let host = state.place.get().host();
    let remote = host.is_some();
    let mut groups = vec![Group::new(
        "This window",
        vec![current_environment(state, host), connect_host(state)],
    )];
    let checks = state.host_checks.get();
    if remote && !checks.is_empty() {
        let mut rows = Vec::new();
        for check in checks.iter().filter(|c| c.blocks()) {
            rows.push(host_check(page, check));
        }
        let show = state.settings.show_checks;
        let searching = !state.settings.query.get().trim().is_empty();
        if !searching && checks.iter().any(|c| !c.blocks()) {
            rows.push(Row::new(
                "check-summary",
                "Setup checks",
                move || {
                    state.host_checks.with(|checks| {
                        let passed = checks.iter().filter(|c| c.ok).count();
                        let optional = checks.iter().filter(|c| !c.ok && !c.required).count();
                        if optional == 0 {
                            format!("{passed} passed")
                        } else {
                            format!("{passed} passed · {optional} optional")
                        }
                    })
                },
                move || {
                    crate::ui::widgets::action_button(
                        move || {
                            if show.get() {
                                "Hide details"
                            } else {
                                "Show details"
                            }
                            .into()
                        },
                        "settings-toggle-checks",
                        false,
                        || true,
                        move || show.update(|v| *v = !*v),
                    )
                    .into_any()
                },
            ));
        }
        if show.get() || searching {
            rows.extend(
                checks
                    .iter()
                    .filter(|c| !c.blocks())
                    .map(|c| host_check(page, c)),
            );
        }
        groups.push(Group::new("Host setup", rows));
    }
    groups
}

fn current_environment(state: crate::state::AppState, host: Option<String>) -> Row {
    let current = host
        .clone()
        .unwrap_or_else(|| perch_engine::host::HERE_FIRST.into());
    let remote = host.is_some();
    Row::new(
        "current-environment",
        current,
        move || {
            use crate::backend::EngineLink;
            state.link.with(|link| {
                match link {
                    Some(EngineLink::Down { .. }) => "Disconnected",
                    Some(EngineLink::Installing { .. }) => "Installing the remote engine…",
                    Some(EngineLink::Restarting { .. }) => "Restarting the remote engine…",
                    Some(EngineLink::Connecting { .. }) => "Connecting over SSH…",
                    Some(EngineLink::Up { .. }) => "Connected over SSH",
                    None => "Repositories and agents run locally.",
                }
                .into()
            })
        },
        move || {
            if remote {
                controls::button(
                    "Use local environment",
                    "settings-use-local",
                    || true,
                    move || crate::windows::switch_environment(state, None),
                )
                .into_any()
            } else {
                floem::views::Empty::new().into_any()
            }
        },
    )
}

fn connect_host(state: crate::state::AppState) -> Row {
    Row::new(
        "connect-host",
        "Switch to an SSH host",
        || "Use a Linux or macOS machine. Switching closes the item open in this window.".into(),
        move || {
            controls::entry(
                state.settings,
                || true,
                state.settings.ssh_draft,
                RwSignal::new(None),
                "user@host:port",
                "Connect",
                move |typed| {
                    let host = perch_github::config::normalize_host(&typed);
                    if let Some(why) = perch_engine::wire::remote::bad_host(host) {
                        state.complain(why);
                        return false;
                    }
                    if state.place.get_untracked().host().as_deref() == Some(host) {
                        state.notify(
                            format!("This window already uses {host}."),
                            crate::state::Tone::Success,
                        );
                        state.settings.ssh_draft.set(String::new());
                        return false;
                    }
                    crate::windows::switch_environment(state, Some(host.to_owned()));
                    true
                },
                "settings-connect-host",
            )
            .into_any()
        },
    )
    .keywords(&["remote", "environment", "engine", "connection"])
}

/// What a host check says under its title: what was found or is missing,
/// and what to run on the host when it is.
fn check_description(check: &HostCheck) -> String {
    match check.fix.as_deref().filter(|_| !check.ok) {
        Some(fix) => format!("{}\nOn the host:\n{fix}", check.detail),
        None if check.ok && check.what == "logind" => {
            "The host keeps agent runs alive after SSH disconnects.".into()
        }
        None if check.ok && check.what == "bubblewrap and socat" => {
            "bubblewrap and socat are available.".into()
        }
        None => check.detail.clone(),
    }
}

/// How a check stands, in a word.
fn check_mark(check: &HostCheck) -> &'static str {
    match (check.ok, check.required) {
        (true, _) => "Ready",
        (false, true) => "Needs setup",
        (false, false) => "Optional",
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
                    "Copy instructions",
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
                        state.settings.query.set(String::new());
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
        match check.what.as_str() {
            "git" => "Git",
            "GitHub token" => "GitHub authentication",
            "agy (Gemini)" => "Gemini (Antigravity CLI)",
            "bubblewrap and socat" => "Claude Code sandbox",
            "logind" => "Keep running after SSH disconnects",
            other => other,
        }
        .to_string(),
        move || {
            current(state, &what)
                .map(|c| check_description(&c))
                .unwrap_or_default()
        },
        control,
    )
    .keywords(match check.what.as_str() {
        "logind" => &["logind", "systemd"],
        "GitHub token" => &["token"],
        "agy (Gemini)" => &["agy"],
        _ => &[],
    })
    .keywords(&["host", "setup", "check", "missing", "install", "perchd"])
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
            "git is not on PATH.\nOn the host:\nsudo apt-get install -y git"
        );
        assert_eq!(check_mark(&check), "Needs setup");
        check.required = false;
        assert_eq!(check_mark(&check), "Optional");
        check.ok = true;
        check.detail = "git version 2.43.0.".into();
        assert_eq!(check_description(&check), "git version 2.43.0.");
        assert_eq!(check_mark(&check), "Ready");
    }

    /// The check for a repository, failing, has a button to the page that
    /// adds one; the others have none.
    #[test]
    fn a_host_with_no_repository_is_a_click_from_adding_one() {
        use super::super::tests::{click, open_on, shown, window};
        use super::super::Section;
        let (mut harness, state, _commands) = window(1400.0, 2000.0, "");
        state.place.set(crate::remote::Place::Host {
            host: "devbox".into(),
            home: "/home/dev".into(),
            data_root: "/home/dev/.perch".into(),
        });
        let check = |what: &str| HostCheck {
            what: what.into(),
            ok: false,
            required: true,
            detail: "Missing.".into(),
            fix: None,
        };
        state.host_checks.set(vec![
            check("git"),
            check(perch_engine::host::REPOSITORIES),
            HostCheck {
                ok: true,
                ..check("logind")
            },
        ]);
        open_on(&mut harness, state, Section::Environment);
        assert!(!shown(&harness, "settings-row:host-check:logind"));
        click(&mut harness, "settings-toggle-checks");
        assert!(shown(&harness, "settings-row:host-check:logind"));
        assert!(!shown(&harness, "settings-host-check-add:git"));
        assert!(shown(&harness, "settings-host-check-add:repositories"));
        click(&mut harness, "settings-host-check-add:repositories");
        assert_eq!(
            state.settings.section.get_untracked(),
            Section::Repositories
        );
        state.place.set(crate::remote::Place::ThisMac);
    }
}
