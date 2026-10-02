//! The gauge in the bar, and the popover it opens: how much of each agent
//! CLI's plan limits is used, and when each window resets.
//!
//! The gauge sits beside the Agents button, on an item's bar and on the
//! pane with nothing open. It turns amber when a window is past four fifths
//! and red when a CLI is refusing work, so a limit is seen coming without
//! opening anything. The numbers are what the CLIs last said: a poll every
//! five minutes, every run as it goes, and a poll as the popover opens.

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate};
use floem::style::Position;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};
use jiff::Timestamp;
use perch_agent::usage::{Account, Board, Usage, Window};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::review::cards::tip;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{clickable, icon_in};

/// Past this share of a window, it is getting close.
const WARM_PERCENT: f64 = 80.0;

/// How the gauge and a window's bar are coloured.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Tone {
    Calm,
    Warm,
    Hot,
}

impl Tone {
    fn of(percent: f64, limited: bool) -> Self {
        if limited || percent >= 100.0 {
            Tone::Hot
        } else if percent >= WARM_PERCENT {
            Tone::Warm
        } else {
            Tone::Calm
        }
    }

    fn color(self, calm: Color) -> Color {
        match self {
            Tone::Calm => calm,
            Tone::Warm => theme::alert_warning(),
            Tone::Hot => theme::alert_caution(),
        }
    }
}

fn usage_tone(usage: &Usage) -> Tone {
    usage
        .windows
        .iter()
        .map(|w| Tone::of(w.used_percent, false))
        .chain(std::iter::once(Tone::of(0.0, usage.limited)))
        .max()
        .unwrap_or(Tone::Calm)
}

/// The CLIs the reader has, by the name the popover gives them.
fn installed(app: AppState) -> Vec<(&'static str, perch_agent::Backend)> {
    app.agent.with(|agent| {
        let mut found = Vec::new();
        if !agent.models.is_empty() {
            found.push(("Claude", perch_agent::Backend::ClaudeCode));
        }
        if !agent.codex.models.is_empty() {
            found.push(("Codex", perch_agent::Backend::Codex));
        }
        found
    })
}

/// The worst any installed CLI's limits stand.
fn board_tone(app: AppState) -> Tone {
    let backends = installed(app);
    app.usage.with(|board| {
        backends
            .iter()
            .filter_map(|(_, backend)| board.account(*backend).usage.as_ref())
            .map(usage_tone)
            .max()
            .unwrap_or(Tone::Calm)
    })
}

/// The gauge, and the popover under it while it is open.
pub fn button(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let color = move || board_tone(app).color(theme::fg_muted());
    let button = Container::new(
        icon_in(
            move || icons::sidebar_icon(SidebarIcon::Gauge, color()),
            color,
        )
        .style(|s| s.pointer_events_none()),
    );
    let button_id = button.view_id();
    let click_backend = backend.clone();
    let button = button
        .on_event_stop(el::Click, move |_, _| {
            toggle(review, &click_backend, Some(button_id))
        })
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            if matches!(key.code, Code::Enter | Code::Space) {
                toggle(review, &backend, Some(button_id));
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        .style(move |s| {
            clickable(s)
                .size(26.0, 26.0)
                .items_center()
                .justify_center()
                .border_radius(13.0)
                .keyboard_navigable()
                .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
                .hover(|s| s.background(theme::badge_bg()))
                .apply_if(review.usage_open.get(), |s| s.background(theme::badge_bg()))
        })
        .debug_name("usage-button");

    let stack = Stack::new((
        tip(button, "Plan usage".to_string()),
        dyn_container(
            move || review.usage_open.get(),
            move |open| {
                if open {
                    popover(app).into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        )
        // Six pixels under the button, as the Agents dropdown hangs.
        .style(|s| {
            s.position(Position::Absolute)
                .inset_top(32.0)
                .inset_right(0.0)
                .z_index(100)
        }),
    ));
    // A press outside the button and what hangs from it shuts it: see
    // `ui::dismiss_popovers`.
    let anchor = stack.view_id();
    Effect::new(move |_| {
        if review.usage_open.get() {
            review.usage_anchor.set(Some(anchor));
        }
    });
    stack
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            if key.code == Code::Escape
                && key.modifiers.is_empty()
                && review.usage_open.get_untracked()
            {
                close(review, true);
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        // The bar it sits in lets the pointer through everywhere but its
        // controls; this is one.
        .style(|s| {
            s.position(Position::Relative)
                .flex_shrink(0.0_f32)
                .pointer_events_auto()
        })
}

/// Open the popover, or shut it when it is open. `opener` gets the
/// keyboard back when it shuts.
pub fn toggle(review: ReviewState, backend: &Backend, opener: Option<floem::ViewId>) {
    if review.usage_open.get_untracked() {
        close(review, true);
    } else {
        open(review, backend, opener);
    }
}

/// Open the popover over whatever else hangs from the bar, and ask the
/// CLIs again unless they were asked a minute ago.
pub fn open(review: ReviewState, backend: &Backend, opener: Option<floem::ViewId>) {
    crate::review::review_menu::close(review, false);
    crate::ui::agents::dropdown::close(review, false);
    crate::ui::feedback::close(review, false);
    review.usage_opener.set(opener);
    review.usage_open.set(true);
    backend.refresh_usage();
}

/// Shut the popover, giving the keyboard back to what had it when it
/// opened when `restore_focus`.
pub fn close(review: ReviewState, restore_focus: bool) {
    if !review.usage_open.get_untracked() {
        return;
    }
    review.usage_open.set(false);
    let opener = review.usage_opener.get_untracked();
    review.usage_opener.set(None);
    if restore_focus {
        if let Some(opener) = opener {
            opener.request_focus();
        }
    }
}

/// What the popover shows, so it is rebuilt when that changes and not on
/// every write to the board.
#[derive(Clone, PartialEq)]
struct Shown {
    accounts: Vec<(&'static str, Account)>,
    now: Timestamp,
}

fn popover(app: AppState) -> impl IntoView {
    // The clock the ages and resets are read against, kept while the
    // popover is up: the app's own only runs while agents do.
    let now = RwSignal::new(Timestamp::now());
    tick(now);
    let shown = Memo::new(move |_| Shown {
        accounts: {
            let backends = installed(app);
            app.usage.with(|board: &Board| {
                backends
                    .into_iter()
                    .map(|(name, backend)| (name, board.account(backend).clone()))
                    .collect()
            })
        },
        now: now.get(),
    });
    dyn_container(
        move || shown.get(),
        |shown| {
            if shown.accounts.is_empty() {
                return Label::derived(|| "No agent CLI found".to_string())
                    .style(|s| {
                        s.font_size(theme::TEXT_META)
                            .color(theme::fg_subtle())
                            .padding(16.0)
                            .selectable(false)
                    })
                    .into_any();
            }
            let sections = shown
                .accounts
                .into_iter()
                .enumerate()
                .map(|(i, (name, account))| section(name, account, shown.now, i > 0).into_any())
                .collect::<Vec<_>>();
            Stack::vertical_from_iter(sections)
                .style(|s| s.flex_col().width_full())
                .into_any()
        },
    )
    .style(|s| {
        s.width(theme::USAGE_POPOVER_WIDTH)
            .flex_col()
            .apply(crate::ui::menu::panel_surface(floem::style::Style::new()))
    })
    .debug_name("usage-popover")
}

/// Move `now` along every half minute until the popover goes.
fn tick(now: RwSignal<Timestamp>) {
    floem::action::exec_after(std::time::Duration::from_secs(30), move |_| {
        // Gone with the popover: the update finds nothing to update.
        if now.try_update(|now| *now = Timestamp::now()).is_some() {
            tick(now);
        }
    });
}

/// One CLI: its name and plan, each window, what is wrong if anything is,
/// and how old the numbers are.
fn section(name: &'static str, account: Account, now: Timestamp, divided: bool) -> impl IntoView {
    let plan = account
        .usage
        .as_ref()
        .and_then(|u| u.plan.clone())
        .unwrap_or_default();
    let mut rows: Vec<floem::AnyView> = vec![Stack::horizontal((
        heading(name.to_string()),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        heading(plan),
    ))
    .style(|s| s.width_full().items_center())
    .into_any()];
    match &account.usage {
        Some(usage) => {
            for window in &usage.windows {
                rows.push(window_row(window.clone(), now).into_any());
            }
            for (text, color) in notes(usage) {
                rows.push(line(text, color).into_any());
            }
            let mut age = format!(
                "Updated {}",
                crate::timefmt::relative(usage.observed_at, now)
            );
            if account.error.is_some() {
                age.push_str(" · the last check failed");
            }
            rows.push(line(age, theme::fg_subtle).into_any());
        }
        None => {
            let text = account
                .error
                .clone()
                .unwrap_or_else(|| "Checking…".to_string());
            rows.push(line(text, theme::fg_subtle).into_any());
        }
    }
    Stack::vertical_from_iter(rows)
        .style(move |s| {
            s.flex_col()
                .width_full()
                .gap(8.0)
                .padding_horiz(14.0)
                .padding_top(10.0)
                .padding_bottom(12.0)
                .apply_if(divided, |s| {
                    s.border_top(1.0).border_color(theme::border_muted())
                })
        })
        .debug_name(format!("usage-section:{name}"))
}

/// What is worth saying beyond the windows.
fn notes(usage: &Usage) -> Vec<(String, fn() -> Color)> {
    let mut notes: Vec<(String, fn() -> Color)> = Vec::new();
    if usage.limited {
        notes.push(("Limit reached".to_string(), theme::alert_caution));
    }
    if usage.overage {
        notes.push(("Using extra usage".to_string(), theme::alert_warning));
    }
    match usage.free_resets {
        Some(1) => notes.push(("1 free reset available".to_string(), theme::fg_muted)),
        Some(n) if n > 1 => notes.push((format!("{n} free resets available"), theme::fg_muted)),
        _ => {}
    }
    notes
}

/// A window: its name and when it resets, how much is used, and a bar.
fn window_row(window: Window, now: Timestamp) -> impl IntoView {
    let tone = Tone::of(window.used_percent, false);
    let resets = match window.resets_at {
        Some(at) if at > now => format!("resets {}", crate::timefmt::ahead(at, now)),
        Some(_) => "reset since".to_string(),
        None => String::new(),
    };
    let percent = format!("{:.0}%", window.used_percent);
    let fill = window.used_percent.clamp(0.0, 100.0);
    let key = window.key.clone();
    Stack::vertical((
        Stack::horizontal((
            Label::derived(move || window.label.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg())
                    .selectable(false)
                    .flex_shrink(0.0_f32)
            }),
            Label::derived(move || resets.clone()).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .selectable(false)
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_grow(1.0_f32)
            }),
            Label::derived(move || percent.clone()).style(move |s| {
                s.font_size(theme::TEXT_META)
                    .font_weight(theme::weight_emphasis())
                    .color(tone.color(theme::fg()))
                    .selectable(false)
                    .flex_shrink(0.0_f32)
            }),
        ))
        .style(|s| s.width_full().items_center().gap(8.0)),
        Container::new(Empty::new().style(move |s| {
            s.height_full()
                .width_pct(fill)
                .background(tone.color(theme::fg_subtle()))
                .border_radius(2.0)
        }))
        .style(|s| {
            s.width_full()
                .height(4.0)
                .background(theme::badge_bg())
                .border_radius(2.0)
        }),
    ))
    .style(|s| s.flex_col().width_full().gap(5.0))
    .debug_name(format!("usage-window:{key}"))
}

fn heading(text: String) -> impl IntoView {
    Label::derived(move || text.to_uppercase()).style(|s| {
        s.font_size(theme::TEXT_SECTION)
            .font_weight(theme::weight_emphasis())
            .color(theme::fg_subtle())
            .selectable(false)
    })
}

fn line(text: String, color: impl theme::Ink) -> impl IntoView {
    Label::derived(move || text.clone()).style(move |s| {
        s.font_size(theme::TEXT_BADGE)
            .color(color())
            .selectable(false)
            .width_full()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use floem::headless::HeadlessHarness;

    use crate::review::header::tests::{named, pull_request_harness, rect};

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn window(key: &str, label: &str, used: f64) -> Window {
        Window {
            key: key.into(),
            label: label.into(),
            used_percent: used,
            resets_at: Some(Timestamp::now() + jiff::SignedDuration::from_hours(3)),
        }
    }

    fn settle(harness: &mut HeadlessHarness) {
        for _ in 0..2 {
            harness.rebuild();
            harness.paint();
        }
    }

    /// Both CLIs installed: Claude most of the way through its week,
    /// Codex out.
    fn window_with_usage() -> (HeadlessHarness, AppState, ReviewState) {
        let (mut harness, app, review) = pull_request_harness();
        app.agent.update(|agent| {
            agent.models = vec!["sonnet".into()];
            agent.codex.models = vec!["gpt-5".into()];
        });
        app.usage.set(Board {
            claude: Account {
                usage: Some(Usage {
                    plan: None,
                    windows: vec![
                        window("session", "5-hour", 3.0),
                        window("weekly", "Weekly", 50.0),
                        window("weekly:Fable", "Weekly · Fable", 16.0),
                    ],
                    limited: false,
                    overage: false,
                    free_resets: None,
                    observed_at: Timestamp::now(),
                }),
                error: None,
            },
            codex: Account {
                usage: Some(Usage {
                    plan: Some("Pro".into()),
                    windows: vec![window("weekly", "Weekly", 100.0)],
                    limited: true,
                    overage: false,
                    free_resets: Some(1),
                    observed_at: at("2026-09-27T17:00:00Z"),
                }),
                error: Some("Codex took too long to report usage".into()),
            },
        });
        settle(&mut harness);
        (harness, app, review)
    }

    fn open(harness: &mut HeadlessHarness) {
        let at = rect(harness, "usage-button").center();
        harness.click(at.x, at.y);
        settle(harness);
    }

    #[test]
    fn the_gauge_is_as_hot_as_the_worst_limit() {
        let (_harness, app, _review) = window_with_usage();
        assert_eq!(board_tone(app), Tone::Hot);
        app.usage.update(|board| board.codex = Account::default());
        assert_eq!(board_tone(app), Tone::Calm);
        app.usage
            .update(|board| board.claude.usage.as_mut().unwrap().windows[1].used_percent = 95.0);
        assert_eq!(board_tone(app), Tone::Warm);
        // A CLI that is not installed says nothing, whatever it last said.
        app.agent.update(|agent| agent.models.clear());
        assert_eq!(board_tone(app), Tone::Calm);
    }

    #[test]
    fn the_popover_lists_each_clis_windows_under_the_gauge() {
        let (mut harness, _app, review) = window_with_usage();
        let gauge = rect(&harness, "usage-button");
        let agents = rect(&harness, "agents-button");
        assert!(gauge.x1 <= agents.x0, "the gauge is not before Agents");

        open(&mut harness);
        assert!(review.usage_open.get());
        let popover = rect(&harness, "usage-popover");
        assert!(popover.y0 >= gauge.y1);
        let claude = rect(&harness, "usage-section:Claude");
        let codex = rect(&harness, "usage-section:Codex");
        assert!(claude.y1 <= codex.y0, "Claude before Codex");
        for name in ["usage-window:session", "usage-window:weekly:Fable"] {
            assert!(named(harness.root_id(), name).is_some(), "no {name}");
        }
        for text in [
            "\"PRO\"",
            "\"Limit reached\"",
            "\"1 free reset available\"",
            "the last check failed",
        ] {
            assert!(
                named(harness.root_id(), text).is_some(),
                "the popover has no {text}"
            );
        }

        open(&mut harness);
        assert!(!review.usage_open.get(), "a second click did not shut it");
    }

    #[test]
    fn the_popover_and_the_agents_dropdown_shut_each_other() {
        let (mut harness, _app, review) = window_with_usage();
        open(&mut harness);
        let agents = rect(&harness, "agents-button").center();
        harness.click(agents.x, agents.y);
        settle(&mut harness);
        assert!(review.agents_dropdown_open.get());
        assert!(!review.usage_open.get());

        open(&mut harness);
        assert!(review.usage_open.get());
        assert!(!review.agents_dropdown_open.get());
    }

    #[test]
    fn a_press_outside_shuts_the_popover_and_the_agents_dropdown() {
        let (mut harness, _app, review) = window_with_usage();
        open(&mut harness);
        // Inside it stays.
        let popover = rect(&harness, "usage-popover").center();
        harness.click(popover.x, popover.y);
        settle(&mut harness);
        assert!(review.usage_open.get(), "a press in it shut it");

        // Anywhere else, the page under the bar, it goes.
        let page = rect(&harness, "usage-button");
        harness.click(page.x0 - 300.0, page.y1 + 400.0);
        settle(&mut harness);
        assert!(!review.usage_open.get(), "a press outside left it up");

        let agents = rect(&harness, "agents-button").center();
        harness.click(agents.x, agents.y);
        settle(&mut harness);
        assert!(review.agents_dropdown_open.get());
        harness.click(page.x0 - 300.0, page.y1 + 400.0);
        settle(&mut harness);
        assert!(!review.agents_dropdown_open.get(), "a press outside left it up");
    }
}
