//! The strip along the foot of every window while the engine on a host
//! cannot be reached: what happened, why, and a button to try now. It
//! stays until the connection is back, as a toast would not: what the
//! window shows meanwhile is what it had, and nothing done in it goes.
//!
//! It also stays while the host's engine is another build than the one
//! the app installs there, left running: an older one busy with runs or
//! other clients, with a button to restart it, or a newer one, which only
//! updating the app answers.
//!
//! Connected, it says the host needs setup while the host's engine finds
//! something it can't work without, with a button to Settings, where each
//! check says what to run.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::SignalWith;
use floem::views::{dyn_container, Container, Decorators, Empty, Stack};

use crate::backend::{Backend, EngineLink};
use crate::icons::SidebarIcon;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{action_button, sidebar_glyph};
use perch_engine::host::HostCheck;
use perch_engine::wire::remote::other_said;

/// What the strip says, and the line under it, for how the connection
/// stands and what the host's engine found missing; `None` while it is up
/// to the build the app installs with nothing required missing, or with
/// the engine in process.
pub fn said(link: &EngineLink, checks: &[HostCheck]) -> Option<(String, Option<String>)> {
    match link {
        EngineLink::Up {
            host, other: None, ..
        } => needs_setup(link, checks).then(|| {
            let missing: Vec<&str> = checks
                .iter()
                .filter(|check| check.blocks())
                .map(|check| check.detail.as_str())
                .collect();
            (format!("{host} needs setup."), Some(missing.join(" ")))
        }),
        EngineLink::Up {
            host,
            other: Some(other),
            ..
        } => Some((other_said(host, other), None)),
        EngineLink::Connecting { host } => Some((format!("Connecting to {host}…"), None)),
        EngineLink::Installing {
            host,
            build,
            progress,
        } => {
            let detail = progress.as_ref().map(|p| match p.percent() {
                Some(percent) => format!("{} · {percent}%", p.phase),
                None => format!("{}…", p.phase),
            });
            Some((format!("Installing perchd {build} on {host}…"), detail))
        }
        EngineLink::Restarting { host, build } => Some((
            format!("Restarting the engine on {host} to update it to perchd {build}…"),
            None,
        )),
        EngineLink::Down {
            host,
            why,
            other: Some(other),
            ..
        } => Some((
            other_said(host, other),
            Some(why.clone()).filter(|why| !why.is_empty()),
        )),
        EngineLink::Down {
            host,
            why,
            retry_in,
            ever,
            other: None,
        } => {
            let headline = match (ever, retry_in) {
                (_, None) => format!("Perch can't connect to {host}."),
                (true, Some(_)) => format!("Disconnected from {host}. Reconnecting…"),
                (false, Some(_)) => format!("Can't connect to {host}. Trying again…"),
            };
            Some((headline, Some(why.clone()).filter(|why| !why.is_empty())))
        }
    }
}

/// Whether the strip offers Settings: connected to the build the app
/// installs, which found something required missing. Another build's
/// strip says what matters more.
fn needs_setup(link: &EngineLink, checks: &[HostCheck]) -> bool {
    matches!(link, EngineLink::Up { other: None, .. }) && perch_engine::host::needs_setup(checks)
}

/// Whether the reader can ask for a try now.
fn can_retry(link: &EngineLink) -> bool {
    matches!(link, EngineLink::Down { .. })
}

/// Whether the reader can restart the host's engine: an older build left
/// running. A newer one is never stopped.
fn can_restart(link: &EngineLink) -> bool {
    matches!(
        link,
        EngineLink::Up { other: Some(other), .. } | EngineLink::Down { other: Some(other), .. }
            if !other.newer
    )
}

/// The strip, hidden while there is nothing to say.
pub fn banner(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    let said_now = move || {
        state.host_checks.with(|checks| {
            state
                .link
                .with(|link| link.as_ref().and_then(|link| said(link, checks)))
        })
    };
    let setup = move || {
        state.host_checks.with(|checks| {
            state
                .link
                .with(|link| link.as_ref().is_some_and(|link| needs_setup(link, checks)))
        })
    };
    let headline = move || said_now().map(|(headline, _)| headline).unwrap_or_default();
    let why = move || said_now().and_then(|(_, why)| why).unwrap_or_default();
    let has_why = move || said_now().is_some_and(|(_, why)| why.is_some());
    let retry = move || state.link.with(|link| link.as_ref().is_some_and(can_retry));
    let restart = move || {
        state
            .link
            .with(|link| link.as_ref().is_some_and(can_restart))
    };
    let restarter = backend.clone();
    Stack::horizontal((
        sidebar_glyph(SidebarIcon::Warning, theme::notice_fg)
            .style(|s| s.size(16.0, 16.0).flex_shrink(0.0_f32).margin_right(10.0)),
        Stack::vertical((
            Label::derived(headline)
                .debug_name("link-banner-headline")
                .style(|s| {
                    s.font_size(theme::TEXT_META)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::notice_fg())
                        .text_wrap()
                }),
            Label::derived(why)
                .debug_name("link-banner-why")
                .style(move |s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::notice_fg())
                        .text_wrap()
                        .apply_if(!has_why(), |s| s.hide())
                }),
            install_progress(state),
        ))
        .style(|s| s.gap(2.0).min_width(0.0).flex_grow(1.0_f32).flex_basis(0.0)),
        action_button(
            || "Reconnect".to_string(),
            "link-reconnect",
            false,
            || true,
            move || backend.reconnect(),
        )
        .style(move |s| s.margin_left(12.0).apply_if(!retry(), |s| s.hide())),
        action_button(
            || "Restart".to_string(),
            "link-restart",
            false,
            || true,
            move || restarter.restart_engine(),
        )
        .style(move |s| s.margin_left(12.0).apply_if(!restart(), |s| s.hide())),
        action_button(
            || "Open Settings".to_string(),
            "link-settings",
            false,
            || true,
            move || {
                state
                    .settings
                    .section
                    .set(crate::ui::settings::Section::General);
                crate::ui::settings::show(state, None);
            },
        )
        .style(move |s| s.margin_left(12.0).apply_if(!setup(), |s| s.hide())),
    ))
    .debug_name("link-banner")
    .style(move |s| {
        s.items_center()
            .width_full()
            .flex_shrink(0.0_f32)
            .padding_horiz(14.0)
            .padding_vert(8.0)
            .background(theme::notice_bg())
            .apply_if(said_now().is_none(), |s| s.hide())
    })
}

/// Rebuild only when changing between hidden, indeterminate, and measured.
/// Byte updates change the fill's width without restarting its view.
fn install_progress(state: AppState) -> impl IntoView {
    let percent = move || {
        state.link.with(|link| match link {
            Some(EngineLink::Installing { progress, .. }) => {
                progress.as_ref().and_then(|p| p.percent())
            }
            _ => None,
        })
    };
    let mode = floem::reactive::Memo::new(move |_| {
        state.link.with(|link| {
            matches!(link, Some(EngineLink::Installing { .. })).then(|| percent().is_some())
        })
    });
    dyn_container(
        move || mode.get(),
        move |measured| {
            let Some(measured) = measured else {
                return Empty::new().into_any();
            };
            let fill = Empty::new()
                .debug_name("link-install-fill")
                .style(move |s| {
                    s.height_full()
                        .width_pct(if measured {
                            f64::from(percent().unwrap_or(0))
                        } else {
                            100.0
                        })
                        .background(theme::notice_fg())
                        .border_radius(2.0)
                });
            let fill = if measured {
                fill.into_any()
            } else {
                fill.animation(|_| {
                    floem::animate::Animation::new()
                        .keyframe(0, |f| f.style(|s| s.opacity(0.2_f32)))
                        .keyframe(100, |f| f.style(|s| s.opacity(0.7_f32)))
                        .duration(std::time::Duration::from_millis(800))
                        .auto_reverse(true)
                        .repeat(true)
                })
                .into_any()
            };
            Container::new(fill)
                .debug_name("link-install-progress")
                .style(|s| {
                    s.width_full()
                        .height(4.0)
                        .margin_top(4.0)
                        .background(theme::notice_fg().with_alpha(0.15))
                        .border_radius(2.0)
                })
                .into_any()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::reactive::SignalUpdate;
    use std::time::Duration;

    /// What the strip says with no host checks heard.
    fn said_(link: &EngineLink) -> Option<(String, Option<String>)> {
        said(link, &[])
    }

    fn check(what: &str, ok: bool, required: bool) -> HostCheck {
        HostCheck {
            what: what.into(),
            ok,
            required,
            detail: format!("No {what}."),
            fix: None,
        }
    }

    fn up(other: Option<perch_engine::backend::OtherEngine>) -> EngineLink {
        EngineLink::Up {
            host: "halo".into(),
            welcome: perch_engine::wire::Welcome {
                protocol: perch_engine::wire::PROTOCOL,
                engine_version: "0".into(),
                connection: 1,
                host: "halo".into(),
                home: "/home/chris".into(),
                data_root: "/home/chris/.perch".into(),
                clients: 0,
                runs: 0,
                build: "0".into(),
            },
            again: true,
            other,
        }
    }

    /// Connected to a host that lacks something required: the strip says
    /// so, with what is missing and a way to Settings. Something optional
    /// missing says nothing, and another build's strip comes first.
    #[test]
    fn a_host_missing_something_required_needs_setup() {
        let missing = [
            check("git", true, true),
            check("GitHub token", false, true),
            check("Codex", false, false),
            check("logind", false, true),
        ];
        assert_eq!(
            said(&up(None), &missing),
            Some((
                "halo needs setup.".to_string(),
                Some("No GitHub token. No logind.".to_string())
            ))
        );
        assert!(needs_setup(&up(None), &missing));
        let optional = [check("git", true, true), check("Codex", false, false)];
        assert_eq!(said(&up(None), &optional), None);
        assert!(!needs_setup(&up(None), &optional));
        assert!(!needs_setup(&up(Some(older(0))), &missing));
        assert!(said(&up(Some(older(0))), &missing)
            .unwrap()
            .0
            .contains("restart it"));
        let down = EngineLink::Down {
            host: "halo".into(),
            why: String::new(),
            retry_in: Some(Duration::from_secs(1)),
            ever: true,
            other: None,
        };
        assert!(!needs_setup(&down, &missing));

        let root = TestRoot::new();
        let state = AppState::new();
        let (backend, _) = Backend::recording();
        let view = banner(state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 200.0);
        state.link.set(Some(up(None)));
        assert!(!shown(&mut harness, "link-banner"));
        state.host_checks.set(missing.to_vec());
        assert!(shown(&mut harness, "link-settings"));
        assert!(!shown(&mut harness, "link-restart"));
        state.host_checks.set(optional.to_vec());
        assert!(!shown(&mut harness, "link-banner"));
    }

    fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().contains(name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
    }

    fn shown(harness: &mut HeadlessHarness, name: &str) -> bool {
        for _ in 0..3 {
            harness.rebuild();
            harness.paint();
        }
        named(harness.root_id(), name).is_some_and(|id| {
            let r = id.get_visual_rect_no_clip();
            r.width() > 0.0 && r.height() > 0.0
        })
    }

    /// In process there is no strip; on a host it is up while the engine
    /// is not, with Reconnect once a try has failed, and gone once it is
    /// back.
    #[test]
    fn the_strip_is_up_only_while_the_engine_is_not() {
        let root = TestRoot::new();
        let state = AppState::new();
        let (backend, _) = Backend::recording();
        let view = banner(state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 200.0);
        assert!(!shown(&mut harness, "link-banner"));
        state.link.set(Some(EngineLink::Connecting {
            host: "halo".into(),
        }));
        assert!(shown(&mut harness, "link-banner"));
        assert!(!shown(&mut harness, "link-reconnect"));
        state.link.set(Some(EngineLink::Down {
            host: "halo".into(),
            why: "ssh: Could not resolve hostname halo".into(),
            retry_in: Some(Duration::from_secs(1)),
            ever: true,
            other: None,
        }));
        assert!(shown(&mut harness, "link-banner-why"));
        assert!(shown(&mut harness, "link-reconnect"));
        assert!(!shown(&mut harness, "link-restart"));
        state.link.set(Some(up(None)));
        assert!(!shown(&mut harness, "link-banner"));
        // Up to an older engine left running, the strip stays, with a
        // restart and no reconnect.
        state.link.set(Some(up(Some(older(2)))));
        assert!(shown(&mut harness, "link-banner"));
        assert!(shown(&mut harness, "link-restart"));
        assert!(!shown(&mut harness, "link-reconnect"));
    }

    #[test]
    fn installation_bar_tracks_the_phase_and_disappears_on_failure() {
        let root = TestRoot::new();
        let state = AppState::new();
        let (backend, _) = Backend::recording();
        let mut harness =
            HeadlessHarness::new_with_size(root, banner(state, Rc::new(backend)), 800.0, 200.0);
        let installing = |completed, total| EngineLink::Installing {
            host: "halo".into(),
            build: "0.9.0".into(),
            progress: Some(perch_engine::backend::InstallProgress {
                phase: "Uploading perchd".into(),
                completed,
                total,
            }),
        };
        state.link.set(Some(installing(25, Some(100))));
        assert!(shown(&mut harness, "link-install-progress"));
        let width = |name| {
            named(harness.root_id(), name)
                .unwrap()
                .get_visual_rect_no_clip()
                .width()
        };
        assert!((width("link-install-fill") / width("link-install-progress") - 0.25).abs() < 0.01);
        assert_eq!(
            said_(&installing(25, Some(100))).unwrap().1.as_deref(),
            Some("Uploading perchd · 25%")
        );
        state.link.set(Some(installing(75, Some(100))));
        assert!(shown(&mut harness, "link-install-progress"));
        let width = |name| {
            named(harness.root_id(), name)
                .unwrap()
                .get_visual_rect_no_clip()
                .width()
        };
        assert!((width("link-install-fill") / width("link-install-progress") - 0.75).abs() < 0.01);
        // Unknown and zero totals have no misleading percentage.
        for total in [None, Some(0)] {
            state.link.set(Some(installing(0, total)));
            assert!(shown(&mut harness, "link-install-progress"));
            assert_eq!(
                said_(&installing(0, total)).unwrap().1.as_deref(),
                Some("Uploading perchd…")
            );
        }
        state.link.set(Some(EngineLink::Down {
            host: "halo".into(),
            why: "Upload failed".into(),
            retry_in: None,
            ever: false,
            other: None,
        }));
        assert!(shown(&mut harness, "link-reconnect"));
        assert!(!shown(&mut harness, "link-install-progress"));
        state.link.set(Some(up(None)));
        assert!(!shown(&mut harness, "link-banner"));
    }

    fn older(runs: usize) -> perch_engine::backend::OtherEngine {
        perch_engine::backend::OtherEngine {
            engine: "0.8.0".into(),
            wanted: "0.9.0".into(),
            runs,
            clients: 0,
            newer: false,
        }
    }

    #[test]
    fn the_strip_says_what_happened_and_why() {
        let down = |ever, retry_in| EngineLink::Down {
            host: "halo".into(),
            why: "ssh: connect to host halo port 22: Operation timed out".into(),
            retry_in,
            ever,
            other: None,
        };
        assert_eq!(
            said_(&down(true, Some(Duration::from_secs(2)))),
            Some((
                "Disconnected from halo. Reconnecting…".to_string(),
                Some("ssh: connect to host halo port 22: Operation timed out".to_string())
            ))
        );
        assert_eq!(
            said_(&down(false, Some(Duration::from_secs(1)))).unwrap().0,
            "Can't connect to halo. Trying again…"
        );
        // A refused protocol says which side to update, in its reason, and
        // that no more tries are coming.
        assert_eq!(
            said_(&down(true, None)).unwrap().0,
            "Perch can't connect to halo."
        );
        assert_eq!(
            said_(&EngineLink::Connecting {
                host: "halo".into()
            }),
            Some(("Connecting to halo…".to_string(), None))
        );
        assert_eq!(
            said_(&EngineLink::Installing {
                host: "halo".into(),
                build: "0.9.0".into(),
                progress: None,
            }),
            Some(("Installing perchd 0.9.0 on halo…".to_string(), None))
        );
        assert_eq!(
            said_(&EngineLink::Restarting {
                host: "halo".into(),
                build: "0.9.0".into()
            }),
            Some((
                "Restarting the engine on halo to update it to perchd 0.9.0…".to_string(),
                None
            ))
        );
        assert!(can_retry(&down(true, None)));
        assert!(!can_retry(&EngineLink::Connecting {
            host: "halo".into()
        }));

        // Refused by an older engine with runs: what a restart would stop,
        // why there is no connection, and both buttons.
        let refused = EngineLink::Down {
            host: "halo".into(),
            why: "It speaks protocol 4, and this app 5, so Perch can't use it until it restarts."
                .into(),
            retry_in: None,
            ever: false,
            other: Some(older(1)),
        };
        let (headline, why) = said_(&refused).unwrap();
        assert_eq!(
            headline,
            "halo's engine is perchd 0.8.0; restart it to update to 0.9.0 (1 agent run would be stopped)."
        );
        assert!(why.unwrap().contains("protocol 4"));
        assert!(can_restart(&refused) && can_retry(&refused));
        // A newer one is never offered a restart.
        let newer = EngineLink::Down {
            host: "halo".into(),
            why: "Update the app.".into(),
            retry_in: None,
            ever: false,
            other: Some(perch_engine::backend::OtherEngine {
                newer: true,
                ..older(0)
            }),
        };
        assert!(said_(&newer)
            .unwrap()
            .0
            .contains(&format!("Update Perch on {}", perch_engine::host::HERE)));
        assert!(!can_restart(&newer));
    }
}
