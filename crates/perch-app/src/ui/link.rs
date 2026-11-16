//! The strip along the foot of every window while the engine on a host
//! cannot be reached: what happened, why, and a button to try now. It
//! stays until the connection is back, as a toast would not: what the
//! window shows meanwhile is what it had, and nothing done in it goes.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::SignalWith;
use floem::views::{Decorators, Stack};

use crate::backend::{Backend, EngineLink};
use crate::icons::SidebarIcon;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{action_button, sidebar_glyph};

/// What the strip says, and the line under it, for how the connection
/// stands; `None` while it is up, or with the engine in process.
pub fn said(link: &EngineLink) -> Option<(String, Option<String>)> {
    match link {
        EngineLink::Up { .. } => None,
        EngineLink::Connecting { host } => Some((format!("Connecting to {host}…"), None)),
        EngineLink::Down {
            host,
            why,
            retry_in,
            ever,
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

/// Whether the reader can ask for a try now.
fn can_retry(link: &EngineLink) -> bool {
    matches!(link, EngineLink::Down { .. })
}

/// The strip, hidden while there is nothing to say.
pub fn banner(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    let said_now = move || state.link.with(|link| link.as_ref().and_then(said));
    let headline = move || said_now().map(|(headline, _)| headline).unwrap_or_default();
    let why = move || said_now().and_then(|(_, why)| why).unwrap_or_default();
    let has_why = move || said_now().is_some_and(|(_, why)| why.is_some());
    let retry = move || state.link.with(|link| link.as_ref().is_some_and(can_retry));
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

#[cfg(test)]
mod tests {
    use super::*;
    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::reactive::SignalUpdate;
    use std::time::Duration;

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
        }));
        assert!(shown(&mut harness, "link-banner-why"));
        assert!(shown(&mut harness, "link-reconnect"));
        state.link.set(Some(EngineLink::Up {
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
            },
            again: true,
        }));
        assert!(!shown(&mut harness, "link-banner"));
    }

    #[test]
    fn the_strip_says_what_happened_and_why() {
        let down = |ever, retry_in| EngineLink::Down {
            host: "halo".into(),
            why: "ssh: connect to host halo port 22: Operation timed out".into(),
            retry_in,
            ever,
        };
        assert_eq!(
            said(&down(true, Some(Duration::from_secs(2)))),
            Some((
                "Disconnected from halo. Reconnecting…".to_string(),
                Some("ssh: connect to host halo port 22: Operation timed out".to_string())
            ))
        );
        assert_eq!(
            said(&down(false, Some(Duration::from_secs(1)))).unwrap().0,
            "Can't connect to halo. Trying again…"
        );
        // A refused protocol says which side to update, in its reason, and
        // that no more tries are coming.
        assert_eq!(
            said(&down(true, None)).unwrap().0,
            "Perch can't connect to halo."
        );
        assert_eq!(
            said(&EngineLink::Connecting {
                host: "halo".into()
            }),
            Some(("Connecting to halo…".to_string(), None))
        );
        assert!(can_retry(&down(true, None)));
        assert!(!can_retry(&EngineLink::Connecting {
            host: "halo".into()
        }));
    }
}
